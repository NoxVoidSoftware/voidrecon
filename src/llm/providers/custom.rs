use super::{ApiKeyPolicy, ProviderSpec};

/// Custom / generic OpenAI-compatible endpoint (LM Studio, vLLM, Groq, Together, LocalAI)
pub struct Custom;

impl ProviderSpec for Custom {
    fn name(&self) -> &'static str {
        "Custom Endpoint"
    }

    fn aliases(&self) -> &'static [&'static str] {
        &["custom"]
    }

    fn default_base_url(&self) -> &'static str {
        "http://localhost:1234/v1"
    }

    fn api_key_policy(&self) -> ApiKeyPolicy {
        ApiKeyPolicy::Optional
    }

    fn accepts_custom_url(&self) -> bool {
        true
    }
}
