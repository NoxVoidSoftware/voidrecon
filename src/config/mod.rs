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
    pub name: String,
    pub description: Option<String>,
    pub context_length: Option<u32>,
    pub pricing: ModelPricing,
    pub architecture: Option<ModelArchitecture>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelPricing {
    pub prompt: String,
    pub completion: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelArchitecture {
    pub modality: String,
    pub input_modalities: Vec<String>,
    pub output_modalities: Vec<String>,
    pub tokenizer: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub api_key: String,
    pub active_model: String,
    pub models_cache: Vec<ModelInfo>,
    pub models_cache_updated_at: Option<DateTime<Utc>>,
}

impl Default for Config {
    fn default() -> Self {
        let fallback_models = vec![
            ModelInfo {
                id: "google/gemini-2.0-flash-exp:free".to_string(),
                name: "Google: Gemini 2.0 Flash (free)".to_string(),
                description: Some("Fast multimodal model with 1M context".to_string()),
                context_length: Some(1048576),
                pricing: ModelPricing {
                    prompt: "0".to_string(),
                    completion: "0".to_string(),
                },
                architecture: None,
            },
            ModelInfo {
                id: "meta-llama/llama-3.3-70b-instruct:free".to_string(),
                name: "Meta: Llama 3.3 70B Instruct (free)".to_string(),
                description: Some("State-of-the-art open weights 70B model".to_string()),
                context_length: Some(131072),
                pricing: ModelPricing {
                    prompt: "0".to_string(),
                    completion: "0".to_string(),
                },
                architecture: None,
            },
            ModelInfo {
                id: "deepseek/deepseek-r1:free".to_string(),
                name: "DeepSeek: R1 (free)".to_string(),
                description: Some("Advanced reasoning and code analysis model".to_string()),
                context_length: Some(65536),
                pricing: ModelPricing {
                    prompt: "0".to_string(),
                    completion: "0".to_string(),
                },
                architecture: None,
            },
            ModelInfo {
                id: "qwen/qwen-2.5-coder-32b-instruct:free".to_string(),
                name: "Qwen: Qwen 2.5 Coder 32B Instruct (free)".to_string(),
                description: Some("Specialized coding & CLI commands model".to_string()),
                context_length: Some(32768),
                pricing: ModelPricing {
                    prompt: "0".to_string(),
                    completion: "0".to_string(),
                },
                architecture: None,
            },
        ];

        Self {
            api_key: String::new(),
            active_model: "google/gemini-2.0-flash-exp:free".to_string(),
            models_cache: fallback_models,
            models_cache_updated_at: None,
        }
    }
}

impl Config {
    pub fn config_path() -> Result<PathBuf> {
        let home = dirs::home_dir().context("Could not find user home directory")?;
        let config_dir = home.join(".audit-tui");
        fs::create_dir_all(&config_dir).context("Failed to create config directory ~/.audit-tui")?;
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

    pub fn update_models_cache(&mut self, models: Vec<ModelInfo>) {
        self.models_cache = models;
        self.models_cache_updated_at = Some(Utc::now());
    }

    pub fn get_free_models(&self) -> Vec<&ModelInfo> {
        self.models_cache
            .iter()
            .filter(|m| {
                m.id.ends_with(":free")
                    || (m.pricing.prompt == "0" && m.pricing.completion == "0")
            })
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