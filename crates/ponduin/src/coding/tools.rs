use crate::coding::config::CodingConfig;
use crate::coding::file::{
    FileReadOptions, FileSnapshot, DEFAULT_READ_LIMIT, MAX_READ_LIMIT, MIN_READ_LIMIT,
};
use crate::coding::patch::{
    MutationBatch, MutationResult, PatchEngine, PatchLimits, RollbackRecord,
    DEFAULT_PATCH_BATCH_LIMIT, MAX_PATCH_FILE_LIMIT,
};
use crate::coding::process::{ProcessLimits, ProcessOutput, ProcessRequest, ProcessRunner};
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
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
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

const DEFAULT_REPOSITORY_FILE_LIMIT: usize = 50_000;
const MAX_REPOSITORY_FILE_LIMIT: usize = 100_000;
const MAX_ROLLBACK_RECORDS: usize = 20;
const MAX_ROLLBACK_BYTES: usize = 64 * 1_024 * 1_024;

#[derive(Debug, Default)]
pub(crate) struct CodingToolState {
    rollback_journal: Mutex<VecDeque<RollbackRecord>>,
}

impl CodingToolState {
    fn remember(&self, record: RollbackRecord) {
        let mut journal = self
            .rollback_journal
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        journal.push_back(record);
        while journal.len() > MAX_ROLLBACK_RECORDS
            || journal
                .iter()
                .map(RollbackRecord::retained_bytes)
                .sum::<usize>()
                > MAX_ROLLBACK_BYTES
        {
            journal.pop_front();
        }
    }

    fn find(&self, rollback_id: &str) -> Option<RollbackRecord> {
        self.rollback_journal
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .find(|record| record.id() == rollback_id)
            .cloned()
    }

    fn forget(&self, rollback_id: &str) {
        self.rollback_journal
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|record| record.id() != rollback_id);
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
    ]
}

pub fn is_reserved_name(name: &str) -> bool {
    name.starts_with(CODING_TOOL_PREFIX)
}

pub fn is_process_tool(name: &str) -> bool {
    name == RUN_PROCESS_TOOL_NAME
}

pub async fn execute_process(
    config: &CodingConfig,
    tool_call: CallToolRequestParams,
    working_dir: &Path,
) -> Result<CallToolResult, ErrorData> {
    if !config.tools_enabled() {
        return Err(tool_unavailable(
            "internal coding tools are disabled for this task",
        ));
    }
    if !is_process_tool(&tool_call.name) {
        return Err(invalid_arguments(format!(
            "`{}` is not an internal coding process tool",
            tool_call.name
        )));
    }

    let workspace = CodingWorkspace::new(working_dir).map_err(invalid_workspace)?;
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
    bounded_process_result(output, config.output_limit)
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
            state.remember(applied.rollback);
            json_result(&applied.result, config.output_limit)
        }
        ROLLBACK_CHANGES_TOOL_NAME => {
            let params: RollbackChangesParams = parse_arguments(&tool_call)?;
            let record = state.find(&params.rollback_id).ok_or_else(|| {
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
            json_result(&result, config.output_limit)
        }
        RUN_PROCESS_TOOL_NAME => Err(internal_error(
            "coding process tools require asynchronous dispatch",
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
    fn definitions_distinguish_read_only_and_mutating_tools() {
        let tools = definitions();
        assert_eq!(tools.len(), 9);
        assert!(tools
            .iter()
            .all(|tool| is_reserved_name(&tool.name) && tool.annotations.is_some()));
        for tool in &tools {
            let annotations = tool.annotations.as_ref().unwrap();
            let mutating = matches!(
                tool.name.as_ref(),
                APPLY_CHANGES_TOOL_NAME | ROLLBACK_CHANGES_TOOL_NAME | RUN_PROCESS_TOOL_NAME
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

        let result = execute_process(&enabled_config(), call, temp_dir.path())
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

        let blocked_error = execute_process(&enabled_config(), blocked, temp_dir.path())
            .await
            .unwrap_err();
        let timeout_error = execute_process(&enabled_config(), excessive, temp_dir.path())
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
}
