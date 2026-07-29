use crate::coding::file::content_digest;
use crate::coding::sensitive::is_sensitive_path;
use crate::coding::workspace::{CodingWorkspace, WorkspaceError};
use crate::subprocess::configure_subprocess;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::process::{Child, Command};
use url::Url;

const MAX_HEADER_BYTES: usize = 8 * 1_024;
const MAX_PROTOCOL_MESSAGE_BYTES: usize = 8 * 1_024 * 1_024;
const MAX_SOURCE_BYTES: usize = 2 * 1_024 * 1_024;
const MAX_RESULT_STRING_BYTES: usize = 4 * 1_024;
const MAX_RESULT_DEPTH: usize = 16;
const SHUTDOWN_GRACE: Duration = Duration::from_secs(2);

/// A bounded, opt-in query against a locally installed language server.
#[derive(Debug)]
pub struct LanguageServerClient<'workspace> {
    workspace: &'workspace CodingWorkspace,
    timeout: Duration,
}

impl<'workspace> LanguageServerClient<'workspace> {
    pub fn new(workspace: &'workspace CodingWorkspace, timeout: Duration) -> Self {
        Self { workspace, timeout }
    }

    pub async fn query(
        &self,
        request: LanguageServerQuery,
    ) -> Result<LanguageServerResult, LanguageServerError> {
        request.validate()?;
        if self.timeout.is_zero() {
            return Err(LanguageServerError::InvalidTimeout);
        }

        let path = self.workspace.resolve_existing(&request.path)?;
        if !path.is_file() {
            return Err(LanguageServerError::SourceNotFile(request.path));
        }
        let relative_path = path
            .strip_prefix(self.workspace.root())
            .map(Path::to_path_buf)
            .map_err(|_| LanguageServerError::OutsideWorkspace(path.clone()))?;
        if is_sensitive_path(&relative_path) {
            return Err(LanguageServerError::SensitiveSource(relative_path));
        }
        let source = read_source(&path)?;
        let launch = discover_server(self.workspace, &path)?;
        let root_uri = file_uri(self.workspace.root())?;
        let document_uri = file_uri(&path)?;
        let process_temp =
            tempfile::tempdir().map_err(LanguageServerError::TemporaryDirectoryUnavailable)?;
        let started = Instant::now();

        let mut command = Command::new(&launch.executable);
        command
            .args(launch.arguments)
            .current_dir(self.workspace.root())
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        apply_restricted_environment(&mut command, self.workspace, process_temp.path());
        configure_subprocess(&mut command);

        let mut child = command
            .spawn()
            .map_err(|source| LanguageServerError::Spawn {
                server: launch.name.to_string(),
                source,
            })?;
        let pid = child.id();
        let mut stdin = child
            .stdin
            .take()
            .ok_or(LanguageServerError::MissingPipe("stdin"))?;
        let mut stdout = child
            .stdout
            .take()
            .ok_or(LanguageServerError::MissingPipe("stdout"))?;

        let exchange = exchange(
            &mut stdout,
            &mut stdin,
            ExchangeContext {
                workspace: self.workspace,
                launch: &launch,
                root_uri: &root_uri,
                document_uri: &document_uri,
                source: &source,
                request: &request,
            },
        );
        let result = tokio::time::timeout(self.timeout, exchange).await;
        drop(stdin);
        drop(stdout);

        match result {
            Ok(Ok(mut result)) => {
                result.path = relative_path;
                result.source_digest = content_digest(source.as_bytes());
                result.duration_ms = started.elapsed().as_millis();
                stop_child(&mut child, pid, false).await;
                Ok(result)
            }
            Ok(Err(error)) => {
                stop_child(&mut child, pid, true).await;
                Err(error)
            }
            Err(_) => {
                stop_child(&mut child, pid, true).await;
                Err(LanguageServerError::TimedOut(self.timeout))
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LanguageServerQuery {
    pub path: PathBuf,
    pub operation: LanguageServerOperation,
    #[serde(default)]
    pub position: Option<LanguageServerPosition>,
    #[serde(default = "default_include_declaration")]
    pub include_declaration: bool,
    #[serde(default = "default_max_results")]
    pub max_results: usize,
}

impl LanguageServerQuery {
    fn validate(&self) -> Result<(), LanguageServerError> {
        if self.path.as_os_str().is_empty() {
            return Err(LanguageServerError::EmptyPath);
        }
        if self.max_results == 0 || self.max_results > 1_000 {
            return Err(LanguageServerError::InvalidResultLimit(self.max_results));
        }
        if self.operation.requires_position() && self.position.is_none() {
            return Err(LanguageServerError::MissingPosition(self.operation));
        }
        if let Some(position) = self.position {
            position.validate()?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LanguageServerOperation {
    DocumentSymbols,
    Definition,
    References,
}

impl LanguageServerOperation {
    const fn method(self) -> &'static str {
        match self {
            Self::DocumentSymbols => "textDocument/documentSymbol",
            Self::Definition => "textDocument/definition",
            Self::References => "textDocument/references",
        }
    }

    const fn requires_position(self) -> bool {
        !matches!(self, Self::DocumentSymbols)
    }
}

impl std::fmt::Display for LanguageServerOperation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::DocumentSymbols => "document_symbols",
            Self::Definition => "definition",
            Self::References => "references",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LanguageServerPosition {
    /// One-based source line.
    pub line: u32,
    /// One-based UTF-16 column, matching the LSP default position encoding.
    pub column: u32,
}

impl LanguageServerPosition {
    fn validate(self) -> Result<(), LanguageServerError> {
        if self.line == 0 || self.column == 0 {
            Err(LanguageServerError::InvalidPosition)
        } else {
            Ok(())
        }
    }

    fn to_lsp(self) -> Value {
        serde_json::json!({
            "line": self.line - 1,
            "character": self.column - 1,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LanguageServerResult {
    pub server: String,
    pub operation: LanguageServerOperation,
    pub path: PathBuf,
    pub source_digest: String,
    pub items: Value,
    pub truncated: bool,
    pub omitted_external_locations: usize,
    pub duration_ms: u128,
}

#[derive(Debug, Clone)]
struct ServerLaunch {
    name: &'static str,
    executable: PathBuf,
    arguments: &'static [&'static str],
    language_id: &'static str,
    initialization_options: Value,
}

#[derive(Debug, Clone, Copy)]
struct ServerCandidate {
    name: &'static str,
    program: &'static str,
    arguments: &'static [&'static str],
    extensions: &'static [&'static str],
    language_id: &'static str,
}

const SERVER_CANDIDATES: &[ServerCandidate] = &[
    ServerCandidate {
        name: "rust-analyzer",
        program: "rust-analyzer",
        arguments: &[],
        extensions: &["rs"],
        language_id: "rust",
    },
    ServerCandidate {
        name: "typescript-language-server",
        program: "typescript-language-server",
        arguments: &["--stdio"],
        extensions: &["ts", "tsx", "js", "jsx", "mts", "cts", "mjs", "cjs"],
        language_id: "typescript",
    },
    ServerCandidate {
        name: "pyright",
        program: "pyright-langserver",
        arguments: &["--stdio"],
        extensions: &["py", "pyi"],
        language_id: "python",
    },
    ServerCandidate {
        name: "python-lsp-server",
        program: "pylsp",
        arguments: &[],
        extensions: &["py", "pyi"],
        language_id: "python",
    },
    ServerCandidate {
        name: "gopls",
        program: "gopls",
        arguments: &[],
        extensions: &["go"],
        language_id: "go",
    },
    ServerCandidate {
        name: "clangd",
        program: "clangd",
        arguments: &["--background-index=false", "--header-insertion=never"],
        extensions: &["c", "cc", "cpp", "cxx", "h", "hh", "hpp", "hxx"],
        language_id: "cpp",
    },
    ServerCandidate {
        name: "jdtls",
        program: "jdtls",
        arguments: &[],
        extensions: &["java"],
        language_id: "java",
    },
    ServerCandidate {
        name: "lua-language-server",
        program: "lua-language-server",
        arguments: &[],
        extensions: &["lua"],
        language_id: "lua",
    },
    ServerCandidate {
        name: "ruby-lsp",
        program: "ruby-lsp",
        arguments: &[],
        extensions: &["rb"],
        language_id: "ruby",
    },
    ServerCandidate {
        name: "intelephense",
        program: "intelephense",
        arguments: &["--stdio"],
        extensions: &["php"],
        language_id: "php",
    },
    ServerCandidate {
        name: "kotlin-language-server",
        program: "kotlin-language-server",
        arguments: &[],
        extensions: &["kt", "kts"],
        language_id: "kotlin",
    },
    ServerCandidate {
        name: "sourcekit-lsp",
        program: "sourcekit-lsp",
        arguments: &[],
        extensions: &["swift"],
        language_id: "swift",
    },
];

fn discover_server(
    workspace: &CodingWorkspace,
    path: &Path,
) -> Result<ServerLaunch, LanguageServerError> {
    let extension = path
        .extension()
        .and_then(OsStr::to_str)
        .map(str::to_ascii_lowercase)
        .ok_or_else(|| LanguageServerError::UnsupportedLanguage(path.to_path_buf()))?;
    let candidates = SERVER_CANDIDATES
        .iter()
        .filter(|candidate| candidate.extensions.contains(&extension.as_str()))
        .collect::<Vec<_>>();
    if candidates.is_empty() {
        return Err(LanguageServerError::UnsupportedLanguage(path.to_path_buf()));
    }

    let search_paths = executable_search_paths(workspace);
    for candidate in &candidates {
        if let Some(executable) = find_executable(candidate.program, &search_paths) {
            return Ok(ServerLaunch {
                name: candidate.name,
                executable,
                arguments: candidate.arguments,
                language_id: language_id(candidate.language_id, &extension),
                initialization_options: safe_initialization_options(candidate.name),
            });
        }
    }
    Err(LanguageServerError::ServerUnavailable {
        language: extension,
        candidates: candidates
            .into_iter()
            .map(|candidate| candidate.program)
            .collect(),
    })
}

fn language_id(default: &'static str, extension: &str) -> &'static str {
    match extension {
        "tsx" => "typescriptreact",
        "jsx" => "javascriptreact",
        "js" | "mjs" | "cjs" => "javascript",
        "c" | "h" => "c",
        _ => default,
    }
}

fn safe_initialization_options(server: &str) -> Value {
    if server == "rust-analyzer" {
        serde_json::json!({
            "cargo": {
                "buildScripts": {"enable": false}
            },
            "procMacro": {"enable": false},
            "checkOnSave": false
        })
    } else {
        Value::Null
    }
}

fn executable_search_paths(workspace: &CodingWorkspace) -> Vec<PathBuf> {
    std::env::var_os("PATH")
        .map(|value| {
            std::env::split_paths(&value)
                .filter(|path| path.is_absolute())
                .filter_map(|path| path.canonicalize().ok())
                .filter(|path| !path.starts_with(workspace.root()))
                .collect()
        })
        .unwrap_or_default()
}

fn find_executable(program: &str, search_paths: &[PathBuf]) -> Option<PathBuf> {
    #[cfg(windows)]
    const EXTENSIONS: &[&str] = &["", ".exe", ".cmd", ".bat", ".com"];
    #[cfg(not(windows))]
    const EXTENSIONS: &[&str] = &[""];

    for directory in search_paths {
        for extension in EXTENSIONS {
            let candidate = directory.join(format!("{program}{extension}"));
            let Ok(metadata) = candidate.metadata() else {
                continue;
            };
            if !metadata.is_file() {
                continue;
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if metadata.permissions().mode() & 0o111 == 0 {
                    continue;
                }
            }
            if let Ok(candidate) = candidate.canonicalize() {
                return Some(candidate);
            }
        }
    }
    None
}

struct ExchangeContext<'a> {
    workspace: &'a CodingWorkspace,
    launch: &'a ServerLaunch,
    root_uri: &'a str,
    document_uri: &'a str,
    source: &'a str,
    request: &'a LanguageServerQuery,
}

async fn exchange<R, W>(
    reader: &mut R,
    writer: &mut W,
    context: ExchangeContext<'_>,
) -> Result<LanguageServerResult, LanguageServerError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let ExchangeContext {
        workspace,
        launch,
        root_uri,
        document_uri,
        source,
        request,
    } = context;
    let initialize_id = 1_u64;
    send_message(
        writer,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": initialize_id,
            "method": "initialize",
            "params": {
                "processId": Value::Null,
                "clientInfo": {
                    "name": "ponduin-internal-coding-agent",
                    "version": env!("CARGO_PKG_VERSION")
                },
                "rootUri": root_uri,
                "workspaceFolders": [{
                    "uri": root_uri,
                    "name": workspace.root().file_name().and_then(OsStr::to_str).unwrap_or("workspace")
                }],
                "capabilities": {
                    "general": {
                        "positionEncodings": ["utf-16"]
                    },
                    "textDocument": {
                        "documentSymbol": {},
                        "definition": {},
                        "references": {}
                    }
                },
                "initializationOptions": launch.initialization_options
            }
        }),
    )
    .await?;
    read_response(reader, writer, initialize_id, root_uri).await?;
    send_message(
        writer,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "method": "initialized",
            "params": {}
        }),
    )
    .await?;
    send_message(
        writer,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didOpen",
            "params": {
                "textDocument": {
                    "uri": document_uri,
                    "languageId": launch.language_id,
                    "version": 1,
                    "text": source
                }
            }
        }),
    )
    .await?;

    let query_id = 2_u64;
    let params = match request.operation {
        LanguageServerOperation::DocumentSymbols => serde_json::json!({
            "textDocument": {"uri": document_uri}
        }),
        LanguageServerOperation::Definition => serde_json::json!({
            "textDocument": {"uri": document_uri},
            "position": request.position.expect("validated position").to_lsp()
        }),
        LanguageServerOperation::References => serde_json::json!({
            "textDocument": {"uri": document_uri},
            "position": request.position.expect("validated position").to_lsp(),
            "context": {"includeDeclaration": request.include_declaration}
        }),
    };
    send_message(
        writer,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": query_id,
            "method": request.operation.method(),
            "params": params
        }),
    )
    .await?;
    let raw_result = read_response(reader, writer, query_id, root_uri).await?;
    let sanitized = sanitize_result(workspace, raw_result, request.max_results);

    let shutdown_id = 3_u64;
    send_message(
        writer,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": shutdown_id,
            "method": "shutdown",
            "params": Value::Null
        }),
    )
    .await?;
    read_response(reader, writer, shutdown_id, root_uri).await?;
    send_message(
        writer,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "method": "exit",
            "params": Value::Null
        }),
    )
    .await?;

    Ok(LanguageServerResult {
        server: launch.name.to_string(),
        operation: request.operation,
        path: PathBuf::new(),
        source_digest: String::new(),
        items: sanitized.value,
        truncated: sanitized.truncated,
        omitted_external_locations: sanitized.omitted_external_locations,
        duration_ms: 0,
    })
}

async fn read_response<R, W>(
    reader: &mut R,
    writer: &mut W,
    expected_id: u64,
    root_uri: &str,
) -> Result<Value, LanguageServerError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    loop {
        let message = read_message(reader).await?;
        if message.get("method").is_none()
            && message.get("id").and_then(Value::as_u64) == Some(expected_id)
        {
            if let Some(error) = message.get("error") {
                return Err(LanguageServerError::ProtocolResponse {
                    id: expected_id,
                    error: truncate_string(&error.to_string(), MAX_RESULT_STRING_BYTES),
                });
            }
            return Ok(message.get("result").cloned().unwrap_or(Value::Null));
        }
        if message.get("method").is_some() && message.get("id").is_some() {
            respond_to_server_request(writer, &message, root_uri).await?;
        }
    }
}

async fn respond_to_server_request<W>(
    writer: &mut W,
    request: &Value,
    root_uri: &str,
) -> Result<(), LanguageServerError>
where
    W: AsyncWrite + Unpin,
{
    let method = request.get("method").and_then(Value::as_str).unwrap_or("");
    let result = match method {
        "workspace/configuration" => {
            let count = request
                .pointer("/params/items")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            Value::Array((0..count).map(|_| Value::Null).collect())
        }
        "workspace/workspaceFolders" => serde_json::json!([{
            "uri": root_uri,
            "name": "workspace"
        }]),
        _ => Value::Null,
    };
    send_message(
        writer,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": request.get("id").cloned().unwrap_or(Value::Null),
            "result": result
        }),
    )
    .await
}

async fn send_message<W>(writer: &mut W, value: &Value) -> Result<(), LanguageServerError>
where
    W: AsyncWrite + Unpin,
{
    let body = serde_json::to_vec(value).map_err(LanguageServerError::Serialize)?;
    if body.len() > MAX_PROTOCOL_MESSAGE_BYTES {
        return Err(LanguageServerError::MessageTooLarge(body.len()));
    }
    writer
        .write_all(format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes())
        .await
        .map_err(LanguageServerError::ProtocolIo)?;
    writer
        .write_all(&body)
        .await
        .map_err(LanguageServerError::ProtocolIo)?;
    writer
        .flush()
        .await
        .map_err(LanguageServerError::ProtocolIo)
}

async fn read_message<R>(reader: &mut R) -> Result<Value, LanguageServerError>
where
    R: AsyncRead + Unpin,
{
    let mut header = Vec::new();
    while !header.ends_with(b"\r\n\r\n") {
        if header.len() == MAX_HEADER_BYTES {
            return Err(LanguageServerError::HeaderTooLarge);
        }
        let mut byte = [0_u8; 1];
        let read = reader
            .read(&mut byte)
            .await
            .map_err(LanguageServerError::ProtocolIo)?;
        if read == 0 {
            return Err(LanguageServerError::UnexpectedEof);
        }
        header.push(byte[0]);
    }
    let header =
        std::str::from_utf8(&header).map_err(|_| LanguageServerError::InvalidHeaderEncoding)?;
    let content_length = header
        .split("\r\n")
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())
                .flatten()
        })
        .ok_or(LanguageServerError::MissingContentLength)?;
    if content_length > MAX_PROTOCOL_MESSAGE_BYTES {
        return Err(LanguageServerError::MessageTooLarge(content_length));
    }
    let mut body = vec![0_u8; content_length];
    reader
        .read_exact(&mut body)
        .await
        .map_err(LanguageServerError::ProtocolIo)?;
    serde_json::from_slice(&body).map_err(LanguageServerError::Deserialize)
}

struct SanitizedResult {
    value: Value,
    truncated: bool,
    omitted_external_locations: usize,
}

fn sanitize_result(
    workspace: &CodingWorkspace,
    value: Value,
    max_results: usize,
) -> SanitizedResult {
    let mut state = SanitizerState {
        remaining_nodes: max_results.saturating_mul(32).max(32),
        truncated: false,
        omitted_external_locations: 0,
    };
    let value = sanitize_value(workspace, value, 0, Some(max_results), &mut state)
        .unwrap_or_else(|| Value::Array(Vec::new()));
    SanitizedResult {
        value,
        truncated: state.truncated,
        omitted_external_locations: state.omitted_external_locations,
    }
}

struct SanitizerState {
    remaining_nodes: usize,
    truncated: bool,
    omitted_external_locations: usize,
}

fn sanitize_value(
    workspace: &CodingWorkspace,
    value: Value,
    depth: usize,
    array_limit: Option<usize>,
    state: &mut SanitizerState,
) -> Option<Value> {
    if depth > MAX_RESULT_DEPTH || state.remaining_nodes == 0 {
        state.truncated = true;
        return None;
    }
    state.remaining_nodes -= 1;
    match value {
        Value::String(value) => Some(Value::String(truncate_string(
            &value,
            MAX_RESULT_STRING_BYTES,
        ))),
        Value::Array(values) => {
            let limit = array_limit.unwrap_or(values.len());
            state.truncated |= values.len() > limit;
            Some(Value::Array(
                values
                    .into_iter()
                    .take(limit)
                    .filter_map(|value| sanitize_value(workspace, value, depth + 1, None, state))
                    .collect(),
            ))
        }
        Value::Object(values) => {
            if points_outside_workspace(workspace, &values) {
                state.omitted_external_locations += 1;
                return None;
            }
            let mut sanitized = Map::new();
            for (key, value) in values {
                if key == "data" {
                    state.truncated = true;
                    continue;
                }
                if let Some(value) = sanitize_value(workspace, value, depth + 1, None, state) {
                    sanitized.insert(key, value);
                }
            }
            Some(Value::Object(sanitized))
        }
        scalar => Some(scalar),
    }
}

fn points_outside_workspace(workspace: &CodingWorkspace, object: &Map<String, Value>) -> bool {
    ["uri", "targetUri"].into_iter().any(|key| {
        let Some(uri) = object.get(key).and_then(Value::as_str) else {
            return false;
        };
        let Ok(uri) = Url::parse(uri) else {
            return true;
        };
        let Ok(path) = uri.to_file_path() else {
            return true;
        };
        match path.canonicalize() {
            Ok(path) => !path.starts_with(workspace.root()),
            Err(_) => true,
        }
    })
}

fn truncate_string(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let mut boundary = max_bytes;
    while !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    format!(
        "{}…[truncated]",
        value
            .get(..boundary)
            .expect("boundary was adjusted to valid UTF-8")
    )
}

fn read_source(path: &Path) -> Result<String, LanguageServerError> {
    let metadata = path
        .metadata()
        .map_err(|source| LanguageServerError::Read {
            path: path.to_path_buf(),
            source,
        })?;
    if metadata.len() > MAX_SOURCE_BYTES as u64 {
        return Err(LanguageServerError::SourceTooLarge {
            path: path.to_path_buf(),
            size: metadata.len(),
            limit: MAX_SOURCE_BYTES,
        });
    }
    let bytes = std::fs::read(path).map_err(|source| LanguageServerError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    if bytes.contains(&0) {
        return Err(LanguageServerError::BinarySource(path.to_path_buf()));
    }
    String::from_utf8(bytes).map_err(|_| LanguageServerError::NonUtf8Source(path.to_path_buf()))
}

fn file_uri(path: &Path) -> Result<String, LanguageServerError> {
    Url::from_file_path(path)
        .map(String::from)
        .map_err(|_| LanguageServerError::InvalidFileUri(path.to_path_buf()))
}

fn apply_restricted_environment(
    command: &mut Command,
    workspace: &CodingWorkspace,
    process_temp: &Path,
) {
    let search_paths = executable_search_paths(workspace);
    if let Ok(path) = std::env::join_paths(search_paths) {
        command.env("PATH", path);
    }
    for name in [
        "LANG",
        "LC_ALL",
        "SYSTEMROOT",
        "WINDIR",
        "COMSPEC",
        "PATHEXT",
    ] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    command
        .env("HOME", process_temp)
        .env("USERPROFILE", process_temp)
        .env("TMPDIR", process_temp)
        .env("TEMP", process_temp)
        .env("TMP", process_temp)
        .env("CI", "1")
        .env("TERM", "dumb")
        .env("NO_COLOR", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("PONDUIN_CODING_AGENT", "1")
        .env("PONDUIN_LSP_SAFE_MODE", "1");
}

async fn stop_child(child: &mut Child, pid: Option<u32>, force: bool) {
    if !force {
        if let Ok(Ok(_)) = tokio::time::timeout(SHUTDOWN_GRACE, child.wait()).await {
            return;
        }
    }
    #[cfg(unix)]
    if let Some(pid) = pid.and_then(|pid| i32::try_from(pid).ok()) {
        unsafe {
            libc::kill(-pid, libc::SIGKILL);
        }
    }
    #[cfg(windows)]
    if let Some(pid) = pid {
        let _ = tokio::time::timeout(
            SHUTDOWN_GRACE,
            Command::new("taskkill")
                .args(["/T", "/F", "/PID", &pid.to_string()])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status(),
        )
        .await;
    }
    let _ = child.kill().await;
    let _ = tokio::time::timeout(SHUTDOWN_GRACE, child.wait()).await;
}

const fn default_include_declaration() -> bool {
    true
}

const fn default_max_results() -> usize {
    100
}

#[derive(Debug, thiserror::Error)]
pub enum LanguageServerError {
    #[error(transparent)]
    Workspace(#[from] WorkspaceError),
    #[error("language-server query path cannot be empty")]
    EmptyPath,
    #[error("language-server query source is not a file: {0}")]
    SourceNotFile(PathBuf),
    #[error("language-server query source is outside the workspace: {0}")]
    OutsideWorkspace(PathBuf),
    #[error("language-server access to sensitive source is blocked: {0}")]
    SensitiveSource(PathBuf),
    #[error("language-server query result limit must be between 1 and 1000, got {0}")]
    InvalidResultLimit(usize),
    #[error("{0} requires a one-based source position")]
    MissingPosition(LanguageServerOperation),
    #[error("language-server positions use one-based line and column values")]
    InvalidPosition,
    #[error("language-server timeout must be greater than zero")]
    InvalidTimeout,
    #[error("no supported language server is known for {0}")]
    UnsupportedLanguage(PathBuf),
    #[error(
        "no local language server is available for {language}; install one of: {candidates:?}"
    )]
    ServerUnavailable {
        language: String,
        candidates: Vec<&'static str>,
    },
    #[error("language-server source exceeds {limit} bytes at {path} ({size} bytes)")]
    SourceTooLarge {
        path: PathBuf,
        size: u64,
        limit: usize,
    },
    #[error("language-server source appears binary: {0}")]
    BinarySource(PathBuf),
    #[error("language-server source is not UTF-8: {0}")]
    NonUtf8Source(PathBuf),
    #[error("failed to read language-server source {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("cannot represent path as a file URI: {0}")]
    InvalidFileUri(PathBuf),
    #[error("failed to create restricted language-server temporary directory: {0}")]
    TemporaryDirectoryUnavailable(std::io::Error),
    #[error("failed to start local language server {server}: {source}")]
    Spawn {
        server: String,
        #[source]
        source: std::io::Error,
    },
    #[error("language-server process is missing its {0} pipe")]
    MissingPipe(&'static str),
    #[error("language-server query timed out after {0:?}")]
    TimedOut(Duration),
    #[error("language-server protocol I/O failed: {0}")]
    ProtocolIo(std::io::Error),
    #[error("language-server protocol ended unexpectedly")]
    UnexpectedEof,
    #[error("language-server protocol header exceeds its limit")]
    HeaderTooLarge,
    #[error("language-server protocol header is not UTF-8")]
    InvalidHeaderEncoding,
    #[error("language-server response is missing Content-Length")]
    MissingContentLength,
    #[error("language-server protocol message exceeds its limit ({0} bytes)")]
    MessageTooLarge(usize),
    #[error("failed to encode language-server request: {0}")]
    Serialize(serde_json::Error),
    #[error("failed to decode language-server response: {0}")]
    Deserialize(serde_json::Error),
    #[error("language server returned an error for request {id}: {error}")]
    ProtocolResponse { id: u64, error: String },
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn selects_known_servers_and_ignores_non_executables() {
        let temp_dir = tempfile::tempdir().unwrap();
        let executable = temp_dir.path().join("rust-analyzer");
        fs::write(&executable, "").unwrap();
        assert!(find_executable("rust-analyzer", &[temp_dir.path().to_path_buf()]).is_none());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
            assert_eq!(
                find_executable("rust-analyzer", &[temp_dir.path().to_path_buf()]),
                Some(executable.canonicalize().unwrap())
            );
        }
    }

    #[test]
    fn rejects_invalid_queries_before_starting_a_server() {
        let missing_position = LanguageServerQuery {
            path: PathBuf::from("lib.rs"),
            operation: LanguageServerOperation::References,
            position: None,
            include_declaration: true,
            max_results: 10,
        };
        assert!(matches!(
            missing_position.validate(),
            Err(LanguageServerError::MissingPosition(
                LanguageServerOperation::References
            ))
        ));

        let invalid_limit = LanguageServerQuery {
            operation: LanguageServerOperation::DocumentSymbols,
            max_results: 0,
            ..missing_position
        };
        assert!(matches!(
            invalid_limit.validate(),
            Err(LanguageServerError::InvalidResultLimit(0))
        ));
    }

    #[test]
    fn sanitizes_external_locations_and_bounds_results() {
        let temp_dir = tempfile::tempdir().unwrap();
        fs::write(temp_dir.path().join("lib.rs"), "fn local() {}\n").unwrap();
        let workspace = CodingWorkspace::new(temp_dir.path()).unwrap();
        let local_uri = file_uri(&temp_dir.path().join("lib.rs")).unwrap();
        let external_dir = tempfile::tempdir().unwrap();
        fs::write(external_dir.path().join("external.rs"), "").unwrap();
        let external_uri = file_uri(&external_dir.path().join("external.rs")).unwrap();
        let value = serde_json::json!([
            {"uri": local_uri, "name": "local", "data": {"unbounded": true}},
            {"uri": external_uri, "name": "external"},
            {"name": "excess"}
        ]);

        let sanitized = sanitize_result(&workspace, value, 2);

        assert!(sanitized.truncated);
        assert_eq!(sanitized.omitted_external_locations, 1);
        assert_eq!(sanitized.value.as_array().unwrap().len(), 1);
        assert!(sanitized.value[0].get("data").is_none());
    }

    #[tokio::test]
    async fn completes_a_framed_json_rpc_query_lifecycle() {
        let temp_dir = tempfile::tempdir().unwrap();
        fs::write(
            temp_dir.path().join("lib.rs"),
            "fn answer() -> u32 { 42 }\n",
        )
        .unwrap();
        let workspace = CodingWorkspace::new(temp_dir.path()).unwrap();
        let root_uri = file_uri(workspace.root()).unwrap();
        let document_uri = file_uri(&temp_dir.path().join("lib.rs")).unwrap();
        let source = fs::read_to_string(temp_dir.path().join("lib.rs")).unwrap();
        let launch = ServerLaunch {
            name: "fake-lsp",
            executable: PathBuf::from("unused"),
            arguments: &[],
            language_id: "rust",
            initialization_options: Value::Null,
        };
        let request = LanguageServerQuery {
            path: PathBuf::from("lib.rs"),
            operation: LanguageServerOperation::DocumentSymbols,
            position: None,
            include_declaration: true,
            max_results: 10,
        };
        let (client, server) = tokio::io::duplex(64 * 1_024);
        let (mut client_reader, mut client_writer) = tokio::io::split(client);
        let (mut server_reader, mut server_writer) = tokio::io::split(server);

        let server_task = tokio::spawn(async move {
            let initialize = read_message(&mut server_reader).await.unwrap();
            assert_eq!(initialize["method"], "initialize");
            send_message(
                &mut server_writer,
                &serde_json::json!({
                    "jsonrpc":"2.0",
                    "id":1,
                    "method":"workspace/configuration",
                    "params":{"items":[{"section":"safe"}]}
                }),
            )
            .await
            .unwrap();
            let configuration = read_message(&mut server_reader).await.unwrap();
            assert_eq!(configuration["id"], 1);
            assert_eq!(configuration["result"].as_array().unwrap().len(), 1);
            send_message(
                &mut server_writer,
                &serde_json::json!({"jsonrpc":"2.0","id":1,"result":{"capabilities":{}}}),
            )
            .await
            .unwrap();
            assert_eq!(
                read_message(&mut server_reader).await.unwrap()["method"],
                "initialized"
            );
            assert_eq!(
                read_message(&mut server_reader).await.unwrap()["method"],
                "textDocument/didOpen"
            );
            let query = read_message(&mut server_reader).await.unwrap();
            assert_eq!(query["method"], "textDocument/documentSymbol");
            send_message(
                &mut server_writer,
                &serde_json::json!({
                    "jsonrpc":"2.0",
                    "id":2,
                    "result":[{"name":"answer","kind":12,"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":25}}}]
                }),
            )
            .await
            .unwrap();
            assert_eq!(
                read_message(&mut server_reader).await.unwrap()["method"],
                "shutdown"
            );
            send_message(
                &mut server_writer,
                &serde_json::json!({"jsonrpc":"2.0","id":3,"result":null}),
            )
            .await
            .unwrap();
            assert_eq!(
                read_message(&mut server_reader).await.unwrap()["method"],
                "exit"
            );
        });

        let result = exchange(
            &mut client_reader,
            &mut client_writer,
            ExchangeContext {
                workspace: &workspace,
                launch: &launch,
                root_uri: &root_uri,
                document_uri: &document_uri,
                source: &source,
                request: &request,
            },
        )
        .await
        .unwrap();
        server_task.await.unwrap();

        assert_eq!(result.server, "fake-lsp");
        assert_eq!(result.items[0]["name"], "answer");
        assert!(!result.truncated);
    }
}
