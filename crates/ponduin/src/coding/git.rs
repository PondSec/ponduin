use crate::coding::process::{ProcessLimits, ProcessOutput, ProcessRunner};
use crate::coding::sensitive::is_sensitive_path;
use crate::coding::workspace::{CodingWorkspace, WorkspaceError};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

const MAX_DIFF_PATHS: usize = 200;
const MAX_DIFF_CONTEXT: u32 = 1_000;
const MAX_HISTORY_ENTRIES: usize = 100;
const MAX_FILTER_OVERRIDES: usize = 100;

#[derive(Debug)]
pub struct GitRepository<'workspace> {
    workspace: &'workspace CodingWorkspace,
    relative_root: PathBuf,
    limits: GitLimits,
}

impl<'workspace> GitRepository<'workspace> {
    pub async fn open(
        workspace: &'workspace CodingWorkspace,
        limits: GitLimits,
    ) -> Result<Self, GitError> {
        limits.validate()?;
        let runner = ProcessRunner::new(
            workspace,
            ProcessLimits {
                timeout: limits.timeout,
                output_limit: limits.output_limit,
            },
        );
        let output = runner
            .run_git(
                vec!["rev-parse".to_string(), "--show-toplevel".to_string()],
                PathBuf::from("."),
            )
            .await?;
        require_success(&output, "discover repository root")?;
        let reported_root = PathBuf::from(output.stdout.trim());
        let root = workspace.resolve_existing(&reported_root)?;
        if !root.is_dir() {
            return Err(GitError::RepositoryRootNotDirectory(root));
        }
        let relative_root = root
            .strip_prefix(workspace.root())
            .map(Path::to_path_buf)
            .map_err(|_| GitError::RepositoryOutsideWorkspace(root.clone()))?;

        Ok(Self {
            workspace,
            relative_root: nonempty_relative(relative_root),
            limits,
        })
    }

    pub async fn status(&self) -> Result<GitStatus, GitError> {
        let output = self
            .run_with_filters_disabled([
                "status",
                "--porcelain=v1",
                "-z",
                "--branch",
                "--untracked-files=all",
                "--ignore-submodules=all",
            ])
            .await?;
        require_success(&output, "read repository status")?;
        let mut records = output.stdout.split('\0');
        let _branch_header = records.next();
        let mut changes = Vec::new();
        let mut truncated = output.output_truncated;

        while let Some(record) = records.next() {
            if record.is_empty() {
                continue;
            }
            if changes.len() == self.limits.max_status_entries {
                truncated = true;
                break;
            }
            if record.len() < 3 {
                if output.output_truncated {
                    truncated = true;
                    break;
                }
                return Err(GitError::MalformedOutput("status entry"));
            }
            let bytes = record.as_bytes();
            let index_status = char::from(bytes[0]);
            let worktree_status = char::from(bytes[1]);
            let path = PathBuf::from(
                record
                    .get(3..)
                    .ok_or(GitError::MalformedOutput("status path"))?,
            );
            let original_path = if matches!(index_status, 'R' | 'C') {
                records
                    .next()
                    .filter(|path| !path.is_empty())
                    .map(PathBuf::from)
            } else {
                None
            };
            changes.push(GitChange {
                untracked: index_status == '?' && worktree_status == '?',
                conflict: is_conflict(index_status, worktree_status),
                index_status,
                worktree_status,
                path,
                original_path,
            });
        }

        let branch = self
            .optional_stdout(["symbolic-ref", "--quiet", "--short", "HEAD"])
            .await?;
        let head_oid = self
            .optional_stdout(["rev-parse", "--verify", "HEAD"])
            .await?;
        let upstream = self
            .optional_stdout([
                "rev-parse",
                "--abbrev-ref",
                "--symbolic-full-name",
                "@{upstream}",
            ])
            .await?;
        let (ahead, behind) = if upstream.is_some() {
            self.ahead_behind().await?
        } else {
            (0, 0)
        };
        let detached = head_oid.is_some() && branch.is_none();

        Ok(GitStatus {
            branch,
            detached,
            unborn: head_oid.is_none(),
            head_oid,
            upstream,
            ahead,
            behind,
            changes,
            truncated,
            lossy_output: output.stdout_lossy,
        })
    }

    pub async fn diff(&self, request: GitDiffRequest) -> Result<GitDiff, GitError> {
        if request.context_lines > MAX_DIFF_CONTEXT {
            return Err(GitError::InvalidDiffContext(request.context_lines));
        }
        if request.paths.len() > MAX_DIFF_PATHS {
            return Err(GitError::TooManyPaths {
                count: request.paths.len(),
                limit: MAX_DIFF_PATHS,
            });
        }

        let candidates = if request.paths.is_empty() {
            self.changed_paths(request.staged).await?
        } else {
            request
                .paths
                .into_iter()
                .map(validate_relative_path)
                .collect::<Result<Vec<_>, _>>()?
        };
        let mut safe_paths = BTreeSet::new();
        let mut skipped_sensitive = BTreeSet::new();
        for path in candidates {
            if is_sensitive_path(&path) {
                skipped_sensitive.insert(path);
            } else {
                safe_paths.insert(path);
            }
        }

        if safe_paths.is_empty() {
            return Ok(GitDiff {
                staged: request.staged,
                files: Vec::new(),
                skipped_sensitive: skipped_sensitive.into_iter().collect(),
                patch: String::new(),
                truncated: false,
                lossy_output: false,
            });
        }

        let mut args = vec![
            "diff".to_string(),
            "--no-ext-diff".to_string(),
            "--no-textconv".to_string(),
            "--ignore-submodules=all".to_string(),
            format!("--unified={}", request.context_lines),
        ];
        if request.staged {
            args.push("--cached".to_string());
        }
        args.push("--".to_string());
        args.extend(safe_paths.iter().map(|path| literal_pathspec(path)));
        let output = self.run_with_filters_disabled(args).await?;
        require_success(&output, "read repository diff")?;

        Ok(GitDiff {
            staged: request.staged,
            files: safe_paths.into_iter().collect(),
            skipped_sensitive: skipped_sensitive.into_iter().collect(),
            patch: output.stdout,
            truncated: output.output_truncated,
            lossy_output: output.stdout_lossy,
        })
    }

    pub async fn history(&self, max_entries: usize) -> Result<GitHistory, GitError> {
        if max_entries == 0 || max_entries > MAX_HISTORY_ENTRIES {
            return Err(GitError::InvalidHistoryLimit(max_entries));
        }
        let output = self
            .run(vec![
                "log".to_string(),
                "-n".to_string(),
                max_entries.to_string(),
                "--no-decorate".to_string(),
                "--format=%H%x00%h%x00%an%x00%aI%x00%s%x00".to_string(),
            ])
            .await?;
        if output.exit_code == Some(128) && output.stderr.contains("does not have any commits") {
            return Ok(GitHistory {
                commits: Vec::new(),
                truncated: false,
                lossy_output: output.stdout_lossy || output.stderr_lossy,
            });
        }
        require_success(&output, "read commit history")?;
        let fields = output.stdout.split('\0').collect::<Vec<_>>();
        let mut commits = Vec::new();
        for chunk in fields.chunks_exact(5) {
            let oid = chunk[0].trim_start_matches('\n');
            if oid.is_empty() {
                continue;
            }
            commits.push(GitCommit {
                oid: oid.to_string(),
                short_oid: chunk[1].to_string(),
                author: chunk[2].to_string(),
                authored_at: chunk[3].to_string(),
                subject: chunk[4].to_string(),
            });
        }

        Ok(GitHistory {
            truncated: output.output_truncated || commits.len() == max_entries,
            lossy_output: output.stdout_lossy,
            commits,
        })
    }

    async fn changed_paths(&self, staged: bool) -> Result<Vec<PathBuf>, GitError> {
        let mut args = vec![
            "diff".to_string(),
            "--no-ext-diff".to_string(),
            "--no-textconv".to_string(),
            "--ignore-submodules=all".to_string(),
            "--name-only".to_string(),
            "-z".to_string(),
        ];
        if staged {
            args.push("--cached".to_string());
        }
        args.push("--".to_string());
        let output = self.run_with_filters_disabled(args).await?;
        require_success(&output, "list changed files")?;
        if output.output_truncated {
            return Err(GitError::ChangedPathOutputTruncated);
        }
        let paths = output
            .stdout
            .split('\0')
            .filter(|path| !path.is_empty())
            .take(MAX_DIFF_PATHS + 1)
            .map(PathBuf::from)
            .collect::<Vec<_>>();
        if paths.len() > MAX_DIFF_PATHS {
            return Err(GitError::TooManyPaths {
                count: paths.len(),
                limit: MAX_DIFF_PATHS,
            });
        }
        paths.into_iter().map(validate_relative_path).collect()
    }

    async fn ahead_behind(&self) -> Result<(u64, u64), GitError> {
        let output = self
            .run(["rev-list", "--left-right", "--count", "HEAD...@{upstream}"])
            .await?;
        require_success(&output, "read upstream divergence")?;
        let mut counts = output.stdout.split_whitespace();
        let ahead = counts
            .next()
            .ok_or(GitError::MalformedOutput("ahead count"))?
            .parse()
            .map_err(|_| GitError::MalformedOutput("ahead count"))?;
        let behind = counts
            .next()
            .ok_or(GitError::MalformedOutput("behind count"))?
            .parse()
            .map_err(|_| GitError::MalformedOutput("behind count"))?;
        Ok((ahead, behind))
    }

    async fn optional_stdout<const N: usize>(
        &self,
        args: [&str; N],
    ) -> Result<Option<String>, GitError> {
        let output = self.run(args).await?;
        match output.exit_code {
            Some(0) if output.success => Ok(Some(output.stdout.trim().to_string())),
            Some(1 | 128) => Ok(None),
            _ => {
                require_success(&output, "read optional repository metadata")?;
                Ok(None)
            }
        }
    }

    async fn run<I, S>(&self, args: I) -> Result<ProcessOutput, GitError>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        ProcessRunner::new(
            self.workspace,
            ProcessLimits {
                timeout: self.limits.timeout,
                output_limit: self.limits.output_limit,
            },
        )
        .run_git(
            args.into_iter().map(Into::into).collect(),
            self.relative_root.clone(),
        )
        .await
        .map_err(GitError::Process)
    }

    async fn run_with_filters_disabled<I, S>(&self, args: I) -> Result<ProcessOutput, GitError>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut protected_args = self.filter_overrides().await?;
        protected_args.extend(args.into_iter().map(Into::into));
        self.run(protected_args).await
    }

    async fn filter_overrides(&self) -> Result<Vec<String>, GitError> {
        let output = self
            .run([
                "config",
                "--name-only",
                "--get-regexp",
                r"^filter\..*\.(clean|smudge|process|required)$",
            ])
            .await?;
        if output.exit_code == Some(1) {
            return Ok(Vec::new());
        }
        require_success(&output, "inspect configured content filters")?;
        if output.output_truncated {
            return Err(GitError::FilterConfigurationTruncated);
        }

        let keys = output
            .stdout
            .lines()
            .filter(|key| !key.is_empty())
            .collect::<BTreeSet<_>>();
        if keys.len() > MAX_FILTER_OVERRIDES {
            return Err(GitError::TooManyFilterOverrides {
                count: keys.len(),
                limit: MAX_FILTER_OVERRIDES,
            });
        }
        let mut args = Vec::with_capacity(keys.len() * 2);
        for key in keys {
            if !key.starts_with("filter.")
                || !key
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
            {
                return Err(GitError::InvalidFilterConfiguration(key.to_string()));
            }
            args.push("-c".to_string());
            let value = if key.ends_with(".required") {
                "false"
            } else {
                ""
            };
            args.push(format!("{key}={value}"));
        }
        Ok(args)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GitLimits {
    pub timeout: Duration,
    pub output_limit: usize,
    pub max_status_entries: usize,
}

impl Default for GitLimits {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            output_limit: 2 * 1_024 * 1_024,
            max_status_entries: 2_000,
        }
    }
}

impl GitLimits {
    fn validate(self) -> Result<(), GitError> {
        if self.max_status_entries == 0 || self.max_status_entries > 100_000 {
            Err(GitError::InvalidStatusLimit(self.max_status_entries))
        } else {
            Ok(())
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitStatus {
    pub branch: Option<String>,
    pub detached: bool,
    pub unborn: bool,
    pub head_oid: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u64,
    pub behind: u64,
    pub changes: Vec<GitChange>,
    pub truncated: bool,
    pub lossy_output: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitChange {
    pub path: PathBuf,
    pub original_path: Option<PathBuf>,
    pub index_status: char,
    pub worktree_status: char,
    pub untracked: bool,
    pub conflict: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitDiffRequest {
    #[serde(default)]
    pub staged: bool,
    #[serde(default)]
    pub paths: Vec<PathBuf>,
    #[serde(default = "default_context_lines")]
    pub context_lines: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitDiff {
    pub staged: bool,
    pub files: Vec<PathBuf>,
    pub skipped_sensitive: Vec<PathBuf>,
    pub patch: String,
    pub truncated: bool,
    pub lossy_output: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitHistory {
    pub commits: Vec<GitCommit>,
    pub truncated: bool,
    pub lossy_output: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitCommit {
    pub oid: String,
    pub short_oid: String,
    pub author: String,
    pub authored_at: String,
    pub subject: String,
}

fn require_success(output: &ProcessOutput, operation: &'static str) -> Result<(), GitError> {
    if output.timed_out {
        Err(GitError::TimedOut(operation))
    } else if output.success {
        Ok(())
    } else {
        Err(GitError::CommandFailed {
            operation,
            exit_code: output.exit_code,
            stderr: output.stderr.clone(),
        })
    }
}

fn validate_relative_path(path: PathBuf) -> Result<PathBuf, GitError> {
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(GitError::InvalidPath(path));
    }
    let normalized = path
        .components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value),
            Component::CurDir => None,
            _ => None,
        })
        .collect::<PathBuf>();
    if normalized.as_os_str().is_empty() {
        Err(GitError::InvalidPath(path))
    } else {
        Ok(normalized)
    }
}

fn literal_pathspec(path: &Path) -> String {
    format!(":(literal){}", path.to_string_lossy())
}

fn nonempty_relative(path: PathBuf) -> PathBuf {
    if path.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        path
    }
}

fn is_conflict(index: char, worktree: char) -> bool {
    matches!(
        (index, worktree),
        ('D', 'D') | ('A', 'U') | ('U', 'D') | ('U', 'A') | ('D', 'U') | ('A', 'A') | ('U', 'U')
    )
}

fn default_context_lines() -> u32 {
    3
}

#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error(transparent)]
    Workspace(#[from] WorkspaceError),
    #[error("git process failed: {0}")]
    Process(#[from] crate::coding::process::ProcessError),
    #[error("repository root is not a directory: {0}")]
    RepositoryRootNotDirectory(PathBuf),
    #[error("git repository root is outside the coding workspace: {0}")]
    RepositoryOutsideWorkspace(PathBuf),
    #[error("git {0} timed out")]
    TimedOut(&'static str),
    #[error("could not {operation}; exit code {exit_code:?}: {stderr}")]
    CommandFailed {
        operation: &'static str,
        exit_code: Option<i32>,
        stderr: String,
    },
    #[error("malformed git output for {0}")]
    MalformedOutput(&'static str),
    #[error("invalid git status entry limit: {0}")]
    InvalidStatusLimit(usize),
    #[error("invalid git history limit: {0}")]
    InvalidHistoryLimit(usize),
    #[error("invalid git diff context line count: {0}")]
    InvalidDiffContext(u32),
    #[error("git path must be a non-empty workspace-relative path: {0}")]
    InvalidPath(PathBuf),
    #[error("git operation received {count} paths, limit is {limit}")]
    TooManyPaths { count: usize, limit: usize },
    #[error("changed-path output was truncated before it could be validated")]
    ChangedPathOutputTruncated,
    #[error("git filter configuration was truncated before it could be neutralized")]
    FilterConfigurationTruncated,
    #[error("git has {count} executable content-filter settings, limit is {limit}")]
    TooManyFilterOverrides { count: usize, limit: usize },
    #[error("invalid git content-filter configuration key: {0}")]
    InvalidFilterConfiguration(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;

    fn git(root: &Path, args: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn fixture() -> (tempfile::TempDir, CodingWorkspace) {
        let temp_dir = tempfile::tempdir().unwrap();
        git(temp_dir.path(), &["init"]);
        git(temp_dir.path(), &["config", "user.name", "Test User"]);
        git(
            temp_dir.path(),
            &["config", "user.email", "test@example.com"],
        );
        fs::write(temp_dir.path().join("app.txt"), "before\n").unwrap();
        fs::write(temp_dir.path().join(".env"), "TOKEN=before\n").unwrap();
        git(temp_dir.path(), &["add", "--", "app.txt", ".env"]);
        git(temp_dir.path(), &["commit", "-m", "initial"]);
        let workspace = CodingWorkspace::new(temp_dir.path()).unwrap();
        (temp_dir, workspace)
    }

    #[tokio::test]
    async fn reads_branch_status_changed_and_untracked_files() {
        let (_temp_dir, workspace) = fixture();
        fs::write(workspace.root().join("app.txt"), "after\n").unwrap();
        fs::write(workspace.root().join("new.rs"), "fn main() {}\n").unwrap();
        let repository = GitRepository::open(&workspace, GitLimits::default())
            .await
            .unwrap();

        let status = repository.status().await.unwrap();

        assert!(status.branch.is_some());
        assert!(!status.detached);
        assert!(!status.unborn);
        assert!(status.head_oid.as_ref().is_some_and(|oid| oid.len() == 40));
        assert!(status.changes.iter().any(|change| {
            change.path == Path::new("app.txt") && change.worktree_status == 'M'
        }));
        assert!(status
            .changes
            .iter()
            .any(|change| change.path == Path::new("new.rs") && change.untracked));
    }

    #[tokio::test]
    async fn returns_unstaged_and_staged_diffs_without_sensitive_content() {
        let (_temp_dir, workspace) = fixture();
        fs::write(workspace.root().join("app.txt"), "after\n").unwrap();
        fs::write(workspace.root().join(".env"), "TOKEN=secret\n").unwrap();
        let repository = GitRepository::open(&workspace, GitLimits::default())
            .await
            .unwrap();

        let unstaged = repository
            .diff(GitDiffRequest {
                staged: false,
                paths: Vec::new(),
                context_lines: 3,
            })
            .await
            .unwrap();
        assert!(unstaged.patch.contains("+after"));
        assert!(!unstaged.patch.contains("TOKEN=secret"));
        assert_eq!(unstaged.skipped_sensitive, vec![PathBuf::from(".env")]);

        git(workspace.root(), &["add", "--", "app.txt"]);
        let staged = repository
            .diff(GitDiffRequest {
                staged: true,
                paths: Vec::new(),
                context_lines: 0,
            })
            .await
            .unwrap();
        assert!(staged.patch.contains("+after"));
        assert_eq!(staged.files, vec![PathBuf::from("app.txt")]);
    }

    #[tokio::test]
    async fn reads_bounded_commit_history() {
        let (_temp_dir, workspace) = fixture();
        let repository = GitRepository::open(&workspace, GitLimits::default())
            .await
            .unwrap();

        let history = repository.history(10).await.unwrap();

        assert_eq!(history.commits.len(), 1);
        assert_eq!(history.commits[0].subject, "initial");
        assert_eq!(history.commits[0].author, "Test User");
        assert!(!history.truncated);
    }

    #[tokio::test]
    async fn rejects_a_repository_root_outside_the_workspace() {
        let (temp_dir, _workspace) = fixture();
        let nested = temp_dir.path().join("nested");
        fs::create_dir(&nested).unwrap();
        let nested_workspace = CodingWorkspace::new(&nested).unwrap();

        let error = GitRepository::open(&nested_workspace, GitLimits::default())
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            GitError::Workspace(WorkspaceError::OutsideWorkspace(_))
        ));
    }

    #[tokio::test]
    async fn rejects_external_and_parent_diff_paths() {
        let (temp_dir, workspace) = fixture();
        let repository = GitRepository::open(&workspace, GitLimits::default())
            .await
            .unwrap();

        for path in [PathBuf::from("../outside"), temp_dir.path().join("outside")] {
            let error = repository
                .diff(GitDiffRequest {
                    staged: false,
                    paths: vec![path],
                    context_lines: 3,
                })
                .await
                .unwrap_err();
            assert!(matches!(error, GitError::InvalidPath(_)));
        }
    }

    #[tokio::test]
    async fn reports_detached_and_unborn_repository_state() {
        let (temp_dir, workspace) = fixture();
        git(temp_dir.path(), &["checkout", "--detach"]);
        let repository = GitRepository::open(&workspace, GitLimits::default())
            .await
            .unwrap();
        let detached = repository.status().await.unwrap();
        assert!(detached.detached);
        assert!(detached.branch.is_none());
        assert!(!detached.unborn);

        let unborn_dir = tempfile::tempdir().unwrap();
        git(unborn_dir.path(), &["init"]);
        let unborn_workspace = CodingWorkspace::new(unborn_dir.path()).unwrap();
        let unborn_repository = GitRepository::open(&unborn_workspace, GitLimits::default())
            .await
            .unwrap();
        let unborn = unborn_repository.status().await.unwrap();
        assert!(unborn.unborn);
        assert!(!unborn.detached);
        assert!(unborn_repository
            .history(10)
            .await
            .unwrap()
            .commits
            .is_empty());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn neutralizes_configured_content_filters_before_status_and_diff() {
        use std::os::unix::fs::PermissionsExt;

        let (_temp_dir, workspace) = fixture();
        let filter = workspace.root().join("filter.sh");
        fs::write(&filter, "#!/bin/sh\ntouch filter-was-executed\ncat\n").unwrap();
        fs::set_permissions(&filter, std::fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(
            workspace.root().join(".gitattributes"),
            "*.txt filter=evil\n",
        )
        .unwrap();
        git(
            workspace.root(),
            &["config", "filter.evil.clean", "./filter.sh"],
        );
        git(
            workspace.root(),
            &["config", "filter.evil.required", "true"],
        );
        fs::write(workspace.root().join("app.txt"), "after\n").unwrap();
        let repository = GitRepository::open(&workspace, GitLimits::default())
            .await
            .unwrap();

        let status = repository.status().await.unwrap();
        let diff = repository
            .diff(GitDiffRequest {
                staged: false,
                paths: vec![PathBuf::from("app.txt")],
                context_lines: 3,
            })
            .await
            .unwrap();

        assert!(status
            .changes
            .iter()
            .any(|change| change.path == Path::new("app.txt")));
        assert!(diff.patch.contains("+after"));
        assert!(!workspace.root().join("filter-was-executed").exists());
    }
}
