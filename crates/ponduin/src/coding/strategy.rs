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
}
