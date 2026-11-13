//! LLM Provider abstraction for voidrecon
//! Enables connecting OpenRouter, OpenAI, Ollama (local offline), and Custom OpenAI-compatible endpoints.

use crate::config::ProviderKind;

/// Trait defining common behavior for LLM providers
pub trait LlmProvider: Send + Sync {
    fn kind(&self) -> ProviderKind;
    fn name(&self) -> &'static str {
        self.kind().as_str()
    }
    fn base_url(&self) -> &str;
    fn api_key(&self) -> Option<&str>;
    fn requires_api_key(&self) -> bool {
        self.kind().requires_api_key()
    }
    fn chat_endpoint(&self) -> String {
        format!("{}/chat/completions", self.base_url().trim_end_matches('/'))
    }
    fn models_endpoint(&self) -> String {
        format!("{}/models", self.base_url().trim_end_matches('/'))
    }
}

mod openrouter;
mod nvidia;
mod openai;
mod ollama;
mod custom;

pub use openrouter::OpenRouterProvider;
pub use nvidia::NvidiaNimProvider;
pub use openai::OpenAiProvider;
pub use ollama::OllamaProvider;
pub use custom::CustomProvider;

/// Dynamic provider enum wrapping any supported LLM provider
#[derive(Debug, Clone)]
pub enum Provider {
    OpenRouter(OpenRouterProvider),
    NvidiaNim(NvidiaNimProvider),
    OpenAi(OpenAiProvider),
    Ollama(OllamaProvider),
    Custom(CustomProvider),
}

impl Provider {
    pub fn from_config(kind: ProviderKind, api_key: &str, custom_base_url: Option<String>) -> Self {
        match kind {
            ProviderKind::OpenRouter => {
                Provider::OpenRouter(OpenRouterProvider::new(api_key.to_string(), custom_base_url))
            }
            ProviderKind::NvidiaNim => {
                Provider::NvidiaNim(NvidiaNimProvider::new(api_key.to_string(), custom_base_url))
            }
            ProviderKind::OpenAi => {
                Provider::OpenAi(OpenAiProvider::new(api_key.to_string(), custom_base_url))
            }
            ProviderKind::Ollama => Provider::Ollama(OllamaProvider::new(custom_base_url)),
            ProviderKind::Custom => {
                let url = custom_base_url.unwrap_or_else(|| ProviderKind::Custom.default_base_url().to_string());
                let key = if api_key.trim().is_empty() {
                    None
                } else {
                    Some(api_key.to_string())
                };
                Provider::Custom(CustomProvider::new(url, key))
            }
        }
    }
}

impl LlmProvider for Provider {
    fn kind(&self) -> ProviderKind {
        match self {
            Self::OpenRouter(p) => p.kind(),
            Self::NvidiaNim(p) => p.kind(),
            Self::OpenAi(p) => p.kind(),
            Self::Ollama(p) => p.kind(),
            Self::Custom(p) => p.kind(),
        }
    }

    fn base_url(&self) -> &str {
        match self {
            Self::OpenRouter(p) => p.base_url(),
            Self::NvidiaNim(p) => p.base_url(),
            Self::OpenAi(p) => p.base_url(),
            Self::Ollama(p) => p.base_url(),
            Self::Custom(p) => p.base_url(),
        }
    }

    fn api_key(&self) -> Option<&str> {
        match self {
            Self::OpenRouter(p) => p.api_key(),
            Self::NvidiaNim(p) => p.api_key(),
            Self::OpenAi(p) => p.api_key(),
            Self::Ollama(p) => p.api_key(),
            Self::Custom(p) => p.api_key(),
        }
    }
}
