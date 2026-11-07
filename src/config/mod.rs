//! Configuration module for voidrecon
//! Handles config.json read/write, model cache, and API key management.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use dirs;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
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
    pub api_key: String,
    pub active_model: String,
    pub models_cache: Vec<ModelInfo>,
    pub models_cache_updated_at: Option<DateTime<Utc>>,
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
            active_model: String::new(),
            models_cache: Vec::new(),
            models_cache_updated_at: None,
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
        let config: Config = serde_json::from_str(&content).context("Failed to parse config JSON")?;
        Ok(config)
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::config_path()?;
        let content = serde_json::to_string_pretty(self).context("Failed to serialize config")?;
        fs::write(&path, content).context("Failed to write config file")?;
        Ok(())
    }

    pub fn is_cache_stale(&self) -> bool {
        match self.models_cache_updated_at {
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
        self.models_cache.clear();
        self.active_model.clear();
        self.models_cache_updated_at = None;
    }

    pub fn update_models_cache(&mut self, models: Vec<ModelInfo>) {
        self.models_cache = models;
        self.models_cache_updated_at = Some(Utc::now());
    }

    pub fn get_free_models(&self) -> Vec<&ModelInfo> {
        self.models_cache
            .iter()
            .filter(|m| m.is_free())
            .collect()
    }

    pub fn search_models(&self, query: &str) -> Vec<&ModelInfo> {
        let query = query.to_lowercase();
        self.models_cache
            .iter()
            .filter(|m| m.id.to_lowercase().contains(&query) || m.name.to_lowercase().contains(&query))
            .collect()
    }
}