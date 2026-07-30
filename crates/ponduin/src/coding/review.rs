use crate::coding::file::content_digest;
use crate::coding::git::GitDiff;
use crate::coding::sensitive::is_sensitive_path;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

const MAX_REVIEW_FINDINGS: usize = 200;
static HUNK_PATTERN: OnceLock<Regex> = OnceLock::new();
static SECRET_PATTERN: OnceLock<Regex> = OnceLock::new();

/// Conservative local checks over added Git diff lines.
#[derive(Debug)]
pub struct ReviewAnalyzer;

impl ReviewAnalyzer {
    pub fn analyze(diff: &GitDiff) -> ReviewReport {
        let mut findings = Vec::new();
        let mut current_file = None;
        let mut file_index = 0usize;
        let mut new_line = None;
        let mut truncated = diff.truncated;

        for line in diff.patch.lines() {
            if line.starts_with("diff --git ") {
                current_file = diff.files.get(file_index).cloned();
                file_index += 1;
                new_line = None;
                continue;
            }
            if let Some(captures) = hunk_pattern().captures(line) {
                new_line = captures
                    .name("line")
                    .and_then(|value| value.as_str().parse::<usize>().ok());
                continue;
            }
            if line.starts_with("+++") || line.starts_with("---") {
                continue;
            }
            if line.starts_with('+') {
                if let (Some(path), Some(line_number)) = (&current_file, new_line) {
                    if !is_sensitive_path(path) {
                        inspect_added_line(path, line_number, line, &mut findings);
                    }
                }
                new_line = new_line.map(|line| line.saturating_add(1));
            } else if !line.starts_with('-') {
                new_line = new_line.map(|line| line.saturating_add(1));
            }
            if findings.len() >= MAX_REVIEW_FINDINGS {
                truncated = true;
                findings.truncate(MAX_REVIEW_FINDINGS);
                break;
            }
        }

        findings.sort_by(|left, right| {
            left.severity
                .cmp(&right.severity)
                .then(left.path.cmp(&right.path))
                .then(left.line.cmp(&right.line))
                .then(left.category.cmp(&right.category))
        });
        let mut counts = BTreeMap::new();
        for finding in &findings {
            *counts.entry(finding.severity).or_insert(0) += 1;
        }
        ReviewReport {
            staged: diff.staged,
            files: diff.files.clone(),
            skipped_sensitive: diff.skipped_sensitive.clone(),
            findings,
            counts,
            analyzed_patch_fingerprint: content_digest(diff.patch.as_bytes()),
            diff_truncated: diff.truncated,
            lossy_output: diff.lossy_output,
            truncated,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewReport {
    pub staged: bool,
    pub files: Vec<PathBuf>,
    pub skipped_sensitive: Vec<PathBuf>,
    pub findings: Vec<ReviewFinding>,
    pub counts: BTreeMap<ReviewSeverity, usize>,
    pub analyzed_patch_fingerprint: String,
    pub diff_truncated: bool,
    pub lossy_output: bool,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewFinding {
    pub severity: ReviewSeverity,
    pub category: ReviewCategory,
    pub message: String,
    pub path: PathBuf,
    pub line: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewSeverity {
    Critical,
    High,
    Medium,
    Low,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewCategory {
    Security,
    Reliability,
    ErrorHandling,
    Maintainability,
    DebugArtifact,
}

fn inspect_added_line(
    path: &Path,
    line_number: usize,
    added_line: &str,
    findings: &mut Vec<ReviewFinding>,
) {
    let code = added_line.trim_start_matches('+').trim();
    if code.is_empty() {
        return;
    }
    if code.to_ascii_uppercase().contains("PRIVATE KEY") || secret_pattern().is_match(code) {
        push_finding(
            findings,
            ReviewSeverity::Critical,
            ReviewCategory::Security,
            "Possible hard-coded credential or private key added; remove it and rotate any exposed value.",
            path,
            line_number,
        );
        return;
    }
    if contains_any(
        code,
        &[
            "danger_accept_invalid_certs(true)",
            "verify=False",
            "rejectUnauthorized: false",
        ],
    ) {
        push_finding(
            findings,
            ReviewSeverity::High,
            ReviewCategory::Security,
            "TLS certificate verification appears to be disabled.",
            path,
            line_number,
        );
    }
    if contains_any(
        code,
        &[
            "shell=True",
            "eval(",
            "Runtime.getRuntime().exec(",
            "Command::new(\"sh\")",
            "Command::new(\"bash\")",
        ],
    ) {
        push_finding(
            findings,
            ReviewSeverity::High,
            ReviewCategory::Security,
            "Dynamic shell or code execution was added; validate that untrusted input cannot reach it.",
            path,
            line_number,
        );
    }
    if contains_any(code, &[".unwrap()", ".expect("]) && !is_test_path(path) {
        push_finding(
            findings,
            ReviewSeverity::Medium,
            ReviewCategory::ErrorHandling,
            "A new panic path was added outside tests; propagate or handle the error where failure is recoverable.",
            path,
            line_number,
        );
    }
    if contains_any(code, &["unsafe {", "unsafe fn "]) {
        push_finding(
            findings,
            ReviewSeverity::Medium,
            ReviewCategory::Reliability,
            "New unsafe Rust requires a documented invariant and focused safety review.",
            path,
            line_number,
        );
    }
    if contains_any(code, &["except: pass", "catch (_) {}", "catch (e) {}"]) {
        push_finding(
            findings,
            ReviewSeverity::Medium,
            ReviewCategory::ErrorHandling,
            "An exception appears to be swallowed without handling or observability.",
            path,
            line_number,
        );
    }
    if contains_any(code, &["console.log(", "dbg!(", "print_r(", "var_dump("])
        && !is_test_path(path)
    {
        push_finding(
            findings,
            ReviewSeverity::Low,
            ReviewCategory::DebugArtifact,
            "A debug-output statement was added outside tests.",
            path,
            line_number,
        );
    }
    if contains_any(code, &["TODO", "FIXME", "HACK"]) {
        push_finding(
            findings,
            ReviewSeverity::Low,
            ReviewCategory::Maintainability,
            "A new unresolved TODO/FIXME/HACK marker was added.",
            path,
            line_number,
        );
    }
}

fn push_finding(
    findings: &mut Vec<ReviewFinding>,
    severity: ReviewSeverity,
    category: ReviewCategory,
    message: &str,
    path: &Path,
    line: usize,
) {
    if findings.len() < MAX_REVIEW_FINDINGS {
        findings.push(ReviewFinding {
            severity,
            category,
            message: message.to_string(),
            path: path.to_path_buf(),
            line,
        });
    }
}

fn contains_any(value: &str, patterns: &[&str]) -> bool {
    patterns.iter().any(|pattern| value.contains(pattern))
}

fn is_test_path(path: &Path) -> bool {
    let normalized = path.to_string_lossy().to_ascii_lowercase();
    normalized.starts_with("test/")
        || normalized.starts_with("tests/")
        || normalized.contains("/test")
        || normalized.contains("/tests/")
        || normalized.ends_with("_test.rs")
        || normalized.ends_with("_test.go")
        || normalized.ends_with("_test.py")
        || normalized.ends_with(".test.js")
        || normalized.ends_with(".test.ts")
        || normalized.ends_with(".spec.js")
        || normalized.ends_with(".spec.ts")
}

fn hunk_pattern() -> &'static Regex {
    HUNK_PATTERN.get_or_init(|| {
        Regex::new(r"^@@ -\d+(?:,\d+)? \+(?P<line>\d+)(?:,\d+)? @@")
            .expect("Git hunk regex is valid")
    })
}

fn secret_pattern() -> &'static Regex {
    SECRET_PATTERN.get_or_init(|| {
        Regex::new(
            r#"(?i)\b(?:api[_-]?key|token|secret|password|passwd|credential)\b(?:\s*:\s*(?:&?str|String))?\s*[:=]\s*(?:"[^"]{4,}"|'[^']{4,}'|[A-Za-z0-9._~+/=-]{8,})"#,
        )
        .expect("review secret regex is valid")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_added_security_reliability_and_debug_findings_in_severity_order() {
        let diff = GitDiff {
            staged: false,
            files: vec![PathBuf::from("src/app.rs")],
            skipped_sensitive: Vec::new(),
            patch: "diff --git a/src/app.rs b/src/app.rs\n\
                    --- a/src/app.rs\n\
                    +++ b/src/app.rs\n\
                    @@ -1,1 +1,5 @@\n\
                    +const API_KEY: &str = \"abcdefgh\";\n\
                    +let client = builder.danger_accept_invalid_certs(true);\n\
                    +let value = result.unwrap();\n\
                    +dbg!(value);\n\
                    +// TODO handle this\n"
                .to_string(),
            truncated: false,
            lossy_output: false,
        };

        let report = ReviewAnalyzer::analyze(&diff);

        assert_eq!(report.findings.len(), 5);
        assert_eq!(report.findings[0].severity, ReviewSeverity::Critical);
        assert_eq!(report.findings[0].line, 1);
        assert_eq!(report.findings[1].severity, ReviewSeverity::High);
        assert_eq!(report.findings[2].severity, ReviewSeverity::Medium);
        assert_eq!(report.findings[3].severity, ReviewSeverity::Low);
        assert_eq!(report.counts[&ReviewSeverity::Critical], 1);
        assert!(report.analyzed_patch_fingerprint.starts_with("blake3:"));
    }

    #[test]
    fn does_not_flag_test_panics_or_context_and_removed_lines() {
        let diff = GitDiff {
            staged: true,
            files: vec![PathBuf::from("tests/app_test.rs")],
            skipped_sensitive: vec![PathBuf::from(".env")],
            patch: "diff --git a/tests/app_test.rs b/tests/app_test.rs\n\
                    --- a/tests/app_test.rs\n\
                    +++ b/tests/app_test.rs\n\
                    @@ -1,2 +1,2 @@\n\
                    -const TOKEN: &str = \"abcdefgh\";\n\
                     context.unwrap();\n\
                    +result.unwrap();\n"
                .to_string(),
            truncated: false,
            lossy_output: false,
        };

        let report = ReviewAnalyzer::analyze(&diff);

        assert!(report.findings.is_empty());
        assert_eq!(report.skipped_sensitive, vec![PathBuf::from(".env")]);
    }
}
