//! Crash-safe persistence and execution coordination for [`super::TaskRuntime`].

use super::{
    ActionOutcome, ActionRecord, GoalBudget, GoalEvidence, GoalId, GoalStatus, NeedUserInput,
    ResourceVersion, RetrySemantics, TaskCheckpoint, TaskEvent, TaskEventKind, TaskId, TaskLimits,
    TaskRuntime, TaskRuntimeError, TaskStatus, ToolDescriptor,
};
use blake3::Hasher;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Component, Path, PathBuf};
use std::time::Instant;
use thiserror::Error;
use uuid::Uuid;

pub const DURABLE_TASK_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceIdentity {
    pub root: Option<PathBuf>,
    pub repository_head: Option<String>,
}

impl WorkspaceIdentity {
    pub fn capture(workspace: Option<&Path>) -> Result<Self, DurableTaskError> {
        let root = workspace.map(fs::canonicalize).transpose()?;
        let repository_head = root.as_deref().and_then(read_repository_head);
        Ok(Self {
            root,
            repository_head,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DurableToolStatus {
    Requested,
    Running,
    Succeeded,
    Failed,
    UnknownOutcome,
    RequiresUserInput,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DurableToolCall {
    pub id: String,
    pub goal: GoalId,
    pub descriptor: ToolDescriptor,
    pub summary: String,
    pub status: DurableToolStatus,
    pub attempts: u32,
    pub failure: Option<String>,
    pub artifacts: Vec<ResourceVersion>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DurableTaskState {
    pub schema_version: u32,
    pub runtime: TaskCheckpoint,
    pub workspace: WorkspaceIdentity,
    pub tool_calls: BTreeMap<String, DurableToolCall>,
    pub artifacts: BTreeMap<PathBuf, ResourceVersion>,
    pub completion_reason: Option<String>,
    pub review_completed: bool,
}

#[derive(Debug, Deserialize)]
struct DurableTaskStateV0 {
    runtime: TaskCheckpoint,
    workspace: WorkspaceIdentity,
    tool_calls: BTreeMap<String, DurableToolCall>,
    artifacts: BTreeMap<PathBuf, ResourceVersion>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalRecord {
    pub schema_version: u32,
    pub task_id: TaskId,
    pub sequence: u64,
    pub event: TaskEventKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskSummary {
    pub id: TaskId,
    pub status: TaskStatus,
    pub original_goal: String,
    pub workspace: Option<PathBuf>,
    pub revision: u32,
    pub actions: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolExecutionResult {
    pub summary: String,
    pub evidence: Option<GoalEvidence>,
    pub changed_artifacts: Vec<PathBuf>,
}

impl ToolExecutionResult {
    pub fn succeeded(summary: impl Into<String>) -> Self {
        Self {
            summary: summary.into(),
            evidence: None,
            changed_artifacts: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolInvocation {
    pub id: String,
    pub goal: GoalId,
    pub tool: String,
    pub summary: String,
    pub attempt: u32,
    pub workspace: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RecoveryReport {
    pub retry_scheduled: Vec<String>,
    pub requires_user_input: Vec<String>,
    pub invalidated_evidence: usize,
    pub replanned_goals: Vec<GoalId>,
}

#[derive(Debug, Error)]
pub enum DurableTaskError {
    #[error("durable task store path is not a directory: {0}")]
    InvalidStorePath(PathBuf),
    #[error("invalid task identifier: {0}")]
    InvalidTaskId(String),
    #[error("task does not exist: {0}")]
    TaskNotFound(String),
    #[error("unsupported durable task schema version: {0}")]
    UnsupportedSchema(u32),
    #[error("durable task state is incomplete")]
    InvalidState,
    #[error("execution journal is corrupt: {0}")]
    CorruptJournal(String),
    #[error("tool call does not exist: {0}")]
    ToolCallNotFound(String),
    #[error("tool call cannot transition from {status:?}: {id}")]
    InvalidToolTransition {
        id: String,
        status: DurableToolStatus,
    },
    #[error("tool execution failed: {0}")]
    ToolExecution(String),
    #[error("tool result changed an undeclared artifact: {0}")]
    UndeclaredArtifact(PathBuf),
    #[error("task has no workspace for artifact tracking")]
    WorkspaceRequired,
    #[error("tool path escapes its declared workspace scope: {0}")]
    ScopeViolation(PathBuf),
    #[error(transparent)]
    Runtime(#[from] TaskRuntimeError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Clone)]
pub struct TaskStore {
    root: PathBuf,
}

impl TaskStore {
    pub fn new(root: impl AsRef<Path>) -> Result<Self, DurableTaskError> {
        let root = root.as_ref().to_path_buf();
        if root.exists() && !root.is_dir() {
            return Err(DurableTaskError::InvalidStorePath(root));
        }
        fs::create_dir_all(&root)?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn create_task(
        &self,
        original_goal: impl Into<String>,
        workspace: Option<PathBuf>,
        limits: TaskLimits,
    ) -> Result<DurableTask, DurableTaskError> {
        let runtime = TaskRuntime::new(original_goal, workspace.clone(), limits)?;
        let task_dir = self.task_dir(&runtime.id)?;
        fs::create_dir_all(&task_dir)?;
        let workspace = WorkspaceIdentity::capture(workspace.as_deref())?;
        let state = DurableTaskState {
            schema_version: DURABLE_TASK_SCHEMA_VERSION,
            runtime: runtime.checkpoint(),
            workspace,
            tool_calls: BTreeMap::new(),
            artifacts: BTreeMap::new(),
            completion_reason: None,
            review_completed: false,
        };
        let mut task = DurableTask {
            store: self.clone(),
            state,
        };
        task.persist()?;
        Ok(task)
    }

    pub fn load(&self, task_id: &str) -> Result<DurableTask, DurableTaskError> {
        let task_dir = self.task_dir_from_str(task_id)?;
        let state_path = task_dir.join("state.json");
        if !state_path.is_file() {
            return Err(DurableTaskError::TaskNotFound(task_id.to_string()));
        }
        let state_bytes = fs::read(&state_path)?;
        let stored_schema_version = state_schema_version(&state_bytes)?;
        let state = decode_state(&state_bytes)?;
        if state.runtime.runtime.id.as_str() != task_id {
            return Err(DurableTaskError::InvalidState);
        }
        TaskRuntime::restore(state.runtime.clone())?;
        let mut task = DurableTask {
            store: self.clone(),
            state,
        };
        if stored_schema_version == DURABLE_TASK_SCHEMA_VERSION {
            task.synchronize_journal()?;
        } else {
            task.persist()?;
        }
        Ok(task)
    }

    pub fn list(&self) -> Result<Vec<TaskSummary>, DurableTaskError> {
        let mut tasks = Vec::new();
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let task_id = entry.file_name().to_string_lossy().into_owned();
            let task = match self.load(&task_id) {
                Ok(task) => task,
                Err(DurableTaskError::TaskNotFound(_)) => continue,
                Err(error) => return Err(error),
            };
            tasks.push(task.summary());
        }
        tasks.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(tasks)
    }

    fn task_dir(&self, task_id: &TaskId) -> Result<PathBuf, DurableTaskError> {
        self.task_dir_from_str(task_id.as_str())
    }

    fn task_dir_from_str(&self, task_id: &str) -> Result<PathBuf, DurableTaskError> {
        if task_id.is_empty()
            || task_id.contains('/')
            || task_id.contains('\\')
            || task_id == "."
            || task_id == ".."
        {
            return Err(DurableTaskError::InvalidTaskId(task_id.to_string()));
        }
        Ok(self.root.join(task_id))
    }
}

pub struct DurableTask {
    store: TaskStore,
    state: DurableTaskState,
}

impl DurableTask {
    pub fn id(&self) -> &TaskId {
        &self.state.runtime.runtime.id
    }

    pub fn runtime(&self) -> &TaskRuntime {
        &self.state.runtime.runtime
    }

    pub fn state(&self) -> &DurableTaskState {
        &self.state
    }

    pub fn summary(&self) -> TaskSummary {
        let runtime = self.runtime();
        TaskSummary {
            id: runtime.id.clone(),
            status: runtime.status,
            original_goal: runtime.memory.original_goal.clone(),
            workspace: runtime.workspace.clone(),
            revision: runtime.revision,
            actions: runtime.actions,
        }
    }

    pub fn state_path(&self) -> PathBuf {
        self.store
            .task_dir(self.id())
            .expect("generated task identifiers are valid")
            .join("state.json")
    }

    pub fn journal_path(&self) -> PathBuf {
        self.store
            .task_dir(self.id())
            .expect("generated task identifiers are valid")
            .join("journal.jsonl")
    }

    pub fn events(&self) -> Result<Vec<JournalRecord>, DurableTaskError> {
        read_journal(&self.journal_path())
    }

    pub fn add_subtask(
        &mut self,
        parent: &GoalId,
        title: impl Into<String>,
        dependencies: BTreeSet<GoalId>,
        required_capabilities: BTreeSet<String>,
        budget: GoalBudget,
    ) -> Result<GoalId, DurableTaskError> {
        let goal = self.state.runtime.runtime.add_subtask(
            parent,
            title,
            dependencies,
            required_capabilities,
            budget,
        )?;
        self.persist()?;
        Ok(goal)
    }

    pub fn start_goal(&mut self, goal: &GoalId) -> Result<(), DurableTaskError> {
        self.state.runtime.runtime.start_goal(goal)?;
        self.persist()
    }

    pub fn add_evidence(
        &mut self,
        goal: &GoalId,
        evidence: GoalEvidence,
    ) -> Result<(), DurableTaskError> {
        self.state.runtime.runtime.add_evidence(goal, evidence)?;
        self.persist()
    }

    pub fn complete_goal(&mut self, goal: &GoalId) -> Result<(), DurableTaskError> {
        self.state.runtime.runtime.complete_goal(goal)?;
        self.persist()
    }

    pub fn replan(
        &mut self,
        goal: &GoalId,
        replacement_title: impl Into<String>,
        reason: impl Into<String>,
    ) -> Result<GoalId, DurableTaskError> {
        let replacement = self
            .state
            .runtime
            .runtime
            .replan(goal, replacement_title, reason)?;
        self.persist()?;
        Ok(replacement)
    }

    pub fn complete(&mut self, reason: impl Into<String>) -> Result<(), DurableTaskError> {
        self.state.runtime.runtime.record_review_started();
        self.state.runtime.runtime.complete()?;
        self.state.completion_reason = Some(reason.into());
        self.state.review_completed = true;
        self.state
            .runtime
            .runtime
            .record_checkpoint_created("task completed")?;
        self.persist()
    }

    pub fn cancel(&mut self, reason: impl Into<String>) -> Result<(), DurableTaskError> {
        self.state.runtime.runtime.cancel(reason);
        self.state
            .runtime
            .runtime
            .record_checkpoint_created("task cancelled")?;
        self.persist()
    }

    pub fn pause(&mut self) -> Result<(), DurableTaskError> {
        self.state.runtime.runtime.pause()?;
        self.state
            .runtime
            .runtime
            .record_checkpoint_created("task paused")?;
        self.persist()
    }

    pub fn provide_user_input(
        &mut self,
        answer: impl Into<String>,
    ) -> Result<(), DurableTaskError> {
        self.state.runtime.runtime.accept_user_input(answer)?;
        self.state
            .runtime
            .runtime
            .record_checkpoint_created("user input accepted")?;
        self.persist()
    }

    pub fn steer(
        &mut self,
        summary: impl Into<String>,
        affected_goals: &[GoalId],
    ) -> Result<Vec<GoalId>, DurableTaskError> {
        let was_paused = self.state.runtime.runtime.status == TaskStatus::Paused;
        if was_paused {
            let observed = self.workspace_versions()?;
            self.state.runtime.runtime.refresh_resources(&observed)?;
            self.state.runtime.runtime.resume()?;
        }
        self.state
            .runtime
            .runtime
            .apply_user_steering(summary, affected_goals)?;
        let mut replacements = Vec::new();
        for goal in affected_goals {
            let status = self
                .state
                .runtime
                .runtime
                .goals
                .get(goal)
                .ok_or_else(|| TaskRuntimeError::UnknownGoal(goal.clone()))?
                .status;
            if status == GoalStatus::Completed {
                self.state
                    .runtime
                    .runtime
                    .invalidate_goal_evidence_for_steering(goal)?;
            }
            if matches!(
                status,
                GoalStatus::Pending | GoalStatus::Running | GoalStatus::Completed
            ) {
                replacements.push(self.state.runtime.runtime.replan(
                    goal,
                    "reassess the affected goal after user steering",
                    "a persisted user constraint changed this part of the plan",
                )?);
            }
        }
        if was_paused {
            self.state.runtime.runtime.pause()?;
        }
        self.state
            .runtime
            .runtime
            .record_checkpoint_created("user steering applied")?;
        self.persist()?;
        Ok(replacements)
    }

    pub fn request_tool(
        &mut self,
        goal: &GoalId,
        descriptor: ToolDescriptor,
        summary: impl Into<String>,
    ) -> Result<String, DurableTaskError> {
        validate_tool_scope(&descriptor)?;
        let summary = summary.into();
        if summary.is_empty() {
            return Err(DurableTaskError::ToolExecution(
                "tool summary must not be empty".to_string(),
            ));
        }
        let id = format!("tool_{}", Uuid::now_v7());
        self.state.runtime.runtime.record_tool_requested(
            id.clone(),
            goal,
            descriptor.name.clone(),
        )?;
        self.state.tool_calls.insert(
            id.clone(),
            DurableToolCall {
                id: id.clone(),
                goal: goal.clone(),
                descriptor,
                summary,
                status: DurableToolStatus::Requested,
                attempts: 0,
                failure: None,
                artifacts: Vec::new(),
            },
        );
        self.persist()?;
        Ok(id)
    }

    pub fn mark_tool_started(
        &mut self,
        tool_call: &str,
    ) -> Result<ToolInvocation, DurableTaskError> {
        let call = self.tool_call(tool_call)?.clone();
        if !matches!(
            call.status,
            DurableToolStatus::Requested | DurableToolStatus::Failed
        ) {
            return Err(DurableTaskError::InvalidToolTransition {
                id: tool_call.to_string(),
                status: call.status,
            });
        }
        let attempt = call.attempts + 1;
        self.state.runtime.runtime.record_tool_started(
            call.id.clone(),
            &call.goal,
            call.descriptor.name.clone(),
            attempt,
        )?;
        let call = self.tool_call_mut(tool_call)?;
        call.status = DurableToolStatus::Running;
        call.attempts = attempt;
        call.failure = None;
        let invocation = ToolInvocation {
            id: call.id.clone(),
            goal: call.goal.clone(),
            tool: call.descriptor.name.clone(),
            summary: call.summary.clone(),
            attempt,
            workspace: self.state.workspace.root.clone(),
        };
        self.persist()?;
        Ok(invocation)
    }

    pub fn execute_tool<F>(
        &mut self,
        tool_call: &str,
        mut execute: F,
    ) -> Result<ToolExecutionResult, DurableTaskError>
    where
        F: FnMut(&ToolInvocation) -> Result<ToolExecutionResult, String>,
    {
        loop {
            let invocation = self.mark_tool_started(tool_call)?;
            let descriptor = self.tool_call(tool_call)?.descriptor.clone();
            let started = Instant::now();
            let result = execute(&invocation);
            let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
            let result = match descriptor.execution.timeout_ms {
                Some(timeout_ms) if elapsed_ms > timeout_ms => Err(format!(
                    "tool execution exceeded its {}ms timeout",
                    timeout_ms
                )),
                _ => result,
            };
            match result {
                Ok(result) => {
                    self.complete_tool_success(tool_call, result.clone(), elapsed_ms)?;
                    return Ok(result);
                }
                Err(reason) => {
                    self.record_tool_failure(tool_call, reason.clone(), elapsed_ms)?;
                    let call = self.tool_call(tool_call)?;
                    let may_retry = matches!(call.descriptor.retry, RetrySemantics::Bounded)
                        && call.descriptor.can_retry_after_interruption()
                        && call.attempts < 2;
                    if !may_retry {
                        return Err(DurableTaskError::ToolExecution(reason));
                    }
                }
            }
        }
    }

    pub fn recover(&mut self) -> Result<RecoveryReport, DurableTaskError> {
        let interrupted = self
            .state
            .tool_calls
            .values()
            .filter(|call| call.status == DurableToolStatus::Running)
            .cloned()
            .collect::<Vec<_>>();
        let mut report = RecoveryReport::default();
        let mut input_request: Option<NeedUserInput> = None;
        for call in interrupted {
            self.state
                .runtime
                .runtime
                .record_recovery_started(call.id.clone())?;
            if call.descriptor.can_retry_after_interruption() {
                self.tool_call_mut(&call.id)?.status = DurableToolStatus::Requested;
                self.state.runtime.runtime.record_recovery_succeeded(
                    call.id.clone(),
                    "retry scheduled from persisted tool contract",
                )?;
                report.retry_scheduled.push(call.id);
            } else {
                let reason =
                    "tool outcome is unknown and its execution contract does not allow replay";
                self.tool_call_mut(&call.id)?.status = DurableToolStatus::UnknownOutcome;
                self.tool_call_mut(&call.id)?.failure = Some(reason.to_string());
                self.state
                    .runtime
                    .runtime
                    .record_recovery_failed(call.id.clone(), reason)?;
                report.requires_user_input.push(call.id.clone());
                if input_request.is_none() {
                    input_request = Some(NeedUserInput {
                        required_information: "confirm the observed result of the interrupted tool or select a safe recovery".to_string(),
                        reason: reason.to_string(),
                        blocked_goal: call.goal,
                        allowed_options: vec![
                            "confirm result".to_string(),
                            "inspect before retry".to_string(),
                            "cancel task".to_string(),
                        ],
                    });
                }
            }
        }
        if let Some(request) = input_request {
            if self.state.runtime.runtime.status == TaskStatus::Running {
                self.state.runtime.runtime.request_user_input(request)?;
            }
        }
        if !report.retry_scheduled.is_empty() || !report.requires_user_input.is_empty() {
            self.state
                .runtime
                .runtime
                .record_checkpoint_created("recovery evaluated")?;
            self.persist()?;
        }
        Ok(report)
    }

    pub fn resume(&mut self) -> Result<RecoveryReport, DurableTaskError> {
        let mut report = self.recover()?;
        if self.state.runtime.runtime.status == TaskStatus::Waiting
            || self.state.runtime.runtime.is_terminal()
        {
            return Ok(report);
        }
        if self.state.runtime.runtime.status == TaskStatus::Running {
            self.state.runtime.runtime.pause()?;
        }
        if self.state.runtime.runtime.status != TaskStatus::Paused {
            return Ok(report);
        }
        let observed = self.workspace_versions()?;
        report.invalidated_evidence = self.state.runtime.runtime.refresh_resources(&observed)?;
        self.state.runtime.runtime.resume()?;
        let stale_goals = self
            .state
            .runtime
            .runtime
            .goals
            .values()
            .filter(|goal| {
                goal.status == GoalStatus::Completed
                    && goal.evidence.iter().any(|evidence| !evidence.valid)
            })
            .map(|goal| goal.id.clone())
            .collect::<Vec<_>>();
        for stale_goal in stale_goals {
            let replacement = self.state.runtime.runtime.replan(
                &stale_goal,
                "revalidate externally changed workspace evidence",
                "a persisted resource digest changed after the checkpoint",
            )?;
            report.replanned_goals.push(replacement);
        }
        self.state
            .runtime
            .runtime
            .record_checkpoint_created("task resumed after workspace refresh")?;
        self.persist()?;
        Ok(report)
    }

    pub fn replay_validate(&self) -> Result<(), DurableTaskError> {
        let events = self.events()?;
        if events.is_empty() {
            return Err(DurableTaskError::CorruptJournal(
                "journal has no task creation event".to_string(),
            ));
        }
        let mut started = BTreeMap::<String, u32>::new();
        let mut completed = BTreeSet::<String>::new();
        for (expected_sequence, event) in events.iter().enumerate() {
            if event.sequence != u64::try_from(expected_sequence).unwrap_or(u64::MAX) {
                return Err(DurableTaskError::CorruptJournal(
                    "event sequences are not contiguous".to_string(),
                ));
            }
            match &event.event {
                TaskEventKind::ToolStarted { tool_call, .. } => {
                    *started.entry(tool_call.clone()).or_default() += 1;
                }
                TaskEventKind::ToolSucceeded { tool_call, .. }
                    if started.get(tool_call).copied().unwrap_or_default() == 0
                        || !completed.insert(tool_call.clone()) =>
                {
                    return Err(DurableTaskError::CorruptJournal(format!(
                        "tool success has no unique preceding start: {tool_call}"
                    )));
                }
                _ => {}
            }
        }
        for call in self.state.tool_calls.values() {
            let starts = started.get(&call.id).copied().unwrap_or_default();
            if call.descriptor.access == super::AccessMode::Write
                && starts > 1
                && !call.descriptor.can_retry_after_interruption()
            {
                return Err(DurableTaskError::CorruptJournal(format!(
                    "non-retry-safe mutating tool was started more than once: {}",
                    call.id
                )));
            }
        }
        if self.state.runtime.runtime.status == TaskStatus::Completed
            && self.state.runtime.runtime.goals.values().any(|goal| {
                goal.status == GoalStatus::Completed
                    && !goal.evidence.iter().any(|evidence| evidence.valid)
            })
        {
            return Err(DurableTaskError::CorruptJournal(
                "completed task contains invalid evidence".to_string(),
            ));
        }
        Ok(())
    }

    fn complete_tool_success(
        &mut self,
        tool_call: &str,
        result: ToolExecutionResult,
        elapsed_ms: u64,
    ) -> Result<(), DurableTaskError> {
        let call = self.tool_call(tool_call)?.clone();
        if call.status != DurableToolStatus::Running {
            return Err(DurableTaskError::InvalidToolTransition {
                id: tool_call.to_string(),
                status: call.status,
            });
        }
        let artifacts = self.capture_artifacts(&call, &result.changed_artifacts)?;
        self.state.runtime.runtime.record_action(
            &call.goal,
            ActionRecord {
                tool: call.descriptor.name.clone(),
                summary: result.summary.clone(),
                outcome: ActionOutcome::Succeeded,
                failure_fingerprint: None,
                process_time_ms: Some(elapsed_ms),
            },
        )?;
        if let Some(evidence) = result.evidence {
            self.state
                .runtime
                .runtime
                .add_evidence(&call.goal, evidence)?;
        }
        for artifact in &artifacts {
            self.state
                .runtime
                .runtime
                .record_artifact_changed(call.id.clone(), artifact.resource.clone())?;
            self.state
                .artifacts
                .insert(artifact.resource.clone(), artifact.clone());
        }
        self.state.runtime.runtime.record_tool_succeeded(
            call.id.clone(),
            &call.goal,
            call.descriptor.name.clone(),
        )?;
        let call = self.tool_call_mut(tool_call)?;
        call.status = DurableToolStatus::Succeeded;
        call.artifacts = artifacts;
        self.state
            .runtime
            .runtime
            .record_checkpoint_created("tool result committed")?;
        self.persist()
    }

    fn record_tool_failure(
        &mut self,
        tool_call: &str,
        reason: String,
        elapsed_ms: u64,
    ) -> Result<(), DurableTaskError> {
        let call = self.tool_call(tool_call)?.clone();
        self.state.runtime.runtime.record_tool_failed(
            call.id.clone(),
            &call.goal,
            call.descriptor.name.clone(),
            reason.clone(),
        )?;
        let action = self.state.runtime.runtime.record_action(
            &call.goal,
            ActionRecord {
                tool: call.descriptor.name.clone(),
                summary: reason.clone(),
                outcome: ActionOutcome::Failed,
                failure_fingerprint: Some(format!("{}:failure", call.descriptor.name)),
                process_time_ms: Some(elapsed_ms),
            },
        );
        let call = self.tool_call_mut(tool_call)?;
        call.status = DurableToolStatus::Failed;
        call.failure = Some(reason);
        self.state
            .runtime
            .runtime
            .record_checkpoint_created("tool failure committed")?;
        self.persist()?;
        action.map_err(DurableTaskError::from)
    }

    fn capture_artifacts(
        &self,
        call: &DurableToolCall,
        changed_artifacts: &[PathBuf],
    ) -> Result<Vec<ResourceVersion>, DurableTaskError> {
        if changed_artifacts.is_empty() {
            return Ok(Vec::new());
        }
        let workspace = self
            .state
            .workspace
            .root
            .as_deref()
            .ok_or(DurableTaskError::WorkspaceRequired)?;
        let mut artifacts = Vec::new();
        for artifact in changed_artifacts {
            ensure_declared_artifact(artifact, &call.descriptor)?;
            let version = resource_version(workspace, artifact)?;
            if version.fingerprint.is_none() {
                return Err(DurableTaskError::UndeclaredArtifact(artifact.clone()));
            }
            artifacts.push(version);
        }
        Ok(artifacts)
    }

    fn workspace_versions(&self) -> Result<Vec<ResourceVersion>, DurableTaskError> {
        let workspace = match self.state.workspace.root.as_deref() {
            Some(workspace) => workspace,
            None => return Ok(Vec::new()),
        };
        let mut resources = BTreeSet::new();
        for goal in self.state.runtime.runtime.goals.values() {
            for evidence in &goal.evidence {
                resources.extend(
                    evidence
                        .resources
                        .iter()
                        .map(|resource| resource.resource.clone()),
                );
            }
        }
        resources.extend(self.state.artifacts.keys().cloned());
        resources
            .into_iter()
            .map(|resource| resource_version(workspace, &resource))
            .collect()
    }

    fn tool_call(&self, tool_call: &str) -> Result<&DurableToolCall, DurableTaskError> {
        self.state
            .tool_calls
            .get(tool_call)
            .ok_or_else(|| DurableTaskError::ToolCallNotFound(tool_call.to_string()))
    }

    fn tool_call_mut(&mut self, tool_call: &str) -> Result<&mut DurableToolCall, DurableTaskError> {
        self.state
            .tool_calls
            .get_mut(tool_call)
            .ok_or_else(|| DurableTaskError::ToolCallNotFound(tool_call.to_string()))
    }

    fn persist(&mut self) -> Result<(), DurableTaskError> {
        self.state.schema_version = DURABLE_TASK_SCHEMA_VERSION;
        self.state.runtime = self.state.runtime.runtime.checkpoint();
        atomic_write_json(&self.state_path(), &self.state)?;
        self.synchronize_journal()
    }

    fn synchronize_journal(&mut self) -> Result<(), DurableTaskError> {
        let path = self.journal_path();
        let last_sequence = read_journal(&path)?.last().map(|event| event.sequence);
        let pending = self
            .state
            .runtime
            .runtime
            .events()
            .filter(|event| last_sequence.is_none_or(|last| event.sequence > last))
            .cloned()
            .collect::<Vec<TaskEvent>>();
        if pending.is_empty() {
            return Ok(());
        }
        let mut journal = OpenOptions::new().create(true).append(true).open(path)?;
        for event in pending {
            let record = JournalRecord {
                schema_version: DURABLE_TASK_SCHEMA_VERSION,
                task_id: self.id().clone(),
                sequence: event.sequence,
                event: event.event,
            };
            serde_json::to_writer(&mut journal, &record)?;
            journal.write_all(b"\n")?;
        }
        journal.sync_data()?;
        Ok(())
    }
}

pub fn resource_version(
    workspace: &Path,
    resource: impl AsRef<Path>,
) -> Result<ResourceVersion, DurableTaskError> {
    let resource = normalized_relative_path(resource.as_ref())?;
    let workspace = fs::canonicalize(workspace)?;
    let path = workspace.join(&resource);
    let fingerprint = match fs::canonicalize(&path) {
        Ok(path) => {
            if !path.starts_with(&workspace) {
                return Err(DurableTaskError::ScopeViolation(resource));
            }
            let bytes = fs::read(path)?;
            let mut hasher = Hasher::new();
            hasher.update(&bytes);
            Some(hasher.finalize().to_hex().to_string())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    Ok(ResourceVersion {
        resource,
        fingerprint,
    })
}

fn decode_state(bytes: &[u8]) -> Result<DurableTaskState, DurableTaskError> {
    let value = serde_json::from_slice::<serde_json::Value>(bytes)?;
    let schema_version = value
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .ok_or(DurableTaskError::InvalidState)?;
    let state = match schema_version {
        DURABLE_TASK_SCHEMA_VERSION => serde_json::from_value(value)?,
        0 => {
            let state = serde_json::from_value::<DurableTaskStateV0>(value)?;
            DurableTaskState {
                schema_version: DURABLE_TASK_SCHEMA_VERSION,
                runtime: state.runtime,
                workspace: state.workspace,
                tool_calls: state.tool_calls,
                artifacts: state.artifacts,
                completion_reason: None,
                review_completed: false,
            }
        }
        version => return Err(DurableTaskError::UnsupportedSchema(version)),
    };
    if state.runtime.schema_version != 1 || state.runtime.runtime.goals.is_empty() {
        return Err(DurableTaskError::InvalidState);
    }
    Ok(state)
}

fn state_schema_version(bytes: &[u8]) -> Result<u32, DurableTaskError> {
    serde_json::from_slice::<serde_json::Value>(bytes)?
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .ok_or(DurableTaskError::InvalidState)
}

fn atomic_write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), DurableTaskError> {
    let parent = path.parent().ok_or(DurableTaskError::InvalidState)?;
    fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(".state-{}.tmp", Uuid::now_v7()));
    let mut file = File::create(&temporary)?;
    serde_json::to_writer_pretty(&mut file, value)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    fs::rename(&temporary, path)?;
    File::open(parent)?.sync_all()?;
    Ok(())
}

fn read_journal(path: &Path) -> Result<Vec<JournalRecord>, DurableTaskError> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let reader = BufReader::new(File::open(path)?);
    let mut records = Vec::new();
    for (line_number, line) in reader.lines().enumerate() {
        let line = line?;
        if line.is_empty() {
            continue;
        }
        let record = serde_json::from_str::<JournalRecord>(&line).map_err(|error| {
            DurableTaskError::CorruptJournal(format!("line {}: {error}", line_number + 1))
        })?;
        if record.schema_version != DURABLE_TASK_SCHEMA_VERSION {
            return Err(DurableTaskError::UnsupportedSchema(record.schema_version));
        }
        records.push(record);
    }
    Ok(records)
}

fn validate_tool_scope(descriptor: &ToolDescriptor) -> Result<(), DurableTaskError> {
    if descriptor.execution.timeout_ms == Some(0) {
        return Err(DurableTaskError::ToolExecution(
            "tool timeout must be positive".to_string(),
        ));
    }
    for path in descriptor
        .execution
        .required_scope
        .iter()
        .chain(descriptor.execution.expected_artifacts.iter())
    {
        normalized_relative_path(path)?;
    }
    Ok(())
}

fn ensure_declared_artifact(
    artifact: &Path,
    descriptor: &ToolDescriptor,
) -> Result<(), DurableTaskError> {
    let artifact = normalized_relative_path(artifact)?;
    let expected = &descriptor.execution.expected_artifacts;
    let in_expected_artifacts = expected
        .iter()
        .any(|path| normalized_relative_path(path).is_ok_and(|expected| expected == artifact));
    let in_scope = descriptor.execution.required_scope.iter().any(|scope| {
        normalized_relative_path(scope).is_ok_and(|scope| {
            artifact.starts_with(scope) && descriptor.execution.expected_artifacts.is_empty()
        })
    });
    if in_expected_artifacts || in_scope {
        Ok(())
    } else {
        Err(DurableTaskError::UndeclaredArtifact(artifact))
    }
}

fn normalized_relative_path(path: &Path) -> Result<PathBuf, DurableTaskError> {
    if path.as_os_str().is_empty() || path.is_absolute() {
        return Err(DurableTaskError::ScopeViolation(path.to_path_buf()));
    }
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => normalized.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(DurableTaskError::ScopeViolation(path.to_path_buf()));
            }
        }
    }
    if normalized.as_os_str().is_empty() {
        return Err(DurableTaskError::ScopeViolation(path.to_path_buf()));
    }
    Ok(normalized)
}

fn read_repository_head(root: &Path) -> Option<String> {
    let head_path = root.join(".git/HEAD");
    let head = fs::read_to_string(&head_path).ok()?;
    match head.strip_prefix("ref: ") {
        Some(reference) => fs::read_to_string(root.join(".git").join(reference.trim()))
            .ok()
            .map(|value| value.trim().to_string())
            .or_else(|| Some(head.trim().to_string())),
        None => Some(head.trim().to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::{
        AccessMode, CostClass, LatencyClass, NetworkRequirement, RiskLevel, SideEffect, TaskDomain,
        WorkspaceRequirement,
    };

    fn descriptor(
        name: &str,
        access: AccessMode,
        retry: RetrySemantics,
        expected_artifacts: impl IntoIterator<Item = PathBuf>,
    ) -> ToolDescriptor {
        let is_read_only = access == AccessMode::Read;
        ToolDescriptor {
            name: name.to_string(),
            domain: TaskDomain::Coding,
            access,
            risk: RiskLevel::Low,
            preconditions: Vec::new(),
            side_effect: if is_read_only {
                SideEffect::None
            } else {
                SideEffect::ScopedWorkspaceWrite
            },
            required_capability: None,
            produced_evidence: vec!["verification".to_string()],
            cost: CostClass::Low,
            latency: LatencyClass::Immediate,
            network: NetworkRequirement::None,
            workspace: if is_read_only {
                WorkspaceRequirement::Readable
            } else {
                WorkspaceRequirement::Writable
            },
            retry,
            execution: super::super::ToolExecutionSemantics {
                idempotent: is_read_only,
                retry_safe: is_read_only,
                recoverable: is_read_only,
                resumable: is_read_only,
                required_scope: vec![PathBuf::from("workspace")],
                expected_artifacts: expected_artifacts.into_iter().collect(),
                timeout_ms: Some(1_000),
            },
        }
    }

    fn evidence(
        workspace: &Path,
        id: &str,
        resource: &str,
    ) -> Result<GoalEvidence, DurableTaskError> {
        Ok(GoalEvidence {
            id: id.to_string(),
            kind: "verification".to_string(),
            summary: "the observed workspace state satisfies this goal".to_string(),
            resources: vec![resource_version(workspace, resource)?],
            revision: 0,
            valid: false,
        })
    }

    fn execute_write(
        task: &mut DurableTask,
        goal: &GoalId,
        file: &str,
        contents: &str,
    ) -> Result<(), DurableTaskError> {
        let call = task.request_tool(
            goal,
            descriptor(
                "filesystem__atomic_write",
                AccessMode::Write,
                RetrySemantics::Never,
                [PathBuf::from(file)],
            ),
            format!("write {file}"),
        )?;
        let workspace = task
            .state()
            .workspace
            .root
            .clone()
            .expect("test task has a workspace");
        task.execute_tool(&call, move |_| {
            fs::create_dir_all(workspace.join("workspace")).map_err(|error| error.to_string())?;
            fs::write(workspace.join(file), contents).map_err(|error| error.to_string())?;
            Ok(ToolExecutionResult {
                summary: format!("wrote {file}"),
                evidence: None,
                changed_artifacts: vec![PathBuf::from(file)],
            })
        })?;
        Ok(())
    }

    #[test]
    fn multi_step_coding_task_replans_after_a_real_validation_failure() {
        let temporary = tempfile::tempdir().unwrap();
        fs::create_dir(temporary.path().join("workspace")).unwrap();
        fs::write(temporary.path().join("workspace/source.txt"), "broken").unwrap();
        let store = TaskStore::new(temporary.path().join("tasks")).unwrap();
        let mut task = store
            .create_task(
                "repair the local project and verify its output",
                Some(temporary.path().to_path_buf()),
                TaskLimits::default(),
            )
            .unwrap();
        let root = task.runtime().root_goal.clone();

        let inspect = task
            .add_subtask(
                &root,
                "inspect the existing project",
                BTreeSet::new(),
                BTreeSet::new(),
                GoalBudget::default(),
            )
            .unwrap();
        task.start_goal(&inspect).unwrap();
        let call = task
            .request_tool(
                &inspect,
                descriptor(
                    "filesystem__read",
                    AccessMode::Read,
                    RetrySemantics::Bounded,
                    [],
                ),
                "inspect source.txt",
            )
            .unwrap();
        task.execute_tool(&call, |_| {
            Ok(ToolExecutionResult {
                summary: "inspected the broken source".to_string(),
                evidence: Some(
                    evidence(temporary.path(), "inspection", "workspace/source.txt").unwrap(),
                ),
                changed_artifacts: Vec::new(),
            })
        })
        .unwrap();
        task.complete_goal(&inspect).unwrap();

        let change = task
            .add_subtask(
                &root,
                "apply the initial repair",
                [inspect.clone()].into_iter().collect(),
                BTreeSet::new(),
                GoalBudget::default(),
            )
            .unwrap();
        task.start_goal(&change).unwrap();
        execute_write(&mut task, &change, "workspace/source.txt", "still broken").unwrap();
        execute_write(&mut task, &change, "workspace/config.txt", "enabled=true").unwrap();
        task.add_evidence(
            &change,
            GoalEvidence {
                id: "initial-change".to_string(),
                kind: "workspace_change".to_string(),
                summary: "the initial source and configuration changes were recorded".to_string(),
                resources: vec![
                    resource_version(temporary.path(), "workspace/source.txt").unwrap(),
                    resource_version(temporary.path(), "workspace/config.txt").unwrap(),
                ],
                revision: 0,
                valid: false,
            },
        )
        .unwrap();
        task.complete_goal(&change).unwrap();

        let validation = task
            .add_subtask(
                &root,
                "run the focused validation",
                [change.clone()].into_iter().collect(),
                BTreeSet::new(),
                GoalBudget::default(),
            )
            .unwrap();
        task.start_goal(&validation).unwrap();
        let call = task
            .request_tool(
                &validation,
                descriptor("coding__test", AccessMode::Read, RetrySemantics::Never, []),
                "run focused validation",
            )
            .unwrap();
        assert!(matches!(
            task.execute_tool(
                &call,
                |_| Err("expected output is still broken".to_string())
            ),
            Err(DurableTaskError::ToolExecution(_))
        ));
        let repair = task
            .replan(
                &validation,
                "repair the diagnosed source",
                "focused validation showed that the initial change did not fix the output",
            )
            .unwrap();
        task.start_goal(&repair).unwrap();
        execute_write(&mut task, &repair, "workspace/source.txt", "fixed").unwrap();
        task.add_evidence(
            &repair,
            evidence(temporary.path(), "repair", "workspace/source.txt").unwrap(),
        )
        .unwrap();
        task.complete_goal(&repair).unwrap();

        let verify = task
            .add_subtask(
                &root,
                "verify the repaired output",
                [repair.clone()].into_iter().collect(),
                BTreeSet::new(),
                GoalBudget::default(),
            )
            .unwrap();
        task.start_goal(&verify).unwrap();
        let call = task
            .request_tool(
                &verify,
                descriptor(
                    "coding__test",
                    AccessMode::Read,
                    RetrySemantics::Bounded,
                    [],
                ),
                "run repaired validation",
            )
            .unwrap();
        task.execute_tool(&call, |_| {
            Ok(ToolExecutionResult {
                summary: "focused validation passed".to_string(),
                evidence: Some(
                    evidence(temporary.path(), "verification", "workspace/source.txt").unwrap(),
                ),
                changed_artifacts: Vec::new(),
            })
        })
        .unwrap();
        task.complete_goal(&verify).unwrap();
        task.add_evidence(
            &root,
            evidence(temporary.path(), "review", "workspace/source.txt").unwrap(),
        )
        .unwrap();
        task.complete_goal(&root).unwrap();
        task.complete("focused validation passed after repair")
            .unwrap();

        assert_eq!(task.runtime().status, TaskStatus::Completed);
        assert_eq!(
            fs::read_to_string(temporary.path().join("workspace/source.txt")).unwrap(),
            "fixed"
        );
        assert!(task
            .events()
            .unwrap()
            .iter()
            .any(|event| matches!(event.event, TaskEventKind::Replanned { .. })));
        task.replay_validate().unwrap();
    }

    #[test]
    fn interrupted_task_reloads_without_repeating_a_confirmed_mutation() {
        let temporary = tempfile::tempdir().unwrap();
        fs::create_dir(temporary.path().join("workspace")).unwrap();
        let store = TaskStore::new(temporary.path().join("tasks")).unwrap();
        let mut task = store
            .create_task(
                "write an output file and resume verification later",
                Some(temporary.path().to_path_buf()),
                TaskLimits::default(),
            )
            .unwrap();
        let root = task.runtime().root_goal.clone();
        execute_write(&mut task, &root, "workspace/output.txt", "written once").unwrap();
        let id = task.id().as_str().to_string();
        drop(task);

        let mut resumed = store.load(&id).unwrap();
        let report = resumed.resume().unwrap();
        assert_eq!(report.invalidated_evidence, 0);
        assert_eq!(resumed.runtime().actions, 1);
        assert_eq!(
            resumed.state().tool_calls.values().next().unwrap().attempts,
            1
        );
        assert_eq!(
            fs::read_to_string(temporary.path().join("workspace/output.txt")).unwrap(),
            "written once"
        );
        resumed
            .add_evidence(
                &root,
                evidence(temporary.path(), "output", "workspace/output.txt").unwrap(),
            )
            .unwrap();
        resumed.complete_goal(&root).unwrap();
        resumed
            .complete("output verified after a new runtime instance")
            .unwrap();
        resumed.replay_validate().unwrap();
    }

    #[test]
    fn external_mutation_invalidates_only_related_evidence_and_replans() {
        let temporary = tempfile::tempdir().unwrap();
        fs::create_dir(temporary.path().join("workspace")).unwrap();
        let store = TaskStore::new(temporary.path().join("tasks")).unwrap();
        let mut task = store
            .create_task(
                "maintain independent workspace artifacts",
                Some(temporary.path().to_path_buf()),
                TaskLimits::default(),
            )
            .unwrap();
        let root = task.runtime().root_goal.clone();
        let first = task
            .add_subtask(
                &root,
                "verify first artifact",
                BTreeSet::new(),
                BTreeSet::new(),
                GoalBudget::default(),
            )
            .unwrap();
        let second = task
            .add_subtask(
                &root,
                "verify second artifact",
                BTreeSet::new(),
                BTreeSet::new(),
                GoalBudget::default(),
            )
            .unwrap();
        task.start_goal(&first).unwrap();
        execute_write(&mut task, &first, "workspace/first.txt", "first-v1").unwrap();
        task.add_evidence(
            &first,
            evidence(temporary.path(), "first-evidence", "workspace/first.txt").unwrap(),
        )
        .unwrap();
        task.complete_goal(&first).unwrap();
        task.start_goal(&second).unwrap();
        execute_write(&mut task, &second, "workspace/second.txt", "second-v1").unwrap();
        task.add_evidence(
            &second,
            evidence(temporary.path(), "second-evidence", "workspace/second.txt").unwrap(),
        )
        .unwrap();
        task.complete_goal(&second).unwrap();
        task.pause().unwrap();
        let id = task.id().as_str().to_string();
        drop(task);

        fs::write(temporary.path().join("workspace/first.txt"), "first-v2").unwrap();
        let mut resumed = store.load(&id).unwrap();
        let report = resumed.resume().unwrap();
        assert_eq!(report.invalidated_evidence, 1);
        assert_eq!(report.replanned_goals.len(), 1);
        assert_eq!(resumed.runtime().goals[&first].status, GoalStatus::Obsolete);
        assert_eq!(
            resumed.runtime().goals[&second].status,
            GoalStatus::Completed
        );
        assert!(resumed.runtime().goals[&second].evidence[0].valid);
        let replacement = report.replanned_goals[0].clone();
        resumed.start_goal(&replacement).unwrap();
        resumed
            .add_evidence(
                &replacement,
                evidence(temporary.path(), "first-revalidated", "workspace/first.txt").unwrap(),
            )
            .unwrap();
        resumed.complete_goal(&replacement).unwrap();
        resumed
            .add_evidence(
                &root,
                evidence(temporary.path(), "review", "workspace/first.txt").unwrap(),
            )
            .unwrap();
        resumed.complete_goal(&root).unwrap();
        resumed
            .complete("only the changed artifact was revalidated")
            .unwrap();
        resumed.replay_validate().unwrap();
    }

    #[test]
    fn interrupted_non_retry_safe_mutation_requires_user_input() {
        let temporary = tempfile::tempdir().unwrap();
        fs::create_dir(temporary.path().join("workspace")).unwrap();
        let store = TaskStore::new(temporary.path().join("tasks")).unwrap();
        let mut task = store
            .create_task(
                "perform a non-retry-safe deployment",
                Some(temporary.path().to_path_buf()),
                TaskLimits::default(),
            )
            .unwrap();
        let root = task.runtime().root_goal.clone();
        let call = task
            .request_tool(
                &root,
                descriptor(
                    "deployment__publish",
                    AccessMode::Write,
                    RetrySemantics::Never,
                    [PathBuf::from("workspace/release.txt")],
                ),
                "publish release",
            )
            .unwrap();
        task.mark_tool_started(&call).unwrap();
        let id = task.id().as_str().to_string();
        drop(task);

        let mut resumed = store.load(&id).unwrap();
        let report = resumed.resume().unwrap();
        assert_eq!(report.requires_user_input, vec![call]);
        assert_eq!(resumed.runtime().status, TaskStatus::Waiting);
        assert_eq!(
            resumed.state().tool_calls.values().next().unwrap().status,
            DurableToolStatus::UnknownOutcome
        );
        resumed.replay_validate().unwrap();
    }

    #[test]
    fn atomic_state_rejects_a_truncated_checkpoint() {
        let temporary = tempfile::tempdir().unwrap();
        let store = TaskStore::new(temporary.path().join("tasks")).unwrap();
        let task = store
            .create_task("keep checkpoints atomic", None, TaskLimits::default())
            .unwrap();
        let id = task.id().as_str().to_string();
        fs::write(task.state_path(), "{\"schema_version\":").unwrap();
        assert!(matches!(store.load(&id), Err(DurableTaskError::Json(_))));
    }

    #[test]
    fn retry_safe_read_tool_retries_after_a_timeout_and_keeps_a_causal_journal() {
        let temporary = tempfile::tempdir().unwrap();
        let store = TaskStore::new(temporary.path().join("tasks")).unwrap();
        let mut task = store
            .create_task("retry a bounded observation", None, TaskLimits::default())
            .unwrap();
        let root = task.runtime().root_goal.clone();
        let call = task
            .request_tool(
                &root,
                descriptor(
                    "system__bounded_observation",
                    AccessMode::Read,
                    RetrySemantics::Bounded,
                    [],
                ),
                "observe the system with a deadline",
            )
            .unwrap();
        task.tool_call_mut(&call)
            .unwrap()
            .descriptor
            .execution
            .timeout_ms = Some(1);
        let mut attempts = 0;
        task.execute_tool(&call, |_| {
            attempts += 1;
            if attempts == 1 {
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            Ok(ToolExecutionResult::succeeded("observation completed"))
        })
        .unwrap();
        assert_eq!(attempts, 2);
        assert_eq!(task.runtime().actions, 2);
        assert_eq!(
            task.state().tool_calls[&call].status,
            DurableToolStatus::Succeeded
        );
        task.replay_validate().unwrap();
    }

    #[test]
    fn version_zero_state_migrates_without_losing_the_runtime() {
        let temporary = tempfile::tempdir().unwrap();
        let store = TaskStore::new(temporary.path().join("tasks")).unwrap();
        let task = store
            .create_task("migrate durable state", None, TaskLimits::default())
            .unwrap();
        let id = task.id().as_str().to_string();
        let state = task.state().clone();
        let legacy = serde_json::json!({
            "schema_version": 0,
            "runtime": state.runtime,
            "workspace": state.workspace,
            "tool_calls": state.tool_calls,
            "artifacts": state.artifacts,
        });
        fs::write(task.state_path(), serde_json::to_vec(&legacy).unwrap()).unwrap();
        let migrated = store.load(&id).unwrap();
        assert_eq!(migrated.state().schema_version, DURABLE_TASK_SCHEMA_VERSION);
        assert_eq!(migrated.id().as_str(), id);
    }

    #[test]
    fn user_steering_persists_a_constraint_and_replans_only_the_affected_goal() {
        let temporary = tempfile::tempdir().unwrap();
        let store = TaskStore::new(temporary.path().join("tasks")).unwrap();
        let mut task = store
            .create_task(
                "keep two goals independently reviewable",
                None,
                TaskLimits::default(),
            )
            .unwrap();
        let root = task.runtime().root_goal.clone();
        let affected = task
            .add_subtask(
                &root,
                "prepare deployment guidance",
                BTreeSet::new(),
                BTreeSet::new(),
                GoalBudget::default(),
            )
            .unwrap();
        let unaffected = task
            .add_subtask(
                &root,
                "retain the unrelated investigation",
                BTreeSet::new(),
                BTreeSet::new(),
                GoalBudget::default(),
            )
            .unwrap();
        task.start_goal(&affected).unwrap();
        task.add_evidence(
            &affected,
            GoalEvidence {
                id: "deployment-guidance".to_string(),
                kind: "review".to_string(),
                summary: "initial deployment guidance was checked".to_string(),
                resources: Vec::new(),
                revision: 0,
                valid: false,
            },
        )
        .unwrap();
        task.complete_goal(&affected).unwrap();

        let replacements = task
            .steer(
                "do not deploy to production without an explicit approval",
                std::slice::from_ref(&affected),
            )
            .unwrap();
        assert_eq!(replacements.len(), 1);
        assert_eq!(task.runtime().goals[&affected].status, GoalStatus::Obsolete);
        assert_eq!(
            task.runtime().goals[&unaffected].status,
            GoalStatus::Pending
        );
        assert!(task
            .runtime()
            .memory
            .normalized_constraints
            .contains(&"do not deploy to production without an explicit approval".to_string()));
        assert!(task
            .events()
            .unwrap()
            .iter()
            .any(|event| matches!(event.event, TaskEventKind::UserSteering { .. })));
    }
}
