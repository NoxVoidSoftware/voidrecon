//! Provider & API key setup modal

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};

use crate::app::state::{App, NotifLevel};
use crate::config::ALL_PROVIDERS;
use crate::llm::LlmClient;

impl App {
    pub(super) async fn handle_api_key_prompt_key(&mut self, key: KeyEvent) -> Result<()> {
        let total_providers = ALL_PROVIDERS.len();
        match key.code {
            KeyCode::Esc => {
                self.ui_state.show_api_key_prompt = false;
                self.ui_state.api_key_input.clear();
                self.add_notification(
                    "Provider configuration closed".to_string(),
                    NotifLevel::Info,
                );
            }
            KeyCode::Tab | KeyCode::Right => {
                self.ui_state.provider_picker_selected =
                    (self.ui_state.provider_picker_selected + 1) % total_providers;
                self.ui_state.api_key_input.clear();
            }
            KeyCode::BackTab | KeyCode::Left => {
                self.ui_state.provider_picker_selected =
                    if self.ui_state.provider_picker_selected == 0 {
                        total_providers - 1
                    } else {
                        self.ui_state.provider_picker_selected - 1
                    };
                self.ui_state.api_key_input.clear();
            }
            KeyCode::Enter => self.confirm_provider_selection().await,
            KeyCode::Char(c) => self.ui_state.api_key_input.push(c),
            KeyCode::Backspace => {
                self.ui_state.api_key_input.pop();
            }
            _ => {}
        }
        Ok(())
    }

    /// Saves the entered key (if any), activates the selected provider and
    /// rebuilds the LLM client. Keeps the modal open when a required key is missing.
    async fn confirm_provider_selection(&mut self) {
        let selected_provider =
            ALL_PROVIDERS[self.ui_state.provider_picker_selected.min(ALL_PROVIDERS.len() - 1)];
        let key_val = self.ui_state.api_key_input.trim().to_string();

        if !key_val.is_empty() {
            self.config.set_api_key(selected_provider, key_val);
            self.add_notification(
                format!("Saved key for {} and activated", selected_provider.as_str()),
                NotifLevel::Info,
            );
        } else if self.config.has_api_key(selected_provider) {
            self.add_notification(
                format!("Activated {} (using saved key)", selected_provider.as_str()),
                NotifLevel::Info,
            );
        } else if !selected_provider.requires_api_key() {
            self.add_notification(
                format!("Activated {}", selected_provider.as_str()),
                NotifLevel::Info,
            );
        } else {
            self.add_notification(
                format!("No API key entered for {}", selected_provider.as_str()),
                NotifLevel::Warning,
            );
            return;
        }

        self.config.set_provider(selected_provider, None);
        let _ = self.config.save().await;
        if let Ok(client) = LlmClient::from_config(&self.config) {
            self.llm_client = client;
        } else {
            self.add_notification(
                "Failed to rebuild LLM client".to_string(),
                NotifLevel::Error,
            );
        }
        self.ui_state.show_api_key_prompt = false;
        self.ui_state.api_key_input.clear();
        if self.config.models_cache.is_empty() {
            self.trigger_fetch_models();
        }
    }
}
