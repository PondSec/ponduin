use crate::coding::config::CodingConfig;
use ponduin_providers::model::ModelConfig;
use serde::{Deserialize, Serialize};

/// Provider-neutral coding strategy derived only from declared model limits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelCapabilityProfile {
    pub context_window_tokens: usize,
    pub max_output_tokens: usize,
    pub reasoning: bool,
    pub tool_transport: ToolTransport,
    pub context_class: ContextClass,
    pub execution_strategy: ExecutionStrategy,
    pub recommended_context_tokens: usize,
    pub recommended_files_per_context: usize,
    pub recommended_files_per_change: usize,
    pub recommended_plan_file_threshold: usize,
}

impl ModelCapabilityProfile {
    pub fn detect(model: &ModelConfig, coding: &CodingConfig) -> Self {
        let context_window_tokens = model.context_limit();
        let max_output_tokens = usize::try_from(model.max_output_tokens().max(0)).unwrap_or(0);
        let reasoning = model.is_reasoning_model();
        let tool_transport = if model.toolshim {
            ToolTransport::EmulatedJson
        } else {
            ToolTransport::Native
        };
        let context_class = match context_window_tokens {
            0..=65_535 => ContextClass::Compact,
            65_536..=199_999 => ContextClass::Standard,
            _ => ContextClass::Extended,
        };
        let execution_strategy = if tool_transport == ToolTransport::EmulatedJson
            || context_class == ContextClass::Compact
        {
            ExecutionStrategy::Sequential
        } else if reasoning {
            ExecutionStrategy::Deliberate
        } else {
            ExecutionStrategy::Incremental
        };
        let reserved_tokens = max_output_tokens.max(context_window_tokens / 10).max(4_096);
        let usable_input = context_window_tokens.saturating_sub(reserved_tokens);
        let suggested_fraction = match context_class {
            ContextClass::Compact => usable_input / 5,
            ContextClass::Standard => usable_input / 4,
            ContextClass::Extended => usable_input / 3,
        };
        let recommended_context_tokens = coding
            .max_context_tokens
            .min(suggested_fraction.max(1_024))
            .min(usable_input.max(1));
        let recommended_files_per_context = match context_class {
            ContextClass::Compact => 6,
            ContextClass::Standard => 12,
            ContextClass::Extended => 20,
        };
        let strategy_change_limit = match execution_strategy {
            ExecutionStrategy::Sequential => 1,
            ExecutionStrategy::Incremental => 3,
            ExecutionStrategy::Deliberate => 5,
        };
        let recommended_files_per_change = coding.max_files_per_batch.min(strategy_change_limit);
        let recommended_plan_file_threshold =
            if context_class == ContextClass::Compact || !reasoning {
                1
            } else {
                coding.plan_file_threshold
            };

        Self {
            context_window_tokens,
            max_output_tokens,
            reasoning,
            tool_transport,
            context_class,
            execution_strategy,
            recommended_context_tokens,
            recommended_files_per_context,
            recommended_files_per_change,
            recommended_plan_file_threshold,
        }
    }

    pub fn prompt_guidance(&self) -> String {
        format!(
            "Model capability profile: context_window={} tokens, max_output={} tokens, \
             context_class={}, reasoning={}, tool_transport={}, execution_strategy={}. Keep each \
             prepared repository context at or below {} tokens and normally within {} files. \
             Change at most {} files per verified step. Create an explicit plan before work \
             expected to touch {} or more files. After each change, validate the narrowest \
             relevant surface before expanding scope.",
            self.context_window_tokens,
            self.max_output_tokens,
            self.context_class,
            self.reasoning,
            self.tool_transport,
            self.execution_strategy,
            self.recommended_context_tokens,
            self.recommended_files_per_context,
            self.recommended_files_per_change,
            self.recommended_plan_file_threshold
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolTransport {
    Native,
    EmulatedJson,
}

impl std::fmt::Display for ToolTransport {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Native => formatter.write_str("native"),
            Self::EmulatedJson => formatter.write_str("emulated_json"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextClass {
    Compact,
    Standard,
    Extended,
}

impl std::fmt::Display for ContextClass {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Compact => formatter.write_str("compact"),
            Self::Standard => formatter.write_str("standard"),
            Self::Extended => formatter.write_str("extended"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionStrategy {
    Sequential,
    Incremental,
    Deliberate,
}

impl std::fmt::Display for ExecutionStrategy {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Sequential => formatter.write_str("sequential"),
            Self::Incremental => formatter.write_str("incremental"),
            Self::Deliberate => formatter.write_str("deliberate"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coding::CodingTaskMode;

    fn coding_config() -> CodingConfig {
        CodingConfig {
            enabled: true,
            task_mode: CodingTaskMode::Coding,
            max_context_tokens: 100_000,
            max_files_per_batch: 20,
            plan_file_threshold: 4,
            ..CodingConfig::default()
        }
    }

    #[test]
    fn constrains_compact_toolshim_models_to_sequential_small_steps() {
        let mut model = ModelConfig::new("compact");
        model.context_limit = Some(32_000);
        model.max_tokens = Some(4_000);
        model.toolshim = true;
        model.reasoning = Some(false);

        let profile = ModelCapabilityProfile::detect(&model, &coding_config());

        assert_eq!(profile.context_class, ContextClass::Compact);
        assert_eq!(profile.tool_transport, ToolTransport::EmulatedJson);
        assert_eq!(profile.execution_strategy, ExecutionStrategy::Sequential);
        assert_eq!(profile.recommended_files_per_change, 1);
        assert_eq!(profile.recommended_plan_file_threshold, 1);
        assert!(profile.recommended_context_tokens < 10_000);
    }

    #[test]
    fn lets_extended_reasoning_models_use_larger_but_bounded_context() {
        let mut model = ModelConfig::new("extended");
        model.context_limit = Some(300_000);
        model.max_tokens = Some(16_000);
        model.reasoning = Some(true);

        let profile = ModelCapabilityProfile::detect(&model, &coding_config());

        assert_eq!(profile.context_class, ContextClass::Extended);
        assert_eq!(profile.tool_transport, ToolTransport::Native);
        assert_eq!(profile.execution_strategy, ExecutionStrategy::Deliberate);
        assert_eq!(profile.recommended_context_tokens, 90_000);
        assert_eq!(profile.recommended_files_per_change, 5);
        assert_eq!(profile.recommended_plan_file_threshold, 4);
        assert!(profile.prompt_guidance().contains("context_window=300000"));
    }
}
