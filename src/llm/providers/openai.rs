use crate::config::ProviderKind;
use super::LlmProvider;

/// OpenAI direct provider
#[derive(Debug, Clone)]
pub struct OpenAiProvider {
    pub base_url: String,
    pub api_key: String,
}

impl OpenAiProvider {
    pub fn new(api_key: String, custom_base_url: Option<String>) -> Self {
        Self {
            base_url: custom_base_url.unwrap_or_else(|| ProviderKind::OpenAi.default_base_url().to_string()),
            api_key,
        }
    }
}

impl LlmProvider for OpenAiProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::OpenAi
    }

    fn base_url(&self) -> &str {
        &self.base_url
    }

    fn api_key(&self) -> Option<&str> {
        Some(&self.api_key)
    }
}
