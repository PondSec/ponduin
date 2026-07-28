use crate::code_analysis::parser::Parser;
use crate::coding::sensitive::is_sensitive_path;
use crate::coding::workspace::CodingWorkspace;
use ignore::WalkBuilder;
use serde::{Deserialize, Serialize};
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

const MAX_RECORDED_WARNINGS: usize = 20;
const EXCLUDED_DIRECTORIES: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    "node_modules",
    ".venv",
    "venv",
    "dist",
    "build",
    "target",
    ".cache",
    "__pycache__",
    ".pytest_cache",
    ".mypy_cache",
    ".ruff_cache",
    ".next",
    ".nuxt",
    "coverage",
    "vendor",
];

#[derive(Debug)]
pub struct RepositoryIntelligence;

impl RepositoryIntelligence {
    pub fn build(
        workspace: &CodingWorkspace,
        limits: IntelligenceLimits,
    ) -> Result<RepositoryIndex, IntelligenceError> {
        limits.validate()?;
        let mut builder = WalkBuilder::new(workspace.root());
        builder
            .git_ignore(true)
            .git_exclude(true)
            .git_global(false)
            .parents(false)
            .require_git(false)
            .ignore(true)
            .hidden(false)
            .follow_links(false)
            .filter_entry(|entry| entry.depth() == 0 || !is_excluded_directory(entry.path()));

        let parser = Parser::new();
        let mut files = Vec::new();
        let mut symbols = Vec::new();
        let mut imports = Vec::new();
        let mut calls = Vec::new();
        let mut frameworks = BTreeMap::<Framework, BTreeSet<PathBuf>>::new();
        let mut entry_points = BTreeSet::new();
        let mut config_files = BTreeSet::new();
        let mut generated_files = BTreeSet::new();
        let mut warnings = Vec::new();
        let mut scanned_files = 0usize;
        let mut analyzed_bytes = 0usize;
        let mut fingerprint_entries = Vec::new();
        let mut truncated = false;

        for entry in builder.build() {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    record_warning(&mut warnings, error.to_string());
                    continue;
                }
            };
            if !entry
                .file_type()
                .is_some_and(|file_type| file_type.is_file())
            {
                continue;
            }
            if scanned_files == limits.max_files {
                truncated = true;
                break;
            }
            scanned_files += 1;
            let relative = match entry.path().strip_prefix(workspace.root()) {
                Ok(path) => path.to_path_buf(),
                Err(_) => continue,
            };
            if is_sensitive_path(&relative) {
                continue;
            }
            classify_repository_path(
                &relative,
                &mut entry_points,
                &mut config_files,
                &mut generated_files,
            );
            if entry
                .metadata()
                .is_ok_and(|metadata| metadata.len() > limits.max_file_bytes as u64)
            {
                continue;
            }
            let source = match read_utf8_bounded(entry.path(), limits.max_file_bytes) {
                Ok(Some(source)) => source,
                Ok(None) => continue,
                Err(error) => {
                    record_warning(&mut warnings, format!("{}: {error}", relative.display()));
                    continue;
                }
            };
            fingerprint_entries.push((
                relative.clone(),
                crate::coding::file::content_digest(source.as_bytes()),
            ));
            detect_frameworks(&relative, &source, &mut frameworks);
            let Some(analysis) = parser.analyze_file(entry.path(), &source) else {
                continue;
            };
            analyzed_bytes = analyzed_bytes.saturating_add(source.len());
            let symbol_start = symbols.len();
            for symbol in &analysis.functions {
                if symbols.len() == limits.max_symbols {
                    truncated = true;
                    break;
                }
                symbols.push(CodeSymbol {
                    path: relative.clone(),
                    name: symbol.name.clone(),
                    qualified_name: qualify(symbol.parent.as_deref(), &symbol.name),
                    kind: SymbolKind::Function,
                    line: symbol.line,
                    detail: symbol.detail.clone(),
                });
            }
            for symbol in &analysis.classes {
                if symbols.len() == limits.max_symbols {
                    truncated = true;
                    break;
                }
                symbols.push(CodeSymbol {
                    path: relative.clone(),
                    name: symbol.name.clone(),
                    qualified_name: qualify(symbol.parent.as_deref(), &symbol.name),
                    kind: SymbolKind::Type,
                    line: symbol.line,
                    detail: symbol.detail.clone(),
                });
            }
            for import in analysis.imports {
                if imports.len() == limits.max_symbols {
                    truncated = true;
                    break;
                }
                imports.push(CodeImport {
                    path: relative.clone(),
                    module: import.module,
                    occurrences: import.count,
                });
            }
            for call in analysis.calls {
                if calls.len() == limits.max_symbols {
                    truncated = true;
                    break;
                }
                calls.push(CodeCall {
                    path: relative.clone(),
                    caller: call.caller,
                    callee: call.callee,
                    line: call.line,
                });
            }
            files.push(CodeFileMap {
                path: relative,
                language: analysis.language.to_string(),
                lines: analysis.loc,
                symbol_count: symbols.len() - symbol_start,
            });
        }

        files.sort_by(|left, right| left.path.cmp(&right.path));
        symbols.sort_by(|left, right| {
            left.path
                .cmp(&right.path)
                .then(left.line.cmp(&right.line))
                .then(left.name.cmp(&right.name))
        });
        imports.sort_by(|left, right| {
            left.path
                .cmp(&right.path)
                .then(left.module.cmp(&right.module))
        });
        calls.sort_by(|left, right| left.path.cmp(&right.path).then(left.line.cmp(&right.line)));

        Ok(RepositoryIndex {
            files,
            symbols,
            imports,
            calls,
            frameworks: frameworks
                .into_iter()
                .map(|(framework, evidence)| FrameworkDetection {
                    framework,
                    evidence: evidence.into_iter().collect(),
                })
                .collect(),
            entry_points: entry_points.into_iter().collect(),
            config_files: config_files.into_iter().collect(),
            generated_files: generated_files.into_iter().collect(),
            excluded_directory_names: EXCLUDED_DIRECTORIES
                .iter()
                .map(|name| (*name).to_string())
                .collect(),
            scanned_files,
            analyzed_bytes,
            source_fingerprint: fingerprint(fingerprint_entries),
            truncated,
            warnings,
        })
    }

    pub fn fingerprint(
        workspace: &CodingWorkspace,
        limits: IntelligenceLimits,
    ) -> Result<String, IntelligenceError> {
        limits.validate()?;
        let mut builder = WalkBuilder::new(workspace.root());
        builder
            .git_ignore(true)
            .git_exclude(true)
            .git_global(false)
            .parents(false)
            .require_git(false)
            .ignore(true)
            .hidden(false)
            .follow_links(false)
            .filter_entry(|entry| entry.depth() == 0 || !is_excluded_directory(entry.path()));
        let mut entries = Vec::new();
        let mut scanned = 0usize;
        for entry in builder.build().filter_map(Result::ok) {
            if !entry
                .file_type()
                .is_some_and(|file_type| file_type.is_file())
            {
                continue;
            }
            if scanned == limits.max_files {
                break;
            }
            scanned += 1;
            let Ok(relative) = entry.path().strip_prefix(workspace.root()) else {
                continue;
            };
            if is_sensitive_path(relative)
                || entry
                    .metadata()
                    .is_ok_and(|metadata| metadata.len() > limits.max_file_bytes as u64)
            {
                continue;
            }
            if let Ok(Some(source)) = read_utf8_bounded(entry.path(), limits.max_file_bytes) {
                entries.push((
                    relative.to_path_buf(),
                    crate::coding::file::content_digest(source.as_bytes()),
                ));
            }
        }
        Ok(fingerprint(entries))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct IntelligenceLimits {
    pub max_files: usize,
    pub max_file_bytes: usize,
    pub max_symbols: usize,
}

impl Default for IntelligenceLimits {
    fn default() -> Self {
        Self {
            max_files: 20_000,
            max_file_bytes: 2 * 1_024 * 1_024,
            max_symbols: 50_000,
        }
    }
}

impl IntelligenceLimits {
    fn validate(self) -> Result<(), IntelligenceError> {
        if self.max_files == 0 || self.max_files > 100_000 {
            return Err(IntelligenceError::InvalidFileLimit(self.max_files));
        }
        if self.max_file_bytes < 8 * 1_024 || self.max_file_bytes > 10 * 1_024 * 1_024 {
            return Err(IntelligenceError::InvalidFileSizeLimit(self.max_file_bytes));
        }
        if self.max_symbols == 0 || self.max_symbols > 500_000 {
            return Err(IntelligenceError::InvalidSymbolLimit(self.max_symbols));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepositoryIndex {
    pub files: Vec<CodeFileMap>,
    pub symbols: Vec<CodeSymbol>,
    pub imports: Vec<CodeImport>,
    pub calls: Vec<CodeCall>,
    pub frameworks: Vec<FrameworkDetection>,
    pub entry_points: Vec<PathBuf>,
    pub config_files: Vec<PathBuf>,
    pub generated_files: Vec<PathBuf>,
    pub excluded_directory_names: Vec<String>,
    pub scanned_files: usize,
    pub analyzed_bytes: usize,
    pub source_fingerprint: String,
    pub truncated: bool,
    pub warnings: Vec<String>,
}

impl RepositoryIndex {
    pub fn search_symbols(
        &self,
        query: &str,
        exact: bool,
        max_results: usize,
    ) -> Result<SymbolSearchResult, IntelligenceError> {
        if query.is_empty() || max_results == 0 || max_results > 1_000 {
            return Err(IntelligenceError::InvalidSymbolSearch);
        }
        let normalized = query.to_ascii_lowercase();
        let mut matches = self
            .symbols
            .iter()
            .filter(|symbol| {
                if exact {
                    symbol.name == query || symbol.qualified_name == query
                } else {
                    symbol.name.to_ascii_lowercase().contains(&normalized)
                        || symbol
                            .qualified_name
                            .to_ascii_lowercase()
                            .contains(&normalized)
                }
            })
            .cloned()
            .collect::<Vec<_>>();
        let truncated = matches.len() > max_results;
        matches.truncate(max_results);
        Ok(SymbolSearchResult { matches, truncated })
    }

    pub fn references(
        &self,
        symbol: &str,
        max_results: usize,
    ) -> Result<ReferenceSearchResult, IntelligenceError> {
        if symbol.is_empty() || max_results == 0 || max_results > 1_000 {
            return Err(IntelligenceError::InvalidSymbolSearch);
        }
        let mut matches = self
            .calls
            .iter()
            .filter(|call| call.callee == symbol)
            .cloned()
            .collect::<Vec<_>>();
        let truncated = matches.len() > max_results;
        matches.truncate(max_results);
        Ok(ReferenceSearchResult { matches, truncated })
    }

    pub fn context_candidates(
        &self,
        query: &str,
        max_results: usize,
    ) -> Result<Vec<ContextCandidate>, IntelligenceError> {
        if query.trim().is_empty() || max_results == 0 || max_results > 200 {
            return Err(IntelligenceError::InvalidContextRequest);
        }
        let terms = query
            .split(|character: char| !character.is_alphanumeric() && character != '_')
            .filter(|term| term.len() > 1)
            .map(str::to_ascii_lowercase)
            .collect::<BTreeSet<_>>();
        let mut scores = BTreeMap::<PathBuf, (u32, BTreeSet<String>)>::new();
        for file in &self.files {
            score_text(
                &file.path.to_string_lossy(),
                &terms,
                4,
                "path",
                scores.entry(file.path.clone()).or_default(),
            );
        }
        for symbol in &self.symbols {
            score_text(
                &symbol.qualified_name,
                &terms,
                8,
                "symbol",
                scores.entry(symbol.path.clone()).or_default(),
            );
        }
        for import in &self.imports {
            score_text(
                &import.module,
                &terms,
                3,
                "import",
                scores.entry(import.path.clone()).or_default(),
            );
        }
        for call in &self.calls {
            score_text(
                &call.callee,
                &terms,
                5,
                "call",
                scores.entry(call.path.clone()).or_default(),
            );
        }
        for path in &self.entry_points {
            let entry = scores.entry(path.clone()).or_default();
            entry.0 += 1;
            entry.1.insert("entry_point".to_string());
        }

        let mut candidates = scores
            .into_iter()
            .filter(|(_, (score, _))| *score > 0)
            .map(|(path, (score, reasons))| ContextCandidate {
                path,
                score,
                reasons: reasons.into_iter().collect(),
            })
            .collect::<Vec<_>>();
        candidates.sort_by_key(|candidate| (Reverse(candidate.score), candidate.path.clone()));
        candidates.truncate(max_results);
        Ok(candidates)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeFileMap {
    pub path: PathBuf,
    pub language: String,
    pub lines: usize,
    pub symbol_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeSymbol {
    pub path: PathBuf,
    pub name: String,
    pub qualified_name: String,
    pub kind: SymbolKind,
    pub line: usize,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SymbolKind {
    Function,
    Type,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeImport {
    pub path: PathBuf,
    pub module: String,
    pub occurrences: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeCall {
    pub path: PathBuf,
    pub caller: String,
    pub callee: String,
    pub line: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SymbolSearchResult {
    pub matches: Vec<CodeSymbol>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReferenceSearchResult {
    pub matches: Vec<CodeCall>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextCandidate {
    pub path: PathBuf,
    pub score: u32,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrameworkDetection {
    pub framework: Framework,
    pub evidence: Vec<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Framework {
    ActixWeb,
    Angular,
    Axum,
    Django,
    Express,
    FastApi,
    Flask,
    NextJs,
    React,
    Rocket,
    Spring,
    Svelte,
    Tauri,
    Vue,
}

fn read_utf8_bounded(path: &Path, max_bytes: usize) -> io::Result<Option<String>> {
    let file = File::open(path)?;
    let mut bytes = Vec::with_capacity(max_bytes.min(64 * 1_024));
    file.take((max_bytes + 1) as u64).read_to_end(&mut bytes)?;
    if bytes.len() > max_bytes || bytes.contains(&0) {
        return Ok(None);
    }
    Ok(String::from_utf8(bytes).ok())
}

fn is_excluded_directory(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| EXCLUDED_DIRECTORIES.contains(&name))
}

fn classify_repository_path(
    path: &Path,
    entry_points: &mut BTreeSet<PathBuf>,
    config_files: &mut BTreeSet<PathBuf>,
    generated_files: &mut BTreeSet<PathBuf>,
) {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    if matches!(
        name,
        "main.rs"
            | "main.go"
            | "main.py"
            | "app.py"
            | "manage.py"
            | "index.js"
            | "index.ts"
            | "server.js"
            | "server.ts"
            | "Program.cs"
            | "Application.java"
    ) {
        entry_points.insert(path.to_path_buf());
    }
    if matches!(
        name,
        "Cargo.toml"
            | "package.json"
            | "pyproject.toml"
            | "requirements.txt"
            | "go.mod"
            | "pom.xml"
            | "build.gradle"
            | "build.gradle.kts"
            | "Makefile"
            | "CMakeLists.txt"
            | ".editorconfig"
    ) || path
        .components()
        .any(|component| component.as_os_str() == ".github")
    {
        config_files.insert(path.to_path_buf());
    }
    let normalized = name.to_ascii_lowercase();
    if normalized.contains(".generated.")
        || normalized.ends_with(".g.rs")
        || normalized.ends_with(".min.js")
        || normalized.ends_with(".min.css")
        || normalized.ends_with(".designer.cs")
    {
        generated_files.insert(path.to_path_buf());
    }
}

fn detect_frameworks(
    path: &Path,
    source: &str,
    frameworks: &mut BTreeMap<Framework, BTreeSet<PathBuf>>,
) {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    let candidates: &[(Framework, &[&str])] = match name {
        "package.json" => &[
            (Framework::React, &["\"react\""]),
            (Framework::NextJs, &["\"next\""]),
            (Framework::Vue, &["\"vue\""]),
            (Framework::Svelte, &["\"svelte\""]),
            (Framework::Angular, &["\"@angular/core\""]),
            (Framework::Express, &["\"express\""]),
        ],
        "Cargo.toml" => &[
            (Framework::Axum, &["axum"]),
            (Framework::ActixWeb, &["actix-web"]),
            (Framework::Rocket, &["rocket"]),
            (Framework::Tauri, &["tauri"]),
        ],
        "pyproject.toml" | "requirements.txt" => &[
            (Framework::Django, &["django"]),
            (Framework::Flask, &["flask"]),
            (Framework::FastApi, &["fastapi"]),
        ],
        "pom.xml" | "build.gradle" | "build.gradle.kts" => {
            &[(Framework::Spring, &["spring-boot", "org.springframework"])]
        }
        _ => &[],
    };
    if candidates.is_empty() {
        return;
    }
    let lower = source.to_ascii_lowercase();
    for (framework, needles) in candidates {
        if needles.iter().any(|needle| lower.contains(needle)) {
            frameworks
                .entry(*framework)
                .or_default()
                .insert(path.to_path_buf());
        }
    }
}

fn qualify(parent: Option<&str>, name: &str) -> String {
    parent.map_or_else(|| name.to_string(), |parent| format!("{parent}::{name}"))
}

fn score_text(
    text: &str,
    terms: &BTreeSet<String>,
    weight: u32,
    reason: &str,
    score: &mut (u32, BTreeSet<String>),
) {
    let normalized = text.to_ascii_lowercase();
    let hits = terms
        .iter()
        .filter(|term| normalized.contains(term.as_str()))
        .count() as u32;
    if hits > 0 {
        score.0 = score.0.saturating_add(hits.saturating_mul(weight));
        score.1.insert(reason.to_string());
    }
}

fn record_warning(warnings: &mut Vec<String>, warning: String) {
    if warnings.len() < MAX_RECORDED_WARNINGS {
        warnings.push(warning);
    }
}

fn fingerprint(mut entries: Vec<(PathBuf, String)>) -> String {
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    let mut hasher = blake3::Hasher::new();
    for (path, digest) in entries {
        hasher.update(path.to_string_lossy().as_bytes());
        hasher.update(&[0]);
        hasher.update(digest.as_bytes());
        hasher.update(&[0]);
    }
    format!("blake3:{}", hasher.finalize().to_hex())
}

#[derive(Debug, thiserror::Error)]
pub enum IntelligenceError {
    #[error("invalid repository intelligence file limit: {0}")]
    InvalidFileLimit(usize),
    #[error("invalid repository intelligence file-size limit: {0}")]
    InvalidFileSizeLimit(usize),
    #[error("invalid repository intelligence symbol limit: {0}")]
    InvalidSymbolLimit(usize),
    #[error("symbol search requires a query and a result limit from 1 through 1000")]
    InvalidSymbolSearch,
    #[error("context selection requires a query and a result limit from 1 through 200")]
    InvalidContextRequest,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn fixture() -> (tempfile::TempDir, CodingWorkspace) {
        let temp_dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp_dir.path().join("src")).unwrap();
        fs::create_dir_all(temp_dir.path().join("api")).unwrap();
        fs::write(
            temp_dir.path().join("src/lib.rs"),
            "pub struct UserService;\nimpl UserService {\n pub fn load_user() { helper(); }\n}\nfn helper() {}\n",
        )
        .unwrap();
        fs::write(
            temp_dir.path().join("api/main.py"),
            "from fastapi import FastAPI\n\ndef start_api():\n    return FastAPI()\n",
        )
        .unwrap();
        fs::write(
            temp_dir.path().join("package.json"),
            r#"{"dependencies":{"react":"1","next":"1"}}"#,
        )
        .unwrap();
        fs::write(temp_dir.path().join("requirements.txt"), "fastapi==1\n").unwrap();
        let workspace = CodingWorkspace::new(temp_dir.path()).unwrap();
        (temp_dir, workspace)
    }

    #[test]
    fn builds_a_mixed_repository_map_and_symbol_index() {
        let (_temp_dir, workspace) = fixture();

        let index =
            RepositoryIntelligence::build(&workspace, IntelligenceLimits::default()).unwrap();

        assert_eq!(index.files.len(), 2);
        assert!(index
            .symbols
            .iter()
            .any(|symbol| symbol.name == "load_user" && symbol.line == 3));
        assert!(index
            .symbols
            .iter()
            .any(|symbol| symbol.name == "start_api"));
        assert!(index.calls.iter().any(|call| call.callee == "helper"));
        assert!(index
            .frameworks
            .iter()
            .any(|framework| framework.framework == Framework::React));
        assert!(index
            .frameworks
            .iter()
            .any(|framework| framework.framework == Framework::FastApi));
        assert!(index.entry_points.contains(&PathBuf::from("api/main.py")));
        assert_eq!(
            index.source_fingerprint,
            RepositoryIntelligence::fingerprint(&workspace, IntelligenceLimits::default()).unwrap()
        );
    }

    #[test]
    fn searches_symbols_references_and_ranked_context() {
        let (_temp_dir, workspace) = fixture();
        let index =
            RepositoryIntelligence::build(&workspace, IntelligenceLimits::default()).unwrap();

        let symbols = index.search_symbols("user", false, 10).unwrap();
        let references = index.references("helper", 10).unwrap();
        let context = index
            .context_candidates("fix UserService helper", 5)
            .unwrap();

        assert!(symbols
            .matches
            .iter()
            .any(|symbol| symbol.name == "UserService"));
        assert_eq!(references.matches[0].path, Path::new("src/lib.rs"));
        assert_eq!(context[0].path, Path::new("src/lib.rs"));
        assert!(context[0].reasons.contains(&"symbol".to_string()));
    }

    #[test]
    fn skips_irrelevant_sensitive_binary_and_oversized_content() {
        let (temp_dir, workspace) = fixture();
        fs::create_dir(temp_dir.path().join("node_modules")).unwrap();
        fs::write(
            temp_dir.path().join("node_modules/ignored.js"),
            "function ignored() {}",
        )
        .unwrap();
        fs::write(temp_dir.path().join(".env"), "TOKEN=secret").unwrap();
        fs::write(temp_dir.path().join("binary.rs"), b"fn hidden() {}\0").unwrap();
        fs::write(temp_dir.path().join("large.py"), "x".repeat(20_000)).unwrap();

        let index = RepositoryIntelligence::build(
            &workspace,
            IntelligenceLimits {
                max_file_bytes: 8 * 1_024,
                ..IntelligenceLimits::default()
            },
        )
        .unwrap();

        assert!(!index
            .symbols
            .iter()
            .any(|symbol| matches!(symbol.name.as_str(), "ignored" | "hidden")));
        assert!(!index
            .files
            .iter()
            .any(|file| file.path == Path::new("large.py")));
    }

    #[test]
    fn source_fingerprint_changes_when_indexed_content_changes() {
        let (_temp_dir, workspace) = fixture();
        let limits = IntelligenceLimits::default();
        let before = RepositoryIntelligence::fingerprint(&workspace, limits).unwrap();

        fs::write(workspace.root().join("src/lib.rs"), "pub fn changed() {}\n").unwrap();
        let after = RepositoryIntelligence::fingerprint(&workspace, limits).unwrap();

        assert_ne!(before, after);
    }
}
