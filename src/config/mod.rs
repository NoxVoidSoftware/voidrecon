//! Configuration module for voidrecon
//! Handles config.json read/write, model cache, and API key management.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use dirs;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

const DEFAULT_CONTEXT_LENGTH: u32 = 200_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub context_length: Option<u32>,
    #[serde(default)]
    pub pricing: ModelPricing,
    #[serde(default)]
    pub architecture: Option<ModelArchitecture>,
}

fn default_zero() -> String {
    "0".to_string()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelPricing {
    #[serde(default = "default_zero")]
    pub prompt: String,
    #[serde(default = "default_zero")]
    pub completion: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelArchitecture {
    pub modality: String,
    pub input_modalities: Vec<String>,
    pub output_modalities: Vec<String>,
    pub tokenizer: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    #[default]
    OpenRouter,
    NvidiaNim,
    OpenAi,
    Ollama,
    Custom,
}

pub const ALL_PROVIDERS: &[ProviderKind] = &[
    ProviderKind::OpenRouter,
    ProviderKind::NvidiaNim,
    ProviderKind::OpenAi,
    ProviderKind::Ollama,
    ProviderKind::Custom,
];

pub fn mask_key(key: &str) -> String {
    let trimmed = key.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    if trimmed.len() <= 8 {
        return "********".to_string();
    }
    let prefix = &trimmed[..6.min(trimmed.len())];
    let suffix = &trimmed[trimmed.len().saturating_sub(4)..];
    format!("{}...{}", prefix, suffix)
}

fn default_provider() -> ProviderKind {
    ProviderKind::OpenRouter
}

impl ProviderKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::OpenRouter => "OpenRouter",
            Self::NvidiaNim => "NVIDIA NIM",
            Self::OpenAi => "OpenAI",
            Self::Ollama => "Ollama (Local)",
            Self::Custom => "Custom Endpoint",
        }
    }

    pub fn default_base_url(&self) -> &'static str {
        match self {
            Self::OpenRouter => "https://openrouter.ai/api/v1",
            Self::NvidiaNim => "https://integrate.api.nvidia.com/v1",
            Self::OpenAi => "https://api.openai.com/v1",
            Self::Ollama => "http://localhost:11434/v1",
            Self::Custom => "http://localhost:1234/v1",
        }
    }

    pub fn requires_api_key(&self) -> bool {
        match self {
            Self::OpenRouter | Self::NvidiaNim | Self::OpenAi => true,
            Self::Ollama | Self::Custom => false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_provider")]
    pub provider: ProviderKind,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub provider_keys: HashMap<ProviderKind, String>,
    #[serde(default)]
    pub provider_models: HashMap<ProviderKind, Vec<ModelInfo>>,
    #[serde(default)]
    pub provider_models_updated_at: HashMap<ProviderKind, DateTime<Utc>>,
    #[serde(default)]
    pub provider_active_models: HashMap<ProviderKind, String>,
    #[serde(default)]
    pub active_model: String,
    #[serde(default)]
    pub models_cache: Vec<ModelInfo>,
    #[serde(default)]
    pub models_cache_updated_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub learned_context_lengths: HashMap<String, u32>,
}

impl ModelInfo {
    pub fn is_free(&self) -> bool {
        if self.id.ends_with(":free") {
            return true;
        }
        let prompt_zero = self.pricing.prompt.trim().parse::<f64>().map(|v| v == 0.0).unwrap_or(false);
        let completion_zero = self.pricing.completion.trim().parse::<f64>().map(|v| v == 0.0).unwrap_or(false);
        prompt_zero && completion_zero
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            provider: ProviderKind::OpenRouter,
            base_url: None,
            api_key: String::new(),
            provider_keys: HashMap::new(),
            provider_models: HashMap::new(),
            provider_models_updated_at: HashMap::new(),
            provider_active_models: HashMap::new(),
            active_model: String::new(),
            models_cache: Vec::new(),
            models_cache_updated_at: None,
            learned_context_lengths: HashMap::new(),
        }
    }
}

impl Config {
    pub fn config_path() -> Result<PathBuf> {
        let home = dirs::home_dir().context("Could not find user home directory")?;
        let config_dir = home.join(".config").join("voidrecon");
        fs::create_dir_all(&config_dir).context("Failed to create config directory ~/.config/voidrecon")?;
        Ok(config_dir.join("config.json"))
    }

    pub fn load() -> Result<Self> {
        let path = Self::config_path()?;
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = fs::read_to_string(&path).context("Failed to read config file")?;
        let mut config: Config = serde_json::from_str(&content).context("Failed to parse config JSON")?;
        if !config.api_key.trim().is_empty() && !config.provider_keys.contains_key(&config.provider) {
            config.provider_keys.insert(config.provider, config.api_key.clone());
        }
        if !config.models_cache.is_empty() && !config.provider_models.contains_key(&config.provider) {
            config.provider_models.insert(config.provider, config.models_cache.clone());
        }
        if !config.active_model.is_empty() && !config.provider_active_models.contains_key(&config.provider) {
            config.provider_active_models.insert(config.provider, config.active_model.clone());
        }
        Ok(config)
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::config_path()?;
        let content = serde_json::to_string_pretty(self).context("Failed to serialize config")?;
        fs::write(&path, content).context("Failed to write config file")?;
        Ok(())
    }

    pub fn get_models_for_provider(&self, provider: ProviderKind) -> &[ModelInfo] {
        if let Some(list) = self.provider_models.get(&provider) {
            list.as_slice()
        } else if self.provider == provider {
            self.models_cache.as_slice()
        } else {
            &[]
        }
    }

    pub fn context_limit_key(provider: ProviderKind, model: &str) -> String {
        format!("{}:{}", provider.as_str(), model)
    }

    pub fn context_limit_for(&self, provider: ProviderKind, model: &str) -> u32 {
        self.learned_context_lengths
            .get(&Self::context_limit_key(provider, model))
            .copied()
            .unwrap_or(DEFAULT_CONTEXT_LENGTH)
    }

    pub fn compaction_threshold_for(&self, provider: ProviderKind, model: &str) -> u32 {
        let key = Self::context_limit_key(provider, model);
        self.learned_context_lengths
            .get(&key)
            .map(|limit| limit.saturating_mul(80) / 100)
            .unwrap_or(DEFAULT_CONTEXT_LENGTH)
    }

    pub fn record_context_limit(&mut self, key: String, observed_limit: u32) -> bool {
        use std::collections::hash_map::Entry;

        match self.learned_context_lengths.entry(key) {
            Entry::Vacant(entry) => {
                entry.insert(observed_limit);
                true
            }
            Entry::Occupied(mut entry) if observed_limit < *entry.get() => {
                entry.insert(observed_limit);
                true
            }
            Entry::Occupied(_) => false,
        }
    }

    pub fn get_free_models_for_provider(&self, provider: ProviderKind) -> Vec<&ModelInfo> {
        self.get_models_for_provider(provider)
            .iter()
            .filter(|m| m.is_free())
            .collect()
    }

    pub fn search_models_for_provider(&self, provider: ProviderKind, query: &str) -> Vec<&ModelInfo> {
        let query = query.to_lowercase();
        self.get_models_for_provider(provider)
            .iter()
            .filter(|m| m.id.to_lowercase().contains(&query) || m.name.to_lowercase().contains(&query))
            .collect()
    }

    pub fn get_api_key(&self, provider: ProviderKind) -> &str {
        if let Some(key) = self.provider_keys.get(&provider) {
            if !key.trim().is_empty() {
                return key.as_str();
            }
        }
        if self.provider == provider {
            &self.api_key
        } else {
            ""
        }
    }

    pub fn set_api_key(&mut self, provider: ProviderKind, key: String) {
        let trimmed = key.trim().to_string();
        self.provider_keys.insert(provider, trimmed.clone());
        if self.provider == provider {
            self.api_key = trimmed;
        }
    }

    pub fn has_api_key(&self, provider: ProviderKind) -> bool {
        if !provider.requires_api_key() {
            return true;
        }
        !self.get_api_key(provider).trim().is_empty()
    }

    pub fn is_configured(&self, provider: ProviderKind) -> bool {
        match provider {
            ProviderKind::Ollama => {
                self.provider == ProviderKind::Ollama
                    || self.has_api_key(provider)
                    || (self.base_url.is_some() && self.provider == ProviderKind::Ollama)
            }
            ProviderKind::Custom => {
                self.provider == ProviderKind::Custom
                    || self.has_api_key(provider)
                    || (self.base_url.is_some() && self.provider == ProviderKind::Custom)
            }
            _ => self.has_api_key(provider) || self.provider == provider,
        }
    }

    pub fn configured_providers(&self) -> Vec<ProviderKind> {
        let list: Vec<ProviderKind> = ALL_PROVIDERS
            .iter()
            .copied()
            .filter(|p| self.is_configured(*p))
            .collect();
        if list.is_empty() {
            vec![self.provider]
        } else {
            list
        }
    }

    pub fn is_cache_stale(&self) -> bool {
        match self.provider_models_updated_at.get(&self.provider).copied().or(self.models_cache_updated_at) {
            Some(updated) => {
                let now = Utc::now();
                (now - updated).num_hours() >= 24
            }
            None => true,
        }
    }

    pub fn set_provider(&mut self, provider: ProviderKind, custom_base_url: Option<String>) {
        self.provider = provider;
        self.base_url = custom_base_url;
        self.api_key = self.get_api_key(provider).to_string();
        if let Some(cached) = self.provider_models.get(&provider) {
            self.models_cache = cached.clone();
            self.models_cache_updated_at = self.provider_models_updated_at.get(&provider).copied();
        } else {
            self.models_cache.clear();
            self.models_cache_updated_at = None;
        }
        if let Some(model) = self.provider_active_models.get(&provider) {
            self.active_model = model.clone();
        }
    }

    pub fn update_models_cache_for_provider(&mut self, provider: ProviderKind, models: Vec<ModelInfo>) {
        let now = Utc::now();
        self.provider_models.insert(provider, models.clone());
        self.provider_models_updated_at.insert(provider, now);
        if self.provider == provider {
            self.models_cache = models;
            self.models_cache_updated_at = Some(now);
            if self.active_model.is_empty() && !self.models_cache.is_empty() {
                self.active_model = self.models_cache[0].id.clone();
            }
        }
    }
}