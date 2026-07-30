use crate::coding::process::{
    ProcessError, ProcessLimits, ProcessOutput, ProcessRequest, ProcessRunner,
};
use crate::coding::project::{ProjectCapabilities, ValidationCommand, ValidationKind};
use crate::coding::workspace::CodingWorkspace;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Executes only commands returned by side-effect-free project discovery.
#[derive(Debug)]
pub struct ValidationService;

impl ValidationService {
    pub async fn run(
        workspace: &CodingWorkspace,
        capabilities: &ProjectCapabilities,
        command_id: &str,
        limits: ProcessLimits,
    ) -> ValidationExecution {
        let command = capabilities
            .projects
            .iter()
            .flat_map(|project| project.validation_commands.iter())
            .find(|command| command.id == command_id);
        let Some(command) = command else {
            return ValidationExecution {
                command: None,
                status: ValidationStatus::NotPresent,
                reason: Some(format!(
                    "validation command id `{command_id}` was not found in current project discovery"
                )),
                output: None,
            };
        };

        let output = ProcessRunner::new(workspace, limits)
            .run(ProcessRequest {
                program: command.program.clone(),
                args: command.args.clone(),
                cwd: command.cwd.clone(),
                environment: BTreeMap::new(),
            })
            .await;
        match output {
            Ok(output) => ValidationExecution {
                command: Some(command.clone()),
                status: status_for_output(&output),
                reason: None,
                output: Some(output),
            },
            Err(error) => ValidationExecution {
                command: Some(command.clone()),
                status: status_for_error(&error),
                reason: Some(error.to_string()),
                output: None,
            },
        }
    }

    pub fn skipped(command: ValidationCommand, reason: String) -> ValidationExecution {
        ValidationExecution {
            command: Some(command),
            status: ValidationStatus::Skipped,
            reason: Some(reason),
            output: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationExecution {
    pub command: Option<ValidationCommand>,
    pub status: ValidationStatus,
    pub reason: Option<String>,
    pub output: Option<ProcessOutput>,
}

impl ValidationExecution {
    pub fn kind(&self) -> Option<ValidationKind> {
        self.command.as_ref().map(|command| command.kind)
    }

    pub fn passed(&self) -> bool {
        self.status == ValidationStatus::Passed
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationStatus {
    Passed,
    Failed,
    NotPresent,
    NotExecutable,
    Skipped,
    Blocked,
    TimedOut,
    IncompleteOutput,
}

fn status_for_output(output: &ProcessOutput) -> ValidationStatus {
    if output.timed_out {
        ValidationStatus::TimedOut
    } else if output.background_process_detected || output.output_collection_error.is_some() {
        ValidationStatus::IncompleteOutput
    } else if output.success {
        ValidationStatus::Passed
    } else {
        ValidationStatus::Failed
    }
}

fn status_for_error(error: &ProcessError) -> ValidationStatus {
    match error {
        ProcessError::SpawnFailed { .. } | ProcessError::ProgramNotFile(_) => {
            ValidationStatus::NotExecutable
        }
        ProcessError::BlockedCommand { .. }
        | ProcessError::InteractiveCommand(_)
        | ProcessError::SensitiveEnvironmentEntry(_)
        | ProcessError::EnvironmentEntryNotAllowed(_)
        | ProcessError::WorkingDirectoryOutside(_)
        | ProcessError::Workspace(_) => ValidationStatus::Blocked,
        _ => ValidationStatus::NotExecutable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coding::project::{DetectedProject, Ecosystem};
    use std::path::PathBuf;
    use std::time::Duration;

    fn command(id: &str, program: &str, args: &[&str]) -> ValidationCommand {
        ValidationCommand {
            id: id.to_string(),
            kind: ValidationKind::Test,
            program: program.to_string(),
            args: args
                .iter()
                .map(|argument| (*argument).to_string())
                .collect(),
            cwd: PathBuf::from("."),
            evidence: "test fixture".to_string(),
        }
    }

    fn capabilities(commands: Vec<ValidationCommand>) -> ProjectCapabilities {
        ProjectCapabilities {
            projects: vec![DetectedProject {
                root: PathBuf::from("."),
                ecosystem: Ecosystem::Rust,
                manifests: vec![PathBuf::from("Cargo.toml")],
                dependencies: Vec::new(),
                dependencies_truncated: false,
                validation_commands: commands,
                warnings: Vec::new(),
            }],
            ci_files: Vec::new(),
            scanned_files: 1,
            truncated: false,
            warnings: Vec::new(),
        }
    }

    fn limits() -> ProcessLimits {
        ProcessLimits {
            timeout: Duration::from_secs(5),
            output_limit: 16 * 1_024,
        }
    }

    #[tokio::test]
    async fn distinguishes_passed_failed_missing_and_not_executable() {
        let temp_dir = tempfile::tempdir().unwrap();
        let workspace = CodingWorkspace::new(temp_dir.path()).unwrap();
        let capabilities = capabilities(vec![
            command("pass", "rustc", &["--version"]),
            command("fail", "rustc", &["--definitely-not-a-rustc-option"]),
            command(
                "missing",
                "ponduin-command-that-does-not-exist",
                &["--version"],
            ),
        ]);

        let passed = ValidationService::run(&workspace, &capabilities, "pass", limits()).await;
        let failed = ValidationService::run(&workspace, &capabilities, "fail", limits()).await;
        let missing = ValidationService::run(&workspace, &capabilities, "missing", limits()).await;
        let absent = ValidationService::run(&workspace, &capabilities, "unknown", limits()).await;

        assert_eq!(passed.status, ValidationStatus::Passed);
        assert!(passed.passed());
        assert_eq!(failed.status, ValidationStatus::Failed);
        assert_eq!(missing.status, ValidationStatus::NotExecutable);
        assert_eq!(absent.status, ValidationStatus::NotPresent);
    }

    #[tokio::test]
    async fn classifies_unsafe_discovered_commands_as_blocked() {
        let temp_dir = tempfile::tempdir().unwrap();
        let workspace = CodingWorkspace::new(temp_dir.path()).unwrap();
        let capabilities = capabilities(vec![command("blocked", "rm", &["file"])]);

        let result = ValidationService::run(&workspace, &capabilities, "blocked", limits()).await;

        assert_eq!(result.status, ValidationStatus::Blocked);
        assert!(result.output.is_none());
    }

    #[test]
    fn represents_policy_skips_without_claiming_success() {
        let result =
            ValidationService::skipped(command("skip", "cargo", &["test"]), "disabled".to_string());

        assert_eq!(result.status, ValidationStatus::Skipped);
        assert!(!result.passed());
    }
}
