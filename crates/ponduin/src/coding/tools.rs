use crate::coding::config::CodingConfig;
use crate::coding::search::{SearchLimits, TextSearchRequest};
use crate::coding::{CodingWorkspace, RepositoryInstructions, RepositoryProfile, RepositorySearch};
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
pub const FIND_FILES_TOOL_NAME: &str = "coding__find_files";
pub const SEARCH_TEXT_TOOL_NAME: &str = "coding__search_text";

const DEFAULT_REPOSITORY_FILE_LIMIT: usize = 50_000;
const MAX_REPOSITORY_FILE_LIMIT: usize = 100_000;

pub fn definitions() -> Vec<Tool> {
    vec![
        repository_profile_tool(),
        repository_instructions_tool(),
        find_files_tool(),
        search_text_tool(),
    ]
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
            json_result(&result)
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
            json_result(&result)
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

fn read_only_annotations(title: &str) -> ToolAnnotations {
    ToolAnnotations::with_title(title.to_string())
        .read_only(true)
        .destructive(false)
        .idempotent(true)
        .open_world(false)
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
        assert_eq!(tools.len(), 4);
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
}
