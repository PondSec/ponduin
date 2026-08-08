use crate::coding::file::content_digest;
use crate::coding::outcome::{ActionFailureKind, ActionResult};
use crate::coding::sensitive::is_sensitive_path;
use crate::coding::workspace::{CodingWorkspace, WorkspaceError};
use serde::{Deserialize, Serialize};
use similar::TextDiff;
use std::collections::HashSet;
use std::fs::{self, File, Permissions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use tempfile::NamedTempFile;
use uuid::Uuid;

pub const DEFAULT_PATCH_FILE_LIMIT: usize = 2 * 1_024 * 1_024;
pub const MAX_PATCH_FILE_LIMIT: usize = 10 * 1_024 * 1_024;
pub const DEFAULT_PATCH_BATCH_LIMIT: usize = 16 * 1_024 * 1_024;
pub const MAX_PATCH_BATCH_LIMIT: usize = 100 * 1_024 * 1_024;

#[derive(Debug)]
pub struct PatchEngine<'workspace> {
    workspace: &'workspace CodingWorkspace,
    limits: PatchLimits,
}

impl<'workspace> PatchEngine<'workspace> {
    pub fn new(workspace: &'workspace CodingWorkspace, limits: PatchLimits) -> Self {
        Self { workspace, limits }
    }

    pub fn prepare(&self, batch: MutationBatch) -> Result<PreparedBatch, PatchError> {
        self.limits.validate()?;
        if batch.changes.is_empty() {
            return Err(PatchError::EmptyBatch);
        }
        let touched_files = batch.touched_file_count();
        if touched_files > self.limits.max_files {
            return Err(PatchError::TooManyFiles {
                count: touched_files,
                limit: self.limits.max_files,
            });
        }

        let mut seen = HashSet::new();
        let mut prepared = Vec::with_capacity(touched_files);
        let mut batch_bytes = 0usize;
        for change in batch.changes {
            for prepared_change in self.prepare_change(change)? {
                if !seen.insert(prepared_change.path.clone()) {
                    return Err(PatchError::DuplicatePath(prepared_change.relative_path));
                }
                batch_bytes = batch_bytes
                    .checked_add(prepared_change.retained_bytes())
                    .ok_or(PatchError::BatchTooLarge {
                        size: usize::MAX,
                        limit: self.limits.max_batch_bytes,
                    })?;
                if batch_bytes > self.limits.max_batch_bytes {
                    return Err(PatchError::BatchTooLarge {
                        size: batch_bytes,
                        limit: self.limits.max_batch_bytes,
                    });
                }
                prepared.push(prepared_change);
            }
        }

        let files = prepared.iter().map(PreparedChange::preview).collect();
        Ok(PreparedBatch {
            preview: MutationPreview { files },
            changes: prepared,
        })
    }

    pub fn apply(&self, prepared: PreparedBatch) -> Result<AppliedBatch, PatchError> {
        for change in &prepared.changes {
            verify_current(self.workspace, change, self.limits.max_file_bytes)?;
        }
        let mut staged = stage_changes(self.workspace, &prepared.changes)?;
        let created_directories =
            create_missing_parent_directories(self.workspace, &prepared.changes)?;
        for change in &prepared.changes {
            if let Err(error) = verify_current(self.workspace, change, self.limits.max_file_bytes) {
                return match remove_created_directories(&created_directories) {
                    Ok(()) => Err(error),
                    Err(cleanup) => Err(PatchError::ApplyAndRestoreFailed {
                        path: change.relative_path.clone(),
                        apply: error.to_string(),
                        rollback: cleanup.to_string(),
                    }),
                };
            }
        }

        let mut applied_indices = Vec::new();
        for (index, change) in prepared.changes.iter().enumerate() {
            verify_current(self.workspace, change, self.limits.max_file_bytes)?;
            let result = match change.new_content.as_ref() {
                Some(_) => {
                    let temp = staged.replacements[index]
                        .take()
                        .expect("every write has a staged file");
                    persist_temp(temp, &change.path)
                }
                None => fs::remove_file(&change.path),
            };

            if let Err(source) = result {
                let restore_result = restore_applied(
                    self.workspace,
                    &prepared.changes,
                    &applied_indices,
                    &mut staged.originals,
                );
                let directory_result = remove_created_directories(&created_directories);
                return match (restore_result, directory_result) {
                    (Ok(()), Ok(())) => Err(PatchError::ApplyFailed {
                        path: change.relative_path.clone(),
                        source,
                    }),
                    (rollback, directory_cleanup) => Err(PatchError::ApplyAndRestoreFailed {
                        path: change.relative_path.clone(),
                        apply: source.to_string(),
                        rollback: [rollback.err(), directory_cleanup.err()]
                            .into_iter()
                            .flatten()
                            .map(|error| error.to_string())
                            .collect::<Vec<_>>()
                            .join("; "),
                    }),
                };
            }
            applied_indices.push(index);
        }

        let rollback = RollbackRecord {
            id: Uuid::now_v7().to_string(),
            entries: prepared
                .changes
                .iter()
                .map(RollbackEntry::from_prepared)
                .collect(),
            created_directories,
        };
        let result = MutationResult {
            rollback_id: rollback.id.clone(),
            preview: prepared.preview,
            action: ActionResult::succeeded(true, true),
        };
        Ok(AppliedBatch { result, rollback })
    }

    pub fn rollback(&self, record: RollbackRecord) -> Result<RollbackResult, PatchError> {
        for entry in &record.entries {
            verify_applied(self.workspace, entry, self.limits.max_file_bytes)?;
        }
        let mut staged_originals = record
            .entries
            .iter()
            .map(|entry| {
                entry
                    .original_content
                    .as_ref()
                    .map(|content| {
                        stage_content(
                            self.workspace,
                            &entry.path,
                            content,
                            entry.original_permissions.clone(),
                        )
                        .map_err(|source| PatchError::RollbackIo {
                            path: entry.relative_path.clone(),
                            source,
                        })
                    })
                    .transpose()
            })
            .collect::<Result<Vec<_>, _>>()?;

        let restored_files = record
            .entries
            .iter()
            .map(|entry| entry.relative_path.clone())
            .collect::<Vec<_>>();
        for (index, entry) in record.entries.iter().enumerate().rev() {
            verify_applied(self.workspace, entry, self.limits.max_file_bytes)?;
            ensure_write_path(self.workspace, &entry.relative_path, &entry.path)?;
            match &entry.original_content {
                Some(_) => {
                    let temp = staged_originals[index]
                        .take()
                        .expect("every original file has a staged restore");
                    persist_temp(temp, &entry.path).map_err(|source| PatchError::RollbackIo {
                        path: entry.relative_path.clone(),
                        source,
                    })?;
                }
                None => {
                    fs::remove_file(&entry.path).map_err(|source| PatchError::RollbackIo {
                        path: entry.relative_path.clone(),
                        source,
                    })?;
                }
            }
        }
        remove_created_directories(&record.created_directories)?;

        Ok(RollbackResult {
            rollback_id: record.id,
            restored_files,
        })
    }

    fn prepare_change(&self, change: FileChange) -> Result<Vec<PreparedChange>, PatchError> {
        match change {
            FileChange::Create { path, content } => {
                let resolved = self.workspace.resolve_for_write(&path)?;
                let relative = self.relative(&resolved)?;
                reject_sensitive(&relative)?;
                if resolved.symlink_metadata().is_ok() {
                    return Err(PatchError::AlreadyExists(relative));
                }
                validate_content_size(&relative, content.len(), self.limits.max_file_bytes)?;
                reject_nul(&relative, content.as_bytes())?;
                Ok(vec![PreparedChange {
                    path: resolved,
                    relative_path: relative,
                    operation: MutationOperation::Create,
                    original_content: None,
                    original_permissions: None,
                    original_digest: None,
                    new_digest: Some(content_digest(content.as_bytes())),
                    new_content: Some(content.into_bytes()),
                }])
            }
            FileChange::Write {
                path,
                expected_digest,
                content,
            } => self
                .prepare_existing(path, expected_digest, MutationOperation::Write, |_, _| {
                    Ok(Some(content.into_bytes()))
                })
                .map(|change| vec![change]),
            FileChange::Replace {
                path,
                expected_digest,
                replacements,
            } => self
                .prepare_existing(
                    path,
                    expected_digest,
                    MutationOperation::Replace,
                    |relative, original| {
                        let original_text = std::str::from_utf8(original)
                            .map_err(|_| PatchError::Binary(relative.to_path_buf()))?;
                        let updated = apply_replacements(relative, original_text, &replacements)?;
                        Ok(Some(updated.into_bytes()))
                    },
                )
                .map(|change| vec![change]),
            FileChange::Delete {
                path,
                expected_digest,
            } => self
                .prepare_existing(path, expected_digest, MutationOperation::Delete, |_, _| {
                    Ok(None)
                })
                .map(|change| vec![change]),
            FileChange::Move {
                path,
                destination,
                expected_digest,
            } => self.prepare_move(path, destination, expected_digest),
        }
    }

    fn prepare_move(
        &self,
        path: PathBuf,
        destination: PathBuf,
        expected_digest: String,
    ) -> Result<Vec<PreparedChange>, PatchError> {
        let source = self.prepare_existing(
            path,
            expected_digest,
            MutationOperation::MoveFrom,
            |_, _| Ok(None),
        )?;
        let resolved = self.workspace.resolve_for_write(&destination)?;
        let relative = self.relative(&resolved)?;
        reject_sensitive(&relative)?;
        if resolved.symlink_metadata().is_ok() {
            return Err(PatchError::AlreadyExists(relative));
        }

        let destination = PreparedChange {
            path: resolved,
            relative_path: relative,
            operation: MutationOperation::MoveTo,
            original_content: None,
            original_permissions: source.original_permissions.clone(),
            original_digest: None,
            new_digest: source.original_digest.clone(),
            new_content: source.original_content.clone(),
        };
        Ok(vec![source, destination])
    }

    fn prepare_existing(
        &self,
        path: PathBuf,
        expected_digest: String,
        operation: MutationOperation,
        update: impl FnOnce(&Path, &[u8]) -> Result<Option<Vec<u8>>, PatchError>,
    ) -> Result<PreparedChange, PatchError> {
        let resolved = self.workspace.resolve_existing(&path)?;
        let relative = self.relative(&resolved)?;
        reject_sensitive(&relative)?;
        if !resolved.is_file() {
            return Err(PatchError::NotFile(relative));
        }
        let metadata = resolved.metadata().map_err(|source| PatchError::Io {
            path: relative.clone(),
            source,
        })?;
        let original = read_bounded(&resolved, self.limits.max_file_bytes, &relative)?;
        if original.contains(&0) || std::str::from_utf8(&original).is_err() {
            return Err(PatchError::Binary(relative));
        }
        let actual_digest = content_digest(&original);
        if expected_digest != actual_digest {
            return Err(PatchError::DigestConflict {
                path: relative,
                expected: expected_digest,
                actual: actual_digest,
            });
        }
        let new_content = update(&relative, &original)?;
        if new_content.as_deref() == Some(original.as_slice()) {
            return Err(PatchError::NoChanges(relative));
        }
        if let Some(content) = &new_content {
            validate_content_size(&relative, content.len(), self.limits.max_file_bytes)?;
            reject_nul(&relative, content)?;
        }

        Ok(PreparedChange {
            path: resolved,
            relative_path: relative,
            operation,
            original_permissions: Some(metadata.permissions()),
            original_digest: Some(actual_digest),
            new_digest: new_content.as_deref().map(content_digest),
            original_content: Some(original),
            new_content,
        })
    }

    fn relative(&self, path: &Path) -> Result<PathBuf, PatchError> {
        path.strip_prefix(self.workspace.root())
            .map(Path::to_path_buf)
            .map_err(|_| PatchError::OutsideWorkspace(path.to_path_buf()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PatchLimits {
    pub max_files: usize,
    pub max_file_bytes: usize,
    pub max_batch_bytes: usize,
}

impl Default for PatchLimits {
    fn default() -> Self {
        Self {
            max_files: 20,
            max_file_bytes: DEFAULT_PATCH_FILE_LIMIT,
            max_batch_bytes: DEFAULT_PATCH_BATCH_LIMIT,
        }
    }
}

impl PatchLimits {
    fn validate(self) -> Result<(), PatchError> {
        if self.max_files == 0 || self.max_files > 1_000 {
            return Err(PatchError::InvalidFileCountLimit(self.max_files));
        }
        if self.max_file_bytes == 0 || self.max_file_bytes > MAX_PATCH_FILE_LIMIT {
            return Err(PatchError::InvalidFileSizeLimit(self.max_file_bytes));
        }
        if self.max_batch_bytes == 0 || self.max_batch_bytes > MAX_PATCH_BATCH_LIMIT {
            return Err(PatchError::InvalidBatchSizeLimit(self.max_batch_bytes));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MutationBatch {
    pub changes: Vec<FileChange>,
}

impl MutationBatch {
    pub fn touched_file_count(&self) -> usize {
        self.changes.iter().fold(0usize, |count, change| {
            count.saturating_add(if matches!(change, FileChange::Move { .. }) {
                2
            } else {
                1
            })
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum FileChange {
    Create {
        path: PathBuf,
        content: String,
    },
    Write {
        path: PathBuf,
        expected_digest: String,
        content: String,
    },
    Replace {
        path: PathBuf,
        expected_digest: String,
        replacements: Vec<TextReplacement>,
    },
    Delete {
        path: PathBuf,
        expected_digest: String,
    },
    Move {
        path: PathBuf,
        destination: PathBuf,
        expected_digest: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextReplacement {
    pub old: String,
    pub new: String,
    #[serde(default)]
    pub replace_all: bool,
}

#[derive(Debug)]
pub struct PreparedBatch {
    pub preview: MutationPreview,
    changes: Vec<PreparedChange>,
}

#[derive(Debug)]
pub struct AppliedBatch {
    pub result: MutationResult,
    pub rollback: RollbackRecord,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MutationResult {
    pub rollback_id: String,
    pub preview: MutationPreview,
    #[serde(default)]
    pub action: ActionResult,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MutationPreview {
    pub files: Vec<FileMutationPreview>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileMutationPreview {
    pub path: PathBuf,
    pub operation: MutationOperation,
    pub original_digest: Option<String>,
    pub new_digest: Option<String>,
    pub diff: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MutationOperation {
    Create,
    Write,
    Replace,
    Delete,
    MoveFrom,
    MoveTo,
}

#[derive(Debug)]
struct PreparedChange {
    path: PathBuf,
    relative_path: PathBuf,
    operation: MutationOperation,
    original_content: Option<Vec<u8>>,
    original_permissions: Option<Permissions>,
    original_digest: Option<String>,
    new_content: Option<Vec<u8>>,
    new_digest: Option<String>,
}

impl PreparedChange {
    fn retained_bytes(&self) -> usize {
        self.original_content.as_ref().map_or(0, Vec::len)
            + self.new_content.as_ref().map_or(0, Vec::len)
    }

    fn preview(&self) -> FileMutationPreview {
        let old = self
            .original_content
            .as_deref()
            .and_then(|content| std::str::from_utf8(content).ok())
            .unwrap_or_default();
        let new = self
            .new_content
            .as_deref()
            .and_then(|content| std::str::from_utf8(content).ok())
            .unwrap_or_default();
        let path = self.relative_path.to_string_lossy();
        let diff = TextDiff::from_lines(old, new)
            .unified_diff()
            .context_radius(3)
            .header(&format!("a/{path}"), &format!("b/{path}"))
            .to_string();

        FileMutationPreview {
            path: self.relative_path.clone(),
            operation: self.operation,
            original_digest: self.original_digest.clone(),
            new_digest: self.new_digest.clone(),
            diff,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RollbackRecord {
    id: String,
    entries: Vec<RollbackEntry>,
    created_directories: Vec<PathBuf>,
}

impl RollbackRecord {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub(crate) fn retained_bytes(&self) -> usize {
        self.entries
            .iter()
            .filter_map(|entry| entry.original_content.as_ref())
            .map(Vec::len)
            .sum()
    }
}

#[derive(Debug, Clone)]
struct RollbackEntry {
    path: PathBuf,
    relative_path: PathBuf,
    original_content: Option<Vec<u8>>,
    original_permissions: Option<Permissions>,
    applied_digest: Option<String>,
}

impl RollbackEntry {
    fn from_prepared(change: &PreparedChange) -> Self {
        Self {
            path: change.path.clone(),
            relative_path: change.relative_path.clone(),
            original_content: change.original_content.clone(),
            original_permissions: change.original_permissions.clone(),
            applied_digest: change.new_digest.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RollbackResult {
    pub rollback_id: String,
    pub restored_files: Vec<PathBuf>,
}

fn apply_replacements(
    path: &Path,
    original: &str,
    replacements: &[TextReplacement],
) -> Result<String, PatchError> {
    if replacements.is_empty() {
        return Err(PatchError::EmptyReplacements(path.to_path_buf()));
    }
    let mut content = original.to_string();
    for replacement in replacements {
        if replacement.old.is_empty() {
            return Err(PatchError::EmptyNeedle(path.to_path_buf()));
        }
        let count = content.matches(&replacement.old).count();
        if count == 0 {
            return Err(PatchError::ReplacementNotFound {
                path: path.to_path_buf(),
                needle: replacement.old.clone(),
            });
        }
        if !replacement.replace_all && count != 1 {
            return Err(PatchError::AmbiguousReplacement {
                path: path.to_path_buf(),
                occurrences: count,
                needle: replacement.old.clone(),
            });
        }
        content = if replacement.replace_all {
            content.replace(&replacement.old, &replacement.new)
        } else {
            content.replacen(&replacement.old, &replacement.new, 1)
        };
    }
    Ok(content)
}

fn verify_current(
    workspace: &CodingWorkspace,
    change: &PreparedChange,
    max_bytes: usize,
) -> Result<(), PatchError> {
    ensure_write_path(workspace, &change.relative_path, &change.path)?;
    match &change.original_digest {
        Some(expected) => {
            let bytes = read_bounded(&change.path, max_bytes, &change.relative_path)?;
            let actual = content_digest(&bytes);
            if &actual == expected {
                Ok(())
            } else {
                Err(PatchError::DigestConflict {
                    path: change.relative_path.clone(),
                    expected: expected.clone(),
                    actual,
                })
            }
        }
        None => match change.path.symlink_metadata() {
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(PatchError::Io {
                path: change.relative_path.clone(),
                source: error,
            }),
            Ok(_) => Err(PatchError::AlreadyExists(change.relative_path.clone())),
        },
    }
}

fn verify_applied(
    workspace: &CodingWorkspace,
    entry: &RollbackEntry,
    max_bytes: usize,
) -> Result<(), PatchError> {
    ensure_write_path(workspace, &entry.relative_path, &entry.path)?;
    match &entry.applied_digest {
        Some(expected) => {
            let bytes = read_bounded(&entry.path, max_bytes, &entry.relative_path)?;
            let actual = content_digest(&bytes);
            if &actual == expected {
                Ok(())
            } else {
                Err(PatchError::RollbackConflict {
                    path: entry.relative_path.clone(),
                    expected: expected.clone(),
                    actual: Some(actual),
                })
            }
        }
        None => match entry.path.symlink_metadata() {
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(PatchError::Io {
                path: entry.relative_path.clone(),
                source: error,
            }),
            Ok(_) => Err(PatchError::RollbackConflict {
                path: entry.relative_path.clone(),
                expected: "absent".to_string(),
                actual: Some("present".to_string()),
            }),
        },
    }
}

struct StagedBatch {
    replacements: Vec<Option<NamedTempFile>>,
    originals: Vec<Option<NamedTempFile>>,
}

fn stage_changes(
    workspace: &CodingWorkspace,
    changes: &[PreparedChange],
) -> Result<StagedBatch, PatchError> {
    let replacements = changes
        .iter()
        .map(|change| {
            change
                .new_content
                .as_ref()
                .map(|content| {
                    stage_content(
                        workspace,
                        &change.path,
                        content,
                        change.original_permissions.clone(),
                    )
                    .map_err(|error| PatchError::StageFailed {
                        path: change.relative_path.clone(),
                        reason: error.to_string(),
                    })
                })
                .transpose()
        })
        .collect::<Result<Vec<_>, _>>()?;
    let originals = changes
        .iter()
        .map(|change| {
            change
                .original_content
                .as_ref()
                .map(|content| {
                    stage_content(
                        workspace,
                        &change.path,
                        content,
                        change.original_permissions.clone(),
                    )
                    .map_err(|error| PatchError::StageFailed {
                        path: change.relative_path.clone(),
                        reason: error.to_string(),
                    })
                })
                .transpose()
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(StagedBatch {
        replacements,
        originals,
    })
}

fn stage_content(
    workspace: &CodingWorkspace,
    destination: &Path,
    content: &[u8],
    permissions: Option<Permissions>,
) -> io::Result<NamedTempFile> {
    let destination_parent = destination
        .parent()
        .ok_or_else(|| io::Error::other("destination has no parent"))?;
    let staging_directory = if destination_parent.is_dir() {
        destination_parent
    } else {
        workspace.root()
    };
    let mut temp = NamedTempFile::new_in(staging_directory)?;
    temp.write_all(content)?;
    temp.as_file_mut().flush()?;
    temp.as_file().sync_all()?;
    if let Some(permissions) = permissions {
        temp.as_file().set_permissions(permissions)?;
    } else {
        set_new_file_permissions(temp.as_file())?;
    }
    Ok(temp)
}

#[cfg(unix)]
fn set_new_file_permissions(file: &File) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    file.set_permissions(Permissions::from_mode(0o644))
}

#[cfg(not(unix))]
fn set_new_file_permissions(_file: &File) -> io::Result<()> {
    Ok(())
}

fn persist_temp(temp: NamedTempFile, destination: &Path) -> io::Result<()> {
    temp.persist(destination)
        .map(|_| ())
        .map_err(|error| error.error)
}

fn restore_applied(
    workspace: &CodingWorkspace,
    changes: &[PreparedChange],
    applied_indices: &[usize],
    staged_originals: &mut [Option<NamedTempFile>],
) -> Result<(), PatchError> {
    for index in applied_indices.iter().rev() {
        let change = &changes[*index];
        ensure_write_path(workspace, &change.relative_path, &change.path)?;
        match &change.original_content {
            Some(_) => {
                let temp = staged_originals[*index]
                    .take()
                    .expect("every original file has a staged restore");
                persist_temp(temp, &change.path).map_err(|source| PatchError::RollbackIo {
                    path: change.relative_path.clone(),
                    source,
                })?;
            }
            None => {
                if change.path.exists() {
                    fs::remove_file(&change.path).map_err(|source| PatchError::RollbackIo {
                        path: change.relative_path.clone(),
                        source,
                    })?;
                }
            }
        }
    }
    Ok(())
}

fn ensure_write_path(
    workspace: &CodingWorkspace,
    relative: &Path,
    expected: &Path,
) -> Result<(), PatchError> {
    let resolved = workspace.resolve_for_write(relative)?;
    if resolved == expected {
        Ok(())
    } else {
        Err(PatchError::PathChanged {
            path: relative.to_path_buf(),
            expected: expected.to_path_buf(),
            actual: resolved,
        })
    }
}

fn create_missing_parent_directories(
    workspace: &CodingWorkspace,
    changes: &[PreparedChange],
) -> Result<Vec<PathBuf>, PatchError> {
    let mut created = Vec::new();

    for change in changes.iter().filter(|change| change.new_content.is_some()) {
        let Some(parent) = change.path.parent() else {
            continue;
        };
        let mut cursor = parent.to_path_buf();
        let mut missing = Vec::new();
        while cursor != workspace.root() {
            match cursor.symlink_metadata() {
                Ok(metadata) => {
                    if !metadata.is_dir() {
                        remove_created_directories(&created)?;
                        return Err(PatchError::MissingParent(change.relative_path.clone()));
                    }
                    if let Err(error) = workspace.resolve_existing(&cursor) {
                        remove_created_directories(&created)?;
                        return Err(error.into());
                    }
                    break;
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    missing.push(cursor.clone());
                    if !cursor.pop() {
                        remove_created_directories(&created)?;
                        return Err(PatchError::MissingParent(change.relative_path.clone()));
                    }
                }
                Err(source) => {
                    remove_created_directories(&created)?;
                    return Err(PatchError::ParentDirectoryIo {
                        path: change.relative_path.clone(),
                        source,
                    });
                }
            }
        }

        for directory in missing.into_iter().rev() {
            match fs::create_dir(&directory) {
                Ok(()) => created.push(directory.clone()),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(source) => {
                    remove_created_directories(&created)?;
                    return Err(PatchError::ParentDirectoryIo {
                        path: change.relative_path.clone(),
                        source,
                    });
                }
            }
            if let Err(error) = workspace.resolve_existing(&directory) {
                remove_created_directories(&created)?;
                return Err(error.into());
            }
        }
    }

    Ok(created)
}

fn remove_created_directories(directories: &[PathBuf]) -> Result<(), PatchError> {
    for directory in directories.iter().rev() {
        match fs::remove_dir(directory) {
            Ok(()) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::DirectoryNotEmpty
                ) => {}
            Err(source) => {
                return Err(PatchError::ParentDirectoryCleanup {
                    path: directory.clone(),
                    source,
                });
            }
        }
    }
    Ok(())
}

fn reject_sensitive(path: &Path) -> Result<(), PatchError> {
    if is_sensitive_path(path) {
        Err(PatchError::Sensitive(path.to_path_buf()))
    } else {
        Ok(())
    }
}

fn reject_nul(path: &Path, content: &[u8]) -> Result<(), PatchError> {
    if content.contains(&0) {
        Err(PatchError::Binary(path.to_path_buf()))
    } else {
        Ok(())
    }
}

fn validate_content_size(path: &Path, size: usize, limit: usize) -> Result<(), PatchError> {
    if size <= limit {
        Ok(())
    } else {
        Err(PatchError::FileTooLarge {
            path: path.to_path_buf(),
            size,
            limit,
        })
    }
}

fn read_bounded(path: &Path, limit: usize, relative: &Path) -> Result<Vec<u8>, PatchError> {
    let file = File::open(path).map_err(|source| PatchError::Io {
        path: relative.to_path_buf(),
        source,
    })?;
    let mut bytes = Vec::with_capacity(limit.min(64 * 1_024));
    file.take((limit + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|source| PatchError::Io {
            path: relative.to_path_buf(),
            source,
        })?;
    if bytes.len() > limit {
        return Err(PatchError::FileTooLarge {
            path: relative.to_path_buf(),
            size: bytes.len(),
            limit,
        });
    }
    Ok(bytes)
}

#[derive(Debug, thiserror::Error)]
pub enum PatchError {
    #[error(transparent)]
    Workspace(#[from] WorkspaceError),
    #[error("path is outside the coding workspace: {0}")]
    OutsideWorkspace(PathBuf),
    #[error("mutation batch must contain at least one file")]
    EmptyBatch,
    #[error("mutation batch contains {count} files, limit is {limit}")]
    TooManyFiles { count: usize, limit: usize },
    #[error("invalid patch file-count limit: {0}")]
    InvalidFileCountLimit(usize),
    #[error("invalid patch file-size limit: {0}")]
    InvalidFileSizeLimit(usize),
    #[error("invalid patch batch-size limit: {0}")]
    InvalidBatchSizeLimit(usize),
    #[error("mutation batch retains {size} bytes, limit is {limit}")]
    BatchTooLarge { size: usize, limit: usize },
    #[error("duplicate mutation path in batch: {0}")]
    DuplicatePath(PathBuf),
    #[error("resolved path changed for {path}: expected {expected}, current {actual}")]
    PathChanged {
        path: PathBuf,
        expected: PathBuf,
        actual: PathBuf,
    },
    #[error("file already exists: {0}")]
    AlreadyExists(PathBuf),
    #[error("path is not a regular file: {0}")]
    NotFile(PathBuf),
    #[error("parent path cannot be used as a directory: {0}")]
    MissingParent(PathBuf),
    #[error("could not create a parent directory for {path}: {source}")]
    ParentDirectoryIo {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("could not clean up generated directory {path}: {source}")]
    ParentDirectoryCleanup {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("sensitive files cannot be mutated by the coding agent: {0}")]
    Sensitive(PathBuf),
    #[error("binary or non-UTF-8 files cannot be patched: {0}")]
    Binary(PathBuf),
    #[error("file {path} is {size} bytes, limit is {limit}")]
    FileTooLarge {
        path: PathBuf,
        size: usize,
        limit: usize,
    },
    #[error("digest conflict for {path}: expected {expected}, current {actual}")]
    DigestConflict {
        path: PathBuf,
        expected: String,
        actual: String,
    },
    #[error("replacement list is empty for {0}")]
    EmptyReplacements(PathBuf),
    #[error("replacement needle is empty for {0}")]
    EmptyNeedle(PathBuf),
    #[error("replacement text was not found in {path}: {needle:?}")]
    ReplacementNotFound { path: PathBuf, needle: String },
    #[error(
        "replacement text occurs {occurrences} times in {path}; set replace_all or use more context: {needle:?}"
    )]
    AmbiguousReplacement {
        path: PathBuf,
        occurrences: usize,
        needle: String,
    },
    #[error("mutation produces no changes for {0}")]
    NoChanges(PathBuf),
    #[error("could not access {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("could not stage {path}: {reason}")]
    StageFailed { path: PathBuf, reason: String },
    #[error("could not apply mutation to {path}: {source}")]
    ApplyFailed {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("apply failed for {path}: {apply}; restoring prior files also failed: {rollback}")]
    ApplyAndRestoreFailed {
        path: PathBuf,
        apply: String,
        rollback: String,
    },
    #[error("rollback conflict for {path}: expected {expected}, current {actual:?}")]
    RollbackConflict {
        path: PathBuf,
        expected: String,
        actual: Option<String>,
    },
    #[error("could not roll back {path}: {source}")]
    RollbackIo {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

impl PatchError {
    pub(crate) fn failure_kind(&self) -> ActionFailureKind {
        match self {
            Self::DigestConflict { .. }
            | Self::PathChanged { .. }
            | Self::RollbackConflict { .. } => ActionFailureKind::StaleState,
            Self::Workspace(error) => error.failure_kind(),
            Self::OutsideWorkspace(_) | Self::Sensitive(_) => ActionFailureKind::PolicyBlocked,
            Self::NotFile(_) | Self::MissingParent(_) => ActionFailureKind::ResourceMissing,
            Self::AlreadyExists(_) => ActionFailureKind::StaleState,
            Self::ParentDirectoryIo { .. }
            | Self::ParentDirectoryCleanup { .. }
            | Self::Io { .. }
            | Self::StageFailed { .. }
            | Self::ApplyFailed { .. }
            | Self::RollbackIo { .. } => ActionFailureKind::TransientFailure,
            Self::ApplyAndRestoreFailed { .. } => ActionFailureKind::InternalFailure,
            _ => ActionFailureKind::InvalidArguments,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, CodingWorkspace) {
        let temp_dir = tempfile::tempdir().unwrap();
        fs::write(
            temp_dir.path().join("app.py"),
            "def value():\n    return 'before'\n",
        )
        .unwrap();
        fs::write(temp_dir.path().join("delete.txt"), "remove me\n").unwrap();
        let workspace = CodingWorkspace::new(temp_dir.path()).unwrap();
        (temp_dir, workspace)
    }

    fn digest(path: &Path) -> String {
        content_digest(&fs::read(path).unwrap())
    }

    #[test]
    fn previews_applies_and_rolls_back_a_multi_file_batch() {
        let (_temp_dir, workspace) = fixture();
        let engine = PatchEngine::new(&workspace, PatchLimits::default());
        let app_digest = digest(&workspace.root().join("app.py"));
        let delete_digest = digest(&workspace.root().join("delete.txt"));
        let batch = MutationBatch {
            changes: vec![
                FileChange::Replace {
                    path: PathBuf::from("app.py"),
                    expected_digest: app_digest,
                    replacements: vec![TextReplacement {
                        old: "'before'".to_string(),
                        new: "'after'".to_string(),
                        replace_all: false,
                    }],
                },
                FileChange::Create {
                    path: PathBuf::from("new.rs"),
                    content: "pub fn created() {}\n".to_string(),
                },
                FileChange::Delete {
                    path: PathBuf::from("delete.txt"),
                    expected_digest: delete_digest,
                },
            ],
        };

        let prepared = engine.prepare(batch).unwrap();
        assert!(fs::read_to_string(workspace.root().join("app.py"))
            .unwrap()
            .contains("before"));
        assert!(!workspace.root().join("new.rs").exists());
        assert!(prepared.preview.files[0]
            .diff
            .contains("-    return 'before'"));
        assert!(prepared.preview.files[0]
            .diff
            .contains("+    return 'after'"));

        let applied = engine.apply(prepared).unwrap();
        assert_eq!(applied.result.action, ActionResult::succeeded(true, true));
        assert!(fs::read_to_string(workspace.root().join("app.py"))
            .unwrap()
            .contains("after"));
        assert!(workspace.root().join("new.rs").exists());
        assert!(!workspace.root().join("delete.txt").exists());

        let rollback_id = applied.result.rollback_id.clone();
        let rolled_back = engine.rollback(applied.rollback).unwrap();
        assert_eq!(rolled_back.rollback_id, rollback_id);
        assert!(fs::read_to_string(workspace.root().join("app.py"))
            .unwrap()
            .contains("before"));
        assert!(!workspace.root().join("new.rs").exists());
        assert!(workspace.root().join("delete.txt").exists());
    }

    #[test]
    fn creates_and_rolls_back_missing_parent_directories() {
        let (_temp_dir, workspace) = fixture();
        let engine = PatchEngine::new(&workspace, PatchLimits::default());
        let prepared = engine
            .prepare(MutationBatch {
                changes: vec![
                    FileChange::Create {
                        path: PathBuf::from("package/src/textslug/__init__.py"),
                        content: "def slugify(value):\n    return value\n".to_string(),
                    },
                    FileChange::Create {
                        path: PathBuf::from("package/tests/test_textslug.py"),
                        content: "def test_placeholder():\n    assert True\n".to_string(),
                    },
                ],
            })
            .unwrap();

        assert!(!workspace.root().join("package").exists());
        let applied = engine.apply(prepared).unwrap();
        assert!(workspace
            .root()
            .join("package/src/textslug/__init__.py")
            .is_file());
        assert!(workspace
            .root()
            .join("package/tests/test_textslug.py")
            .is_file());

        engine.rollback(applied.rollback).unwrap();

        assert!(!workspace.root().join("package").exists());
    }

    #[test]
    fn rollback_preserves_later_content_in_generated_directories() {
        let (_temp_dir, workspace) = fixture();
        let engine = PatchEngine::new(&workspace, PatchLimits::default());
        let prepared = engine
            .prepare(MutationBatch {
                changes: vec![FileChange::Create {
                    path: PathBuf::from("package/src/generated.py"),
                    content: "GENERATED = True\n".to_string(),
                }],
            })
            .unwrap();
        let applied = engine.apply(prepared).unwrap();
        fs::write(workspace.root().join("package/user.txt"), "keep\n").unwrap();

        engine.rollback(applied.rollback).unwrap();

        assert!(!workspace.root().join("package/src/generated.py").exists());
        assert_eq!(
            fs::read_to_string(workspace.root().join("package/user.txt")).unwrap(),
            "keep\n"
        );
    }

    #[test]
    fn failed_batch_removes_parent_directories_it_created() {
        let (_temp_dir, workspace) = fixture();
        let engine = PatchEngine::new(&workspace, PatchLimits::default());
        let prepared = engine
            .prepare(MutationBatch {
                changes: vec![
                    FileChange::Create {
                        path: PathBuf::from("nested/file.txt"),
                        content: "file\n".to_string(),
                    },
                    FileChange::Create {
                        path: PathBuf::from("nested"),
                        content: "conflict\n".to_string(),
                    },
                ],
            })
            .unwrap();

        assert!(matches!(
            engine.apply(prepared),
            Err(PatchError::AlreadyExists(path)) if path == Path::new("nested")
        ));
        assert!(!workspace.root().join("nested").exists());
    }

    #[test]
    fn moves_and_rolls_back_a_versioned_file_as_one_batch() {
        let (_temp_dir, workspace) = fixture();
        fs::create_dir(workspace.root().join("moved")).unwrap();
        let source = workspace.root().join("app.py");
        let engine = PatchEngine::new(&workspace, PatchLimits::default());
        let prepared = engine
            .prepare(MutationBatch {
                changes: vec![FileChange::Move {
                    path: PathBuf::from("app.py"),
                    destination: PathBuf::from("moved/app.py"),
                    expected_digest: digest(&source),
                }],
            })
            .unwrap();

        assert_eq!(prepared.preview.files.len(), 2);
        assert_eq!(
            prepared.preview.files[0].operation,
            MutationOperation::MoveFrom
        );
        assert_eq!(
            prepared.preview.files[1].operation,
            MutationOperation::MoveTo
        );
        assert!(source.exists());
        assert!(!workspace.root().join("moved/app.py").exists());

        let applied = engine.apply(prepared).unwrap();

        assert!(!source.exists());
        assert!(fs::read_to_string(workspace.root().join("moved/app.py"))
            .unwrap()
            .contains("before"));

        engine.rollback(applied.rollback).unwrap();

        assert!(fs::read_to_string(source).unwrap().contains("before"));
        assert!(!workspace.root().join("moved/app.py").exists());
    }

    #[test]
    fn moves_into_and_rolls_back_missing_parent_directories() {
        let (_temp_dir, workspace) = fixture();
        let source = workspace.root().join("app.py");
        let engine = PatchEngine::new(&workspace, PatchLimits::default());
        let prepared = engine
            .prepare(MutationBatch {
                changes: vec![FileChange::Move {
                    path: PathBuf::from("app.py"),
                    destination: PathBuf::from("package/src/app.py"),
                    expected_digest: digest(&source),
                }],
            })
            .unwrap();

        let applied = engine.apply(prepared).unwrap();
        assert!(!source.exists());
        assert!(workspace.root().join("package/src/app.py").is_file());

        engine.rollback(applied.rollback).unwrap();

        assert!(source.is_file());
        assert!(!workspace.root().join("package").exists());
    }

    #[test]
    fn move_refuses_destination_collisions_and_post_preview_changes() {
        let (_temp_dir, workspace) = fixture();
        fs::create_dir(workspace.root().join("moved")).unwrap();
        let source = workspace.root().join("app.py");
        let destination = workspace.root().join("moved/app.py");
        let engine = PatchEngine::new(&workspace, PatchLimits::default());
        let change = || MutationBatch {
            changes: vec![FileChange::Move {
                path: PathBuf::from("app.py"),
                destination: PathBuf::from("moved/app.py"),
                expected_digest: digest(&source),
            }],
        };

        fs::write(&destination, "occupied\n").unwrap();
        assert!(matches!(
            engine.prepare(change()),
            Err(PatchError::AlreadyExists(path)) if path == Path::new("moved/app.py")
        ));
        fs::remove_file(&destination).unwrap();

        let prepared = engine.prepare(change()).unwrap();
        fs::write(&destination, "created later\n").unwrap();
        assert!(matches!(
            engine.apply(prepared),
            Err(PatchError::AlreadyExists(path)) if path == Path::new("moved/app.py")
        ));
        assert!(source.exists());
        assert_eq!(fs::read_to_string(destination).unwrap(), "created later\n");
    }

    #[test]
    fn move_counts_both_touched_paths_against_the_batch_limit() {
        let (_temp_dir, workspace) = fixture();
        let source = workspace.root().join("app.py");
        let engine = PatchEngine::new(
            &workspace,
            PatchLimits {
                max_files: 1,
                ..PatchLimits::default()
            },
        );

        let error = engine
            .prepare(MutationBatch {
                changes: vec![FileChange::Move {
                    path: PathBuf::from("app.py"),
                    destination: PathBuf::from("renamed.py"),
                    expected_digest: digest(&source),
                }],
            })
            .unwrap_err();

        assert!(matches!(
            error,
            PatchError::TooManyFiles { count: 2, limit: 1 }
        ));
    }

    #[test]
    fn detects_changes_between_prepare_and_apply() {
        let (_temp_dir, workspace) = fixture();
        let engine = PatchEngine::new(&workspace, PatchLimits::default());
        let path = workspace.root().join("app.py");
        let prepared = engine
            .prepare(MutationBatch {
                changes: vec![FileChange::Write {
                    path: PathBuf::from("app.py"),
                    expected_digest: digest(&path),
                    content: "replacement\n".to_string(),
                }],
            })
            .unwrap();
        fs::write(&path, "changed by another process\n").unwrap();

        let error = engine.apply(prepared).unwrap_err();

        assert!(matches!(error, PatchError::DigestConflict { .. }));
        assert_eq!(
            fs::read_to_string(path).unwrap(),
            "changed by another process\n"
        );
    }

    #[test]
    fn rollback_refuses_to_overwrite_later_changes() {
        let (_temp_dir, workspace) = fixture();
        let engine = PatchEngine::new(&workspace, PatchLimits::default());
        let path = workspace.root().join("app.py");
        let prepared = engine
            .prepare(MutationBatch {
                changes: vec![FileChange::Write {
                    path: PathBuf::from("app.py"),
                    expected_digest: digest(&path),
                    content: "applied\n".to_string(),
                }],
            })
            .unwrap();
        let applied = engine.apply(prepared).unwrap();
        fs::write(&path, "later change\n").unwrap();

        let error = engine.rollback(applied.rollback).unwrap_err();

        assert!(matches!(error, PatchError::RollbackConflict { .. }));
        assert_eq!(fs::read_to_string(path).unwrap(), "later change\n");
    }

    #[test]
    fn invalid_change_prevents_every_file_in_the_batch() {
        let (_temp_dir, workspace) = fixture();
        let engine = PatchEngine::new(&workspace, PatchLimits::default());
        let app_path = workspace.root().join("app.py");
        let batch = MutationBatch {
            changes: vec![
                FileChange::Write {
                    path: PathBuf::from("app.py"),
                    expected_digest: digest(&app_path),
                    content: "would change\n".to_string(),
                },
                FileChange::Write {
                    path: PathBuf::from("delete.txt"),
                    expected_digest: "blake3:wrong".to_string(),
                    content: "invalid\n".to_string(),
                },
            ],
        };

        let error = engine.prepare(batch).unwrap_err();

        assert!(matches!(error, PatchError::DigestConflict { .. }));
        assert!(fs::read_to_string(app_path).unwrap().contains("before"));
    }

    #[test]
    fn rejects_batches_that_exceed_the_total_retained_byte_limit() {
        let (_temp_dir, workspace) = fixture();
        let engine = PatchEngine::new(
            &workspace,
            PatchLimits {
                max_batch_bytes: 10,
                ..PatchLimits::default()
            },
        );

        let error = engine
            .prepare(MutationBatch {
                changes: vec![FileChange::Create {
                    path: PathBuf::from("large.txt"),
                    content: "more than ten bytes".to_string(),
                }],
            })
            .unwrap_err();

        assert!(matches!(
            error,
            PatchError::BatchTooLarge {
                size: 19,
                limit: 10
            }
        ));
        assert!(!workspace.root().join("large.txt").exists());
    }

    #[test]
    fn exact_replacements_must_be_unique_unless_explicitly_all() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("values.txt");
        fs::write(&path, "same\nsame\n").unwrap();
        let workspace = CodingWorkspace::new(temp_dir.path()).unwrap();
        let engine = PatchEngine::new(&workspace, PatchLimits::default());
        let change = |replace_all| MutationBatch {
            changes: vec![FileChange::Replace {
                path: PathBuf::from("values.txt"),
                expected_digest: digest(&path),
                replacements: vec![TextReplacement {
                    old: "same".to_string(),
                    new: "changed".to_string(),
                    replace_all,
                }],
            }],
        };

        assert!(matches!(
            engine.prepare(change(false)),
            Err(PatchError::AmbiguousReplacement { occurrences: 2, .. })
        ));
        let applied = engine.apply(engine.prepare(change(true)).unwrap()).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "changed\nchanged\n");
        engine.rollback(applied.rollback).unwrap();
    }

    #[test]
    fn rejects_duplicate_sensitive_and_external_paths() {
        let (_temp_dir, workspace) = fixture();
        let external = tempfile::tempdir().unwrap();
        let engine = PatchEngine::new(&workspace, PatchLimits::default());
        let duplicate = MutationBatch {
            changes: vec![
                FileChange::Create {
                    path: PathBuf::from("first.txt"),
                    content: "one".to_string(),
                },
                FileChange::Create {
                    path: PathBuf::from("first.txt"),
                    content: "two".to_string(),
                },
            ],
        };

        assert!(matches!(
            engine.prepare(duplicate),
            Err(PatchError::DuplicatePath(_))
        ));
        assert!(matches!(
            engine.prepare(MutationBatch {
                changes: vec![FileChange::Create {
                    path: PathBuf::from(".env"),
                    content: "TOKEN=secret".to_string(),
                }]
            }),
            Err(PatchError::Sensitive(_))
        ));
        assert!(matches!(
            engine.prepare(MutationBatch {
                changes: vec![FileChange::Create {
                    path: external.path().join("outside.txt"),
                    content: "outside".to_string(),
                }]
            }),
            Err(PatchError::Workspace(WorkspaceError::OutsideWorkspace(_)))
        ));
    }

    #[test]
    fn rejects_binary_files_even_with_a_matching_digest() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("binary.dat");
        fs::write(&path, b"text\0binary").unwrap();
        let workspace = CodingWorkspace::new(temp_dir.path()).unwrap();
        let engine = PatchEngine::new(&workspace, PatchLimits::default());

        let error = engine
            .prepare(MutationBatch {
                changes: vec![FileChange::Write {
                    path: PathBuf::from("binary.dat"),
                    expected_digest: digest(&path),
                    content: "replacement".to_string(),
                }],
            })
            .unwrap_err();

        assert!(matches!(error, PatchError::Binary(_)));
        assert_eq!(fs::read(path).unwrap(), b"text\0binary");
    }

    #[test]
    fn rejects_new_content_containing_nul_bytes() {
        let (_temp_dir, workspace) = fixture();
        let engine = PatchEngine::new(&workspace, PatchLimits::default());
        let existing = workspace.root().join("app.py");

        let create_error = engine
            .prepare(MutationBatch {
                changes: vec![FileChange::Create {
                    path: PathBuf::from("binary.dat"),
                    content: "text\0binary".to_string(),
                }],
            })
            .unwrap_err();
        let write_error = engine
            .prepare(MutationBatch {
                changes: vec![FileChange::Write {
                    path: PathBuf::from("app.py"),
                    expected_digest: digest(&existing),
                    content: "text\0binary".to_string(),
                }],
            })
            .unwrap_err();

        assert!(matches!(create_error, PatchError::Binary(_)));
        assert!(matches!(write_error, PatchError::Binary(_)));
        assert!(!workspace.root().join("binary.dat").exists());
        assert!(fs::read_to_string(existing).unwrap().contains("before"));
    }

    #[cfg(unix)]
    #[test]
    fn revalidates_paths_after_a_parent_is_replaced_by_a_symlink() {
        use std::os::unix::fs::symlink;

        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path().join("workspace");
        let source = root.join("source");
        let outside = temp_dir.path().join("outside");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir(&outside).unwrap();
        let original_path = source.join("app.py");
        fs::write(&original_path, "original\n").unwrap();
        let outside_path = outside.join("app.py");
        fs::write(&outside_path, "outside\n").unwrap();
        let workspace = CodingWorkspace::new(&root).unwrap();
        let engine = PatchEngine::new(&workspace, PatchLimits::default());
        let prepared = engine
            .prepare(MutationBatch {
                changes: vec![FileChange::Write {
                    path: PathBuf::from("source/app.py"),
                    expected_digest: digest(&original_path),
                    content: "replacement\n".to_string(),
                }],
            })
            .unwrap();

        fs::rename(&source, root.join("moved-source")).unwrap();
        symlink(&outside, &source).unwrap();
        let error = engine.apply(prepared).unwrap_err();

        assert!(matches!(
            error,
            PatchError::Workspace(WorkspaceError::OutsideWorkspace(_))
        ));
        assert_eq!(fs::read_to_string(outside_path).unwrap(), "outside\n");
        assert_eq!(
            fs::read_to_string(root.join("moved-source/app.py")).unwrap(),
            "original\n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn rejects_a_future_parent_replaced_by_an_escaping_symlink() {
        use std::os::unix::fs::symlink;

        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path().join("workspace");
        let outside = temp_dir.path().join("outside");
        fs::create_dir(&root).unwrap();
        fs::create_dir(&outside).unwrap();
        let workspace = CodingWorkspace::new(&root).unwrap();
        let engine = PatchEngine::new(&workspace, PatchLimits::default());
        let prepared = engine
            .prepare(MutationBatch {
                changes: vec![FileChange::Create {
                    path: PathBuf::from("future/file.txt"),
                    content: "inside\n".to_string(),
                }],
            })
            .unwrap();

        symlink(&outside, root.join("future")).unwrap();
        let error = engine.apply(prepared).unwrap_err();

        assert!(matches!(
            error,
            PatchError::Workspace(WorkspaceError::OutsideWorkspace(_))
        ));
        assert!(!outside.join("file.txt").exists());
    }

    #[cfg(unix)]
    #[test]
    fn preserves_existing_file_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let (_temp_dir, workspace) = fixture();
        let path = workspace.root().join("app.py");
        fs::set_permissions(&path, Permissions::from_mode(0o755)).unwrap();
        let engine = PatchEngine::new(&workspace, PatchLimits::default());
        let prepared = engine
            .prepare(MutationBatch {
                changes: vec![FileChange::Write {
                    path: PathBuf::from("app.py"),
                    expected_digest: digest(&path),
                    content: "#!/usr/bin/env python3\n".to_string(),
                }],
            })
            .unwrap();

        let applied = engine.apply(prepared).unwrap();

        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o755
        );
        engine.rollback(applied.rollback).unwrap();
    }
}
