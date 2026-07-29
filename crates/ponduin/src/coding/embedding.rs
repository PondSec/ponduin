use crate::coding::intelligence::{ContextCandidate, IntelligenceError, RepositoryIndex};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

const EMBEDDING_DIMENSIONS: usize = 512;
const MAX_FEATURES_PER_DOCUMENT: usize = EMBEDDING_DIMENSIONS;
const MAX_TOKEN_CHARS: usize = 128;
const MAX_NGRAMS_PER_TOKEN: usize = 32;
const MIN_SIMILARITY: f32 = 0.05;
const MAX_CONTEXT_RESULTS: usize = 200;

/// A compact, deterministic feature-hashing index for optional local retrieval.
///
/// It embeds repository metadata that has already been extracted by the
/// Tree-sitter index. It does not reread complete source files, invoke a model,
/// contact a provider, or persist repository content.
#[derive(Debug, Clone)]
pub struct LocalEmbeddingIndex {
    documents: Vec<EmbeddedDocument>,
}

impl LocalEmbeddingIndex {
    pub fn build(index: &RepositoryIndex) -> Self {
        let mut accumulators = index
            .files
            .iter()
            .map(|file| {
                let mut accumulator = FeatureAccumulator::default();
                accumulator.add_text(&file.path.to_string_lossy(), 3.0);
                accumulator.add_text(&file.language, 1.0);
                (file.path.clone(), accumulator)
            })
            .collect::<BTreeMap<_, _>>();

        for symbol in &index.symbols {
            if let Some(accumulator) = accumulators.get_mut(&symbol.path) {
                accumulator.add_text(&symbol.qualified_name, 4.0);
                if let Some(detail) = &symbol.detail {
                    accumulator.add_text(detail, 1.0);
                }
            }
        }
        for import in &index.imports {
            if let Some(accumulator) = accumulators.get_mut(&import.path) {
                accumulator.add_text(&import.module, 2.0);
            }
        }
        for call in &index.calls {
            if let Some(accumulator) = accumulators.get_mut(&call.path) {
                accumulator.add_text(&call.caller, 1.0);
                accumulator.add_text(&call.callee, 2.0);
            }
        }
        for path in &index.entry_points {
            if let Some(accumulator) = accumulators.get_mut(path) {
                accumulator.add_feature("role:entry_point", 2.0);
            }
        }
        for path in &index.config_files {
            if let Some(accumulator) = accumulators.get_mut(path) {
                accumulator.add_feature("role:configuration", 2.0);
            }
        }
        for framework in &index.frameworks {
            let framework_name = format!("{:?}", framework.framework);
            for path in &framework.evidence {
                if let Some(accumulator) = accumulators.get_mut(path) {
                    accumulator.add_text(&framework_name, 2.0);
                }
            }
        }

        let documents = accumulators
            .into_iter()
            .filter_map(|(path, accumulator)| {
                accumulator
                    .finish()
                    .map(|embedding| EmbeddedDocument { path, embedding })
            })
            .collect();
        Self { documents }
    }

    pub fn rank(
        &self,
        query: &str,
        max_results: usize,
    ) -> Result<Vec<EmbeddingMatch>, EmbeddingError> {
        if query.trim().is_empty() {
            return Err(EmbeddingError::EmptyQuery);
        }
        if max_results == 0 || max_results > MAX_CONTEXT_RESULTS {
            return Err(EmbeddingError::InvalidResultLimit(max_results));
        }
        let mut query_features = FeatureAccumulator::default();
        query_features.add_text(query, 1.0);
        let Some(query_embedding) = query_features.finish() else {
            return Ok(Vec::new());
        };

        let mut matches = self
            .documents
            .iter()
            .filter_map(|document| {
                let similarity = document.embedding.similarity(&query_embedding);
                (similarity >= MIN_SIMILARITY).then(|| EmbeddingMatch {
                    path: document.path.clone(),
                    similarity,
                })
            })
            .collect::<Vec<_>>();
        matches.sort_by(|left, right| {
            right
                .similarity
                .partial_cmp(&left.similarity)
                .unwrap_or(Ordering::Equal)
                .then(left.path.cmp(&right.path))
        });
        matches.truncate(max_results);
        Ok(matches)
    }
}

/// Combines exact lexical evidence with the optional local feature embedding.
///
/// Lexical scores remain intact. Embeddings contribute a bounded bonus and can
/// add metadata-related files that exact substring matching would miss.
pub fn hybrid_context_candidates(
    index: &RepositoryIndex,
    query: &str,
    max_results: usize,
) -> Result<Vec<ContextCandidate>, EmbeddingError> {
    if max_results == 0 || max_results > MAX_CONTEXT_RESULTS {
        return Err(EmbeddingError::InvalidResultLimit(max_results));
    }
    let pool_limit = max_results.saturating_mul(4).min(MAX_CONTEXT_RESULTS);
    let lexical = index.context_candidates(query, pool_limit)?;
    let embedded = LocalEmbeddingIndex::build(index).rank(query, pool_limit)?;
    let mut combined = BTreeMap::<PathBuf, (u32, BTreeSet<String>)>::new();

    for candidate in lexical {
        let entry = combined.entry(candidate.path).or_default();
        entry.0 = entry.0.saturating_add(candidate.score);
        entry.1.extend(candidate.reasons);
    }
    for candidate in embedded {
        let entry = combined.entry(candidate.path).or_default();
        let bonus = (candidate.similarity * 20.0).ceil().clamp(1.0, 20.0) as u32;
        entry.0 = entry.0.saturating_add(bonus);
        entry.1.insert("local_embedding".to_string());
    }

    let mut candidates = combined
        .into_iter()
        .map(|(path, (score, reasons))| ContextCandidate {
            path,
            score,
            reasons: reasons.into_iter().collect(),
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then(left.path.cmp(&right.path))
    });
    candidates.truncate(max_results);
    Ok(candidates)
}

#[derive(Debug, Clone)]
struct EmbeddedDocument {
    path: PathBuf,
    embedding: SparseEmbedding,
}

#[derive(Debug, Clone)]
struct SparseEmbedding {
    features: Vec<(u16, f32)>,
}

impl SparseEmbedding {
    fn similarity(&self, other: &Self) -> f32 {
        let mut left = 0;
        let mut right = 0;
        let mut dot = 0.0;
        while left < self.features.len() && right < other.features.len() {
            match self.features[left].0.cmp(&other.features[right].0) {
                Ordering::Less => left += 1,
                Ordering::Greater => right += 1,
                Ordering::Equal => {
                    dot += self.features[left].1 * other.features[right].1;
                    left += 1;
                    right += 1;
                }
            }
        }
        dot.clamp(-1.0, 1.0)
    }
}

#[derive(Debug, Default)]
struct FeatureAccumulator {
    features: BTreeMap<u16, f32>,
}

impl FeatureAccumulator {
    fn add_text(&mut self, value: &str, weight: f32) {
        let tokens = tokenize(value);
        for token in &tokens {
            self.add_feature(&format!("word:{token}"), weight);
            if let Some(concept) = concept_for(token) {
                self.add_feature(&format!("concept:{concept}"), weight);
            }
            for ngram in character_ngrams(token) {
                self.add_feature(&format!("ngram:{ngram}"), weight * 0.35);
            }
        }
        for pair in tokens.windows(2) {
            self.add_feature(&format!("pair:{}:{}", pair[0], pair[1]), weight * 0.5);
        }
    }

    fn add_feature(&mut self, feature: &str, weight: f32) {
        let (slot, sign) = feature_slot(feature);
        *self.features.entry(slot).or_default() += weight * sign;
    }

    fn finish(mut self) -> Option<SparseEmbedding> {
        self.features
            .retain(|_, weight| weight.abs() > f32::EPSILON);
        if self.features.is_empty() {
            return None;
        }
        if self.features.len() > MAX_FEATURES_PER_DOCUMENT {
            let mut strongest = self.features.into_iter().collect::<Vec<_>>();
            strongest.sort_by(|left, right| {
                right
                    .1
                    .abs()
                    .partial_cmp(&left.1.abs())
                    .unwrap_or(Ordering::Equal)
                    .then(left.0.cmp(&right.0))
            });
            strongest.truncate(MAX_FEATURES_PER_DOCUMENT);
            self.features = strongest.into_iter().collect();
        }
        let norm = self
            .features
            .values()
            .map(|weight| weight * weight)
            .sum::<f32>()
            .sqrt();
        if norm <= f32::EPSILON {
            return None;
        }
        Some(SparseEmbedding {
            features: self
                .features
                .into_iter()
                .map(|(slot, weight)| (slot, weight / norm))
                .collect(),
        })
    }
}

fn tokenize(value: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut previous_was_lowercase = false;
    for character in value.chars() {
        if character.is_alphanumeric() {
            let camel_boundary =
                character.is_uppercase() && previous_was_lowercase && !current.is_empty();
            if camel_boundary {
                push_token(&mut tokens, &mut current);
            }
            for lowercase in character.to_lowercase() {
                if current.chars().count() < MAX_TOKEN_CHARS {
                    current.push(lowercase);
                }
            }
            previous_was_lowercase = character.is_lowercase();
        } else {
            push_token(&mut tokens, &mut current);
            previous_was_lowercase = false;
        }
    }
    push_token(&mut tokens, &mut current);
    tokens
}

fn push_token(tokens: &mut Vec<String>, current: &mut String) {
    if !current.is_empty() {
        tokens.push(std::mem::take(current));
    }
}

fn character_ngrams(token: &str) -> Vec<String> {
    let characters = token.chars().collect::<Vec<_>>();
    if characters.len() < 3 {
        return Vec::new();
    }
    characters
        .windows(3)
        .take(MAX_NGRAMS_PER_TOKEN)
        .map(|window| window.iter().collect())
        .collect()
}

fn concept_for(token: &str) -> Option<&'static str> {
    match token {
        "auth" | "authenticate" | "authentication" | "credential" | "credentials" | "login"
        | "signin" => Some("authentication"),
        "bug" | "debug" | "error" | "errors" | "fail" | "failed" | "failure" | "exception" => {
            Some("failure")
        }
        "test" | "tests" | "spec" | "specs" | "validate" | "validation" | "verify" => {
            Some("verification")
        }
        "config" | "configuration" | "option" | "options" | "setting" | "settings" => {
            Some("configuration")
        }
        "database" | "db" | "persistence" | "query" | "sql" | "storage" => Some("database"),
        "api" | "endpoint" | "http" | "route" | "router" => Some("endpoint"),
        "account" | "profile" | "user" | "users" => Some("user"),
        "bill" | "billing" | "invoice" | "payment" | "payments" => Some("billing"),
        _ => None,
    }
}

fn feature_slot(feature: &str) -> (u16, f32) {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in feature.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    let slot = (hash % EMBEDDING_DIMENSIONS as u64) as u16;
    let sign = if hash & (1 << 63) == 0 { 1.0 } else { -1.0 };
    (slot, sign)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EmbeddingMatch {
    pub path: PathBuf,
    pub similarity: f32,
}

#[derive(Debug, thiserror::Error)]
pub enum EmbeddingError {
    #[error("local embedding query cannot be empty")]
    EmptyQuery,
    #[error("local embedding result limit must be between 1 and {MAX_CONTEXT_RESULTS}, got {0}")]
    InvalidResultLimit(usize),
    #[error(transparent)]
    Intelligence(#[from] IntelligenceError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coding::intelligence::{IntelligenceLimits, RepositoryIntelligence};
    use crate::coding::workspace::CodingWorkspace;
    use std::fs;

    fn fixture() -> (tempfile::TempDir, RepositoryIndex) {
        let temp_dir = tempfile::tempdir().unwrap();
        fs::write(
            temp_dir.path().join("identity.rs"),
            "pub fn authenticate_user(credentials: Credentials) -> bool { validate(credentials) }\n",
        )
        .unwrap();
        fs::write(
            temp_dir.path().join("invoice.rs"),
            "pub fn calculate_payment(invoice: Invoice) -> Money { total(invoice) }\n",
        )
        .unwrap();
        fs::write(
            temp_dir.path().join("unrelated.rs"),
            "pub fn render_widget() -> String { String::new() }\n",
        )
        .unwrap();
        let workspace = CodingWorkspace::new(temp_dir.path()).unwrap();
        let index =
            RepositoryIntelligence::build(&workspace, IntelligenceLimits::default()).unwrap();
        (temp_dir, index)
    }

    #[test]
    fn ranks_related_metadata_without_reading_source_again() {
        let (_temp_dir, index) = fixture();
        let matches = LocalEmbeddingIndex::build(&index)
            .rank("login credential checks", 3)
            .unwrap();

        assert_eq!(matches[0].path, PathBuf::from("identity.rs"));
        assert!(matches[0].similarity > 0.0);
    }

    #[test]
    fn hybrid_ranking_retains_lexical_evidence_and_marks_embedding_evidence() {
        let (_temp_dir, index) = fixture();
        let candidates =
            hybrid_context_candidates(&index, "billing invoice validation", 3).unwrap();

        assert_eq!(candidates[0].path, PathBuf::from("invoice.rs"));
        assert!(candidates[0]
            .reasons
            .contains(&"local_embedding".to_string()));
        assert!(candidates[0]
            .reasons
            .iter()
            .any(|reason| reason != "local_embedding"));
    }

    #[test]
    fn ranking_is_bounded_and_deterministic() {
        let (_temp_dir, index) = fixture();
        let embeddings = LocalEmbeddingIndex::build(&index);
        let first = embeddings.rank("user authentication", 2).unwrap();
        let second = embeddings.rank("user authentication", 2).unwrap();

        assert_eq!(first, second);
        assert!(first.len() <= 2);
        assert!(matches!(
            embeddings.rank("query", 0),
            Err(EmbeddingError::InvalidResultLimit(0))
        ));
    }
}
