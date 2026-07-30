use crate::coding::file::{content_digest, FileReadOptions, FileSnapshot, MAX_READ_LIMIT};
use crate::coding::process::{ProcessLimits, ProcessOutput, ProcessRunner};
use crate::coding::sensitive::is_sensitive_path;
use crate::coding::workspace::{CodingWorkspace, WorkspaceError};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

const MAX_DIFF_PATHS: usize = 100;
const MAX_DIFF_CONTEXT: u32 = 1_000;
const MAX_HISTORY_ENTRIES: usize = 100;
const MAX_FILTER_OVERRIDES: usize = 20;
const MAX_WRITE_PATHS: usize = 50;
const MAX_COMMIT_MESSAGE_BYTES: usize = 4 * 1_024;

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

    pub async fn stage_owned(&self, paths: &[GitOwnedPath]) -> Result<GitStageResult, GitError> {
        let paths = validate_owned_paths(paths)?;
        for owned in &paths {
            self.verify_owned_worktree(owned)?;
            let index = self.index_entry(&owned.path).await?;
            let head = self.head_entry(&owned.path).await?;
            if index.as_ref().map(|entry| &entry.oid) != head.as_ref().map(|entry| &entry.oid) {
                return Err(GitError::PreexistingStagedChange(owned.path.clone()));
            }
            match (&owned.original_digest, &index) {
                (Some(expected), Some(entry)) => {
                    let actual = self.index_digest(entry).await?;
                    if &actual != expected {
                        return Err(GitError::OriginalDoesNotMatchIndex {
                            path: owned.path.clone(),
                            expected: expected.clone(),
                            actual,
                        });
                    }
                }
                (None, None) => {}
                _ => return Err(GitError::OriginalIndexStateMismatch(owned.path.clone())),
            }
        }

        let mut args = vec!["add".to_string(), "-A".to_string(), "--".to_string()];
        args.extend(paths.iter().map(|owned| literal_pathspec(&owned.path)));
        let output = self.run_with_filters_disabled(args).await?;
        require_success(&output, "stage owned files")?;

        for owned in &paths {
            self.verify_index_matches_applied(owned).await?;
        }
        Ok(GitStageResult {
            staged_files: paths.into_iter().map(|owned| owned.path).collect(),
        })
    }

    pub async fn unstage_owned(
        &self,
        paths: &[GitOwnedPath],
    ) -> Result<GitUnstageResult, GitError> {
        let paths = validate_owned_paths(paths)?;
        for owned in &paths {
            self.verify_index_matches_applied(owned).await?;
        }

        let head_exists = self
            .optional_stdout(["rev-parse", "--verify", "HEAD"])
            .await?
            .is_some();
        let mut args = if head_exists {
            vec![
                "reset".to_string(),
                "--quiet".to_string(),
                "HEAD".to_string(),
                "--".to_string(),
            ]
        } else {
            vec![
                "rm".to_string(),
                "--cached".to_string(),
                "--quiet".to_string(),
                "--ignore-unmatch".to_string(),
                "--".to_string(),
            ]
        };
        args.extend(paths.iter().map(|owned| literal_pathspec(&owned.path)));
        let output = self.run(args).await?;
        require_success(&output, "unstage owned files")?;

        for owned in &paths {
            let index = self.index_entry(&owned.path).await?;
            match (&owned.original_digest, index) {
                (Some(expected), Some(entry)) => {
                    let actual = self.index_digest(&entry).await?;
                    if &actual != expected {
                        return Err(GitError::OriginalDoesNotMatchIndex {
                            path: owned.path.clone(),
                            expected: expected.clone(),
                            actual,
                        });
                    }
                }
                (None, None) => {}
                _ => {
                    return Err(GitError::OriginalIndexStateMismatch(owned.path.clone()));
                }
            }
        }
        Ok(GitUnstageResult {
            unstaged_files: paths.into_iter().map(|owned| owned.path).collect(),
        })
    }

    pub async fn commit_owned(
        &self,
        message: &str,
        paths: &[GitOwnedPath],
    ) -> Result<GitCommitResult, GitError> {
        validate_commit_message(message)?;
        let paths = validate_owned_paths(paths)?;
        for owned in &paths {
            self.verify_index_matches_applied(owned).await?;
        }

        let expected = paths
            .iter()
            .map(|owned| owned.path.clone())
            .collect::<BTreeSet<_>>();
        let staged = self
            .staged_paths()
            .await?
            .into_iter()
            .collect::<BTreeSet<_>>();
        if staged != expected {
            return Err(GitError::StagedPathsMismatch {
                expected: expected.into_iter().collect(),
                actual: staged.into_iter().collect(),
            });
        }

        let hooks_path = if cfg!(windows) { "NUL" } else { "/dev/null" };
        let output = self
            .run(vec![
                "-c".to_string(),
                format!("core.hooksPath={hooks_path}"),
                "-c".to_string(),
                "commit.gpgSign=false".to_string(),
                "commit".to_string(),
                "--no-verify".to_string(),
                "--no-gpg-sign".to_string(),
                "-m".to_string(),
                message.to_string(),
            ])
            .await?;
        require_success(&output, "commit owned files")?;
        let oid = self
            .optional_stdout(["rev-parse", "--verify", "HEAD"])
            .await?
            .ok_or(GitError::MalformedOutput("commit oid"))?;

        Ok(GitCommitResult {
            oid,
            committed_files: paths.into_iter().map(|owned| owned.path).collect(),
        })
    }

    pub async fn revert_owned_commit(
        &self,
        owned_commit_oid: &str,
    ) -> Result<GitRevertResult, GitError> {
        validate_object_id(owned_commit_oid)?;
        let before = self.status().await?;
        if before.truncated || !before.changes.is_empty() {
            return Err(GitError::DirtyRevert);
        }
        let head_oid = before
            .head_oid
            .ok_or(GitError::MalformedOutput("revert HEAD"))?;
        if head_oid != owned_commit_oid {
            return Err(GitError::RevertHeadMismatch {
                expected: owned_commit_oid.to_string(),
                actual: head_oid,
            });
        }

        let hooks_path = if cfg!(windows) { "NUL" } else { "/dev/null" };
        let output = self
            .run_with_filters_disabled(vec![
                "-c".to_string(),
                format!("core.hooksPath={hooks_path}"),
                "-c".to_string(),
                "commit.gpgSign=false".to_string(),
                "revert".to_string(),
                "--no-edit".to_string(),
                owned_commit_oid.to_string(),
            ])
            .await?;
        if !output.success {
            return self.recover_failed_revert(owned_commit_oid, output).await;
        }

        let after = self.status().await?;
        if after.truncated || !after.changes.is_empty() {
            return Err(GitError::RevertLeftChanges);
        }
        let revert_oid = after
            .head_oid
            .ok_or(GitError::MalformedOutput("revert commit oid"))?;
        if revert_oid == owned_commit_oid {
            return Err(GitError::MalformedOutput("unchanged revert HEAD"));
        }
        Ok(GitRevertResult {
            reverted_oid: owned_commit_oid.to_string(),
            revert_oid,
        })
    }

    pub async fn create_branch(
        &self,
        name: &str,
        start_point: Option<&str>,
    ) -> Result<GitBranchResult, GitError> {
        validate_revision_text(name, "branch name")?;
        let checked = self.run(["check-ref-format", "--branch", name]).await?;
        require_success(&checked, "validate branch name")?;
        let start = start_point.unwrap_or("HEAD");
        validate_revision_text(start, "branch start point")?;
        let resolved = self
            .run(vec![
                "rev-parse".to_string(),
                "--verify".to_string(),
                format!("{start}^{{commit}}"),
            ])
            .await?;
        require_success(&resolved, "resolve branch start point")?;
        let start_oid = resolved.stdout.trim().to_string();
        let created = self
            .run(["branch", "--no-track", "--", name, start_oid.as_str()])
            .await?;
        require_success(&created, "create branch")?;

        Ok(GitBranchResult {
            name: name.to_string(),
            start_oid,
        })
    }

    pub async fn push_current_branch(
        &self,
        owned_commit_oid: &str,
        remote: &str,
    ) -> Result<GitPushResult, GitError> {
        validate_object_id(owned_commit_oid)?;
        validate_revision_text(remote, "remote name")?;
        let head_oid = self
            .optional_stdout(["rev-parse", "--verify", "HEAD"])
            .await?
            .ok_or(GitError::MalformedOutput("push HEAD"))?;
        if head_oid != owned_commit_oid {
            return Err(GitError::PushHeadMismatch {
                expected: owned_commit_oid.to_string(),
                actual: head_oid,
            });
        }
        let branch = self
            .optional_stdout(["symbolic-ref", "--quiet", "--short", "HEAD"])
            .await?
            .ok_or(GitError::DetachedPush)?;
        validate_revision_text(&branch, "current branch")?;

        let remote_output = self.run(["remote", "get-url", "--push", remote]).await?;
        if !remote_output.success {
            return Err(GitError::InvalidPushRemote(remote.to_string()));
        }
        let remote_url = remote_output.stdout.trim();
        if !self.allowed_push_url(remote_url)? {
            return Err(GitError::UnsafePushRemote(remote.to_string()));
        }

        let output = self
            .run(vec![
                "-c".to_string(),
                format!(
                    "core.hooksPath={}",
                    if cfg!(windows) { "NUL" } else { "/dev/null" }
                ),
                "-c".to_string(),
                "credential.helper=".to_string(),
                "push".to_string(),
                "--no-verify".to_string(),
                "--porcelain".to_string(),
                "--".to_string(),
                remote.to_string(),
                format!("{owned_commit_oid}:refs/heads/{branch}"),
            ])
            .await?;
        if !output.success {
            return Err(GitError::PushFailed {
                remote: remote.to_string(),
                exit_code: output.exit_code,
            });
        }
        Ok(GitPushResult {
            oid: owned_commit_oid.to_string(),
            remote: remote.to_string(),
            branch,
        })
    }

    fn allowed_push_url(&self, url: &str) -> Result<bool, GitError> {
        if let Some(authority) = url
            .strip_prefix("https://")
            .and_then(|rest| rest.split('/').next())
        {
            return Ok(!authority.is_empty() && !authority.contains('@'));
        }
        if let Some(rest) = url.strip_prefix("ssh://") {
            return Ok(!rest.is_empty() && !rest.chars().any(char::is_whitespace));
        }
        if url.contains('@')
            && url.contains(':')
            && !url.chars().any(char::is_whitespace)
            && !url.starts_with('-')
        {
            return Ok(true);
        }

        let path = Path::new(url.strip_prefix("file://").unwrap_or(url));
        let local_path = url.starts_with("file://")
            || (!url.is_empty() && !url.contains("://") && !url.contains(':'));
        if local_path {
            return match self.workspace.resolve_existing(path) {
                Ok(resolved) => Ok(resolved.is_dir()),
                Err(WorkspaceError::OutsideWorkspace(_)) => Ok(false),
                Err(error) => Err(GitError::Workspace(error)),
            };
        }
        Ok(false)
    }

    fn verify_owned_worktree(&self, owned: &GitOwnedPath) -> Result<(), GitError> {
        if is_sensitive_path(&owned.path) {
            return Err(GitError::SensitivePath(owned.path.clone()));
        }
        match &owned.applied_digest {
            Some(expected) => {
                let snapshot = FileSnapshot::read(
                    self.workspace,
                    &owned.path,
                    FileReadOptions {
                        max_bytes: self.limits.output_limit.min(MAX_READ_LIMIT),
                        start_line: None,
                        end_line: None,
                    },
                )
                .map_err(|error| GitError::OwnedWorktreeUnavailable {
                    path: owned.path.clone(),
                    reason: error.to_string(),
                })?;
                if &snapshot.digest == expected {
                    Ok(())
                } else {
                    Err(GitError::AppliedDigestMismatch {
                        path: owned.path.clone(),
                        expected: expected.clone(),
                        actual: Some(snapshot.digest),
                    })
                }
            }
            None => {
                let resolved = self.workspace.resolve_for_write(&owned.path)?;
                match resolved.symlink_metadata() {
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                    Err(error) => Err(GitError::OwnedWorktreeUnavailable {
                        path: owned.path.clone(),
                        reason: error.to_string(),
                    }),
                    Ok(_) => Err(GitError::AppliedDigestMismatch {
                        path: owned.path.clone(),
                        expected: "absent".to_string(),
                        actual: Some("present".to_string()),
                    }),
                }
            }
        }
    }

    async fn verify_index_matches_applied(&self, owned: &GitOwnedPath) -> Result<(), GitError> {
        let index = self.index_entry(&owned.path).await?;
        match (&owned.applied_digest, index) {
            (Some(expected), Some(entry)) => {
                let actual = self.index_digest(&entry).await?;
                if &actual == expected {
                    Ok(())
                } else {
                    Err(GitError::AppliedDigestMismatch {
                        path: owned.path.clone(),
                        expected: expected.clone(),
                        actual: Some(actual),
                    })
                }
            }
            (None, None) => Ok(()),
            (expected, actual) => Err(GitError::AppliedDigestMismatch {
                path: owned.path.clone(),
                expected: expected.clone().unwrap_or_else(|| "absent".to_string()),
                actual: actual.map(|entry| entry.oid),
            }),
        }
    }

    async fn index_digest(&self, entry: &GitIndexEntry) -> Result<String, GitError> {
        let output = self.run(["cat-file", "blob", entry.oid.as_str()]).await?;
        require_success(&output, "read index blob")?;
        if output.output_truncated || output.stdout_lossy {
            return Err(GitError::IndexBlobUnavailable(entry.oid.clone()));
        }
        Ok(content_digest(output.stdout.as_bytes()))
    }

    async fn index_entry(&self, path: &Path) -> Result<Option<GitIndexEntry>, GitError> {
        let output = self
            .run(vec![
                "ls-files".to_string(),
                "--stage".to_string(),
                "-z".to_string(),
                "--".to_string(),
                literal_pathspec(path),
            ])
            .await?;
        require_success(&output, "read index entry")?;
        if output.output_truncated {
            return Err(GitError::MalformedOutput("truncated index entry"));
        }
        parse_index_entry(&output.stdout)
    }

    async fn head_entry(&self, path: &Path) -> Result<Option<GitIndexEntry>, GitError> {
        let output = self
            .run(vec![
                "ls-tree".to_string(),
                "-z".to_string(),
                "HEAD".to_string(),
                "--".to_string(),
                literal_pathspec(path),
            ])
            .await?;
        if output.exit_code == Some(128) && output.stderr.contains("Not a valid object name") {
            return Ok(None);
        }
        require_success(&output, "read HEAD entry")?;
        if output.output_truncated {
            return Err(GitError::MalformedOutput("truncated HEAD entry"));
        }
        parse_tree_entry(&output.stdout)
    }

    async fn staged_paths(&self) -> Result<Vec<PathBuf>, GitError> {
        let output = self
            .run([
                "diff",
                "--cached",
                "--no-ext-diff",
                "--no-textconv",
                "--ignore-submodules=all",
                "--name-only",
                "-z",
                "--",
            ])
            .await?;
        require_success(&output, "list staged files")?;
        if output.output_truncated {
            return Err(GitError::ChangedPathOutputTruncated);
        }
        output
            .stdout
            .split('\0')
            .filter(|path| !path.is_empty())
            .map(PathBuf::from)
            .map(validate_relative_path)
            .collect()
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

    async fn recover_failed_revert(
        &self,
        owned_commit_oid: &str,
        failed: ProcessOutput,
    ) -> Result<GitRevertResult, GitError> {
        let status = self.status().await?;
        if !status.truncated
            && status.changes.is_empty()
            && status.head_oid.as_deref() == Some(owned_commit_oid)
        {
            return Err(GitError::CommandFailed {
                operation: "revert owned commit",
                exit_code: failed.exit_code,
                stderr: failed.stderr,
            });
        }

        let aborted = self
            .run_with_filters_disabled(["revert", "--abort"])
            .await?;
        if !aborted.success {
            return Err(GitError::RevertCleanupFailed {
                revert_exit_code: failed.exit_code,
                abort_exit_code: aborted.exit_code,
            });
        }
        let restored = self.status().await?;
        if restored.truncated
            || !restored.changes.is_empty()
            || restored.head_oid.as_deref() != Some(owned_commit_oid)
        {
            return Err(GitError::RevertCleanupIncomplete);
        }
        Err(GitError::CommandFailed {
            operation: "revert owned commit",
            exit_code: failed.exit_code,
            stderr: failed.stderr,
        })
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitOwnedPath {
    pub path: PathBuf,
    pub original_digest: Option<String>,
    pub applied_digest: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitStageResult {
    pub staged_files: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitUnstageResult {
    pub unstaged_files: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitCommitResult {
    pub oid: String,
    pub committed_files: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitRevertResult {
    pub reverted_oid: String,
    pub revert_oid: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitBranchResult {
    pub name: String,
    pub start_oid: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitPushResult {
    pub oid: String,
    pub remote: String,
    pub branch: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GitIndexEntry {
    oid: String,
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

fn validate_owned_paths(paths: &[GitOwnedPath]) -> Result<Vec<GitOwnedPath>, GitError> {
    if paths.is_empty() {
        return Err(GitError::EmptyOwnedPaths);
    }
    if paths.len() > MAX_WRITE_PATHS {
        return Err(GitError::TooManyPaths {
            count: paths.len(),
            limit: MAX_WRITE_PATHS,
        });
    }
    let mut seen = BTreeSet::new();
    let mut validated = Vec::with_capacity(paths.len());
    for owned in paths {
        let path = validate_relative_path(owned.path.clone())?;
        if path.to_string_lossy().starts_with('-') {
            return Err(GitError::InvalidPath(path));
        }
        if !seen.insert(path.clone()) {
            return Err(GitError::DuplicateOwnedPath(path));
        }
        validated.push(GitOwnedPath {
            path,
            original_digest: owned.original_digest.clone(),
            applied_digest: owned.applied_digest.clone(),
        });
    }
    Ok(validated)
}

fn validate_commit_message(message: &str) -> Result<(), GitError> {
    if message.trim().is_empty()
        || message.len() > MAX_COMMIT_MESSAGE_BYTES
        || message.contains('\0')
        || message
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\n' | '\t'))
    {
        Err(GitError::InvalidCommitMessage)
    } else {
        Ok(())
    }
}

fn validate_revision_text(value: &str, kind: &'static str) -> Result<(), GitError> {
    if value.is_empty()
        || value.len() > 256
        || value.starts_with('-')
        || value.chars().any(char::is_control)
    {
        Err(GitError::InvalidRevision { kind })
    } else {
        Ok(())
    }
}

fn validate_object_id(oid: &str) -> Result<(), GitError> {
    if matches!(oid.len(), 40 | 64) && oid.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(GitError::InvalidOwnedCommit)
    }
}

fn parse_index_entry(output: &str) -> Result<Option<GitIndexEntry>, GitError> {
    let records = output
        .split('\0')
        .filter(|record| !record.is_empty())
        .collect::<Vec<_>>();
    if records.is_empty() {
        return Ok(None);
    }
    if records.len() != 1 {
        return Err(GitError::ConflictedIndexEntry);
    }
    let metadata = records[0]
        .split_once('\t')
        .ok_or(GitError::MalformedOutput("index entry"))?
        .0;
    let fields = metadata.split_whitespace().collect::<Vec<_>>();
    if fields.len() != 3 || fields[2] != "0" {
        return Err(GitError::ConflictedIndexEntry);
    }
    Ok(Some(GitIndexEntry {
        oid: fields[1].to_string(),
    }))
}

fn parse_tree_entry(output: &str) -> Result<Option<GitIndexEntry>, GitError> {
    let record = output.split('\0').find(|record| !record.is_empty());
    let Some(record) = record else {
        return Ok(None);
    };
    let metadata = record
        .split_once('\t')
        .ok_or(GitError::MalformedOutput("HEAD entry"))?
        .0;
    let fields = metadata.split_whitespace().collect::<Vec<_>>();
    if fields.len() != 3 || fields[1] != "blob" {
        return Err(GitError::MalformedOutput("HEAD entry"));
    }
    Ok(Some(GitIndexEntry {
        oid: fields[2].to_string(),
    }))
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
    #[error("owned Git path list must not be empty")]
    EmptyOwnedPaths,
    #[error("duplicate owned Git path: {0}")]
    DuplicateOwnedPath(PathBuf),
    #[error("sensitive file cannot be staged by the coding agent: {0}")]
    SensitivePath(PathBuf),
    #[error("owned worktree file is unavailable at {path}: {reason}")]
    OwnedWorktreeUnavailable { path: PathBuf, reason: String },
    #[error(
        "file changed after the agent mutation for {path}: expected {expected}, current {actual:?}"
    )]
    AppliedDigestMismatch {
        path: PathBuf,
        expected: String,
        actual: Option<String>,
    },
    #[error("file already had staged changes before the agent mutation: {0}")]
    PreexistingStagedChange(PathBuf),
    #[error("agent mutation origin does not match the Git index state: {0}")]
    OriginalIndexStateMismatch(PathBuf),
    #[error("original content for {path} does not match the Git index: expected {expected}, index {actual}")]
    OriginalDoesNotMatchIndex {
        path: PathBuf,
        expected: String,
        actual: String,
    },
    #[error("could not read complete UTF-8 Git index blob {0}")]
    IndexBlobUnavailable(String),
    #[error("Git index entry is conflicted")]
    ConflictedIndexEntry,
    #[error("staged paths do not exactly match agent-owned paths; expected {expected:?}, actual {actual:?}")]
    StagedPathsMismatch {
        expected: Vec<PathBuf>,
        actual: Vec<PathBuf>,
    },
    #[error("commit message is empty, too large, or contains unsupported control characters")]
    InvalidCommitMessage,
    #[error("invalid Git {kind}")]
    InvalidRevision { kind: &'static str },
    #[error("invalid agent-owned commit object id")]
    InvalidOwnedCommit,
    #[error("owned commit revert requires a completely clean and untruncated Git status")]
    DirtyRevert,
    #[error("current HEAD {actual} does not match agent-owned commit {expected} to revert")]
    RevertHeadMismatch { expected: String, actual: String },
    #[error("owned commit revert unexpectedly left worktree or index changes")]
    RevertLeftChanges,
    #[error(
        "owned commit revert failed with exit code {revert_exit_code:?}, and cleanup failed with exit code {abort_exit_code:?}"
    )]
    RevertCleanupFailed {
        revert_exit_code: Option<i32>,
        abort_exit_code: Option<i32>,
    },
    #[error("owned commit revert cleanup did not restore the original clean HEAD")]
    RevertCleanupIncomplete,
    #[error("current HEAD {actual} does not match agent-owned commit {expected}")]
    PushHeadMismatch { expected: String, actual: String },
    #[error("cannot push from a detached HEAD")]
    DetachedPush,
    #[error("Git push remote does not exist or has no push URL: {0}")]
    InvalidPushRemote(String),
    #[error("Git push remote is not an approved network URL or workspace-local repository: {0}")]
    UnsafePushRemote(String),
    #[error("Git push to remote {remote} failed with exit code {exit_code:?}")]
    PushFailed {
        remote: String,
        exit_code: Option<i32>,
    },
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
        fs::write(temp_dir.path().join("other.txt"), "other\n").unwrap();
        fs::write(temp_dir.path().join(".env"), "TOKEN=before\n").unwrap();
        git(
            temp_dir.path(),
            &["add", "--", "app.txt", "other.txt", ".env"],
        );
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
    async fn stages_and_commits_only_digest_owned_agent_changes() {
        let (_temp_dir, workspace) = fixture();
        let app = workspace.root().join("app.txt");
        let original_digest = content_digest(&fs::read(&app).unwrap());
        fs::write(&app, "after\n").unwrap();
        let applied_digest = content_digest(&fs::read(&app).unwrap());
        let owned = GitOwnedPath {
            path: PathBuf::from("app.txt"),
            original_digest: Some(original_digest),
            applied_digest: Some(applied_digest),
        };
        let repository = GitRepository::open(&workspace, GitLimits::default())
            .await
            .unwrap();

        let staged = repository
            .stage_owned(std::slice::from_ref(&owned))
            .await
            .unwrap();
        assert_eq!(staged.staged_files, vec![PathBuf::from("app.txt")]);
        assert!(repository
            .diff(GitDiffRequest {
                staged: true,
                paths: vec![PathBuf::from("app.txt")],
                context_lines: 3,
            })
            .await
            .unwrap()
            .patch
            .contains("+after"));
        let unstaged = repository
            .unstage_owned(std::slice::from_ref(&owned))
            .await
            .unwrap();
        assert_eq!(unstaged.unstaged_files, vec![PathBuf::from("app.txt")]);
        assert!(repository
            .diff(GitDiffRequest {
                staged: true,
                paths: vec![PathBuf::from("app.txt")],
                context_lines: 3,
            })
            .await
            .unwrap()
            .patch
            .is_empty());
        repository
            .stage_owned(std::slice::from_ref(&owned))
            .await
            .unwrap();

        let committed = repository
            .commit_owned("agent change", std::slice::from_ref(&owned))
            .await
            .unwrap();
        assert_eq!(committed.committed_files, vec![PathBuf::from("app.txt")]);
        assert_eq!(committed.oid.len(), 40);
        assert!(repository.status().await.unwrap().changes.is_empty());
        assert_eq!(
            repository.history(1).await.unwrap().commits[0].subject,
            "agent change"
        );

        git(workspace.root(), &["init", "--bare", ".test-remote.git"]);
        git(
            workspace.root(),
            &["remote", "add", "test-origin", ".test-remote.git"],
        );
        let pushed = repository
            .push_current_branch(&committed.oid, "test-origin")
            .await
            .unwrap();
        assert_eq!(pushed.oid, committed.oid);
        assert_eq!(pushed.remote, "test-origin");
        let remote_head = Command::new("git")
            .arg("--git-dir")
            .arg(workspace.root().join(".test-remote.git"))
            .args(["rev-parse", &format!("refs/heads/{}", pushed.branch)])
            .output()
            .unwrap();
        assert!(remote_head.status.success());
        assert_eq!(
            String::from_utf8(remote_head.stdout).unwrap().trim(),
            committed.oid
        );
    }

    #[tokio::test]
    async fn refuses_to_stage_changes_that_started_from_a_dirty_user_file() {
        let (_temp_dir, workspace) = fixture();
        let app = workspace.root().join("app.txt");
        fs::write(&app, "user change\n").unwrap();
        let user_digest = content_digest(&fs::read(&app).unwrap());
        fs::write(&app, "user change plus agent\n").unwrap();
        let agent_digest = content_digest(&fs::read(&app).unwrap());
        let repository = GitRepository::open(&workspace, GitLimits::default())
            .await
            .unwrap();

        let error = repository
            .stage_owned(&[GitOwnedPath {
                path: PathBuf::from("app.txt"),
                original_digest: Some(user_digest),
                applied_digest: Some(agent_digest),
            }])
            .await
            .unwrap_err();

        assert!(matches!(error, GitError::OriginalDoesNotMatchIndex { .. }));
        let status = repository.status().await.unwrap();
        assert!(status
            .changes
            .iter()
            .any(|change| change.path == Path::new("app.txt") && change.index_status == ' '));
    }

    #[tokio::test]
    async fn refuses_to_commit_when_any_foreign_path_is_staged() {
        let (_temp_dir, workspace) = fixture();
        fs::write(workspace.root().join("other.txt"), "user staged\n").unwrap();
        git(workspace.root(), &["add", "--", "other.txt"]);
        let app = workspace.root().join("app.txt");
        let original_digest = content_digest(&fs::read(&app).unwrap());
        fs::write(&app, "agent\n").unwrap();
        let applied_digest = content_digest(&fs::read(&app).unwrap());
        let owned = GitOwnedPath {
            path: PathBuf::from("app.txt"),
            original_digest: Some(original_digest),
            applied_digest: Some(applied_digest),
        };
        let repository = GitRepository::open(&workspace, GitLimits::default())
            .await
            .unwrap();
        repository
            .stage_owned(std::slice::from_ref(&owned))
            .await
            .unwrap();

        let error = repository
            .commit_owned("must fail", std::slice::from_ref(&owned))
            .await
            .unwrap_err();

        assert!(matches!(error, GitError::StagedPathsMismatch { .. }));
        assert_eq!(
            repository.history(1).await.unwrap().commits[0].subject,
            "initial"
        );
    }

    #[tokio::test]
    async fn reverts_only_a_clean_current_commit_without_rewriting_history() {
        let (_temp_dir, workspace) = fixture();
        let repository = GitRepository::open(&workspace, GitLimits::default())
            .await
            .unwrap();
        let app = workspace.root().join("app.txt");
        let original_digest = content_digest(&fs::read(&app).unwrap());
        fs::write(&app, "agent\n").unwrap();
        let owned = GitOwnedPath {
            path: PathBuf::from("app.txt"),
            original_digest: Some(original_digest),
            applied_digest: Some(content_digest(&fs::read(&app).unwrap())),
        };
        repository
            .stage_owned(std::slice::from_ref(&owned))
            .await
            .unwrap();
        let committed = repository
            .commit_owned("agent change", std::slice::from_ref(&owned))
            .await
            .unwrap();

        let reverted = repository
            .revert_owned_commit(&committed.oid)
            .await
            .unwrap();

        assert_eq!(reverted.reverted_oid, committed.oid);
        assert_ne!(reverted.revert_oid, reverted.reverted_oid);
        assert_eq!(fs::read_to_string(&app).unwrap(), "before\n");
        let status = repository.status().await.unwrap();
        assert_eq!(
            status.head_oid.as_deref(),
            Some(reverted.revert_oid.as_str())
        );
        assert!(status.changes.is_empty());
        let history = repository.history(2).await.unwrap();
        assert_eq!(history.commits[0].subject, "Revert \"agent change\"");
        assert_eq!(history.commits[1].subject, "agent change");

        fs::write(workspace.root().join("other.txt"), "user change\n").unwrap();
        let error = repository
            .revert_owned_commit(&reverted.revert_oid)
            .await
            .unwrap_err();
        assert!(matches!(error, GitError::DirtyRevert));
        assert_eq!(
            repository.status().await.unwrap().head_oid.as_deref(),
            Some(reverted.revert_oid.as_str())
        );
    }

    #[tokio::test]
    async fn creates_a_valid_branch_without_switching_or_overwriting() {
        let (_temp_dir, workspace) = fixture();
        let repository = GitRepository::open(&workspace, GitLimits::default())
            .await
            .unwrap();
        let current = repository.status().await.unwrap().branch.unwrap();

        let created = repository
            .create_branch("agent/topic", Some("HEAD"))
            .await
            .unwrap();

        assert_eq!(created.name, "agent/topic");
        assert_eq!(created.start_oid.len(), 40);
        assert_eq!(repository.status().await.unwrap().branch.unwrap(), current);
        assert!(matches!(
            repository.create_branch("-invalid", None).await,
            Err(GitError::InvalidRevision {
                kind: "branch name"
            })
        ));
    }

    #[tokio::test]
    async fn stages_unstages_and_commits_agent_created_files_on_an_unborn_branch() {
        let temp_dir = tempfile::tempdir().unwrap();
        git(temp_dir.path(), &["init"]);
        git(temp_dir.path(), &["config", "user.name", "Test User"]);
        git(
            temp_dir.path(),
            &["config", "user.email", "test@example.com"],
        );
        let path = temp_dir.path().join("new.txt");
        fs::write(&path, "created\n").unwrap();
        let owned = GitOwnedPath {
            path: PathBuf::from("new.txt"),
            original_digest: None,
            applied_digest: Some(content_digest(&fs::read(&path).unwrap())),
        };
        let workspace = CodingWorkspace::new(temp_dir.path()).unwrap();
        let repository = GitRepository::open(&workspace, GitLimits::default())
            .await
            .unwrap();

        repository
            .stage_owned(std::slice::from_ref(&owned))
            .await
            .unwrap();
        repository
            .unstage_owned(std::slice::from_ref(&owned))
            .await
            .unwrap();
        assert!(repository.status().await.unwrap().changes[0].untracked);
        repository
            .stage_owned(std::slice::from_ref(&owned))
            .await
            .unwrap();
        let committed = repository
            .commit_owned("initial agent file", std::slice::from_ref(&owned))
            .await
            .unwrap();

        assert_eq!(committed.committed_files, vec![PathBuf::from("new.txt")]);
        assert!(!repository.status().await.unwrap().unborn);
    }

    #[tokio::test]
    async fn rejects_pushes_to_local_repositories_outside_the_workspace() {
        let (_temp_dir, workspace) = fixture();
        let outside = tempfile::tempdir().unwrap();
        git(outside.path(), &["init", "--bare"]);
        git(
            workspace.root(),
            &["remote", "add", "outside", outside.path().to_str().unwrap()],
        );
        let repository = GitRepository::open(&workspace, GitLimits::default())
            .await
            .unwrap();
        let oid = repository.status().await.unwrap().head_oid.unwrap();

        let error = repository
            .push_current_branch(&oid, "outside")
            .await
            .unwrap_err();

        assert!(matches!(error, GitError::UnsafePushRemote(_)));
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
        let app = workspace.root().join("app.txt");
        let original_digest = content_digest(&fs::read(&app).unwrap());
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
        fs::write(&app, "after\n").unwrap();
        let applied_digest = content_digest(&fs::read(&app).unwrap());
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
        repository
            .stage_owned(&[GitOwnedPath {
                path: PathBuf::from("app.txt"),
                original_digest: Some(original_digest),
                applied_digest: Some(applied_digest),
            }])
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
