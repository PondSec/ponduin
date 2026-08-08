use crate::coding::config::CodingConfig;
use crate::coding::context::{ContextLimits, ContextPlanner};
use crate::coding::embedding::hybrid_context_candidates;
use crate::coding::file::{
    FileReadOptions, FileSnapshot, DEFAULT_READ_LIMIT, MAX_READ_LIMIT, MIN_READ_LIMIT,
};
use crate::coding::git::{GitDiff, GitDiffRequest, GitLimits, GitOwnedPath, GitRepository};
use crate::coding::intelligence::{IntelligenceLimits, RepositoryIndex, RepositoryIntelligence};
use crate::coding::lsp::{
    LanguageServerClient, LanguageServerOperation, LanguageServerPosition, LanguageServerQuery,
};
use crate::coding::patch::{
    MutationBatch, MutationPreview, MutationResult, PatchEngine, PatchLimits, RollbackRecord,
    DEFAULT_PATCH_BATCH_LIMIT, MAX_PATCH_FILE_LIMIT,
};
use crate::coding::process::{ProcessLimits, ProcessOutput, ProcessRequest, ProcessRunner};
use crate::coding::project::ProjectDiscovery;
use crate::coding::review::{ReviewAnalyzer, ReviewReport};
use crate::coding::search::{SearchLimits, TextSearchRequest};
use crate::coding::validation::{ValidationExecution, ValidationService};
use crate::coding::workflow::{
    CodingWorkflow, RepairApproach, RequirementPriority, RequirementSource,
    RequirementVerification, WorkflowCheck, WorkflowCommand, WorkflowId, WorkflowLimits,
    WorkflowNextAction, WorkflowPhase, WorkflowPlan, WorkflowReport, WorkflowRequirement,
    WorkflowStatus, WorkflowTaskState,
};
use crate::coding::{CodingWorkspace, RepositoryInstructions, RepositoryProfile, RepositorySearch};
use ponduin_providers::thinking::ThinkingEffort;
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
pub const ACTIVATE_AGENT_TOOL_NAME: &str = "coding__activate_agent";
pub const CONTINUE_WITHOUT_AGENT_TOOL_NAME: &str = "coding__continue_without_agent";
pub const REPOSITORY_PROFILE_TOOL_NAME: &str = "coding__repository_profile";
pub const REPOSITORY_INSTRUCTIONS_TOOL_NAME: &str = "coding__repository_instructions";
pub const FIND_FILES_TOOL_NAME: &str = "coding__find_files";
pub const SEARCH_TEXT_TOOL_NAME: &str = "coding__search_text";
pub const READ_FILE_TOOL_NAME: &str = "coding__read_file";
pub const PREVIEW_CHANGES_TOOL_NAME: &str = "coding__preview_changes";
pub const APPLY_CHANGES_TOOL_NAME: &str = "coding__apply_changes";
pub const WRITE_FILE_TOOL_NAME: &str = "coding__write_file";
pub const ROLLBACK_CHANGES_TOOL_NAME: &str = "coding__rollback_changes";
pub const RUN_PROCESS_TOOL_NAME: &str = "coding__run_process";
pub const GIT_STATUS_TOOL_NAME: &str = "coding__git_status";
pub const GIT_DIFF_TOOL_NAME: &str = "coding__git_diff";
pub const GIT_HISTORY_TOOL_NAME: &str = "coding__git_history";
pub const GIT_STAGE_OWNED_TOOL_NAME: &str = "coding__git_stage_owned";
pub const GIT_UNSTAGE_OWNED_TOOL_NAME: &str = "coding__git_unstage_owned";
pub const GIT_COMMIT_OWNED_TOOL_NAME: &str = "coding__git_commit_owned";
pub const GIT_REVERT_OWNED_TOOL_NAME: &str = "coding__git_revert_owned";
pub const GIT_CREATE_BRANCH_TOOL_NAME: &str = "coding__git_create_branch";
pub const GIT_PUSH_OWNED_TOOL_NAME: &str = "coding__git_push_owned";
pub const REPOSITORY_MAP_TOOL_NAME: &str = "coding__repository_map";
pub const SEARCH_SYMBOLS_TOOL_NAME: &str = "coding__search_symbols";
pub const FIND_REFERENCES_TOOL_NAME: &str = "coding__find_references";
pub const SELECT_CONTEXT_TOOL_NAME: &str = "coding__select_context";
pub const PROJECT_CAPABILITIES_TOOL_NAME: &str = "coding__project_capabilities";
pub const PREPARE_CONTEXT_TOOL_NAME: &str = "coding__prepare_context";
pub const WORKFLOW_START_TOOL_NAME: &str = "coding__workflow_start";
pub const WORKFLOW_SET_PLAN_TOOL_NAME: &str = "coding__workflow_set_plan";
pub const WORKFLOW_UPDATE_MEMORY_TOOL_NAME: &str = "coding__workflow_update_memory";
pub const WORKFLOW_SET_REPAIR_STRATEGY_TOOL_NAME: &str = "coding__workflow_set_repair_strategy";
pub const WORKFLOW_TRANSITION_TOOL_NAME: &str = "coding__workflow_transition";
pub const WORKFLOW_STATUS_TOOL_NAME: &str = "coding__workflow_status";
pub const WORKFLOW_COMPLETE_TOOL_NAME: &str = "coding__workflow_complete";
pub const RUN_VALIDATION_TOOL_NAME: &str = "coding__run_validation";
pub const REVIEW_CHANGES_TOOL_NAME: &str = "coding__review_changes";
pub const LSP_QUERY_TOOL_NAME: &str = "coding__lsp_query";

const DEFAULT_REPOSITORY_FILE_LIMIT: usize = 50_000;
const MAX_REPOSITORY_FILE_LIMIT: usize = 100_000;
const MAX_ROLLBACK_RECORDS: usize = 20;
const MAX_ROLLBACK_BYTES: usize = 64 * 1_024 * 1_024;
const MAX_INTELLIGENCE_CACHE_ENTRIES: usize = 4;
const MAX_WORKSPACE_WORKFLOWS: usize = 4;

#[derive(Debug, Default)]
pub(crate) struct CodingToolState {
    rollback_journal: Mutex<VecDeque<RollbackJournalEntry>>,
    committed: Mutex<VecDeque<OwnedCommit>>,
    intelligence_cache: Mutex<VecDeque<IntelligenceCacheEntry>>,
    workflows: Mutex<VecDeque<WorkspaceWorkflow>>,
    task_contexts: Mutex<VecDeque<WorkspaceTaskContext>>,
    mutation_lock: Mutex<()>,
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

#[derive(Debug)]
struct WorkspaceWorkflow {
    workspace_root: PathBuf,
    workflow: CodingWorkflow,
}

#[derive(Debug)]
struct WorkspaceTaskContext {
    workspace_root: PathBuf,
    task: WorkflowTaskState,
}

#[derive(Debug, Clone)]
struct WorkflowToolContext {
    status: WorkflowStatus,
}

impl CodingToolState {
    pub(crate) fn has_active_task_state(&self) -> bool {
        if self
            .workflows
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .any(|entry| !entry.workflow.is_terminal())
        {
            return true;
        }
        !self
            .task_contexts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty()
    }

    pub(crate) fn next_action_thinking_effort(&self, workspace_root: &Path) -> ThinkingEffort {
        match self.workflow_status(workspace_root, None) {
            Ok(status)
                if status.phase == WorkflowPhase::Debugging
                    || status.repair_attempts > 0
                    || !status.memory.tool_contract_errors.is_empty() =>
            {
                ThinkingEffort::Low
            }
            _ => ThinkingEffort::Off,
        }
    }

    pub(crate) fn definitions_for_workspace(&self, workspace_root: &Path) -> Vec<Tool> {
        let context = self.workflow_tool_context(workspace_root);
        definitions_for_workflow(context.as_ref())
    }

    pub(crate) fn compact_native_definitions_for_workspace(
        &self,
        _workspace_root: &Path,
    ) -> Vec<Tool> {
        // Ollama-native Qwen runs do not reliably accept a changed tool list on the turn after a
        // tool call. Keep the small contract stable; execute still derives authority from the
        // workflow state through definitions_for_workspace before every call.
        compact_native_definitions(definitions())
    }

    pub(crate) fn workflow_guidance_for_workspace(&self, workspace_root: &Path) -> Option<String> {
        let context = self.workflow_tool_context(workspace_root)?;
        let status = &context.status;
        let guidance = match status.next_action {
            WorkflowNextAction::Inspect => {
                "Inspect only the repository context needed for the objective, then call \
                 coding__workflow_set_plan. Editing and execution tools remain withheld until \
                 the plan is accepted."
                    .to_string()
            }
            WorkflowNextAction::BeginEditing => {
                "The plan is accepted. Call coding__workflow_transition exactly once with \
                 begin_editing; do not repeat a completed transition."
                    .to_string()
            }
            WorkflowNextAction::Modify if status.repair_pending => repair_pending_guidance(),
            WorkflowNextAction::Modify => {
                "The workflow is in Editing with no retained change. Use the currently exposed \
                 mutation tool now. Phase-transition tools are intentionally withheld \
                 until a real change exists."
                    .to_string()
            }
            WorkflowNextAction::BeginReview if status.phase == WorkflowPhase::Editing => {
                "The planned change is retained and the plan requires no validation. Call \
                 coding__workflow_transition with begin_review."
                    .to_string()
            }
            WorkflowNextAction::BeginValidation => {
                "The planned change is retained. Call coding__workflow_transition with \
                 begin_validation before executing checks."
                    .to_string()
            }
            WorkflowNextAction::BeginReview => {
                "Current-revision validation evidence is acceptable. Call \
                 coding__workflow_transition with begin_review."
                    .to_string()
            }
            WorkflowNextAction::Validate => {
                "Run an actual check now. Use coding__run_validation only with an exact command \
                 id returned by coding__project_capabilities; a file path or command text is not \
                 a command id. If no matching discovered command exists, call \
                 coding__run_process with its executable in program and only following arguments \
                 in args. The review transition remains withheld until successful current-revision \
                 evidence exists."
                    .to_string()
            }
            WorkflowNextAction::SetRepairStrategy => {
                "A repeated validation failure is recorded. Inspect its evidence and record a \
                 distinct hypothesis and repair approach with \
                 coding__workflow_set_repair_strategy before attempting another repair."
                    .to_string()
            }
            WorkflowNextAction::BeginRepair => {
                "A validation failure is recorded. Inspect its evidence. For a repeated failure, \
                 record a distinct hypothesis and repair approach with \
                 coding__workflow_set_repair_strategy, then call \
                 coding__workflow_transition exactly once with begin_repair before applying a \
                 corrective change."
                    .to_string()
            }
            WorkflowNextAction::Review => {
                "Run coding__review_changes now. Completion remains withheld until a complete \
                 review of the current revision is recorded."
                    .to_string()
            }
            WorkflowNextAction::Complete => {
                "A complete review of the retained change is recorded. Call \
                 coding__workflow_complete with an evidence-backed summary and remaining risks."
                    .to_string()
            }
            WorkflowNextAction::ReturnResult => {
                "The workflow is complete. Return its evidence-backed result to the user without \
                 starting another workflow."
                    .to_string()
            }
            WorkflowNextAction::ReportBlocked => {
                "The workflow reached a terminal stop condition. Report the machine-detected \
                 stop reason and do not claim completion."
                    .to_string()
            }
            WorkflowNextAction::ReportCancelled => {
                "The workflow was cancelled. Do not resume it or claim completion; wait for a \
                 new user request."
                    .to_string()
            }
        };
        Some(format!(
            "Current internal workflow phase: {:?}; next action: {:?}. {guidance}",
            status.phase, status.next_action
        ))
    }

    fn workflow_tool_context(&self, workspace_root: &Path) -> Option<WorkflowToolContext> {
        self.workflows
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .rev()
            .find(|entry| entry.workspace_root == workspace_root)
            .map(|entry| WorkflowToolContext {
                status: entry.workflow.status(),
            })
    }

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

    fn replace_commit(&self, workspace_root: &Path, old_oid: &str, new_oid: &str) {
        let mut committed = self
            .committed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        committed.retain(|commit| commit.workspace_root != workspace_root || commit.oid != old_oid);
        committed.push_back(OwnedCommit {
            workspace_root: workspace_root.to_path_buf(),
            oid: new_oid.to_string(),
        });
        while committed.len() > MAX_ROLLBACK_RECORDS {
            committed.pop_front();
        }
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

    fn start_workflow(
        &self,
        workspace_root: &Path,
        objective: String,
        config: &CodingConfig,
    ) -> Result<WorkflowStatus, ErrorData> {
        let _mutation = self
            .mutation_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut workflows = self
            .workflows
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if workflows
            .iter()
            .any(|entry| entry.workspace_root == workspace_root && !entry.workflow.is_terminal())
        {
            return Err(invalid_arguments(
                "an unfinished coding workflow already exists for this workspace",
            ));
        }
        workflows.retain(|entry| entry.workspace_root != workspace_root);
        let task = self
            .take_task_context(workspace_root)
            .unwrap_or_else(|| {
                WorkflowTaskState::new(
                    objective.clone(),
                    crate::coding::TaskInteractionMode::Autonomous,
                    workspace_root.to_path_buf(),
                )
                .expect("a validated workflow objective must form a task state")
            })
            .with_objective(objective.clone())
            .map_err(|error| invalid_arguments(error.to_string()))?;
        let workflow = CodingWorkflow::new_with_task(
            objective,
            task,
            WorkflowLimits {
                max_iterations: config.max_iterations,
                max_repair_attempts: config.max_repair_attempts,
            },
        )
        .map_err(|error| invalid_arguments(error.to_string()))?;
        let status = workflow.status();
        workflows.push_back(WorkspaceWorkflow {
            workspace_root: workspace_root.to_path_buf(),
            workflow,
        });
        while workflows.len() > MAX_WORKSPACE_WORKFLOWS {
            workflows.pop_front();
        }
        Ok(status)
    }

    pub(crate) fn register_task_context(
        &self,
        workspace_root: &Path,
        original_user_request: String,
        interaction_mode: crate::coding::TaskInteractionMode,
    ) -> Result<(), ErrorData> {
        let _mutation = self
            .mutation_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let task = WorkflowTaskState::new(
            original_user_request,
            interaction_mode,
            workspace_root.to_path_buf(),
        )
        .map_err(|error| invalid_arguments(error.to_string()))?;
        let workflows = self
            .workflows
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if workflows
            .iter()
            .any(|entry| entry.workspace_root == workspace_root && !entry.workflow.is_terminal())
        {
            return Ok(());
        }
        drop(workflows);

        let mut contexts = self
            .task_contexts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        contexts.retain(|entry| entry.workspace_root != workspace_root);
        contexts.push_back(WorkspaceTaskContext {
            workspace_root: workspace_root.to_path_buf(),
            task,
        });
        while contexts.len() > MAX_WORKSPACE_WORKFLOWS {
            contexts.pop_front();
        }
        Ok(())
    }

    pub(crate) fn recovery_instruction(&self, workspace_root: &Path) -> Option<String> {
        let status = match self.workflow_status(workspace_root, None) {
            Ok(status) => status,
            Err(_) => return self.workflow_continuation(workspace_root),
        };
        if matches!(
            status.phase,
            WorkflowPhase::Completed
                | WorkflowPhase::Blocked
                | WorkflowPhase::Failed
                | WorkflowPhase::Cancelled
        ) {
            return None;
        }
        let guidance = self.workflow_guidance_for_workspace(workspace_root)?;
        Some(format!(
            "Resume the active coding task after an internal model failure. Original user request: \
             {}. Objective: {}. Intent: {:?}. Interaction mode: {:?}. {} Do not ask the user to \
             repeat the task; take the next allowed tool action.",
            status.task.original_user_request,
            status.task.normalized_objective,
            status.task.intent,
            status.task.interaction_mode,
            guidance,
        ))
    }

    pub(crate) fn active_workflow_id(&self, workspace_root: &Path) -> Option<WorkflowId> {
        self.workflow_status(workspace_root, None)
            .ok()
            .map(|status| status.id)
    }

    pub(crate) fn active_workflow_continuation(&self, workspace_root: &Path) -> Option<String> {
        let status = self.workflow_status(workspace_root, None).ok()?;
        if matches!(
            status.phase,
            WorkflowPhase::Completed
                | WorkflowPhase::Blocked
                | WorkflowPhase::Failed
                | WorkflowPhase::Cancelled
        ) {
            return None;
        }
        let guidance = self.workflow_guidance_for_workspace(workspace_root)?;
        Some(format!(
            "The active coding workflow is incomplete. {} Do not provide a final prose response \
             until the workflow has reached an evidence-backed terminal state.",
            guidance
        ))
    }

    pub(crate) fn workflow_continuation(&self, workspace_root: &Path) -> Option<String> {
        if let Some(continuation) = self.active_workflow_continuation(workspace_root) {
            return Some(continuation);
        }
        let task = self.pending_task_context(workspace_root)?;
        Some(format!(
            "A coding task is active but no workflow has been started yet. Retained objective: {}. \
             Call coding__workflow_start now with a concise objective, then inspect only what is \
             needed and set the concrete plan. Do not stop after narration, announce a tool, or \
             use execute_typescript or an invented wrapper for direct file or process work; use \
             the currently exposed coding__ tools.",
            task.normalized_objective
        ))
    }

    pub(crate) fn terminal_workflow_message(&self, workspace_root: &Path) -> Option<String> {
        let status = self.workflow_status(workspace_root, None).ok()?;
        if !matches!(
            status.phase,
            WorkflowPhase::Blocked | WorkflowPhase::Failed | WorkflowPhase::Cancelled
        ) {
            return None;
        }
        Some(format!(
            "The coding workflow stopped at {:?} with recorded reason {:?}. The original request \
             and workflow evidence were retained; no user resubmission is required.",
            status.phase, status.stop_reason
        ))
    }

    pub(crate) fn recovery_exhausted_message(&self, workspace_root: &Path) -> Option<String> {
        let status = match self.workflow_status(workspace_root, None) {
            Ok(status) => status,
            Err(_) => {
                let task = self.pending_task_context(workspace_root)?;
                return Some(format!(
                    "The coding task remains preserved before workflow startup after repeated \
                     unusable model responses. Retained objective: {}. No user resubmission is \
                     required.",
                    task.normalized_objective
                ));
            }
        };
        if matches!(
            status.phase,
            WorkflowPhase::Completed
                | WorkflowPhase::Blocked
                | WorkflowPhase::Failed
                | WorkflowPhase::Cancelled
        ) {
            return None;
        }
        Some(format!(
            "The active coding task remains preserved at {:?} after repeated empty model responses. \
             The original request and workflow evidence were retained; no user resubmission is required.",
            status.phase
        ))
    }

    fn take_task_context(&self, workspace_root: &Path) -> Option<WorkflowTaskState> {
        let mut contexts = self
            .task_contexts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        contexts
            .iter()
            .position(|entry| entry.workspace_root == workspace_root)
            .and_then(|position| contexts.remove(position))
            .map(|entry| entry.task)
    }

    pub(crate) fn pending_task_context(&self, workspace_root: &Path) -> Option<WorkflowTaskState> {
        self.task_contexts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .rev()
            .find(|entry| entry.workspace_root == workspace_root)
            .map(|entry| entry.task.clone())
    }

    fn note_repository_activity(&self, workspace_root: &Path) {
        let _mutation = self
            .mutation_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.with_current_workflow_mut(workspace_root, |workflow| {
            workflow.note_repository_activity();
        });
    }

    fn note_read_files(&self, workspace_root: &Path, paths: Vec<PathBuf>) {
        let _mutation = self
            .mutation_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.with_current_workflow_mut(workspace_root, |workflow| {
            workflow.note_read_files(paths);
        });
    }

    fn note_symbols(
        &self,
        workspace_root: &Path,
        symbols: &[crate::coding::intelligence::CodeSymbol],
    ) {
        let _mutation = self
            .mutation_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.with_current_workflow_mut(workspace_root, |workflow| {
            workflow.note_symbols(symbols);
        });
    }

    fn set_workflow_plan(
        &self,
        workspace_root: &Path,
        workflow_id: &WorkflowId,
        plan: WorkflowPlan,
    ) -> Result<WorkflowStatus, ErrorData> {
        let _mutation = self
            .mutation_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.with_workflow_mut(workspace_root, workflow_id, |workflow| {
            workflow
                .set_plan(plan)
                .map_err(|error| invalid_arguments(error.to_string()))?;
            Ok(workflow.status())
        })
    }

    fn update_workflow_memory(
        &self,
        workspace_root: &Path,
        workflow_id: &WorkflowId,
        assumptions: Option<Vec<String>>,
        open_points: Option<Vec<String>>,
    ) -> Result<WorkflowStatus, ErrorData> {
        let _mutation = self
            .mutation_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.with_workflow_mut(workspace_root, workflow_id, |workflow| {
            workflow
                .update_memory_notes(assumptions, open_points)
                .map_err(|error| invalid_arguments(error.to_string()))?;
            Ok(workflow.status())
        })
    }

    fn set_repair_strategy(
        &self,
        workspace_root: &Path,
        workflow_id: &WorkflowId,
        approach: RepairApproach,
        hypothesis: String,
        target_files: Vec<PathBuf>,
    ) -> Result<WorkflowStatus, ErrorData> {
        let _mutation = self
            .mutation_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.with_workflow_mut(workspace_root, workflow_id, |workflow| {
            workflow
                .set_repair_strategy(approach, hypothesis, target_files)
                .map_err(|error| invalid_arguments(error.to_string()))?;
            Ok(workflow.status())
        })
    }

    fn transition_workflow(
        &self,
        workspace_root: &Path,
        workflow_id: &WorkflowId,
        transition: WorkflowTransition,
    ) -> Result<WorkflowStatus, ErrorData> {
        let _mutation = self
            .mutation_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.with_workflow_mut(workspace_root, workflow_id, |workflow| {
            match transition {
                WorkflowTransition::Editing => workflow.begin_editing(),
                WorkflowTransition::Validation => workflow.begin_validation(),
                WorkflowTransition::Repair => workflow.begin_repair(),
                WorkflowTransition::Review => workflow.begin_review(),
            }
            .map_err(|error| invalid_arguments(error.to_string()))?;
            Ok(workflow.status())
        })
    }

    fn workflow_status(
        &self,
        workspace_root: &Path,
        workflow_id: Option<&WorkflowId>,
    ) -> Result<WorkflowStatus, ErrorData> {
        self.workflows
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .rev()
            .find(|entry| {
                entry.workspace_root == workspace_root
                    && workflow_id.is_none_or(|id| entry.workflow.id() == id)
            })
            .map(|entry| entry.workflow.status())
            .ok_or_else(|| invalid_arguments("no matching coding workflow exists"))
    }

    fn complete_workflow(
        &self,
        workspace_root: &Path,
        workflow_id: &WorkflowId,
        summary: String,
        remaining_risks: Vec<String>,
    ) -> Result<WorkflowReport, ErrorData> {
        let _mutation = self
            .mutation_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.with_workflow_mut(workspace_root, workflow_id, |workflow| {
            workflow
                .complete(summary, remaining_risks)
                .map_err(|error| invalid_arguments(error.to_string()))
        })
    }

    fn record_process(
        &self,
        workspace_root: &Path,
        program: &str,
        args: &[String],
        output: &ProcessOutput,
    ) -> Result<(), ErrorData> {
        let _mutation = self
            .mutation_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(result) = self.with_current_workflow_mut(workspace_root, |workflow| {
            workflow
                .record_process(program, args, output)
                .map_err(|error| internal_error(error.to_string()))
        }) {
            result?;
        }
        Ok(())
    }

    fn record_validation_execution(
        &self,
        workspace_root: &Path,
        execution: &ValidationExecution,
    ) -> Result<(), ErrorData> {
        let _mutation = self
            .mutation_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(result) = self.with_current_workflow_mut(workspace_root, |workflow| {
            workflow
                .record_validation_execution(execution)
                .map_err(|error| internal_error(error.to_string()))
        }) {
            result?;
        }
        Ok(())
    }

    fn record_review(&self, workspace_root: &Path, review: &ReviewReport) -> Result<(), ErrorData> {
        let _mutation = self
            .mutation_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(result) = self.with_current_workflow_mut(workspace_root, |workflow| {
            workflow
                .record_review(review)
                .map_err(|error| invalid_arguments(error.to_string()))
        }) {
            result?;
        }
        Ok(())
    }

    pub(crate) fn record_tool_contract_failure(
        &self,
        workspace_root: &Path,
        tool_name: &str,
        error_class: &str,
    ) -> Option<usize> {
        let _mutation = self
            .mutation_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.with_current_workflow_mut(workspace_root, |workflow| {
            workflow.record_tool_contract_failure(tool_name, error_class)
        })
    }

    pub(crate) fn block_for_action_limit(&self, workspace_root: &Path, limit: u32) {
        let _mutation = self
            .mutation_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.with_current_workflow_mut(workspace_root, |workflow| {
            workflow.block_for_action_limit(limit);
        });
    }

    pub(crate) fn cancel_active_workflow(&self, workspace_root: &Path) {
        let _mutation = self
            .mutation_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.with_current_workflow_mut(workspace_root, CodingWorkflow::cancel);
        self.task_contexts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|entry| entry.workspace_root != workspace_root);
    }

    fn mutation_guard(&self) -> std::sync::MutexGuard<'_, ()> {
        self.mutation_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn authorize_mutation_locked(&self, workspace_root: &Path) -> Result<(), ErrorData> {
        let authorization = self.with_current_workflow_mut(workspace_root, |workflow| {
            if workflow.is_terminal() {
                None
            } else {
                Some(
                    workflow
                        .authorize_change()
                        .map_err(|error| invalid_arguments(error.to_string())),
                )
            }
        });
        match authorization.flatten() {
            Some(result) => result,
            None => Err(invalid_arguments(
                "every workspace mutation requires an active coding workflow with an accepted plan",
            )),
        }
    }

    fn record_mutation_locked(
        &self,
        workspace_root: &Path,
        change_id: String,
        preview: &MutationPreview,
    ) -> Result<(), ErrorData> {
        if let Some(result) = self.with_current_workflow_mut(workspace_root, |workflow| {
            if workflow.is_terminal() {
                Ok(())
            } else {
                workflow
                    .record_change(change_id, preview)
                    .map_err(|error| internal_error(error.to_string()))
            }
        }) {
            result?;
        }
        Ok(())
    }

    fn record_rollback_locked(
        &self,
        workspace_root: &Path,
        change_id: &str,
    ) -> Result<(), ErrorData> {
        if let Some(result) = self.with_current_workflow_mut(workspace_root, |workflow| {
            if workflow.tracks_change(change_id) {
                workflow
                    .record_rollback(change_id)
                    .map_err(|error| internal_error(error.to_string()))
            } else {
                Ok(())
            }
        }) {
            result?;
        }
        Ok(())
    }

    fn with_workflow_mut<R>(
        &self,
        workspace_root: &Path,
        workflow_id: &WorkflowId,
        operation: impl FnOnce(&mut CodingWorkflow) -> Result<R, ErrorData>,
    ) -> Result<R, ErrorData> {
        let mut workflows = self
            .workflows
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let workflow = workflows
            .iter_mut()
            .rev()
            .find(|entry| {
                entry.workspace_root == workspace_root && entry.workflow.id() == workflow_id
            })
            .map(|entry| &mut entry.workflow)
            .ok_or_else(|| invalid_arguments("no matching coding workflow exists"))?;
        operation(workflow)
    }

    fn with_current_workflow_mut<R>(
        &self,
        workspace_root: &Path,
        operation: impl FnOnce(&mut CodingWorkflow) -> R,
    ) -> Option<R> {
        let mut workflows = self
            .workflows
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        workflows
            .iter_mut()
            .rev()
            .find(|entry| entry.workspace_root == workspace_root)
            .map(|entry| operation(&mut entry.workflow))
    }
}

fn repair_pending_guidance() -> String {
    "A repair is active. The diagnostic path is the primary repair target: read that existing \
     file before changing it, and do not create or rewrite another existing file unless the \
     diagnostic proves it is the cause. For an undefined symbol, repair the failing file's import \
     or reference before changing the symbol definition. Read every existing file you will write \
     to obtain each file's current digest, then apply one substantive correction that directly \
     addresses the latest process diagnostic in the conversation. Each write needs that target \
     file's own expected_digest from coding__read_file; never reuse a digest or omit one in a \
     multi-file change. Do not make formatting-only changes, repeat the failed content, or begin \
     validation until that correction is retained."
        .to_string()
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
        write_file_tool(),
        rollback_changes_tool(),
        run_process_tool(),
        git_status_tool(),
        git_diff_tool(),
        git_history_tool(),
        git_stage_owned_tool(),
        git_unstage_owned_tool(),
        git_commit_owned_tool(),
        git_revert_owned_tool(),
        git_create_branch_tool(),
        git_push_owned_tool(),
        repository_map_tool(),
        search_symbols_tool(),
        find_references_tool(),
        select_context_tool(),
        project_capabilities_tool(),
        prepare_context_tool(),
        workflow_start_tool(),
        workflow_set_plan_tool(),
        workflow_update_memory_tool(),
        workflow_set_repair_strategy_tool(),
        workflow_transition_tool(),
        workflow_status_tool(),
        workflow_complete_tool(),
        run_validation_tool(),
        review_changes_tool(),
        lsp_query_tool(),
    ]
}

fn compact_native_definitions(tools: Vec<Tool>) -> Vec<Tool> {
    tools
        .into_iter()
        .filter(|tool| compact_native_tool_allowed(tool.name.as_ref()))
        .map(compact_native_tool_schema)
        .collect()
}

fn compact_native_tool_allowed(name: &str) -> bool {
    matches!(
        name,
        REPOSITORY_PROFILE_TOOL_NAME
            | REPOSITORY_INSTRUCTIONS_TOOL_NAME
            | FIND_FILES_TOOL_NAME
            | SEARCH_TEXT_TOOL_NAME
            | READ_FILE_TOOL_NAME
            | PROJECT_CAPABILITIES_TOOL_NAME
            | WORKFLOW_START_TOOL_NAME
            | WORKFLOW_SET_PLAN_TOOL_NAME
            | WORKFLOW_TRANSITION_TOOL_NAME
            | WORKFLOW_STATUS_TOOL_NAME
            | WORKFLOW_COMPLETE_TOOL_NAME
            | WRITE_FILE_TOOL_NAME
            | ROLLBACK_CHANGES_TOOL_NAME
            | RUN_PROCESS_TOOL_NAME
            | RUN_VALIDATION_TOOL_NAME
            | REVIEW_CHANGES_TOOL_NAME
    )
}

fn compact_native_tool_schema(mut tool: Tool) -> Tool {
    tool.description = Some(
        match tool.name.as_ref() {
            REPOSITORY_PROFILE_TOOL_NAME => "Inspect repository.",
            REPOSITORY_INSTRUCTIONS_TOOL_NAME => "Read root instructions.",
            FIND_FILES_TOOL_NAME => "Find files; terms and globs allowed.",
            SEARCH_TEXT_TOOL_NAME => "Search text; pattern required.",
            READ_FILE_TOOL_NAME => "Read one relative path.",
            PROJECT_CAPABILITIES_TOOL_NAME => "Inspect capabilities.",
            WORKFLOW_START_TOOL_NAME => "Start workflow; objective required.",
            WORKFLOW_SET_PLAN_TOOL_NAME => "Set inspected plan.",
            WORKFLOW_TRANSITION_TOOL_NAME => "Advance workflow phase.",
            WORKFLOW_STATUS_TOOL_NAME => "Read workflow status.",
            WORKFLOW_COMPLETE_TOOL_NAME => "Complete after review.",
            APPLY_CHANGES_TOOL_NAME => "Apply versioned changes.",
            WRITE_FILE_TOOL_NAME => "Create or update one file.",
            ROLLBACK_CHANGES_TOOL_NAME => "Rollback change batch.",
            RUN_PROCESS_TOOL_NAME => "Run validation process.",
            RUN_VALIDATION_TOOL_NAME => "Run planned validation.",
            REVIEW_CHANGES_TOOL_NAME => "Review retained changes.",
            _ => "Compact coding workflow action.",
        }
        .into(),
    );
    tool.input_schema = match tool.name.as_ref() {
        REPOSITORY_PROFILE_TOOL_NAME | PROJECT_CAPABILITIES_TOOL_NAME => object!({
            "type": "object",
            "properties": {"max_files": {"type": "integer"}}
        }),
        REPOSITORY_INSTRUCTIONS_TOOL_NAME => object!({
            "type": "object",
            "properties": {"path": {"type": "string"}}
        }),
        FIND_FILES_TOOL_NAME => object!({
            "type": "object",
            "required": ["query"],
            "properties": {"query": {"type": "string"}}
        }),
        SEARCH_TEXT_TOOL_NAME => object!({
            "type": "object",
            "required": ["pattern"],
            "properties": {"pattern": {"type": "string"}, "scope": {"type": "string"}}
        }),
        READ_FILE_TOOL_NAME => object!({
            "type": "object",
            "required": ["path"],
            "properties": {"path": {"type": "string"}}
        }),
        WORKFLOW_START_TOOL_NAME => object!({
            "type": "object",
            "required": ["objective"],
            "properties": {"objective": {"type": "string"}}
        }),
        PREVIEW_CHANGES_TOOL_NAME | APPLY_CHANGES_TOOL_NAME => object!({
            "type": "object",
            "required": ["changes"],
            "properties": {
                "changes": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "required": ["operation", "path"],
                        "properties": {
                            "operation": {"type": "string", "enum": ["create", "write", "replace", "delete", "move"]},
                            "path": {"type": "string"},
                            "content": {"type": "string"},
                            "expected_digest": {"type": "string"},
                            "destination": {"type": "string"}
                        }
                    }
                }
            }
        }),
        WRITE_FILE_TOOL_NAME => object!({
            "type": "object",
            "required": ["path", "content"],
            "properties": {
                "path": {"type": "string"},
                "content": {"type": "string"},
                "expected_digest": {"type": "string"}
            }
        }),
        ROLLBACK_CHANGES_TOOL_NAME => object!({
            "type": "object",
            "required": ["rollback_id"],
            "properties": {"rollback_id": {"type": "string"}}
        }),
        RUN_PROCESS_TOOL_NAME => object!({
            "type": "object",
            "required": ["program"],
            "properties": {
                "program": {"type": "string"},
                "args": {"type": "array", "items": {"type": "string"}},
                "timeout_seconds": {"type": "integer"}
            }
        }),
        WORKFLOW_SET_PLAN_TOOL_NAME => object!({
            "type": "object",
            "required": ["workflow_id", "relevant_files", "intended_change", "validation_program", "plan_steps"],
            "properties": {
                "workflow_id": {"type": "string"},
                "relevant_files": {"type": "array", "items": {"type": "string"}},
                "intended_change": {"type": "string"},
                "plan_steps": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": 12,
                    "description": "Concrete ordered task actions, never generic phases.",
                    "items": {"type": "string"}
                },
                "validation_program": {"type": "string"},
                "args": {"type": "array", "items": {"type": "string"}}
            }
        }),
        WORKFLOW_TRANSITION_TOOL_NAME => object!({
            "type": "object",
            "required": ["workflow_id", "transition"],
            "properties": {
                "workflow_id": {"type": "string"},
                "transition": {"type": "string", "enum": ["begin_editing", "begin_validation", "begin_repair", "begin_review"]}
            }
        }),
        WORKFLOW_STATUS_TOOL_NAME => object!({
            "type": "object",
            "properties": {"workflow_id": {"type": "string"}}
        }),
        WORKFLOW_COMPLETE_TOOL_NAME => object!({
            "type": "object",
            "required": ["workflow_id", "summary", "remaining_risks"],
            "properties": {
                "workflow_id": {"type": "string"},
                "summary": {"type": "string"},
                "remaining_risks": {"type": "array", "items": {"type": "string"}}
            }
        }),
        RUN_VALIDATION_TOOL_NAME => object!({
            "type": "object",
            "required": ["command_id"],
            "properties": {"command_id": {"type": "string"}, "timeout_seconds": {"type": "integer"}}
        }),
        REVIEW_CHANGES_TOOL_NAME => object!({"type": "object"}),
        _ => return tool,
    }
    .into();
    tool
}

fn definitions_for_workflow(context: Option<&WorkflowToolContext>) -> Vec<Tool> {
    let Some(context) = context else {
        return definitions()
            .into_iter()
            .filter(|tool| {
                !matches!(
                    tool.name.as_ref(),
                    PREVIEW_CHANGES_TOOL_NAME
                        | APPLY_CHANGES_TOOL_NAME
                        | WRITE_FILE_TOOL_NAME
                        | ROLLBACK_CHANGES_TOOL_NAME
                        | RUN_PROCESS_TOOL_NAME
                        | GIT_STAGE_OWNED_TOOL_NAME
                        | GIT_UNSTAGE_OWNED_TOOL_NAME
                        | GIT_COMMIT_OWNED_TOOL_NAME
                        | GIT_REVERT_OWNED_TOOL_NAME
                        | GIT_CREATE_BRANCH_TOOL_NAME
                        | GIT_PUSH_OWNED_TOOL_NAME
                        | RUN_VALIDATION_TOOL_NAME
                        | REVIEW_CHANGES_TOOL_NAME
                        | WORKFLOW_SET_PLAN_TOOL_NAME
                        | WORKFLOW_UPDATE_MEMORY_TOOL_NAME
                        | WORKFLOW_SET_REPAIR_STRATEGY_TOOL_NAME
                        | WORKFLOW_TRANSITION_TOOL_NAME
                        | WORKFLOW_STATUS_TOOL_NAME
                        | WORKFLOW_COMPLETE_TOOL_NAME
                )
            })
            .collect();
    };

    let status = &context.status;
    if matches!(
        status.phase,
        WorkflowPhase::Completed
            | WorkflowPhase::Blocked
            | WorkflowPhase::Failed
            | WorkflowPhase::Cancelled
    ) {
        return definitions()
            .into_iter()
            .filter(|tool| {
                matches!(
                    tool.name.as_ref(),
                    REPOSITORY_PROFILE_TOOL_NAME
                        | REPOSITORY_INSTRUCTIONS_TOOL_NAME
                        | FIND_FILES_TOOL_NAME
                        | SEARCH_TEXT_TOOL_NAME
                        | READ_FILE_TOOL_NAME
                        | GIT_STATUS_TOOL_NAME
                        | GIT_DIFF_TOOL_NAME
                        | GIT_HISTORY_TOOL_NAME
                        | WORKFLOW_START_TOOL_NAME
                        | WORKFLOW_STATUS_TOOL_NAME
                )
            })
            .collect();
    }

    let phase_allows = |name: &str| match status.phase {
        WorkflowPhase::Analyzing | WorkflowPhase::Searching => matches!(
            name,
            REPOSITORY_PROFILE_TOOL_NAME
                | REPOSITORY_INSTRUCTIONS_TOOL_NAME
                | FIND_FILES_TOOL_NAME
                | SEARCH_TEXT_TOOL_NAME
                | READ_FILE_TOOL_NAME
                | GIT_STATUS_TOOL_NAME
                | GIT_DIFF_TOOL_NAME
                | GIT_HISTORY_TOOL_NAME
                | REPOSITORY_MAP_TOOL_NAME
                | SEARCH_SYMBOLS_TOOL_NAME
                | FIND_REFERENCES_TOOL_NAME
                | SELECT_CONTEXT_TOOL_NAME
                | PROJECT_CAPABILITIES_TOOL_NAME
                | PREPARE_CONTEXT_TOOL_NAME
                | WORKFLOW_SET_PLAN_TOOL_NAME
                | WORKFLOW_UPDATE_MEMORY_TOOL_NAME
                | WORKFLOW_STATUS_TOOL_NAME
                | LSP_QUERY_TOOL_NAME
        ),
        WorkflowPhase::Planning => matches!(
            name,
            FIND_FILES_TOOL_NAME
                | SEARCH_TEXT_TOOL_NAME
                | READ_FILE_TOOL_NAME
                | GIT_STATUS_TOOL_NAME
                | GIT_DIFF_TOOL_NAME
                | REPOSITORY_MAP_TOOL_NAME
                | SELECT_CONTEXT_TOOL_NAME
                | WORKFLOW_UPDATE_MEMORY_TOOL_NAME
                | WORKFLOW_STATUS_TOOL_NAME
        ),
        WorkflowPhase::Editing => matches!(
            name,
            FIND_FILES_TOOL_NAME
                | SEARCH_TEXT_TOOL_NAME
                | READ_FILE_TOOL_NAME
                | PREVIEW_CHANGES_TOOL_NAME
                | APPLY_CHANGES_TOOL_NAME
                | WRITE_FILE_TOOL_NAME
                | ROLLBACK_CHANGES_TOOL_NAME
                | GIT_STATUS_TOOL_NAME
                | GIT_DIFF_TOOL_NAME
                | WORKFLOW_STATUS_TOOL_NAME
        ),
        WorkflowPhase::Testing => matches!(
            name,
            FIND_FILES_TOOL_NAME
                | SEARCH_TEXT_TOOL_NAME
                | READ_FILE_TOOL_NAME
                | GIT_DIFF_TOOL_NAME
                | PROJECT_CAPABILITIES_TOOL_NAME
                | RUN_PROCESS_TOOL_NAME
                | RUN_VALIDATION_TOOL_NAME
                | WORKFLOW_UPDATE_MEMORY_TOOL_NAME
                | WORKFLOW_STATUS_TOOL_NAME
        ),
        WorkflowPhase::Debugging => matches!(
            name,
            FIND_FILES_TOOL_NAME
                | SEARCH_TEXT_TOOL_NAME
                | READ_FILE_TOOL_NAME
                | GIT_DIFF_TOOL_NAME
                | REPOSITORY_MAP_TOOL_NAME
                | SEARCH_SYMBOLS_TOOL_NAME
                | FIND_REFERENCES_TOOL_NAME
                | SELECT_CONTEXT_TOOL_NAME
                | PREPARE_CONTEXT_TOOL_NAME
                | WORKFLOW_UPDATE_MEMORY_TOOL_NAME
                | WORKFLOW_SET_REPAIR_STRATEGY_TOOL_NAME
                | WORKFLOW_STATUS_TOOL_NAME
                | LSP_QUERY_TOOL_NAME
        ),
        WorkflowPhase::Reviewing => matches!(
            name,
            SEARCH_TEXT_TOOL_NAME
                | READ_FILE_TOOL_NAME
                | GIT_STATUS_TOOL_NAME
                | GIT_DIFF_TOOL_NAME
                | GIT_HISTORY_TOOL_NAME
                | GIT_STAGE_OWNED_TOOL_NAME
                | GIT_UNSTAGE_OWNED_TOOL_NAME
                | GIT_COMMIT_OWNED_TOOL_NAME
                | GIT_REVERT_OWNED_TOOL_NAME
                | GIT_CREATE_BRANCH_TOOL_NAME
                | GIT_PUSH_OWNED_TOOL_NAME
                | REVIEW_CHANGES_TOOL_NAME
                | WORKFLOW_UPDATE_MEMORY_TOOL_NAME
                | WORKFLOW_STATUS_TOOL_NAME
        ),
        WorkflowPhase::Completed
        | WorkflowPhase::Blocked
        | WorkflowPhase::Failed
        | WorkflowPhase::Cancelled => false,
    };

    let mut tools = definitions()
        .into_iter()
        .filter(|tool| {
            tool.name.as_ref() != WORKFLOW_START_TOOL_NAME
                && tool.name.as_ref() != WORKFLOW_TRANSITION_TOOL_NAME
                && phase_allows(tool.name.as_ref())
        })
        .collect::<Vec<_>>();

    let next_transition = match status.next_action {
        WorkflowNextAction::BeginEditing => Some((
            "begin_editing",
            "The accepted plan makes begin_editing the only valid next workflow transition.",
        )),
        WorkflowNextAction::BeginReview if status.phase == WorkflowPhase::Editing => Some((
            "begin_review",
            "The retained change requires no validation, so begin_review is the only valid next \
             workflow transition.",
        )),
        WorkflowNextAction::BeginValidation => Some((
            "begin_validation",
            "A retained change exists, so begin_validation is the only valid next workflow \
             transition.",
        )),
        WorkflowNextAction::BeginReview => Some((
            "begin_review",
            "Current-revision validation evidence is acceptable, so begin_review is the only \
             valid next workflow transition.",
        )),
        WorkflowNextAction::BeginRepair => Some((
            "begin_repair",
            "Recorded failure evidence makes begin_repair the only valid next workflow transition.",
        )),
        _ => None,
    };
    if let Some((transition, description)) = next_transition {
        tools.push(workflow_transition_tool_for(transition, description));
    }
    if status.next_action == WorkflowNextAction::Complete {
        tools.push(workflow_complete_tool());
    }
    tools
}

pub(crate) fn routing_definitions() -> Vec<Tool> {
    let empty_input = || {
        object!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        })
    };
    vec![
        Tool::new(
            ACTIVATE_AGENT_TOOL_NAME.to_string(),
            "Select this route when fulfilling the current user turn requires inspecting, \
             creating, changing, validating, debugging, reviewing, or otherwise working with a \
             software project. Decide from the request's complete semantic meaning and \
             conversation context, not keywords."
                .to_string(),
            empty_input(),
        )
        .annotate(read_only_annotations(
            "Route through internal coding capability",
        )),
        Tool::new(
            CONTINUE_WITHOUT_AGENT_TOOL_NAME.to_string(),
            "Select this route when the current user turn can be fulfilled without working with a \
             software project, including general knowledge, conversation, and explanations that \
             require no repository inspection or change. Decide from the request's complete \
             semantic meaning and conversation context, not keywords."
                .to_string(),
            empty_input(),
        )
        .annotate(read_only_annotations(
            "Continue without internal coding capability",
        )),
    ]
}

pub fn is_reserved_name(name: &str) -> bool {
    name.starts_with(CODING_TOOL_PREFIX)
}

pub(crate) fn canonical_native_tool_name(name: &str) -> Option<&'static str> {
    match name {
        "repository_profile" => Some(REPOSITORY_PROFILE_TOOL_NAME),
        "repository_instructions" => Some(REPOSITORY_INSTRUCTIONS_TOOL_NAME),
        "find_files" => Some(FIND_FILES_TOOL_NAME),
        "search_text" => Some(SEARCH_TEXT_TOOL_NAME),
        "read_file" => Some(READ_FILE_TOOL_NAME),
        "project_capabilities" => Some(PROJECT_CAPABILITIES_TOOL_NAME),
        "workflow_start" => Some(WORKFLOW_START_TOOL_NAME),
        "workflow_set_plan" => Some(WORKFLOW_SET_PLAN_TOOL_NAME),
        "workflow_transition" => Some(WORKFLOW_TRANSITION_TOOL_NAME),
        "workflow_status" => Some(WORKFLOW_STATUS_TOOL_NAME),
        "workflow_complete" => Some(WORKFLOW_COMPLETE_TOOL_NAME),
        "apply_changes" => Some(APPLY_CHANGES_TOOL_NAME),
        "write_file" => Some(WRITE_FILE_TOOL_NAME),
        "rollback_changes" => Some(ROLLBACK_CHANGES_TOOL_NAME),
        "run_process" => Some(RUN_PROCESS_TOOL_NAME),
        "run_validation" => Some(RUN_VALIDATION_TOOL_NAME),
        "review_changes" => Some(REVIEW_CHANGES_TOOL_NAME),
        _ => None,
    }
}

fn is_repository_activity(name: &str) -> bool {
    matches!(
        name,
        REPOSITORY_PROFILE_TOOL_NAME
            | REPOSITORY_INSTRUCTIONS_TOOL_NAME
            | FIND_FILES_TOOL_NAME
            | SEARCH_TEXT_TOOL_NAME
            | READ_FILE_TOOL_NAME
            | GIT_STATUS_TOOL_NAME
            | GIT_DIFF_TOOL_NAME
            | GIT_HISTORY_TOOL_NAME
            | REPOSITORY_MAP_TOOL_NAME
            | SEARCH_SYMBOLS_TOOL_NAME
            | FIND_REFERENCES_TOOL_NAME
            | SELECT_CONTEXT_TOOL_NAME
            | PROJECT_CAPABILITIES_TOOL_NAME
            | PREPARE_CONTEXT_TOOL_NAME
            | REVIEW_CHANGES_TOOL_NAME
            | LSP_QUERY_TOOL_NAME
    )
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
            | GIT_REVERT_OWNED_TOOL_NAME
            | GIT_CREATE_BRANCH_TOOL_NAME
            | GIT_PUSH_OWNED_TOOL_NAME
            | RUN_VALIDATION_TOOL_NAME
            | REVIEW_CHANGES_TOOL_NAME
            | LSP_QUERY_TOOL_NAME
    )
}

fn validate_process_invocation(program: &str, args: &[String]) -> Result<(), ErrorData> {
    if args.first().is_some_and(|argument| argument == program) {
        return Err(invalid_arguments(format!(
            "args must contain only arguments after program `{program}`; remove the duplicate executable from args"
        )));
    }
    Ok(())
}

pub(crate) async fn execute_async(
    config: &CodingConfig,
    state: &CodingToolState,
    tool_call: CallToolRequestParams,
    working_dir: &Path,
) -> Result<CallToolResult, ErrorData> {
    if !is_async_tool(&tool_call.name) {
        return Err(invalid_arguments(format!(
            "`{}` is not an asynchronous internal coding tool",
            tool_call.name
        )));
    }

    let workspace = CodingWorkspace::new(working_dir).map_err(invalid_workspace)?;
    let result = match tool_call.name.as_ref() {
        RUN_PROCESS_TOOL_NAME => {
            let params: RunProcessParams = parse_arguments(&tool_call)?;
            validate_process_invocation(&params.program, &params.args)?;
            let evidence_program = params.program.clone();
            let evidence_args = params.args.clone();
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
            state.record_process(workspace.root(), &evidence_program, &evidence_args, &output)?;
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
        GIT_REVERT_OWNED_TOOL_NAME => {
            let params: GitOwnedCommitParams = parse_arguments(&tool_call)?;
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
                .revert_owned_commit(&params.oid)
                .await
                .map_err(|error| invalid_arguments(error.to_string()))?;
            state.replace_commit(workspace.root(), &result.reverted_oid, &result.revert_oid);
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
        REVIEW_CHANGES_TOOL_NAME => {
            let request: GitDiffRequest = parse_arguments(&tool_call)?;
            let repository = GitRepository::open(&workspace, git_limits(config, 2_000))
                .await
                .map_err(|error| invalid_arguments(error.to_string()))?;
            let diff = repository
                .diff(request)
                .await
                .map_err(|error| invalid_arguments(error.to_string()))?;
            let review = ReviewAnalyzer::analyze(&diff);
            state.record_review(workspace.root(), &review)?;
            bounded_review_result(review, config.output_limit)
        }
        RUN_VALIDATION_TOOL_NAME => {
            let params: RunValidationParams = parse_arguments(&tool_call)?;
            if params.max_files == 0 || params.max_files > MAX_REPOSITORY_FILE_LIMIT {
                return Err(invalid_arguments(format!(
                    "max_files must be between 1 and {MAX_REPOSITORY_FILE_LIMIT}"
                )));
            }
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
            let capabilities = ProjectDiscovery::discover(&workspace, params.max_files)
                .map_err(|error| invalid_arguments(error.to_string()))?;
            let execution = ValidationService::run(
                &workspace,
                &capabilities,
                &params.command_id,
                ProcessLimits {
                    timeout,
                    output_limit: config.output_limit,
                },
            )
            .await;
            state.invalidate_intelligence(workspace.root());
            state.record_validation_execution(workspace.root(), &execution)?;
            bounded_validation_result(execution, config.output_limit)
        }
        LSP_QUERY_TOOL_NAME => {
            if !config.lsp {
                return Err(tool_unavailable(
                    "Language Server Protocol support is disabled by coding configuration",
                ));
            }
            let params: LspQueryParams = parse_arguments(&tool_call)?;
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
            let result = LanguageServerClient::new(&workspace, timeout)
                .query(params.into_query())
                .await
                .map_err(|error| invalid_arguments(error.to_string()))?;
            json_result(&result, config.output_limit)
        }
        _ => unreachable!("async coding tool name was checked"),
    };
    if result.is_ok() && is_repository_activity(&tool_call.name) {
        state.note_repository_activity(workspace.root());
    }
    result
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
    let workspace = CodingWorkspace::new(working_dir).map_err(invalid_workspace)?;
    let result = match tool_call.name.as_ref() {
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
            state.note_read_files(workspace.root(), vec![snapshot.path.clone()]);
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
            let _mutation = state.mutation_guard();
            state.authorize_mutation_locked(workspace.root())?;
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
            let change_id = applied.result.rollback_id.clone();
            state.remember(workspace.root(), applied.rollback, &applied.result.preview);
            state.record_mutation_locked(workspace.root(), change_id, &applied.result.preview)?;
            state.invalidate_intelligence(workspace.root());
            json_result(&applied.result, config.output_limit)
        }
        WRITE_FILE_TOOL_NAME => {
            let params: WriteFileParams = parse_arguments(&tool_call)?;
            let write_path = workspace
                .resolve_for_write(&params.path)
                .map_err(|error| invalid_arguments(error.to_string()))?;
            let path_exists = match write_path.symlink_metadata() {
                Ok(_) => true,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
                Err(error) => return Err(invalid_arguments(error.to_string())),
            };
            let updating_existing_file = path_exists && params.expected_digest.is_some();
            let mut change = serde_json::Map::new();
            change.insert(
                "operation".to_string(),
                Value::String(
                    if updating_existing_file {
                        "write"
                    } else {
                        "create"
                    }
                    .to_string(),
                ),
            );
            change.insert(
                "path".to_string(),
                Value::String(params.path.to_string_lossy().into_owned()),
            );
            change.insert("content".to_string(), Value::String(params.content));
            if let Some(expected_digest) = params.expected_digest.filter(|_| updating_existing_file)
            {
                change.insert(
                    "expected_digest".to_string(),
                    Value::String(expected_digest),
                );
            }
            let mut arguments = serde_json::Map::new();
            arguments.insert(
                "changes".to_string(),
                Value::Array(vec![Value::Object(change)]),
            );
            execute_with_state(
                config,
                state,
                CallToolRequestParams::new(APPLY_CHANGES_TOOL_NAME).with_arguments(arguments),
                working_dir,
            )
        }
        ROLLBACK_CHANGES_TOOL_NAME => {
            let params: RollbackChangesParams = parse_arguments(&tool_call)?;
            let _mutation = state.mutation_guard();
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
            state.record_rollback_locked(workspace.root(), &params.rollback_id)?;
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
            state.note_symbols(workspace.root(), &result.matches);
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
            let result = if config.embeddings {
                hybrid_context_candidates(index.as_ref(), &params.query, params.max_results)
                    .map_err(|error| invalid_arguments(error.to_string()))?
            } else {
                index
                    .context_candidates(&params.query, params.max_results)
                    .map_err(|error| invalid_arguments(error.to_string()))?
            };
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
        PREPARE_CONTEXT_TOOL_NAME => {
            let params: PrepareContextParams = parse_arguments(&tool_call)?;
            let requested_budget = params
                .token_budget
                .unwrap_or_else(|| config.max_context_tokens.min(config.output_limit / 8));
            if requested_budget > config.max_context_tokens {
                return Err(invalid_arguments(format!(
                    "token_budget cannot exceed the configured coding context limit of {}",
                    config.max_context_tokens
                )));
            }
            let output_safe_budget = config.output_limit / 8;
            if requested_budget > output_safe_budget {
                return Err(invalid_arguments(format!(
                    "token_budget {requested_budget} can exceed the configured serialized output \
                     limit; use at most {output_safe_budget}"
                )));
            }
            let index = state.intelligence_index(config, &workspace, params.index_limits())?;
            let planner = ContextPlanner::new(&workspace, index.as_ref());
            let limits = ContextLimits {
                token_budget: requested_budget,
                max_files: params.max_files,
                max_file_bytes: params.max_file_bytes,
                chunk_lines: params.chunk_lines,
                overlap_lines: params.overlap_lines,
            };
            let bundle = if config.embeddings {
                planner.prepare_with_local_embeddings(&params.query, limits)
            } else {
                planner.prepare(&params.query, limits)
            }
            .map_err(|error| invalid_arguments(error.to_string()))?;
            state.note_read_files(
                workspace.root(),
                bundle
                    .chunks
                    .iter()
                    .map(|chunk| chunk.path.clone())
                    .collect(),
            );
            json_result(&bundle, config.output_limit)
        }
        WORKFLOW_START_TOOL_NAME => {
            let params: WorkflowStartParams = parse_arguments(&tool_call)?;
            let status = state.start_workflow(workspace.root(), params.objective, config)?;
            json_result(&status, config.output_limit)
        }
        WORKFLOW_SET_PLAN_TOOL_NAME => {
            let mut params = if tool_call
                .arguments
                .as_ref()
                .is_some_and(|arguments| arguments.contains_key("plan"))
            {
                parse_arguments::<WorkflowSetPlanParams>(&tool_call)?
            } else {
                let compact: WorkflowCompactPlanParams = parse_arguments(&tool_call)?;
                WorkflowSetPlanParams {
                    workflow_id: compact.workflow_id.clone(),
                    plan: compact.into_plan(),
                }
            };
            if params.plan.relevant_files.is_empty() {
                return Err(invalid_arguments(
                    "`plan.relevant_files` must list workspace-relative paths affected by the \
                     plan. In an empty or greenfield project, list the intended new paths (for \
                     example `pyproject.toml`, `src/package/__init__.py`, and `tests/test_package.py`); \
                     do not send an empty array.",
                ));
            }
            normalize_plan_paths(&workspace, &mut params.plan)?;
            let status =
                state.set_workflow_plan(workspace.root(), &params.workflow_id, params.plan)?;
            json_result(&status, config.output_limit)
        }
        WORKFLOW_UPDATE_MEMORY_TOOL_NAME => {
            let params: WorkflowUpdateMemoryParams = parse_arguments(&tool_call)?;
            if params.assumptions.is_none() && params.open_points.is_none() {
                return Err(invalid_arguments(
                    "at least one of assumptions or open_points must be supplied",
                ));
            }
            let status = state.update_workflow_memory(
                workspace.root(),
                &params.workflow_id,
                params.assumptions,
                params.open_points,
            )?;
            json_result(&status, config.output_limit)
        }
        WORKFLOW_SET_REPAIR_STRATEGY_TOOL_NAME => {
            let params: WorkflowSetRepairStrategyParams = parse_arguments(&tool_call)?;
            let status = state.set_repair_strategy(
                workspace.root(),
                &params.workflow_id,
                params.approach,
                params.hypothesis,
                params.target_files,
            )?;
            json_result(&status, config.output_limit)
        }
        WORKFLOW_TRANSITION_TOOL_NAME => {
            let params: WorkflowTransitionParams = parse_arguments(&tool_call)?;
            let status = state.transition_workflow(
                workspace.root(),
                &params.workflow_id,
                params.transition,
            )?;
            json_result(&status, config.output_limit)
        }
        WORKFLOW_STATUS_TOOL_NAME => {
            let params: WorkflowStatusParams = parse_arguments(&tool_call)?;
            let status = state.workflow_status(workspace.root(), params.workflow_id.as_ref())?;
            json_result(&status, config.output_limit)
        }
        WORKFLOW_COMPLETE_TOOL_NAME => {
            let params: WorkflowCompleteParams = parse_arguments(&tool_call)?;
            let report = state.complete_workflow(
                workspace.root(),
                &params.workflow_id,
                params.summary,
                params.remaining_risks,
            )?;
            json_result(&report, config.output_limit)
        }
        RUN_PROCESS_TOOL_NAME
        | GIT_STATUS_TOOL_NAME
        | GIT_DIFF_TOOL_NAME
        | GIT_HISTORY_TOOL_NAME
        | GIT_STAGE_OWNED_TOOL_NAME
        | GIT_UNSTAGE_OWNED_TOOL_NAME
        | GIT_COMMIT_OWNED_TOOL_NAME
        | GIT_REVERT_OWNED_TOOL_NAME
        | GIT_CREATE_BRANCH_TOOL_NAME
        | GIT_PUSH_OWNED_TOOL_NAME
        | RUN_VALIDATION_TOOL_NAME
        | REVIEW_CHANGES_TOOL_NAME
        | LSP_QUERY_TOOL_NAME => Err(internal_error(
            "asynchronous coding tools require asynchronous dispatch",
        )),
        _ => Err(invalid_arguments(format!(
            "unknown internal coding tool `{}`",
            tool_call.name
        ))),
    };
    if result.is_ok() && is_repository_activity(&tool_call.name) {
        state.note_repository_activity(workspace.root());
    }
    result
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
         replace_all is explicitly true. Moves validate both source and destination and are shown \
         as paired move_from and move_to previews."
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
         bounded agent-local rollback_id. Moves are applied and rolled back as one guarded batch."
            .to_string(),
        mutation_batch_schema(),
    )
    .annotate(mutation_annotations("Apply versioned coding changes"))
}

fn write_file_tool() -> Tool {
    Tool::new(
        WRITE_FILE_TOOL_NAME.to_string(),
        "Create one new workspace file or update one versioned file. Existing files require the \
         complete BLAKE3 digest returned by coding__read_file. This applies the same guarded \
         mutation contract as coding__apply_changes with a smaller single-file input."
            .to_string(),
        object!({
            "type": "object",
            "required": ["path", "content"],
            "properties": {
                "path": {"type": "string"},
                "content": {"type": "string"},
                "expected_digest": {"type": "string"}
            },
            "additionalProperties": false
        }),
    )
    .annotate(mutation_annotations("Create or update one coding file"))
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
         recursive deletion, privilege escalation, network clients, and host administration \
         commands are blocked in favor of dedicated safer workflows. Docker commands remain \
         available as mutation actions and follow the active session confirmation policy. Captures \
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

fn git_revert_owned_tool() -> Tool {
    Tool::new(
        GIT_REVERT_OWNED_TOOL_NAME.to_string(),
        "Create one inverse commit for a commit retained as created by this agent. Requires that \
         the owned commit is still the exact current HEAD and that the worktree and index are \
         completely clean. Hooks, signing, executable filters, history rewriting, reset, and \
         force operations are disabled. The returned revert commit remains agent-owned so it can \
         be pushed explicitly."
            .to_string(),
        object!({
            "type": "object",
            "required": ["oid"],
            "properties": {
                "oid": {
                    "type": "string",
                    "minLength": 40,
                    "maxLength": 64,
                    "description": "Current commit object id returned by coding__git_commit_owned."
                }
            },
            "additionalProperties": false
        }),
    )
    .annotate(mutation_annotations("Revert agent-owned Git commit"))
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
         the internal Tree-sitter index. When explicitly configured, a deterministic local \
         feature embedding adds bounded hybrid relevance without a model, provider, network, or \
         source reread. This selects files but does not read their contents."
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

fn prepare_context_tool() -> Tool {
    Tool::new(
        PREPARE_CONTEXT_TOOL_NAME.to_string(),
        "Prepare exact-token-budgeted, versioned source chunks for a coding query using the \
         internal Tree-sitter index and the optional local hybrid feature embedding. Relevant \
         symbol and call-site windows are ranked; sensitive, generated, binary, oversized, and \
         excess files are excluded with explicit evidence. The result states which retrieval \
         strategy actually ran."
            .to_string(),
        object!({
            "type": "object",
            "required": ["query"],
            "properties": {
                "query": {"type": "string", "minLength": 1},
                "token_budget": {
                    "type": ["integer", "null"],
                    "minimum": 128,
                    "maximum": 1000000,
                    "default": null,
                    "description": "Defaults to the lower configured context or serialized-output-safe limit."
                },
                "max_files": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 100,
                    "default": 20
                },
                "max_file_bytes": {
                    "type": "integer",
                    "minimum": MIN_READ_LIMIT,
                    "maximum": MAX_READ_LIMIT,
                    "default": DEFAULT_READ_LIMIT
                },
                "chunk_lines": {
                    "type": "integer",
                    "minimum": 10,
                    "maximum": 400,
                    "default": 120
                },
                "overlap_lines": {
                    "type": "integer",
                    "minimum": 0,
                    "maximum": 399,
                    "default": 20
                },
                "index_max_files": intelligence_max_files_schema(),
                "index_max_file_bytes": intelligence_max_file_bytes_schema(),
                "index_max_symbols": intelligence_max_symbols_schema()
            },
            "additionalProperties": false
        }),
    )
    .annotate(read_only_annotations("Prepare bounded coding context"))
}

fn workflow_start_tool() -> Tool {
    Tool::new(
        WORKFLOW_START_TOOL_NAME.to_string(),
        "Start one bounded, agent-local coding workflow for this workspace. Returns the workflow \
         id and actual analyzing status. A second unfinished workflow cannot replace it."
            .to_string(),
        object!({
            "type": "object",
            "required": ["objective"],
            "properties": {
                "objective": {
                    "type": "string",
                    "minLength": 1,
                    "maxLength": 16384
                }
            },
            "additionalProperties": false
        }),
    )
    .annotate(stateful_annotations("Start coding workflow"))
}

fn workflow_set_plan_tool() -> Tool {
    Tool::new(
        WORKFLOW_SET_PLAN_TOOL_NAME.to_string(),
        "Attach a complete, bounded plan to the current workflow after repository analysis. \
         relevant_files includes both existing paths and workspace-relative paths that the task \
         will create; it must not be empty even for a greenfield project. Relevant paths are \
         revalidated against the workspace. Absolute paths are accepted only when they resolve \
         inside the workspace and are normalized to workspace-relative paths before storage. The \
         plan must identify components, files, intended changes, risks, tests, validation, and \
         rollback."
            .to_string(),
        object!({
            "type": "object",
            "required": ["workflow_id", "plan"],
            "properties": {
                "workflow_id": {"type": "string", "pattern": "^workflow_[0-9a-fA-F-]{36}$"},
                "plan": {
                    "type": "object",
                    "required": [
                        "affected_components",
                        "relevant_files",
                        "risks",
                        "intended_changes",
                        "requirements",
                        "tests",
                        "validation",
                        "rollback_strategy"
                    ],
                    "properties": {
                        "affected_components": bounded_string_array_schema(1),
                        "relevant_files": {
                            "type": "array",
                            "minItems": 1,
                            "maxItems": 200,
                            "description": "Existing workspace-relative paths and intended new \
                                workspace-relative paths affected by the plan. For an empty project, \
                                list every planned new file; never use an empty array.",
                            "items": {"type": "string", "minLength": 1}
                        },
                        "risks": bounded_string_array_schema(0),
                        "intended_changes": bounded_string_array_schema(1),
                        "requirements": workflow_requirement_array_schema(),
                        "tests": workflow_check_array_schema(),
                        "validation": workflow_check_array_schema(),
                        "rollback_strategy": {
                            "type": "string",
                            "minLength": 1,
                            "maxLength": 16384
                        }
                    },
                    "additionalProperties": false
                }
            },
            "additionalProperties": false
        }),
    )
    .annotate(stateful_annotations("Set coding workflow plan"))
}

fn workflow_update_memory_tool() -> Tool {
    Tool::new(
        WORKFLOW_UPDATE_MEMORY_TOOL_NAME.to_string(),
        "Replace the bounded assumptions or open points in the active agent-local workflow \
         memory. Read files, relevant symbols, executed commands, and known validation errors are \
         captured automatically without source text, command arguments, or diagnostic text. The \
         complete memory is ephemeral and never persisted."
            .to_string(),
        object!({
            "type": "object",
            "required": ["workflow_id"],
            "properties": {
                "workflow_id": {"type": "string", "pattern": "^workflow_[0-9a-fA-F-]{36}$"},
                "assumptions": {
                    "type": ["array", "null"],
                    "maxItems": 200,
                    "items": {
                        "type": "string",
                        "minLength": 1,
                        "maxLength": 16384
                    },
                    "default": null
                },
                "open_points": {
                    "type": ["array", "null"],
                    "maxItems": 200,
                    "items": {
                        "type": "string",
                        "minLength": 1,
                        "maxLength": 16384
                    },
                    "default": null
                }
            },
            "additionalProperties": false
        }),
    )
    .annotate(stateful_annotations("Update coding workflow memory"))
}

fn workflow_set_repair_strategy_tool() -> Tool {
    Tool::new(
        WORKFLOW_SET_REPAIR_STRATEGY_TOOL_NAME.to_string(),
        "Record a distinct, bounded repair hypothesis for the latest failed validation. The raw \
         hypothesis is never retained in workflow memory; only its fingerprint, chosen approach, \
         and workspace-relative target files are recorded. A repeated diagnostic requires this \
         before repair can begin."
            .to_string(),
        object!({
            "type": "object",
            "required": ["workflow_id", "approach", "hypothesis", "target_files"],
            "properties": {
                "workflow_id": {"type": "string", "pattern": "^workflow_[0-9a-fA-F-]{36}$"},
                "approach": {
                    "type": "string",
                    "enum": ["local_logic", "dependency_boundary", "configuration", "test_fixture"]
                },
                "hypothesis": {"type": "string", "minLength": 1, "maxLength": 16384},
                "target_files": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": 200,
                    "items": {"type": "string", "minLength": 1}
                }
            },
            "additionalProperties": false
        }),
    )
    .annotate(stateful_annotations("Record coding repair strategy"))
}

fn workflow_transition_tool() -> Tool {
    workflow_transition_tool_with_values(
        &[
            "begin_editing",
            "begin_validation",
            "begin_repair",
            "begin_review",
        ],
        "Move the current coding workflow through a validated transition. Editing requires a \
         complete plan; validation requires a retained change; repair and review require actual \
         prior process evidence and obey configured attempt limits.",
    )
}

fn workflow_transition_tool_for(transition: &str, description: &str) -> Tool {
    workflow_transition_tool_with_values(&[transition], description)
}

fn workflow_transition_tool_with_values(transitions: &[&str], description: &str) -> Tool {
    Tool::new(
        WORKFLOW_TRANSITION_TOOL_NAME.to_string(),
        description.to_string(),
        object!({
            "type": "object",
            "required": ["workflow_id", "transition"],
            "properties": {
                "workflow_id": {"type": "string", "pattern": "^workflow_[0-9a-fA-F-]{36}$"},
                "transition": {
                    "type": "string",
                    "enum": transitions
                }
            },
            "additionalProperties": false
        }),
    )
    .annotate(stateful_annotations("Transition coding workflow"))
}

fn workflow_status_tool() -> Tool {
    Tool::new(
        WORKFLOW_STATUS_TOOL_NAME.to_string(),
        "Read the actual current workflow phase, plan, iteration and repair counters, changed \
         files, validation count, and any machine-detected stop reason."
            .to_string(),
        object!({
            "type": "object",
            "properties": {
                "workflow_id": {
                    "type": ["string", "null"],
                    "pattern": "^workflow_[0-9a-fA-F-]{36}$",
                    "default": null
                }
            },
            "additionalProperties": false
        }),
    )
    .annotate(read_only_annotations("Read coding workflow status"))
}

fn workflow_complete_tool() -> Tool {
    Tool::new(
        WORKFLOW_COMPLETE_TOOL_NAME.to_string(),
        "Complete a reviewed coding workflow and return an evidence-backed report. Verification \
         status and process outcomes are derived from captured execution results and cannot be \
         supplied by the model."
            .to_string(),
        object!({
            "type": "object",
            "required": ["workflow_id", "summary", "remaining_risks"],
            "properties": {
                "workflow_id": {"type": "string", "pattern": "^workflow_[0-9a-fA-F-]{36}$"},
                "summary": {
                    "type": "string",
                    "minLength": 1,
                    "maxLength": 16384
                },
                "remaining_risks": bounded_string_array_schema(0)
            },
            "additionalProperties": false
        }),
    )
    .annotate(stateful_annotations("Complete coding workflow"))
}

fn run_validation_tool() -> Tool {
    Tool::new(
        RUN_VALIDATION_TOOL_NAME.to_string(),
        "Run exactly one validation command id returned by coding__project_capabilities. Never \
         pass a file path, executable name, or command text as command_id; use \
         coding__run_process when no exact discovered id exists. The command is rediscovered \
         before execution and runs through the bounded process policy. \
         Results distinguish passed, failed, missing, unavailable, blocked, timed out, and \
         incomplete checks and are automatically attached to an active testing workflow."
            .to_string(),
        object!({
            "type": "object",
            "required": ["command_id"],
            "properties": {
                "command_id": {
                    "type": "string",
                    "minLength": 1,
                    "description": "Stable id from a current coding__project_capabilities result."
                },
                "max_files": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": MAX_REPOSITORY_FILE_LIMIT,
                    "default": DEFAULT_REPOSITORY_FILE_LIMIT
                },
                "timeout_seconds": {
                    "type": ["integer", "null"],
                    "minimum": 1,
                    "default": null
                }
            },
            "additionalProperties": false
        }),
    )
    .annotate(mutation_annotations("Run discovered project validation"))
}

fn review_changes_tool() -> Tool {
    Tool::new(
        REVIEW_CHANGES_TOOL_NAME.to_string(),
        "Review bounded staged or unstaged local Git changes without executing repository code. \
         Reports conservative security, reliability, error-handling, maintainability, and debug \
         findings with exact paths and added-line numbers. Sensitive files and secret values are \
         omitted."
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
                    "maxItems": 100,
                    "items": {
                        "type": "string",
                        "minLength": 1
                    }
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
    .annotate(read_only_annotations("Review local coding changes"))
}

fn lsp_query_tool() -> Tool {
    Tool::new(
        LSP_QUERY_TOOL_NAME.to_string(),
        "Query a locally installed language server for document symbols, definitions, or \
         references. This optional feature is disabled by default and never replaces the \
         built-in Tree-sitter fallback. The source path is workspace-confined, sensitive files \
         and external result locations are excluded, protocol traffic is bounded, the server \
         receives a restricted environment, and the complete process group is stopped at the \
         configured timeout. Enabling LSP explicitly trusts the selected local executable to \
         inspect the repository."
            .to_string(),
        object!({
            "type": "object",
            "required": ["path", "operation"],
            "properties": {
                "path": {
                    "type": "string",
                    "minLength": 1,
                    "description": "Workspace-relative UTF-8 source file."
                },
                "operation": {
                    "type": "string",
                    "enum": ["document_symbols", "definition", "references"]
                },
                "position": {
                    "type": ["object", "null"],
                    "default": null,
                    "description": "Required for definition and references; values are one-based.",
                    "required": ["line", "column"],
                    "properties": {
                        "line": {"type": "integer", "minimum": 1},
                        "column": {"type": "integer", "minimum": 1}
                    },
                    "additionalProperties": false
                },
                "include_declaration": {
                    "type": "boolean",
                    "default": true
                },
                "max_results": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 1000,
                    "default": 100
                },
                "timeout_seconds": {
                    "type": ["integer", "null"],
                    "minimum": 1,
                    "default": null
                }
            },
            "additionalProperties": false
        }),
    )
    .annotate(read_only_annotations("Query local language server"))
}

fn workflow_requirement_array_schema() -> Value {
    object!({
        "type": "array",
        "minItems": 1,
        "maxItems": 200,
        "items": {
            "type": "object",
            "required": ["id", "description", "source", "priority", "mandatory", "verification"],
            "properties": {
                "id": {"type": "string", "minLength": 1, "maxLength": 16384},
                "description": {"type": "string", "minLength": 1, "maxLength": 16384},
                "source": {"type": "string", "enum": ["user", "inferred"]},
                "priority": {"type": "string", "enum": ["critical", "high", "normal", "low"]},
                "mandatory": {"type": "boolean"},
                "verification": {
                    "type": "object",
                    "required": ["expected_files", "check_ids"],
                    "properties": {
                        "expected_files": {"type": "array", "maxItems": 200, "items": {"type": "string", "minLength": 1}},
                        "check_ids": bounded_string_array_schema(0)
                    },
                    "additionalProperties": false
                }
            },
            "additionalProperties": false
        }
    })
    .into()
}

fn workflow_check_array_schema() -> Value {
    object!({
        "type": "array",
        "maxItems": 200,
        "items": {
            "type": "object",
            "required": ["id", "description", "command", "required"],
            "properties": {
                "id": {"type": "string", "minLength": 1, "maxLength": 16384},
                "description": {"type": "string", "minLength": 1, "maxLength": 16384},
                "command": {
                    "type": "object",
                    "required": ["program", "args", "cwd"],
                    "properties": {
                        "program": {"type": "string", "minLength": 1, "maxLength": 16384},
                        "args": bounded_string_array_schema(0),
                        "cwd": {"type": "string", "minLength": 1}
                    },
                    "additionalProperties": false
                },
                "required": {"type": "boolean"}
            },
            "additionalProperties": false
        }
    })
    .into()
}

fn bounded_string_array_schema(min_items: usize) -> Value {
    serde_json::json!({
        "type": "array",
        "minItems": min_items,
        "maxItems": 200,
        "items": {
            "type": "string",
            "minLength": 1,
            "maxLength": 16384
        }
    })
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
                    "type": "object",
                    "required": ["operation", "path"],
                    "properties": {
                        "operation": {
                            "type": "string",
                            "enum": ["create", "write", "replace", "delete", "move"],
                            "description": "create requires content; write requires expected_digest and content; replace requires expected_digest and replacements; delete requires expected_digest; move requires expected_digest and destination."
                        },
                        "path": {
                            "type": "string",
                            "minLength": 1,
                            "description": "Workspace-relative path, or an absolute path resolving inside the coding workspace. For create, missing parent directories are created safely."
                        },
                        "content": {
                            "type": "string",
                            "description": "Complete new file content for create or write."
                        },
                        "expected_digest": {
                            "type": "string",
                            "description": "Complete BLAKE3 digest returned by coding__read_file; required for every operation on an existing file."
                        },
                        "destination": {
                            "type": "string",
                            "minLength": 1,
                            "description": "Unoccupied workspace-relative destination for move."
                        },
                        "replacements": {
                            "type": "array",
                            "minItems": 1,
                            "description": "Exact conflict-safe replacements for replace.",
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
                }
            }
        },
        "additionalProperties": false
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

fn stateful_annotations(title: &str) -> ToolAnnotations {
    ToolAnnotations::with_title(title.to_string())
        .read_only(false)
        .destructive(false)
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

fn normalize_plan_paths(
    workspace: &CodingWorkspace,
    plan: &mut WorkflowPlan,
) -> Result<(), ErrorData> {
    for path in plan.relevant_files.iter_mut().chain(
        plan.requirements
            .iter_mut()
            .flat_map(|requirement| requirement.verification.expected_files.iter_mut()),
    ) {
        let resolved = workspace
            .resolve_for_write(&*path)
            .map_err(|error| invalid_arguments(error.to_string()))?;
        let relative = resolved
            .strip_prefix(workspace.root())
            .map(Path::to_path_buf)
            .map_err(|_| {
                invalid_arguments(format!(
                    "path is outside the coding workspace: {}",
                    resolved.display()
                ))
            })?;
        *path = relative;
    }
    Ok(())
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
        } else if !output.diagnostics.diagnostics.is_empty() {
            output.diagnostics.truncated = true;
            let retained = output.diagnostics.diagnostics.len() / 2;
            output.diagnostics.diagnostics.truncate(retained);
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

fn bounded_validation_result(
    mut execution: ValidationExecution,
    output_limit: usize,
) -> Result<CallToolResult, ErrorData> {
    for _ in 0..40 {
        let json = serialize_json(&execution)?;
        if json.len() <= output_limit {
            return Ok(CallToolResult::success(vec![ContentBlock::text(json)]));
        }
        if let Some(output) = execution.output.as_mut() {
            output.output_truncated = true;
            if output.stdout.len() >= output.stderr.len() && !output.stdout.is_empty() {
                let requested_len = output.stdout.len() / 2;
                truncate_utf8(&mut output.stdout, requested_len);
                continue;
            }
            if !output.stderr.is_empty() {
                let requested_len = output.stderr.len() / 2;
                truncate_utf8(&mut output.stderr, requested_len);
                continue;
            }
            if !output.diagnostics.diagnostics.is_empty() {
                output.diagnostics.truncated = true;
                let retained = output.diagnostics.diagnostics.len() / 2;
                output.diagnostics.diagnostics.truncate(retained);
                continue;
            }
        }
        if let Some(command) = execution.command.as_mut() {
            if !command.args.is_empty() {
                command.args.clear();
                continue;
            }
            if command.evidence.len() > 64 {
                let requested_len = command.evidence.len() / 2;
                truncate_utf8(&mut command.evidence, requested_len);
                continue;
            }
        }
        if let Some(reason) = execution.reason.as_mut() {
            if reason.len() > 64 {
                let requested_len = reason.len() / 2;
                truncate_utf8(reason, requested_len);
                continue;
            }
            execution.reason = None;
            continue;
        }
        return Err(output_too_large(json.len(), output_limit));
    }
    Err(internal_error(
        "failed to fit validation result within the configured output limit",
    ))
}

fn bounded_review_result(
    mut report: ReviewReport,
    output_limit: usize,
) -> Result<CallToolResult, ErrorData> {
    for _ in 0..64 {
        let json = serialize_json(&report)?;
        if json.len() <= output_limit {
            return Ok(CallToolResult::success(vec![ContentBlock::text(json)]));
        }
        report.truncated = true;
        if !report.findings.is_empty() {
            let retained = report.findings.len() / 2;
            report.findings.truncate(retained);
        } else if !report.files.is_empty() {
            let retained = report.files.len() / 2;
            report.files.truncate(retained);
        } else if !report.skipped_sensitive.is_empty() {
            let retained = report.skipped_sensitive.len() / 2;
            report.skipped_sensitive.truncate(retained);
        } else {
            return Err(output_too_large(json.len(), output_limit));
        }
    }
    Err(internal_error(
        "failed to fit review result within the configured output limit",
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
    #[serde(alias = "relative_path")]
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

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PrepareContextParams {
    query: String,
    #[serde(default)]
    token_budget: Option<usize>,
    #[serde(default = "default_context_results")]
    max_files: usize,
    #[serde(default = "default_read_limit")]
    max_file_bytes: usize,
    #[serde(default = "default_context_chunk_lines")]
    chunk_lines: usize,
    #[serde(default = "default_context_overlap_lines")]
    overlap_lines: usize,
    #[serde(default = "default_intelligence_max_files")]
    index_max_files: usize,
    #[serde(default = "default_intelligence_max_file_bytes")]
    index_max_file_bytes: usize,
    #[serde(default = "default_intelligence_max_symbols")]
    index_max_symbols: usize,
}

impl PrepareContextParams {
    fn index_limits(&self) -> IntelligenceLimits {
        intelligence_limits(
            self.index_max_files,
            self.index_max_file_bytes,
            self.index_max_symbols,
        )
    }
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
struct WriteFileParams {
    path: PathBuf,
    content: String,
    #[serde(default)]
    expected_digest: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RollbackChangesParams {
    rollback_id: String,
}

#[derive(Debug, Clone, Deserialize)]
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
#[serde(deny_unknown_fields)]
struct WorkflowStartParams {
    objective: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkflowSetPlanParams {
    #[serde(alias = "flow_id")]
    workflow_id: WorkflowId,
    plan: WorkflowPlan,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkflowCompactPlanParams {
    #[serde(alias = "flow_id")]
    workflow_id: WorkflowId,
    relevant_files: Vec<PathBuf>,
    intended_change: String,
    #[serde(default, alias = "steps", alias = "planned_steps")]
    plan_steps: Vec<String>,
    validation_program: String,
    #[serde(default, alias = "validation_args")]
    args: Vec<String>,
}

impl WorkflowCompactPlanParams {
    fn into_plan(self) -> WorkflowPlan {
        let mut command_parts = self.validation_program.split_whitespace();
        let program = command_parts.next().unwrap_or_default().to_string();
        let mut args = command_parts
            .map(|part| part.to_string())
            .collect::<Vec<_>>();
        if args.is_empty() || !self.args.starts_with(&args) {
            args.extend(self.args);
        }
        let validation_id = "required-validation".to_string();
        let expected_files = self.relevant_files.clone();
        let intended_changes = if self.plan_steps.is_empty() {
            vec![self.intended_change]
        } else {
            self.plan_steps
        };
        WorkflowPlan {
            affected_components: self
                .relevant_files
                .iter()
                .map(|path| path.display().to_string())
                .collect(),
            relevant_files: self.relevant_files,
            risks: vec![
                "The declared change must remain limited to the planned files and pass validation."
                    .to_string(),
            ],
            intended_changes: intended_changes.clone(),
            requirements: intended_changes
                .into_iter()
                .enumerate()
                .map(|(index, description)| WorkflowRequirement {
                    id: format!("planned-change-{}", index + 1),
                    description,
                    source: RequirementSource::User,
                    priority: RequirementPriority::Critical,
                    mandatory: true,
                    verification: RequirementVerification {
                        expected_files: expected_files.clone(),
                        check_ids: vec![validation_id.clone()],
                    },
            })
            .collect(),
            tests: Vec::new(),
            validation: vec![WorkflowCheck {
                id: validation_id,
                description: "Run the declared validation command.".to_string(),
                command: WorkflowCommand {
                    program,
                    args,
                    cwd: PathBuf::from("."),
                },
                required: true,
            }],
            rollback_strategy:
                "Roll back the change batch using the returned rollback identifier if validation fails."
                    .to_string(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkflowUpdateMemoryParams {
    workflow_id: WorkflowId,
    #[serde(default)]
    assumptions: Option<Vec<String>>,
    #[serde(default)]
    open_points: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkflowSetRepairStrategyParams {
    workflow_id: WorkflowId,
    approach: RepairApproach,
    hypothesis: String,
    target_files: Vec<PathBuf>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
enum WorkflowTransition {
    #[serde(rename = "begin_editing")]
    Editing,
    #[serde(rename = "begin_validation")]
    Validation,
    #[serde(rename = "begin_repair")]
    Repair,
    #[serde(rename = "begin_review")]
    Review,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkflowTransitionParams {
    workflow_id: WorkflowId,
    transition: WorkflowTransition,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct WorkflowStatusParams {
    workflow_id: Option<WorkflowId>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkflowCompleteParams {
    workflow_id: WorkflowId,
    summary: String,
    remaining_risks: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RunValidationParams {
    command_id: String,
    #[serde(default = "default_repository_file_limit")]
    max_files: usize,
    #[serde(default)]
    timeout_seconds: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LspQueryParams {
    path: PathBuf,
    operation: LanguageServerOperation,
    #[serde(default)]
    position: Option<LanguageServerPosition>,
    #[serde(default = "default_include_declaration")]
    include_declaration: bool,
    #[serde(default = "default_symbol_results")]
    max_results: usize,
    #[serde(default)]
    timeout_seconds: Option<u64>,
}

impl LspQueryParams {
    fn into_query(self) -> LanguageServerQuery {
        LanguageServerQuery {
            path: self.path,
            operation: self.operation,
            position: self.position,
            include_declaration: self.include_declaration,
            max_results: self.max_results,
        }
    }
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
struct GitOwnedCommitParams {
    oid: String,
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

fn default_repository_file_limit() -> usize {
    DEFAULT_REPOSITORY_FILE_LIMIT
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

fn default_include_declaration() -> bool {
    true
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

fn default_context_chunk_lines() -> usize {
    ContextLimits::default().chunk_lines
}

fn default_context_overlap_lines() -> usize {
    ContextLimits::default().overlap_lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;

    fn enabled_config() -> CodingConfig {
        CodingConfig::default()
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

    fn exposed_tool_names(state: &CodingToolState, root: &Path) -> BTreeSet<String> {
        let root = root.canonicalize().expect("canonical test workspace");
        state
            .definitions_for_workspace(&root)
            .into_iter()
            .map(|tool| tool.name.into_owned())
            .collect()
    }

    fn exposed_transition_values(state: &CodingToolState, root: &Path) -> Vec<String> {
        let root = root.canonicalize().expect("canonical test workspace");
        let tool = state
            .definitions_for_workspace(&root)
            .into_iter()
            .find(|tool| tool.name == WORKFLOW_TRANSITION_TOOL_NAME)
            .expect("expected a phase-specific workflow transition tool");
        Value::Object((*tool.input_schema).clone())["properties"]["transition"]["enum"]
            .as_array()
            .expect("transition enum")
            .iter()
            .map(|value| value.as_str().expect("string transition").to_string())
            .collect()
    }

    fn transition(
        config: &CodingConfig,
        state: &CodingToolState,
        root: &Path,
        workflow_id: &str,
        transition: &str,
    ) {
        execute_with_state(
            config,
            state,
            CallToolRequestParams::new(WORKFLOW_TRANSITION_TOOL_NAME).with_arguments(object!({
                "workflow_id": workflow_id,
                "transition": transition
            })),
            root,
        )
        .unwrap();
    }

    fn begin_editing_workflow(
        config: &CodingConfig,
        state: &CodingToolState,
        root: &Path,
        relevant_files: &[&str],
    ) -> String {
        let relevant_files = relevant_files
            .iter()
            .map(|path| (*path).to_string())
            .collect::<Vec<_>>();
        let requirements = relevant_files
            .iter()
            .enumerate()
            .map(|(index, path)| {
                serde_json::json!({
                    "id": format!("file-{index}"),
                    "description": format!("update {path}"),
                    "source": "user",
                    "priority": "high",
                    "mandatory": true,
                    "verification": {"expected_files": [path], "check_ids": []}
                })
            })
            .collect::<Vec<_>>();
        let started = execute_with_state(
            config,
            state,
            CallToolRequestParams::new(WORKFLOW_START_TOOL_NAME).with_arguments(
                serde_json::json!({"objective": "test workflow mutation"})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
            root,
        )
        .unwrap();
        let workflow_id = serde_json::from_str::<Value>(&result_text(started)).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        execute_with_state(
            config,
            state,
            CallToolRequestParams::new(WORKFLOW_SET_PLAN_TOOL_NAME).with_arguments(
                serde_json::json!({
                    "workflow_id": workflow_id,
                    "plan": {
                        "affected_components": ["test fixture"],
                        "relevant_files": relevant_files,
                        "risks": [],
                        "intended_changes": ["exercise a workspace mutation"],
                        "requirements": requirements,
                        "tests": [],
                        "validation": [],
                        "rollback_strategy": "roll back the test mutation"
                    }
                })
                .as_object()
                .unwrap()
                .clone(),
            ),
            root,
        )
        .unwrap();
        transition(config, state, root, &workflow_id, "begin_editing");
        workflow_id
    }

    #[test]
    fn rejects_a_process_argument_that_repeats_the_program() {
        let args = vec![
            "python3".to_string(),
            "-m".to_string(),
            "unittest".to_string(),
        ];

        assert!(validate_process_invocation("python3", &args).is_err());
        assert!(validate_process_invocation("python3", &args[1..]).is_ok());
    }

    #[test]
    fn repair_guidance_prioritizes_the_diagnostic_file_and_per_file_digests() {
        let guidance = repair_pending_guidance();

        assert!(guidance.contains("diagnostic path is the primary repair target"));
        assert!(guidance.contains("undefined symbol"));
        assert!(guidance.contains("file's own expected_digest"));
    }

    #[tokio::test]
    async fn records_a_redacted_repair_strategy_after_validation_failure() {
        let temp_dir = tempfile::tempdir().unwrap();
        let config = enabled_config();
        let state = CodingToolState::default();
        let workflow_id = begin_editing_workflow(&config, &state, temp_dir.path(), &["fixture.rs"]);
        execute_with_state(
            &config,
            &state,
            CallToolRequestParams::new(APPLY_CHANGES_TOOL_NAME).with_arguments(object!({
                "changes": [{"operation": "create", "path": "fixture.rs", "content": "fn main() {}\n"}]
            })),
            temp_dir.path(),
        )
        .unwrap();
        transition(
            &config,
            &state,
            temp_dir.path(),
            &workflow_id,
            "begin_validation",
        );
        execute_async(
            &config,
            &state,
            CallToolRequestParams::new(RUN_PROCESS_TOOL_NAME).with_arguments(object!({
                "program": "rustc",
                "args": ["--invalid-ponduin-flag"],
                "timeout_seconds": 5
            })),
            temp_dir.path(),
        )
        .await
        .unwrap();

        assert!(exposed_tool_names(&state, temp_dir.path())
            .contains(WORKFLOW_SET_REPAIR_STRATEGY_TOOL_NAME));
        let status = execute_with_state(
            &config,
            &state,
            CallToolRequestParams::new(WORKFLOW_SET_REPAIR_STRATEGY_TOOL_NAME).with_arguments(
                object!({
                    "workflow_id": workflow_id,
                    "approach": "local_logic",
                    "hypothesis": "the parser rejects the unsupported invocation",
                    "target_files": ["fixture.rs"]
                }),
            ),
            temp_dir.path(),
        )
        .unwrap();
        let status = result_text(status);

        assert!(status.contains("repair_strategies"));
        assert!(!status.contains("the parser rejects the unsupported invocation"));
    }

    #[test]
    fn definitions_distinguish_read_only_and_mutating_tools() {
        let tools = definitions();
        assert_eq!(tools.len(), 35);
        assert!(tools
            .iter()
            .all(|tool| is_reserved_name(&tool.name) && tool.annotations.is_some()));
        for tool in &tools {
            let annotations = tool.annotations.as_ref().unwrap();
            let destructive = matches!(
                tool.name.as_ref(),
                APPLY_CHANGES_TOOL_NAME
                    | WRITE_FILE_TOOL_NAME
                    | ROLLBACK_CHANGES_TOOL_NAME
                    | RUN_PROCESS_TOOL_NAME
                    | GIT_STAGE_OWNED_TOOL_NAME
                    | GIT_UNSTAGE_OWNED_TOOL_NAME
                    | GIT_COMMIT_OWNED_TOOL_NAME
                    | GIT_REVERT_OWNED_TOOL_NAME
                    | GIT_CREATE_BRANCH_TOOL_NAME
                    | GIT_PUSH_OWNED_TOOL_NAME
                    | RUN_VALIDATION_TOOL_NAME
            );
            let stateful = matches!(
                tool.name.as_ref(),
                WORKFLOW_START_TOOL_NAME
                    | WORKFLOW_SET_PLAN_TOOL_NAME
                    | WORKFLOW_UPDATE_MEMORY_TOOL_NAME
                    | WORKFLOW_SET_REPAIR_STRATEGY_TOOL_NAME
                    | WORKFLOW_TRANSITION_TOOL_NAME
                    | WORKFLOW_COMPLETE_TOOL_NAME
            );
            assert_eq!(annotations.read_only_hint, Some(!destructive && !stateful));
            assert_eq!(annotations.destructive_hint, Some(destructive));
            assert_eq!(annotations.open_world_hint, Some(false));
        }
    }

    #[test]
    fn write_file_uses_the_guarded_single_file_mutation_path() {
        let temp_dir = tempfile::tempdir().unwrap();
        let config = enabled_config();
        let state = CodingToolState::default();
        begin_editing_workflow(&config, &state, temp_dir.path(), &["index.html"]);

        execute_with_state(
            &config,
            &state,
            CallToolRequestParams::new(WRITE_FILE_TOOL_NAME).with_arguments(object!({
                "path": "index.html",
                "content": "<h1>America</h1>\n"
            })),
            temp_dir.path(),
        )
        .unwrap();

        assert_eq!(
            fs::read_to_string(temp_dir.path().join("index.html")).unwrap(),
            "<h1>America</h1>\n"
        );
        assert_eq!(
            state
                .workflow_status(CodingWorkspace::new(temp_dir.path()).unwrap().root(), None)
                .unwrap()
                .changed_files,
            vec![PathBuf::from("index.html")]
        );
    }

    #[test]
    fn write_file_creates_a_new_file_when_a_model_supplies_an_unneeded_digest() {
        let temp_dir = tempfile::tempdir().unwrap();
        let config = enabled_config();
        let state = CodingToolState::default();
        begin_editing_workflow(&config, &state, temp_dir.path(), &["index.html"]);

        execute_with_state(
            &config,
            &state,
            CallToolRequestParams::new(WRITE_FILE_TOOL_NAME).with_arguments(object!({
                "path": "index.html",
                "content": "<h1>America</h1>\n",
                "expected_digest": "sha256:not-a-real-blake3-digest"
            })),
            temp_dir.path(),
        )
        .unwrap();

        assert_eq!(
            fs::read_to_string(temp_dir.path().join("index.html")).unwrap(),
            "<h1>America</h1>\n"
        );
    }

    #[test]
    fn empty_greenfield_plan_explains_how_to_name_intended_files() {
        let temp_dir = tempfile::tempdir().unwrap();
        let config = enabled_config();
        let state = CodingToolState::default();
        let started = execute_with_state(
            &config,
            &state,
            CallToolRequestParams::new(WORKFLOW_START_TOOL_NAME).with_arguments(object!({
                "objective": "create a new package"
            })),
            temp_dir.path(),
        )
        .unwrap();
        let started: Value = serde_json::from_str(&result_text(started)).unwrap();
        let error = execute_with_state(
            &config,
            &state,
            CallToolRequestParams::new(WORKFLOW_SET_PLAN_TOOL_NAME).with_arguments(object!({
                "flow_id": started["id"].as_str().unwrap(),
                "plan": {
                    "affected_components": ["package"],
                    "relevant_files": [],
                    "risks": [],
                    "intended_changes": ["create package"],
                    "requirements": [],
                    "tests": [],
                    "validation": [],
                    "rollback_strategy": "roll back the change batch"
                }
            })),
            temp_dir.path(),
        )
        .unwrap_err();

        assert!(error.message.contains("greenfield project"));
        assert!(error.message.contains("intended new paths"));
        assert!(error.message.contains("pyproject.toml"));
    }

    #[test]
    fn workflow_start_uses_host_task_context_and_rejects_rollback_ids() {
        let temp_dir = tempfile::tempdir().unwrap();
        let config = enabled_config();
        let state = CodingToolState::default();
        let workspace = CodingWorkspace::new(temp_dir.path()).unwrap();
        state
            .register_task_context(
                workspace.root(),
                "Erstelle ein neues Webprojekt mit HTML, CSS und JavaScript.".to_string(),
                crate::coding::TaskInteractionMode::Autonomous,
            )
            .unwrap();

        let started = execute_with_state(
            &config,
            &state,
            CallToolRequestParams::new(WORKFLOW_START_TOOL_NAME).with_arguments(object!({
                "objective": "create the requested web project"
            })),
            temp_dir.path(),
        )
        .unwrap();
        let started: Value = serde_json::from_str(&result_text(started)).unwrap();

        assert_eq!(
            started["task"]["original_user_request"],
            "Erstelle ein neues Webprojekt mit HTML, CSS und JavaScript."
        );
        assert_eq!(started["task"]["intent"], "create");
        assert!(started["id"]
            .as_str()
            .is_some_and(|id| id.starts_with("workflow_")));

        let error = execute_with_state(
            &config,
            &state,
            CallToolRequestParams::new(WORKFLOW_STATUS_TOOL_NAME).with_arguments(object!({
                "workflow_id": "00000000-0000-7000-8000-000000000000"
            })),
            temp_dir.path(),
        )
        .unwrap_err();
        assert!(error
            .message
            .contains("workflow_id must start with `workflow_`"));
    }

    #[test]
    fn plan_normalizes_absolute_paths_inside_workspace() {
        let temp_dir = tempfile::tempdir().unwrap();
        let config = enabled_config();
        let state = CodingToolState::default();
        let started = execute_with_state(
            &config,
            &state,
            CallToolRequestParams::new(WORKFLOW_START_TOOL_NAME).with_arguments(object!({
                "objective": "create a new package"
            })),
            temp_dir.path(),
        )
        .unwrap();
        let started: Value = serde_json::from_str(&result_text(started)).unwrap();
        let intended_path = temp_dir.path().join("src/package/__init__.py");
        let planned = execute_with_state(
            &config,
            &state,
            CallToolRequestParams::new(WORKFLOW_SET_PLAN_TOOL_NAME).with_arguments(object!({
                "workflow_id": started["id"].as_str().unwrap(),
                "plan": {
                    "affected_components": ["package"],
                    "relevant_files": [intended_path.to_string_lossy().to_string()],
                    "risks": [],
                    "intended_changes": ["create package"],
                    "requirements": [{
                        "id": "package-file",
                        "description": "create the package file",
                        "source": "user",
                        "priority": "high",
                        "mandatory": true,
                        "verification": {"expected_files": ["src/package/__init__.py"], "check_ids": []}
                    }],
                    "tests": [],
                    "validation": [],
                    "rollback_strategy": "roll back the change batch"
                }
            })),
            temp_dir.path(),
        )
        .unwrap();
        let planned: Value = serde_json::from_str(&result_text(planned)).unwrap();

        assert_eq!(
            planned["plan"]["relevant_files"],
            serde_json::json!(["src/package/__init__.py"])
        );
    }

    #[test]
    fn compact_plan_contract_builds_a_complete_validated_workflow_plan() {
        let temp_dir = tempfile::tempdir().unwrap();
        let config = enabled_config();
        let state = CodingToolState::default();
        let started = execute_with_state(
            &config,
            &state,
            CallToolRequestParams::new(WORKFLOW_START_TOOL_NAME).with_arguments(object!({
                "objective": "repair the package"
            })),
            temp_dir.path(),
        )
        .unwrap();
        let started: Value = serde_json::from_str(&result_text(started)).unwrap();

        let planned = execute_with_state(
            &config,
            &state,
            CallToolRequestParams::new(WORKFLOW_SET_PLAN_TOOL_NAME).with_arguments(object!({
                "workflow_id": started["id"].as_str().unwrap(),
                "relevant_files": ["lib.rs"],
                "intended_change": "normalize labels as lowercase",
                "plan_steps": [
                    "Update lib.rs to normalize labels as lowercase",
                    "Run the library test suite"
                ],
                "validation_program": "cargo test --lib",
                "args": ["--no-fail-fast"]
            })),
            temp_dir.path(),
        )
        .unwrap();
        let planned: Value = serde_json::from_str(&result_text(planned)).unwrap();

        assert_eq!(planned["phase"], "planning");
        assert_eq!(planned["plan"]["requirements"][0]["mandatory"], true);
        assert_eq!(
            planned["plan"]["intended_changes"],
            serde_json::json!([
                "Update lib.rs to normalize labels as lowercase",
                "Run the library test suite"
            ])
        );
        assert_eq!(planned["plan"]["requirements"].as_array().unwrap().len(), 2);
        assert_eq!(
            planned["plan"]["validation"][0]["command"]["program"],
            "cargo"
        );
        assert_eq!(
            planned["plan"]["validation"][0]["command"]["args"],
            serde_json::json!(["test", "--lib", "--no-fail-fast"])
        );
    }

    #[test]
    fn compact_plan_deduplicates_program_arguments_repeated_in_args() {
        let plan = WorkflowCompactPlanParams {
            workflow_id: serde_json::from_str("\"workflow_00000000-0000-7000-8000-000000000000\"")
                .unwrap(),
            relevant_files: vec![PathBuf::from("script.js")],
            intended_change: "validate script.js".to_string(),
            plan_steps: Vec::new(),
            validation_program: "node --check script.js".to_string(),
            args: vec!["--check".to_string(), "script.js".to_string()],
        }
        .into_plan();

        assert_eq!(
            plan.validation[0].command.args,
            vec!["--check".to_string(), "script.js".to_string()]
        );
    }

    #[test]
    fn plan_rejects_absolute_paths_outside_workspace() {
        let temp_dir = tempfile::tempdir().unwrap();
        let external_dir = tempfile::tempdir().unwrap();
        let config = enabled_config();
        let state = CodingToolState::default();
        let started = execute_with_state(
            &config,
            &state,
            CallToolRequestParams::new(WORKFLOW_START_TOOL_NAME).with_arguments(object!({
                "objective": "create a new package"
            })),
            temp_dir.path(),
        )
        .unwrap();
        let started: Value = serde_json::from_str(&result_text(started)).unwrap();
        let external_path = external_dir.path().join("outside.py");
        let error = execute_with_state(
            &config,
            &state,
            CallToolRequestParams::new(WORKFLOW_SET_PLAN_TOOL_NAME).with_arguments(object!({
                "workflow_id": started["id"].as_str().unwrap(),
                "plan": {
                    "affected_components": ["package"],
                    "relevant_files": [external_path.to_string_lossy().to_string()],
                    "risks": [],
                    "intended_changes": ["create package"],
                    "requirements": [],
                    "tests": [],
                    "validation": [],
                    "rollback_strategy": "roll back the change batch"
                }
            })),
            temp_dir.path(),
        )
        .unwrap_err();

        assert!(error.message.contains("outside the coding workspace"));
    }

    #[test]
    fn definitions_use_object_schemas_for_every_property() {
        fn visit_schema(tool_name: &str, value: &Value, path: &str) {
            match value {
                Value::Object(object) => {
                    if let Some(properties) = object.get("properties") {
                        let properties = properties.as_object().unwrap_or_else(|| {
                            panic!("{tool_name} has non-object properties at {path}")
                        });
                        for (name, schema) in properties {
                            assert!(
                                schema.is_object(),
                                "{tool_name} has a non-object property schema at {path}.properties.{name}: {schema}"
                            );
                        }
                    }

                    for (key, child) in object {
                        visit_schema(tool_name, child, &format!("{path}.{key}"));
                    }
                }
                Value::Array(array) => {
                    for (index, child) in array.iter().enumerate() {
                        visit_schema(tool_name, child, &format!("{path}[{index}]"));
                    }
                }
                _ => {}
            }
        }

        for tool in definitions() {
            visit_schema(
                &tool.name,
                &Value::Object((*tool.input_schema).clone()),
                "$",
            );
        }
    }

    #[test]
    fn mutation_schema_is_flat_while_runtime_validation_remains_strict() {
        let schema = Value::Object(mutation_batch_schema());
        let item = &schema["properties"]["changes"]["items"];
        assert!(item.get("oneOf").is_none());
        assert!(item.get("anyOf").is_none());
        assert_eq!(
            item["properties"]["operation"]["enum"],
            serde_json::json!(["create", "write", "replace", "delete", "move"])
        );

        let temp_dir = tempfile::tempdir().unwrap();
        let error = execute(
            &enabled_config(),
            CallToolRequestParams::new(APPLY_CHANGES_TOOL_NAME).with_arguments(object!({
                "changes": [{"operation": "create", "path": "missing-content.txt"}]
            })),
            temp_dir.path(),
        )
        .unwrap_err();

        assert!(error.message.contains("missing field `content`"));
        assert!(!temp_dir.path().join("missing-content.txt").exists());
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
    fn prepares_versioned_context_within_the_requested_token_budget() {
        let temp_dir = tempfile::tempdir().unwrap();
        fs::write(
            temp_dir.path().join("billing.rs"),
            "pub fn process_invoice() {\n    validate_invoice();\n}\n",
        )
        .unwrap();

        let result = execute(
            &enabled_config(),
            CallToolRequestParams::new(PREPARE_CONTEXT_TOOL_NAME).with_arguments(object!({
                "query": "process invoice",
                "token_budget": 256,
                "max_files": 2,
                "chunk_lines": 20,
                "overlap_lines": 2
            })),
            temp_dir.path(),
        )
        .unwrap();
        let json: Value = serde_json::from_str(&result_text(result)).unwrap();

        assert!(json["used_tokens"].as_u64().unwrap() <= 256);
        assert_eq!(json["chunks"][0]["path"], "billing.rs");
        assert!(json["chunks"][0]["digest"]
            .as_str()
            .is_some_and(|digest| digest.starts_with("blake3:")));
        assert!(json["chunks"][0]["content"]
            .as_str()
            .is_some_and(|content| content.contains("process_invoice")));
    }

    #[tokio::test]
    async fn enforces_and_completes_a_large_planned_workflow_with_real_evidence() {
        let temp_dir = tempfile::tempdir().unwrap();
        let config = enabled_config();
        let state = CodingToolState::default();
        run_git(temp_dir.path(), &["init"]);
        let initial_tools = exposed_tool_names(&state, temp_dir.path());
        assert!(initial_tools.contains(WORKFLOW_START_TOOL_NAME));
        assert!(!initial_tools.contains(APPLY_CHANGES_TOOL_NAME));
        assert!(!initial_tools.contains(RUN_PROCESS_TOOL_NAME));
        assert!(!initial_tools.contains(WORKFLOW_SET_PLAN_TOOL_NAME));
        assert!(!initial_tools.contains(WORKFLOW_TRANSITION_TOOL_NAME));
        fs::write(
            temp_dir.path().join("context.rs"),
            "pub fn fixture_context() {}\n",
        )
        .unwrap();

        let blocked = CallToolRequestParams::new(APPLY_CHANGES_TOOL_NAME).with_arguments(object!({
            "changes": [
                {"operation": "create", "path": "one.txt", "content": "one\n"}
            ]
        }));
        let error = execute_with_state(&config, &state, blocked, temp_dir.path()).unwrap_err();
        assert!(error.message.contains("active coding workflow"));
        assert!(!temp_dir.path().join("one.txt").exists());

        let started = execute_with_state(
            &config,
            &state,
            CallToolRequestParams::new(WORKFLOW_START_TOOL_NAME).with_arguments(object!({
                "objective": "create four fixture files"
            })),
            temp_dir.path(),
        )
        .unwrap();
        let started: Value = serde_json::from_str(&result_text(started)).unwrap();
        let workflow_id = started["id"].as_str().unwrap().to_string();
        assert_eq!(started["phase"], "analyzing");
        assert_eq!(started["next_action"], "inspect");
        let analyzing_tools = exposed_tool_names(&state, temp_dir.path());
        assert!(analyzing_tools.contains(WORKFLOW_SET_PLAN_TOOL_NAME));
        assert!(analyzing_tools.contains(PREPARE_CONTEXT_TOOL_NAME));
        assert!(!analyzing_tools.contains(APPLY_CHANGES_TOOL_NAME));
        assert!(!analyzing_tools.contains(RUN_PROCESS_TOOL_NAME));
        assert!(!analyzing_tools.contains(WORKFLOW_TRANSITION_TOOL_NAME));

        execute_with_state(
            &config,
            &state,
            CallToolRequestParams::new(REPOSITORY_PROFILE_TOOL_NAME),
            temp_dir.path(),
        )
        .unwrap();
        execute_with_state(
            &config,
            &state,
            CallToolRequestParams::new(READ_FILE_TOOL_NAME).with_arguments(object!({
                "path": "context.rs"
            })),
            temp_dir.path(),
        )
        .unwrap();
        execute_with_state(
            &config,
            &state,
            CallToolRequestParams::new(SEARCH_SYMBOLS_TOOL_NAME).with_arguments(object!({
                "query": "fixture_context"
            })),
            temp_dir.path(),
        )
        .unwrap();
        execute_with_state(
            &config,
            &state,
            CallToolRequestParams::new(WORKFLOW_UPDATE_MEMORY_TOOL_NAME).with_arguments(object!({
                "workflow_id": workflow_id.clone(),
                "assumptions": ["the fixture directory is writable"],
                "open_points": ["confirm command execution"]
            })),
            temp_dir.path(),
        )
        .unwrap();
        let plan =
            CallToolRequestParams::new(WORKFLOW_SET_PLAN_TOOL_NAME).with_arguments(object!({
                "workflow_id": workflow_id.clone(),
                "plan": {
                    "affected_components": ["fixture"],
                    "relevant_files": ["one.txt", "two.txt", "three.txt", "four.txt"],
                    "risks": [],
                    "intended_changes": ["create four files"],
                    "requirements": [{
                        "id": "fixture-files",
                        "description": "create and validate the fixture files",
                        "source": "user",
                        "priority": "high",
                        "mandatory": true,
                        "verification": {
                            "expected_files": ["one.txt", "two.txt", "three.txt", "four.txt"],
                            "check_ids": ["rustc-version"]
                        }
                    }],
                    "tests": [],
                    "validation": [{
                        "id": "rustc-version",
                        "description": "confirm the Rust compiler runtime",
                        "command": {"program": "rustc", "args": ["--version"], "cwd": "."},
                        "required": true
                    }],
                    "rollback_strategy": "use the returned rollback id"
                }
            }));
        let planned = execute_with_state(&config, &state, plan, temp_dir.path()).unwrap();
        let planned: Value = serde_json::from_str(&result_text(planned)).unwrap();
        assert_eq!(planned["phase"], "planning");
        assert_eq!(planned["next_action"], "begin_editing");
        assert_eq!(
            exposed_transition_values(&state, temp_dir.path()),
            ["begin_editing"]
        );
        assert!(!exposed_tool_names(&state, temp_dir.path()).contains(APPLY_CHANGES_TOOL_NAME));

        transition(
            &config,
            &state,
            temp_dir.path(),
            &workflow_id,
            "begin_editing",
        );
        let editing_tools = exposed_tool_names(&state, temp_dir.path());
        assert!(editing_tools.contains(APPLY_CHANGES_TOOL_NAME));
        assert!(!editing_tools.contains(PREPARE_CONTEXT_TOOL_NAME));
        assert!(!editing_tools.contains(RUN_PROCESS_TOOL_NAME));
        assert!(!editing_tools.contains(WORKFLOW_UPDATE_MEMORY_TOOL_NAME));
        assert!(!editing_tools.contains(WORKFLOW_TRANSITION_TOOL_NAME));
        let canonical_root = temp_dir.path().canonicalize().unwrap();
        assert!(state
            .workflow_guidance_for_workspace(&canonical_root)
            .is_some_and(|guidance| guidance.contains("exposed mutation tool now")));
        let apply = CallToolRequestParams::new(APPLY_CHANGES_TOOL_NAME).with_arguments(object!({
            "changes": [
                {"operation": "create", "path": "one.txt", "content": "one\n"},
                {"operation": "create", "path": "two.txt", "content": "two\n"},
                {"operation": "create", "path": "three.txt", "content": "three\n"},
                {"operation": "create", "path": "four.txt", "content": "four\n"}
            ]
        }));
        execute_with_state(&config, &state, apply, temp_dir.path()).unwrap();
        assert_eq!(
            state
                .workflow_status(&canonical_root, None)
                .unwrap()
                .next_action,
            WorkflowNextAction::BeginValidation
        );
        assert_eq!(
            exposed_transition_values(&state, temp_dir.path()),
            ["begin_validation"]
        );
        transition(
            &config,
            &state,
            temp_dir.path(),
            &workflow_id,
            "begin_validation",
        );
        let testing_tools = exposed_tool_names(&state, temp_dir.path());
        assert!(testing_tools.contains(RUN_PROCESS_TOOL_NAME));
        assert!(testing_tools.contains(RUN_VALIDATION_TOOL_NAME));
        assert!(!testing_tools.contains(APPLY_CHANGES_TOOL_NAME));
        assert!(!testing_tools.contains(WORKFLOW_TRANSITION_TOOL_NAME));

        execute_async(
            &config,
            &state,
            CallToolRequestParams::new(RUN_PROCESS_TOOL_NAME).with_arguments(object!({
                "program": "rustc",
                "args": ["--version"]
            })),
            temp_dir.path(),
        )
        .await
        .unwrap();
        assert_eq!(
            state
                .workflow_status(&canonical_root, None)
                .unwrap()
                .next_action,
            WorkflowNextAction::BeginReview
        );
        assert_eq!(
            exposed_transition_values(&state, temp_dir.path()),
            ["begin_review"]
        );
        transition(
            &config,
            &state,
            temp_dir.path(),
            &workflow_id,
            "begin_review",
        );
        let reviewing_tools = exposed_tool_names(&state, temp_dir.path());
        assert!(reviewing_tools.contains(REVIEW_CHANGES_TOOL_NAME));
        assert!(!reviewing_tools.contains(WORKFLOW_COMPLETE_TOOL_NAME));
        assert!(!reviewing_tools.contains(APPLY_CHANGES_TOOL_NAME));
        execute_async(
            &config,
            &state,
            CallToolRequestParams::new(REVIEW_CHANGES_TOOL_NAME),
            temp_dir.path(),
        )
        .await
        .unwrap();
        assert_eq!(
            state
                .workflow_status(&canonical_root, None)
                .unwrap()
                .next_action,
            WorkflowNextAction::Complete
        );
        assert!(exposed_tool_names(&state, temp_dir.path()).contains(WORKFLOW_COMPLETE_TOOL_NAME));
        let completed = execute_with_state(
            &config,
            &state,
            CallToolRequestParams::new(WORKFLOW_COMPLETE_TOOL_NAME).with_arguments(object!({
                "workflow_id": workflow_id,
                "summary": "created and validated four fixture files",
                "remaining_risks": []
            })),
            temp_dir.path(),
        )
        .unwrap();
        let completed: Value = serde_json::from_str(&result_text(completed)).unwrap();

        assert_eq!(completed["phase"], "completed");
        assert_eq!(completed["verified"], true);
        assert_eq!(completed["changed_files"].as_array().unwrap().len(), 4);
        assert_eq!(completed["validations"][0]["outcome"], "passed");
        assert!(completed["validations"][0].get("stdout").is_none());
        assert!(completed["validations"][0].get("stderr").is_none());
        assert_eq!(
            completed["memory"]["assumptions"][0],
            "the fixture directory is writable"
        );
        assert_eq!(completed["memory"]["read_files"][0], "context.rs");
        assert_eq!(
            completed["memory"]["relevant_symbols"][0]["name"],
            "fixture_context"
        );
        assert_eq!(
            completed["memory"]["executed_commands"][0]["program"],
            "rustc"
        );
        assert!(completed["memory"]["executed_commands"][0]
            .get("args")
            .is_none());
        let terminal_tools = exposed_tool_names(&state, temp_dir.path());
        assert!(terminal_tools.contains(WORKFLOW_START_TOOL_NAME));
        assert!(terminal_tools.contains(WORKFLOW_STATUS_TOOL_NAME));
        assert!(!terminal_tools.contains(WORKFLOW_COMPLETE_TOOL_NAME));
    }

    #[tokio::test]
    async fn validation_tool_never_reports_an_unknown_command_as_passed() {
        let temp_dir = tempfile::tempdir().unwrap();
        let result = execute_async(
            &enabled_config(),
            &CodingToolState::default(),
            CallToolRequestParams::new(RUN_VALIDATION_TOOL_NAME).with_arguments(object!({
                "command_id": "validation:unknown"
            })),
            temp_dir.path(),
        )
        .await
        .unwrap();
        let json: Value = serde_json::from_str(&result_text(result)).unwrap();

        assert_eq!(json["status"], "not_present");
        assert!(json["command"].is_null());
        assert!(json["output"].is_null());
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
    fn opt_in_local_embeddings_are_visible_in_context_evidence() {
        let temp_dir = tempfile::tempdir().unwrap();
        fs::write(
            temp_dir.path().join("identity.rs"),
            "pub fn authenticate_user(credentials: Credentials) { validate(credentials); }\n",
        )
        .unwrap();
        fs::write(temp_dir.path().join("widget.rs"), "pub fn render() {}\n").unwrap();
        let mut config = enabled_config();
        config.embeddings = true;

        let context = execute(
            &config,
            CallToolRequestParams::new(SELECT_CONTEXT_TOOL_NAME).with_arguments(object!({
                "query": "login credential checks"
            })),
            temp_dir.path(),
        )
        .unwrap();
        let context: Value = serde_json::from_str(&result_text(context)).unwrap();

        assert_eq!(context[0]["path"], "identity.rs");
        assert!(context[0]["reasons"]
            .as_array()
            .unwrap()
            .contains(&Value::String("local_embedding".to_string())));
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
        fs::create_dir(temp_dir.path().join("outside")).unwrap();
        let call =
            CallToolRequestParams::new(REPOSITORY_INSTRUCTIONS_TOOL_NAME).with_arguments(object!({
                "path": temp_dir.path().join("outside")
            }));

        let error = execute(&enabled_config(), call, &root).unwrap_err();

        assert_eq!(error.code, ErrorCode::INVALID_PARAMS);
        assert!(error.message.contains("outside the coding workspace"));
    }

    #[test]
    fn default_configuration_exposes_the_core_tools_without_opt_in() {
        let temp_dir = tempfile::tempdir().unwrap();
        let call = CallToolRequestParams::new(REPOSITORY_PROFILE_TOOL_NAME);

        let result = execute(&CodingConfig::default(), call, temp_dir.path()).unwrap();
        let json: Value = serde_json::from_str(&result_text(result)).unwrap();

        assert!(json["root"].as_str().is_some_and(|root| !root.is_empty()));
        assert_eq!(json["scanned_files"], 0);
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
            "relative_path": "app.py",
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
        let config = enabled_config();
        begin_editing_workflow(&config, &state, temp_dir.path(), &["app.py"]);
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
        let preview = execute_with_state(&config, &state, preview_call, temp_dir.path()).unwrap();
        let preview_json: Value = serde_json::from_str(&result_text(preview)).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "before\n");
        assert!(preview_json["files"][0]["diff"]
            .as_str()
            .is_some_and(|diff| diff.contains("+after")));

        let apply_call =
            CallToolRequestParams::new(APPLY_CHANGES_TOOL_NAME).with_arguments(change_arguments);
        let applied = execute_with_state(&config, &state, apply_call, temp_dir.path()).unwrap();
        let applied_json: Value = serde_json::from_str(&result_text(applied)).unwrap();
        let rollback_id = applied_json["rollback_id"].as_str().unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "after\n");

        let rollback_call =
            CallToolRequestParams::new(ROLLBACK_CHANGES_TOOL_NAME).with_arguments(object!({
                "rollback_id": rollback_id
            }));
        execute_with_state(&config, &state, rollback_call, temp_dir.path()).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "before\n");

        let repeated =
            CallToolRequestParams::new(ROLLBACK_CHANGES_TOOL_NAME).with_arguments(object!({
                "rollback_id": rollback_id
            }));
        let error = execute_with_state(&config, &state, repeated, temp_dir.path()).unwrap_err();
        assert!(error.message.contains("unknown or expired rollback_id"));
    }

    #[cfg(unix)]
    #[test]
    fn applies_nested_absolute_paths_through_a_workspace_alias() {
        use std::os::unix::fs::symlink;

        let temp_dir = tempfile::tempdir().unwrap();
        let workspace = temp_dir.path().join("workspace");
        let alias = temp_dir.path().join("workspace-alias");
        fs::create_dir(&workspace).unwrap();
        symlink(&workspace, &alias).unwrap();
        let state = CodingToolState::default();
        let config = enabled_config();
        begin_editing_workflow(
            &config,
            &state,
            &alias,
            &["package/src/textslug/__init__.py"],
        );
        let nested = alias.join("package/src/textslug/__init__.py");
        let arguments = object!({
            "changes": [{
                "operation": "create",
                "path": nested,
                "content": "def slugify(value):\n    return value\n"
            }]
        });

        let applied = execute_with_state(
            &config,
            &state,
            CallToolRequestParams::new(APPLY_CHANGES_TOOL_NAME).with_arguments(arguments),
            &alias,
        )
        .unwrap();
        let applied: Value = serde_json::from_str(&result_text(applied)).unwrap();
        assert_eq!(
            fs::read_to_string(workspace.join("package/src/textslug/__init__.py")).unwrap(),
            "def slugify(value):\n    return value\n"
        );
        assert_eq!(
            applied["preview"]["files"][0]["path"],
            "package/src/textslug/__init__.py"
        );

        execute_with_state(
            &config,
            &state,
            CallToolRequestParams::new(ROLLBACK_CHANGES_TOOL_NAME).with_arguments(object!({
                "rollback_id": applied["rollback_id"].as_str().unwrap()
            })),
            &alias,
        )
        .unwrap();
        assert!(!workspace.join("package").exists());
    }

    #[test]
    fn moves_and_rolls_back_through_the_internal_patch_tools() {
        let temp_dir = tempfile::tempdir().unwrap();
        fs::create_dir(temp_dir.path().join("src")).unwrap();
        fs::write(temp_dir.path().join("app.py"), "print('safe')\n").unwrap();
        let digest =
            crate::coding::file::content_digest(&fs::read(temp_dir.path().join("app.py")).unwrap());
        let state = CodingToolState::default();
        let config = enabled_config();
        begin_editing_workflow(&config, &state, temp_dir.path(), &["app.py", "src/app.py"]);
        let arguments = object!({
            "changes": [{
                "operation": "move",
                "path": "app.py",
                "destination": "src/app.py",
                "expected_digest": digest
            }]
        });

        let preview = execute_with_state(
            &config,
            &state,
            CallToolRequestParams::new(PREVIEW_CHANGES_TOOL_NAME).with_arguments(arguments.clone()),
            temp_dir.path(),
        )
        .unwrap();
        let preview: Value = serde_json::from_str(&result_text(preview)).unwrap();
        assert_eq!(preview["files"][0]["operation"], "move_from");
        assert_eq!(preview["files"][1]["operation"], "move_to");

        let applied = execute_with_state(
            &config,
            &state,
            CallToolRequestParams::new(APPLY_CHANGES_TOOL_NAME).with_arguments(arguments),
            temp_dir.path(),
        )
        .unwrap();
        let applied: Value = serde_json::from_str(&result_text(applied)).unwrap();
        assert!(!temp_dir.path().join("app.py").exists());
        assert_eq!(
            fs::read_to_string(temp_dir.path().join("src/app.py")).unwrap(),
            "print('safe')\n"
        );

        execute_with_state(
            &config,
            &state,
            CallToolRequestParams::new(ROLLBACK_CHANGES_TOOL_NAME).with_arguments(object!({
                "rollback_id": applied["rollback_id"].as_str().unwrap()
            })),
            temp_dir.path(),
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(temp_dir.path().join("app.py")).unwrap(),
            "print('safe')\n"
        );
        assert!(!temp_dir.path().join("src/app.py").exists());
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
        let state = CodingToolState::default();
        begin_editing_workflow(&enabled_config(), &state, temp_dir.path(), &["large.txt"]);
        let call = CallToolRequestParams::new(APPLY_CHANGES_TOOL_NAME).with_arguments(object!({
            "changes": [{
                "operation": "write",
                "path": "large.txt",
                "expected_digest": digest,
                "content": replacement
            }]
        }));

        let error = execute_with_state(&config, &state, call, temp_dir.path()).unwrap_err();

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
    async fn lsp_queries_fail_closed_until_explicitly_enabled() {
        let temp_dir = tempfile::tempdir().unwrap();
        fs::write(temp_dir.path().join("lib.rs"), "fn value() {}\n").unwrap();
        let error = execute_async(
            &enabled_config(),
            &CodingToolState::default(),
            CallToolRequestParams::new(LSP_QUERY_TOOL_NAME).with_arguments(object!({
                "path": "lib.rs",
                "operation": "document_symbols"
            })),
            temp_dir.path(),
        )
        .await
        .unwrap_err();

        assert_eq!(error.code, ErrorCode::INVALID_REQUEST);
        assert!(error.message.contains("disabled by coding configuration"));
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

    #[tokio::test]
    async fn malformed_async_tool_calls_do_not_execute_or_mutate_workflow_state() {
        let temp_dir = tempfile::tempdir().unwrap();
        let workspace = CodingWorkspace::new(temp_dir.path()).unwrap();
        let mut config = enabled_config();
        config.lsp = true;
        let state = CodingToolState::default();
        execute_with_state(
            &config,
            &state,
            CallToolRequestParams::new(WORKFLOW_START_TOOL_NAME).with_arguments(object!({
                "objective": "inspect the fixture workspace"
            })),
            temp_dir.path(),
        )
        .unwrap();

        let marker = temp_dir.path().join("must-not-exist");
        let process_error = execute_async(
            &config,
            &state,
            CallToolRequestParams::new(RUN_PROCESS_TOOL_NAME).with_arguments(object!({
                "args": ["-c", "touch must-not-exist"]
            })),
            temp_dir.path(),
        )
        .await
        .unwrap_err();
        let lsp_error = execute_async(
            &config,
            &state,
            CallToolRequestParams::new(LSP_QUERY_TOOL_NAME).with_arguments(object!({
                "operation": "document_symbols"
            })),
            temp_dir.path(),
        )
        .await
        .unwrap_err();

        assert_eq!(process_error.code, ErrorCode::INVALID_PARAMS);
        assert!(process_error.message.contains("missing field `program`"));
        assert_eq!(lsp_error.code, ErrorCode::INVALID_PARAMS);
        assert!(lsp_error.message.contains("missing field `path`"));
        assert!(!marker.exists());
        assert_eq!(
            state.workflow_status(workspace.root(), None).unwrap().phase,
            WorkflowPhase::Analyzing
        );
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
            diagnostics: crate::coding::diagnostic::DiagnosticReport::default(),
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
    async fn reviews_local_added_lines_without_returning_secret_values() {
        let temp_dir = tempfile::tempdir().unwrap();
        run_git(temp_dir.path(), &["init"]);
        run_git(temp_dir.path(), &["config", "user.name", "Test User"]);
        run_git(
            temp_dir.path(),
            &["config", "user.email", "test@example.com"],
        );
        fs::create_dir(temp_dir.path().join("src")).unwrap();
        fs::write(temp_dir.path().join("src/app.rs"), "fn main() {}\n").unwrap();
        run_git(temp_dir.path(), &["add", "--", "src/app.rs"]);
        run_git(temp_dir.path(), &["commit", "-m", "initial"]);
        fs::write(
            temp_dir.path().join("src/app.rs"),
            "const API_KEY: &str = \"abcdefgh\";\nfn main() {}\n",
        )
        .unwrap();

        let result = execute_async(
            &enabled_config(),
            &CodingToolState::default(),
            CallToolRequestParams::new(REVIEW_CHANGES_TOOL_NAME).with_arguments(object!({
                "paths": ["src/app.rs"],
                "context_lines": 1
            })),
            temp_dir.path(),
        )
        .await
        .unwrap();
        let text = result_text(result);
        let report: Value = serde_json::from_str(&text).unwrap();

        assert_eq!(report["findings"][0]["severity"], "critical");
        assert_eq!(report["findings"][0]["path"], "src/app.rs");
        assert_eq!(report["findings"][0]["line"], 1);
        assert!(!text.contains("abcdefgh"));
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
        begin_editing_workflow(&config, &state, temp_dir.path(), &["app.txt"]);
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
        let commit_oid = committed_json["oid"].as_str().unwrap().to_string();
        let revert =
            CallToolRequestParams::new(GIT_REVERT_OWNED_TOOL_NAME).with_arguments(object!({
                "oid": commit_oid.clone()
            }));
        let reverted = execute_async(&config, &state, revert, temp_dir.path())
            .await
            .unwrap();
        let reverted_json: Value = serde_json::from_str(&result_text(reverted)).unwrap();
        let revert_oid = reverted_json["revert_oid"].as_str().unwrap().to_string();
        assert_eq!(reverted_json["reverted_oid"], commit_oid);
        assert_eq!(fs::read_to_string(&path).unwrap(), "before\n");
        let canonical_root = CodingWorkspace::new(temp_dir.path()).unwrap();
        assert!(!state.owns_commit(canonical_root.root(), &commit_oid));
        assert!(state.owns_commit(canonical_root.root(), &revert_oid));

        run_git(temp_dir.path(), &["init", "--bare", ".test-remote.git"]);
        run_git(
            temp_dir.path(),
            &["remote", "add", "test-origin", ".test-remote.git"],
        );
        let push = CallToolRequestParams::new(GIT_PUSH_OWNED_TOOL_NAME).with_arguments(object!({
            "oid": revert_oid,
            "remote": "test-origin"
        }));
        let pushed = execute_async(&config, &state, push, temp_dir.path())
            .await
            .unwrap();
        let pushed_json: Value = serde_json::from_str(&result_text(pushed)).unwrap();
        assert_eq!(pushed_json["oid"], revert_oid);
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
        assert_eq!(
            history_json["commits"][0]["subject"],
            "Revert \"agent-owned change\""
        );

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
