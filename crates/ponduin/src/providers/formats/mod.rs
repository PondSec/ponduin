pub mod anthropic {
    pub use ponduin_providers::formats::anthropic::*;
}
#[cfg(feature = "aws-providers")]
pub mod bedrock;
pub mod databricks {
    pub use ponduin_providers::formats::databricks::*;
}
pub mod gcpvertexai;
pub mod google {
    use anyhow::Result;
    use ponduin_providers::conversation::message::Message;
    pub use ponduin_providers::formats::google::*;
    use ponduin_providers::model::ModelConfig;
    use rmcp::model::Tool;
    use serde_json::Value;

    use crate::config::Config;

    pub fn create_request(
        model_config: &ModelConfig,
        system: &str,
        messages: &[Message],
        tools: &[Tool],
    ) -> Result<Value> {
        // TODO: Remove this config fallback wrapper once gemini_oauth and Vertex/GCP Gemini
        // move into ponduin-providers and receive provider config during construction.
        let thinking_budget = Config::global().get_param("GEMINI25_THINKING_BUDGET").ok();
        create_request_with_thinking_budget(model_config, system, messages, tools, thinking_budget)
    }
}
pub mod openrouter;
pub mod snowflake {
    pub use ponduin_providers::formats::snowflake::*;
}
