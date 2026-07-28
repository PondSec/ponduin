use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// Selects coding behavior independently from the tool-confirmation mode.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodingTaskMode {
    #[default]
    General,
    Coding,
    Debugging,
    Refactoring,
    RepositoryAnalysis,
    TestGeneration,
    Documentation,
    Review,
}

impl CodingTaskMode {
    pub const ALL: [Self; 8] = [
        Self::General,
        Self::Coding,
        Self::Debugging,
        Self::Refactoring,
        Self::RepositoryAnalysis,
        Self::TestGeneration,
        Self::Documentation,
        Self::Review,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::General => "general",
            Self::Coding => "coding",
            Self::Debugging => "debugging",
            Self::Refactoring => "refactoring",
            Self::RepositoryAnalysis => "repository_analysis",
            Self::TestGeneration => "test_generation",
            Self::Documentation => "documentation",
            Self::Review => "review",
        }
    }

    pub const fn enables_coding_tools(self) -> bool {
        !matches!(self, Self::General)
    }

    pub const fn prompt_guidance(self) -> &'static str {
        match self {
            Self::General => "Use the general agent workflow.",
            Self::Coding => {
                "Implement the requested behavior in small coherent patches, follow existing \
                 architecture and style, add meaningful regression coverage, and validate the \
                 narrowest relevant checks after each step."
            }
            Self::Debugging => {
                "Separate symptoms from causes. Parse diagnostics, locate related symbols and \
                 callers, state a falsifiable hypothesis, test it with the smallest targeted \
                 check, and change code only when evidence supports the hypothesis."
            }
            Self::Refactoring => {
                "Establish behavior-preserving baseline checks before editing, keep public APIs \
                 compatible unless explicitly requested, make reversible structural steps, and \
                 rerun the same checks after every step."
            }
            Self::RepositoryAnalysis => {
                "Remain read-only unless the user explicitly requests implementation. Map \
                 components, entry points, dependencies, call relationships, build and test \
                 paths, risks, and unknowns with file-backed evidence."
            }
            Self::TestGeneration => {
                "Detect and reuse the repository's existing test framework and conventions. Add \
                 behavior-focused success, boundary, failure, and regression cases without \
                 introducing a competing test architecture or tests tied only to implementation \
                 details."
            }
            Self::Documentation => {
                "Verify names, commands, configuration keys, and examples against repository \
                 sources. Match the existing documentation structure and do not claim behavior \
                 that has not been implemented and tested."
            }
            Self::Review => {
                "Do not edit by default. Inspect local diffs and relevant surrounding code, then \
                 report only actionable findings ordered by severity with exact file and line, \
                 impact, and a concise remediation; explicitly say when no finding is proven."
            }
        }
    }
}

impl fmt::Display for CodingTaskMode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for CodingTaskMode {
    type Err = CodingTaskModeParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|mode| mode.as_str() == value)
            .ok_or_else(|| CodingTaskModeParseError(value.to_string()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown coding task mode `{0}`")]
pub struct CodingTaskModeParseError(String);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serde_names_are_stable() {
        for mode in CodingTaskMode::ALL {
            let serialized = serde_json::to_string(&mode).unwrap();
            assert_eq!(serialized, format!("\"{}\"", mode.as_str()));
            assert_eq!(
                serde_json::from_str::<CodingTaskMode>(&serialized).unwrap(),
                mode
            );
        }
    }

    #[test]
    fn general_is_the_only_mode_without_coding_tools() {
        assert!(!CodingTaskMode::General.enables_coding_tools());
        assert!(CodingTaskMode::ALL
            .into_iter()
            .filter(|mode| mode.enables_coding_tools())
            .all(|mode| mode != CodingTaskMode::General));
    }

    #[test]
    fn every_coding_mode_has_specific_nonempty_guidance() {
        let guidance = CodingTaskMode::ALL
            .into_iter()
            .map(CodingTaskMode::prompt_guidance)
            .collect::<std::collections::BTreeSet<_>>();

        assert_eq!(guidance.len(), CodingTaskMode::ALL.len());
        assert!(CodingTaskMode::Debugging
            .prompt_guidance()
            .contains("hypothesis"));
        assert!(CodingTaskMode::Review
            .prompt_guidance()
            .contains("file and line"));
    }
}
