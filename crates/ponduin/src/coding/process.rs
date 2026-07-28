use crate::coding::workspace::{CodingWorkspace, WorkspaceError};
use crate::subprocess::configure_subprocess;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

pub const DEFAULT_PROCESS_OUTPUT_LIMIT: usize = 2 * 1_024 * 1_024;
pub const MAX_PROCESS_OUTPUT_LIMIT: usize = 100 * 1_024 * 1_024;
pub const MAX_PROCESS_TIMEOUT: Duration = Duration::from_secs(3_600);
const MAX_ARGUMENTS: usize = 256;
const MAX_ARGUMENT_BYTES: usize = 256 * 1_024;
const MAX_ENVIRONMENT_ENTRIES: usize = 32;
const MAX_ENVIRONMENT_VALUE_BYTES: usize = 8 * 1_024;
#[cfg(not(test))]
const OUTPUT_DRAIN_TIMEOUT: Duration = Duration::from_secs(5);
#[cfg(test)]
const OUTPUT_DRAIN_TIMEOUT: Duration = Duration::from_millis(250);

#[derive(Debug)]
pub struct ProcessRunner<'workspace> {
    workspace: &'workspace CodingWorkspace,
    limits: ProcessLimits,
}

impl<'workspace> ProcessRunner<'workspace> {
    pub fn new(workspace: &'workspace CodingWorkspace, limits: ProcessLimits) -> Self {
        Self { workspace, limits }
    }

    pub async fn run(&self, request: ProcessRequest) -> Result<ProcessOutput, ProcessError> {
        self.limits.validate()?;
        let validated = self.validate(request)?;
        let started = Instant::now();
        let process_temp =
            tempfile::tempdir().map_err(ProcessError::TemporaryDirectoryUnavailable)?;
        let mut command = Command::new(&validated.program);
        command
            .args(&validated.args)
            .current_dir(&validated.cwd)
            .env_clear()
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        apply_minimal_environment(&mut command, process_temp.path());
        command.envs(&validated.environment);
        configure_subprocess(&mut command);

        let mut child = command
            .spawn()
            .map_err(|source| ProcessError::SpawnFailed {
                program: validated.display_program.clone(),
                source,
            })?;
        let pid = child.id();
        let stdout = child
            .stdout
            .take()
            .ok_or(ProcessError::MissingOutputPipe("stdout"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or(ProcessError::MissingOutputPipe("stderr"))?;
        let budget = Arc::new(Mutex::new(self.limits.output_limit));
        let stdout_task = tokio::spawn(read_bounded(stdout, Arc::clone(&budget)));
        let stderr_task = tokio::spawn(read_bounded(stderr, budget));

        let wait = tokio::time::timeout(self.limits.timeout, child.wait()).await;
        let (status, timed_out) = match wait {
            Ok(result) => (
                Some(result.map_err(|source| ProcessError::WaitFailed {
                    program: validated.display_program.clone(),
                    source,
                })?),
                false,
            ),
            Err(_) => {
                terminate_process_tree(&mut child, pid).await;
                let status = tokio::time::timeout(OUTPUT_DRAIN_TIMEOUT, child.wait())
                    .await
                    .ok()
                    .and_then(Result::ok);
                (status, true)
            }
        };

        let ((stdout, stdout_collection_error), (stderr, stderr_collection_error)) = tokio::join!(
            collect_reader(stdout_task, "stdout"),
            collect_reader(stderr_task, "stderr")
        );
        if stdout_collection_error.is_some() || stderr_collection_error.is_some() {
            terminate_process_tree(&mut child, pid).await;
        }
        let output_collection_error = [stdout_collection_error, stderr_collection_error]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        let background_process_detected = output_collection_error
            .iter()
            .any(|error| error.contains("collection timed out"));
        let success = status.as_ref().is_some_and(|status| status.success())
            && !timed_out
            && output_collection_error.is_empty();

        Ok(ProcessOutput {
            program: validated.display_program,
            cwd: validated.relative_cwd,
            exit_code: status.as_ref().and_then(ExitStatus::code),
            success,
            timed_out,
            stdout: String::from_utf8_lossy(&stdout.bytes).into_owned(),
            stderr: String::from_utf8_lossy(&stderr.bytes).into_owned(),
            stdout_lossy: std::str::from_utf8(&stdout.bytes).is_err(),
            stderr_lossy: std::str::from_utf8(&stderr.bytes).is_err(),
            output_truncated: stdout.truncated || stderr.truncated,
            background_process_detected,
            output_collection_error: if output_collection_error.is_empty() {
                None
            } else {
                Some(output_collection_error.join("; "))
            },
            duration_ms: started.elapsed().as_millis(),
        })
    }

    fn validate(&self, request: ProcessRequest) -> Result<ValidatedProcess, ProcessError> {
        validate_program_text(&request.program)?;
        validate_arguments(&request.args)?;
        validate_environment(&request.environment)?;
        classify_command(&request.program, &request.args)?;

        let cwd = self.workspace.resolve_existing(&request.cwd)?;
        if !cwd.is_dir() {
            return Err(ProcessError::WorkingDirectoryNotDirectory(
                request.cwd.clone(),
            ));
        }
        let relative_cwd = cwd
            .strip_prefix(self.workspace.root())
            .map(Path::to_path_buf)
            .map_err(|_| ProcessError::WorkingDirectoryOutside(cwd.clone()))?;
        let program = resolve_program(self.workspace, &request.program)?;

        Ok(ValidatedProcess {
            display_program: request.program,
            program,
            args: request.args,
            cwd,
            relative_cwd,
            environment: request.environment,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessLimits {
    pub timeout: Duration,
    pub output_limit: usize,
}

impl Default for ProcessLimits {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(120),
            output_limit: DEFAULT_PROCESS_OUTPUT_LIMIT,
        }
    }
}

impl ProcessLimits {
    fn validate(self) -> Result<(), ProcessError> {
        if self.timeout.is_zero() || self.timeout > MAX_PROCESS_TIMEOUT {
            return Err(ProcessError::InvalidTimeout(self.timeout));
        }
        if self.output_limit == 0 || self.output_limit > MAX_PROCESS_OUTPUT_LIMIT {
            return Err(ProcessError::InvalidOutputLimit(self.output_limit));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessRequest {
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default = "default_cwd")]
    pub cwd: PathBuf,
    #[serde(default)]
    pub environment: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessOutput {
    pub program: String,
    pub cwd: PathBuf,
    pub exit_code: Option<i32>,
    pub success: bool,
    pub timed_out: bool,
    pub stdout: String,
    pub stderr: String,
    pub stdout_lossy: bool,
    pub stderr_lossy: bool,
    pub output_truncated: bool,
    pub background_process_detected: bool,
    pub output_collection_error: Option<String>,
    pub duration_ms: u128,
}

struct ValidatedProcess {
    display_program: String,
    program: PathBuf,
    args: Vec<String>,
    cwd: PathBuf,
    relative_cwd: PathBuf,
    environment: BTreeMap<String, String>,
}

#[derive(Default)]
struct BoundedBytes {
    bytes: Vec<u8>,
    truncated: bool,
}

async fn read_bounded(
    mut reader: impl AsyncRead + Unpin,
    remaining: Arc<Mutex<usize>>,
) -> std::io::Result<BoundedBytes> {
    let mut output = BoundedBytes::default();
    let mut chunk = [0_u8; 8 * 1_024];
    loop {
        let read = reader.read(&mut chunk).await?;
        if read == 0 {
            break;
        }
        let retained = {
            let mut remaining = remaining.lock().await;
            let retained = read.min(*remaining);
            *remaining -= retained;
            retained
        };
        output.bytes.extend_from_slice(&chunk[..retained]);
        output.truncated |= retained < read;
    }
    Ok(output)
}

async fn collect_reader(
    mut task: tokio::task::JoinHandle<std::io::Result<BoundedBytes>>,
    stream: &'static str,
) -> (BoundedBytes, Option<String>) {
    match tokio::time::timeout(OUTPUT_DRAIN_TIMEOUT, &mut task).await {
        Ok(Ok(Ok(output))) => (output, None),
        Ok(Ok(Err(error))) => (
            BoundedBytes {
                truncated: true,
                ..BoundedBytes::default()
            },
            Some(format!("{stream}: {error}")),
        ),
        Ok(Err(error)) => (
            BoundedBytes {
                truncated: true,
                ..BoundedBytes::default()
            },
            Some(format!("{stream} task: {error}")),
        ),
        Err(_) => {
            task.abort();
            let _ = task.await;
            (
                BoundedBytes {
                    truncated: true,
                    ..BoundedBytes::default()
                },
                Some(format!("{stream}: collection timed out")),
            )
        }
    }
}

async fn terminate_process_tree(child: &mut Child, pid: Option<u32>) {
    #[cfg(unix)]
    if let Some(pid) = pid.and_then(|pid| i32::try_from(pid).ok()) {
        // configure_subprocess places the child in a process group whose id is
        // the child pid. A negative pid addresses the complete group.
        unsafe {
            libc::kill(-pid, libc::SIGKILL);
        }
    }

    #[cfg(windows)]
    if let Some(pid) = pid {
        let _ = tokio::time::timeout(
            OUTPUT_DRAIN_TIMEOUT,
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
}

fn resolve_program(workspace: &CodingWorkspace, requested: &str) -> Result<PathBuf, ProcessError> {
    let path = Path::new(requested);
    let is_bare_name = path.components().all(|component| {
        matches!(
            component,
            Component::Normal(_) | Component::Prefix(_) | Component::RootDir
        )
    }) && path.file_name() == Some(OsStr::new(requested));
    if is_bare_name {
        Ok(path.to_path_buf())
    } else {
        let resolved = workspace.resolve_existing(path)?;
        if resolved.is_file() {
            Ok(resolved)
        } else {
            Err(ProcessError::ProgramNotFile(path.to_path_buf()))
        }
    }
}

fn validate_program_text(program: &str) -> Result<(), ProcessError> {
    if program.is_empty() || program.len() > 1_024 || program.contains('\0') {
        Err(ProcessError::InvalidProgram(program.to_string()))
    } else {
        Ok(())
    }
}

fn validate_arguments(args: &[String]) -> Result<(), ProcessError> {
    if args.len() > MAX_ARGUMENTS {
        return Err(ProcessError::TooManyArguments {
            count: args.len(),
            limit: MAX_ARGUMENTS,
        });
    }
    let mut total = 0usize;
    for argument in args {
        if argument.contains('\0') {
            return Err(ProcessError::InvalidArgument);
        }
        total = total.saturating_add(argument.len());
        if total > MAX_ARGUMENT_BYTES {
            return Err(ProcessError::ArgumentsTooLarge {
                size: total,
                limit: MAX_ARGUMENT_BYTES,
            });
        }
    }
    Ok(())
}

fn validate_environment(environment: &BTreeMap<String, String>) -> Result<(), ProcessError> {
    if environment.len() > MAX_ENVIRONMENT_ENTRIES {
        return Err(ProcessError::TooManyEnvironmentEntries {
            count: environment.len(),
            limit: MAX_ENVIRONMENT_ENTRIES,
        });
    }
    for (name, value) in environment {
        let normalized = name.to_ascii_uppercase();
        if name.is_empty()
            || !name
                .chars()
                .all(|character| character == '_' || character.is_ascii_alphanumeric())
            || name.contains('=')
            || value.contains('\0')
            || value.len() > MAX_ENVIRONMENT_VALUE_BYTES
        {
            return Err(ProcessError::InvalidEnvironmentEntry(name.clone()));
        }
        if is_sensitive_environment_name(&normalized) {
            return Err(ProcessError::SensitiveEnvironmentEntry(name.clone()));
        }
        if !is_allowed_environment_name(&normalized) {
            return Err(ProcessError::EnvironmentEntryNotAllowed(name.clone()));
        }
    }
    Ok(())
}

fn is_sensitive_environment_name(name: &str) -> bool {
    [
        "TOKEN",
        "SECRET",
        "PASSWORD",
        "PASSWD",
        "CREDENTIAL",
        "COOKIE",
        "AUTH",
        "PRIVATE",
        "ACCESS_KEY",
        "API_KEY",
    ]
    .iter()
    .any(|fragment| name.contains(fragment))
}

fn is_allowed_environment_name(name: &str) -> bool {
    matches!(
        name,
        "CI" | "NO_COLOR"
            | "TERM"
            | "LANG"
            | "LC_ALL"
            | "TZ"
            | "RUST_BACKTRACE"
            | "RUST_LOG"
            | "CARGO_TERM_COLOR"
            | "CARGO_INCREMENTAL"
            | "NODE_ENV"
            | "PYTHONWARNINGS"
            | "PYTHONDONTWRITEBYTECODE"
            | "GOMAXPROCS"
            | "PIP_DISABLE_PIP_VERSION_CHECK"
            | "UV_NO_PROGRESS"
    )
}

fn classify_command(program: &str, args: &[String]) -> Result<(), ProcessError> {
    let executable = Path::new(program)
        .file_stem()
        .and_then(OsStr::to_str)
        .unwrap_or(program)
        .to_ascii_lowercase();
    let always_blocked = [
        "sh",
        "bash",
        "zsh",
        "fish",
        "csh",
        "tcsh",
        "dash",
        "ksh",
        "nu",
        "cmd",
        "powershell",
        "pwsh",
        "sudo",
        "doas",
        "su",
        "passwd",
        "useradd",
        "usermod",
        "userdel",
        "shutdown",
        "reboot",
        "halt",
        "poweroff",
        "mount",
        "umount",
        "fdisk",
        "diskpart",
        "mkfs",
        "iptables",
        "nft",
        "systemctl",
        "launchctl",
        "reg",
        "git",
        "rm",
        "rmdir",
        "del",
        "erase",
        "curl",
        "wget",
        "ssh",
        "scp",
        "sftp",
        "ftp",
        "telnet",
        "nc",
        "netcat",
        "docker",
        "podman",
        "kubectl",
        "helm",
        "terraform",
        "ansible",
        "apt",
        "apt-get",
        "dnf",
        "yum",
        "pacman",
        "brew",
        "winget",
        "choco",
    ];
    if always_blocked.contains(&executable.as_str()) {
        return Err(ProcessError::BlockedCommand {
            program: program.to_string(),
            reason: "command requires a dedicated safer workflow or can affect the host system"
                .to_string(),
        });
    }

    let blocks_external_action = matches!(
        executable.as_str(),
        "npm" | "pnpm" | "yarn" | "cargo" | "twine" | "pip" | "pip3" | "gradle" | "mvn"
    ) && args.iter().any(|argument| {
        matches!(
            argument.to_ascii_lowercase().as_str(),
            "publish" | "deploy" | "release" | "upload"
        )
    });
    if blocks_external_action {
        return Err(ProcessError::BlockedCommand {
            program: program.to_string(),
            reason: "publishing, deployment, release, and upload commands are not local validation"
                .to_string(),
        });
    }

    let interactive_without_arguments = args.is_empty()
        && matches!(
            executable.as_str(),
            "vi" | "vim"
                | "nvim"
                | "nano"
                | "emacs"
                | "less"
                | "more"
                | "man"
                | "top"
                | "htop"
                | "watch"
                | "python"
                | "python3"
                | "node"
                | "irb"
                | "ghci"
                | "jshell"
        );
    if interactive_without_arguments {
        return Err(ProcessError::InteractiveCommand(program.to_string()));
    }
    Ok(())
}

fn apply_minimal_environment(command: &mut Command, process_temp: &Path) {
    const INHERITED: &[&str] = &[
        "PATH",
        "HOME",
        "USER",
        "LOGNAME",
        "LANG",
        "LC_ALL",
        "SYSTEMROOT",
        "WINDIR",
        "COMSPEC",
        "PATHEXT",
        "USERPROFILE",
    ];
    for name in INHERITED {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    command
        .env("TMPDIR", process_temp)
        .env("TEMP", process_temp)
        .env("TMP", process_temp)
        .env("CI", "1")
        .env("TERM", "dumb")
        .env("NO_COLOR", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("PONDUIN_CODING_AGENT", "1");
}

fn default_cwd() -> PathBuf {
    PathBuf::from(".")
}

#[derive(Debug, thiserror::Error)]
pub enum ProcessError {
    #[error(transparent)]
    Workspace(#[from] WorkspaceError),
    #[error("invalid process timeout: {0:?}")]
    InvalidTimeout(Duration),
    #[error("invalid process output limit: {0}")]
    InvalidOutputLimit(usize),
    #[error("invalid executable name")]
    InvalidProgram(String),
    #[error("process has {count} arguments, limit is {limit}")]
    TooManyArguments { count: usize, limit: usize },
    #[error("process arguments contain {size} bytes, limit is {limit}")]
    ArgumentsTooLarge { size: usize, limit: usize },
    #[error("process argument contains a NUL byte")]
    InvalidArgument,
    #[error("process has {count} environment entries, limit is {limit}")]
    TooManyEnvironmentEntries { count: usize, limit: usize },
    #[error("invalid environment entry: {0}")]
    InvalidEnvironmentEntry(String),
    #[error("sensitive environment entry is not accepted: {0}")]
    SensitiveEnvironmentEntry(String),
    #[error("environment entry is not on the coding-process allowlist: {0}")]
    EnvironmentEntryNotAllowed(String),
    #[error("working directory is not a directory: {0}")]
    WorkingDirectoryNotDirectory(PathBuf),
    #[error("working directory resolved outside the workspace: {0}")]
    WorkingDirectoryOutside(PathBuf),
    #[error("workspace-relative executable is not a regular file: {0}")]
    ProgramNotFile(PathBuf),
    #[error("blocked command `{program}`: {reason}")]
    BlockedCommand { program: String, reason: String },
    #[error("interactive command requires unsupported terminal input: {0}")]
    InteractiveCommand(String),
    #[error("could not start `{program}`: {source}")]
    SpawnFailed {
        program: String,
        #[source]
        source: std::io::Error,
    },
    #[error("missing child process {0} pipe")]
    MissingOutputPipe(&'static str),
    #[error("could not create an isolated temporary process directory: {0}")]
    TemporaryDirectoryUnavailable(#[source] std::io::Error),
    #[error("could not wait for `{program}`: {source}")]
    WaitFailed {
        program: String,
        #[source]
        source: std::io::Error,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn runner(
        workspace: &CodingWorkspace,
        timeout: Duration,
        output_limit: usize,
    ) -> ProcessRunner<'_> {
        ProcessRunner::new(
            workspace,
            ProcessLimits {
                timeout,
                output_limit,
            },
        )
    }

    fn request(program: &str, args: &[&str]) -> ProcessRequest {
        ProcessRequest {
            program: program.to_string(),
            args: args
                .iter()
                .map(|argument| (*argument).to_string())
                .collect(),
            cwd: PathBuf::from("."),
            environment: BTreeMap::new(),
        }
    }

    #[cfg(not(windows))]
    fn python_program() -> &'static str {
        "python3"
    }

    #[cfg(windows)]
    fn python_program() -> &'static str {
        "python"
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn captures_separate_output_exit_code_and_workspace_cwd() {
        let temp_dir = tempfile::tempdir().unwrap();
        let workspace = CodingWorkspace::new(temp_dir.path()).unwrap();
        let output = runner(&workspace, Duration::from_secs(2), 4_096)
            .run(request(
                "python3",
                &[
                    "-c",
                    "import os,sys; print(os.getcwd()); print('problem', file=sys.stderr); sys.exit(7)",
                ],
            ))
            .await
            .unwrap();

        assert_eq!(output.exit_code, Some(7));
        assert!(!output.success);
        assert!(!output.timed_out);
        assert!(output.stdout.contains(temp_dir.path().to_str().unwrap()));
        assert_eq!(output.stderr, "problem\n");
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn captures_separate_output_exit_code_and_workspace_cwd() {
        let temp_dir = tempfile::tempdir().unwrap();
        let workspace = CodingWorkspace::new(temp_dir.path()).unwrap();
        let output = runner(&workspace, Duration::from_secs(2), 4_096)
            .run(request(
                "python",
                &[
                    "-c",
                    "import os,sys; print(os.getcwd()); print('problem', file=sys.stderr); sys.exit(7)",
                ],
            ))
            .await
            .unwrap();

        assert_eq!(output.exit_code, Some(7));
        assert!(!output.success);
        assert!(!output.timed_out);
        assert!(output.stdout.contains(temp_dir.path().to_str().unwrap()));
        assert_eq!(output.stderr, "problem\r\n");
    }

    #[tokio::test]
    async fn enforces_a_combined_output_limit_without_deadlock() {
        let temp_dir = tempfile::tempdir().unwrap();
        let workspace = CodingWorkspace::new(temp_dir.path()).unwrap();
        let output = runner(&workspace, Duration::from_secs(5), 1_024)
            .run(request(
                python_program(),
                &[
                    "-c",
                    "import sys; print('o' * 10000); print('e' * 10000, file=sys.stderr)",
                ],
            ))
            .await
            .unwrap();

        assert!(output.output_truncated);
        assert!(output.stdout.len() + output.stderr.len() <= 1_024);
        assert_eq!(output.exit_code, Some(0));
    }

    #[tokio::test]
    async fn kills_a_process_group_at_timeout() {
        let temp_dir = tempfile::tempdir().unwrap();
        let workspace = CodingWorkspace::new(temp_dir.path()).unwrap();
        let output = runner(&workspace, Duration::from_millis(100), 1_024)
            .run(request(
                python_program(),
                &["-c", "import time; time.sleep(30)"],
            ))
            .await
            .unwrap();

        assert!(output.timed_out);
        assert!(!output.success);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn detects_and_terminates_unmanaged_background_processes() {
        let temp_dir = tempfile::tempdir().unwrap();
        let workspace = CodingWorkspace::new(temp_dir.path()).unwrap();
        let output = runner(&workspace, Duration::from_secs(2), 1_024)
            .run(request(
                python_program(),
                &[
                    "-c",
                    r#"import subprocess,sys; subprocess.Popen([sys.executable, "-c", "import pathlib,time; time.sleep(0.6); pathlib.Path('leaked').write_text('bad')"])"#,
                ],
            ))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(700)).await;

        assert!(!output.success);
        assert!(output.background_process_detected);
        assert!(output
            .output_collection_error
            .as_deref()
            .is_some_and(|error| error.contains("collection timed out")));
        assert!(!temp_dir.path().join("leaked").exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn runs_workspace_local_executables_without_shell_parsing() {
        let temp_dir = tempfile::tempdir().unwrap();
        let script = temp_dir.path().join("literal script.py");
        fs::write(
            &script,
            "#!/usr/bin/env python3\nimport sys\nprint(sys.argv[1])\n",
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let workspace = CodingWorkspace::new(temp_dir.path()).unwrap();
        let output = runner(&workspace, Duration::from_secs(2), 1_024)
            .run(request("./literal script.py", &["$(touch injected)"]))
            .await
            .unwrap();

        assert!(output.success);
        assert!(output.stdout.contains("$(touch injected)"));
        assert!(!temp_dir.path().join("injected").exists());
    }

    #[tokio::test]
    async fn blocks_shells_dangerous_commands_interactive_repls_and_secrets() {
        let temp_dir = tempfile::tempdir().unwrap();
        let workspace = CodingWorkspace::new(temp_dir.path()).unwrap();
        let runner = runner(&workspace, Duration::from_secs(2), 1_024);

        for blocked in ["sh", "git", "rm", "sudo", "curl", "docker"] {
            assert!(matches!(
                runner.run(request(blocked, &[])).await,
                Err(ProcessError::BlockedCommand { .. })
            ));
        }
        assert!(matches!(
            runner.run(request(python_program(), &[])).await,
            Err(ProcessError::InteractiveCommand(_))
        ));
        let mut secret = request(python_program(), &["-c", "print('safe')"]);
        secret
            .environment
            .insert("API_TOKEN".to_string(), "secret".to_string());
        assert!(matches!(
            runner.run(secret).await,
            Err(ProcessError::SensitiveEnvironmentEntry(_))
        ));
        let mut path_override = request(python_program(), &["-c", "print('safe')"]);
        path_override
            .environment
            .insert("PATH".to_string(), "/tmp".to_string());
        assert!(matches!(
            runner.run(path_override).await,
            Err(ProcessError::EnvironmentEntryNotAllowed(_))
        ));
    }

    #[tokio::test]
    async fn rejects_external_working_directories_and_executables() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path().join("workspace");
        let outside = temp_dir.path().join("outside");
        fs::create_dir(&root).unwrap();
        fs::create_dir(&outside).unwrap();
        let workspace = CodingWorkspace::new(&root).unwrap();
        let runner = runner(&workspace, Duration::from_secs(2), 1_024);
        let mut external_cwd = request(python_program(), &["-c", "print('safe')"]);
        external_cwd.cwd = outside.clone();

        assert!(matches!(
            runner.run(external_cwd).await,
            Err(ProcessError::Workspace(WorkspaceError::OutsideWorkspace(_)))
        ));
        assert!(matches!(
            runner.run(request(outside.to_str().unwrap(), &[])).await,
            Err(ProcessError::Workspace(WorkspaceError::OutsideWorkspace(_)))
        ));
    }
}
