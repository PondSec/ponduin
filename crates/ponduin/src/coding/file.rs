use crate::coding::outcome::ActionFailureKind;
use crate::coding::sensitive::is_sensitive_path;
use crate::coding::workspace::{CodingWorkspace, WorkspaceError};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

pub const DEFAULT_READ_LIMIT: usize = 512 * 1_024;
pub const MIN_READ_LIMIT: usize = 1_024;
pub const MAX_READ_LIMIT: usize = 2 * 1_024 * 1_024;

/// UTF-8 file content tied to a digest of the complete file bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileSnapshot {
    pub path: PathBuf,
    pub digest: String,
    pub size_bytes: usize,
    pub total_lines: usize,
    pub start_line: Option<usize>,
    pub end_line: Option<usize>,
    pub content: String,
}

impl FileSnapshot {
    pub fn read(
        workspace: &CodingWorkspace,
        path: impl AsRef<Path>,
        options: FileReadOptions,
    ) -> Result<Self, FileReadError> {
        options.validate()?;
        let resolved = workspace.resolve_existing(path)?;
        if !resolved.is_file() {
            return Err(FileReadError::NotFile(resolved));
        }
        let relative = resolved
            .strip_prefix(workspace.root())
            .map(Path::to_path_buf)
            .map_err(|_| FileReadError::OutsideWorkspace(resolved.clone()))?;
        if is_sensitive_path(&relative) {
            return Err(FileReadError::Sensitive(relative));
        }

        let bytes = read_bounded(&resolved, options.max_bytes)?;
        let size_bytes = bytes.len();
        if bytes.contains(&0) {
            return Err(FileReadError::Binary(relative));
        }
        let complete_content =
            String::from_utf8(bytes).map_err(|_| FileReadError::Binary(relative.clone()))?;
        let digest = content_digest(complete_content.as_bytes());
        let lines = split_lines_preserving_endings(&complete_content);
        let total_lines = lines.len();
        let (content, start_line, end_line) =
            select_lines(&lines, options.start_line, options.end_line)?;

        Ok(Self {
            path: relative,
            digest,
            size_bytes,
            total_lines,
            start_line,
            end_line,
            content,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileReadOptions {
    pub max_bytes: usize,
    pub start_line: Option<usize>,
    pub end_line: Option<usize>,
}

impl Default for FileReadOptions {
    fn default() -> Self {
        Self {
            max_bytes: DEFAULT_READ_LIMIT,
            start_line: None,
            end_line: None,
        }
    }
}

impl FileReadOptions {
    fn validate(self) -> Result<(), FileReadError> {
        if !(MIN_READ_LIMIT..=MAX_READ_LIMIT).contains(&self.max_bytes) {
            return Err(FileReadError::InvalidLimit {
                value: self.max_bytes,
            });
        }
        if self.start_line == Some(0) || self.end_line == Some(0) {
            return Err(FileReadError::InvalidLineRange {
                start: self.start_line,
                end: self.end_line,
                total: None,
            });
        }
        if let (Some(start), Some(end)) = (self.start_line, self.end_line) {
            if start > end {
                return Err(FileReadError::InvalidLineRange {
                    start: self.start_line,
                    end: self.end_line,
                    total: None,
                });
            }
        }
        Ok(())
    }
}

pub fn content_digest(bytes: &[u8]) -> String {
    format!("blake3:{}", blake3::hash(bytes).to_hex())
}

fn read_bounded(path: &Path, max_bytes: usize) -> Result<Vec<u8>, FileReadError> {
    let mut file = File::open(path).map_err(|source| FileReadError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mut bytes = Vec::with_capacity(max_bytes.min(64 * 1_024));
    file.by_ref()
        .take((max_bytes + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|source| FileReadError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    if bytes.len() > max_bytes {
        Err(FileReadError::TooLarge {
            path: path.to_path_buf(),
            limit: max_bytes,
        })
    } else {
        Ok(bytes)
    }
}

fn split_lines_preserving_endings(content: &str) -> Vec<&str> {
    if content.is_empty() {
        Vec::new()
    } else {
        content.split_inclusive('\n').collect()
    }
}

fn select_lines(
    lines: &[&str],
    requested_start: Option<usize>,
    requested_end: Option<usize>,
) -> Result<(String, Option<usize>, Option<usize>), FileReadError> {
    if requested_start.is_none() && requested_end.is_none() {
        return Ok((lines.concat(), None, None));
    }

    let start = requested_start.unwrap_or(1);
    let end = requested_end.unwrap_or(lines.len());
    if lines.is_empty() || start == 0 || start > end || end > lines.len() {
        return Err(FileReadError::InvalidLineRange {
            start: requested_start,
            end: requested_end,
            total: Some(lines.len()),
        });
    }

    Ok((lines[start - 1..end].concat(), Some(start), Some(end)))
}

#[derive(Debug, thiserror::Error)]
pub enum FileReadError {
    #[error(transparent)]
    Workspace(#[from] WorkspaceError),
    #[error("resolved file path is outside the workspace: {0}")]
    OutsideWorkspace(PathBuf),
    #[error("path is not a regular file: {0}")]
    NotFile(PathBuf),
    #[error("sensitive file contents are unavailable to the coding agent: {0}")]
    Sensitive(PathBuf),
    #[error("binary or non-UTF-8 file contents are unavailable: {0}")]
    Binary(PathBuf),
    #[error("file exceeds the {limit}-byte read limit: {path}")]
    TooLarge { path: PathBuf, limit: usize },
    #[error("read limit is {value}, expected {MIN_READ_LIMIT} through {MAX_READ_LIMIT}")]
    InvalidLimit { value: usize },
    #[error("invalid line range {start:?} through {end:?} for {total:?} total lines")]
    InvalidLineRange {
        start: Option<usize>,
        end: Option<usize>,
        total: Option<usize>,
    },
    #[error("could not read {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

impl FileReadError {
    pub(crate) fn failure_kind(&self) -> ActionFailureKind {
        match self {
            Self::NotFile(_) => ActionFailureKind::ResourceMissing,
            Self::Io { source, .. } if source.kind() == io::ErrorKind::NotFound => {
                ActionFailureKind::ResourceMissing
            }
            Self::Workspace(error) => error.failure_kind(),
            Self::OutsideWorkspace(_) | Self::Sensitive(_) => ActionFailureKind::PolicyBlocked,
            Self::Io { .. } => ActionFailureKind::TransientFailure,
            Self::Binary(_) | Self::TooLarge { .. } => ActionFailureKind::CapabilityUnavailable,
            Self::InvalidLimit { .. } | Self::InvalidLineRange { .. } => {
                ActionFailureKind::InvalidArguments
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn reads_complete_utf8_content_with_stable_digest() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("lib.rs");
        fs::write(&path, "fn value() {\n    println!(\"✓\");\n}\n").unwrap();
        let workspace = CodingWorkspace::new(temp_dir.path()).unwrap();

        let first = FileSnapshot::read(&workspace, "lib.rs", FileReadOptions::default()).unwrap();
        let second = FileSnapshot::read(&workspace, "lib.rs", FileReadOptions::default()).unwrap();

        assert_eq!(first, second);
        assert_eq!(first.path, PathBuf::from("lib.rs"));
        assert_eq!(first.total_lines, 3);
        assert!(first.digest.starts_with("blake3:"));
        assert_eq!(first.digest.len(), "blake3:".len() + 64);
        assert_eq!(first.content, "fn value() {\n    println!(\"✓\");\n}\n");
    }

    #[test]
    fn digest_covers_the_complete_file_when_returning_a_line_range() {
        let temp_dir = tempfile::tempdir().unwrap();
        fs::write(temp_dir.path().join("app.py"), "one\ntwo\nthree\n").unwrap();
        let workspace = CodingWorkspace::new(temp_dir.path()).unwrap();

        let complete =
            FileSnapshot::read(&workspace, "app.py", FileReadOptions::default()).unwrap();
        let range = FileSnapshot::read(
            &workspace,
            "app.py",
            FileReadOptions {
                start_line: Some(2),
                end_line: Some(2),
                ..FileReadOptions::default()
            },
        )
        .unwrap();

        assert_eq!(complete.digest, range.digest);
        assert_eq!(range.content, "two\n");
        assert_eq!(range.start_line, Some(2));
        assert_eq!(range.end_line, Some(2));
        assert_eq!(range.total_lines, 3);
    }

    #[test]
    fn digest_changes_with_file_content() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("file.txt");
        fs::write(&path, "before").unwrap();
        let workspace = CodingWorkspace::new(temp_dir.path()).unwrap();
        let before =
            FileSnapshot::read(&workspace, "file.txt", FileReadOptions::default()).unwrap();
        fs::write(&path, "after").unwrap();

        let after = FileSnapshot::read(&workspace, "file.txt", FileReadOptions::default()).unwrap();

        assert_ne!(before.digest, after.digest);
    }

    #[test]
    fn rejects_sensitive_binary_oversized_and_external_files() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path().join("workspace");
        fs::create_dir(&root).unwrap();
        fs::write(root.join(".env"), "TOKEN=secret").unwrap();
        fs::write(root.join("binary.dat"), b"text\0binary").unwrap();
        fs::write(root.join("large.txt"), "x".repeat(MIN_READ_LIMIT + 1)).unwrap();
        fs::write(temp_dir.path().join("outside.txt"), "outside").unwrap();
        let workspace = CodingWorkspace::new(&root).unwrap();

        assert!(matches!(
            FileSnapshot::read(&workspace, ".env", FileReadOptions::default()),
            Err(FileReadError::Sensitive(_))
        ));
        assert!(matches!(
            FileSnapshot::read(&workspace, "binary.dat", FileReadOptions::default()),
            Err(FileReadError::Binary(_))
        ));
        assert!(matches!(
            FileSnapshot::read(
                &workspace,
                "large.txt",
                FileReadOptions {
                    max_bytes: MIN_READ_LIMIT,
                    ..FileReadOptions::default()
                }
            ),
            Err(FileReadError::TooLarge { .. })
        ));
        assert!(matches!(
            FileSnapshot::read(
                &workspace,
                temp_dir.path().join("outside.txt"),
                FileReadOptions::default()
            ),
            Err(FileReadError::Workspace(WorkspaceError::OutsideWorkspace(
                _
            )))
        ));
    }

    #[test]
    fn validates_line_ranges_and_limits() {
        let temp_dir = tempfile::tempdir().unwrap();
        fs::write(temp_dir.path().join("file.txt"), "one\ntwo").unwrap();
        let workspace = CodingWorkspace::new(temp_dir.path()).unwrap();

        assert!(matches!(
            FileSnapshot::read(
                &workspace,
                "file.txt",
                FileReadOptions {
                    start_line: Some(2),
                    end_line: Some(3),
                    ..FileReadOptions::default()
                }
            ),
            Err(FileReadError::InvalidLineRange { total: Some(2), .. })
        ));
        assert!(matches!(
            FileSnapshot::read(
                &workspace,
                "file.txt",
                FileReadOptions {
                    max_bytes: MIN_READ_LIMIT - 1,
                    ..FileReadOptions::default()
                }
            ),
            Err(FileReadError::InvalidLimit { .. })
        ));
    }
}
