use crate::coding::config::CodingConfig;
use crate::coding::{CodingWorkspace, RepositoryInstructions, RepositoryProfile};
use rmcp::model::{
    CallToolRequestParams, CallToolResult, ContentBlock, ErrorCode, ErrorData, Tool,
    ToolAnnotations,
};
use rmcp::object;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::Value;
use std::path::{Path, PathBuf};

pub const CODING_TOOL_PREFIX: &str = "coding__";
pub const REPOSITORY_PROFILE_TOOL_NAME: &str = "coding__repository_profile";
pub const REPOSITORY_INSTRUCTIONS_TOOL_NAME: &str = "coding__repository_instructions";

const DEFAULT_REPOSITORY_FILE_LIMIT: usize = 50_000;
const MAX_REPOSITORY_FILE_LIMIT: usize = 100_000;

pub fn definitions() -> Vec<Tool> {
    vec![repository_profile_tool(), repository_instructions_tool()]
}

pub fn is_reserved_name(name: &str) -> bool {
    name.starts_with(CODING_TOOL_PREFIX)
}

pub fn execute(
    config: &CodingConfig,
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
            json_result(&profile)
        }
        REPOSITORY_INSTRUCTIONS_TOOL_NAME => {
            let params: RepositoryInstructionsParams = parse_arguments(&tool_call)?;
            let instructions = RepositoryInstructions::load_for_path(&workspace, params.path)
                .map_err(|error| invalid_arguments(error.to_string()))?;
            json_result(&instructions)
        }
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

fn read_only_annotations(title: &str) -> ToolAnnotations {
    ToolAnnotations::with_title(title.to_string())
        .read_only(true)
        .destructive(false)
        .idempotent(true)
        .open_world(false)
}

fn parse_arguments<T>(tool_call: &CallToolRequestParams) -> Result<T, ErrorData>
where
    T: DeserializeOwned + Default,
{
    match tool_call.arguments.clone() {
        Some(arguments) => serde_json::from_value(Value::Object(arguments))
            .map_err(|error| invalid_arguments(error.to_string())),
        None => Ok(T::default()),
    }
}

fn json_result(value: &impl serde::Serialize) -> Result<CallToolResult, ErrorData> {
    serde_json::to_string_pretty(value)
        .map(|json| CallToolResult::success(vec![ContentBlock::text(json)]))
        .map_err(|error| internal_error(format!("failed to serialize coding tool result: {error}")))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coding::CodingTaskMode;
    use std::fs;

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

    #[test]
    fn definitions_are_internal_read_only_tools() {
        let tools = definitions();
        assert_eq!(tools.len(), 2);
        assert!(tools
            .iter()
            .all(|tool| is_reserved_name(&tool.name) && tool.annotations.is_some()));
        assert!(tools.iter().all(|tool| {
            tool.annotations
                .as_ref()
                .is_some_and(|annotations| annotations.read_only_hint == Some(true))
        }));
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
}
