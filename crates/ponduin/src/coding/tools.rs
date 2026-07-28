use crate::coding::config::CodingConfig;
use crate::coding::file::{
    FileReadOptions, FileSnapshot, DEFAULT_READ_LIMIT, MAX_READ_LIMIT, MIN_READ_LIMIT,
};
use crate::coding::git::{GitDiff, GitDiffRequest, GitLimits, GitOwnedPath, GitRepository};
use crate::coding::intelligence::{IntelligenceLimits, RepositoryIndex, RepositoryIntelligence};
use crate::coding::patch::{
    MutationBatch, MutationPreview, MutationResult, PatchEngine, PatchLimits, RollbackRecord,
    DEFAULT_PATCH_BATCH_LIMIT, MAX_PATCH_FILE_LIMIT,
};
use crate::coding::process::{ProcessLimits, ProcessOutput, ProcessRequest, ProcessRunner};
use crate::coding::project::ProjectDiscovery;
use crate::coding::search::{SearchLimits, TextSearchRequest};
use crate::coding::{CodingWorkspace, RepositoryInstructions, RepositoryProfile, RepositorySearch};
use rmcp::model::{
    CallToolRequestParams, CallToolResult, ContentBlock, ErrorCode, ErrorData, Tool,
    ToolAnnotations,
};
use rmcp::object;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub const CODING_TOOL_PREFIX: &str = "coding__";
pub const REPOSITORY_PROFILE_TOOL_NAME: &str = "coding__repository_profile";
pub const REPOSITORY_INSTRUCTIONS_TOOL_NAME: &str = "coding__repository_instructions";
pub const FIND_FILES_TOOL_NAME: &str = "coding__find_files";
pub const SEARCH_TEXT_TOOL_NAME: &str = "coding__search_text";
pub const READ_FILE_TOOL_NAME: &str = "coding__read_file";
pub const PREVIEW_CHANGES_TOOL_NAME: &str = "coding__preview_changes";
pub const APPLY_CHANGES_TOOL_NAME: &str = "coding__apply_changes";
pub const ROLLBACK_CHANGES_TOOL_NAME: &str = "coding__rollback_changes";
pub const RUN_PROCESS_TOOL_NAME: &str = "coding__run_process";
pub const GIT_STATUS_TOOL_NAME: &str = "coding__git_status";
pub const GIT_DIFF_TOOL_NAME: &str = "coding__git_diff";
pub const GIT_HISTORY_TOOL_NAME: &str = "coding__git_history";
pub const GIT_STAGE_OWNED_TOOL_NAME: &str = "coding__git_stage_owned";
pub const GIT_UNSTAGE_OWNED_TOOL_NAME: &str = "coding__git_unstage_owned";
pub const GIT_COMMIT_OWNED_TOOL_NAME: &str = "coding__git_commit_owned";
pub const GIT_CREATE_BRANCH_TOOL_NAME: &str = "coding__git_create_branch";
pub const GIT_PUSH_OWNED_TOOL_NAME: &str = "coding__git_push_owned";
pub const REPOSITORY_MAP_TOOL_NAME: &str = "coding__repository_map";
pub const SEARCH_SYMBOLS_TOOL_NAME: &str = "coding__search_symbols";
pub const FIND_REFERENCES_TOOL_NAME: &str = "coding__find_references";
pub const SELECT_CONTEXT_TOOL_NAME: &str = "coding__select_context";
pub const PROJECT_CAPABILITIES_TOOL_NAME: &str = "coding__project_capabilities";

const DEFAULT_REPOSITORY_FILE_LIMIT: usize = 50_000;
const MAX_REPOSITORY_FILE_LIMIT: usize = 100_000;
const MAX_ROLLBACK_RECORDS: usize = 20;
const MAX_ROLLBACK_BYTES: usize = 64 * 1_024 * 1_024;
const MAX_INTELLIGENCE_CACHE_ENTRIES: usize = 4;

#[derive(Debug, Default)]
pub(crate) struct CodingToolState {
    rollback_journal: Mutex<VecDeque<RollbackJournalEntry>>,
    committed: Mutex<VecDeque<OwnedCommit>>,
    intelligence_cache: Mutex<VecDeque<IntelligenceCacheEntry>>,
}

#[derive(Debug)]
struct RollbackJournalEntry {
    record: RollbackRecord,
    workspace_root: PathBuf,
    owned_paths: Vec<GitOwnedPath>,
    staged_paths: BTreeSet<PathBuf>,
}

#[derive(Debug)]
struct OwnedCommit {
    workspace_root: PathBuf,
    oid: String,
}

#[derive(Debug)]
struct IntelligenceCacheEntry {
    workspace_root: PathBuf,
    limits: IntelligenceLimits,
    index: Arc<RepositoryIndex>,
}

impl CodingToolState {
    fn remember(&self, workspace_root: &Path, record: RollbackRecord, preview: &MutationPreview) {
        let mut journal = self
            .rollback_journal
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        journal.push_back(RollbackJournalEntry {
            record,
            workspace_root: workspace_root.to_path_buf(),
            owned_paths: preview
                .files
                .iter()
                .map(|file| GitOwnedPath {
                    path: file.path.clone(),
                    original_digest: file.original_digest.clone(),
                    applied_digest: file.new_digest.clone(),
                })
                .collect(),
            staged_paths: BTreeSet::new(),
        });
        while journal.len() > MAX_ROLLBACK_RECORDS
            || journal
                .iter()
                .map(|entry| entry.record.retained_bytes())
                .sum::<usize>()
                > MAX_ROLLBACK_BYTES
        {
            journal.pop_front();
        }
    }

    fn find(&self, rollback_id: &str) -> Result<Option<RollbackRecord>, ErrorData> {
        let journal = self
            .rollback_journal
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(entry) = journal
            .iter()
            .find(|entry| entry.record.id() == rollback_id)
        else {
            return Ok(None);
        };
        if entry.staged_paths.is_empty() {
            Ok(Some(entry.record.clone()))
        } else {
            Err(invalid_arguments(format!(
                "rollback_id `{rollback_id}` has staged files; unstage the owned files before rollback"
            )))
        }
    }

    fn forget(&self, rollback_id: &str) {
        self.rollback_journal
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|entry| entry.record.id() != rollback_id);
    }

    fn owned_paths(
        &self,
        workspace_root: &Path,
        requested: &[PathBuf],
    ) -> Result<Vec<GitOwnedPath>, ErrorData> {
        if requested.is_empty() {
            return Err(invalid_arguments(
                "at least one agent-owned path is required",
            ));
        }
        let journal = self
            .rollback_journal
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut seen = std::collections::HashSet::new();
        let mut owned = Vec::with_capacity(requested.len());
        for path in requested {
            if !seen.insert(path.clone()) {
                return Err(invalid_arguments(format!(
                    "duplicate agent-owned path `{}`",
                    path.display()
                )));
            }
            let found = journal
                .iter()
                .rev()
                .filter(|entry| entry.workspace_root == workspace_root)
                .flat_map(|entry| entry.owned_paths.iter())
                .find(|owned| owned.path == *path)
                .cloned()
                .ok_or_else(|| {
                    invalid_arguments(format!(
                        "path `{}` is not retained as an agent-owned mutation",
                        path.display()
                    ))
                })?;
            owned.push(found);
        }
        Ok(owned)
    }

    fn mark_staged(&self, workspace_root: &Path, paths: &[PathBuf]) {
        let mut journal = self
            .rollback_journal
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for path in paths {
            if let Some(entry) = journal.iter_mut().rev().find(|entry| {
                entry.workspace_root == workspace_root
                    && entry.owned_paths.iter().any(|owned| owned.path == *path)
            }) {
                entry.staged_paths.insert(path.clone());
            }
        }
    }

    fn mark_unstaged(&self, workspace_root: &Path, paths: &[PathBuf]) {
        let mut journal = self
            .rollback_journal
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for entry in journal
            .iter_mut()
            .filter(|entry| entry.workspace_root == workspace_root)
        {
            entry.staged_paths.retain(|path| !paths.contains(path));
        }
    }

    fn expire_committed(&self, workspace_root: &Path, paths: &[PathBuf]) {
        self.rollback_journal
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|entry| {
                entry.workspace_root != workspace_root
                    || !entry
                        .owned_paths
                        .iter()
                        .any(|owned| paths.contains(&owned.path))
            });
    }

    fn remember_commit(&self, workspace_root: &Path, oid: &str) {
        let mut committed = self
            .committed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        committed.push_back(OwnedCommit {
            workspace_root: workspace_root.to_path_buf(),
            oid: oid.to_string(),
        });
        while committed.len() > MAX_ROLLBACK_RECORDS {
            committed.pop_front();
        }
    }

    fn owns_commit(&self, workspace_root: &Path, oid: &str) -> bool {
        self.committed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .any(|commit| commit.workspace_root == workspace_root && commit.oid == oid)
    }

    fn intelligence_index(
        &self,
        config: &CodingConfig,
        workspace: &CodingWorkspace,
        limits: IntelligenceLimits,
    ) -> Result<Arc<RepositoryIndex>, ErrorData> {
        if !config.tree_sitter {
            return Err(tool_unavailable(
                "Tree-sitter repository intelligence is disabled by coding configuration",
            ));
        }
        if !config.indexing {
            return RepositoryIntelligence::build(workspace, limits)
                .map(Arc::new)
                .map_err(|error| invalid_arguments(error.to_string()));
        }

        let fingerprint = RepositoryIntelligence::fingerprint(workspace, limits)
            .map_err(|error| invalid_arguments(error.to_string()))?;
        if let Some(index) = self
            .intelligence_cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .find(|entry| {
                entry.workspace_root == workspace.root()
                    && entry.limits == limits
                    && entry.index.source_fingerprint == fingerprint
            })
            .map(|entry| Arc::clone(&entry.index))
        {
            return Ok(index);
        }

        let index = Arc::new(
            RepositoryIntelligence::build(workspace, limits)
                .map_err(|error| invalid_arguments(error.to_string()))?,
        );
        let mut cache = self
            .intelligence_cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        cache.retain(|entry| entry.workspace_root != workspace.root() || entry.limits != limits);
        cache.push_back(IntelligenceCacheEntry {
            workspace_root: workspace.root().to_path_buf(),
            limits,
            index: Arc::clone(&index),
        });
        while cache.len() > MAX_INTELLIGENCE_CACHE_ENTRIES {
            cache.pop_front();
        }
        Ok(index)
    }

    fn invalidate_intelligence(&self, workspace_root: &Path) {
        self.intelligence_cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|entry| entry.workspace_root != workspace_root);
    }
}

pub fn definitions() -> Vec<Tool> {
    vec![
        repository_profile_tool(),
        repository_instructions_tool(),
        find_files_tool(),
        search_text_tool(),
        read_file_tool(),
        preview_changes_tool(),
        apply_changes_tool(),
        rollback_changes_tool(),
        run_process_tool(),
        git_status_tool(),
        git_diff_tool(),
        git_history_tool(),
        git_stage_owned_tool(),
        git_unstage_owned_tool(),
        git_commit_owned_tool(),
        git_create_branch_tool(),
        git_push_owned_tool(),
        repository_map_tool(),
        search_symbols_tool(),
        find_references_tool(),
        select_context_tool(),
        project_capabilities_tool(),
    ]
}

pub fn is_reserved_name(name: &str) -> bool {
    name.starts_with(CODING_TOOL_PREFIX)
}

pub(crate) fn is_async_tool(name: &str) -> bool {
    matches!(
        name,
        RUN_PROCESS_TOOL_NAME
            | GIT_STATUS_TOOL_NAME
            | GIT_DIFF_TOOL_NAME
            | GIT_HISTORY_TOOL_NAME
            | GIT_STAGE_OWNED_TOOL_NAME
            | GIT_UNSTAGE_OWNED_TOOL_NAME
            | GIT_COMMIT_OWNED_TOOL_NAME
            | GIT_CREATE_BRANCH_TOOL_NAME
            | GIT_PUSH_OWNED_TOOL_NAME
    )
}

pub(crate) async fn execute_async(
    config: &CodingConfig,
    state: &CodingToolState,
    tool_call: CallToolRequestParams,
    working_dir: &Path,
) -> Result<CallToolResult, ErrorData> {
    if !config.tools_enabled() {
        return Err(tool_unavailable(
            "internal coding tools are disabled for this task",
        ));
    }
    if !is_async_tool(&tool_call.name) {
        return Err(invalid_arguments(format!(
            "`{}` is not an asynchronous internal coding tool",
            tool_call.name
        )));
    }

    let workspace = CodingWorkspace::new(working_dir).map_err(invalid_workspace)?;
    match tool_call.name.as_ref() {
        RUN_PROCESS_TOOL_NAME => {
            let params: RunProcessParams = parse_arguments(&tool_call)?;
            let timeout = params
                .timeout_seconds
                .map(Duration::from_secs)
                .unwrap_or(config.shell_timeout);
            if timeout.is_zero() || timeout > config.shell_timeout {
                return Err(invalid_arguments(format!(
                    "timeout_seconds must be between 1 and the configured coding shell timeout of {}",
                    config.shell_timeout.as_secs()
                )));
            }
            let request = ProcessRequest {
                program: params.program,
                args: params.args,
                cwd: params.cwd,
                environment: params.environment,
            };
            let output = ProcessRunner::new(
                &workspace,
                ProcessLimits {
                    timeout,
                    output_limit: config.output_limit,
                },
            )
            .run(request)
            .await
            .map_err(|error| invalid_arguments(error.to_string()))?;
            state.invalidate_intelligence(workspace.root());
            bounded_process_result(output, config.output_limit)
        }
        GIT_STATUS_TOOL_NAME => {
            let params: GitStatusParams = parse_arguments(&tool_call)?;
            let repository =
                GitRepository::open(&workspace, git_limits(config, params.max_entries))
                    .await
                    .map_err(|error| invalid_arguments(error.to_string()))?;
            let status = repository
                .status()
                .await
                .map_err(|error| invalid_arguments(error.to_string()))?;
            json_result(&status, config.output_limit)
        }
        GIT_DIFF_TOOL_NAME => {
            let request: GitDiffRequest = parse_arguments(&tool_call)?;
            let repository = GitRepository::open(&workspace, git_limits(config, 2_000))
                .await
                .map_err(|error| invalid_arguments(error.to_string()))?;
            let diff = repository
                .diff(request)
                .await
                .map_err(|error| invalid_arguments(error.to_string()))?;
            bounded_git_diff_result(diff, config.output_limit)
        }
        GIT_HISTORY_TOOL_NAME => {
            let params: GitHistoryParams = parse_arguments(&tool_call)?;
            let repository = GitRepository::open(&workspace, git_limits(config, 2_000))
                .await
                .map_err(|error| invalid_arguments(error.to_string()))?;
            let history = repository
                .history(params.max_entries)
                .await
                .map_err(|error| invalid_arguments(error.to_string()))?;
            json_result(&history, config.output_limit)
        }
        GIT_STAGE_OWNED_TOOL_NAME => {
            let params: GitOwnedPathsParams = parse_arguments(&tool_call)?;
            let owned = state.owned_paths(workspace.root(), &params.paths)?;
            let repository = GitRepository::open(&workspace, git_limits(config, 2_000))
                .await
                .map_err(|error| invalid_arguments(error.to_string()))?;
            let result = repository
                .stage_owned(&owned)
                .await
                .map_err(|error| invalid_arguments(error.to_string()))?;
            state.mark_staged(workspace.root(), &result.staged_files);
            json_result(&result, config.output_limit)
        }
        GIT_UNSTAGE_OWNED_TOOL_NAME => {
            let params: GitOwnedPathsParams = parse_arguments(&tool_call)?;
            let owned = state.owned_paths(workspace.root(), &params.paths)?;
            let repository = GitRepository::open(&workspace, git_limits(config, 2_000))
                .await
                .map_err(|error| invalid_arguments(error.to_string()))?;
            let result = repository
                .unstage_owned(&owned)
                .await
                .map_err(|error| invalid_arguments(error.to_string()))?;
            state.mark_unstaged(workspace.root(), &result.unstaged_files);
            json_result(&result, config.output_limit)
        }
        GIT_COMMIT_OWNED_TOOL_NAME => {
            let params: GitCommitOwnedParams = parse_arguments(&tool_call)?;
            let owned = state.owned_paths(workspace.root(), &params.paths)?;
            let repository = GitRepository::open(&workspace, git_limits(config, 2_000))
                .await
                .map_err(|error| invalid_arguments(error.to_string()))?;
            let result = repository
                .commit_owned(&params.message, &owned)
                .await
                .map_err(|error| invalid_arguments(error.to_string()))?;
            state.remember_commit(workspace.root(), &result.oid);
            state.expire_committed(workspace.root(), &result.committed_files);
            json_result(&result, config.output_limit)
        }
        GIT_CREATE_BRANCH_TOOL_NAME => {
            let params: GitCreateBranchParams = parse_arguments(&tool_call)?;
            let repository = GitRepository::open(&workspace, git_limits(config, 2_000))
                .await
                .map_err(|error| invalid_arguments(error.to_string()))?;
            let result = repository
                .create_branch(&params.name, params.start_point.as_deref())
                .await
                .map_err(|error| invalid_arguments(error.to_string()))?;
            json_result(&result, config.output_limit)
        }
        GIT_PUSH_OWNED_TOOL_NAME => {
            let params: GitPushOwnedParams = parse_arguments(&tool_call)?;
            if !state.owns_commit(workspace.root(), &params.oid) {
                return Err(invalid_arguments(format!(
                    "commit `{}` is not retained as an agent-owned commit",
                    params.oid
                )));
            }
            let repository = GitRepository::open(&workspace, git_limits(config, 2_000))
                .await
                .map_err(|error| invalid_arguments(error.to_string()))?;
            let result = repository
                .push_current_branch(&params.oid, &params.remote)
                .await
                .map_err(|error| invalid_arguments(error.to_string()))?;
            json_result(&result, config.output_limit)
        }
        _ => unreachable!("async coding tool name was checked"),
    }
}

#[cfg(test)]
fn execute(
    config: &CodingConfig,
    tool_call: CallToolRequestParams,
    working_dir: &Path,
) -> Result<CallToolResult, ErrorData> {
    execute_with_state(config, &CodingToolState::default(), tool_call, working_dir)
}

pub(crate) fn execute_with_state(
    config: &CodingConfig,
    state: &CodingToolState,
    tool_call: CallToolRequestParams,
    working_dir: &Path,
) -> Result<CallToolResult, ErrorData> {
    if !config.tools_enabled() {
        return Err(tool_unavailable(
            "internal coding tools are disabled for this task",
        ));
    }

    let workspace = CodingWorkspace::new(working_dir).map_err(invalid_workspace)?;
    match tool_call.name.as_ref() {
        REPOSITORY_PROFILE_TOOL_NAME => {
            let params: RepositoryProfileParams = parse_arguments(&tool_call)?;
            if params.max_files == 0 || params.max_files > MAX_REPOSITORY_FILE_LIMIT {
                return Err(invalid_arguments(format!(
                    "max_files must be between 1 and {MAX_REPOSITORY_FILE_LIMIT}"
                )));
            }
            let profile = RepositoryProfile::discover(&workspace, params.max_files)
                .map_err(|error| internal_error(error.to_string()))?;
            json_result(&profile, config.output_limit)
        }
        REPOSITORY_INSTRUCTIONS_TOOL_NAME => {
            let params: RepositoryInstructionsParams = parse_arguments(&tool_call)?;
            let instructions = RepositoryInstructions::load_for_path(&workspace, params.path)
                .map_err(|error| invalid_arguments(error.to_string()))?;
            json_result(&instructions, config.output_limit)
        }
        FIND_FILES_TOOL_NAME => {
            let params: FindFilesParams = parse_arguments(&tool_call)?;
            let limits = SearchLimits {
                max_results: params.max_results,
                max_files: params.max_files,
                ..SearchLimits::default()
            };
            let result = RepositorySearch::new(&workspace)
                .find_files(&params.query, params.scope, limits)
                .map_err(|error| invalid_arguments(error.to_string()))?;
            json_result(&result, config.output_limit)
        }
        SEARCH_TEXT_TOOL_NAME => {
            let params: SearchTextParams = parse_arguments(&tool_call)?;
            let limits = SearchLimits {
                max_results: params.max_results,
                max_files: params.max_files,
                max_file_bytes: params.max_file_bytes,
                max_line_bytes: params.max_line_bytes,
            };
            let request = TextSearchRequest {
                pattern: params.pattern,
                scope: params.scope,
                regex: params.regex,
                case_sensitive: params.case_sensitive,
                include: params.include,
            };
            let result = RepositorySearch::new(&workspace)
                .search_text(&request, limits)
                .map_err(|error| invalid_arguments(error.to_string()))?;
            json_result(&result, config.output_limit)
        }
        READ_FILE_TOOL_NAME => {
            let params: ReadFileParams = parse_arguments(&tool_call)?;
            if params.max_bytes > config.output_limit {
                return Err(invalid_arguments(format!(
                    "max_bytes cannot exceed the configured coding output limit of {}",
                    config.output_limit
                )));
            }
            let snapshot = FileSnapshot::read(
                &workspace,
                params.path,
                FileReadOptions {
                    max_bytes: params.max_bytes,
                    start_line: params.start_line,
                    end_line: params.end_line,
                },
            )
            .map_err(|error| invalid_arguments(error.to_string()))?;
            json_result(&snapshot, config.output_limit)
        }
        PREVIEW_CHANGES_TOOL_NAME => {
            let batch: MutationBatch = parse_arguments(&tool_call)?;
            let engine = PatchEngine::new(&workspace, patch_limits(config));
            let prepared = engine
                .prepare(batch)
                .map_err(|error| invalid_arguments(error.to_string()))?;
            json_result(&prepared.preview, config.output_limit)
        }
        APPLY_CHANGES_TOOL_NAME => {
            let batch: MutationBatch = parse_arguments(&tool_call)?;
            let engine = PatchEngine::new(&workspace, patch_limits(config));
            let prepared = engine
                .prepare(batch)
                .map_err(|error| invalid_arguments(error.to_string()))?;
            let prospective_result = MutationResult {
                rollback_id: "00000000-0000-7000-8000-000000000000".to_string(),
                preview: prepared.preview.clone(),
            };
            ensure_json_fits(&prospective_result, config.output_limit)?;
            let applied = engine
                .apply(prepared)
                .map_err(|error| invalid_arguments(error.to_string()))?;
            state.remember(workspace.root(), applied.rollback, &applied.result.preview);
            state.invalidate_intelligence(workspace.root());
            json_result(&applied.result, config.output_limit)
        }
        ROLLBACK_CHANGES_TOOL_NAME => {
            let params: RollbackChangesParams = parse_arguments(&tool_call)?;
            let record = state.find(&params.rollback_id)?.ok_or_else(|| {
                invalid_arguments(format!(
                    "unknown or expired rollback_id `{}`",
                    params.rollback_id
                ))
            })?;
            let engine = PatchEngine::new(&workspace, patch_limits(config));
            let result = engine
                .rollback(record)
                .map_err(|error| invalid_arguments(error.to_string()))?;
            state.forget(&params.rollback_id);
            state.invalidate_intelligence(workspace.root());
            json_result(&result, config.output_limit)
        }
        REPOSITORY_MAP_TOOL_NAME => {
            let params: RepositoryMapParams = parse_arguments(&tool_call)?;
            let index = state.intelligence_index(config, &workspace, params.limits())?;
            json_result(
                &RepositoryMapResult::from(index.as_ref()),
                config.output_limit,
            )
        }
        SEARCH_SYMBOLS_TOOL_NAME => {
            let params: SymbolSearchParams = parse_arguments(&tool_call)?;
            let index = state.intelligence_index(config, &workspace, params.limits())?;
            let result = index
                .search_symbols(&params.query, params.exact, params.max_results)
                .map_err(|error| invalid_arguments(error.to_string()))?;
            json_result(&result, config.output_limit)
        }
        FIND_REFERENCES_TOOL_NAME => {
            let params: ReferenceSearchParams = parse_arguments(&tool_call)?;
            let index = state.intelligence_index(config, &workspace, params.limits())?;
            let result = index
                .references(&params.symbol, params.max_results)
                .map_err(|error| invalid_arguments(error.to_string()))?;
            json_result(&result, config.output_limit)
        }
        SELECT_CONTEXT_TOOL_NAME => {
            let params: ContextSelectionParams = parse_arguments(&tool_call)?;
            let index = state.intelligence_index(config, &workspace, params.limits())?;
            let result = index
                .context_candidates(&params.query, params.max_results)
                .map_err(|error| invalid_arguments(error.to_string()))?;
            json_result(&result, config.output_limit)
        }
        PROJECT_CAPABILITIES_TOOL_NAME => {
            let params: RepositoryProfileParams = parse_arguments(&tool_call)?;
            if params.max_files == 0 || params.max_files > MAX_REPOSITORY_FILE_LIMIT {
                return Err(invalid_arguments(format!(
                    "max_files must be between 1 and {MAX_REPOSITORY_FILE_LIMIT}"
                )));
            }
            let capabilities = ProjectDiscovery::discover(&workspace, params.max_files)
                .map_err(|error| invalid_arguments(error.to_string()))?;
            json_result(&capabilities, config.output_limit)
        }
        RUN_PROCESS_TOOL_NAME
        | GIT_STATUS_TOOL_NAME
        | GIT_DIFF_TOOL_NAME
        | GIT_HISTORY_TOOL_NAME
        | GIT_STAGE_OWNED_TOOL_NAME
        | GIT_UNSTAGE_OWNED_TOOL_NAME
        | GIT_COMMIT_OWNED_TOOL_NAME
        | GIT_CREATE_BRANCH_TOOL_NAME
        | GIT_PUSH_OWNED_TOOL_NAME => Err(internal_error(
            "asynchronous coding tools require asynchronous dispatch",
        )),
        _ => Err(invalid_arguments(format!(
            "unknown internal coding tool `{}`",
            tool_call.name
        ))),
    }
}

fn repository_profile_tool() -> Tool {
    Tool::new(
        REPOSITORY_PROFILE_TOOL_NAME.to_string(),
        "Inspect the current workspace without executing project code. Returns version control, \
         detected project manifests, language counts, scan bounds, and warnings."
            .to_string(),
        object!({
            "type": "object",
            "properties": {
                "max_files": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": MAX_REPOSITORY_FILE_LIMIT,
                    "default": DEFAULT_REPOSITORY_FILE_LIMIT,
                    "description": "Maximum number of repository files to inspect."
                }
            },
            "additionalProperties": false
        }),
    )
    .annotate(read_only_annotations("Inspect coding repository"))
}

fn repository_instructions_tool() -> Tool {
    Tool::new(
        REPOSITORY_INSTRUCTIONS_TOOL_NAME.to_string(),
        "Resolve AGENTS.md and .ponduinhints files from the workspace root through a target path. \
         Repository instructions are returned with explicit untrusted provenance; global user \
         hints and external imports are excluded."
            .to_string(),
        object!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "default": ".",
                    "description": "Existing workspace-relative file or directory."
                }
            },
            "additionalProperties": false
        }),
    )
    .annotate(read_only_annotations("Read repository coding instructions"))
}

fn find_files_tool() -> Tool {
    Tool::new(
        FIND_FILES_TOOL_NAME.to_string(),
        "Find workspace-relative file paths by a case-insensitive substring. Respects repository \
         ignore files, does not follow symlinks, and returns explicit scan and truncation state."
            .to_string(),
        object!({
            "type": "object",
            "required": ["query"],
            "properties": {
                "query": {
                    "type": "string",
                    "minLength": 1,
                    "description": "Case-insensitive substring to match against relative paths."
                },
                "scope": {
                    "type": "string",
                    "default": ".",
                    "description": "Existing workspace-relative file or directory."
                },
                "max_results": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 1000,
                    "default": 200
                },
                "max_files": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 100000,
                    "default": 50000
                }
            },
            "additionalProperties": false
        }),
    )
    .annotate(read_only_annotations("Find coding files"))
}

fn search_text_tool() -> Tool {
    Tool::new(
        SEARCH_TEXT_TOOL_NAME.to_string(),
        "Search bounded UTF-8 source text inside the workspace using a literal or Rust regular \
         expression. Respects ignore files, skips binary, oversized, and sensitive files, and \
         returns line, column, matched text, line text, and truncation evidence."
            .to_string(),
        object!({
            "type": "object",
            "required": ["pattern"],
            "properties": {
                "pattern": {
                    "type": "string",
                    "minLength": 1
                },
                "scope": {
                    "type": "string",
                    "default": "."
                },
                "regex": {
                    "type": "boolean",
                    "default": false
                },
                "case_sensitive": {
                    "type": "boolean",
                    "default": false
                },
                "include": {
                    "type": "array",
                    "items": {"type": "string"},
                    "default": [],
                    "description": "Optional glob patterns matched against workspace-relative paths."
                },
                "max_results": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 1000,
                    "default": 200
                },
                "max_files": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 100000,
                    "default": 50000
                },
                "max_file_bytes": {
                    "type": "integer",
                    "minimum": 8192,
                    "maximum": 10485760,
                    "default": 2097152
                },
                "max_line_bytes": {
                    "type": "integer",
                    "minimum": 128,
                    "maximum": 65536,
                    "default": 4096
                }
            },
            "additionalProperties": false
        }),
    )
    .annotate(read_only_annotations("Search coding text"))
}

fn read_file_tool() -> Tool {
    Tool::new(
        READ_FILE_TOOL_NAME.to_string(),
        "Read a bounded UTF-8 file inside the workspace and return a BLAKE3 digest of the complete \
         file. Optional line bounds reduce returned content without changing the digest. Sensitive, \
         binary, oversized, external, and symlink-escaping files are rejected."
            .to_string(),
        object!({
            "type": "object",
            "required": ["path"],
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Existing workspace-relative file path."
                },
                "start_line": {
                    "type": ["integer", "null"],
                    "minimum": 1
                },
                "end_line": {
                    "type": ["integer", "null"],
                    "minimum": 1
                },
                "max_bytes": {
                    "type": "integer",
                    "minimum": MIN_READ_LIMIT,
                    "maximum": MAX_READ_LIMIT,
                    "default": DEFAULT_READ_LIMIT
                }
            },
            "additionalProperties": false
        }),
    )
    .annotate(read_only_annotations("Read versioned coding file"))
}

fn preview_changes_tool() -> Tool {
    Tool::new(
        PREVIEW_CHANGES_TOOL_NAME.to_string(),
        "Validate a bounded batch of workspace file changes and return unified diffs without \
         modifying the filesystem. Existing files require the complete BLAKE3 digest returned by \
         coding__read_file. Exact replacements are conflict-safe and must be unique unless \
         replace_all is explicitly true."
            .to_string(),
        mutation_batch_schema(),
    )
    .annotate(read_only_annotations("Preview versioned coding changes"))
}

fn apply_changes_tool() -> Tool {
    Tool::new(
        APPLY_CHANGES_TOOL_NAME.to_string(),
        "Atomically apply a validated batch of workspace file changes. Existing files require the \
         complete BLAKE3 digest returned by coding__read_file. All files are validated and staged \
         before mutation; failures restore already-applied files. Returns unified diffs and a \
         bounded agent-local rollback_id."
            .to_string(),
        mutation_batch_schema(),
    )
    .annotate(mutation_annotations("Apply versioned coding changes"))
}

fn rollback_changes_tool() -> Tool {
    Tool::new(
        ROLLBACK_CHANGES_TOOL_NAME.to_string(),
        "Roll back one previously applied coding change batch using its agent-local rollback_id. \
         Rollback refuses to overwrite files that changed after the original apply."
            .to_string(),
        object!({
            "type": "object",
            "required": ["rollback_id"],
            "properties": {
                "rollback_id": {
                    "type": "string",
                    "minLength": 1
                }
            },
            "additionalProperties": false
        }),
    )
    .annotate(mutation_annotations("Roll back coding changes"))
}

fn run_process_tool() -> Tool {
    Tool::new(
        RUN_PROCESS_TOOL_NAME.to_string(),
        "Run one bounded, non-interactive development process in the workspace using an executable \
         plus a literal argument array. Shell syntax is never evaluated. The environment is \
         cleared and rebuilt from a small safe baseline plus allowlisted overrides. Git, shells, \
         recursive deletion, privilege escalation, network clients, deployment tools, and host \
         administration commands are blocked in favor of dedicated safer workflows. Captures \
         stdout and stderr separately, enforces timeout and combined output limits, and terminates \
         lingering process groups."
            .to_string(),
        object!({
            "type": "object",
            "required": ["program"],
            "properties": {
                "program": {
                    "type": "string",
                    "minLength": 1,
                    "description": "Executable name from PATH or an existing workspace-relative executable path."
                },
                "args": {
                    "type": "array",
                    "items": {"type": "string"},
                    "maxItems": 256,
                    "default": [],
                    "description": "Literal argv entries. Do not use shell quoting, pipes, redirects, substitutions, or chaining."
                },
                "cwd": {
                    "type": "string",
                    "default": ".",
                    "description": "Existing workspace-relative working directory."
                },
                "environment": {
                    "type": "object",
                    "additionalProperties": {"type": "string"},
                    "maxProperties": 32,
                    "default": {},
                    "description": "Optional neutral allowlisted overrides; secrets and execution-control variables are rejected."
                },
                "timeout_seconds": {
                    "type": ["integer", "null"],
                    "minimum": 1,
                    "description": "Optional shorter timeout; cannot exceed the configured coding shell timeout."
                }
            },
            "additionalProperties": false
        }),
    )
    .annotate(mutation_annotations("Run bounded coding process"))
}

fn git_status_tool() -> Tool {
    Tool::new(
        GIT_STATUS_TOOL_NAME.to_string(),
        "Read the current Git branch, HEAD, upstream divergence, staged and unstaged status, \
         conflicts, and untracked files. Repository roots outside the workspace are rejected. \
         Configured executable content filters and fsmonitor hooks are disabled."
            .to_string(),
        object!({
            "type": "object",
            "properties": {
                "max_entries": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 100000,
                    "default": 2000
                }
            },
            "additionalProperties": false
        }),
    )
    .annotate(read_only_annotations("Inspect Git status"))
}

fn git_diff_tool() -> Tool {
    Tool::new(
        GIT_DIFF_TOOL_NAME.to_string(),
        "Read a bounded unstaged or staged Git patch without external diff drivers, textconv, \
         content filters, or submodule traversal. Sensitive files are omitted and reported. \
         Optional paths are treated as literal workspace-relative paths."
            .to_string(),
        object!({
            "type": "object",
            "properties": {
                "staged": {
                    "type": "boolean",
                    "default": false
                },
                "paths": {
                    "type": "array",
                    "items": {"type": "string", "minLength": 1},
                    "maxItems": 100,
                    "default": []
                },
                "context_lines": {
                    "type": "integer",
                    "minimum": 0,
                    "maximum": 1000,
                    "default": 3
                }
            },
            "additionalProperties": false
        }),
    )
    .annotate(read_only_annotations("Read safe Git diff"))
}

fn git_history_tool() -> Tool {
    Tool::new(
        GIT_HISTORY_TOOL_NAME.to_string(),
        "Read bounded local Git commit history with commit ids, author, timestamp, and subject. \
         Does not contact remotes or execute repository hooks."
            .to_string(),
        object!({
            "type": "object",
            "properties": {
                "max_entries": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 100,
                    "default": 20
                }
            },
            "additionalProperties": false
        }),
    )
    .annotate(read_only_annotations("Read Git history"))
}

fn git_stage_owned_tool() -> Tool {
    Tool::new(
        GIT_STAGE_OWNED_TOOL_NAME.to_string(),
        "Stage only explicitly listed files retained in this agent's mutation journal. Refuses \
         files that were already dirty or staged before the agent change, files changed after \
         apply, sensitive files, conflicts, and expired ownership records. Executable Git content \
         filters are neutralized."
            .to_string(),
        owned_paths_schema(),
    )
    .annotate(mutation_annotations("Stage agent-owned Git changes"))
}

fn git_unstage_owned_tool() -> Tool {
    Tool::new(
        GIT_UNSTAGE_OWNED_TOOL_NAME.to_string(),
        "Unstage only explicitly listed files whose staged index content still exactly matches \
         this agent's retained mutation. Restores their prior index state without changing the \
         working tree and re-enables patch rollback."
            .to_string(),
        owned_paths_schema(),
    )
    .annotate(mutation_annotations("Unstage agent-owned Git changes"))
}

fn git_commit_owned_tool() -> Tool {
    Tool::new(
        GIT_COMMIT_OWNED_TOOL_NAME.to_string(),
        "Commit exactly the listed agent-owned staged files. Refuses the commit if any foreign, \
         missing, changed, or additional staged path exists. Git hooks and commit signing are \
         disabled. This never pushes."
            .to_string(),
        object!({
            "type": "object",
            "required": ["message", "paths"],
            "properties": {
                "message": {
                    "type": "string",
                    "minLength": 1,
                    "maxLength": 4096
                },
                "paths": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": 50,
                    "items": {"type": "string", "minLength": 1}
                }
            },
            "additionalProperties": false
        }),
    )
    .annotate(mutation_annotations("Commit agent-owned Git changes"))
}

fn git_create_branch_tool() -> Tool {
    Tool::new(
        GIT_CREATE_BRANCH_TOOL_NAME.to_string(),
        "Create a new local Git branch at a verified commit without switching branches, deleting \
         branches, overwriting an existing branch, contacting remotes, or pushing."
            .to_string(),
        object!({
            "type": "object",
            "required": ["name"],
            "properties": {
                "name": {
                    "type": "string",
                    "minLength": 1,
                    "maxLength": 256
                },
                "start_point": {
                    "type": ["string", "null"],
                    "minLength": 1,
                    "maxLength": 256,
                    "default": null
                }
            },
            "additionalProperties": false
        }),
    )
    .annotate(mutation_annotations("Create local Git branch"))
}

fn git_push_owned_tool() -> Tool {
    Tool::new(
        GIT_PUSH_OWNED_TOOL_NAME.to_string(),
        "Push exactly one commit retained as created by this agent from the current local branch. \
         Requires an explicit configured remote, disables pre-push hooks and credential helpers, \
         rejects force/deletion refspecs, detached or changed HEAD, embedded HTTPS credentials, \
         unsafe protocols, and local repositories outside the workspace."
            .to_string(),
        object!({
            "type": "object",
            "required": ["oid", "remote"],
            "properties": {
                "oid": {
                    "type": "string",
                    "minLength": 40,
                    "maxLength": 64,
                    "description": "Commit object id returned by coding__git_commit_owned."
                },
                "remote": {
                    "type": "string",
                    "minLength": 1,
                    "maxLength": 256,
                    "description": "Existing configured Git remote name."
                }
            },
            "additionalProperties": false
        }),
    )
    .annotate(mutation_annotations("Push agent-owned Git commit"))
}

fn repository_map_tool() -> Tool {
    Tool::new(
        REPOSITORY_MAP_TOOL_NAME.to_string(),
        "Build a bounded internal Tree-sitter repository map without using an extension. Returns \
         analyzed source files, framework evidence, entry points, configuration and generated \
         files, aggregate symbol/import/call counts, scan bounds, fingerprint, and warnings."
            .to_string(),
        intelligence_limits_schema(),
    )
    .annotate(read_only_annotations("Map repository code"))
}

fn search_symbols_tool() -> Tool {
    Tool::new(
        SEARCH_SYMBOLS_TOOL_NAME.to_string(),
        "Search function and type definitions in the bounded internal Tree-sitter repository \
         index. Results include workspace-relative paths, qualified names, kinds, and source \
         lines; no extension or project code is executed."
            .to_string(),
        object!({
            "type": "object",
            "required": ["query"],
            "properties": {
                "query": {"type": "string", "minLength": 1},
                "exact": {"type": "boolean", "default": false},
                "max_results": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 1000,
                    "default": 100
                },
                "max_files": intelligence_max_files_schema(),
                "max_file_bytes": intelligence_max_file_bytes_schema(),
                "max_symbols": intelligence_max_symbols_schema()
            },
            "additionalProperties": false
        }),
    )
    .annotate(read_only_annotations("Search repository symbols"))
}

fn find_references_tool() -> Tool {
    Tool::new(
        FIND_REFERENCES_TOOL_NAME.to_string(),
        "Find bounded call-site references to an exact callee name in the internal Tree-sitter \
         repository index. Results identify the caller, source path, and line without executing \
         repository code."
            .to_string(),
        object!({
            "type": "object",
            "required": ["symbol"],
            "properties": {
                "symbol": {"type": "string", "minLength": 1},
                "max_results": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 1000,
                    "default": 100
                },
                "max_files": intelligence_max_files_schema(),
                "max_file_bytes": intelligence_max_file_bytes_schema(),
                "max_symbols": intelligence_max_symbols_schema()
            },
            "additionalProperties": false
        }),
    )
    .annotate(read_only_annotations("Find repository references"))
}

fn select_context_tool() -> Tool {
    Tool::new(
        SELECT_CONTEXT_TOOL_NAME.to_string(),
        "Rank bounded repository files as context candidates for a coding query using paths, \
         symbols, imports, calls, framework evidence, entry points, and configuration files from \
         the internal Tree-sitter index. This selects files but does not read their contents."
            .to_string(),
        object!({
            "type": "object",
            "required": ["query"],
            "properties": {
                "query": {"type": "string", "minLength": 1},
                "max_results": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 200,
                    "default": 20
                },
                "max_files": intelligence_max_files_schema(),
                "max_file_bytes": intelligence_max_file_bytes_schema(),
                "max_symbols": intelligence_max_symbols_schema()
            },
            "additionalProperties": false
        }),
    )
    .annotate(read_only_annotations("Select repository context"))
}

fn project_capabilities_tool() -> Tool {
    Tool::new(
        PROJECT_CAPABILITIES_TOOL_NAME.to_string(),
        "Detect nested polyglot projects, bounded dependency names, CI configuration, package \
         managers, and evidence-backed build, test, lint, format, and typecheck commands. \
         Manifests are treated as untrusted text and repository code is never executed."
            .to_string(),
        object!({
            "type": "object",
            "properties": {
                "max_files": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": MAX_REPOSITORY_FILE_LIMIT,
                    "default": DEFAULT_REPOSITORY_FILE_LIMIT
                }
            },
            "additionalProperties": false
        }),
    )
    .annotate(read_only_annotations("Detect project capabilities"))
}

fn intelligence_limits_schema() -> serde_json::Map<String, Value> {
    object!({
        "type": "object",
        "properties": {
            "max_files": intelligence_max_files_schema(),
            "max_file_bytes": intelligence_max_file_bytes_schema(),
            "max_symbols": intelligence_max_symbols_schema()
        },
        "additionalProperties": false
    })
}

fn intelligence_max_files_schema() -> Value {
    serde_json::json!({
        "type": "integer",
        "minimum": 1,
        "maximum": 100000,
        "default": 20000
    })
}

fn intelligence_max_file_bytes_schema() -> Value {
    serde_json::json!({
        "type": "integer",
        "minimum": 8192,
        "maximum": 10485760,
        "default": 2097152
    })
}

fn intelligence_max_symbols_schema() -> Value {
    serde_json::json!({
        "type": "integer",
        "minimum": 1,
        "maximum": 500000,
        "default": 50000
    })
}

fn owned_paths_schema() -> serde_json::Map<String, Value> {
    object!({
        "type": "object",
        "required": ["paths"],
        "properties": {
            "paths": {
                "type": "array",
                "minItems": 1,
                "maxItems": 50,
                "items": {"type": "string", "minLength": 1}
            }
        },
        "additionalProperties": false
    })
}

fn mutation_batch_schema() -> serde_json::Map<String, Value> {
    object!({
        "type": "object",
        "required": ["changes"],
        "properties": {
            "changes": {
                "type": "array",
                "minItems": 1,
                "items": {
                    "oneOf": [
                        {
                            "type": "object",
                            "required": ["operation", "path", "content"],
                            "properties": {
                                "operation": {"const": "create"},
                                "path": {
                                    "type": "string",
                                    "minLength": 1,
                                    "description": "New workspace-relative file path whose parent exists."
                                },
                                "content": {"type": "string"}
                            },
                            "additionalProperties": false
                        },
                        {
                            "type": "object",
                            "required": ["operation", "path", "expected_digest", "content"],
                            "properties": {
                                "operation": {"const": "write"},
                                "path": {"type": "string", "minLength": 1},
                                "expected_digest": {
                                    "type": "string",
                                    "description": "Complete BLAKE3 digest returned by coding__read_file."
                                },
                                "content": {"type": "string"}
                            },
                            "additionalProperties": false
                        },
                        {
                            "type": "object",
                            "required": ["operation", "path", "expected_digest", "replacements"],
                            "properties": {
                                "operation": {"const": "replace"},
                                "path": {"type": "string", "minLength": 1},
                                "expected_digest": {
                                    "type": "string",
                                    "description": "Complete BLAKE3 digest returned by coding__read_file."
                                },
                                "replacements": {
                                    "type": "array",
                                    "minItems": 1,
                                    "items": {
                                        "type": "object",
                                        "required": ["old", "new"],
                                        "properties": {
                                            "old": {"type": "string", "minLength": 1},
                                            "new": {"type": "string"},
                                            "replace_all": {"type": "boolean", "default": false}
                                        },
                                        "additionalProperties": false
                                    }
                                }
                            },
                            "additionalProperties": false
                        },
                        {
                            "type": "object",
                            "required": ["operation", "path", "expected_digest"],
                            "properties": {
                                "operation": {"const": "delete"},
                                "path": {"type": "string", "minLength": 1},
                                "expected_digest": {
                                    "type": "string",
                                    "description": "Complete BLAKE3 digest returned by coding__read_file."
                                }
                            },
                            "additionalProperties": false
                        }
                    ]
                }
            },
            "additionalProperties": false
        }
    })
}

fn read_only_annotations(title: &str) -> ToolAnnotations {
    ToolAnnotations::with_title(title.to_string())
        .read_only(true)
        .destructive(false)
        .idempotent(true)
        .open_world(false)
}

fn mutation_annotations(title: &str) -> ToolAnnotations {
    ToolAnnotations::with_title(title.to_string())
        .read_only(false)
        .destructive(true)
        .idempotent(false)
        .open_world(false)
}

fn patch_limits(config: &CodingConfig) -> PatchLimits {
    PatchLimits {
        max_files: config.max_files_per_batch,
        max_file_bytes: config.output_limit.min(MAX_PATCH_FILE_LIMIT),
        max_batch_bytes: DEFAULT_PATCH_BATCH_LIMIT,
    }
}

fn git_limits(config: &CodingConfig, max_status_entries: usize) -> GitLimits {
    GitLimits {
        timeout: config.shell_timeout,
        output_limit: config.output_limit,
        max_status_entries,
    }
}

fn parse_arguments<T>(tool_call: &CallToolRequestParams) -> Result<T, ErrorData>
where
    T: DeserializeOwned,
{
    serde_json::from_value(Value::Object(
        tool_call.arguments.clone().unwrap_or_default(),
    ))
    .map_err(|error| invalid_arguments(error.to_string()))
}

fn json_result(
    value: &impl serde::Serialize,
    output_limit: usize,
) -> Result<CallToolResult, ErrorData> {
    let json = serialize_json(value)?;
    if json.len() > output_limit {
        return Err(output_too_large(json.len(), output_limit));
    }
    Ok(CallToolResult::success(vec![ContentBlock::text(json)]))
}

fn bounded_process_result(
    mut output: ProcessOutput,
    output_limit: usize,
) -> Result<CallToolResult, ErrorData> {
    for _ in 0..32 {
        let json = serialize_json(&output)?;
        if json.len() <= output_limit {
            return Ok(CallToolResult::success(vec![ContentBlock::text(json)]));
        }

        output.output_truncated = true;
        if output.stdout.len() >= output.stderr.len() && !output.stdout.is_empty() {
            let requested_len = output.stdout.len() / 2;
            truncate_utf8(&mut output.stdout, requested_len);
        } else if !output.stderr.is_empty() {
            let requested_len = output.stderr.len() / 2;
            truncate_utf8(&mut output.stderr, requested_len);
        } else if let Some(error) = output.output_collection_error.as_mut() {
            if error.len() > 64 {
                let requested_len = error.len() / 2;
                truncate_utf8(error, requested_len);
            } else {
                output.output_collection_error = None;
            }
        } else if !output.program.is_empty() || output.cwd != Path::new(".") {
            output.program.clear();
            output.cwd = PathBuf::from(".");
        } else {
            return Err(output_too_large(json.len(), output_limit));
        }
    }
    Err(internal_error(
        "failed to fit coding process result within the configured output limit",
    ))
}

fn bounded_git_diff_result(
    mut diff: GitDiff,
    output_limit: usize,
) -> Result<CallToolResult, ErrorData> {
    for _ in 0..32 {
        let json = serialize_json(&diff)?;
        if json.len() <= output_limit {
            return Ok(CallToolResult::success(vec![ContentBlock::text(json)]));
        }
        if diff.patch.is_empty() {
            return Err(output_too_large(json.len(), output_limit));
        }
        diff.truncated = true;
        let requested_len = diff.patch.len() / 2;
        truncate_utf8(&mut diff.patch, requested_len);
    }
    Err(internal_error(
        "failed to fit Git diff within the configured output limit",
    ))
}

fn truncate_utf8(value: &mut String, requested_len: usize) {
    let mut boundary = requested_len.min(value.len());
    while !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    value.truncate(boundary);
}

fn ensure_json_fits(value: &impl serde::Serialize, output_limit: usize) -> Result<(), ErrorData> {
    let json = serialize_json(value)?;
    if json.len() > output_limit {
        Err(output_too_large(json.len(), output_limit))
    } else {
        Ok(())
    }
}

fn serialize_json(value: &impl serde::Serialize) -> Result<String, ErrorData> {
    serde_json::to_string_pretty(value)
        .map_err(|error| internal_error(format!("failed to serialize coding tool result: {error}")))
}

fn output_too_large(size: usize, output_limit: usize) -> ErrorData {
    invalid_arguments(format!(
        "coding tool result is {size} bytes, configured output limit is {output_limit}; narrow the \
         request or split the change batch"
    ))
}

fn invalid_workspace(error: impl std::fmt::Display) -> ErrorData {
    invalid_arguments(format!("invalid coding workspace: {error}"))
}

fn invalid_arguments(message: impl Into<String>) -> ErrorData {
    ErrorData::new(ErrorCode::INVALID_PARAMS, message.into(), None)
}

fn internal_error(message: impl Into<String>) -> ErrorData {
    ErrorData::new(ErrorCode::INTERNAL_ERROR, message.into(), None)
}

fn tool_unavailable(message: impl Into<String>) -> ErrorData {
    ErrorData::new(ErrorCode::INVALID_REQUEST, message.into(), None)
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RepositoryProfileParams {
    max_files: usize,
}

impl Default for RepositoryProfileParams {
    fn default() -> Self {
        Self {
            max_files: DEFAULT_REPOSITORY_FILE_LIMIT,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RepositoryInstructionsParams {
    path: PathBuf,
}

impl Default for RepositoryInstructionsParams {
    fn default() -> Self {
        Self {
            path: PathBuf::from("."),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FindFilesParams {
    query: String,
    #[serde(default = "default_path")]
    scope: PathBuf,
    #[serde(default = "default_max_results")]
    max_results: usize,
    #[serde(default = "default_max_files")]
    max_files: usize,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchTextParams {
    pattern: String,
    #[serde(default = "default_path")]
    scope: PathBuf,
    #[serde(default)]
    regex: bool,
    #[serde(default)]
    case_sensitive: bool,
    #[serde(default)]
    include: Vec<String>,
    #[serde(default = "default_max_results")]
    max_results: usize,
    #[serde(default = "default_max_files")]
    max_files: usize,
    #[serde(default = "default_max_file_bytes")]
    max_file_bytes: usize,
    #[serde(default = "default_max_line_bytes")]
    max_line_bytes: usize,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadFileParams {
    path: PathBuf,
    #[serde(default)]
    start_line: Option<usize>,
    #[serde(default)]
    end_line: Option<usize>,
    #[serde(default = "default_read_limit")]
    max_bytes: usize,
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RepositoryMapParams {
    max_files: usize,
    max_file_bytes: usize,
    max_symbols: usize,
}

impl RepositoryMapParams {
    fn limits(&self) -> IntelligenceLimits {
        IntelligenceLimits {
            max_files: self.max_files,
            max_file_bytes: self.max_file_bytes,
            max_symbols: self.max_symbols,
        }
    }
}

impl Default for RepositoryMapParams {
    fn default() -> Self {
        let limits = IntelligenceLimits::default();
        Self {
            max_files: limits.max_files,
            max_file_bytes: limits.max_file_bytes,
            max_symbols: limits.max_symbols,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SymbolSearchParams {
    query: String,
    #[serde(default)]
    exact: bool,
    #[serde(default = "default_symbol_results")]
    max_results: usize,
    #[serde(default = "default_intelligence_max_files")]
    max_files: usize,
    #[serde(default = "default_intelligence_max_file_bytes")]
    max_file_bytes: usize,
    #[serde(default = "default_intelligence_max_symbols")]
    max_symbols: usize,
}

impl SymbolSearchParams {
    fn limits(&self) -> IntelligenceLimits {
        intelligence_limits(self.max_files, self.max_file_bytes, self.max_symbols)
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReferenceSearchParams {
    symbol: String,
    #[serde(default = "default_symbol_results")]
    max_results: usize,
    #[serde(default = "default_intelligence_max_files")]
    max_files: usize,
    #[serde(default = "default_intelligence_max_file_bytes")]
    max_file_bytes: usize,
    #[serde(default = "default_intelligence_max_symbols")]
    max_symbols: usize,
}

impl ReferenceSearchParams {
    fn limits(&self) -> IntelligenceLimits {
        intelligence_limits(self.max_files, self.max_file_bytes, self.max_symbols)
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ContextSelectionParams {
    query: String,
    #[serde(default = "default_context_results")]
    max_results: usize,
    #[serde(default = "default_intelligence_max_files")]
    max_files: usize,
    #[serde(default = "default_intelligence_max_file_bytes")]
    max_file_bytes: usize,
    #[serde(default = "default_intelligence_max_symbols")]
    max_symbols: usize,
}

impl ContextSelectionParams {
    fn limits(&self) -> IntelligenceLimits {
        intelligence_limits(self.max_files, self.max_file_bytes, self.max_symbols)
    }
}

#[derive(Debug, Serialize)]
struct RepositoryMapResult<'a> {
    files: &'a [crate::coding::intelligence::CodeFileMap],
    frameworks: &'a [crate::coding::intelligence::FrameworkDetection],
    entry_points: &'a [PathBuf],
    config_files: &'a [PathBuf],
    generated_files: &'a [PathBuf],
    excluded_directory_names: &'a [String],
    symbol_count: usize,
    import_count: usize,
    call_count: usize,
    scanned_files: usize,
    analyzed_bytes: usize,
    source_fingerprint: &'a str,
    truncated: bool,
    warnings: &'a [String],
}

impl<'a> From<&'a RepositoryIndex> for RepositoryMapResult<'a> {
    fn from(index: &'a RepositoryIndex) -> Self {
        Self {
            files: &index.files,
            frameworks: &index.frameworks,
            entry_points: &index.entry_points,
            config_files: &index.config_files,
            generated_files: &index.generated_files,
            excluded_directory_names: &index.excluded_directory_names,
            symbol_count: index.symbols.len(),
            import_count: index.imports.len(),
            call_count: index.calls.len(),
            scanned_files: index.scanned_files,
            analyzed_bytes: index.analyzed_bytes,
            source_fingerprint: &index.source_fingerprint,
            truncated: index.truncated,
            warnings: &index.warnings,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RollbackChangesParams {
    rollback_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RunProcessParams {
    program: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default = "default_path")]
    cwd: PathBuf,
    #[serde(default)]
    environment: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    timeout_seconds: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct GitStatusParams {
    max_entries: usize,
}

impl Default for GitStatusParams {
    fn default() -> Self {
        Self { max_entries: 2_000 }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct GitHistoryParams {
    max_entries: usize,
}

impl Default for GitHistoryParams {
    fn default() -> Self {
        Self { max_entries: 20 }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GitOwnedPathsParams {
    paths: Vec<PathBuf>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GitCommitOwnedParams {
    message: String,
    paths: Vec<PathBuf>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GitCreateBranchParams {
    name: String,
    #[serde(default)]
    start_point: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GitPushOwnedParams {
    oid: String,
    remote: String,
}

fn default_path() -> PathBuf {
    PathBuf::from(".")
}

fn default_max_results() -> usize {
    SearchLimits::default().max_results
}

fn default_max_files() -> usize {
    SearchLimits::default().max_files
}

fn default_max_file_bytes() -> usize {
    SearchLimits::default().max_file_bytes
}

fn default_max_line_bytes() -> usize {
    SearchLimits::default().max_line_bytes
}

fn default_read_limit() -> usize {
    DEFAULT_READ_LIMIT
}

fn intelligence_limits(
    max_files: usize,
    max_file_bytes: usize,
    max_symbols: usize,
) -> IntelligenceLimits {
    IntelligenceLimits {
        max_files,
        max_file_bytes,
        max_symbols,
    }
}

fn default_symbol_results() -> usize {
    100
}

fn default_context_results() -> usize {
    20
}

fn default_intelligence_max_files() -> usize {
    IntelligenceLimits::default().max_files
}

fn default_intelligence_max_file_bytes() -> usize {
    IntelligenceLimits::default().max_file_bytes
}

fn default_intelligence_max_symbols() -> usize {
    IntelligenceLimits::default().max_symbols
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coding::CodingTaskMode;
    use std::fs;
    use std::process::Command;

    fn enabled_config() -> CodingConfig {
        CodingConfig {
            enabled: true,
            task_mode: CodingTaskMode::Coding,
            ..CodingConfig::default()
        }
    }

    fn result_text(result: CallToolResult) -> String {
        let content = result.content.into_iter().next().unwrap();
        content
            .as_text()
            .expect("expected text result")
            .text
            .clone()
    }

    fn run_git(root: &Path, args: &[&str]) {
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

    #[test]
    fn definitions_distinguish_read_only_and_mutating_tools() {
        let tools = definitions();
        assert_eq!(tools.len(), 22);
        assert!(tools
            .iter()
            .all(|tool| is_reserved_name(&tool.name) && tool.annotations.is_some()));
        for tool in &tools {
            let annotations = tool.annotations.as_ref().unwrap();
            let mutating = matches!(
                tool.name.as_ref(),
                APPLY_CHANGES_TOOL_NAME
                    | ROLLBACK_CHANGES_TOOL_NAME
                    | RUN_PROCESS_TOOL_NAME
                    | GIT_STAGE_OWNED_TOOL_NAME
                    | GIT_UNSTAGE_OWNED_TOOL_NAME
                    | GIT_COMMIT_OWNED_TOOL_NAME
                    | GIT_CREATE_BRANCH_TOOL_NAME
                    | GIT_PUSH_OWNED_TOOL_NAME
            );
            assert_eq!(annotations.read_only_hint, Some(!mutating));
            assert_eq!(annotations.destructive_hint, Some(mutating));
            assert_eq!(annotations.open_world_hint, Some(false));
        }
    }

    #[test]
    fn executes_repository_profile_inside_workspace() {
        let temp_dir = tempfile::tempdir().unwrap();
        fs::write(temp_dir.path().join("Cargo.toml"), "[workspace]").unwrap();
        fs::write(temp_dir.path().join("lib.rs"), "pub fn value() {}").unwrap();
        let call = CallToolRequestParams::new(REPOSITORY_PROFILE_TOOL_NAME);

        let result = execute(&enabled_config(), call, temp_dir.path()).unwrap();
        let json: Value = serde_json::from_str(&result_text(result)).unwrap();

        assert_eq!(json["manifests"][0]["kind"], "cargo");
        assert_eq!(json["languages"]["rust"], 1);
    }

    #[test]
    fn exposes_polyglot_project_capabilities_without_execution() {
        let temp_dir = tempfile::tempdir().unwrap();
        fs::create_dir(temp_dir.path().join("web")).unwrap();
        fs::write(
            temp_dir.path().join("web/package.json"),
            r#"{"scripts":{"test":"vitest"},"dependencies":{"react":"latest"}}"#,
        )
        .unwrap();
        fs::write(
            temp_dir.path().join("web/pnpm-lock.yaml"),
            "lockfileVersion: 9",
        )
        .unwrap();

        let result = execute(
            &enabled_config(),
            CallToolRequestParams::new(PROJECT_CAPABILITIES_TOOL_NAME),
            temp_dir.path(),
        )
        .unwrap();
        let json: Value = serde_json::from_str(&result_text(result)).unwrap();

        assert_eq!(json["projects"][0]["ecosystem"], "node");
        assert_eq!(json["projects"][0]["dependencies"][0], "react");
        assert_eq!(
            json["projects"][0]["validation_commands"][0]["program"],
            "pnpm"
        );
        assert_eq!(
            json["projects"][0]["validation_commands"][0]["args"],
            serde_json::json!(["run", "test"])
        );
    }

    #[test]
    fn exposes_direct_repository_intelligence_tools() {
        let temp_dir = tempfile::tempdir().unwrap();
        fs::create_dir(temp_dir.path().join("src")).unwrap();
        fs::write(
            temp_dir.path().join("src/lib.rs"),
            "pub struct Service;\npub fn target() {}\npub fn caller() { target(); }\n",
        )
        .unwrap();
        fs::write(
            temp_dir.path().join("worker.py"),
            "def process_item():\n    return 1\n",
        )
        .unwrap();
        let state = CodingToolState::default();

        let map = execute_with_state(
            &enabled_config(),
            &state,
            CallToolRequestParams::new(REPOSITORY_MAP_TOOL_NAME),
            temp_dir.path(),
        )
        .unwrap();
        let map: Value = serde_json::from_str(&result_text(map)).unwrap();
        assert_eq!(map["symbol_count"], 4);
        assert_eq!(map["files"].as_array().unwrap().len(), 2);
        assert!(map["source_fingerprint"]
            .as_str()
            .is_some_and(|digest| digest.starts_with("blake3:")));

        let symbols = execute_with_state(
            &enabled_config(),
            &state,
            CallToolRequestParams::new(SEARCH_SYMBOLS_TOOL_NAME).with_arguments(object!({
                "query": "process"
            })),
            temp_dir.path(),
        )
        .unwrap();
        let symbols: Value = serde_json::from_str(&result_text(symbols)).unwrap();
        assert_eq!(symbols["matches"][0]["name"], "process_item");
        assert_eq!(symbols["matches"][0]["path"], "worker.py");

        let references = execute_with_state(
            &enabled_config(),
            &state,
            CallToolRequestParams::new(FIND_REFERENCES_TOOL_NAME).with_arguments(object!({
                "symbol": "target"
            })),
            temp_dir.path(),
        )
        .unwrap();
        let references: Value = serde_json::from_str(&result_text(references)).unwrap();
        assert_eq!(references["matches"][0]["caller"], "caller");
        assert_eq!(references["matches"][0]["path"], "src/lib.rs");

        let context = execute_with_state(
            &enabled_config(),
            &state,
            CallToolRequestParams::new(SELECT_CONTEXT_TOOL_NAME).with_arguments(object!({
                "query": "process item"
            })),
            temp_dir.path(),
        )
        .unwrap();
        let context: Value = serde_json::from_str(&result_text(context)).unwrap();
        assert_eq!(context[0]["path"], "worker.py");
        assert!(context[0]["score"].as_u64().unwrap() > 0);
    }

    #[test]
    fn repository_intelligence_cache_tracks_external_content_changes() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("lib.rs");
        fs::write(&path, "pub fn before() {}\n").unwrap();
        let workspace = CodingWorkspace::new(temp_dir.path()).unwrap();
        let state = CodingToolState::default();
        let config = enabled_config();
        let limits = IntelligenceLimits::default();

        let first = state
            .intelligence_index(&config, &workspace, limits)
            .unwrap();
        let cached = state
            .intelligence_index(&config, &workspace, limits)
            .unwrap();
        assert!(Arc::ptr_eq(&first, &cached));

        fs::write(&path, "pub fn after() {}\n").unwrap();
        let refreshed = state
            .intelligence_index(&config, &workspace, limits)
            .unwrap();

        assert!(!Arc::ptr_eq(&first, &refreshed));
        assert!(refreshed
            .symbols
            .iter()
            .any(|symbol| symbol.name == "after"));
        assert!(!refreshed
            .symbols
            .iter()
            .any(|symbol| symbol.name == "before"));
    }

    #[test]
    fn disabled_tree_sitter_fails_intelligence_tools_closed() {
        let temp_dir = tempfile::tempdir().unwrap();
        fs::write(temp_dir.path().join("lib.rs"), "pub fn value() {}\n").unwrap();
        let mut config = enabled_config();
        config.tree_sitter = false;

        let error = execute(
            &config,
            CallToolRequestParams::new(REPOSITORY_MAP_TOOL_NAME),
            temp_dir.path(),
        )
        .unwrap_err();

        assert_eq!(error.code, ErrorCode::INVALID_REQUEST);
        assert!(error.message.contains("Tree-sitter"));
    }

    #[test]
    fn rejects_external_instruction_paths() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path().join("workspace");
        fs::create_dir(&root).unwrap();
        let call =
            CallToolRequestParams::new(REPOSITORY_INSTRUCTIONS_TOOL_NAME).with_arguments(object!({
                "path": temp_dir.path().join("outside")
            }));

        let error = execute(&enabled_config(), call, &root).unwrap_err();

        assert_eq!(error.code, ErrorCode::INVALID_PARAMS);
        assert!(error.message.contains("outside the coding workspace"));
    }

    #[test]
    fn disabled_configuration_fails_closed() {
        let temp_dir = tempfile::tempdir().unwrap();
        let call = CallToolRequestParams::new(REPOSITORY_PROFILE_TOOL_NAME);

        let error = execute(&CodingConfig::default(), call, temp_dir.path()).unwrap_err();

        assert_eq!(error.code, ErrorCode::INVALID_REQUEST);
    }

    #[test]
    fn executes_bounded_text_search() {
        let temp_dir = tempfile::tempdir().unwrap();
        fs::create_dir(temp_dir.path().join("src")).unwrap();
        fs::write(
            temp_dir.path().join("src/lib.rs"),
            "pub fn internal_agent() {}",
        )
        .unwrap();
        let call = CallToolRequestParams::new(SEARCH_TEXT_TOOL_NAME).with_arguments(object!({
            "pattern": "internal_agent",
            "include": ["**/*.rs"],
            "max_results": 10
        }));

        let result = execute(&enabled_config(), call, temp_dir.path()).unwrap();
        let json: Value = serde_json::from_str(&result_text(result)).unwrap();

        assert_eq!(json["matches"][0]["path"], "src/lib.rs");
        assert_eq!(json["matches"][0]["line"], 1);
        assert_eq!(json["truncated"], false);
    }

    #[test]
    fn required_search_arguments_fail_closed() {
        let temp_dir = tempfile::tempdir().unwrap();
        let call = CallToolRequestParams::new(FIND_FILES_TOOL_NAME);

        let error = execute(&enabled_config(), call, temp_dir.path()).unwrap_err();

        assert_eq!(error.code, ErrorCode::INVALID_PARAMS);
        assert!(error.message.contains("missing field `query`"));
    }

    #[test]
    fn reads_versioned_line_ranges_through_internal_tool() {
        let temp_dir = tempfile::tempdir().unwrap();
        fs::write(temp_dir.path().join("app.py"), "one\ntwo\nthree\n").unwrap();
        let call = CallToolRequestParams::new(READ_FILE_TOOL_NAME).with_arguments(object!({
            "path": "app.py",
            "start_line": 2,
            "end_line": 2
        }));

        let result = execute(&enabled_config(), call, temp_dir.path()).unwrap();
        let json: Value = serde_json::from_str(&result_text(result)).unwrap();

        assert_eq!(json["path"], "app.py");
        assert_eq!(json["content"], "two\n");
        assert_eq!(json["total_lines"], 3);
        assert!(json["digest"]
            .as_str()
            .is_some_and(|digest| digest.starts_with("blake3:")));
    }

    #[test]
    fn previews_applies_and_rolls_back_through_shared_agent_state() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("app.py");
        fs::write(&path, "before\n").unwrap();
        let digest = crate::coding::file::content_digest(&fs::read(&path).unwrap());
        let state = CodingToolState::default();
        let change_arguments = object!({
            "changes": [{
                "operation": "write",
                "path": "app.py",
                "expected_digest": digest,
                "content": "after\n"
            }]
        });

        let preview_call = CallToolRequestParams::new(PREVIEW_CHANGES_TOOL_NAME)
            .with_arguments(change_arguments.clone());
        let preview =
            execute_with_state(&enabled_config(), &state, preview_call, temp_dir.path()).unwrap();
        let preview_json: Value = serde_json::from_str(&result_text(preview)).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "before\n");
        assert!(preview_json["files"][0]["diff"]
            .as_str()
            .is_some_and(|diff| diff.contains("+after")));

        let apply_call =
            CallToolRequestParams::new(APPLY_CHANGES_TOOL_NAME).with_arguments(change_arguments);
        let applied =
            execute_with_state(&enabled_config(), &state, apply_call, temp_dir.path()).unwrap();
        let applied_json: Value = serde_json::from_str(&result_text(applied)).unwrap();
        let rollback_id = applied_json["rollback_id"].as_str().unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "after\n");

        let rollback_call =
            CallToolRequestParams::new(ROLLBACK_CHANGES_TOOL_NAME).with_arguments(object!({
                "rollback_id": rollback_id
            }));
        execute_with_state(&enabled_config(), &state, rollback_call, temp_dir.path()).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "before\n");

        let repeated =
            CallToolRequestParams::new(ROLLBACK_CHANGES_TOOL_NAME).with_arguments(object!({
                "rollback_id": rollback_id
            }));
        let error =
            execute_with_state(&enabled_config(), &state, repeated, temp_dir.path()).unwrap_err();
        assert!(error.message.contains("unknown or expired rollback_id"));
    }

    #[test]
    fn oversized_apply_result_is_rejected_before_mutation() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("large.txt");
        let original = format!("{}\n", "a".repeat(700));
        let replacement = format!("{}\n", "b".repeat(700));
        fs::write(&path, &original).unwrap();
        let digest = crate::coding::file::content_digest(&fs::read(&path).unwrap());
        let config = CodingConfig {
            output_limit: 1_024,
            ..enabled_config()
        };
        let call = CallToolRequestParams::new(APPLY_CHANGES_TOOL_NAME).with_arguments(object!({
            "changes": [{
                "operation": "write",
                "path": "large.txt",
                "expected_digest": digest,
                "content": replacement
            }]
        }));

        let error = execute_with_state(&config, &CodingToolState::default(), call, temp_dir.path())
            .unwrap_err();

        assert!(error.message.contains("configured output limit is 1024"));
        assert_eq!(fs::read_to_string(path).unwrap(), original);
    }

    #[tokio::test]
    async fn runs_a_bounded_process_through_direct_async_dispatch() {
        let program = if cfg!(windows) { "python" } else { "python3" };
        if which::which(program).is_err() {
            return;
        }
        let temp_dir = tempfile::tempdir().unwrap();
        let call = CallToolRequestParams::new(RUN_PROCESS_TOOL_NAME).with_arguments(object!({
            "program": program,
            "args": ["-c", "import sys; print('out'); print('err', file=sys.stderr)"],
            "timeout_seconds": 2
        }));

        let result = execute_async(
            &enabled_config(),
            &CodingToolState::default(),
            call,
            temp_dir.path(),
        )
        .await
        .unwrap();
        let json: Value = serde_json::from_str(&result_text(result)).unwrap();

        assert_eq!(json["success"], true);
        assert_eq!(json["exit_code"], 0);
        assert_eq!(json["stdout"].as_str().unwrap().trim(), "out");
        assert_eq!(json["stderr"].as_str().unwrap().trim(), "err");
        assert_eq!(json["timed_out"], false);
    }

    #[tokio::test]
    async fn process_tool_rejects_blocked_commands_and_excessive_timeouts() {
        let temp_dir = tempfile::tempdir().unwrap();
        let blocked = CallToolRequestParams::new(RUN_PROCESS_TOOL_NAME).with_arguments(object!({
            "program": "sh",
            "args": ["-c", "echo unsafe"]
        }));
        let excessive = CallToolRequestParams::new(RUN_PROCESS_TOOL_NAME).with_arguments(object!({
            "program": "rustc",
            "args": ["--version"],
            "timeout_seconds": 121
        }));

        let blocked_error = execute_async(
            &enabled_config(),
            &CodingToolState::default(),
            blocked,
            temp_dir.path(),
        )
        .await
        .unwrap_err();
        let timeout_error = execute_async(
            &enabled_config(),
            &CodingToolState::default(),
            excessive,
            temp_dir.path(),
        )
        .await
        .unwrap_err();

        assert!(blocked_error.message.contains("blocked command"));
        assert!(timeout_error
            .message
            .contains("configured coding shell timeout of 120"));
    }

    #[test]
    fn process_results_are_truncated_to_the_serialized_output_limit() {
        let output = ProcessOutput {
            program: "python3".to_string(),
            cwd: PathBuf::from("."),
            exit_code: Some(0),
            success: true,
            timed_out: false,
            stdout: "\u{1}".repeat(10_000),
            stderr: String::new(),
            stdout_lossy: false,
            stderr_lossy: false,
            output_truncated: false,
            background_process_detected: false,
            output_collection_error: None,
            duration_ms: 1,
        };

        let result = bounded_process_result(output, 1_024).unwrap();
        let text = result_text(result);
        let json: Value = serde_json::from_str(&text).unwrap();

        assert!(text.len() <= 1_024);
        assert_eq!(json["output_truncated"], true);
    }

    #[tokio::test]
    async fn exposes_safe_git_status_diff_and_history_tools() {
        let temp_dir = tempfile::tempdir().unwrap();
        run_git(temp_dir.path(), &["init"]);
        run_git(temp_dir.path(), &["config", "user.name", "Test User"]);
        run_git(
            temp_dir.path(),
            &["config", "user.email", "test@example.com"],
        );
        fs::write(temp_dir.path().join("app.txt"), "before\n").unwrap();
        run_git(temp_dir.path(), &["add", "--", "app.txt"]);
        run_git(temp_dir.path(), &["commit", "-m", "initial"]);
        fs::write(temp_dir.path().join("app.txt"), "after\n").unwrap();

        let status = execute_async(
            &enabled_config(),
            &CodingToolState::default(),
            CallToolRequestParams::new(GIT_STATUS_TOOL_NAME),
            temp_dir.path(),
        )
        .await
        .unwrap();
        let status_json: Value = serde_json::from_str(&result_text(status)).unwrap();
        assert!(status_json["changes"]
            .as_array()
            .is_some_and(|changes| changes.iter().any(|change| change["path"] == "app.txt")));

        let diff = execute_async(
            &enabled_config(),
            &CodingToolState::default(),
            CallToolRequestParams::new(GIT_DIFF_TOOL_NAME).with_arguments(object!({
                "paths": ["app.txt"],
                "context_lines": 1
            })),
            temp_dir.path(),
        )
        .await
        .unwrap();
        let diff_json: Value = serde_json::from_str(&result_text(diff)).unwrap();
        assert!(diff_json["patch"]
            .as_str()
            .is_some_and(|patch| patch.contains("+after")));

        let history = execute_async(
            &enabled_config(),
            &CodingToolState::default(),
            CallToolRequestParams::new(GIT_HISTORY_TOOL_NAME).with_arguments(object!({
                "max_entries": 5
            })),
            temp_dir.path(),
        )
        .await
        .unwrap();
        let history_json: Value = serde_json::from_str(&result_text(history)).unwrap();
        assert_eq!(history_json["commits"][0]["subject"], "initial");
    }

    #[tokio::test]
    async fn stages_and_commits_only_changes_owned_by_shared_agent_state() {
        let temp_dir = tempfile::tempdir().unwrap();
        run_git(temp_dir.path(), &["init"]);
        run_git(temp_dir.path(), &["config", "user.name", "Test User"]);
        run_git(
            temp_dir.path(),
            &["config", "user.email", "test@example.com"],
        );
        let path = temp_dir.path().join("app.txt");
        fs::write(&path, "before\n").unwrap();
        run_git(temp_dir.path(), &["add", "--", "app.txt"]);
        run_git(temp_dir.path(), &["commit", "-m", "initial"]);
        let digest = crate::coding::file::content_digest(&fs::read(&path).unwrap());
        let state = CodingToolState::default();
        let config = enabled_config();
        let apply = CallToolRequestParams::new(APPLY_CHANGES_TOOL_NAME).with_arguments(object!({
            "changes": [{
                "operation": "write",
                "path": "app.txt",
                "expected_digest": digest,
                "content": "after\n"
            }]
        }));
        let applied = execute_with_state(&config, &state, apply, temp_dir.path()).unwrap();
        let applied_json: Value = serde_json::from_str(&result_text(applied)).unwrap();
        let rollback_id = applied_json["rollback_id"].as_str().unwrap().to_string();

        let stage = CallToolRequestParams::new(GIT_STAGE_OWNED_TOOL_NAME).with_arguments(object!({
            "paths": ["app.txt"]
        }));
        let staged = execute_async(&config, &state, stage, temp_dir.path())
            .await
            .unwrap();
        let staged_json: Value = serde_json::from_str(&result_text(staged)).unwrap();
        assert_eq!(staged_json["staged_files"][0], "app.txt");

        let blocked_rollback = CallToolRequestParams::new(ROLLBACK_CHANGES_TOOL_NAME)
            .with_arguments(object!({
                "rollback_id": rollback_id
            }));
        let rollback_error =
            execute_with_state(&config, &state, blocked_rollback, temp_dir.path()).unwrap_err();
        assert!(rollback_error
            .message
            .contains("unstage the owned files before rollback"));

        let unstage =
            CallToolRequestParams::new(GIT_UNSTAGE_OWNED_TOOL_NAME).with_arguments(object!({
                "paths": ["app.txt"]
            }));
        execute_async(&config, &state, unstage, temp_dir.path())
            .await
            .unwrap();
        assert!(state.find(&rollback_id).unwrap().is_some());

        let restage =
            CallToolRequestParams::new(GIT_STAGE_OWNED_TOOL_NAME).with_arguments(object!({
                "paths": ["app.txt"]
            }));
        execute_async(&config, &state, restage, temp_dir.path())
            .await
            .unwrap();

        let commit =
            CallToolRequestParams::new(GIT_COMMIT_OWNED_TOOL_NAME).with_arguments(object!({
                "message": "agent-owned change",
                "paths": ["app.txt"]
            }));
        let committed = execute_async(&config, &state, commit, temp_dir.path())
            .await
            .unwrap();
        let committed_json: Value = serde_json::from_str(&result_text(committed)).unwrap();
        assert_eq!(committed_json["committed_files"][0], "app.txt");
        assert!(state.find(&rollback_id).unwrap().is_none());
        let commit_oid = committed_json["oid"].as_str().unwrap();

        run_git(temp_dir.path(), &["init", "--bare", ".test-remote.git"]);
        run_git(
            temp_dir.path(),
            &["remote", "add", "test-origin", ".test-remote.git"],
        );
        let push = CallToolRequestParams::new(GIT_PUSH_OWNED_TOOL_NAME).with_arguments(object!({
            "oid": commit_oid,
            "remote": "test-origin"
        }));
        let pushed = execute_async(&config, &state, push, temp_dir.path())
            .await
            .unwrap();
        let pushed_json: Value = serde_json::from_str(&result_text(pushed)).unwrap();
        assert_eq!(pushed_json["oid"], commit_oid);
        assert_eq!(pushed_json["remote"], "test-origin");

        let history = execute_async(
            &config,
            &state,
            CallToolRequestParams::new(GIT_HISTORY_TOOL_NAME).with_arguments(object!({
                "max_entries": 1
            })),
            temp_dir.path(),
        )
        .await
        .unwrap();
        let history_json: Value = serde_json::from_str(&result_text(history)).unwrap();
        assert_eq!(history_json["commits"][0]["subject"], "agent-owned change");

        let unknown_stage =
            CallToolRequestParams::new(GIT_STAGE_OWNED_TOOL_NAME).with_arguments(object!({
                "paths": ["app.txt"]
            }));
        let error = execute_async(
            &config,
            &CodingToolState::default(),
            unknown_stage,
            temp_dir.path(),
        )
        .await
        .unwrap_err();
        assert!(error
            .message
            .contains("not retained as an agent-owned mutation"));
    }
}
