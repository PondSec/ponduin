//! Provider-independent runtime state for bounded, resumable local tasks.
//!
//! The runtime records facts produced by tools and hosts. Models may propose a
//! plan, but cannot mark goals, capabilities, or verification as complete.

use crate::coding::sensitive::is_sensitive_path;
use crate::coding::workspace::{CodingWorkspace, WorkspaceError};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use uuid::Uuid;

const MAX_TEXT_BYTES: usize = 16 * 1_024;
const MAX_GOALS: usize = 200;
const MAX_MEMORY_ITEMS: usize = 200;
const MAX_EVENTS: usize = 500;
const MAX_EVIDENCE_PER_GOAL: usize = 100;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TaskId(String);

impl TaskId {
    fn new() -> Self {
        Self(format!("task_{}", Uuid::now_v7()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct GoalId(String);

impl GoalId {
    fn new() -> Self {
        Self(format!("goal_{}", Uuid::now_v7()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Running,
    Paused,
    Waiting,
    Blocked,
    Completed,
    Failed,
    Cancelled,
}

impl TaskStatus {
    fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Blocked | Self::Completed | Self::Failed | Self::Cancelled
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalStatus {
    Pending,
    Running,
    Waiting,
    Blocked,
    Completed,
    Failed,
    Obsolete,
}

impl GoalStatus {
    fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Blocked | Self::Completed | Self::Failed | Self::Obsolete
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskDomain {
    Coding,
    Filesystem,
    SystemInspection,
    Git,
    Document,
    WebResearch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessMode {
    Read,
    Write,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskLevel {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CostClass {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LatencyClass {
    Immediate,
    Short,
    Long,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetrySemantics {
    Never,
    AfterRefresh,
    Bounded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkRequirement {
    None,
    Optional,
    Required,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceRequirement {
    None,
    Readable,
    Writable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SideEffect {
    None,
    ScopedWorkspaceWrite,
    GitMutation,
    ExternalWrite,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolDescriptor {
    pub name: String,
    pub domain: TaskDomain,
    pub access: AccessMode,
    pub risk: RiskLevel,
    pub preconditions: Vec<String>,
    pub side_effect: SideEffect,
    pub required_capability: Option<String>,
    pub produced_evidence: Vec<String>,
    pub cost: CostClass,
    pub latency: LatencyClass,
    pub network: NetworkRequirement,
    pub workspace: WorkspaceRequirement,
    pub retry: RetrySemantics,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillBundle {
    pub id: String,
    pub purpose: String,
    pub domains: BTreeSet<TaskDomain>,
    pub capabilities: BTreeSet<String>,
    pub guidance: Vec<String>,
    pub validation_patterns: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolRegistry {
    tools: BTreeMap<String, ToolDescriptor>,
    skills: BTreeMap<String, SkillBundle>,
}

impl ToolRegistry {
    pub fn register_tool(&mut self, tool: ToolDescriptor) -> Result<(), TaskRuntimeError> {
        validate_text("tool name", &tool.name)?;
        if self.tools.contains_key(&tool.name) {
            return Err(TaskRuntimeError::DuplicateTool(tool.name));
        }
        self.tools.insert(tool.name.clone(), tool);
        Ok(())
    }

    pub fn register_skill(&mut self, skill: SkillBundle) -> Result<(), TaskRuntimeError> {
        validate_text("skill id", &skill.id)?;
        validate_text("skill purpose", &skill.purpose)?;
        if skill.domains.is_empty() {
            return Err(TaskRuntimeError::InvalidSkill(skill.id));
        }
        if self.skills.contains_key(&skill.id) {
            return Err(TaskRuntimeError::DuplicateSkill(skill.id));
        }
        self.skills.insert(skill.id.clone(), skill);
        Ok(())
    }

    pub fn tool(&self, name: &str) -> Option<&ToolDescriptor> {
        self.tools.get(name)
    }

    pub fn skill(&self, id: &str) -> Option<&SkillBundle> {
        self.skills.get(id)
    }

    pub fn disclose(&self, request: &ToolDisclosureRequest) -> Vec<ToolDescriptor> {
        self.tools
            .values()
            .filter(|tool| request.domains.contains(&tool.domain))
            .filter(|tool| request.allow_writes || tool.access == AccessMode::Read)
            .filter(|tool| request.allow_network || tool.network != NetworkRequirement::Required)
            .filter(|tool| request.workspace_satisfies(tool.workspace))
            .filter(|tool| tool.risk <= request.maximum_risk)
            .filter(|tool| {
                tool.required_capability
                    .as_ref()
                    .is_none_or(|capability| request.available_capabilities.contains(capability))
            })
            .cloned()
            .collect()
    }

    pub fn expand_for_skill(
        &self,
        skill_id: &str,
        request: &ToolDisclosureRequest,
    ) -> Result<Vec<ToolDescriptor>, TaskRuntimeError> {
        let skill = self
            .skill(skill_id)
            .ok_or_else(|| TaskRuntimeError::UnknownSkill(skill_id.to_string()))?;
        let mut expanded = request.clone();
        expanded.domains.extend(skill.domains.iter().copied());
        expanded
            .available_capabilities
            .extend(skill.capabilities.iter().cloned());
        Ok(self.disclose(&expanded))
    }

    pub fn next_actions(&self, request: &ToolDisclosureRequest) -> Vec<ToolDescriptor> {
        let mut tools = self.disclose(request);
        tools.sort_by_key(|tool| {
            (
                cost_rank(tool.cost),
                latency_rank(tool.latency),
                risk_rank(tool.risk),
                tool.name.clone(),
            )
        });
        tools
    }

    pub fn builtin() -> Self {
        let mut registry = Self::default();
        for tool in [
            tool(
                "coding__repository_profile",
                TaskDomain::Coding,
                AccessMode::Read,
                RiskLevel::Low,
                ToolProperties {
                    side_effect: SideEffect::None,
                    evidence: &["repository_profile"],
                    cost: CostClass::Low,
                    latency: LatencyClass::Short,
                    network: NetworkRequirement::None,
                    workspace: WorkspaceRequirement::Readable,
                    retry: RetrySemantics::AfterRefresh,
                },
            ),
            tool(
                "coding__run_process",
                TaskDomain::Coding,
                AccessMode::Write,
                RiskLevel::Medium,
                ToolProperties {
                    side_effect: SideEffect::ScopedWorkspaceWrite,
                    evidence: &["process_output"],
                    cost: CostClass::Medium,
                    latency: LatencyClass::Long,
                    network: NetworkRequirement::Optional,
                    workspace: WorkspaceRequirement::Writable,
                    retry: RetrySemantics::Bounded,
                },
            ),
            tool(
                "coding__git_status",
                TaskDomain::Git,
                AccessMode::Read,
                RiskLevel::Low,
                ToolProperties {
                    side_effect: SideEffect::None,
                    evidence: &["git_status"],
                    cost: CostClass::Low,
                    latency: LatencyClass::Immediate,
                    network: NetworkRequirement::None,
                    workspace: WorkspaceRequirement::Readable,
                    retry: RetrySemantics::AfterRefresh,
                },
            ),
            tool(
                "filesystem__list",
                TaskDomain::Filesystem,
                AccessMode::Read,
                RiskLevel::Low,
                ToolProperties {
                    side_effect: SideEffect::None,
                    evidence: &["directory_listing"],
                    cost: CostClass::Low,
                    latency: LatencyClass::Immediate,
                    network: NetworkRequirement::None,
                    workspace: WorkspaceRequirement::Readable,
                    retry: RetrySemantics::AfterRefresh,
                },
            ),
            tool(
                "filesystem__find",
                TaskDomain::Filesystem,
                AccessMode::Read,
                RiskLevel::Low,
                ToolProperties {
                    side_effect: SideEffect::None,
                    evidence: &["file_inventory"],
                    cost: CostClass::Medium,
                    latency: LatencyClass::Short,
                    network: NetworkRequirement::None,
                    workspace: WorkspaceRequirement::Readable,
                    retry: RetrySemantics::AfterRefresh,
                },
            ),
            tool(
                "filesystem__copy",
                TaskDomain::Filesystem,
                AccessMode::Write,
                RiskLevel::Medium,
                ToolProperties {
                    side_effect: SideEffect::ScopedWorkspaceWrite,
                    evidence: &["filesystem_mutation"],
                    cost: CostClass::Low,
                    latency: LatencyClass::Immediate,
                    network: NetworkRequirement::None,
                    workspace: WorkspaceRequirement::Writable,
                    retry: RetrySemantics::Never,
                },
            ),
            tool(
                "filesystem__move",
                TaskDomain::Filesystem,
                AccessMode::Write,
                RiskLevel::Medium,
                ToolProperties {
                    side_effect: SideEffect::ScopedWorkspaceWrite,
                    evidence: &["filesystem_mutation"],
                    cost: CostClass::Low,
                    latency: LatencyClass::Immediate,
                    network: NetworkRequirement::None,
                    workspace: WorkspaceRequirement::Writable,
                    retry: RetrySemantics::Never,
                },
            ),
            tool(
                "system__inspect",
                TaskDomain::SystemInspection,
                AccessMode::Read,
                RiskLevel::Low,
                ToolProperties {
                    side_effect: SideEffect::None,
                    evidence: &["system_facts"],
                    cost: CostClass::Low,
                    latency: LatencyClass::Immediate,
                    network: NetworkRequirement::None,
                    workspace: WorkspaceRequirement::None,
                    retry: RetrySemantics::AfterRefresh,
                },
            ),
            tool(
                "document__inspect",
                TaskDomain::Document,
                AccessMode::Read,
                RiskLevel::Low,
                ToolProperties {
                    side_effect: SideEffect::None,
                    evidence: &["document_metadata"],
                    cost: CostClass::Low,
                    latency: LatencyClass::Short,
                    network: NetworkRequirement::None,
                    workspace: WorkspaceRequirement::Readable,
                    retry: RetrySemantics::AfterRefresh,
                },
            ),
        ] {
            registry
                .register_tool(tool)
                .expect("built-in tools are valid");
        }
        for skill in [
            skill(
                "coding",
                "Inspect, modify, validate, and review a repository.",
                [TaskDomain::Coding, TaskDomain::Git],
                ["git"],
                ["targeted validation before broad validation"],
            ),
            skill(
                "filesystem",
                "Inspect and safely organize files inside the task workspace.",
                [TaskDomain::Filesystem],
                [],
                ["verify the requested end state"],
            ),
            skill(
                "system-inspection",
                "Collect read-only local system facts.",
                [TaskDomain::SystemInspection],
                [],
                ["report observed facts only"],
            ),
            skill(
                "git",
                "Inspect a repository state and use owned Git mutations.",
                [TaskDomain::Git],
                ["git"],
                ["inspect status and diff before mutation"],
            ),
            skill(
                "document-processing",
                "Inspect structured local text documents.",
                [TaskDomain::Document],
                [],
                ["validate output structure"],
            ),
        ] {
            registry
                .register_skill(skill)
                .expect("built-in skills are valid");
        }
        registry
    }
}

fn cost_rank(cost: CostClass) -> u8 {
    match cost {
        CostClass::Low => 0,
        CostClass::Medium => 1,
        CostClass::High => 2,
    }
}

fn latency_rank(latency: LatencyClass) -> u8 {
    match latency {
        LatencyClass::Immediate => 0,
        LatencyClass::Short => 1,
        LatencyClass::Long => 2,
    }
}

fn risk_rank(risk: RiskLevel) -> u8 {
    match risk {
        RiskLevel::Low => 0,
        RiskLevel::Medium => 1,
        RiskLevel::High => 2,
    }
}

struct ToolProperties {
    side_effect: SideEffect,
    evidence: &'static [&'static str],
    cost: CostClass,
    latency: LatencyClass,
    network: NetworkRequirement,
    workspace: WorkspaceRequirement,
    retry: RetrySemantics,
}

fn tool(
    name: &str,
    domain: TaskDomain,
    access: AccessMode,
    risk: RiskLevel,
    properties: ToolProperties,
) -> ToolDescriptor {
    ToolDescriptor {
        name: name.to_string(),
        domain,
        access,
        risk,
        preconditions: Vec::new(),
        side_effect: properties.side_effect,
        required_capability: None,
        produced_evidence: properties
            .evidence
            .iter()
            .map(ToString::to_string)
            .collect(),
        cost: properties.cost,
        latency: properties.latency,
        network: properties.network,
        workspace: properties.workspace,
        retry: properties.retry,
    }
}

fn skill(
    id: &str,
    purpose: &str,
    domains: impl IntoIterator<Item = TaskDomain>,
    capabilities: impl IntoIterator<Item = &'static str>,
    validation_patterns: impl IntoIterator<Item = &'static str>,
) -> SkillBundle {
    SkillBundle {
        id: id.to_string(),
        purpose: purpose.to_string(),
        domains: domains.into_iter().collect(),
        capabilities: capabilities.into_iter().map(str::to_string).collect(),
        guidance: Vec::new(),
        validation_patterns: validation_patterns
            .into_iter()
            .map(str::to_string)
            .collect(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolDisclosureRequest {
    pub domains: BTreeSet<TaskDomain>,
    pub allow_writes: bool,
    pub allow_network: bool,
    pub readable_workspace: bool,
    pub writable_workspace: bool,
    pub maximum_risk: RiskLevel,
    pub available_capabilities: BTreeSet<String>,
}

impl ToolDisclosureRequest {
    fn workspace_satisfies(&self, requirement: WorkspaceRequirement) -> bool {
        match requirement {
            WorkspaceRequirement::None => true,
            WorkspaceRequirement::Readable => self.readable_workspace,
            WorkspaceRequirement::Writable => self.writable_workspace,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityObservation {
    pub name: String,
    pub available: bool,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityDiscovery {
    pub workspace_readable: bool,
    pub workspace_writable: bool,
    pub executables: Vec<CapabilityObservation>,
}

impl CapabilityDiscovery {
    pub fn probe(workspace: Option<&Path>, executables: &[&str]) -> Self {
        let (workspace_readable, workspace_writable) = workspace
            .and_then(|path| fs::metadata(path).ok())
            .filter(|metadata| metadata.is_dir())
            .map_or((false, false), |metadata| {
                (true, !metadata.permissions().readonly())
            });
        let executables = executables
            .iter()
            .map(|name| match which::which(name) {
                Ok(path) => CapabilityObservation {
                    name: (*name).to_string(),
                    available: true,
                    detail: path.display().to_string(),
                },
                Err(_) => CapabilityObservation {
                    name: (*name).to_string(),
                    available: false,
                    detail: "not found on PATH".to_string(),
                },
            })
            .collect();
        Self {
            workspace_readable,
            workspace_writable,
            executables,
        }
    }

    pub fn available_capabilities(&self) -> BTreeSet<String> {
        self.executables
            .iter()
            .filter(|observation| observation.available)
            .map(|observation| observation.name.clone())
            .collect()
    }

    pub fn changed_capabilities(&self, previous: &Self) -> BTreeSet<String> {
        let previous = previous
            .executables
            .iter()
            .map(|observation| (observation.name.as_str(), observation.available))
            .collect::<BTreeMap<_, _>>();
        self.executables
            .iter()
            .filter(|observation| {
                previous.get(observation.name.as_str()) != Some(&observation.available)
            })
            .map(|observation| observation.name.clone())
            .collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskLimits {
    pub max_actions: u32,
    pub max_replans: u32,
    pub max_repair_attempts: u32,
    pub max_repeated_failures: u32,
    pub max_process_time_ms: u64,
    pub max_total_duration_ms: u64,
    pub max_context_bytes: usize,
    pub max_tool_errors: u32,
}

impl Default for TaskLimits {
    fn default() -> Self {
        Self {
            max_actions: 100,
            max_replans: 10,
            max_repair_attempts: 5,
            max_repeated_failures: 3,
            max_process_time_ms: 120_000,
            max_total_duration_ms: 3_600_000,
            max_context_bytes: 128 * 1_024,
            max_tool_errors: 10,
        }
    }
}

impl TaskLimits {
    fn validate(self) -> Result<(), TaskRuntimeError> {
        if self.max_actions == 0
            || self.max_replans == 0
            || self.max_repeated_failures == 0
            || self.max_process_time_ms == 0
            || self.max_total_duration_ms == 0
            || self.max_context_bytes == 0
        {
            return Err(TaskRuntimeError::InvalidLimits);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalBudget {
    pub max_actions: u32,
}

impl Default for GoalBudget {
    fn default() -> Self {
        Self { max_actions: 20 }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskGoal {
    pub id: GoalId,
    pub parent: Option<GoalId>,
    pub title: String,
    pub status: GoalStatus,
    pub dependencies: BTreeSet<GoalId>,
    pub required_capabilities: BTreeSet<String>,
    pub budget: GoalBudget,
    pub actions: u32,
    pub evidence: Vec<GoalEvidence>,
    pub blocked_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalEvidence {
    pub id: String,
    pub kind: String,
    pub summary: String,
    pub resources: Vec<ResourceVersion>,
    pub revision: u32,
    pub valid: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ResourceVersion {
    pub resource: PathBuf,
    pub fingerprint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NeedUserInput {
    pub required_information: String,
    pub reason: String,
    pub blocked_goal: GoalId,
    pub allowed_options: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionOutcome {
    Succeeded,
    Failed,
    Blocked,
    DeniedByPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionRecord {
    pub tool: String,
    pub summary: String,
    pub outcome: ActionOutcome,
    pub failure_fingerprint: Option<String>,
    pub process_time_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskMemory {
    pub original_goal: String,
    pub assumptions: Vec<String>,
    pub open_questions: Vec<String>,
    pub completed_goals: Vec<GoalId>,
    pub known_failures: Vec<String>,
    pub failed_strategies: Vec<String>,
    pub relevant_resources: Vec<PathBuf>,
    pub unavailable_capabilities: Vec<String>,
}

impl TaskMemory {
    fn new(original_goal: String) -> Self {
        Self {
            original_goal,
            assumptions: Vec::new(),
            open_questions: Vec::new(),
            completed_goals: Vec::new(),
            known_failures: Vec::new(),
            failed_strategies: Vec::new(),
            relevant_resources: Vec::new(),
            unavailable_capabilities: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum TaskEventKind {
    TaskStarted,
    GoalAdded {
        goal: GoalId,
    },
    GoalStarted {
        goal: GoalId,
    },
    ActionRecorded {
        goal: GoalId,
        outcome: ActionOutcome,
    },
    EvidenceAdded {
        goal: GoalId,
        evidence: String,
    },
    EvidenceInvalidated {
        resource: PathBuf,
    },
    Replanned {
        previous_goal: GoalId,
        replacement_goal: GoalId,
        reason: String,
    },
    GoalCompleted {
        goal: GoalId,
    },
    TaskPaused,
    TaskResumed,
    WaitingForUserInput {
        goal: GoalId,
    },
    TaskBlocked {
        reason: String,
    },
    TaskCompleted,
    TaskFailed {
        reason: String,
    },
    TaskCancelled {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskEvent {
    pub sequence: u64,
    pub event: TaskEventKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskCheckpoint {
    pub schema_version: u32,
    pub runtime: TaskRuntime,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskRuntime {
    pub id: TaskId,
    pub status: TaskStatus,
    pub workspace: Option<PathBuf>,
    pub limits: TaskLimits,
    pub root_goal: GoalId,
    pub goals: BTreeMap<GoalId, TaskGoal>,
    pub revision: u32,
    pub actions: u32,
    pub replans: u32,
    pub repair_attempts: u32,
    pub tool_errors: u32,
    pub total_process_time_ms: u64,
    pub observed_total_duration_ms: Option<u64>,
    pub memory: TaskMemory,
    pub need_user_input: Option<NeedUserInput>,
    events: VecDeque<TaskEvent>,
    failure_counts: BTreeMap<String, u32>,
    next_event_sequence: u64,
    refreshed_since_pause: bool,
}

impl TaskRuntime {
    pub fn new(
        original_goal: impl Into<String>,
        workspace: Option<PathBuf>,
        limits: TaskLimits,
    ) -> Result<Self, TaskRuntimeError> {
        limits.validate()?;
        let original_goal = original_goal.into();
        validate_text("original goal", &original_goal)?;
        let root_goal = GoalId::new();
        let root = TaskGoal {
            id: root_goal.clone(),
            parent: None,
            title: original_goal.clone(),
            status: GoalStatus::Running,
            dependencies: BTreeSet::new(),
            required_capabilities: BTreeSet::new(),
            budget: GoalBudget {
                max_actions: limits.max_actions,
            },
            actions: 0,
            evidence: Vec::new(),
            blocked_reason: None,
        };
        let mut goals = BTreeMap::new();
        goals.insert(root_goal.clone(), root);
        let mut runtime = Self {
            id: TaskId::new(),
            status: TaskStatus::Running,
            workspace,
            limits,
            root_goal,
            goals,
            revision: 0,
            actions: 0,
            replans: 0,
            repair_attempts: 0,
            tool_errors: 0,
            total_process_time_ms: 0,
            observed_total_duration_ms: None,
            memory: TaskMemory::new(original_goal),
            need_user_input: None,
            events: VecDeque::new(),
            failure_counts: BTreeMap::new(),
            next_event_sequence: 0,
            refreshed_since_pause: false,
        };
        runtime.push_event(TaskEventKind::TaskStarted);
        Ok(runtime)
    }

    pub fn is_terminal(&self) -> bool {
        self.status.is_terminal()
    }

    pub fn events(&self) -> impl Iterator<Item = &TaskEvent> {
        self.events.iter()
    }

    pub fn add_subtask(
        &mut self,
        parent: &GoalId,
        title: impl Into<String>,
        dependencies: BTreeSet<GoalId>,
        required_capabilities: BTreeSet<String>,
        budget: GoalBudget,
    ) -> Result<GoalId, TaskRuntimeError> {
        self.ensure_active()?;
        if self.goals.len() >= MAX_GOALS || budget.max_actions == 0 {
            return Err(TaskRuntimeError::GoalLimit);
        }
        if !self.goals.contains_key(parent) {
            return Err(TaskRuntimeError::UnknownGoal(parent.clone()));
        }
        if dependencies
            .iter()
            .any(|dependency| !self.goals.contains_key(dependency))
        {
            return Err(TaskRuntimeError::UnknownDependency);
        }
        let title = title.into();
        validate_text("goal title", &title)?;
        let id = GoalId::new();
        self.goals.insert(
            id.clone(),
            TaskGoal {
                id: id.clone(),
                parent: Some(parent.clone()),
                title,
                status: GoalStatus::Pending,
                dependencies,
                required_capabilities,
                budget,
                actions: 0,
                evidence: Vec::new(),
                blocked_reason: None,
            },
        );
        self.push_event(TaskEventKind::GoalAdded { goal: id.clone() });
        Ok(id)
    }

    pub fn runnable_goals(&self, available_capabilities: &BTreeSet<String>) -> Vec<&TaskGoal> {
        self.goals
            .values()
            .filter(|goal| goal.status == GoalStatus::Pending)
            .filter(|goal| {
                goal.dependencies.iter().all(|dependency| {
                    self.goals
                        .get(dependency)
                        .is_some_and(|dependency| dependency.status == GoalStatus::Completed)
                })
            })
            .filter(|goal| {
                goal.required_capabilities
                    .iter()
                    .all(|capability| available_capabilities.contains(capability))
            })
            .collect()
    }

    pub fn start_goal(&mut self, goal: &GoalId) -> Result<(), TaskRuntimeError> {
        self.start_goal_with_capabilities(goal, &BTreeSet::new())
    }

    pub fn start_goal_with_capabilities(
        &mut self,
        goal: &GoalId,
        available_capabilities: &BTreeSet<String>,
    ) -> Result<(), TaskRuntimeError> {
        self.ensure_active()?;
        let goal_state = self
            .goals
            .get(goal)
            .ok_or_else(|| TaskRuntimeError::UnknownGoal(goal.clone()))?;
        let dependencies_satisfied = goal_state.dependencies.iter().all(|dependency| {
            self.goals
                .get(dependency)
                .is_some_and(|dependency| dependency.status == GoalStatus::Completed)
        });
        let capabilities_satisfied = goal_state
            .required_capabilities
            .iter()
            .all(|capability| available_capabilities.contains(capability));
        if !dependencies_satisfied || !capabilities_satisfied {
            return Err(TaskRuntimeError::GoalNotRunnable(goal.clone()));
        }
        let goal_state = self.goal_mut(goal)?;
        if goal_state.status != GoalStatus::Pending {
            return Err(TaskRuntimeError::InvalidGoalTransition {
                goal: goal.clone(),
                from: goal_state.status,
                to: GoalStatus::Running,
            });
        }
        goal_state.status = GoalStatus::Running;
        self.push_event(TaskEventKind::GoalStarted { goal: goal.clone() });
        Ok(())
    }

    pub fn record_action(
        &mut self,
        goal: &GoalId,
        action: ActionRecord,
    ) -> Result<(), TaskRuntimeError> {
        self.ensure_active()?;
        validate_text("tool", &action.tool)?;
        validate_text("action summary", &action.summary)?;
        if action
            .process_time_ms
            .is_some_and(|duration| duration > self.limits.max_process_time_ms)
        {
            self.block("process time budget exceeded".to_string());
            return Err(TaskRuntimeError::ProcessBudgetExceeded);
        }
        let next_process_time = self
            .total_process_time_ms
            .saturating_add(action.process_time_ms.unwrap_or_default());
        if next_process_time > self.limits.max_total_duration_ms {
            self.block("total task duration budget exceeded".to_string());
            return Err(TaskRuntimeError::TotalDurationBudgetExceeded);
        }
        let goal_state = self
            .goals
            .get(goal)
            .ok_or_else(|| TaskRuntimeError::UnknownGoal(goal.clone()))?;
        if goal_state.status != GoalStatus::Running {
            return Err(TaskRuntimeError::GoalNotRunning(goal.clone()));
        }
        if goal_state.actions >= goal_state.budget.max_actions {
            let title = goal_state.title.clone();
            self.block(format!("subtask action budget exceeded for {title}"));
            return Err(TaskRuntimeError::GoalActionBudgetExceeded(goal.clone()));
        }
        let goal_state = self.goal_mut(goal)?;
        goal_state.actions += 1;
        self.actions += 1;
        self.total_process_time_ms = next_process_time;
        if self.actions > self.limits.max_actions {
            self.block("task action budget exceeded".to_string());
            return Err(TaskRuntimeError::ActionBudgetExceeded);
        }
        if matches!(
            action.outcome,
            ActionOutcome::Failed | ActionOutcome::DeniedByPolicy
        ) {
            self.tool_errors += 1;
            if self.tool_errors > self.limits.max_tool_errors {
                self.block("tool error budget exceeded".to_string());
                return Err(TaskRuntimeError::ToolErrorBudgetExceeded);
            }
        }
        if let Some(fingerprint) = action.failure_fingerprint {
            validate_text("failure fingerprint", &fingerprint)?;
            let failures = self.failure_counts.entry(fingerprint.clone()).or_default();
            *failures += 1;
            if *failures > self.limits.max_repeated_failures {
                self.block(format!("repeated failure budget exceeded: {fingerprint}"));
                return Err(TaskRuntimeError::RepeatedFailureBudgetExceeded);
            }
            push_unique_bounded(&mut self.memory.known_failures, fingerprint);
        }
        self.push_event(TaskEventKind::ActionRecorded {
            goal: goal.clone(),
            outcome: action.outcome,
        });
        Ok(())
    }

    pub fn observe_total_duration(&mut self, elapsed_ms: u64) -> Result<(), TaskRuntimeError> {
        self.ensure_active()?;
        if elapsed_ms > self.limits.max_total_duration_ms {
            self.block("total task duration budget exceeded".to_string());
            return Err(TaskRuntimeError::TotalDurationBudgetExceeded);
        }
        self.observed_total_duration_ms = Some(elapsed_ms);
        Ok(())
    }

    pub fn add_evidence(
        &mut self,
        goal: &GoalId,
        mut evidence: GoalEvidence,
    ) -> Result<(), TaskRuntimeError> {
        self.ensure_active()?;
        validate_text("evidence id", &evidence.id)?;
        validate_text("evidence kind", &evidence.kind)?;
        validate_text("evidence summary", &evidence.summary)?;
        evidence.revision = self.revision;
        let goal_state = self.goal_mut(goal)?;
        if goal_state.evidence.len() >= MAX_EVIDENCE_PER_GOAL {
            return Err(TaskRuntimeError::EvidenceLimit(goal.clone()));
        }
        evidence.valid = true;
        let evidence_id = evidence.id.clone();
        goal_state.evidence.push(evidence);
        self.push_event(TaskEventKind::EvidenceAdded {
            goal: goal.clone(),
            evidence: evidence_id,
        });
        Ok(())
    }

    pub fn complete_goal(&mut self, goal: &GoalId) -> Result<(), TaskRuntimeError> {
        self.ensure_active()?;
        if self.goals.values().any(|candidate| {
            candidate.parent.as_ref() == Some(goal)
                && !matches!(
                    candidate.status,
                    GoalStatus::Completed | GoalStatus::Obsolete
                )
        }) {
            return Err(TaskRuntimeError::ChildGoalsOpen(goal.clone()));
        }
        let goal_state = self.goal_mut(goal)?;
        if goal_state.status != GoalStatus::Running {
            return Err(TaskRuntimeError::GoalNotRunning(goal.clone()));
        }
        if !goal_state.evidence.iter().any(|evidence| evidence.valid) {
            return Err(TaskRuntimeError::EvidenceRequired(goal.clone()));
        }
        goal_state.status = GoalStatus::Completed;
        if !self.memory.completed_goals.contains(goal) {
            self.memory.completed_goals.push(goal.clone());
        }
        self.push_event(TaskEventKind::GoalCompleted { goal: goal.clone() });
        Ok(())
    }

    pub fn replan(
        &mut self,
        obsolete_goal: &GoalId,
        replacement_title: impl Into<String>,
        reason: impl Into<String>,
    ) -> Result<GoalId, TaskRuntimeError> {
        self.ensure_active()?;
        if self.replans >= self.limits.max_replans {
            self.block("replan budget exceeded".to_string());
            return Err(TaskRuntimeError::ReplanBudgetExceeded);
        }
        let replacement_title = replacement_title.into();
        let reason = reason.into();
        validate_text("replacement goal", &replacement_title)?;
        validate_text("replan reason", &reason)?;
        let previous = self
            .goals
            .get(obsolete_goal)
            .cloned()
            .ok_or_else(|| TaskRuntimeError::UnknownGoal(obsolete_goal.clone()))?;
        if previous.status.is_terminal() {
            return Err(TaskRuntimeError::CannotReplanTerminalGoal(
                obsolete_goal.clone(),
            ));
        }
        let replacement_parent = previous.parent.unwrap_or_else(|| self.root_goal.clone());
        let replacement = self.add_subtask(
            &replacement_parent,
            replacement_title,
            previous.dependencies,
            previous.required_capabilities,
            previous.budget,
        )?;
        let previous_goal = self.goal_mut(obsolete_goal)?;
        previous_goal.status = GoalStatus::Obsolete;
        previous_goal.blocked_reason = Some(reason.clone());
        self.replans += 1;
        self.revision += 1;
        self.push_event(TaskEventKind::Replanned {
            previous_goal: obsolete_goal.clone(),
            replacement_goal: replacement.clone(),
            reason,
        });
        Ok(replacement)
    }

    pub fn mark_repair_attempt(
        &mut self,
        strategy: impl Into<String>,
    ) -> Result<(), TaskRuntimeError> {
        self.ensure_active()?;
        let strategy = strategy.into();
        validate_text("repair strategy", &strategy)?;
        self.repair_attempts += 1;
        if self.repair_attempts > self.limits.max_repair_attempts {
            self.block("repair attempt budget exceeded".to_string());
            return Err(TaskRuntimeError::RepairBudgetExceeded);
        }
        push_unique_bounded(&mut self.memory.failed_strategies, strategy);
        Ok(())
    }

    pub fn request_user_input(&mut self, request: NeedUserInput) -> Result<(), TaskRuntimeError> {
        self.ensure_active()?;
        validate_text("required information", &request.required_information)?;
        validate_text("input reason", &request.reason)?;
        let goal = self.goal_mut(&request.blocked_goal)?;
        if goal.status != GoalStatus::Running {
            return Err(TaskRuntimeError::GoalNotRunning(request.blocked_goal));
        }
        goal.status = GoalStatus::Waiting;
        self.status = TaskStatus::Waiting;
        self.need_user_input = Some(request.clone());
        self.push_event(TaskEventKind::WaitingForUserInput {
            goal: request.blocked_goal,
        });
        Ok(())
    }

    pub fn accept_user_input(
        &mut self,
        answer_summary: impl Into<String>,
    ) -> Result<(), TaskRuntimeError> {
        if self.status != TaskStatus::Waiting {
            return Err(TaskRuntimeError::NotWaiting);
        }
        let answer_summary = answer_summary.into();
        validate_text("user input summary", &answer_summary)?;
        let request = self
            .need_user_input
            .take()
            .ok_or(TaskRuntimeError::NotWaiting)?;
        let goal = self.goal_mut(&request.blocked_goal)?;
        goal.status = GoalStatus::Running;
        self.status = TaskStatus::Running;
        push_unique_bounded(&mut self.memory.assumptions, answer_summary);
        self.push_event(TaskEventKind::TaskResumed);
        Ok(())
    }

    pub fn pause(&mut self) -> Result<(), TaskRuntimeError> {
        self.ensure_active()?;
        self.status = TaskStatus::Paused;
        self.refreshed_since_pause = false;
        self.push_event(TaskEventKind::TaskPaused);
        Ok(())
    }

    pub fn refresh_resources(
        &mut self,
        observed: &[ResourceVersion],
    ) -> Result<usize, TaskRuntimeError> {
        if self.status != TaskStatus::Paused && self.status != TaskStatus::Waiting {
            return Err(TaskRuntimeError::RefreshRequiresPause);
        }
        let observed = observed
            .iter()
            .map(|resource| (resource.resource.clone(), resource.fingerprint.clone()))
            .collect::<BTreeMap<_, _>>();
        let mut invalidated = 0;
        let mut changed_resources = BTreeSet::new();
        for goal in self.goals.values_mut() {
            for evidence in &mut goal.evidence {
                let changed = evidence.resources.iter().any(|resource| {
                    observed
                        .get(&resource.resource)
                        .is_some_and(|fingerprint| fingerprint != &resource.fingerprint)
                });
                if evidence.valid && changed {
                    changed_resources.extend(evidence.resources.iter().filter_map(|resource| {
                        observed
                            .get(&resource.resource)
                            .filter(|fingerprint| *fingerprint != &resource.fingerprint)
                            .map(|_| resource.resource.clone())
                    }));
                    evidence.valid = false;
                    invalidated += 1;
                }
            }
        }
        for resource in changed_resources {
            self.push_event(TaskEventKind::EvidenceInvalidated { resource });
        }
        self.revision += u32::from(invalidated > 0);
        self.refreshed_since_pause = true;
        Ok(invalidated)
    }

    pub fn resume(&mut self) -> Result<(), TaskRuntimeError> {
        if self.status != TaskStatus::Paused {
            return Err(TaskRuntimeError::NotPaused);
        }
        if !self.refreshed_since_pause {
            return Err(TaskRuntimeError::ResumeRefreshRequired);
        }
        self.status = TaskStatus::Running;
        self.push_event(TaskEventKind::TaskResumed);
        Ok(())
    }

    pub fn complete(&mut self) -> Result<(), TaskRuntimeError> {
        self.ensure_active()?;
        let open_goals = self
            .goals
            .values()
            .filter(|goal| !matches!(goal.status, GoalStatus::Completed | GoalStatus::Obsolete))
            .map(|goal| goal.title.clone())
            .collect::<Vec<_>>();
        if !open_goals.is_empty() {
            return Err(TaskRuntimeError::GoalsOpen(open_goals));
        }
        if self.goals.values().any(|goal| {
            goal.status == GoalStatus::Completed
                && !goal.evidence.iter().any(|evidence| evidence.valid)
        }) {
            return Err(TaskRuntimeError::StaleEvidence);
        }
        self.status = TaskStatus::Completed;
        self.push_event(TaskEventKind::TaskCompleted);
        Ok(())
    }

    pub fn block(&mut self, reason: String) {
        if self.is_terminal() {
            return;
        }
        self.status = TaskStatus::Blocked;
        self.push_event(TaskEventKind::TaskBlocked { reason });
    }

    pub fn fail(&mut self, reason: impl Into<String>) {
        if self.is_terminal() {
            return;
        }
        self.status = TaskStatus::Failed;
        self.push_event(TaskEventKind::TaskFailed {
            reason: reason.into(),
        });
    }

    pub fn cancel(&mut self, reason: impl Into<String>) {
        if self.is_terminal() {
            return;
        }
        self.status = TaskStatus::Cancelled;
        self.push_event(TaskEventKind::TaskCancelled {
            reason: reason.into(),
        });
    }

    pub fn checkpoint(&self) -> TaskCheckpoint {
        TaskCheckpoint {
            schema_version: 1,
            runtime: self.clone(),
        }
    }

    pub fn restore(checkpoint: TaskCheckpoint) -> Result<Self, TaskRuntimeError> {
        if checkpoint.schema_version != 1 {
            return Err(TaskRuntimeError::UnsupportedCheckpoint(
                checkpoint.schema_version,
            ));
        }
        checkpoint.runtime.limits.validate()?;
        if checkpoint.runtime.goals.is_empty()
            || !checkpoint
                .runtime
                .goals
                .contains_key(&checkpoint.runtime.root_goal)
        {
            return Err(TaskRuntimeError::InvalidCheckpoint);
        }
        Ok(checkpoint.runtime)
    }

    pub fn prompt_summary(&self, max_bytes: usize) -> String {
        let current = self
            .goals
            .values()
            .filter(|goal| {
                matches!(
                    goal.status,
                    GoalStatus::Running | GoalStatus::Pending | GoalStatus::Waiting
                )
            })
            .map(|goal| goal.title.as_str())
            .collect::<Vec<_>>();
        let summary = format!(
            "Goal: {}\nStatus: {:?}\nCurrent goals: {}\nCompleted goals: {}\nKnown failures: {}\nOpen questions: {}",
            self.memory.original_goal,
            self.status,
            current.join("; "),
            self.memory.completed_goals.len(),
            self.memory.known_failures.join("; "),
            self.memory.open_questions.join("; "),
        );
        truncate_utf8(&summary, max_bytes.min(self.limits.max_context_bytes))
    }

    pub fn update_memory(
        &mut self,
        assumptions: impl IntoIterator<Item = String>,
        open_questions: impl IntoIterator<Item = String>,
        relevant_resources: impl IntoIterator<Item = PathBuf>,
    ) -> Result<(), TaskRuntimeError> {
        self.ensure_active()?;
        for assumption in assumptions {
            validate_text("assumption", &assumption)?;
            push_unique_bounded(&mut self.memory.assumptions, assumption);
        }
        for question in open_questions {
            validate_text("open question", &question)?;
            push_unique_bounded(&mut self.memory.open_questions, question);
        }
        for resource in relevant_resources {
            if resource.as_os_str().is_empty() {
                return Err(TaskRuntimeError::InvalidResource(resource));
            }
            push_unique_path_bounded(&mut self.memory.relevant_resources, resource);
        }
        Ok(())
    }

    fn ensure_active(&self) -> Result<(), TaskRuntimeError> {
        if self.status == TaskStatus::Running {
            Ok(())
        } else {
            Err(TaskRuntimeError::TaskNotRunning(self.status))
        }
    }

    fn goal_mut(&mut self, goal: &GoalId) -> Result<&mut TaskGoal, TaskRuntimeError> {
        self.goals
            .get_mut(goal)
            .ok_or_else(|| TaskRuntimeError::UnknownGoal(goal.clone()))
    }

    fn push_event(&mut self, event: TaskEventKind) {
        if self.events.len() == MAX_EVENTS {
            self.events.pop_front();
        }
        self.events.push_back(TaskEvent {
            sequence: self.next_event_sequence,
            event,
        });
        self.next_event_sequence += 1;
    }
}

fn push_unique_bounded(values: &mut Vec<String>, value: String) {
    if values.contains(&value) {
        return;
    }
    if values.len() == MAX_MEMORY_ITEMS {
        values.remove(0);
    }
    values.push(value);
}

fn push_unique_path_bounded(values: &mut Vec<PathBuf>, value: PathBuf) {
    if values.contains(&value) {
        return;
    }
    if values.len() == MAX_MEMORY_ITEMS {
        values.remove(0);
    }
    values.push(value);
}

fn validate_text(field: &'static str, value: &str) -> Result<(), TaskRuntimeError> {
    if value.trim().is_empty() || value.len() > MAX_TEXT_BYTES {
        Err(TaskRuntimeError::InvalidText(field))
    } else {
        Ok(())
    }
}

fn truncate_utf8(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let mut index = max_bytes.saturating_sub(3);
    while index > 0 && !value.is_char_boundary(index) {
        index -= 1;
    }
    format!("{}...", value.get(..index).unwrap_or_default())
}

#[derive(Debug, Clone)]
pub struct ScopedFilesystem {
    workspace: CodingWorkspace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileKind {
    Directory,
    Text,
    Binary,
    Sensitive,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileInspection {
    pub path: PathBuf,
    pub kind: FileKind,
    pub size: u64,
}

impl ScopedFilesystem {
    pub fn new(root: impl AsRef<Path>) -> Result<Self, WorkspaceError> {
        Ok(Self {
            workspace: CodingWorkspace::new(root)?,
        })
    }

    pub fn list(&self, path: impl AsRef<Path>) -> Result<Vec<FileInspection>, TaskRuntimeError> {
        let directory = self.workspace.resolve_existing(path)?;
        let mut entries = fs::read_dir(directory)?
            .map(|entry| entry.map_err(TaskRuntimeError::Io))
            .collect::<Result<Vec<_>, _>>()?;
        entries.sort_by_key(|entry| entry.file_name());
        entries
            .into_iter()
            .take(MAX_GOALS)
            .map(|entry| self.inspect_absolute(&entry.path()))
            .collect()
    }

    pub fn find_by_name(
        &self,
        needle: &str,
        max_results: usize,
    ) -> Result<Vec<FileInspection>, TaskRuntimeError> {
        validate_text("file search", needle)?;
        let max_results = max_results.clamp(1, MAX_GOALS);
        let mut results = Vec::new();
        let mut pending = vec![self.workspace.root().to_path_buf()];
        while let Some(directory) = pending.pop() {
            for entry in fs::read_dir(directory)? {
                let entry = entry?;
                let path = entry.path();
                let resolved = match self.workspace.resolve_existing(&path) {
                    Ok(path) => path,
                    Err(_) => continue,
                };
                let file_type = entry.file_type()?;
                if file_type.is_dir() {
                    pending.push(resolved);
                    continue;
                }
                if entry
                    .file_name()
                    .to_string_lossy()
                    .to_ascii_lowercase()
                    .contains(&needle.to_ascii_lowercase())
                {
                    results.push(self.inspect_absolute(&resolved)?);
                    if results.len() == max_results {
                        return Ok(results);
                    }
                }
            }
        }
        results.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(results)
    }

    pub fn copy_file(
        &self,
        source: impl AsRef<Path>,
        destination: impl AsRef<Path>,
        mutation_allowed: bool,
    ) -> Result<FileInspection, TaskRuntimeError> {
        self.require_mutation_allowed(mutation_allowed)?;
        let source = self.workspace.resolve_existing(source)?;
        let destination = self.workspace.resolve_for_write(destination)?;
        self.reject_sensitive_mutation(&source)?;
        self.reject_sensitive_mutation(&destination)?;
        if destination.exists() {
            return Err(TaskRuntimeError::DestinationExists(destination));
        }
        fs::copy(source, &destination)?;
        self.inspect_absolute(&destination)
    }

    pub fn move_file(
        &self,
        source: impl AsRef<Path>,
        destination: impl AsRef<Path>,
        mutation_allowed: bool,
    ) -> Result<FileInspection, TaskRuntimeError> {
        self.require_mutation_allowed(mutation_allowed)?;
        let source = self.workspace.resolve_existing(source)?;
        let destination = self.workspace.resolve_for_write(destination)?;
        self.reject_sensitive_mutation(&source)?;
        self.reject_sensitive_mutation(&destination)?;
        if destination.exists() {
            return Err(TaskRuntimeError::DestinationExists(destination));
        }
        fs::rename(source, &destination)?;
        self.inspect_absolute(&destination)
    }

    fn inspect_absolute(&self, path: &Path) -> Result<FileInspection, TaskRuntimeError> {
        let path = self.workspace.resolve_existing(path)?;
        let relative = self.workspace.relative_path(&path)?;
        let metadata = fs::metadata(&path)?;
        let kind = if is_sensitive_path(&relative) {
            FileKind::Sensitive
        } else if metadata.is_dir() {
            FileKind::Directory
        } else if looks_like_text(&path) {
            FileKind::Text
        } else {
            FileKind::Binary
        };
        Ok(FileInspection {
            path: relative,
            kind,
            size: metadata.len(),
        })
    }

    fn require_mutation_allowed(&self, mutation_allowed: bool) -> Result<(), TaskRuntimeError> {
        if mutation_allowed {
            Ok(())
        } else {
            Err(TaskRuntimeError::PolicyDenied(
                "filesystem mutation requires an explicit write policy".to_string(),
            ))
        }
    }

    fn reject_sensitive_mutation(&self, path: &Path) -> Result<(), TaskRuntimeError> {
        let relative = path.strip_prefix(self.workspace.root()).unwrap_or(path);
        if is_sensitive_path(relative) {
            Err(TaskRuntimeError::PolicyDenied(format!(
                "sensitive file mutation is unavailable: {}",
                relative.display()
            )))
        } else {
            Ok(())
        }
    }
}

fn looks_like_text(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "md" | "txt"
                    | "rs"
                    | "toml"
                    | "json"
                    | "yaml"
                    | "yml"
                    | "ts"
                    | "tsx"
                    | "js"
                    | "jsx"
                    | "py"
                    | "go"
                    | "java"
                    | "c"
                    | "h"
                    | "css"
                    | "html"
            )
        })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemInspection {
    pub operating_system: String,
    pub architecture: String,
    pub logical_cpus: usize,
    pub total_memory_kib: Option<u64>,
    pub available_memory_kib: Option<u64>,
    pub total_disk_kib: Option<u64>,
    pub available_disk_kib: Option<u64>,
    pub process_count: Option<u64>,
    pub network_interfaces: Vec<String>,
    pub current_executable: Option<PathBuf>,
    pub developer_tools: Vec<CapabilityObservation>,
}

impl SystemInspection {
    pub fn inspect() -> Self {
        let memory = sys_info::mem_info().ok();
        let disk = sys_info::disk_info().ok();
        Self {
            operating_system: sys_info::os_type()
                .unwrap_or_else(|_| std::env::consts::OS.to_string()),
            architecture: std::env::consts::ARCH.to_string(),
            logical_cpus: sys_info::cpu_num()
                .ok()
                .and_then(|count| usize::try_from(count).ok())
                .or_else(|| std::thread::available_parallelism().ok().map(usize::from))
                .unwrap_or(1),
            total_memory_kib: memory.as_ref().map(|memory| memory.total),
            available_memory_kib: memory.as_ref().map(|memory| memory.avail),
            total_disk_kib: disk.as_ref().map(|disk| disk.total),
            available_disk_kib: disk.as_ref().map(|disk| disk.free),
            process_count: sys_info::proc_total().ok(),
            network_interfaces: network_interfaces(),
            current_executable: std::env::current_exe().ok(),
            developer_tools: CapabilityDiscovery::probe(None, &["git", "cargo", "node", "python3"])
                .executables,
        }
    }
}

#[cfg(unix)]
fn network_interfaces() -> Vec<String> {
    use std::ffi::CStr;

    let mut addresses = std::ptr::null_mut();
    if unsafe { libc::getifaddrs(&mut addresses) } != 0 {
        return Vec::new();
    }
    let mut interfaces = BTreeSet::new();
    let mut current = addresses;
    while !current.is_null() {
        let address = unsafe { &*current };
        if !address.ifa_name.is_null() {
            let name = unsafe { CStr::from_ptr(address.ifa_name) }
                .to_string_lossy()
                .into_owned();
            interfaces.insert(name);
        }
        current = address.ifa_next;
    }
    unsafe { libc::freeifaddrs(addresses) };
    interfaces.into_iter().collect()
}

#[cfg(not(unix))]
fn network_interfaces() -> Vec<String> {
    Vec::new()
}

#[derive(Debug, thiserror::Error)]
pub enum TaskRuntimeError {
    #[error("{0} is empty or exceeds the runtime text bound")]
    InvalidText(&'static str),
    #[error("task limits must be positive")]
    InvalidLimits,
    #[error("task is not running: {0:?}")]
    TaskNotRunning(TaskStatus),
    #[error("unknown task goal: {0:?}")]
    UnknownGoal(GoalId),
    #[error("a goal dependency is unknown")]
    UnknownDependency,
    #[error("goal action or task goal limit reached")]
    GoalLimit,
    #[error("goal {0:?} cannot run because its dependencies or capabilities are unavailable")]
    GoalNotRunnable(GoalId),
    #[error("goal {0:?} is not running")]
    GoalNotRunning(GoalId),
    #[error("invalid goal transition for {goal:?}: {from:?} to {to:?}")]
    InvalidGoalTransition {
        goal: GoalId,
        from: GoalStatus,
        to: GoalStatus,
    },
    #[error("goal {0:?} reached its action budget")]
    GoalActionBudgetExceeded(GoalId),
    #[error("task action budget exceeded")]
    ActionBudgetExceeded,
    #[error("task process-time budget exceeded")]
    ProcessBudgetExceeded,
    #[error("task total-duration budget exceeded")]
    TotalDurationBudgetExceeded,
    #[error("task tool-error budget exceeded")]
    ToolErrorBudgetExceeded,
    #[error("task repeated-failure budget exceeded")]
    RepeatedFailureBudgetExceeded,
    #[error("task repair budget exceeded")]
    RepairBudgetExceeded,
    #[error("task replan budget exceeded")]
    ReplanBudgetExceeded,
    #[error("goal {0:?} needs valid runtime evidence before completion")]
    EvidenceRequired(GoalId),
    #[error("goal {0:?} has reached its evidence bound")]
    EvidenceLimit(GoalId),
    #[error("goal {0:?} still has incomplete child goals")]
    ChildGoalsOpen(GoalId),
    #[error("terminal goal {0:?} cannot be replanned")]
    CannotReplanTerminalGoal(GoalId),
    #[error("task is not waiting for user input")]
    NotWaiting,
    #[error("task resource refresh requires a paused or waiting task")]
    RefreshRequiresPause,
    #[error("task is not paused")]
    NotPaused,
    #[error("task resume requires a resource refresh after pause")]
    ResumeRefreshRequired,
    #[error("task has incomplete goals: {0:?}")]
    GoalsOpen(Vec<String>),
    #[error("completed task evidence is stale")]
    StaleEvidence,
    #[error("unsupported task checkpoint schema version: {0}")]
    UnsupportedCheckpoint(u32),
    #[error("task checkpoint is incomplete")]
    InvalidCheckpoint,
    #[error("duplicate tool descriptor: {0}")]
    DuplicateTool(String),
    #[error("duplicate skill bundle: {0}")]
    DuplicateSkill(String),
    #[error("invalid skill bundle: {0}")]
    InvalidSkill(String),
    #[error("unknown skill bundle: {0}")]
    UnknownSkill(String),
    #[error("filesystem mutation is blocked by policy: {0}")]
    PolicyDenied(String),
    #[error("filesystem destination already exists: {0}")]
    DestinationExists(PathBuf),
    #[error("invalid task resource: {0}")]
    InvalidResource(PathBuf),
    #[error(transparent)]
    Workspace(#[from] WorkspaceError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runtime() -> TaskRuntime {
        TaskRuntime::new("organize a project safely", None, TaskLimits::default()).unwrap()
    }

    fn evidence(id: &str, resources: Vec<ResourceVersion>) -> GoalEvidence {
        GoalEvidence {
            id: id.to_string(),
            kind: "verification".to_string(),
            summary: "verified requested end state".to_string(),
            resources,
            revision: 0,
            valid: false,
        }
    }

    #[test]
    fn preserves_completed_goals_when_replanning_an_obsolete_hypothesis() {
        let mut runtime = runtime();
        let completed = runtime
            .add_subtask(
                &runtime.root_goal.clone(),
                "inspect project",
                BTreeSet::new(),
                BTreeSet::new(),
                GoalBudget::default(),
            )
            .unwrap();
        runtime.start_goal(&completed).unwrap();
        runtime
            .add_evidence(&completed, evidence("inspection", Vec::new()))
            .unwrap();
        runtime.complete_goal(&completed).unwrap();

        let incorrect = runtime
            .add_subtask(
                &runtime.root_goal.clone(),
                "patch dependency",
                BTreeSet::new(),
                BTreeSet::new(),
                GoalBudget::default(),
            )
            .unwrap();
        runtime.start_goal(&incorrect).unwrap();
        let replacement = runtime
            .replan(
                &incorrect,
                "inspect compiler output",
                "dependency was not the cause",
            )
            .unwrap();

        assert_eq!(runtime.goals[&completed].status, GoalStatus::Completed);
        assert_eq!(runtime.goals[&incorrect].status, GoalStatus::Obsolete);
        assert_eq!(runtime.goals[&replacement].status, GoalStatus::Pending);
        assert_eq!(runtime.replans, 1);
    }

    #[test]
    fn checkpoint_restores_compact_state_and_invalidates_stale_evidence_before_resume() {
        let mut runtime = runtime();
        let child = runtime
            .add_subtask(
                &runtime.root_goal.clone(),
                "validate configuration",
                BTreeSet::new(),
                BTreeSet::new(),
                GoalBudget::default(),
            )
            .unwrap();
        runtime.start_goal(&child).unwrap();
        runtime
            .add_evidence(
                &child,
                evidence(
                    "config-digest",
                    vec![ResourceVersion {
                        resource: PathBuf::from("config.toml"),
                        fingerprint: Some("before".to_string()),
                    }],
                ),
            )
            .unwrap();
        runtime.pause().unwrap();
        let checkpoint = serde_json::from_str::<TaskCheckpoint>(
            &serde_json::to_string(&runtime.checkpoint()).unwrap(),
        )
        .unwrap();
        let mut restored = TaskRuntime::restore(checkpoint).unwrap();
        assert_eq!(
            restored
                .refresh_resources(&[ResourceVersion {
                    resource: PathBuf::from("config.toml"),
                    fingerprint: Some("after".to_string()),
                }])
                .unwrap(),
            1
        );
        restored.resume().unwrap();
        assert!(!restored.goals[&child].evidence[0].valid);
    }

    #[test]
    fn completion_requires_evidence_for_every_completed_goal() {
        let mut runtime = runtime();
        let root = runtime.root_goal.clone();
        runtime
            .add_evidence(&root, evidence("root", Vec::new()))
            .unwrap();
        runtime.complete_goal(&root).unwrap();
        runtime.complete().unwrap();
        assert_eq!(runtime.status, TaskStatus::Completed);
    }

    #[test]
    fn repeated_failures_stop_the_task_with_a_machine_detected_reason() {
        let mut runtime = TaskRuntime::new(
            "repair fixture",
            None,
            TaskLimits {
                max_repeated_failures: 1,
                ..TaskLimits::default()
            },
        )
        .unwrap();
        let root = runtime.root_goal.clone();
        for _ in 0..2 {
            let result = runtime.record_action(
                &root,
                ActionRecord {
                    tool: "inspect".to_string(),
                    summary: "same diagnostic".to_string(),
                    outcome: ActionOutcome::Failed,
                    failure_fingerprint: Some("same-error".to_string()),
                    process_time_ms: None,
                },
            );
            if result.is_err() {
                break;
            }
        }
        assert_eq!(runtime.status, TaskStatus::Blocked);
    }

    #[test]
    fn total_duration_budget_blocks_before_another_action_is_recorded() {
        let mut runtime = TaskRuntime::new(
            "inspect a slow local project",
            None,
            TaskLimits {
                max_total_duration_ms: 5,
                ..TaskLimits::default()
            },
        )
        .unwrap();
        let root = runtime.root_goal.clone();
        assert!(matches!(
            runtime.record_action(
                &root,
                ActionRecord {
                    tool: "system__inspect".to_string(),
                    summary: "observed local facts".to_string(),
                    outcome: ActionOutcome::Succeeded,
                    failure_fingerprint: None,
                    process_time_ms: Some(6),
                },
            ),
            Err(TaskRuntimeError::TotalDurationBudgetExceeded)
        ));
        assert_eq!(runtime.status, TaskStatus::Blocked);
    }

    #[test]
    fn progressive_disclosure_does_not_unlock_mutation_or_unrelated_domains() {
        let registry = ToolRegistry::builtin();
        let disclosed = registry.disclose(&ToolDisclosureRequest {
            domains: [TaskDomain::Filesystem].into_iter().collect(),
            allow_writes: false,
            allow_network: false,
            readable_workspace: true,
            writable_workspace: false,
            maximum_risk: RiskLevel::Low,
            available_capabilities: BTreeSet::new(),
        });
        assert_eq!(
            disclosed
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            vec!["filesystem__find", "filesystem__list"]
        );
    }

    #[test]
    fn cost_aware_disclosure_prefers_cheap_relevant_observations() {
        let registry = ToolRegistry::builtin();
        let tools = registry.next_actions(&ToolDisclosureRequest {
            domains: [TaskDomain::Coding].into_iter().collect(),
            allow_writes: true,
            allow_network: false,
            readable_workspace: true,
            writable_workspace: true,
            maximum_risk: RiskLevel::Medium,
            available_capabilities: BTreeSet::new(),
        });

        assert_eq!(tools[0].name, "coding__repository_profile");
        assert_eq!(tools[1].name, "coding__run_process");
    }

    #[test]
    fn unavailable_capabilities_keep_a_subtask_pending() {
        let mut runtime = runtime();
        let goal = runtime
            .add_subtask(
                &runtime.root_goal.clone(),
                "inspect a repository",
                BTreeSet::new(),
                ["git".to_string()].into_iter().collect(),
                GoalBudget::default(),
            )
            .unwrap();

        assert!(runtime.runnable_goals(&BTreeSet::new()).is_empty());
        assert!(matches!(
            runtime.start_goal(&goal),
            Err(TaskRuntimeError::GoalNotRunnable(_))
        ));
        runtime
            .start_goal_with_capabilities(&goal, &["git".to_string()].into_iter().collect())
            .unwrap();
    }

    #[test]
    fn filesystem_operations_stay_scoped_and_respect_policy_and_sensitive_files() {
        let temporary = tempfile::tempdir().unwrap();
        fs::write(temporary.path().join("notes.md"), "notes").unwrap();
        fs::write(temporary.path().join(".env"), "secret").unwrap();
        let filesystem = ScopedFilesystem::new(temporary.path()).unwrap();
        assert!(matches!(
            filesystem.copy_file("notes.md", "copy.md", false),
            Err(TaskRuntimeError::PolicyDenied(_))
        ));
        assert!(matches!(
            filesystem.copy_file(".env", "copy.env", true),
            Err(TaskRuntimeError::PolicyDenied(_))
        ));
        assert_eq!(
            filesystem
                .copy_file("notes.md", "copy.md", true)
                .unwrap()
                .kind,
            FileKind::Text
        );
        assert!(matches!(
            filesystem.move_file("notes.md", "../outside.md", true),
            Err(TaskRuntimeError::Workspace(_))
        ));
    }

    #[test]
    fn explicit_user_input_is_a_waiting_state_not_an_invented_assumption() {
        let mut runtime = runtime();
        let root = runtime.root_goal.clone();
        runtime
            .request_user_input(NeedUserInput {
                required_information: "target directory".to_string(),
                reason: "multiple directories are equally safe".to_string(),
                blocked_goal: root.clone(),
                allowed_options: vec!["archive".to_string(), "keep".to_string()],
            })
            .unwrap();
        assert_eq!(runtime.status, TaskStatus::Waiting);
        runtime
            .accept_user_input("user selected archive".to_string())
            .unwrap();
        assert_eq!(runtime.status, TaskStatus::Running);
        assert!(runtime
            .memory
            .assumptions
            .contains(&"user selected archive".to_string()));
    }

    #[test]
    fn system_inspection_reports_observed_local_facts() {
        let inspection = SystemInspection::inspect();

        assert!(!inspection.operating_system.is_empty());
        assert!(!inspection.architecture.is_empty());
        assert!(inspection.logical_cpus > 0);
        assert_eq!(inspection.developer_tools.len(), 4);
    }

    #[test]
    fn resume_requires_a_refresh_even_when_no_evidence_changes() {
        let mut runtime = runtime();
        runtime.pause().unwrap();
        assert!(matches!(
            runtime.resume(),
            Err(TaskRuntimeError::ResumeRefreshRequired)
        ));
        assert_eq!(runtime.refresh_resources(&[]).unwrap(), 0);
        runtime.resume().unwrap();
    }
}
