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

/// NVIDIA NIM provider (https://integrate.api.nvidia.com/v1)
#[derive(Debug, Clone)]
pub struct NvidiaNimProvider {
    pub base_url: String,
    pub api_key: String,
}

impl NvidiaNimProvider {
    pub fn new(api_key: String, custom_base_url: Option<String>) -> Self {
        Self {
            base_url: custom_base_url.unwrap_or_else(|| ProviderKind::NvidiaNim.default_base_url().to_string()),
            api_key,
        }
    }
}

impl LlmProvider for NvidiaNimProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::NvidiaNim
    }

    fn base_url(&self) -> &str {
        &self.base_url
    }

    fn api_key(&self) -> Option<&str> {
        Some(&self.api_key)
    }
}

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
