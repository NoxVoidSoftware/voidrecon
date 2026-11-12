use crate::config::ProviderKind;
use super::LlmProvider;

/// OpenRouter provider
#[derive(Debug, Clone)]
pub struct OpenRouterProvider {
    pub base_url: String,
    pub api_key: String,
}

impl OpenRouterProvider {
    pub fn new(api_key: String, custom_base_url: Option<String>) -> Self {
        Self {
            base_url: custom_base_url.unwrap_or_else(|| ProviderKind::OpenRouter.default_base_url().to_string()),
            api_key,
        }
    }
}

impl LlmProvider for OpenRouterProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::OpenRouter
    }

    fn base_url(&self) -> &str {
        &self.base_url
    }

    fn api_key(&self) -> Option<&str> {
        Some(&self.api_key)
    }
}

