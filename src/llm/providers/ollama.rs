use crate::config::ProviderKind;
use super::LlmProvider;

/// Ollama local offline provider
#[derive(Debug, Clone)]
pub struct OllamaProvider {
    pub base_url: String,
}

impl OllamaProvider {
    pub fn new(custom_base_url: Option<String>) -> Self {
        Self {
            base_url: custom_base_url.unwrap_or_else(|| ProviderKind::Ollama.default_base_url().to_string()),
        }
    }
}

impl LlmProvider for OllamaProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Ollama
    }

    fn base_url(&self) -> &str {
        &self.base_url
    }

    fn api_key(&self) -> Option<&str> {
        None
    }
}

