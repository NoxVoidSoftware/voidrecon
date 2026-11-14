use super::{ApiKeyPolicy, ProviderSpec};

/// Ollama local offline provider
pub struct Ollama;

impl ProviderSpec for Ollama {
    fn name(&self) -> &'static str {
        "Ollama (Local)"
    }

    fn aliases(&self) -> &'static [&'static str] {
        &["ollama"]
    }

    fn default_base_url(&self) -> &'static str {
        "http://localhost:11434/v1"
    }

    fn api_key_policy(&self) -> ApiKeyPolicy {
        ApiKeyPolicy::Unused
    }

    fn accepts_custom_url(&self) -> bool {
        true
    }
}
