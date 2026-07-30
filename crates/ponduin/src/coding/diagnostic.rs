use crate::coding::file::content_digest;
use crate::coding::sensitive::is_sensitive_path;
use crate::coding::workspace::CodingWorkspace;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;

const MAX_DIAGNOSTICS: usize = 200;
const MAX_DIAGNOSTIC_BYTES: usize = 2 * 1_024 * 1_024;
const MAX_MESSAGE_BYTES: usize = 2_000;

static LOCATION_PATTERN: OnceLock<Regex> = OnceLock::new();
static RUST_HEADER_PATTERN: OnceLock<Regex> = OnceLock::new();
static PYTHON_FILE_PATTERN: OnceLock<Regex> = OnceLock::new();
static PYTHON_ERROR_PATTERN: OnceLock<Regex> = OnceLock::new();
static SECRET_ASSIGNMENT_PATTERN: OnceLock<Regex> = OnceLock::new();
static BEARER_PATTERN: OnceLock<Regex> = OnceLock::new();
static ACCESS_KEY_PATTERN: OnceLock<Regex> = OnceLock::new();

/// Bounded parser for compiler, test, typechecker, and traceback output.
#[derive(Debug)]
pub struct DiagnosticAnalyzer;

impl DiagnosticAnalyzer {
    pub fn analyze(workspace: &CodingWorkspace, stdout: &str, stderr: &str) -> DiagnosticReport {
        let mut diagnostics = Vec::new();
        let mut truncated = false;
        parse_stream(
            workspace,
            stdout,
            DiagnosticSource::Stdout,
            &mut diagnostics,
            &mut truncated,
        );
        parse_stream(
            workspace,
            stderr,
            DiagnosticSource::Stderr,
            &mut diagnostics,
            &mut truncated,
        );
        let mut seen = BTreeSet::new();
        diagnostics.retain(|diagnostic| {
            seen.insert((
                diagnostic.path.clone(),
                diagnostic.line,
                diagnostic.column,
                diagnostic.severity,
                diagnostic.message.clone(),
            ))
        });
        if diagnostics.len() > MAX_DIAGNOSTICS {
            diagnostics.truncate(MAX_DIAGNOSTICS);
            truncated = true;
        }
        let error_count = diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.severity == DiagnosticSeverity::Error)
            .count();
        let warning_count = diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.severity == DiagnosticSeverity::Warning)
            .count();
        let encoded = serde_json::to_vec(&diagnostics).unwrap_or_default();
        DiagnosticReport {
            diagnostics,
            error_count,
            warning_count,
            truncated,
            fingerprint: content_digest(&encoded),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosticReport {
    pub diagnostics: Vec<Diagnostic>,
    pub error_count: usize,
    pub warning_count: usize,
    pub truncated: bool,
    pub fingerprint: String,
}

impl Default for DiagnosticReport {
    fn default() -> Self {
        Self {
            diagnostics: Vec::new(),
            error_count: 0,
            warning_count: 0,
            truncated: false,
            fingerprint: content_digest(&[]),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub severity: DiagnosticSeverity,
    pub message: String,
    pub code: Option<String>,
    pub path: Option<PathBuf>,
    pub line: Option<usize>,
    pub column: Option<usize>,
    pub source: DiagnosticSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticSeverity {
    Error,
    Warning,
    Note,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticSource {
    Stdout,
    Stderr,
}

fn parse_stream(
    workspace: &CodingWorkspace,
    text: &str,
    source: DiagnosticSource,
    diagnostics: &mut Vec<Diagnostic>,
    truncated: &mut bool,
) {
    let text = bounded_text(text, truncated);
    let mut pending_rust: Option<(DiagnosticSeverity, String, Option<String>)> = None;
    let mut python_location: Option<(Option<PathBuf>, usize)> = None;
    for raw_line in text.lines() {
        if diagnostics.len() >= MAX_DIAGNOSTICS {
            *truncated = true;
            return;
        }
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }

        if let Some(captures) = rust_header_pattern().captures(line) {
            pending_rust = Some((
                severity(captures.name("severity").map(|value| value.as_str())),
                redact(captures.name("message").map_or("", |value| value.as_str())),
                captures
                    .name("code")
                    .map(|value| value.as_str().to_string()),
            ));
            continue;
        }
        if let Some(location) = line.strip_prefix("-->") {
            if let Some((severity, message, code)) = pending_rust.take() {
                let (path, line, column) = parse_location(workspace, location.trim());
                diagnostics.push(Diagnostic {
                    severity,
                    message,
                    code,
                    path,
                    line,
                    column,
                    source,
                });
            }
            continue;
        }
        if let Some(captures) = location_pattern().captures(line) {
            diagnostics.push(Diagnostic {
                severity: severity(captures.name("severity").map(|value| value.as_str())),
                message: redact(captures.name("message").map_or("", |value| value.as_str())),
                code: captures
                    .name("code")
                    .or_else(|| captures.name("code2"))
                    .map(|value| value.as_str().to_string()),
                path: captures
                    .name("path")
                    .and_then(|value| safe_diagnostic_path(workspace, value.as_str())),
                line: parse_number(captures.name("line").map(|value| value.as_str())),
                column: parse_number(captures.name("column").map(|value| value.as_str())),
                source,
            });
            continue;
        }
        if let Some(captures) = python_file_pattern().captures(line) {
            python_location = Some((
                captures
                    .name("path")
                    .and_then(|value| safe_diagnostic_path(workspace, value.as_str())),
                parse_number(captures.name("line").map(|value| value.as_str())).unwrap_or(1),
            ));
            continue;
        }
        if let Some(captures) = python_error_pattern().captures(line) {
            let (path, line) = python_location.take().unwrap_or((None, 1));
            diagnostics.push(Diagnostic {
                severity: DiagnosticSeverity::Error,
                message: redact(captures.name("message").map_or("", |value| value.as_str())),
                code: captures
                    .name("code")
                    .map(|value| value.as_str().to_string()),
                path,
                line: Some(line),
                column: None,
                source,
            });
            continue;
        }
        if let Some((severity, message, code)) = pending_rust.take() {
            diagnostics.push(Diagnostic {
                severity,
                message,
                code,
                path: None,
                line: None,
                column: None,
                source,
            });
            continue;
        }
        if let Some((severity, message)) = generic_diagnostic(line) {
            diagnostics.push(Diagnostic {
                severity,
                message: redact(message),
                code: None,
                path: None,
                line: None,
                column: None,
                source,
            });
        }
    }
    if let Some((severity, message, code)) = pending_rust {
        diagnostics.push(Diagnostic {
            severity,
            message,
            code,
            path: None,
            line: None,
            column: None,
            source,
        });
    }
}

fn bounded_text<'a>(text: &'a str, truncated: &mut bool) -> &'a str {
    if text.len() <= MAX_DIAGNOSTIC_BYTES {
        return text;
    }
    *truncated = true;
    let mut boundary = MAX_DIAGNOSTIC_BYTES;
    while !text.is_char_boundary(boundary) {
        boundary -= 1;
    }
    text.get(..boundary).unwrap_or("")
}

fn parse_location(
    workspace: &CodingWorkspace,
    location: &str,
) -> (Option<PathBuf>, Option<usize>, Option<usize>) {
    let mut parts = location.rsplitn(3, ':');
    let column = parts.next().and_then(|value| value.parse().ok());
    let line = parts.next().and_then(|value| value.parse().ok());
    let path = parts
        .next()
        .and_then(|value| safe_diagnostic_path(workspace, value));
    (path, line, column)
}

fn safe_diagnostic_path(workspace: &CodingWorkspace, value: &str) -> Option<PathBuf> {
    let path = Path::new(value.trim_matches(['"', '\'', '(', ')']));
    let relative = if path.is_absolute() {
        let resolved = path.canonicalize().ok()?;
        resolved.strip_prefix(workspace.root()).ok()?.to_path_buf()
    } else {
        if path
            .components()
            .any(|component| component == Component::ParentDir)
        {
            return None;
        }
        path.to_path_buf()
    };
    (!relative.as_os_str().is_empty() && !is_sensitive_path(&relative)).then_some(relative)
}

fn generic_diagnostic(line: &str) -> Option<(DiagnosticSeverity, &str)> {
    let normalized = line.to_ascii_lowercase();
    for (needle, severity) in [
        ("error:", DiagnosticSeverity::Error),
        ("error ", DiagnosticSeverity::Error),
        ("failed:", DiagnosticSeverity::Error),
        ("panic:", DiagnosticSeverity::Error),
        ("warning:", DiagnosticSeverity::Warning),
        ("warn:", DiagnosticSeverity::Warning),
    ] {
        if normalized.starts_with(needle) {
            return Some((severity, line.get(needle.len()..)?.trim()));
        }
    }
    None
}

fn severity(value: Option<&str>) -> DiagnosticSeverity {
    match value.unwrap_or("").to_ascii_lowercase().as_str() {
        "warning" | "warn" => DiagnosticSeverity::Warning,
        "note" | "info" => DiagnosticSeverity::Note,
        _ => DiagnosticSeverity::Error,
    }
}

fn parse_number(value: Option<&str>) -> Option<usize> {
    value.and_then(|value| value.parse().ok())
}

fn redact(value: &str) -> String {
    let mut redacted = if value.to_ascii_uppercase().contains("PRIVATE KEY") {
        "[REDACTED PRIVATE KEY MATERIAL]".to_string()
    } else {
        secret_assignment_pattern()
            .replace_all(value, "${name}=[REDACTED]")
            .into_owned()
    };
    redacted = bearer_pattern()
        .replace_all(&redacted, "Bearer [REDACTED]")
        .into_owned();
    redacted = access_key_pattern()
        .replace_all(&redacted, "[REDACTED ACCESS KEY]")
        .into_owned();
    truncate_message(redacted)
}

fn truncate_message(mut message: String) -> String {
    if message.len() <= MAX_MESSAGE_BYTES {
        return message;
    }
    let mut boundary = MAX_MESSAGE_BYTES;
    while !message.is_char_boundary(boundary) {
        boundary -= 1;
    }
    message.truncate(boundary);
    message
}

fn location_pattern() -> &'static Regex {
    LOCATION_PATTERN.get_or_init(|| {
        Regex::new(
            r"^(?P<path>(?:[A-Za-z]:)?[^:\n]+):(?P<line>\d+):(?P<column>\d+):\s*(?P<severity>error|warning|warn|note)(?:\[(?P<code>[^\]]+)\]|\s+(?P<code2>[A-Za-z]+\d+))?:\s*(?P<message>.+)$",
        )
        .expect("diagnostic location regex is valid")
    })
}

fn rust_header_pattern() -> &'static Regex {
    RUST_HEADER_PATTERN.get_or_init(|| {
        Regex::new(r"^(?P<severity>error|warning)(?:\[(?P<code>[^\]]+)\])?:\s*(?P<message>.+)$")
            .expect("Rust diagnostic regex is valid")
    })
}

fn python_file_pattern() -> &'static Regex {
    PYTHON_FILE_PATTERN.get_or_init(|| {
        Regex::new(r#"^File ["'](?P<path>.+)["'], line (?P<line>\d+)"#)
            .expect("Python file regex is valid")
    })
}

fn python_error_pattern() -> &'static Regex {
    PYTHON_ERROR_PATTERN.get_or_init(|| {
        Regex::new(r"^(?P<code>[A-Za-z_][A-Za-z0-9_]*(?:Error|Exception)):\s*(?P<message>.+)$")
            .expect("Python error regex is valid")
    })
}

fn secret_assignment_pattern() -> &'static Regex {
    SECRET_ASSIGNMENT_PATTERN.get_or_init(|| {
        Regex::new(
            r#"(?i)(?P<name>(?:api[_-]?key|token|secret|password|passwd|credential))\s*[:=]\s*(?:"[^"]*"|'[^']*'|[^\s,;]+)"#,
        )
        .expect("secret assignment regex is valid")
    })
}

fn bearer_pattern() -> &'static Regex {
    BEARER_PATTERN.get_or_init(|| {
        Regex::new(r"(?i)\bBearer\s+[A-Za-z0-9._~+/=-]+").expect("Bearer regex is valid")
    })
}

fn access_key_pattern() -> &'static Regex {
    ACCESS_KEY_PATTERN.get_or_init(|| {
        Regex::new(r"\b(?:AKIA|ASIA)[A-Z0-9]{16}\b").expect("access key regex is valid")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn parses_rust_typescript_and_python_locations() {
        let temp_dir = tempfile::tempdir().unwrap();
        fs::create_dir(temp_dir.path().join("src")).unwrap();
        fs::write(temp_dir.path().join("src/lib.rs"), "fn value() {}\n").unwrap();
        fs::write(temp_dir.path().join("src/app.ts"), "export {};\n").unwrap();
        fs::write(temp_dir.path().join("src/app.py"), "raise ValueError()\n").unwrap();
        let workspace = CodingWorkspace::new(temp_dir.path()).unwrap();
        let stderr = "error[E0308]: mismatched types\n  --> src/lib.rs:1:4\n\
                      src/app.ts:1:8: error TS2322: wrong type\n\
                      Traceback (most recent call last):\n\
                      File \"src/app.py\", line 1, in <module>\n\
                      ValueError: bad value\n";

        let report = DiagnosticAnalyzer::analyze(&workspace, "", stderr);

        assert_eq!(report.error_count, 3);
        assert_eq!(report.diagnostics[0].code.as_deref(), Some("E0308"));
        assert_eq!(
            report.diagnostics[0].path,
            Some(PathBuf::from("src/lib.rs"))
        );
        assert_eq!(
            report.diagnostics[1].path,
            Some(PathBuf::from("src/app.ts"))
        );
        assert_eq!(report.diagnostics[2].code.as_deref(), Some("ValueError"));
        assert_eq!(report.diagnostics[2].line, Some(1));
    }

    #[test]
    fn redacts_secrets_and_rejects_sensitive_or_external_paths() {
        let temp_dir = tempfile::tempdir().unwrap();
        let workspace = CodingWorkspace::new(temp_dir.path()).unwrap();
        let stderr = "src/lib.rs:2:1: error: password=hunter2 token=abc\n\
                      .env:1:1: error: API_KEY=secret\n\
                      ../outside.rs:1:1: error: Bearer abc.def\n";

        let report = DiagnosticAnalyzer::analyze(&workspace, "", stderr);

        assert_eq!(
            report.diagnostics[0].path,
            Some(PathBuf::from("src/lib.rs"))
        );
        assert!(report.diagnostics[0].message.contains("[REDACTED]"));
        assert_eq!(report.diagnostics[1].path, None);
        assert_eq!(report.diagnostics[2].path, None);
        assert!(report.diagnostics[2].message.contains("Bearer [REDACTED]"));
    }
}
