use crate::coding::file::content_digest;
use crate::coding::intelligence::CodeSymbol;
use crate::coding::patch::MutationPreview;
use crate::coding::process::ProcessOutput;
use crate::coding::validation::{ValidationExecution, ValidationStatus};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Component, Path, PathBuf};
use uuid::Uuid;

const MAX_PLAN_ITEMS: usize = 200;
const MAX_TEXT_BYTES: usize = 16 * 1_024;
const MAX_EVIDENCE_RECORDS: usize = 200;
const LOOP_THRESHOLD: usize = 3;

/// Auditable state machine for multi-step coding tasks.
#[derive(Debug, Clone)]
pub struct CodingWorkflow {
    id: String,
    objective: String,
    phase: WorkflowPhase,
    plan: Option<WorkflowPlan>,
    limits: WorkflowLimits,
    iterations: u32,
    repair_attempts: u32,
    revision: u32,
    changes: Vec<ChangeEvidence>,
    validations: Vec<ValidationEvidence>,
    invocation_history: VecDeque<InvocationEvidence>,
    failure_counts: BTreeMap<String, usize>,
    non_improving_failures: usize,
    last_error_count: Option<usize>,
    stop_reason: Option<WorkflowStopReason>,
    completion: Option<CompletionDetails>,
    memory: WorkflowMemory,
}

impl CodingWorkflow {
    pub fn new(objective: String, limits: WorkflowLimits) -> Result<Self, WorkflowError> {
        limits.validate()?;
        validate_text("objective", &objective, false)?;
        Ok(Self {
            id: Uuid::now_v7().to_string(),
            objective,
            phase: WorkflowPhase::Analyzing,
            plan: None,
            limits,
            iterations: 0,
            repair_attempts: 0,
            revision: 0,
            changes: Vec::new(),
            validations: Vec::new(),
            invocation_history: VecDeque::new(),
            failure_counts: BTreeMap::new(),
            non_improving_failures: 0,
            last_error_count: None,
            stop_reason: None,
            completion: None,
            memory: WorkflowMemory::default(),
        })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn phase(&self) -> WorkflowPhase {
        self.phase
    }

    pub fn is_terminal(&self) -> bool {
        matches!(
            self.phase,
            WorkflowPhase::Completed | WorkflowPhase::Blocked | WorkflowPhase::Failed
        )
    }

    pub fn tracks_change(&self, change_id: &str) -> bool {
        self.changes
            .iter()
            .any(|change| change.change_id == change_id && !change.rolled_back)
    }

    pub fn note_repository_activity(&mut self) {
        if self.phase == WorkflowPhase::Analyzing {
            self.phase = WorkflowPhase::Searching;
        }
    }

    pub fn note_read_files(&mut self, paths: impl IntoIterator<Item = PathBuf>) {
        if self.is_terminal() {
            return;
        }
        let mut known = self
            .memory
            .read_files
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        for path in paths {
            if known.len() == MAX_EVIDENCE_RECORDS {
                break;
            }
            known.insert(path);
        }
        self.memory.read_files = known.into_iter().collect();
    }

    pub fn note_symbols<'a>(&mut self, symbols: impl IntoIterator<Item = &'a CodeSymbol>) {
        if self.is_terminal() {
            return;
        }
        let mut known = self
            .memory
            .relevant_symbols
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        for symbol in symbols {
            if known.len() == MAX_EVIDENCE_RECORDS {
                break;
            }
            known.insert(RelevantSymbolEvidence {
                path: symbol.path.clone(),
                name: symbol.name.clone(),
                qualified_name: symbol.qualified_name.clone(),
                line: symbol.line,
            });
        }
        self.memory.relevant_symbols = known.into_iter().collect();
    }

    pub fn update_memory_notes(
        &mut self,
        assumptions: Option<Vec<String>>,
        open_points: Option<Vec<String>>,
    ) -> Result<(), WorkflowError> {
        if self.is_terminal() {
            return Err(WorkflowError::TerminalMemoryUpdate);
        }
        if let Some(assumptions) = assumptions {
            validate_items("workflow assumptions", &assumptions, true)?;
            self.memory.assumptions = assumptions;
        }
        if let Some(open_points) = open_points {
            validate_items("workflow open points", &open_points, true)?;
            self.memory.open_points = open_points;
        }
        Ok(())
    }

    pub fn set_plan(&mut self, plan: WorkflowPlan) -> Result<(), WorkflowError> {
        self.require_phase(&[WorkflowPhase::Analyzing, WorkflowPhase::Searching])?;
        plan.validate()?;
        self.plan = Some(plan);
        self.phase = WorkflowPhase::Planning;
        Ok(())
    }

    pub fn begin_editing(&mut self) -> Result<(), WorkflowError> {
        self.require_phase(&[WorkflowPhase::Planning])?;
        if self.plan.is_none() {
            return Err(WorkflowError::PlanRequired);
        }
        self.phase = WorkflowPhase::Editing;
        Ok(())
    }

    pub fn authorize_change(&mut self) -> Result<(), WorkflowError> {
        self.require_phase(&[WorkflowPhase::Editing])?;
        if self.iterations >= self.limits.max_iterations {
            self.fail(WorkflowStopReason::IterationLimit {
                limit: self.limits.max_iterations,
            });
            return Err(WorkflowError::IterationLimit(self.limits.max_iterations));
        }
        Ok(())
    }

    pub fn record_change(
        &mut self,
        change_id: String,
        preview: &MutationPreview,
    ) -> Result<(), WorkflowError> {
        self.require_phase(&[WorkflowPhase::Editing])?;
        let serialized = serde_json::to_vec(preview)
            .map_err(|error| WorkflowError::Evidence(error.to_string()))?;
        let diff_fingerprint = content_digest(&serialized);
        let repeated = self
            .changes
            .iter()
            .filter(|change| !change.rolled_back && change.diff_fingerprint == diff_fingerprint)
            .count()
            + 1;
        self.iterations += 1;
        self.revision += 1;
        self.changes.push(ChangeEvidence {
            change_id,
            revision: self.revision,
            files: preview.files.iter().map(|file| file.path.clone()).collect(),
            diff_fingerprint,
            rolled_back: false,
        });
        self.last_error_count = None;
        self.non_improving_failures = 0;
        if repeated >= LOOP_THRESHOLD {
            self.block(WorkflowStopReason::RepeatedDiff {
                repetitions: repeated,
            });
        }
        Ok(())
    }

    pub fn record_rollback(&mut self, change_id: &str) -> Result<(), WorkflowError> {
        let change = self
            .changes
            .iter_mut()
            .find(|change| change.change_id == change_id && !change.rolled_back)
            .ok_or_else(|| WorkflowError::UnknownChange(change_id.to_string()))?;
        change.rolled_back = true;
        self.revision += 1;
        if !self.is_terminal() {
            self.phase = WorkflowPhase::Editing;
        }
        Ok(())
    }

    pub fn begin_validation(&mut self) -> Result<(), WorkflowError> {
        self.require_phase(&[WorkflowPhase::Editing])?;
        if !self.changes.iter().any(|change| !change.rolled_back) {
            return Err(WorkflowError::ChangeRequired);
        }
        self.phase = WorkflowPhase::Testing;
        Ok(())
    }

    pub fn record_process(
        &mut self,
        program: &str,
        args: &[String],
        output: &ProcessOutput,
    ) -> Result<(), WorkflowError> {
        if self.is_terminal() {
            return Ok(());
        }
        let invocation_fingerprint =
            invocation_fingerprint(program, args, &output.cwd, self.revision);
        self.invocation_history.push_back(InvocationEvidence {
            revision: self.revision,
            fingerprint: invocation_fingerprint.clone(),
        });
        while self.invocation_history.len() > MAX_EVIDENCE_RECORDS {
            self.invocation_history.pop_front();
        }
        self.record_command_evidence(CommandEvidence::from_process(
            self.revision,
            program,
            args,
            output,
        ));
        let repetitions = self
            .invocation_history
            .iter()
            .filter(|evidence| {
                evidence.revision == self.revision && evidence.fingerprint == invocation_fingerprint
            })
            .count();
        if repetitions >= LOOP_THRESHOLD {
            self.block(WorkflowStopReason::RepeatedToolCall { repetitions });
            return Ok(());
        }
        if self.phase != WorkflowPhase::Testing {
            return Ok(());
        }

        let validation = ValidationEvidence::from_process(self.revision, program, args, output);
        self.accept_validation(validation)
    }

    pub fn record_validation_execution(
        &mut self,
        execution: &ValidationExecution,
    ) -> Result<(), WorkflowError> {
        if self.is_terminal() || self.phase != WorkflowPhase::Testing {
            return Ok(());
        }
        if let Some(command) = CommandEvidence::from_execution(self.revision, execution) {
            self.record_command_evidence(command);
        }
        self.accept_validation(ValidationEvidence::from_execution(self.revision, execution))
    }

    fn accept_validation(&mut self, validation: ValidationEvidence) -> Result<(), WorkflowError> {
        let outcome = validation.outcome;
        let failed = outcome.requires_repair();
        let diagnostic_fingerprint = validation.diagnostic_fingerprint.clone();
        let error_count = validation.error_count;
        self.validations.push(validation);
        if self.validations.len() > MAX_EVIDENCE_RECORDS {
            self.validations.remove(0);
        }
        if outcome.is_success() {
            self.non_improving_failures = 0;
            self.last_error_count = Some(0);
            return Ok(());
        }
        if let Some(validation) = self.validations.last() {
            let known = KnownErrorEvidence {
                revision: validation.revision,
                outcome: validation.outcome,
                error_count: validation.error_count,
                diagnostic_fingerprint: validation.diagnostic_fingerprint.clone(),
            };
            if !self.memory.known_errors.contains(&known) {
                self.memory.known_errors.push(known);
                if self.memory.known_errors.len() > MAX_EVIDENCE_RECORDS {
                    self.memory.known_errors.remove(0);
                }
            }
        }
        if !failed {
            return Ok(());
        }

        let repetitions = {
            let count = self
                .failure_counts
                .entry(diagnostic_fingerprint)
                .or_insert(0);
            *count += 1;
            *count
        };
        if repetitions >= LOOP_THRESHOLD {
            self.block(WorkflowStopReason::RepeatedFailure { repetitions });
            return Ok(());
        }
        if self
            .last_error_count
            .is_some_and(|previous| error_count >= previous)
        {
            self.non_improving_failures += 1;
        } else {
            self.non_improving_failures = 1;
        }
        self.last_error_count = Some(error_count);
        if self.non_improving_failures >= LOOP_THRESHOLD {
            self.block(WorkflowStopReason::NoDiagnosticProgress {
                attempts: self.non_improving_failures,
            });
        } else {
            self.phase = WorkflowPhase::Debugging;
        }
        Ok(())
    }

    pub fn begin_repair(&mut self) -> Result<(), WorkflowError> {
        self.require_phase(&[WorkflowPhase::Debugging])?;
        if self.repair_attempts >= self.limits.max_repair_attempts {
            self.block(WorkflowStopReason::RepairLimit {
                limit: self.limits.max_repair_attempts,
            });
            return Err(WorkflowError::RepairLimit(self.limits.max_repair_attempts));
        }
        self.repair_attempts += 1;
        self.phase = WorkflowPhase::Editing;
        Ok(())
    }

    pub fn begin_review(&mut self) -> Result<(), WorkflowError> {
        let validation_required = self
            .plan
            .as_ref()
            .is_some_and(|plan| !plan.validation.is_empty() || !plan.tests.is_empty());
        match self.phase {
            WorkflowPhase::Testing => {
                let current = self
                    .validations
                    .iter()
                    .filter(|validation| validation.revision == self.revision)
                    .collect::<Vec<_>>();
                if validation_required && current.is_empty() {
                    return Err(WorkflowError::ValidationRequired);
                }
                if current
                    .iter()
                    .any(|validation| validation.outcome.requires_repair())
                {
                    return Err(WorkflowError::ValidationFailed);
                }
            }
            WorkflowPhase::Editing if !validation_required => {}
            _ => {
                return Err(WorkflowError::InvalidTransition {
                    from: self.phase,
                    expected: vec![WorkflowPhase::Testing],
                });
            }
        }
        self.phase = WorkflowPhase::Reviewing;
        Ok(())
    }

    pub fn can_begin_review(&self) -> bool {
        let mut candidate = self.clone();
        candidate.begin_review().is_ok()
    }

    pub fn complete(
        &mut self,
        summary: String,
        remaining_risks: Vec<String>,
    ) -> Result<WorkflowReport, WorkflowError> {
        self.require_phase(&[WorkflowPhase::Reviewing])?;
        validate_text("completion summary", &summary, false)?;
        validate_items("remaining risks", &remaining_risks, true)?;
        self.completion = Some(CompletionDetails {
            summary,
            remaining_risks,
        });
        self.phase = WorkflowPhase::Completed;
        Ok(self.report())
    }

    pub fn status(&self) -> WorkflowStatus {
        WorkflowStatus {
            id: self.id.clone(),
            objective: self.objective.clone(),
            phase: self.phase,
            plan: self.plan.clone(),
            iterations: self.iterations,
            max_iterations: self.limits.max_iterations,
            repair_attempts: self.repair_attempts,
            max_repair_attempts: self.limits.max_repair_attempts,
            revision: self.revision,
            changed_files: self.changed_files(),
            validation_count: self.validations.len(),
            stop_reason: self.stop_reason.clone(),
            memory: self.memory.clone(),
        }
    }

    pub fn report(&self) -> WorkflowReport {
        let current_validations = self
            .validations
            .iter()
            .filter(|validation| validation.revision == self.revision)
            .cloned()
            .collect::<Vec<_>>();
        let verified = !current_validations.is_empty()
            && current_validations
                .iter()
                .all(|validation| validation.outcome.is_success());
        WorkflowReport {
            id: self.id.clone(),
            objective: self.objective.clone(),
            phase: self.phase,
            changed_files: self.changed_files(),
            iterations: self.iterations,
            repair_attempts: self.repair_attempts,
            validations: current_validations,
            verified,
            summary: self
                .completion
                .as_ref()
                .map(|completion| completion.summary.clone()),
            remaining_risks: self
                .completion
                .as_ref()
                .map(|completion| completion.remaining_risks.clone())
                .unwrap_or_default(),
            stop_reason: self.stop_reason.clone(),
            memory: self.memory.clone(),
        }
    }

    fn changed_files(&self) -> Vec<PathBuf> {
        self.changes
            .iter()
            .filter(|change| !change.rolled_back)
            .flat_map(|change| change.files.iter().cloned())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    fn record_command_evidence(&mut self, command: CommandEvidence) {
        self.memory.executed_commands.push(command);
        if self.memory.executed_commands.len() > MAX_EVIDENCE_RECORDS {
            self.memory.executed_commands.remove(0);
        }
    }

    fn require_phase(&self, expected: &[WorkflowPhase]) -> Result<(), WorkflowError> {
        if expected.contains(&self.phase) {
            Ok(())
        } else {
            Err(WorkflowError::InvalidTransition {
                from: self.phase,
                expected: expected.to_vec(),
            })
        }
    }

    fn block(&mut self, reason: WorkflowStopReason) {
        self.phase = WorkflowPhase::Blocked;
        self.stop_reason = Some(reason);
    }

    fn fail(&mut self, reason: WorkflowStopReason) {
        self.phase = WorkflowPhase::Failed;
        self.stop_reason = Some(reason);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkflowLimits {
    pub max_iterations: u32,
    pub max_repair_attempts: u32,
}

impl WorkflowLimits {
    fn validate(self) -> Result<(), WorkflowError> {
        if self.max_iterations == 0 || self.max_iterations > 1_000 {
            return Err(WorkflowError::InvalidIterationLimit(self.max_iterations));
        }
        if self.max_repair_attempts > 100 {
            return Err(WorkflowError::InvalidRepairLimit(self.max_repair_attempts));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowPlan {
    pub affected_components: Vec<String>,
    pub relevant_files: Vec<PathBuf>,
    pub risks: Vec<String>,
    pub intended_changes: Vec<String>,
    pub tests: Vec<String>,
    pub validation: Vec<String>,
    pub rollback_strategy: String,
}

impl WorkflowPlan {
    fn validate(&self) -> Result<(), WorkflowError> {
        validate_items("affected components", &self.affected_components, false)?;
        validate_paths(&self.relevant_files)?;
        validate_items("risks", &self.risks, true)?;
        validate_items("intended changes", &self.intended_changes, false)?;
        validate_items("tests", &self.tests, true)?;
        validate_items("validation", &self.validation, true)?;
        validate_text("rollback strategy", &self.rollback_strategy, false)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowPhase {
    Analyzing,
    Searching,
    Planning,
    Editing,
    Testing,
    Debugging,
    Reviewing,
    Completed,
    Blocked,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowStatus {
    pub id: String,
    pub objective: String,
    pub phase: WorkflowPhase,
    pub plan: Option<WorkflowPlan>,
    pub iterations: u32,
    pub max_iterations: u32,
    pub repair_attempts: u32,
    pub max_repair_attempts: u32,
    pub revision: u32,
    pub changed_files: Vec<PathBuf>,
    pub validation_count: usize,
    pub stop_reason: Option<WorkflowStopReason>,
    #[serde(default)]
    pub memory: WorkflowMemory,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowReport {
    pub id: String,
    pub objective: String,
    pub phase: WorkflowPhase,
    pub changed_files: Vec<PathBuf>,
    pub iterations: u32,
    pub repair_attempts: u32,
    pub validations: Vec<ValidationEvidence>,
    pub verified: bool,
    pub summary: Option<String>,
    pub remaining_risks: Vec<String>,
    pub stop_reason: Option<WorkflowStopReason>,
    #[serde(default)]
    pub memory: WorkflowMemory,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowMemory {
    pub assumptions: Vec<String>,
    pub read_files: Vec<PathBuf>,
    pub relevant_symbols: Vec<RelevantSymbolEvidence>,
    pub executed_commands: Vec<CommandEvidence>,
    pub known_errors: Vec<KnownErrorEvidence>,
    pub open_points: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct RelevantSymbolEvidence {
    pub path: PathBuf,
    pub name: String,
    pub qualified_name: String,
    pub line: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandEvidence {
    pub revision: u32,
    pub program: String,
    pub cwd: PathBuf,
    pub argument_count: usize,
    pub argument_fingerprint: String,
    pub outcome: ValidationOutcome,
    pub duration_ms: u128,
}

impl CommandEvidence {
    fn from_process(revision: u32, program: &str, args: &[String], output: &ProcessOutput) -> Self {
        let mut argument_bytes = Vec::new();
        for argument in args {
            argument_bytes.extend_from_slice(argument.as_bytes());
            argument_bytes.push(0);
        }
        let outcome = if output.timed_out {
            ValidationOutcome::TimedOut
        } else if output.background_process_detected || output.output_collection_error.is_some() {
            ValidationOutcome::IncompleteOutput
        } else if output.success {
            ValidationOutcome::Passed
        } else {
            ValidationOutcome::Failed
        };
        Self {
            revision,
            program: program.to_string(),
            cwd: output.cwd.clone(),
            argument_count: args.len(),
            argument_fingerprint: content_digest(&argument_bytes),
            outcome,
            duration_ms: output.duration_ms,
        }
    }

    fn from_execution(revision: u32, execution: &ValidationExecution) -> Option<Self> {
        let command = execution.command.as_ref()?;
        if let Some(output) = &execution.output {
            return Some(Self::from_process(
                revision,
                &command.program,
                &command.args,
                output,
            ));
        }
        let mut argument_bytes = Vec::new();
        for argument in &command.args {
            argument_bytes.extend_from_slice(argument.as_bytes());
            argument_bytes.push(0);
        }
        Some(Self {
            revision,
            program: command.program.clone(),
            cwd: command.cwd.clone(),
            argument_count: command.args.len(),
            argument_fingerprint: content_digest(&argument_bytes),
            outcome: ValidationOutcome::from(execution.status),
            duration_ms: 0,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnownErrorEvidence {
    pub revision: u32,
    pub outcome: ValidationOutcome,
    pub error_count: usize,
    pub diagnostic_fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeEvidence {
    pub change_id: String,
    pub revision: u32,
    pub files: Vec<PathBuf>,
    pub diff_fingerprint: String,
    pub rolled_back: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationEvidence {
    pub revision: u32,
    pub program: Option<String>,
    pub cwd: Option<PathBuf>,
    pub argument_count: Option<usize>,
    pub argument_fingerprint: Option<String>,
    pub outcome: ValidationOutcome,
    pub exit_code: Option<i32>,
    pub duration_ms: u128,
    pub output_truncated: bool,
    pub error_count: usize,
    pub diagnostic_fingerprint: String,
}

impl ValidationEvidence {
    fn from_process(revision: u32, program: &str, args: &[String], output: &ProcessOutput) -> Self {
        let outcome = if output.timed_out {
            ValidationOutcome::TimedOut
        } else if output.background_process_detected || output.output_collection_error.is_some() {
            ValidationOutcome::IncompleteOutput
        } else if output.success {
            ValidationOutcome::Passed
        } else {
            ValidationOutcome::Failed
        };
        let diagnostics = normalized_diagnostics(&output.stdout, &output.stderr);
        let mut argument_bytes = Vec::new();
        for argument in args {
            argument_bytes.extend_from_slice(argument.as_bytes());
            argument_bytes.push(0);
        }
        let error_count = output
            .diagnostics
            .error_count
            .max(diagnostic_error_count(&diagnostics, output.success));
        let diagnostic_fingerprint = if output.diagnostics.diagnostics.is_empty() {
            content_digest(diagnostics.as_bytes())
        } else {
            output.diagnostics.fingerprint.clone()
        };
        Self {
            revision,
            program: Some(program.to_string()),
            cwd: Some(output.cwd.clone()),
            argument_count: Some(args.len()),
            argument_fingerprint: Some(content_digest(&argument_bytes)),
            outcome,
            exit_code: output.exit_code,
            duration_ms: output.duration_ms,
            output_truncated: output.output_truncated,
            error_count,
            diagnostic_fingerprint,
        }
    }

    fn from_execution(revision: u32, execution: &ValidationExecution) -> Self {
        if let (Some(command), Some(output)) = (&execution.command, &execution.output) {
            let mut evidence =
                Self::from_process(revision, &command.program, &command.args, output);
            evidence.outcome = ValidationOutcome::from(execution.status);
            return evidence;
        }

        let (program, cwd, argument_count, argument_fingerprint) = execution
            .command
            .as_ref()
            .map(|command| {
                let mut argument_bytes = Vec::new();
                for argument in &command.args {
                    argument_bytes.extend_from_slice(argument.as_bytes());
                    argument_bytes.push(0);
                }
                (
                    Some(command.program.clone()),
                    Some(command.cwd.clone()),
                    Some(command.args.len()),
                    Some(content_digest(&argument_bytes)),
                )
            })
            .unwrap_or((None, None, None, None));
        let diagnostics = execution.reason.as_deref().unwrap_or("");
        Self {
            revision,
            program,
            cwd,
            argument_count,
            argument_fingerprint,
            outcome: ValidationOutcome::from(execution.status),
            exit_code: None,
            duration_ms: 0,
            output_truncated: false,
            error_count: 0,
            diagnostic_fingerprint: content_digest(diagnostics.as_bytes()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationOutcome {
    Passed,
    Failed,
    NotPresent,
    NotExecutable,
    Skipped,
    Blocked,
    TimedOut,
    IncompleteOutput,
}

impl ValidationOutcome {
    fn is_success(self) -> bool {
        self == Self::Passed
    }

    fn requires_repair(self) -> bool {
        self == Self::Failed
    }
}

impl From<ValidationStatus> for ValidationOutcome {
    fn from(status: ValidationStatus) -> Self {
        match status {
            ValidationStatus::Passed => Self::Passed,
            ValidationStatus::Failed => Self::Failed,
            ValidationStatus::NotPresent => Self::NotPresent,
            ValidationStatus::NotExecutable => Self::NotExecutable,
            ValidationStatus::Skipped => Self::Skipped,
            ValidationStatus::Blocked => Self::Blocked,
            ValidationStatus::TimedOut => Self::TimedOut,
            ValidationStatus::IncompleteOutput => Self::IncompleteOutput,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkflowStopReason {
    IterationLimit { limit: u32 },
    RepairLimit { limit: u32 },
    RepeatedDiff { repetitions: usize },
    RepeatedFailure { repetitions: usize },
    RepeatedToolCall { repetitions: usize },
    NoDiagnosticProgress { attempts: usize },
}

#[derive(Debug, Clone)]
struct InvocationEvidence {
    revision: u32,
    fingerprint: String,
}

#[derive(Debug, Clone)]
struct CompletionDetails {
    summary: String,
    remaining_risks: Vec<String>,
}

fn invocation_fingerprint(program: &str, args: &[String], cwd: &Path, revision: u32) -> String {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(program.as_bytes());
    bytes.push(0);
    for argument in args {
        bytes.extend_from_slice(argument.as_bytes());
        bytes.push(0);
    }
    bytes.extend_from_slice(cwd.to_string_lossy().as_bytes());
    bytes.extend_from_slice(&revision.to_le_bytes());
    content_digest(&bytes)
}

fn normalized_diagnostics(stdout: &str, stderr: &str) -> String {
    stdout
        .lines()
        .chain(stderr.lines())
        .map(|line| {
            line.split_whitespace()
                .map(|word| {
                    if word.chars().all(|character| character.is_ascii_digit()) {
                        "#"
                    } else {
                        word
                    }
                })
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn diagnostic_error_count(diagnostics: &str, success: bool) -> usize {
    let count = diagnostics
        .lines()
        .filter(|line| {
            let normalized = line.to_ascii_lowercase();
            normalized.contains("error")
                || normalized.contains("fail")
                || normalized.contains("panic")
                || normalized.contains("exception")
        })
        .count();
    if success {
        count
    } else {
        count.max(1)
    }
}

fn validate_text(
    field: &'static str,
    value: &str,
    empty_allowed: bool,
) -> Result<(), WorkflowError> {
    if (!empty_allowed && value.trim().is_empty()) || value.len() > MAX_TEXT_BYTES {
        Err(WorkflowError::InvalidPlanField(field))
    } else {
        Ok(())
    }
}

fn validate_items(
    field: &'static str,
    values: &[String],
    empty_allowed: bool,
) -> Result<(), WorkflowError> {
    if values.len() > MAX_PLAN_ITEMS || (!empty_allowed && values.is_empty()) {
        return Err(WorkflowError::InvalidPlanField(field));
    }
    for value in values {
        validate_text(field, value, false)?;
    }
    Ok(())
}

fn validate_paths(paths: &[PathBuf]) -> Result<(), WorkflowError> {
    if paths.is_empty() || paths.len() > MAX_PLAN_ITEMS {
        return Err(WorkflowError::InvalidPlanField("relevant files"));
    }
    let mut seen = BTreeSet::new();
    for path in paths {
        if path.as_os_str().is_empty()
            || path.is_absolute()
            || path
                .components()
                .any(|component| component == Component::ParentDir)
            || !seen.insert(path.clone())
        {
            return Err(WorkflowError::InvalidPlanPath(path.clone()));
        }
    }
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum WorkflowError {
    #[error("workflow objective or plan field `{0}` is empty or exceeds its bound")]
    InvalidPlanField(&'static str),
    #[error("invalid workflow plan path: {0}")]
    InvalidPlanPath(PathBuf),
    #[error("workflow iteration limit must be between 1 and 1000, got {0}")]
    InvalidIterationLimit(u32),
    #[error("workflow repair limit must be at most 100, got {0}")]
    InvalidRepairLimit(u32),
    #[error("invalid workflow transition from {from:?}; expected one of {expected:?}")]
    InvalidTransition {
        from: WorkflowPhase,
        expected: Vec<WorkflowPhase>,
    },
    #[error("workflow plan is required before editing")]
    PlanRequired,
    #[error("at least one active change is required before validation")]
    ChangeRequired,
    #[error("actual validation evidence is required before review")]
    ValidationRequired,
    #[error("the current revision still has failed validation")]
    ValidationFailed,
    #[error("workflow iteration limit reached: {0}")]
    IterationLimit(u32),
    #[error("workflow repair attempt limit reached: {0}")]
    RepairLimit(u32),
    #[error("unknown or already rolled back workflow change: {0}")]
    UnknownChange(String),
    #[error("could not encode workflow evidence: {0}")]
    Evidence(String),
    #[error("completed, blocked, or failed workflows cannot update working memory")]
    TerminalMemoryUpdate,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coding::intelligence::SymbolKind;
    use crate::coding::patch::{FileMutationPreview, MutationOperation};

    fn limits() -> WorkflowLimits {
        WorkflowLimits {
            max_iterations: 10,
            max_repair_attempts: 2,
        }
    }

    fn plan() -> WorkflowPlan {
        WorkflowPlan {
            affected_components: vec!["coding core".to_string()],
            relevant_files: vec![PathBuf::from("src/lib.rs")],
            risks: vec!["behavior regression".to_string()],
            intended_changes: vec!["update implementation".to_string()],
            tests: vec!["unit tests".to_string()],
            validation: vec!["cargo test".to_string()],
            rollback_strategy: "roll back the retained patch".to_string(),
        }
    }

    fn preview(text: &str) -> MutationPreview {
        MutationPreview {
            files: vec![FileMutationPreview {
                path: PathBuf::from("src/lib.rs"),
                operation: MutationOperation::Write,
                original_digest: Some("blake3:before".to_string()),
                new_digest: Some(content_digest(text.as_bytes())),
                diff: text.to_string(),
            }],
        }
    }

    fn output(success: bool, diagnostic: &str) -> ProcessOutput {
        ProcessOutput {
            program: "cargo".to_string(),
            cwd: PathBuf::from("."),
            exit_code: Some(if success { 0 } else { 1 }),
            success,
            timed_out: false,
            stdout: String::new(),
            stderr: diagnostic.to_string(),
            stdout_lossy: false,
            stderr_lossy: false,
            output_truncated: false,
            background_process_detected: false,
            output_collection_error: None,
            diagnostics: crate::coding::diagnostic::DiagnosticReport::default(),
            duration_ms: 5,
        }
    }

    fn planned_workflow() -> CodingWorkflow {
        let mut workflow = CodingWorkflow::new("implement feature".to_string(), limits()).unwrap();
        workflow.note_repository_activity();
        workflow.set_plan(plan()).unwrap();
        workflow.begin_editing().unwrap();
        workflow
    }

    #[test]
    fn completes_only_after_real_change_validation_and_review() {
        let mut workflow = planned_workflow();
        workflow.authorize_change().unwrap();
        workflow
            .record_change("change-1".to_string(), &preview("+new"))
            .unwrap();
        workflow.begin_validation().unwrap();
        workflow
            .record_process("cargo", &["test".to_string()], &output(true, "ok"))
            .unwrap();
        workflow.begin_review().unwrap();
        let report = workflow
            .complete("implemented safely".to_string(), Vec::new())
            .unwrap();

        assert_eq!(report.phase, WorkflowPhase::Completed);
        assert!(report.verified);
        assert_eq!(report.changed_files, vec![PathBuf::from("src/lib.rs")]);
        assert_eq!(report.validations[0].outcome, ValidationOutcome::Passed);
        assert!(!report.validations[0].diagnostic_fingerprint.is_empty());
    }

    #[test]
    fn blocks_repeated_failures_across_bounded_repairs() {
        let mut workflow = CodingWorkflow::new(
            "repair".to_string(),
            WorkflowLimits {
                max_iterations: 10,
                max_repair_attempts: 5,
            },
        )
        .unwrap();
        workflow.set_plan(plan()).unwrap();
        workflow.begin_editing().unwrap();
        for revision in 0..3 {
            workflow.authorize_change().unwrap();
            workflow
                .record_change(
                    format!("change-{revision}"),
                    &preview(&format!("diff-{revision}")),
                )
                .unwrap();
            workflow.begin_validation().unwrap();
            workflow
                .record_process(
                    "cargo",
                    &["test".to_string()],
                    &output(false, "error 123: same failure"),
                )
                .unwrap();
            if revision < 2 {
                workflow.begin_repair().unwrap();
            }
        }

        assert_eq!(workflow.phase(), WorkflowPhase::Blocked);
        assert!(matches!(
            workflow.status().stop_reason,
            Some(WorkflowStopReason::RepeatedFailure { repetitions: 3 })
        ));
    }

    #[test]
    fn enforces_iteration_and_validation_gates() {
        let mut workflow = CodingWorkflow::new(
            "bounded".to_string(),
            WorkflowLimits {
                max_iterations: 1,
                max_repair_attempts: 0,
            },
        )
        .unwrap();
        workflow.set_plan(plan()).unwrap();
        workflow.begin_editing().unwrap();
        workflow.authorize_change().unwrap();
        workflow
            .record_change("first".to_string(), &preview("first"))
            .unwrap();
        workflow.phase = WorkflowPhase::Editing;

        assert!(matches!(
            workflow.authorize_change(),
            Err(WorkflowError::IterationLimit(1))
        ));
        assert_eq!(workflow.phase(), WorkflowPhase::Failed);

        let mut workflow = planned_workflow();
        workflow.authorize_change().unwrap();
        workflow
            .record_change("change".to_string(), &preview("change"))
            .unwrap();
        workflow.begin_validation().unwrap();
        assert!(matches!(
            workflow.begin_review(),
            Err(WorkflowError::ValidationRequired)
        ));
    }

    #[test]
    fn rolling_back_removes_files_from_the_report() {
        let mut workflow = planned_workflow();
        workflow.authorize_change().unwrap();
        workflow
            .record_change("change".to_string(), &preview("change"))
            .unwrap();
        workflow.record_rollback("change").unwrap();

        assert!(workflow.report().changed_files.is_empty());
        assert_eq!(workflow.phase(), WorkflowPhase::Editing);
    }

    #[test]
    fn unavailable_validation_remains_explicitly_unverified() {
        let mut workflow = planned_workflow();
        workflow.authorize_change().unwrap();
        workflow
            .record_change("change".to_string(), &preview("change"))
            .unwrap();
        workflow.begin_validation().unwrap();
        workflow
            .record_validation_execution(&ValidationExecution {
                command: None,
                status: ValidationStatus::NotPresent,
                reason: Some("not discovered".to_string()),
                output: None,
            })
            .unwrap();
        workflow.begin_review().unwrap();
        let report = workflow
            .complete(
                "implemented but validation was unavailable".to_string(),
                vec!["validation command was not present".to_string()],
            )
            .unwrap();

        assert!(!report.verified);
        assert_eq!(report.validations[0].outcome, ValidationOutcome::NotPresent);
    }

    #[test]
    fn retains_bounded_working_memory_without_source_or_diagnostics() {
        let mut workflow = planned_workflow();
        workflow
            .update_memory_notes(
                Some(vec!["repository is writable".to_string()]),
                Some(vec!["run focused tests".to_string()]),
            )
            .unwrap();
        workflow.note_read_files([PathBuf::from("src/lib.rs"), PathBuf::from("src/lib.rs")]);
        workflow.note_symbols([&CodeSymbol {
            path: PathBuf::from("src/lib.rs"),
            name: "target".to_string(),
            qualified_name: "module::target".to_string(),
            kind: SymbolKind::Function,
            line: 12,
            detail: Some("fn target(secret: String)".to_string()),
        }]);
        workflow
            .record_process(
                "cargo",
                &["test".to_string(), "secret-filter".to_string()],
                &output(false, "password=must-not-be-retained"),
            )
            .unwrap();

        let memory = workflow.status().memory;
        assert_eq!(memory.read_files, vec![PathBuf::from("src/lib.rs")]);
        assert_eq!(memory.relevant_symbols[0].name, "target");
        assert_eq!(memory.relevant_symbols[0].qualified_name, "module::target");
        assert_eq!(memory.executed_commands[0].program, "cargo");
        assert_eq!(memory.executed_commands[0].argument_count, 2);
        let serialized = serde_json::to_string(&memory).unwrap();
        assert!(!serialized.contains("secret-filter"));
        assert!(!serialized.contains("must-not-be-retained"));
        assert!(!serialized.contains("fn target"));
    }
}
