use crate::config::ProviderKind;
use super::LlmProvider;

/// Custom / generic OpenAI-compatible provider (LM Studio, vLLM, Groq, Together, LocalAI)
#[derive(Debug, Clone)]
pub struct CustomProvider {
    pub base_url: String,
    pub api_key: Option<String>,
}

impl CustomProvider {
    pub fn new(base_url: String, api_key: Option<String>) -> Self {
        Self { base_url, api_key }
    }
}

impl LlmProvider for CustomProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Custom
    }

    fn base_url(&self) -> &str {
        &self.base_url
    }

    fn api_key(&self) -> Option<&str> {
        self.api_key.as_deref()
    }
}

