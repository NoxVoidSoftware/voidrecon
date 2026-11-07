//! Multi-provider LLM client for voidrecon
//! Integrates OpenRouter, OpenAI, Ollama, and Custom OpenAI-compatible endpoints with SSE streaming.

use anyhow::{Context, Result};
use reqwest::{Client, header::CONTENT_TYPE};
use std::sync::Arc;
use std::time::Duration;

use crate::config::{Config, ModelInfo, ModelPricing, ProviderKind};
use crate::llm::provider::{LlmProvider, Provider};
use crate::llm::types::{
    ChatCompletionRequest, ChatCompletionResponse, Choice, StreamChunk,
};

#[derive(Clone)]
pub struct LlmClient {
    inner: Arc<LlmClientInner>,
}

struct LlmClientInner {
    client: Client,
    provider: Provider,
}

impl LlmClient {
    pub fn new(provider: Provider) -> Self {
        let inner = LlmClientInner {
            client: Client::builder()
                .timeout(Duration::from_secs(120))
                .build()
                .expect("Failed to create HTTP client"),
            provider,
        };
        Self {
            inner: Arc::new(inner),
        }
    }

    pub fn from_config(config: &Config) -> Self {
        let provider = Provider::from_config(
            config.provider,
            &config.api_key,
            config.base_url.clone(),
        );
        Self::new(provider)
    }

    pub fn provider(&self) -> &Provider {
        &self.inner.provider
    }

    /// Fetch available models from the active provider dynamically
    pub async fn fetch_models(&self) -> Result<Vec<ModelInfo>> {
        let mut req = self
            .inner
            .client
            .get(self.inner.provider.models_endpoint());

        if let Some(key) = self.inner.provider.api_key() {
            if !key.trim().is_empty() {
                req = req.header("Authorization", format!("Bearer {}", key.trim()));
            }
        }

        let response = req.send().await.context("Failed to fetch models from provider")?;

        if !response.status().is_success() {
            let err = response.text().await.unwrap_or_default();
            anyhow::bail!("Failed to fetch models from {}: {}", self.inner.provider.name(), err);
        }

        let text = response.text().await.context("Failed to read models response body")?;
        let json_val: serde_json::Value = serde_json::from_str(&text)
            .context("Failed to parse models response as JSON")?;

        let mut models = Vec::new();

        // 1. Check if "data" is an array (OpenRouter, NVIDIA NIM, OpenAI, vLLM, LM Studio)
        if let Some(items) = json_val.get("data").and_then(|v| v.as_array()) {
            for item in items {
                if let Some(id) = item.get("id").and_then(|v| v.as_str()) {
                    let name = item.get("name").and_then(|v| v.as_str()).unwrap_or(id).to_string();
                    let description = item.get("description").and_then(|v| v.as_str()).map(|s| s.to_string());
                    let context_length = item.get("context_length").and_then(|v| v.as_u64()).map(|n| n as u32);
                    let prompt_price = item
                        .get("pricing")
                        .and_then(|p| p.get("prompt"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("0")
                        .to_string();
                    let completion_price = item
                        .get("pricing")
                        .and_then(|p| p.get("completion"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("0")
                        .to_string();

                    models.push(ModelInfo {
                        id: id.to_string(),
                        name,
                        description,
                        context_length,
                        pricing: ModelPricing {
                            prompt: prompt_price,
                            completion: completion_price,
                        },
                        architecture: None,
                    });
                }
            }
        }
        // 2. Check if "models" is an array (Ollama native /api/tags or similar)
        else if let Some(items) = json_val.get("models").and_then(|v| v.as_array()) {
            for item in items {
                let id = item.get("model").or_else(|| item.get("name")).and_then(|v| v.as_str());
                if let Some(id) = id {
                    let name = item.get("name").and_then(|v| v.as_str()).unwrap_or(id).to_string();
                    models.push(ModelInfo {
                        id: id.to_string(),
                        name,
                        description: Some("Local Ollama model".to_string()),
                        context_length: None,
                        pricing: ModelPricing {
                            prompt: "0".to_string(),
                            completion: "0".to_string(),
                        },
                        architecture: None,
                    });
                }
            }
        }
        // 3. Check if root is an array
        else if let Some(items) = json_val.as_array() {
            for item in items {
                if let Some(id) = item.get("id").and_then(|v| v.as_str()) {
                    let name = item.get("name").and_then(|v| v.as_str()).unwrap_or(id).to_string();
                    models.push(ModelInfo {
                        id: id.to_string(),
                        name,
                        description: None,
                        context_length: None,
                        pricing: ModelPricing {
                            prompt: "0".to_string(),
                            completion: "0".to_string(),
                        },
                        architecture: None,
                    });
                }
            }
        }

        models.sort_by(|a, b| a.id.cmp(&b.id));

        Ok(models)
    }

    /// Stream chat completion with SSE across any connected provider
    pub async fn stream_chat(
        &self,
        request: ChatCompletionRequest,
        tx: tokio::sync::mpsc::UnboundedSender<crate::app::AppEvent>,
        request_id: u64,
    ) -> Result<ChatCompletionResponse> {
        let mut request = request;
        request.stream = true;

        let mut req = self
            .inner
            .client
            .post(self.inner.provider.chat_endpoint())
            .header(CONTENT_TYPE, "application/json");

        if let Some(key) = self.inner.provider.api_key() {
            if !key.trim().is_empty() {
                req = req.header("Authorization", format!("Bearer {}", key.trim()));
            }
        }

        if self.inner.provider.kind() == ProviderKind::OpenRouter {
            req = req
                .header("HTTP-Referer", "https://github.com/NoxVoid/voidrecon")
                .header("X-Title", "voidrecon");
        }

        let response = req.json(&request).send().await.context("Failed to send chat request")?;

        if !response.status().is_success() {
            let err = response.text().await.unwrap_or_default();
            let _ = tx.send(crate::app::AppEvent::LlmRequestFailed(request_id, err.clone()));
            anyhow::bail!("Chat request failed for {}: {}", self.inner.provider.name(), err);
        }

        let _ = tx.send(crate::app::AppEvent::LlmRequestStarted(request_id));

        let mut stream = response.bytes_stream();
        let mut tool_calls_accumulated: Vec<crate::llm::types::ToolCall> = Vec::new();

        use futures_util::StreamExt;
        while let Some(chunk_result) = stream.next().await {
            let chunk = chunk_result.context("Stream error")?;
            let text = String::from_utf8_lossy(&chunk);

            for line in text.lines() {
                if line.starts_with("data: ") {
                    let data = &line[6..];
                    if data == "[DONE]" {
                        let _ = tx.send(crate::app::AppEvent::LlmDone(request_id));
                        break;
                    }
                    if let Ok(stream_chunk) = serde_json::from_str::<StreamChunk>(data) {
                        if let Some(choice) = stream_chunk.choices.first() {
                            // Handle content delta
                            let content = &choice.delta.content;
                            if !content.is_empty() {
                                let _ = tx.send(crate::app::AppEvent::LlmChunk(request_id, content.clone()));
                            }
                            // Handle tool call delta
                            if let Some(tool_calls) = &choice.delta.tool_calls {
                                for tc in tool_calls {
                                    if tc.id.is_empty() {
                                        continue;
                                    }
                                    if let Some(existing) = tool_calls_accumulated.iter_mut().find(|t| t.id == tc.id) {
                                        existing.function.arguments.push_str(&tc.function.arguments);
                                    } else {
                                        tool_calls_accumulated.push(tc.clone());
                                    }
                                }
                            }
                            // Handle finish reason
                            if let Some(finish_reason) = &choice.finish_reason {
                                if finish_reason == "tool_calls" && !tool_calls_accumulated.is_empty() {
                                    for tc in &tool_calls_accumulated {
                                        let _ = tx.send(crate::app::AppEvent::LlmToolCall(request_id, tc.clone()));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // Build final response
        let final_message = if !tool_calls_accumulated.is_empty() {
            crate::llm::types::ChatMessage {
                role: "assistant".to_string(),
                content: String::new(),
                tool_calls: Some(tool_calls_accumulated),
                tool_call_id: None,
            }
        } else {
            crate::llm::types::ChatMessage {
                role: "assistant".to_string(),
                content: String::new(),
                tool_calls: None,
                tool_call_id: None,
            }
        };

        Ok(ChatCompletionResponse {
            id: "stream".to_string(),
            choices: vec![Choice {
                index: 0,
                message: final_message,
                finish_reason: Some("stop".to_string()),
                delta: None,
            }],
            usage: None,
        })
    }
}
