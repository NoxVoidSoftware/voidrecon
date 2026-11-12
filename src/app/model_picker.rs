//! Model picker key handling
use crate::app::state::{App, NotifLevel};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use anyhow::Result;

impl App {
    pub async fn handle_model_picker_key(&mut self, key: KeyEvent) -> Result<()> {
        let provider = self.ui_state.model_picker_provider;
        let models_count = if self.ui_state.model_picker_free_only {
            self.config.get_free_models_for_provider(provider).len()
        } else {
            self.config.search_models_for_provider(provider, &self.ui_state.model_picker_search).len()
        };

        match key.code {
            KeyCode::Esc | KeyCode::F(2) => {
                self.ui_state.show_model_picker = false;
                self.ui_state.model_picker_search.clear();
                self.ui_state.model_picker_selected = 0;
            }
            KeyCode::F(5) => {
                let prov = self.ui_state.model_picker_provider;
                self.trigger_fetch_models_for_provider(prov);
                self.add_notification(
                    format!("Fetching models from {}...", prov.as_str()),
                    NotifLevel::Info,
                );
            }
            KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                let prov = self.ui_state.model_picker_provider;
                self.trigger_fetch_models_for_provider(prov);
                self.add_notification(
                    format!("Fetching models from {}...", prov.as_str()),
                    NotifLevel::Info,
                );
            }
            KeyCode::Up => {
                if self.ui_state.model_picker_selected > 0 {
                    self.ui_state.model_picker_selected -= 1;
                }
            }
            KeyCode::Down => {
                if models_count > 0 && self.ui_state.model_picker_selected + 1 < models_count {
                    self.ui_state.model_picker_selected += 1;
                }
            }
            KeyCode::Tab => {
                let configured = self.config.configured_providers();
                if configured.len() > 1 {
                    let cur = self.ui_state.model_picker_provider;
                    let cur_idx = configured.iter().position(|&p| p == cur).unwrap_or(0);
                    let next_idx = (cur_idx + 1) % configured.len();
                    let next_provider = configured[next_idx];
                    self.ui_state.model_picker_provider = next_provider;
                    self.ui_state.model_picker_selected = 0;
                    self.ui_state.model_picker_search.clear();

                    if self.config.get_models_for_provider(next_provider).is_empty() {
                        self.trigger_fetch_models_for_provider(next_provider);
                    }
                }
            }
            KeyCode::BackTab => {
                let configured = self.config.configured_providers();
                if configured.len() > 1 {
                    let cur = self.ui_state.model_picker_provider;
                    let cur_idx = configured.iter().position(|&p| p == cur).unwrap_or(0);
                    let prev_idx = if cur_idx == 0 { configured.len() - 1 } else { cur_idx - 1 };
                    let prev_provider = configured[prev_idx];
                    self.ui_state.model_picker_provider = prev_provider;
                    self.ui_state.model_picker_selected = 0;
                    self.ui_state.model_picker_search.clear();

                    if self.config.get_models_for_provider(prev_provider).is_empty() {
                        self.trigger_fetch_models_for_provider(prev_provider);
                    }
                }
            }
            KeyCode::F(4) => {
                self.ui_state.model_picker_free_only = !self.ui_state.model_picker_free_only;
                self.ui_state.model_picker_selected = 0;
            }
            KeyCode::Char('f') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.ui_state.model_picker_free_only = !self.ui_state.model_picker_free_only;
                self.ui_state.model_picker_selected = 0;
            }
            KeyCode::Enter => {
                let provider = self.ui_state.model_picker_provider;
                let models: Vec<&crate::config::ModelInfo> = if self.ui_state.model_picker_free_only {
                    self.config.get_free_models_for_provider(provider)
                } else {
                    self.config.search_models_for_provider(provider, &self.ui_state.model_picker_search)
                };
                if let Some(selected) = models.get(self.ui_state.model_picker_selected) {
                    let model_id = selected.id.clone();
                    self.config.set_provider(provider, None);
                    self.config.provider_active_models.insert(provider, model_id.clone());
                    self.config.active_model = model_id.clone();
                    if let Err(e) = self.config.save() {
                        self.add_notification(format!("Failed to save config: {}", e), NotifLevel::Error);
                    } else {
                        if let Ok(client) = crate::llm::LlmClient::from_config(&self.config) {
                        self.llm_client = client;
                    } else {
                        self.add_notification("Failed to rebuild LLM client".to_string(), NotifLevel::Error);
                    }
                        self.add_notification(
                            format!("Active model: {} ({})", self.config.active_model, provider.as_str()),
                            NotifLevel::Info,
                        );
                    }
                }
                self.ui_state.show_model_picker = false;
                self.ui_state.model_picker_search.clear();
                self.ui_state.model_picker_selected = 0;
            }
            KeyCode::Char(c) => {
                self.ui_state.model_picker_search.push(c);
                self.ui_state.model_picker_selected = 0;
            }
            KeyCode::Backspace => {
                self.ui_state.model_picker_search.pop();
                self.ui_state.model_picker_selected = 0;
            }
            _ => {}
        }
        Ok(())
    }


}

impl App {
    pub async fn switch_to_provider(&mut self, provider: crate::config::ProviderKind) -> Result<()> {
        if self.config.provider == provider {
            return Ok(());
        }
        self.config.set_provider(provider, None);
        let _ = self.config.save();
        if let Ok(client) = crate::llm::LlmClient::from_config(&self.config) {
                        self.llm_client = client;
                    } else {
                        self.add_notification("Failed to rebuild LLM client".to_string(), NotifLevel::Error);
                    }
        self.ui_state.model_picker_selected = 0;
        self.ui_state.model_picker_search.clear();

        if let Some(cached) = self.config.provider_models.get(&provider) {
            if !cached.is_empty() {
                self.config.models_cache = cached.clone();
                if self.config.active_model.is_empty() {
                    if let Some(first) = self.config.models_cache.first() {
                        self.config.active_model = first.id.clone();
                    }
                }
                self.add_notification(
                    format!("Switched to {} ({} models loaded)", provider.as_str(), self.config.models_cache.len()),
                    NotifLevel::Info,
                );
                return Ok(());
            }
        }

        self.add_notification(
            format!("Switched to {}. Fetching models...", provider.as_str()),
            NotifLevel::Info,
        );
        self.trigger_fetch_models();
        Ok(())
    }
    
}
