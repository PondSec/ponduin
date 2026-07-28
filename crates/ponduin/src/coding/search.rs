use crate::coding::sensitive::is_sensitive_path;
use crate::coding::workspace::{CodingWorkspace, WorkspaceError};
use globset::{Glob, GlobSet, GlobSetBuilder};
use ignore::WalkBuilder;
use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

const MAX_RECORDED_WARNINGS: usize = 20;
const BINARY_PROBE_BYTES: usize = 8 * 1_024;

#[derive(Debug)]
pub struct RepositorySearch<'workspace> {
    workspace: &'workspace CodingWorkspace,
}

impl<'workspace> RepositorySearch<'workspace> {
    pub fn new(workspace: &'workspace CodingWorkspace) -> Self {
        Self { workspace }
    }

    pub fn find_files(
        &self,
        query: &str,
        scope: impl AsRef<Path>,
        limits: SearchLimits,
    ) -> Result<FileSearchResult, SearchError> {
        limits.validate()?;
        if query.is_empty() {
            return Err(SearchError::EmptyQuery);
        }

        let query = query.to_ascii_lowercase();
        let mut state = WalkState::default();
        let mut matches = Vec::new();
        self.walk_files(scope.as_ref(), limits.max_files, &mut state, |path| {
            if matches.len() == limits.max_results {
                return WalkControl::Stop;
            }
            let relative = self.relative(path);
            if relative
                .to_string_lossy()
                .to_ascii_lowercase()
                .contains(&query)
            {
                matches.push(relative);
            }
            WalkControl::Continue
        })?;
        matches.sort();

        Ok(FileSearchResult {
            matches,
            scanned_files: state.scanned_files,
            truncated: state.truncated,
            warnings: state.warnings,
        })
    }

    pub fn search_text(
        &self,
        request: &TextSearchRequest,
        limits: SearchLimits,
    ) -> Result<TextSearchResult, SearchError> {
        limits.validate()?;
        if request.pattern.is_empty() {
            return Err(SearchError::EmptyQuery);
        }
        let matcher = TextMatcher::new(&request.pattern, request.regex, request.case_sensitive)?;
        let include = compile_globs(&request.include)?;
        let mut state = WalkState::default();
        let mut matches = Vec::new();
        let mut skipped_binary = 0;
        let mut skipped_large = 0;
        let mut skipped_sensitive = 0;
        let mut read_warnings = Vec::new();

        self.walk_files(&request.scope, limits.max_files, &mut state, |path| {
            if matches.len() == limits.max_results {
                return WalkControl::Stop;
            }
            let relative = self.relative(path);
            if include
                .as_ref()
                .is_some_and(|patterns| !patterns.is_match(&relative))
            {
                return WalkControl::Continue;
            }
            if is_sensitive_path(&relative) {
                skipped_sensitive += 1;
                return WalkControl::Continue;
            }

            match read_bounded_text(path, limits.max_file_bytes) {
                Ok(BoundedText::Text(content)) => {
                    for (line_index, line) in content.lines().enumerate() {
                        let Some((start, end)) = matcher.find(line) else {
                            continue;
                        };
                        matches.push(TextMatch {
                            path: relative.clone(),
                            line: line_index + 1,
                            column: start + 1,
                            match_text: line.get(start..end).unwrap_or_default().to_string(),
                            line_text: truncate_line(line, limits.max_line_bytes),
                        });
                        if matches.len() == limits.max_results {
                            return WalkControl::Stop;
                        }
                    }
                }
                Ok(BoundedText::Binary) => skipped_binary += 1,
                Ok(BoundedText::TooLarge) => skipped_large += 1,
                Err(error) if read_warnings.len() < MAX_RECORDED_WARNINGS => {
                    read_warnings.push(format!("could not read {}: {error}", relative.display()))
                }
                Err(_) => {}
            }
            WalkControl::Continue
        })?;
        for warning in read_warnings {
            state.record_warning(warning);
        }
        matches.sort_by(|left, right| {
            (&left.path, left.line, left.column).cmp(&(&right.path, right.line, right.column))
        });

        Ok(TextSearchResult {
            matches,
            scanned_files: state.scanned_files,
            skipped_binary,
            skipped_large,
            skipped_sensitive,
            truncated: state.truncated,
            warnings: state.warnings,
        })
    }

    fn walk_files(
        &self,
        scope: &Path,
        max_files: usize,
        state: &mut WalkState,
        mut visit: impl FnMut(&Path) -> WalkControl,
    ) -> Result<(), SearchError> {
        let scope = self.workspace.resolve_existing(scope)?;
        if scope.is_file() {
            state.scanned_files = 1;
            let _ = visit(&scope);
            return Ok(());
        }
        if !scope.is_dir() {
            return Err(SearchError::InvalidScope(scope));
        }

        let root = self.workspace.root();
        let mut builder = WalkBuilder::new(root);
        builder
            .git_ignore(true)
            .git_exclude(true)
            .git_global(false)
            .parents(false)
            .require_git(false)
            .ignore(true)
            .hidden(true)
            .follow_links(false);
        let filter_root = root.to_path_buf();
        let filter_scope = scope.clone();
        builder.filter_entry(move |entry| {
            let path = entry.path();
            path == filter_root || filter_scope.starts_with(path) || path.starts_with(&filter_scope)
        });

        for entry in builder.build() {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    state.record_warning(error.to_string());
                    continue;
                }
            };
            let path = entry.path();
            if path != scope && !path.starts_with(&scope) {
                continue;
            }
            if !entry
                .file_type()
                .is_some_and(|file_type| file_type.is_file())
            {
                continue;
            }
            if state.scanned_files == max_files {
                state.truncated = true;
                break;
            }
            state.scanned_files += 1;
            if visit(path) == WalkControl::Stop {
                state.truncated = true;
                break;
            }
        }
        Ok(())
    }

    fn relative(&self, path: &Path) -> PathBuf {
        path.strip_prefix(self.workspace.root())
            .unwrap_or(path)
            .to_path_buf()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WalkControl {
    Continue,
    Stop,
}

#[derive(Debug, Default)]
struct WalkState {
    scanned_files: usize,
    truncated: bool,
    warnings: Vec<String>,
}

impl WalkState {
    fn record_warning(&mut self, warning: String) {
        if self.warnings.len() < MAX_RECORDED_WARNINGS {
            self.warnings.push(warning);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchLimits {
    pub max_results: usize,
    pub max_files: usize,
    pub max_file_bytes: usize,
    pub max_line_bytes: usize,
}

impl Default for SearchLimits {
    fn default() -> Self {
        Self {
            max_results: 200,
            max_files: 50_000,
            max_file_bytes: 2 * 1_024 * 1_024,
            max_line_bytes: 4 * 1_024,
        }
    }
}

impl SearchLimits {
    fn validate(self) -> Result<(), SearchError> {
        validate_limit("max_results", self.max_results, 1, 1_000)?;
        validate_limit("max_files", self.max_files, 1, 100_000)?;
        validate_limit(
            "max_file_bytes",
            self.max_file_bytes,
            BINARY_PROBE_BYTES,
            10 * 1_024 * 1_024,
        )?;
        validate_limit("max_line_bytes", self.max_line_bytes, 128, 64 * 1_024)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextSearchRequest {
    pub pattern: String,
    #[serde(default = "default_scope")]
    pub scope: PathBuf,
    #[serde(default)]
    pub regex: bool,
    #[serde(default)]
    pub case_sensitive: bool,
    #[serde(default)]
    pub include: Vec<String>,
}

fn default_scope() -> PathBuf {
    PathBuf::from(".")
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileSearchResult {
    pub matches: Vec<PathBuf>,
    pub scanned_files: usize,
    pub truncated: bool,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextSearchResult {
    pub matches: Vec<TextMatch>,
    pub scanned_files: usize,
    pub skipped_binary: usize,
    pub skipped_large: usize,
    pub skipped_sensitive: usize,
    pub truncated: bool,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextMatch {
    pub path: PathBuf,
    pub line: usize,
    pub column: usize,
    pub match_text: String,
    pub line_text: String,
}

struct TextMatcher(Regex);

impl TextMatcher {
    fn new(pattern: &str, regex: bool, case_sensitive: bool) -> Result<Self, SearchError> {
        let expression = if regex {
            pattern.to_string()
        } else {
            regex::escape(pattern)
        };
        RegexBuilder::new(&expression)
            .case_insensitive(!case_sensitive)
            .size_limit(2 * 1_024 * 1_024)
            .build()
            .map(Self)
            .map_err(|error| SearchError::InvalidRegex(error.to_string()))
    }

    fn find(&self, line: &str) -> Option<(usize, usize)> {
        self.0.find(line).map(|found| (found.start(), found.end()))
    }
}

enum BoundedText {
    Text(String),
    Binary,
    TooLarge,
}

fn read_bounded_text(path: &Path, max_bytes: usize) -> io::Result<BoundedText> {
    let mut file = File::open(path)?;
    let mut bytes = Vec::with_capacity(max_bytes.min(64 * 1_024));
    file.by_ref()
        .take((max_bytes + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > max_bytes {
        return Ok(BoundedText::TooLarge);
    }
    if bytes.iter().take(BINARY_PROBE_BYTES).any(|byte| *byte == 0) {
        return Ok(BoundedText::Binary);
    }
    match String::from_utf8(bytes) {
        Ok(content) => Ok(BoundedText::Text(content)),
        Err(_) => Ok(BoundedText::Binary),
    }
}

fn compile_globs(patterns: &[String]) -> Result<Option<GlobSet>, SearchError> {
    if patterns.is_empty() {
        return Ok(None);
    }
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        let glob = Glob::new(pattern)
            .map_err(|error| SearchError::InvalidGlob(pattern.clone(), error.to_string()))?;
        builder.add(glob);
    }
    builder
        .build()
        .map(Some)
        .map_err(|error| SearchError::InvalidGlobSet(error.to_string()))
}

fn truncate_line(line: &str, max_bytes: usize) -> String {
    if line.len() <= max_bytes {
        return line.to_string();
    }
    let mut end = max_bytes;
    while !line.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", line.get(..end).unwrap_or_default())
}

fn validate_limit(
    name: &'static str,
    value: usize,
    minimum: usize,
    maximum: usize,
) -> Result<(), SearchError> {
    if (minimum..=maximum).contains(&value) {
        Ok(())
    } else {
        Err(SearchError::InvalidLimit {
            name,
            value,
            minimum,
            maximum,
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SearchError {
    #[error(transparent)]
    Workspace(#[from] WorkspaceError),
    #[error("search query must not be empty")]
    EmptyQuery,
    #[error("search scope must be a file or directory: {0}")]
    InvalidScope(PathBuf),
    #[error("invalid regular expression: {0}")]
    InvalidRegex(String),
    #[error("invalid include glob `{0}`: {1}")]
    InvalidGlob(String, String),
    #[error("could not compile include globs: {0}")]
    InvalidGlobSet(String),
    #[error("{name} is {value}, expected {minimum} through {maximum}")]
    InvalidLimit {
        name: &'static str,
        value: usize,
        minimum: usize,
        maximum: usize,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn search_fixture() -> (tempfile::TempDir, CodingWorkspace) {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path().join("repository");
        fs::create_dir_all(root.join("src/nested")).unwrap();
        fs::create_dir_all(root.join("ignored")).unwrap();
        fs::create_dir(root.join(".git")).unwrap();
        fs::write(root.join(".gitignore"), "ignored/\n").unwrap();
        fs::write(
            root.join("src/lib.rs"),
            "pub fn greeting() {\n    println!(\"Hello World\");\n}\n",
        )
        .unwrap();
        fs::write(
            root.join("src/nested/service.py"),
            "def greeting():\n    return 'hello api'\n",
        )
        .unwrap();
        fs::write(root.join("ignored/secret.py"), "SECRET_MATCH = True").unwrap();
        let workspace = CodingWorkspace::new(&root).unwrap();
        (temp_dir, workspace)
    }

    #[test]
    fn finds_files_and_text_across_languages() {
        let (_temp_dir, workspace) = search_fixture();
        let search = RepositorySearch::new(&workspace);

        let files = search
            .find_files("service", ".", SearchLimits::default())
            .unwrap();
        let text = search
            .search_text(
                &TextSearchRequest {
                    pattern: "hello".to_string(),
                    scope: PathBuf::from("."),
                    regex: false,
                    case_sensitive: false,
                    include: vec!["**/*.{rs,py}".to_string()],
                },
                SearchLimits::default(),
            )
            .unwrap();

        assert_eq!(files.matches, vec![PathBuf::from("src/nested/service.py")]);
        assert_eq!(text.matches.len(), 2);
        assert_eq!(text.matches[0].path, PathBuf::from("src/lib.rs"));
        assert_eq!(text.matches[0].line, 2);
        assert_eq!(text.matches[1].path, PathBuf::from("src/nested/service.py"));
    }

    #[test]
    fn supports_regex_and_scoped_search() {
        let (_temp_dir, workspace) = search_fixture();
        let search = RepositorySearch::new(&workspace);

        let result = search
            .search_text(
                &TextSearchRequest {
                    pattern: r"(?m)^def\s+\w+".to_string(),
                    scope: PathBuf::from("src/nested"),
                    regex: true,
                    case_sensitive: true,
                    include: Vec::new(),
                },
                SearchLimits::default(),
            )
            .unwrap();

        assert_eq!(result.matches.len(), 1);
        assert_eq!(result.matches[0].match_text, "def greeting");
    }

    #[test]
    fn skips_ignored_binary_large_and_sensitive_files() {
        let (_temp_dir, workspace) = search_fixture();
        fs::write(workspace.root().join("binary.dat"), b"match\0binary").unwrap();
        fs::write(workspace.root().join("large.txt"), "match".repeat(3_000)).unwrap();
        fs::write(workspace.root().join(".env"), "match=secret").unwrap();
        let search = RepositorySearch::new(&workspace);
        let limits = SearchLimits {
            max_file_bytes: BINARY_PROBE_BYTES,
            ..SearchLimits::default()
        };

        let result = search
            .search_text(
                &TextSearchRequest {
                    pattern: "match".to_string(),
                    scope: PathBuf::from("."),
                    regex: false,
                    case_sensitive: false,
                    include: Vec::new(),
                },
                limits,
            )
            .unwrap();

        assert!(result.matches.is_empty());
        assert_eq!(result.skipped_binary, 1);
        assert_eq!(result.skipped_large, 1);
        assert_eq!(result.skipped_sensitive, 0);
        assert!(result
            .warnings
            .iter()
            .all(|warning| !warning.contains("secret")));
    }

    #[test]
    fn explicit_sensitive_file_scope_is_still_skipped() {
        let (_temp_dir, workspace) = search_fixture();
        fs::write(workspace.root().join(".env"), "TOKEN=secret").unwrap();
        let search = RepositorySearch::new(&workspace);

        let result = search
            .search_text(
                &TextSearchRequest {
                    pattern: "secret".to_string(),
                    scope: PathBuf::from(".env"),
                    regex: false,
                    case_sensitive: false,
                    include: Vec::new(),
                },
                SearchLimits::default(),
            )
            .unwrap();

        assert!(result.matches.is_empty());
        assert_eq!(result.skipped_sensitive, 1);
    }

    #[cfg(unix)]
    #[test]
    fn never_follows_external_symlinks() {
        use std::os::unix::fs::symlink;

        let (temp_dir, workspace) = search_fixture();
        let outside = temp_dir.path().join("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("external.rs"), "EXTERNAL_MATCH").unwrap();
        symlink(&outside, workspace.root().join("linked")).unwrap();
        let search = RepositorySearch::new(&workspace);

        let result = search
            .search_text(
                &TextSearchRequest {
                    pattern: "EXTERNAL_MATCH".to_string(),
                    scope: PathBuf::from("."),
                    regex: false,
                    case_sensitive: true,
                    include: Vec::new(),
                },
                SearchLimits::default(),
            )
            .unwrap();

        assert!(result.matches.is_empty());
    }

    #[test]
    fn result_limit_is_reported_as_truncation() {
        let (_temp_dir, workspace) = search_fixture();
        let search = RepositorySearch::new(&workspace);
        let limits = SearchLimits {
            max_results: 1,
            ..SearchLimits::default()
        };

        let result = search
            .search_text(
                &TextSearchRequest {
                    pattern: "greeting".to_string(),
                    scope: PathBuf::from("."),
                    regex: false,
                    case_sensitive: true,
                    include: Vec::new(),
                },
                limits,
            )
            .unwrap();

        assert_eq!(result.matches.len(), 1);
        assert!(result.truncated);
    }

    #[test]
    fn rejects_invalid_regex_and_limits() {
        let (_temp_dir, workspace) = search_fixture();
        let search = RepositorySearch::new(&workspace);
        let request = TextSearchRequest {
            pattern: "(".to_string(),
            scope: PathBuf::from("."),
            regex: true,
            case_sensitive: true,
            include: Vec::new(),
        };

        assert!(matches!(
            search.search_text(&request, SearchLimits::default()),
            Err(SearchError::InvalidRegex(_))
        ));
        assert!(matches!(
            search.find_files(
                "src",
                ".",
                SearchLimits {
                    max_results: 0,
                    ..SearchLimits::default()
                }
            ),
            Err(SearchError::InvalidLimit {
                name: "max_results",
                ..
            })
        ));
    }
}
