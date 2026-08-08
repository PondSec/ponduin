use rmcp::model::{ErrorCode, ErrorData};
use serde::{Deserialize, Serialize};

/// A semantic result for a coding action. Detailed process, validation, and
/// mutation evidence remains in the subsystem that produced it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionOutcome {
    Succeeded,
    Failed,
}

/// Stable failure categories used by the coding runtime, independent of a
/// provider's prose error messages.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionFailureKind {
    InvalidArguments,
    WorkflowStateViolation,
    StaleState,
    ResourceMissing,
    ValidationFailed,
    ProcessFailed,
    TimedOut,
    CapabilityUnavailable,
    PermissionRequired,
    PolicyBlocked,
    Cancelled,
    TransientFailure,
    RepeatedFailure,
    #[default]
    InternalFailure,
}

impl ActionFailureKind {
    pub const fn retryability(self) -> Retryability {
        match self {
            Self::InvalidArguments => Retryability::Correctable,
            Self::WorkflowStateViolation => Retryability::RequiresStateChange,
            Self::StaleState => Retryability::RequiresStateRefresh,
            Self::ResourceMissing | Self::CapabilityUnavailable => {
                Retryability::AlternativeRequired
            }
            Self::ValidationFailed
            | Self::ProcessFailed
            | Self::TimedOut
            | Self::RepeatedFailure => Retryability::RequiresStrategyChange,
            Self::PermissionRequired => Retryability::RequiresApproval,
            Self::TransientFailure => Retryability::MayRetry,
            Self::PolicyBlocked | Self::Cancelled | Self::InternalFailure => Retryability::Never,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Retryability {
    Never,
    Correctable,
    RequiresStateChange,
    RequiresStateRefresh,
    AlternativeRequired,
    RequiresStrategyChange,
    RequiresApproval,
    MayRetry,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionResult {
    pub outcome: ActionOutcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure_kind: Option<ActionFailureKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retryability: Option<Retryability>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recovery_decision: Option<RecoveryDecision>,
    pub state_changed: bool,
    /// Signals that the producing subsystem retained its detailed evidence.
    pub evidence_recorded: bool,
}

impl ActionResult {
    pub const fn succeeded(state_changed: bool, evidence_recorded: bool) -> Self {
        Self {
            outcome: ActionOutcome::Succeeded,
            failure_kind: None,
            retryability: None,
            recovery_decision: None,
            state_changed,
            evidence_recorded,
        }
    }

    pub const fn failed(kind: ActionFailureKind, evidence_recorded: bool) -> Self {
        Self {
            outcome: ActionOutcome::Failed,
            failure_kind: Some(kind),
            retryability: Some(kind.retryability()),
            recovery_decision: None,
            state_changed: false,
            evidence_recorded,
        }
    }
}

impl Default for ActionResult {
    fn default() -> Self {
        Self::failed(ActionFailureKind::InternalFailure, false)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryPhase {
    Inspecting,
    Editing,
    Validating,
    Repairing,
    Terminal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecoveryContext {
    pub phase: RecoveryPhase,
    pub repetitions: usize,
    pub alternative_available: bool,
    pub strategy_change_required: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryDecision {
    RetryWithCorrectedArguments,
    RefreshState,
    ReinspectResource,
    InspectDiagnostics,
    Replan,
    UseAlternativeCapability,
    ChangeStrategy,
    RequestApproval,
    StopBlocked,
    StopCancelled,
    StopFailed,
}

/// The one recovery mapping for coding action failures. It deliberately uses
/// semantic results and retained workflow context rather than diagnostics text.
pub fn decide_recovery(failure: ActionFailureKind, context: RecoveryContext) -> RecoveryDecision {
    if failure == ActionFailureKind::Cancelled {
        return RecoveryDecision::StopCancelled;
    }
    if context.phase == RecoveryPhase::Terminal {
        return RecoveryDecision::StopFailed;
    }
    match failure {
        ActionFailureKind::InvalidArguments => RecoveryDecision::RetryWithCorrectedArguments,
        ActionFailureKind::WorkflowStateViolation => RecoveryDecision::Replan,
        ActionFailureKind::StaleState => RecoveryDecision::RefreshState,
        ActionFailureKind::ResourceMissing => RecoveryDecision::ReinspectResource,
        ActionFailureKind::ValidationFailed
        | ActionFailureKind::ProcessFailed
        | ActionFailureKind::TimedOut => {
            if context.strategy_change_required || context.repetitions > 1 {
                RecoveryDecision::ChangeStrategy
            } else {
                RecoveryDecision::InspectDiagnostics
            }
        }
        ActionFailureKind::CapabilityUnavailable => {
            if context.alternative_available {
                RecoveryDecision::UseAlternativeCapability
            } else {
                RecoveryDecision::StopBlocked
            }
        }
        ActionFailureKind::PermissionRequired => RecoveryDecision::RequestApproval,
        ActionFailureKind::PolicyBlocked => RecoveryDecision::StopBlocked,
        ActionFailureKind::TransientFailure => {
            if context.repetitions > 1 {
                RecoveryDecision::InspectDiagnostics
            } else {
                RecoveryDecision::RefreshState
            }
        }
        ActionFailureKind::RepeatedFailure => RecoveryDecision::ChangeStrategy,
        ActionFailureKind::InternalFailure | ActionFailureKind::Cancelled => {
            RecoveryDecision::StopFailed
        }
    }
}

pub(crate) fn error_with_action(
    code: ErrorCode,
    message: impl Into<String>,
    result: ActionResult,
) -> ErrorData {
    let data = serde_json::to_value(result).unwrap_or(serde_json::Value::Null);
    ErrorData::new(code, message.into(), Some(data))
}

pub(crate) fn action_result_from_error(error: &ErrorData) -> ActionResult {
    error
        .data
        .as_ref()
        .and_then(|data| {
            serde_json::from_value(data.clone()).ok().or_else(|| {
                data.get("action")
                    .and_then(|action| serde_json::from_value(action.clone()).ok())
            })
        })
        .unwrap_or_else(|| {
            let kind = match error.code {
                ErrorCode::INVALID_PARAMS | ErrorCode::PARSE_ERROR => {
                    ActionFailureKind::InvalidArguments
                }
                ErrorCode::INVALID_REQUEST | ErrorCode::RESOURCE_NOT_FOUND => {
                    ActionFailureKind::CapabilityUnavailable
                }
                _ => ActionFailureKind::InternalFailure,
            };
            ActionResult::failed(kind, false)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovery_mapping_requires_state_or_strategy_changes() {
        let first_attempt = RecoveryContext {
            phase: RecoveryPhase::Validating,
            repetitions: 1,
            alternative_available: false,
            strategy_change_required: false,
        };
        assert_eq!(
            decide_recovery(ActionFailureKind::StaleState, first_attempt),
            RecoveryDecision::RefreshState
        );
        assert_eq!(
            decide_recovery(ActionFailureKind::ValidationFailed, first_attempt),
            RecoveryDecision::InspectDiagnostics
        );
        assert_eq!(
            decide_recovery(
                ActionFailureKind::ValidationFailed,
                RecoveryContext {
                    repetitions: 2,
                    ..first_attempt
                }
            ),
            RecoveryDecision::ChangeStrategy
        );
        assert_eq!(
            decide_recovery(ActionFailureKind::Cancelled, first_attempt),
            RecoveryDecision::StopCancelled
        );
    }

    #[test]
    fn recovery_mapping_stops_or_escalates_without_diagnostic_text() {
        let context = RecoveryContext {
            phase: RecoveryPhase::Editing,
            repetitions: 1,
            alternative_available: false,
            strategy_change_required: false,
        };

        assert_eq!(
            decide_recovery(ActionFailureKind::InvalidArguments, context),
            RecoveryDecision::RetryWithCorrectedArguments
        );
        assert_eq!(
            decide_recovery(ActionFailureKind::WorkflowStateViolation, context),
            RecoveryDecision::Replan
        );
        assert_eq!(
            decide_recovery(ActionFailureKind::ResourceMissing, context),
            RecoveryDecision::ReinspectResource
        );
        assert_eq!(
            decide_recovery(ActionFailureKind::CapabilityUnavailable, context),
            RecoveryDecision::StopBlocked
        );
        assert_eq!(
            decide_recovery(
                ActionFailureKind::CapabilityUnavailable,
                RecoveryContext {
                    alternative_available: true,
                    ..context
                }
            ),
            RecoveryDecision::UseAlternativeCapability
        );
        assert_eq!(
            decide_recovery(ActionFailureKind::PermissionRequired, context),
            RecoveryDecision::RequestApproval
        );
        assert_eq!(
            decide_recovery(ActionFailureKind::PolicyBlocked, context),
            RecoveryDecision::StopBlocked
        );
        assert_eq!(
            decide_recovery(ActionFailureKind::RepeatedFailure, context),
            RecoveryDecision::ChangeStrategy
        );
        assert_eq!(
            decide_recovery(ActionFailureKind::InternalFailure, context),
            RecoveryDecision::StopFailed
        );
    }

    #[test]
    fn unknown_error_data_fails_safely() {
        let error = ErrorData::new(ErrorCode::INTERNAL_ERROR, "unclassified diagnostic", None);
        let action = action_result_from_error(&error);

        assert_eq!(
            action.failure_kind,
            Some(ActionFailureKind::InternalFailure)
        );
        assert_eq!(
            decide_recovery(
                action
                    .failure_kind
                    .expect("fallback error has a semantic kind"),
                RecoveryContext {
                    phase: RecoveryPhase::Inspecting,
                    repetitions: 0,
                    alternative_available: false,
                    strategy_change_required: false,
                },
            ),
            RecoveryDecision::StopFailed
        );
    }
}
