use crate::coding::capabilities::{
    CapabilitySupport, CodingSuitability, PerformanceClass, ResourceClass,
};
use crate::config::{Config, ConfigError};
use serde::de::DeserializeOwned;
use std::time::Duration;

pub const CODING_MAX_ITERATIONS_KEY: &str = "PONDUIN_CODING_MAX_ITERATIONS";
pub const CODING_MAX_REPAIR_ATTEMPTS_KEY: &str = "PONDUIN_CODING_MAX_REPAIR_ATTEMPTS";
pub const CODING_MAX_CONTEXT_TOKENS_KEY: &str = "PONDUIN_CODING_MAX_CONTEXT_TOKENS";
pub const CODING_MAX_FILES_PER_BATCH_KEY: &str = "PONDUIN_CODING_MAX_FILES_PER_BATCH";
pub const CODING_AUTO_TEST_KEY: &str = "PONDUIN_CODING_AUTO_TEST";
pub const CODING_AUTO_FORMAT_KEY: &str = "PONDUIN_CODING_AUTO_FORMAT";
pub const CODING_INDEXING_KEY: &str = "PONDUIN_CODING_INDEXING";
pub const CODING_LSP_KEY: &str = "PONDUIN_CODING_LSP";
pub const CODING_TREE_SITTER_KEY: &str = "PONDUIN_CODING_TREE_SITTER";
pub const CODING_EMBEDDINGS_KEY: &str = "PONDUIN_CODING_EMBEDDINGS";
pub const CODING_SHELL_TIMEOUT_KEY: &str = "PONDUIN_CODING_SHELL_TIMEOUT";
pub const CODING_OUTPUT_LIMIT_KEY: &str = "PONDUIN_CODING_OUTPUT_LIMIT";
pub const CODING_MODEL_TOOL_CALLING_KEY: &str = "PONDUIN_CODING_MODEL_TOOL_CALLING";
pub const CODING_MODEL_STRUCTURED_OUTPUT_KEY: &str = "PONDUIN_CODING_MODEL_STRUCTURED_OUTPUT";
pub const CODING_MODEL_CODING_SUITABILITY_KEY: &str = "PONDUIN_CODING_MODEL_CODING_SUITABILITY";
pub const CODING_MODEL_MULTIMODALITY_KEY: &str = "PONDUIN_CODING_MODEL_MULTIMODALITY";
pub const CODING_MODEL_EMBEDDING_SUPPORT_KEY: &str = "PONDUIN_CODING_MODEL_EMBEDDING_SUPPORT";
pub const CODING_MODEL_SPEED_KEY: &str = "PONDUIN_CODING_MODEL_SPEED";
pub const CODING_MODEL_RESOURCE_DEMAND_KEY: &str = "PONDUIN_CODING_MODEL_RESOURCE_DEMAND";

const MIN_CONTEXT_TOKENS: usize = 1_024;
const MAX_CONTEXT_TOKENS: usize = 1_000_000;
const MAX_ITERATIONS_LIMIT: u32 = 1_000;
const MAX_REPAIR_ATTEMPTS_LIMIT: u32 = 100;
const MAX_FILES_PER_BATCH_LIMIT: usize = 1_000;
const MIN_SHELL_TIMEOUT_SECONDS: u64 = 1;
const MAX_SHELL_TIMEOUT_SECONDS: u64 = 3_600;
const MIN_OUTPUT_LIMIT: usize = 1_024;
const MAX_OUTPUT_LIMIT: usize = 100 * 1_024 * 1_024;

/// Validated tuning settings for the internal coding agent.
///
/// The agent is a core Ponduin capability and is always available outside Chat
/// mode. Confirmation behavior is intentionally absent: it remains controlled
/// exclusively by the session's `PonduinMode`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodingConfig {
    pub max_iterations: u32,
    pub max_repair_attempts: u32,
    pub max_context_tokens: usize,
    pub max_files_per_batch: usize,
    pub auto_test: bool,
    pub auto_format: bool,
    pub indexing: bool,
    pub lsp: bool,
    pub tree_sitter: bool,
    pub embeddings: bool,
    pub shell_timeout: Duration,
    pub output_limit: usize,
    pub model_tool_calling: CapabilitySupport,
    pub model_structured_output: CapabilitySupport,
    pub model_coding_suitability: CodingSuitability,
    pub model_multimodality: CapabilitySupport,
    pub model_embedding_support: CapabilitySupport,
    pub model_speed: PerformanceClass,
    pub model_resource_demand: ResourceClass,
}

impl Default for CodingConfig {
    fn default() -> Self {
        Self {
            max_iterations: 50,
            max_repair_attempts: 3,
            max_context_tokens: 32_768,
            max_files_per_batch: 20,
            auto_test: true,
            auto_format: false,
            indexing: true,
            lsp: false,
            tree_sitter: true,
            embeddings: false,
            shell_timeout: Duration::from_secs(120),
            output_limit: 2 * 1_024 * 1_024,
            model_tool_calling: CapabilitySupport::Unknown,
            model_structured_output: CapabilitySupport::Unknown,
            model_coding_suitability: CodingSuitability::Unknown,
            model_multimodality: CapabilitySupport::Unknown,
            model_embedding_support: CapabilitySupport::Unknown,
            model_speed: PerformanceClass::Unknown,
            model_resource_demand: ResourceClass::Unknown,
        }
    }
}

impl CodingConfig {
    pub fn from_config(config: &Config) -> Result<Self, CodingConfigError> {
        let defaults = Self::default();
        let shell_timeout_seconds = optional(
            config,
            CODING_SHELL_TIMEOUT_KEY,
            defaults.shell_timeout.as_secs(),
        )?;

        let resolved = Self {
            max_iterations: optional(config, CODING_MAX_ITERATIONS_KEY, defaults.max_iterations)?,
            max_repair_attempts: optional(
                config,
                CODING_MAX_REPAIR_ATTEMPTS_KEY,
                defaults.max_repair_attempts,
            )?,
            max_context_tokens: optional(
                config,
                CODING_MAX_CONTEXT_TOKENS_KEY,
                defaults.max_context_tokens,
            )?,
            max_files_per_batch: optional(
                config,
                CODING_MAX_FILES_PER_BATCH_KEY,
                defaults.max_files_per_batch,
            )?,
            auto_test: optional(config, CODING_AUTO_TEST_KEY, defaults.auto_test)?,
            auto_format: optional(config, CODING_AUTO_FORMAT_KEY, defaults.auto_format)?,
            indexing: optional(config, CODING_INDEXING_KEY, defaults.indexing)?,
            lsp: optional(config, CODING_LSP_KEY, defaults.lsp)?,
            tree_sitter: optional(config, CODING_TREE_SITTER_KEY, defaults.tree_sitter)?,
            embeddings: optional(config, CODING_EMBEDDINGS_KEY, defaults.embeddings)?,
            shell_timeout: Duration::from_secs(shell_timeout_seconds),
            output_limit: optional(config, CODING_OUTPUT_LIMIT_KEY, defaults.output_limit)?,
            model_tool_calling: optional(
                config,
                CODING_MODEL_TOOL_CALLING_KEY,
                defaults.model_tool_calling,
            )?,
            model_structured_output: optional(
                config,
                CODING_MODEL_STRUCTURED_OUTPUT_KEY,
                defaults.model_structured_output,
            )?,
            model_coding_suitability: optional(
                config,
                CODING_MODEL_CODING_SUITABILITY_KEY,
                defaults.model_coding_suitability,
            )?,
            model_multimodality: optional(
                config,
                CODING_MODEL_MULTIMODALITY_KEY,
                defaults.model_multimodality,
            )?,
            model_embedding_support: optional(
                config,
                CODING_MODEL_EMBEDDING_SUPPORT_KEY,
                defaults.model_embedding_support,
            )?,
            model_speed: optional(config, CODING_MODEL_SPEED_KEY, defaults.model_speed)?,
            model_resource_demand: optional(
                config,
                CODING_MODEL_RESOURCE_DEMAND_KEY,
                defaults.model_resource_demand,
            )?,
        };

        resolved.validate()?;
        Ok(resolved)
    }

    fn validate(&self) -> Result<(), CodingConfigError> {
        validate_range(
            CODING_MAX_ITERATIONS_KEY,
            u64::from(self.max_iterations),
            1,
            u64::from(MAX_ITERATIONS_LIMIT),
        )?;
        validate_range(
            CODING_MAX_REPAIR_ATTEMPTS_KEY,
            u64::from(self.max_repair_attempts),
            0,
            u64::from(MAX_REPAIR_ATTEMPTS_LIMIT),
        )?;
        validate_range(
            CODING_MAX_CONTEXT_TOKENS_KEY,
            self.max_context_tokens as u64,
            MIN_CONTEXT_TOKENS as u64,
            MAX_CONTEXT_TOKENS as u64,
        )?;
        validate_range(
            CODING_MAX_FILES_PER_BATCH_KEY,
            self.max_files_per_batch as u64,
            1,
            MAX_FILES_PER_BATCH_LIMIT as u64,
        )?;
        validate_range(
            CODING_SHELL_TIMEOUT_KEY,
            self.shell_timeout.as_secs(),
            MIN_SHELL_TIMEOUT_SECONDS,
            MAX_SHELL_TIMEOUT_SECONDS,
        )?;
        validate_range(
            CODING_OUTPUT_LIMIT_KEY,
            self.output_limit as u64,
            MIN_OUTPUT_LIMIT as u64,
            MAX_OUTPUT_LIMIT as u64,
        )
    }
}

fn optional<T>(config: &Config, key: &'static str, default: T) -> Result<T, CodingConfigError>
where
    T: DeserializeOwned,
{
    match config.get_param(key) {
        Ok(value) => Ok(value),
        Err(ConfigError::NotFound(_)) => Ok(default),
        Err(source) => Err(CodingConfigError::InvalidValue { key, source }),
    }
}

fn validate_range(
    key: &'static str,
    value: u64,
    minimum: u64,
    maximum: u64,
) -> Result<(), CodingConfigError> {
    if (minimum..=maximum).contains(&value) {
        Ok(())
    } else {
        Err(CodingConfigError::OutOfRange {
            key,
            value,
            minimum,
            maximum,
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CodingConfigError {
    #[error("invalid coding configuration value for {key}: {source}")]
    InvalidValue {
        key: &'static str,
        #[source]
        source: ConfigError,
    },
    #[error(
        "coding configuration value for {key} is {value}, expected {minimum} through {maximum}"
    )]
    OutOfRange {
        key: &'static str,
        value: u64,
        minimum: u64,
        maximum: u64,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn test_config(temp_dir: &TempDir) -> Config {
        Config::new_with_file_secrets(
            temp_dir.path().join("config.yaml"),
            temp_dir.path().join("secrets.yaml"),
        )
        .unwrap()
    }

    #[test]
    fn defaults_keep_optional_subsystems_conservative() {
        let defaults = CodingConfig::default();
        assert!(defaults.tree_sitter);
        assert!(!defaults.embeddings);
        assert!(!defaults.lsp);
        assert_eq!(defaults.model_tool_calling, CapabilitySupport::Unknown);
        assert_eq!(
            defaults.model_coding_suitability,
            CodingSuitability::Unknown
        );
    }

    #[test]
    fn resolves_typed_values_from_existing_config() {
        let temp_dir = tempfile::tempdir().unwrap();
        let config = test_config(&temp_dir);
        config.set_param(CODING_MAX_ITERATIONS_KEY, 12).unwrap();
        config.set_param(CODING_SHELL_TIMEOUT_KEY, 45).unwrap();
        config
            .set_param(
                CODING_MODEL_STRUCTURED_OUTPUT_KEY,
                CapabilitySupport::Supported,
            )
            .unwrap();
        config
            .set_param(
                CODING_MODEL_CODING_SUITABILITY_KEY,
                CodingSuitability::Strong,
            )
            .unwrap();
        config
            .set_param(CODING_MODEL_SPEED_KEY, PerformanceClass::Fast)
            .unwrap();
        config
            .set_param(CODING_MODEL_RESOURCE_DEMAND_KEY, ResourceClass::Low)
            .unwrap();

        let resolved = CodingConfig::from_config(&config).unwrap();

        assert_eq!(resolved.max_iterations, 12);
        assert_eq!(resolved.shell_timeout, Duration::from_secs(45));
        assert_eq!(
            resolved.model_structured_output,
            CapabilitySupport::Supported
        );
        assert_eq!(resolved.model_coding_suitability, CodingSuitability::Strong);
        assert_eq!(resolved.model_speed, PerformanceClass::Fast);
        assert_eq!(resolved.model_resource_demand, ResourceClass::Low);
    }

    #[test]
    fn rejects_values_outside_hard_limits() {
        let temp_dir = tempfile::tempdir().unwrap();
        let config = test_config(&temp_dir);
        config.set_param(CODING_MAX_ITERATIONS_KEY, 0).unwrap();

        let error = CodingConfig::from_config(&config).unwrap_err();

        assert!(matches!(
            error,
            CodingConfigError::OutOfRange {
                key: CODING_MAX_ITERATIONS_KEY,
                ..
            }
        ));
    }

    #[test]
    fn legacy_opt_in_keys_do_not_control_the_core_capability() {
        let temp_dir = tempfile::tempdir().unwrap();
        let config = test_config(&temp_dir);
        config.set_param("PONDUIN_CODING_ENABLED", false).unwrap();
        config.set_param("PONDUIN_CODING_MODE", "general").unwrap();

        let resolved = CodingConfig::from_config(&config).unwrap();

        assert_eq!(resolved, CodingConfig::default());
    }
}
