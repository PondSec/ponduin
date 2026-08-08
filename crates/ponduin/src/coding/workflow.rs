use crate::coding::file::content_digest;
use crate::coding::intelligence::CodeSymbol;
use crate::coding::patch::MutationPreview;
use crate::coding::process::ProcessOutput;
use crate::coding::review::{ReviewReport, ReviewSeverity};
use crate::coding::validation::{ValidationExecution, ValidationStatus};
use serde::{de::Error as _, Deserialize, Deserializer, Serialize};
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
    id: WorkflowId,
    objective: String,
    task: WorkflowTaskState,
    phase: WorkflowPhase,
    plan: Option<WorkflowPlan>,
    limits: WorkflowLimits,
    iterations: u32,
    repair_attempts: u32,
    repair_pending: bool,
    revision: u32,
    changes: Vec<ChangeEvidence>,
    validations: Vec<ValidationEvidence>,
    invocation_history: VecDeque<InvocationEvidence>,
    failure_counts: BTreeMap<String, usize>,
    non_improving_failures: usize,
    last_error_count: Option<usize>,
    stop_reason: Option<WorkflowStopReason>,
    completion: Option<CompletionDetails>,
    review: Option<ReviewEvidence>,
    memory: WorkflowMemory,
}

impl CodingWorkflow {
    pub fn new(objective: String, limits: WorkflowLimits) -> Result<Self, WorkflowError> {
        Self::new_with_task(
            objective.clone(),
            WorkflowTaskState::new(objective, TaskInteractionMode::Autonomous, PathBuf::new())?,
            limits,
        )
    }

    pub fn new_with_task(
        objective: String,
        task: WorkflowTaskState,
        limits: WorkflowLimits,
    ) -> Result<Self, WorkflowError> {
        limits.validate()?;
        validate_text("objective", &objective, false)?;
        if task.normalized_objective != objective {
            return Err(WorkflowError::TaskObjectiveMismatch);
        }
        Ok(Self {
            id: WorkflowId::new(),
            objective,
            task,
            phase: WorkflowPhase::Analyzing,
            plan: None,
            limits,
            iterations: 0,
            repair_attempts: 0,
            repair_pending: false,
            revision: 0,
            changes: Vec::new(),
            validations: Vec::new(),
            invocation_history: VecDeque::new(),
            failure_counts: BTreeMap::new(),
            non_improving_failures: 0,
            last_error_count: None,
            stop_reason: None,
            completion: None,
            review: None,
            memory: WorkflowMemory::default(),
        })
    }

    pub fn id(&self) -> &WorkflowId {
        &self.id
    }

    pub fn phase(&self) -> WorkflowPhase {
        self.phase
    }

    pub fn next_action(&self) -> WorkflowNextAction {
        match self.phase {
            WorkflowPhase::Analyzing | WorkflowPhase::Searching => WorkflowNextAction::Inspect,
            WorkflowPhase::Planning => WorkflowNextAction::BeginEditing,
            WorkflowPhase::Editing if self.can_begin_review() => WorkflowNextAction::BeginReview,
            WorkflowPhase::Editing if !self.changed_files().is_empty() => {
                WorkflowNextAction::BeginValidation
            }
            WorkflowPhase::Editing => WorkflowNextAction::Modify,
            WorkflowPhase::Testing if self.can_begin_review() => WorkflowNextAction::BeginReview,
            WorkflowPhase::Testing => WorkflowNextAction::Validate,
            WorkflowPhase::Debugging if self.repair_strategy_required() => {
                WorkflowNextAction::SetRepairStrategy
            }
            WorkflowPhase::Debugging => WorkflowNextAction::BeginRepair,
            WorkflowPhase::Reviewing if self.can_complete() => WorkflowNextAction::Complete,
            WorkflowPhase::Reviewing => WorkflowNextAction::Review,
            WorkflowPhase::Completed => WorkflowNextAction::ReturnResult,
            WorkflowPhase::Blocked | WorkflowPhase::Failed => WorkflowNextAction::ReportBlocked,
            WorkflowPhase::Cancelled => WorkflowNextAction::ReportCancelled,
        }
    }

    pub fn is_terminal(&self) -> bool {
        matches!(
            self.phase,
            WorkflowPhase::Completed
                | WorkflowPhase::Blocked
                | WorkflowPhase::Failed
                | WorkflowPhase::Cancelled
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

    pub fn record_tool_contract_failure(&mut self, tool_name: &str, error_class: &str) -> usize {
        if self.is_terminal() {
            return 0;
        }
        let diagnostic_fingerprint =
            content_digest(format!("{tool_name}\0{error_class}").as_bytes());
        let repetitions = self
            .memory
            .tool_contract_errors
            .iter()
            .filter(|error| error.diagnostic_fingerprint == diagnostic_fingerprint)
            .count()
            + 1;
        self.memory
            .tool_contract_errors
            .push(ToolContractErrorEvidence {
                revision: self.revision,
                tool_name: tool_name.to_string(),
                diagnostic_fingerprint,
                repetitions,
            });
        if self.memory.tool_contract_errors.len() > MAX_EVIDENCE_RECORDS {
            self.memory.tool_contract_errors.remove(0);
        }
        if repetitions >= LOOP_THRESHOLD {
            self.block(WorkflowStopReason::RepeatedToolContract {
                tool_name: tool_name.to_string(),
                repetitions,
            });
        }
        repetitions
    }

    pub fn block_for_action_limit(&mut self, limit: u32) {
        if !self.is_terminal() {
            self.block(WorkflowStopReason::ActionLimit { limit });
        }
    }

    pub fn set_repair_strategy(
        &mut self,
        approach: RepairApproach,
        hypothesis: String,
        target_files: Vec<PathBuf>,
    ) -> Result<(), WorkflowError> {
        self.require_phase(&[WorkflowPhase::Debugging])?;
        validate_text("repair hypothesis", &hypothesis, false)?;
        validate_paths(&target_files)?;
        let diagnostic_fingerprint = self
            .current_failed_validation()
            .map(|validation| validation.diagnostic_fingerprint.clone())
            .ok_or(WorkflowError::RepairStrategyWithoutFailure)?;
        if self.memory.repair_strategies.iter().any(|strategy| {
            strategy.diagnostic_fingerprint == diagnostic_fingerprint
                && strategy.approach == approach
        }) {
            return Err(WorkflowError::RepeatedRepairStrategy(approach));
        }
        self.memory.repair_strategies.push(RepairStrategyEvidence {
            revision: self.revision,
            diagnostic_fingerprint,
            approach,
            hypothesis_fingerprint: content_digest(hypothesis.as_bytes()),
            target_files,
        });
        if self.memory.repair_strategies.len() > MAX_EVIDENCE_RECORDS {
            self.memory.repair_strategies.remove(0);
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
        if matches!(self.task.intent, TaskIntent::Inspect | TaskIntent::Verify) {
            return Err(WorkflowError::MutationForbiddenForIntent(self.task.intent));
        }
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
        self.repair_pending = false;
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

        let planned_check_ids = self.matching_check_ids(program, args, &output.cwd);
        let validation = ValidationEvidence::from_process(
            self.revision,
            program,
            args,
            output,
            planned_check_ids,
        );
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
        let planned_check_ids = execution.command.as_ref().map_or_else(Vec::new, |command| {
            self.matching_check_ids(&command.program, &command.args, &command.cwd)
        });
        self.accept_validation(ValidationEvidence::from_execution(
            self.revision,
            execution,
            planned_check_ids,
        ))
    }

    pub fn record_review(&mut self, review: &ReviewReport) -> Result<(), WorkflowError> {
        self.require_phase(&[WorkflowPhase::Reviewing])?;
        let changed_files = self.changed_files().into_iter().collect::<BTreeSet<_>>();
        let reviewed_files = review.files.iter().cloned().collect::<BTreeSet<_>>();
        let blocking_findings = review
            .findings
            .iter()
            .filter(|finding| {
                matches!(
                    finding.severity,
                    ReviewSeverity::Critical | ReviewSeverity::High
                )
            })
            .count();
        self.review = Some(ReviewEvidence {
            revision: self.revision,
            analyzed_patch_fingerprint: review.analyzed_patch_fingerprint.clone(),
            reviewed_files: review.files.clone(),
            finding_count: review.findings.len(),
            blocking_findings,
            complete: changed_files.is_subset(&reviewed_files)
                && !(review.diff_truncated || review.lossy_output || review.truncated),
        });
        if blocking_findings > 0 {
            self.phase = WorkflowPhase::Debugging;
        }
        Ok(())
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
            self.failure_counts.clear();
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
        let diagnostic_fingerprint = self
            .current_failed_validation()
            .map(|validation| validation.diagnostic_fingerprint.clone())
            .ok_or(WorkflowError::RepairStrategyWithoutFailure)?;
        let repetitions = self
            .failure_counts
            .get(&diagnostic_fingerprint)
            .copied()
            .unwrap_or_default();
        if self.repair_strategy_required_for(&diagnostic_fingerprint, repetitions) {
            return Err(WorkflowError::RepairStrategyRequired);
        }
        if self.repair_attempts >= self.limits.max_repair_attempts {
            self.block(WorkflowStopReason::RepairLimit {
                limit: self.limits.max_repair_attempts,
            });
            return Err(WorkflowError::RepairLimit(self.limits.max_repair_attempts));
        }
        self.repair_attempts += 1;
        self.repair_pending = true;
        self.phase = WorkflowPhase::Editing;
        Ok(())
    }

    pub fn begin_review(&mut self) -> Result<(), WorkflowError> {
        let required_checks = self
            .plan
            .as_ref()
            .map(|plan| {
                plan.checks()
                    .into_iter()
                    .filter(|check| check.required)
                    .map(|check| check.id.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        match self.phase {
            WorkflowPhase::Testing => {
                let current = self
                    .validations
                    .iter()
                    .filter(|validation| validation.revision == self.revision)
                    .collect::<Vec<_>>();
                if current.iter().any(|validation| {
                    validation
                        .planned_check_ids
                        .iter()
                        .any(|id| required_checks.contains(id))
                        && !validation.outcome.is_success()
                }) {
                    return Err(WorkflowError::ValidationFailed);
                }
                if required_checks.iter().any(|check_id| {
                    !current.iter().any(|validation| {
                        validation.outcome.is_success()
                            && validation.planned_check_ids.iter().any(|id| id == check_id)
                    })
                }) {
                    return Err(WorkflowError::ValidationRequired);
                }
            }
            WorkflowPhase::Editing if required_checks.is_empty() => {}
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

    pub fn can_complete(&self) -> bool {
        self.phase == WorkflowPhase::Reviewing
            && self
                .review
                .as_ref()
                .is_some_and(|review| review.revision == self.revision && review.complete)
    }

    pub fn cancel(&mut self) {
        if self.is_terminal() {
            return;
        }
        self.phase = WorkflowPhase::Cancelled;
        self.stop_reason = Some(WorkflowStopReason::Cancelled);
    }

    pub fn complete(
        &mut self,
        summary: String,
        remaining_risks: Vec<String>,
    ) -> Result<WorkflowReport, WorkflowError> {
        self.require_phase(&[WorkflowPhase::Reviewing])?;
        let review = self.review.as_ref().ok_or(WorkflowError::ReviewRequired)?;
        if review.revision != self.revision || !review.complete {
            return Err(WorkflowError::ReviewRequired);
        }
        if review.blocking_findings > 0 {
            return Err(WorkflowError::ReviewFailed);
        }
        let requirements = self.requirement_evidence();
        let open_requirements = requirements
            .iter()
            .filter(|requirement| {
                requirement.mandatory && requirement.status != RequirementStatus::Verified
            })
            .map(|requirement| requirement.id.clone())
            .collect::<Vec<_>>();
        if !open_requirements.is_empty() {
            return Err(WorkflowError::MandatoryRequirementsOpen(open_requirements));
        }
        validate_text("completion summary", &summary, false)?;
        validate_items("remaining risks", &remaining_risks, true)?;
        self.completion = Some(CompletionDetails {
            summary,
            remaining_risks,
            requirements,
        });
        self.phase = WorkflowPhase::Completed;
        Ok(self.report())
    }

    pub fn status(&self) -> WorkflowStatus {
        WorkflowStatus {
            id: self.id.clone(),
            objective: self.objective.clone(),
            task: self.task.clone(),
            phase: self.phase,
            next_action: self.next_action(),
            plan: self.plan.clone(),
            iterations: self.iterations,
            max_iterations: self.limits.max_iterations,
            repair_attempts: self.repair_attempts,
            max_repair_attempts: self.limits.max_repair_attempts,
            repair_pending: self.repair_pending,
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
        let requirements = self
            .completion
            .as_ref()
            .map(|completion| completion.requirements.clone())
            .unwrap_or_else(|| self.requirement_evidence());
        let verified = self.phase == WorkflowPhase::Completed
            && requirements
                .iter()
                .filter(|requirement| requirement.mandatory)
                .all(|requirement| requirement.status == RequirementStatus::Verified);
        WorkflowReport {
            id: self.id.clone(),
            objective: self.objective.clone(),
            task: self.task.clone(),
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
            requirements,
            review: self.review.clone(),
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

    fn requirement_evidence(&self) -> Vec<RequirementEvidence> {
        let Some(plan) = &self.plan else {
            return Vec::new();
        };
        let changed_files = self.changed_files().into_iter().collect::<BTreeSet<_>>();
        let current_validations = self
            .validations
            .iter()
            .filter(|validation| validation.revision == self.revision)
            .collect::<Vec<_>>();
        plan.requirements
            .iter()
            .map(|requirement| {
                let files_verified = requirement
                    .verification
                    .expected_files
                    .iter()
                    .all(|path| changed_files.contains(path));
                let matching_checks = current_validations
                    .iter()
                    .filter(|validation| {
                        validation
                            .planned_check_ids
                            .iter()
                            .any(|id| requirement.verification.check_ids.contains(id))
                    })
                    .collect::<Vec<_>>();
                let checks_verified = requirement.verification.check_ids.iter().all(|check_id| {
                    matching_checks.iter().any(|validation| {
                        validation.outcome.is_success()
                            && validation.planned_check_ids.iter().any(|id| id == check_id)
                    })
                });
                let status = if files_verified && checks_verified {
                    RequirementStatus::Verified
                } else if matching_checks
                    .iter()
                    .any(|validation| validation.outcome == ValidationOutcome::Failed)
                {
                    RequirementStatus::Failed
                } else if matching_checks.iter().any(|validation| {
                    matches!(
                        validation.outcome,
                        ValidationOutcome::Blocked
                            | ValidationOutcome::NotExecutable
                            | ValidationOutcome::NotPresent
                            | ValidationOutcome::TimedOut
                            | ValidationOutcome::IncompleteOutput
                    )
                }) {
                    RequirementStatus::Blocked
                } else if files_verified || checks_verified {
                    RequirementStatus::PartiallyVerified
                } else {
                    RequirementStatus::Pending
                };
                RequirementEvidence {
                    id: requirement.id.clone(),
                    description: requirement.description.clone(),
                    mandatory: requirement.mandatory,
                    status,
                    expected_files: requirement.verification.expected_files.clone(),
                    check_ids: requirement.verification.check_ids.clone(),
                }
            })
            .collect()
    }

    fn matching_check_ids(&self, program: &str, args: &[String], cwd: &Path) -> Vec<String> {
        self.plan
            .as_ref()
            .map(|plan| {
                plan.checks()
                    .into_iter()
                    .filter(|check| check.command.matches(program, args, cwd))
                    .map(|check| check.id.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn current_failed_validation(&self) -> Option<&ValidationEvidence> {
        self.validations.iter().rev().find(|validation| {
            validation.revision == self.revision && validation.outcome.requires_repair()
        })
    }

    fn repair_strategy_required(&self) -> bool {
        let Some(diagnostic_fingerprint) = self
            .current_failed_validation()
            .map(|validation| &validation.diagnostic_fingerprint)
        else {
            return false;
        };
        let repetitions = self
            .failure_counts
            .get(diagnostic_fingerprint)
            .copied()
            .unwrap_or_default();
        self.repair_strategy_required_for(diagnostic_fingerprint, repetitions)
    }

    fn repair_strategy_required_for(
        &self,
        diagnostic_fingerprint: &str,
        repetitions: usize,
    ) -> bool {
        repetitions >= 2
            && !self.memory.repair_strategies.iter().any(|strategy| {
                strategy.revision == self.revision
                    && strategy.diagnostic_fingerprint == diagnostic_fingerprint
            })
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

/// A distinct, externally serialized identity for one coding workflow.
///
/// The `workflow_` prefix prevents a rollback UUID or another identifier class
/// from being accepted at the tool boundary as a workflow identifier.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct WorkflowId(String);

impl WorkflowId {
    fn new() -> Self {
        Self(format!("workflow_{}", Uuid::now_v7()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for WorkflowId {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl std::fmt::Display for WorkflowId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for WorkflowId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        let Some(uuid) = value.strip_prefix("workflow_") else {
            return Err(D::Error::custom(
                "workflow_id must start with `workflow_`; rollback and other IDs are not valid workflow IDs",
            ));
        };
        Uuid::parse_str(uuid).map_err(|_| {
            D::Error::custom("workflow_id must contain a valid UUID after `workflow_`")
        })?;
        Ok(Self(value))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskInteractionMode {
    ReadOnly,
    Ask,
    Autonomous,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskIntent {
    Create,
    Modify,
    Repair,
    Inspect,
    Verify,
}

impl TaskIntent {
    pub fn detect(request: &str) -> Self {
        let request = request.to_ascii_lowercase();
        if ["repair", "fix", "beheb", "reparier", "bug"]
            .iter()
            .any(|term| request.contains(term))
        {
            Self::Repair
        } else if ["create", "erstelle", "neu", "new project", "anlegen"]
            .iter()
            .any(|term| request.contains(term))
        {
            Self::Create
        } else if [
            "modify",
            "change",
            "update",
            "edit",
            "implement",
            "add",
            "write",
            "änder",
            "aender",
            "aktualis",
            "anpass",
            "bearbeit",
            "hinzufüg",
            "hinzufueg",
            "schreib",
        ]
        .iter()
        .any(|term| request.contains(term))
        {
            Self::Modify
        } else if [
            "analyse",
            "analys",
            "analyze",
            "untersuche",
            "inspect",
            "read-only",
            "read only",
        ]
        .iter()
        .any(|term| request.contains(term))
        {
            Self::Inspect
        } else if [
            "verify",
            "verif",
            "prüfe",
            "pruefe",
            "testen ohne",
            "test only",
        ]
        .iter()
        .any(|term| request.contains(term))
        {
            Self::Verify
        } else {
            Self::Modify
        }
    }
}

/// Durable, compact task information supplied by the host before a model can
/// simplify the request into a plan. The surrounding workflow owns plans,
/// evidence, errors, and state transitions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowTaskState {
    pub original_user_request: String,
    pub normalized_objective: String,
    pub intent: TaskIntent,
    pub interaction_mode: TaskInteractionMode,
    pub workspace: PathBuf,
}

impl WorkflowTaskState {
    pub fn new(
        original_user_request: String,
        interaction_mode: TaskInteractionMode,
        workspace: PathBuf,
    ) -> Result<Self, WorkflowError> {
        validate_text("original user request", &original_user_request, false)?;
        Ok(Self {
            normalized_objective: original_user_request.clone(),
            intent: TaskIntent::detect(&original_user_request),
            original_user_request,
            interaction_mode,
            workspace,
        })
    }

    pub fn with_objective(mut self, objective: String) -> Result<Self, WorkflowError> {
        validate_text("objective", &objective, false)?;
        self.normalized_objective = objective;
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowPlan {
    pub affected_components: Vec<String>,
    pub relevant_files: Vec<PathBuf>,
    pub risks: Vec<String>,
    pub intended_changes: Vec<String>,
    pub requirements: Vec<WorkflowRequirement>,
    pub tests: Vec<WorkflowCheck>,
    pub validation: Vec<WorkflowCheck>,
    pub rollback_strategy: String,
}

impl WorkflowPlan {
    fn validate(&self) -> Result<(), WorkflowError> {
        validate_items("affected components", &self.affected_components, false)?;
        validate_paths(&self.relevant_files)?;
        validate_items("risks", &self.risks, true)?;
        validate_items("intended changes", &self.intended_changes, false)?;
        let relevant_files = self.relevant_files.iter().cloned().collect::<BTreeSet<_>>();
        validate_requirements(&self.requirements, &relevant_files)?;
        let checks = self.checks();
        validate_checks(&checks)?;
        let check_ids = checks
            .iter()
            .map(|check| check.id.as_str())
            .collect::<BTreeSet<_>>();
        for requirement in &self.requirements {
            if requirement
                .verification
                .check_ids
                .iter()
                .any(|id| !check_ids.contains(id.as_str()))
            {
                return Err(WorkflowError::UnknownRequirementCheck(
                    requirement.id.clone(),
                ));
            }
        }
        validate_text("rollback strategy", &self.rollback_strategy, false)
    }

    fn checks(&self) -> Vec<&WorkflowCheck> {
        self.tests.iter().chain(&self.validation).collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowRequirement {
    pub id: String,
    pub description: String,
    pub source: RequirementSource,
    pub priority: RequirementPriority,
    pub mandatory: bool,
    pub verification: RequirementVerification,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequirementSource {
    User,
    Inferred,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequirementPriority {
    Critical,
    High,
    Normal,
    Low,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequirementVerification {
    #[serde(default)]
    pub expected_files: Vec<PathBuf>,
    #[serde(default)]
    pub check_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowCheck {
    pub id: String,
    pub description: String,
    pub command: WorkflowCommand,
    #[serde(default = "default_required")]
    pub required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowCommand {
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default = "default_cwd")]
    pub cwd: PathBuf,
}

impl WorkflowCommand {
    fn matches(&self, program: &str, args: &[String], cwd: &Path) -> bool {
        self.program == program
            && self.args == args
            && (self.cwd == cwd || (self.cwd == Path::new(".") && cwd.as_os_str().is_empty()))
    }
}

fn default_required() -> bool {
    true
}

fn default_cwd() -> PathBuf {
    PathBuf::from(".")
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
    Cancelled,
}

/// The runtime-derived next action for the workflow's retained task state.
///
/// This is not a second state machine: it is computed from phase, plan,
/// requirement evidence, validation results, and recovery state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowNextAction {
    Inspect,
    BeginEditing,
    Modify,
    BeginValidation,
    Validate,
    SetRepairStrategy,
    BeginRepair,
    BeginReview,
    Review,
    Complete,
    ReturnResult,
    ReportBlocked,
    ReportCancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowStatus {
    pub id: WorkflowId,
    pub objective: String,
    pub task: WorkflowTaskState,
    pub phase: WorkflowPhase,
    pub next_action: WorkflowNextAction,
    pub plan: Option<WorkflowPlan>,
    pub iterations: u32,
    pub max_iterations: u32,
    pub repair_attempts: u32,
    pub max_repair_attempts: u32,
    pub repair_pending: bool,
    pub revision: u32,
    pub changed_files: Vec<PathBuf>,
    pub validation_count: usize,
    pub stop_reason: Option<WorkflowStopReason>,
    #[serde(default)]
    pub memory: WorkflowMemory,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowReport {
    pub id: WorkflowId,
    pub objective: String,
    pub task: WorkflowTaskState,
    pub phase: WorkflowPhase,
    pub changed_files: Vec<PathBuf>,
    pub iterations: u32,
    pub repair_attempts: u32,
    pub validations: Vec<ValidationEvidence>,
    pub verified: bool,
    pub summary: Option<String>,
    pub remaining_risks: Vec<String>,
    pub requirements: Vec<RequirementEvidence>,
    pub review: Option<ReviewEvidence>,
    pub stop_reason: Option<WorkflowStopReason>,
    #[serde(default)]
    pub memory: WorkflowMemory,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequirementEvidence {
    pub id: String,
    pub description: String,
    pub mandatory: bool,
    pub status: RequirementStatus,
    pub expected_files: Vec<PathBuf>,
    pub check_ids: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequirementStatus {
    Pending,
    PartiallyVerified,
    Verified,
    Blocked,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewEvidence {
    pub revision: u32,
    pub analyzed_patch_fingerprint: String,
    pub reviewed_files: Vec<PathBuf>,
    pub finding_count: usize,
    pub blocking_findings: usize,
    pub complete: bool,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowMemory {
    pub assumptions: Vec<String>,
    pub read_files: Vec<PathBuf>,
    pub relevant_symbols: Vec<RelevantSymbolEvidence>,
    pub executed_commands: Vec<CommandEvidence>,
    pub known_errors: Vec<KnownErrorEvidence>,
    pub tool_contract_errors: Vec<ToolContractErrorEvidence>,
    pub repair_strategies: Vec<RepairStrategyEvidence>,
    pub open_points: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolContractErrorEvidence {
    pub revision: u32,
    pub tool_name: String,
    pub diagnostic_fingerprint: String,
    pub repetitions: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepairStrategyEvidence {
    pub revision: u32,
    pub diagnostic_fingerprint: String,
    pub approach: RepairApproach,
    pub hypothesis_fingerprint: String,
    pub target_files: Vec<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepairApproach {
    LocalLogic,
    DependencyBoundary,
    Configuration,
    TestFixture,
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
    #[serde(default)]
    pub planned_check_ids: Vec<String>,
}

impl ValidationEvidence {
    fn from_process(
        revision: u32,
        program: &str,
        args: &[String],
        output: &ProcessOutput,
        planned_check_ids: Vec<String>,
    ) -> Self {
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
            planned_check_ids,
        }
    }

    fn from_execution(
        revision: u32,
        execution: &ValidationExecution,
        planned_check_ids: Vec<String>,
    ) -> Self {
        if let (Some(command), Some(output)) = (&execution.command, &execution.output) {
            let mut evidence = Self::from_process(
                revision,
                &command.program,
                &command.args,
                output,
                planned_check_ids,
            );
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
            planned_check_ids,
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
    Cancelled,
    ActionLimit {
        limit: u32,
    },
    IterationLimit {
        limit: u32,
    },
    RepairLimit {
        limit: u32,
    },
    RepeatedDiff {
        repetitions: usize,
    },
    RepeatedFailure {
        repetitions: usize,
    },
    RepeatedToolCall {
        repetitions: usize,
    },
    RepeatedToolContract {
        tool_name: String,
        repetitions: usize,
    },
    NoDiagnosticProgress {
        attempts: usize,
    },
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
    requirements: Vec<RequirementEvidence>,
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

fn validate_requirements(
    requirements: &[WorkflowRequirement],
    relevant_files: &BTreeSet<PathBuf>,
) -> Result<(), WorkflowError> {
    if requirements.is_empty() || requirements.len() > MAX_PLAN_ITEMS {
        return Err(WorkflowError::InvalidPlanField("requirements"));
    }
    let mut ids = BTreeSet::new();
    for requirement in requirements {
        validate_text("requirement id", &requirement.id, false)?;
        validate_text("requirement description", &requirement.description, false)?;
        if !ids.insert(requirement.id.as_str()) {
            return Err(WorkflowError::DuplicateRequirementId(
                requirement.id.clone(),
            ));
        }
        if requirement.verification.expected_files.is_empty()
            && requirement.verification.check_ids.is_empty()
        {
            return Err(WorkflowError::MissingRequirementVerification(
                requirement.id.clone(),
            ));
        }
        validate_paths_allow_empty(&requirement.verification.expected_files)?;
        if requirement
            .verification
            .expected_files
            .iter()
            .any(|path| !relevant_files.contains(path))
        {
            return Err(WorkflowError::RequirementFileOutsidePlan(
                requirement.id.clone(),
            ));
        }
        validate_items(
            "requirement check ids",
            &requirement.verification.check_ids,
            true,
        )?;
    }
    Ok(())
}

fn validate_checks(checks: &[&WorkflowCheck]) -> Result<(), WorkflowError> {
    if checks.len() > MAX_PLAN_ITEMS {
        return Err(WorkflowError::InvalidPlanField("workflow checks"));
    }
    let mut ids = BTreeSet::new();
    for check in checks {
        validate_text("workflow check id", &check.id, false)?;
        validate_text("workflow check description", &check.description, false)?;
        validate_text("workflow check program", &check.command.program, false)?;
        validate_paths_allow_empty(std::slice::from_ref(&check.command.cwd))?;
        validate_items("workflow check arguments", &check.command.args, true)?;
        if !ids.insert(check.id.as_str()) {
            return Err(WorkflowError::DuplicateCheckId(check.id.clone()));
        }
    }
    Ok(())
}

fn validate_paths_allow_empty(paths: &[PathBuf]) -> Result<(), WorkflowError> {
    if paths.len() > MAX_PLAN_ITEMS {
        return Err(WorkflowError::InvalidPlanField("relevant files"));
    }
    if paths.is_empty() {
        return Ok(());
    }
    validate_paths(paths)
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
    #[error("workflow task objective must match the workflow objective")]
    TaskObjectiveMismatch,
    #[error("a {0:?} task does not permit workspace mutations")]
    MutationForbiddenForIntent(TaskIntent),
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
    #[error("an actual complete review is required before workflow completion")]
    ReviewRequired,
    #[error("the current review contains blocking findings")]
    ReviewFailed,
    #[error("mandatory workflow requirements remain open: {0:?}")]
    MandatoryRequirementsOpen(Vec<String>),
    #[error("workflow requirement IDs must be unique: {0}")]
    DuplicateRequirementId(String),
    #[error("workflow check IDs must be unique: {0}")]
    DuplicateCheckId(String),
    #[error("workflow requirement `{0}` needs a verification method")]
    MissingRequirementVerification(String),
    #[error("workflow requirement `{0}` references a file outside the plan")]
    RequirementFileOutsidePlan(String),
    #[error("workflow requirement `{0}` references an unknown check")]
    UnknownRequirementCheck(String),
    #[error("workflow iteration limit reached: {0}")]
    IterationLimit(u32),
    #[error("workflow repair attempt limit reached: {0}")]
    RepairLimit(u32),
    #[error("a recorded validation failure is required before selecting a repair strategy")]
    RepairStrategyWithoutFailure,
    #[error("a distinct repair strategy is required after a repeated validation failure")]
    RepairStrategyRequired,
    #[error("repair strategy `{0:?}` was already used for the current diagnostic")]
    RepeatedRepairStrategy(RepairApproach),
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
    use crate::coding::review::{ReviewCategory, ReviewFinding};

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
            requirements: vec![WorkflowRequirement {
                id: "implementation".to_string(),
                description: "update the implementation and validate it".to_string(),
                source: RequirementSource::User,
                priority: RequirementPriority::High,
                mandatory: true,
                verification: RequirementVerification {
                    expected_files: vec![PathBuf::from("src/lib.rs")],
                    check_ids: vec!["cargo-test".to_string()],
                },
            }],
            tests: vec![WorkflowCheck {
                id: "cargo-test".to_string(),
                description: "run the unit tests".to_string(),
                command: WorkflowCommand {
                    program: "cargo".to_string(),
                    args: vec!["test".to_string()],
                    cwd: PathBuf::from("."),
                },
                required: true,
            }],
            validation: Vec::new(),
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

    fn review() -> ReviewReport {
        ReviewReport {
            staged: false,
            files: vec![PathBuf::from("src/lib.rs")],
            skipped_sensitive: Vec::new(),
            findings: Vec::new(),
            counts: BTreeMap::new(),
            analyzed_patch_fingerprint: "blake3:review".to_string(),
            diff_truncated: false,
            lossy_output: false,
            truncated: false,
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
        workflow.record_review(&review()).unwrap();
        let report = workflow
            .complete("implemented safely".to_string(), Vec::new())
            .unwrap();

        assert_eq!(report.phase, WorkflowPhase::Completed);
        assert!(report.verified);
        assert_eq!(report.changed_files, vec![PathBuf::from("src/lib.rs")]);
        assert_eq!(report.validations[0].outcome, ValidationOutcome::Passed);
        assert_eq!(report.requirements[0].status, RequirementStatus::Verified);
        assert!(report.review.is_some());
        assert!(!report.validations[0].diagnostic_fingerprint.is_empty());
    }

    #[test]
    fn next_action_follows_evidence_backed_lifecycle() {
        let mut workflow = CodingWorkflow::new("implement feature".to_string(), limits()).unwrap();

        assert_eq!(workflow.status().next_action, WorkflowNextAction::Inspect);
        workflow.set_plan(plan()).unwrap();
        assert_eq!(
            workflow.status().next_action,
            WorkflowNextAction::BeginEditing
        );

        workflow.begin_editing().unwrap();
        assert_eq!(workflow.status().next_action, WorkflowNextAction::Modify);
        workflow.authorize_change().unwrap();
        workflow
            .record_change("change-1".to_string(), &preview("+new"))
            .unwrap();
        assert_eq!(
            workflow.status().next_action,
            WorkflowNextAction::BeginValidation
        );

        workflow.begin_validation().unwrap();
        assert_eq!(workflow.status().next_action, WorkflowNextAction::Validate);
        workflow
            .record_process("cargo", &["test".to_string()], &output(true, "ok"))
            .unwrap();
        assert_eq!(
            workflow.status().next_action,
            WorkflowNextAction::BeginReview
        );

        workflow.begin_review().unwrap();
        assert_eq!(workflow.status().next_action, WorkflowNextAction::Review);
        workflow.record_review(&review()).unwrap();
        assert_eq!(workflow.status().next_action, WorkflowNextAction::Complete);

        workflow
            .complete("implemented safely".to_string(), Vec::new())
            .unwrap();
        assert_eq!(
            workflow.status().next_action,
            WorkflowNextAction::ReturnResult
        );
    }

    #[test]
    fn repeated_failure_requires_a_distinct_repair_strategy_before_retrying() {
        let mut workflow = planned_workflow();
        workflow.authorize_change().unwrap();
        workflow
            .record_change("initial".to_string(), &preview("initial"))
            .unwrap();
        workflow.begin_validation().unwrap();
        workflow
            .record_process("cargo", &["test".to_string()], &output(false, "same error"))
            .unwrap();
        assert_eq!(
            workflow.status().next_action,
            WorkflowNextAction::BeginRepair
        );

        workflow.begin_repair().unwrap();
        workflow.authorize_change().unwrap();
        workflow
            .record_change("retry".to_string(), &preview("retry"))
            .unwrap();
        workflow.begin_validation().unwrap();
        workflow
            .record_process("cargo", &["test".to_string()], &output(false, "same error"))
            .unwrap();
        assert_eq!(
            workflow.status().next_action,
            WorkflowNextAction::SetRepairStrategy
        );

        workflow
            .set_repair_strategy(
                RepairApproach::LocalLogic,
                "correct the local branch".to_string(),
                vec![PathBuf::from("src/lib.rs")],
            )
            .unwrap();
        assert_eq!(
            workflow.status().next_action,
            WorkflowNextAction::BeginRepair
        );
    }

    #[test]
    fn requires_every_mandatory_planned_check_before_review() {
        let mut plan = plan();
        plan.validation.push(WorkflowCheck {
            id: "rustc-version".to_string(),
            description: "confirm the compiler runtime".to_string(),
            command: WorkflowCommand {
                program: "rustc".to_string(),
                args: vec!["--version".to_string()],
                cwd: PathBuf::from("."),
            },
            required: true,
        });
        plan.requirements[0]
            .verification
            .check_ids
            .push("rustc-version".to_string());
        let mut workflow = CodingWorkflow::new("implement feature".to_string(), limits()).unwrap();
        workflow.set_plan(plan).unwrap();
        workflow.begin_editing().unwrap();
        workflow.authorize_change().unwrap();
        workflow
            .record_change("change-1".to_string(), &preview("+new"))
            .unwrap();
        workflow.begin_validation().unwrap();
        workflow
            .record_process("cargo", &["test".to_string()], &output(true, "ok"))
            .unwrap();

        assert!(matches!(
            workflow.begin_review(),
            Err(WorkflowError::ValidationRequired)
        ));
    }

    #[test]
    fn completion_requires_review_and_verified_mandatory_requirements() {
        let mut plan = plan();
        plan.relevant_files.push(PathBuf::from("src/missing.rs"));
        plan.requirements[0]
            .verification
            .expected_files
            .push(PathBuf::from("src/missing.rs"));
        let mut workflow = CodingWorkflow::new("implement feature".to_string(), limits()).unwrap();
        workflow.set_plan(plan).unwrap();
        workflow.begin_editing().unwrap();
        workflow.authorize_change().unwrap();
        workflow
            .record_change("change-1".to_string(), &preview("+new"))
            .unwrap();
        workflow.begin_validation().unwrap();
        workflow
            .record_process("cargo", &["test".to_string()], &output(true, "ok"))
            .unwrap();
        workflow.begin_review().unwrap();

        assert!(matches!(
            workflow.complete("incomplete".to_string(), Vec::new()),
            Err(WorkflowError::ReviewRequired)
        ));

        workflow.record_review(&review()).unwrap();
        assert!(matches!(
            workflow.complete("incomplete".to_string(), Vec::new()),
            Err(WorkflowError::MandatoryRequirementsOpen(requirements))
                if requirements == ["implementation"]
        ));
    }

    #[test]
    fn blocking_review_finding_returns_to_diagnosis() {
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
        let mut review = review();
        review.findings.push(ReviewFinding {
            severity: ReviewSeverity::Critical,
            category: ReviewCategory::Security,
            message: "credential detected".to_string(),
            path: PathBuf::from("src/lib.rs"),
            line: 1,
        });

        workflow.record_review(&review).unwrap();

        assert_eq!(workflow.phase(), WorkflowPhase::Debugging);
        assert!(!workflow.can_complete());
    }

    #[test]
    fn review_must_cover_each_retained_changed_file() {
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
        let mut review = review();
        review.files.clear();

        workflow.record_review(&review).unwrap();

        assert!(!workflow.can_complete());
        assert!(matches!(
            workflow.complete("incomplete review".to_string(), Vec::new()),
            Err(WorkflowError::ReviewRequired)
        ));
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
            if revision == 1 {
                assert!(matches!(
                    workflow.begin_repair(),
                    Err(WorkflowError::RepairStrategyRequired)
                ));
                workflow
                    .set_repair_strategy(
                        RepairApproach::DependencyBoundary,
                        "inspect the failing dependency boundary".to_string(),
                        vec![PathBuf::from("src/lib.rs")],
                    )
                    .unwrap();
                assert!(matches!(
                    workflow.set_repair_strategy(
                        RepairApproach::DependencyBoundary,
                        "repeat the same approach".to_string(),
                        vec![PathBuf::from("src/lib.rs")],
                    ),
                    Err(WorkflowError::RepeatedRepairStrategy(
                        RepairApproach::DependencyBoundary
                    ))
                ));
            }
            if revision < 2 {
                workflow.begin_repair().unwrap();
            }
        }

        assert_eq!(workflow.phase(), WorkflowPhase::Blocked);
        assert!(matches!(
            workflow.status().stop_reason,
            Some(WorkflowStopReason::RepeatedFailure { repetitions: 3 })
        ));
        assert_eq!(
            workflow.status().memory.repair_strategies[0].approach,
            RepairApproach::DependencyBoundary
        );
    }

    #[test]
    fn marks_a_repair_as_pending_until_a_corrective_change_is_recorded() {
        let mut workflow = planned_workflow();
        workflow.authorize_change().unwrap();
        workflow
            .record_change("initial".to_string(), &preview("initial"))
            .unwrap();
        workflow.begin_validation().unwrap();
        workflow
            .record_process("cargo", &["test".to_string()], &output(false, "error"))
            .unwrap();
        workflow.begin_repair().unwrap();

        assert!(workflow.status().repair_pending);

        workflow.authorize_change().unwrap();
        workflow
            .record_change("repair".to_string(), &preview("repair"))
            .unwrap();

        assert!(!workflow.status().repair_pending);
    }

    #[test]
    fn successful_validation_resets_repeated_failure_detection() {
        let mut workflow = planned_workflow();
        workflow.authorize_change().unwrap();
        workflow
            .record_change("initial".to_string(), &preview("initial"))
            .unwrap();
        workflow.begin_validation().unwrap();
        workflow
            .record_process("cargo", &["test".to_string()], &output(false, "error"))
            .unwrap();
        assert_eq!(workflow.failure_counts.len(), 1);

        workflow.begin_repair().unwrap();
        workflow.authorize_change().unwrap();
        workflow
            .record_change("repair".to_string(), &preview("repair"))
            .unwrap();
        workflow.begin_validation().unwrap();
        workflow
            .record_process("cargo", &["test".to_string()], &output(true, "ok"))
            .unwrap();

        assert!(workflow.failure_counts.is_empty());
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
    fn cancellation_is_terminal_and_cannot_be_completed_later() {
        let mut workflow = planned_workflow();
        workflow.cancel();

        assert!(workflow.is_terminal());
        assert_eq!(workflow.phase(), WorkflowPhase::Cancelled);
        assert_eq!(
            workflow.status().next_action,
            WorkflowNextAction::ReportCancelled
        );
        assert!(matches!(
            workflow.status().stop_reason,
            Some(WorkflowStopReason::Cancelled)
        ));
        assert!(matches!(
            workflow.begin_editing(),
            Err(WorkflowError::InvalidTransition { .. })
        ));
        assert!(matches!(
            workflow.complete("must not complete".to_string(), Vec::new()),
            Err(WorkflowError::InvalidTransition { .. })
        ));
    }

    #[test]
    fn unavailable_validation_cannot_close_a_required_validation_plan() {
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
        assert!(matches!(
            workflow.begin_review(),
            Err(WorkflowError::ValidationRequired)
        ));
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

    #[test]
    fn task_state_preserves_original_request_and_blocks_inspection_mutations() {
        let task = WorkflowTaskState::new(
            "Nur analysiere den vorhandenen Fehler.".to_string(),
            TaskInteractionMode::ReadOnly,
            PathBuf::from("/tmp/workspace"),
        )
        .unwrap()
        .with_objective("analyze the existing failure".to_string())
        .unwrap();
        let mut workflow = CodingWorkflow::new_with_task(
            "analyze the existing failure".to_string(),
            task,
            limits(),
        )
        .unwrap();

        workflow.set_plan(plan()).unwrap();
        workflow.begin_editing().unwrap();

        assert!(matches!(
            workflow.authorize_change(),
            Err(WorkflowError::MutationForbiddenForIntent(
                TaskIntent::Inspect
            ))
        ));
        let status = workflow.status();
        assert_eq!(
            status.task.original_user_request,
            "Nur analysiere den vorhandenen Fehler."
        );
        assert_eq!(status.task.interaction_mode, TaskInteractionMode::ReadOnly);
        assert_eq!(status.task.intent, TaskIntent::Inspect);
    }

    #[test]
    fn creation_intent_wins_over_inspection_and_validation_steps() {
        assert_eq!(
            TaskIntent::detect(
                "Create index.html, styles.css, and script.js. Work through inspection and validation."
            ),
            TaskIntent::Create
        );
    }

    #[test]
    fn mutation_intent_wins_over_inspection_and_validation_steps() {
        assert_eq!(
            TaskIntent::detect("Analysiere den Fehler und aktualisiere die Konfiguration."),
            TaskIntent::Modify
        );
        assert_eq!(
            TaskIntent::detect("Inspect the current implementation, then edit the parser."),
            TaskIntent::Modify
        );
    }

    #[test]
    fn blocks_repeated_tool_contract_failures_with_retained_evidence() {
        let mut workflow = CodingWorkflow::new("repair the fixture".to_string(), limits()).unwrap();

        assert_eq!(
            workflow.record_tool_contract_failure("coding__apply_changes", "invalid_params"),
            1
        );
        assert_eq!(
            workflow.record_tool_contract_failure("coding__apply_changes", "invalid_params"),
            2
        );
        assert_eq!(
            workflow.record_tool_contract_failure("coding__apply_changes", "invalid_params"),
            3
        );

        let status = workflow.status();
        assert_eq!(status.phase, WorkflowPhase::Blocked);
        assert_eq!(status.memory.tool_contract_errors.len(), 3);
        assert!(matches!(
            status.stop_reason,
            Some(WorkflowStopReason::RepeatedToolContract {
                tool_name,
                repetitions: 3
            }) if tool_name == "coding__apply_changes"
        ));
    }

    #[test]
    fn workflow_identifiers_reject_rollback_identifier_shape() {
        let workflow = CodingWorkflow::new("change a file".to_string(), limits()).unwrap();
        assert!(workflow.id().as_str().starts_with("workflow_"));
        assert!(
            serde_json::from_str::<WorkflowId>("\"00000000-0000-7000-8000-000000000000\"").is_err()
        );
        assert!(serde_json::from_str::<WorkflowId>(
            "\"rollback_00000000-0000-7000-8000-000000000000\""
        )
        .is_err());
    }
}
