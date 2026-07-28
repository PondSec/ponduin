use crate::coding::file::{FileReadOptions, FileSnapshot, MAX_READ_LIMIT, MIN_READ_LIMIT};
use crate::coding::intelligence::{ContextCandidate, RepositoryIndex};
use crate::coding::workspace::CodingWorkspace;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use tiktoken_rs::CoreBPE;

const MAX_CONTEXT_FILES: usize = 100;
const MAX_CONTEXT_CHUNKS: usize = 200;
const MAX_CHUNKS_PER_FILE: usize = 3;
const MAX_OMISSIONS: usize = 100;

static CONTEXT_TOKENIZER: OnceLock<Result<CoreBPE, String>> = OnceLock::new();

/// Produces bounded, versioned source chunks from a repository index.
pub struct ContextPlanner<'a> {
    workspace: &'a CodingWorkspace,
    index: &'a RepositoryIndex,
}

impl<'a> ContextPlanner<'a> {
    pub fn new(workspace: &'a CodingWorkspace, index: &'a RepositoryIndex) -> Self {
        Self { workspace, index }
    }

    pub fn prepare(
        &self,
        query: &str,
        limits: ContextLimits,
    ) -> Result<ContextBundle, ContextError> {
        limits.validate()?;
        if query.trim().is_empty() {
            return Err(ContextError::EmptyQuery);
        }
        let tokenizer = context_tokenizer()?;
        let mut candidates = self
            .index
            .context_candidates(query, (limits.max_files * 4).min(200))?;
        append_fallback_candidates(self.index, &mut candidates);

        let generated = self
            .index
            .generated_files
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        let terms = query_terms(query);
        let mut chunks = Vec::new();
        let mut omissions = Vec::new();
        let mut omitted_count = 0usize;
        let mut selected_files = BTreeSet::new();
        let mut used_tokens = 0usize;
        let mut truncated = self.index.truncated;

        for candidate in candidates {
            if selected_files.len() == limits.max_files || chunks.len() == MAX_CONTEXT_CHUNKS {
                truncated = true;
                record_omission(
                    &mut omissions,
                    &mut omitted_count,
                    candidate.path,
                    ContextOmissionReason::FileLimit,
                );
                continue;
            }
            if generated.contains(&candidate.path) {
                record_omission(
                    &mut omissions,
                    &mut omitted_count,
                    candidate.path,
                    ContextOmissionReason::Generated,
                );
                continue;
            }
            let snapshot = match FileSnapshot::read(
                self.workspace,
                &candidate.path,
                FileReadOptions {
                    max_bytes: limits.max_file_bytes,
                    start_line: None,
                    end_line: None,
                },
            ) {
                Ok(snapshot) => snapshot,
                Err(error) => {
                    record_omission(
                        &mut omissions,
                        &mut omitted_count,
                        candidate.path,
                        ContextOmissionReason::Unreadable {
                            detail: error.to_string(),
                        },
                    );
                    continue;
                }
            };
            let lines = split_lines(&snapshot.content);
            if lines.is_empty() {
                continue;
            }
            let ranges = relevant_ranges(
                self.index,
                &candidate.path,
                &terms,
                lines.len(),
                limits.chunk_lines,
                limits.overlap_lines,
            );
            let mut added_for_file = false;
            for range in ranges.into_iter().take(MAX_CHUNKS_PER_FILE) {
                if chunks.len() == MAX_CONTEXT_CHUNKS {
                    truncated = true;
                    break;
                }
                let remaining = limits.token_budget.saturating_sub(used_tokens);
                let Some(chunk) =
                    fit_chunk(tokenizer, &snapshot, &lines, &candidate, range, remaining)
                else {
                    truncated = true;
                    record_omission(
                        &mut omissions,
                        &mut omitted_count,
                        candidate.path.clone(),
                        ContextOmissionReason::TokenBudget,
                    );
                    break;
                };
                used_tokens += chunk.token_count;
                chunks.push(chunk);
                added_for_file = true;
                if used_tokens == limits.token_budget {
                    truncated = true;
                    break;
                }
            }
            if added_for_file {
                selected_files.insert(candidate.path);
            }
            if used_tokens == limits.token_budget {
                break;
            }
        }

        Ok(ContextBundle {
            query: query.to_string(),
            token_budget: limits.token_budget,
            used_tokens,
            selected_files: selected_files.len(),
            chunks,
            omitted_count,
            omissions,
            truncated,
            source_fingerprint: self.index.source_fingerprint.clone(),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextLimits {
    pub token_budget: usize,
    pub max_files: usize,
    pub max_file_bytes: usize,
    pub chunk_lines: usize,
    pub overlap_lines: usize,
}

impl Default for ContextLimits {
    fn default() -> Self {
        Self {
            token_budget: 8_192,
            max_files: 20,
            max_file_bytes: 512 * 1_024,
            chunk_lines: 120,
            overlap_lines: 20,
        }
    }
}

impl ContextLimits {
    fn validate(self) -> Result<(), ContextError> {
        if !(128..=1_000_000).contains(&self.token_budget) {
            return Err(ContextError::InvalidTokenBudget(self.token_budget));
        }
        if self.max_files == 0 || self.max_files > MAX_CONTEXT_FILES {
            return Err(ContextError::InvalidFileLimit(self.max_files));
        }
        if !(MIN_READ_LIMIT..=MAX_READ_LIMIT).contains(&self.max_file_bytes) {
            return Err(ContextError::InvalidFileSizeLimit(self.max_file_bytes));
        }
        if !(10..=400).contains(&self.chunk_lines) {
            return Err(ContextError::InvalidChunkLines(self.chunk_lines));
        }
        if self.overlap_lines >= self.chunk_lines {
            return Err(ContextError::InvalidOverlap {
                overlap: self.overlap_lines,
                chunk: self.chunk_lines,
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextBundle {
    pub query: String,
    pub token_budget: usize,
    pub used_tokens: usize,
    pub selected_files: usize,
    pub chunks: Vec<ContextChunk>,
    pub omitted_count: usize,
    pub omissions: Vec<ContextOmission>,
    pub truncated: bool,
    pub source_fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextChunk {
    pub path: PathBuf,
    pub digest: String,
    pub start_line: usize,
    pub end_line: usize,
    pub total_lines: usize,
    pub content: String,
    pub score: u32,
    pub reasons: Vec<String>,
    pub token_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextOmission {
    pub path: PathBuf,
    pub reason: ContextOmissionReason,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ContextOmissionReason {
    Generated,
    Unreadable { detail: String },
    TokenBudget,
    FileLimit,
}

#[derive(Debug, Clone, Copy)]
struct LineRange {
    start: usize,
    end: usize,
    anchor: usize,
}

fn context_tokenizer() -> Result<&'static CoreBPE, ContextError> {
    CONTEXT_TOKENIZER
        .get_or_init(|| tiktoken_rs::o200k_base().map_err(|error| error.to_string()))
        .as_ref()
        .map_err(|error| ContextError::Tokenizer(error.clone()))
}

fn append_fallback_candidates(index: &RepositoryIndex, candidates: &mut Vec<ContextCandidate>) {
    let mut seen = candidates
        .iter()
        .map(|candidate| candidate.path.clone())
        .collect::<BTreeSet<_>>();
    for path in index
        .entry_points
        .iter()
        .chain(index.config_files.iter())
        .chain(index.files.iter().map(|file| &file.path))
    {
        if seen.insert(path.clone()) {
            candidates.push(ContextCandidate {
                path: path.clone(),
                score: 0,
                reasons: vec!["fallback".to_string()],
            });
        }
    }
}

fn relevant_ranges(
    index: &RepositoryIndex,
    path: &Path,
    terms: &BTreeSet<String>,
    total_lines: usize,
    chunk_lines: usize,
    overlap_lines: usize,
) -> Vec<LineRange> {
    let mut anchors = index
        .symbols
        .iter()
        .filter(|symbol| symbol.path == path && matches_terms(&symbol.qualified_name, terms))
        .map(|symbol| symbol.line)
        .chain(
            index
                .calls
                .iter()
                .filter(|call| {
                    call.path == path
                        && (matches_terms(&call.callee, terms)
                            || matches_terms(&call.caller, terms))
                })
                .map(|call| call.line),
        )
        .collect::<Vec<_>>();
    if anchors.is_empty() {
        anchors.push(1);
    }
    anchors.sort_unstable();
    anchors.dedup();

    let mut ranges = Vec::new();
    for anchor in anchors {
        let anchor = anchor.clamp(1, total_lines);
        let half = chunk_lines / 2;
        let mut start = anchor.saturating_sub(half).max(1);
        let end = (start + chunk_lines - 1).min(total_lines);
        start = end.saturating_sub(chunk_lines - 1).max(1);
        if ranges
            .last()
            .is_some_and(|previous: &LineRange| start <= previous.end + overlap_lines)
        {
            continue;
        }
        ranges.push(LineRange { start, end, anchor });
        if ranges.len() == MAX_CHUNKS_PER_FILE {
            break;
        }
    }
    ranges
}

fn fit_chunk(
    tokenizer: &CoreBPE,
    snapshot: &FileSnapshot,
    lines: &[&str],
    candidate: &ContextCandidate,
    mut range: LineRange,
    remaining_tokens: usize,
) -> Option<ContextChunk> {
    if remaining_tokens == 0 {
        return None;
    }
    loop {
        let content = lines.get(range.start - 1..range.end)?.concat();
        let header = format!(
            "{}:{}-{} score={} reasons={}\n",
            snapshot.path.display(),
            range.start,
            range.end,
            candidate.score,
            candidate.reasons.join(",")
        );
        let token_count = tokenizer
            .encode_with_special_tokens(&(header + content.as_str()))
            .len();
        if token_count <= remaining_tokens {
            return Some(ContextChunk {
                path: snapshot.path.clone(),
                digest: snapshot.digest.clone(),
                start_line: range.start,
                end_line: range.end,
                total_lines: snapshot.total_lines,
                content,
                score: candidate.score,
                reasons: candidate.reasons.clone(),
                token_count,
            });
        }
        if range.start == range.end {
            return None;
        }
        let lines_before_anchor = range.anchor.saturating_sub(range.start);
        let lines_after_anchor = range.end.saturating_sub(range.anchor);
        if lines_after_anchor > lines_before_anchor {
            range.end -= 1;
        } else {
            range.start += 1;
        }
    }
}

fn query_terms(query: &str) -> BTreeSet<String> {
    query
        .split(|character: char| !character.is_alphanumeric() && character != '_')
        .filter(|term| term.len() > 1)
        .map(str::to_ascii_lowercase)
        .collect()
}

fn matches_terms(value: &str, terms: &BTreeSet<String>) -> bool {
    let normalized = value.to_ascii_lowercase();
    terms.iter().any(|term| normalized.contains(term))
}

fn split_lines(content: &str) -> Vec<&str> {
    if content.is_empty() {
        Vec::new()
    } else {
        content.split_inclusive('\n').collect()
    }
}

fn record_omission(
    omissions: &mut Vec<ContextOmission>,
    omitted_count: &mut usize,
    path: PathBuf,
    reason: ContextOmissionReason,
) {
    *omitted_count += 1;
    if omissions.len() < MAX_OMISSIONS {
        omissions.push(ContextOmission { path, reason });
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ContextError {
    #[error("context query cannot be empty")]
    EmptyQuery,
    #[error("context token budget must be between 128 and 1000000, got {0}")]
    InvalidTokenBudget(usize),
    #[error("context file limit must be between 1 and {MAX_CONTEXT_FILES}, got {0}")]
    InvalidFileLimit(usize),
    #[error(
        "context file byte limit must be between {MIN_READ_LIMIT} and {MAX_READ_LIMIT}, got {0}"
    )]
    InvalidFileSizeLimit(usize),
    #[error("context chunk lines must be between 10 and 400, got {0}")]
    InvalidChunkLines(usize),
    #[error("context overlap {overlap} must be smaller than chunk size {chunk}")]
    InvalidOverlap { overlap: usize, chunk: usize },
    #[error("context tokenizer initialization failed: {0}")]
    Tokenizer(String),
    #[error(transparent)]
    Intelligence(#[from] crate::coding::intelligence::IntelligenceError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coding::intelligence::{IntelligenceLimits, RepositoryIntelligence};
    use std::fs;

    #[test]
    fn ranks_versioned_symbol_chunks_within_an_exact_token_budget() {
        let temp_dir = tempfile::tempdir().unwrap();
        fs::create_dir(temp_dir.path().join("src")).unwrap();
        let mut source = String::new();
        for line in 1..=180 {
            if line == 90 {
                source.push_str("pub fn process_invoice() { validate_invoice(); }\n");
            } else {
                source.push_str(&format!("// filler line {line}\n"));
            }
        }
        fs::write(temp_dir.path().join("src/billing.rs"), source).unwrap();
        fs::write(
            temp_dir.path().join("src/unrelated.rs"),
            "pub fn unrelated() {}\n",
        )
        .unwrap();
        let workspace = CodingWorkspace::new(temp_dir.path()).unwrap();
        let index =
            RepositoryIntelligence::build(&workspace, IntelligenceLimits::default()).unwrap();

        let bundle = ContextPlanner::new(&workspace, &index)
            .prepare(
                "fix process invoice validation",
                ContextLimits {
                    token_budget: 160,
                    max_files: 2,
                    chunk_lines: 40,
                    overlap_lines: 5,
                    ..ContextLimits::default()
                },
            )
            .unwrap();

        assert!(bundle.used_tokens <= bundle.token_budget);
        assert!(!bundle.chunks.is_empty());
        assert_eq!(bundle.chunks[0].path, PathBuf::from("src/billing.rs"));
        assert!(bundle.chunks[0].start_line <= 90);
        assert!(bundle.chunks[0].end_line >= 90);
        assert!(bundle.chunks[0].content.contains("process_invoice"));
        assert!(bundle.chunks[0].digest.starts_with("blake3:"));
        assert_eq!(
            bundle.used_tokens,
            bundle
                .chunks
                .iter()
                .map(|chunk| chunk.token_count)
                .sum::<usize>()
        );
    }

    #[test]
    fn omits_generated_files_and_reports_hard_limits() {
        let temp_dir = tempfile::tempdir().unwrap();
        fs::write(
            temp_dir.path().join("client.generated.rs"),
            "pub fn generated_target() {}\n",
        )
        .unwrap();
        fs::write(
            temp_dir.path().join("lib.rs"),
            "pub fn handwritten_target() {}\n",
        )
        .unwrap();
        let workspace = CodingWorkspace::new(temp_dir.path()).unwrap();
        let index =
            RepositoryIntelligence::build(&workspace, IntelligenceLimits::default()).unwrap();

        let bundle = ContextPlanner::new(&workspace, &index)
            .prepare("target", ContextLimits::default())
            .unwrap();

        assert!(bundle
            .chunks
            .iter()
            .all(|chunk| chunk.path != Path::new("client.generated.rs")));
        assert!(bundle.omissions.iter().any(|omission| {
            omission.path == Path::new("client.generated.rs")
                && omission.reason == ContextOmissionReason::Generated
        }));
        assert!(ContextPlanner::new(&workspace, &index)
            .prepare(
                "target",
                ContextLimits {
                    token_budget: 127,
                    ..ContextLimits::default()
                }
            )
            .is_err());
    }
}
