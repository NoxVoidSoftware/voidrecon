use crate::config::ProviderKind;
use super::LlmProvider;

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

