//! LLM client for voidrecon - OpenRouter API integration

use anyhow::{Context, Result};
use reqwest::{Client, header::CONTENT_TYPE};
use std::sync::Arc;
use std::time::Duration;

use crate::llm::types::{ChatCompletionRequest, ChatCompletionResponse, Choice, ModelListResponse, StreamChunk};

#[derive(Clone)]
pub struct LlmClient {
    inner: Arc<LlmClientInner>,
}

#[derive(Clone)]
struct LlmClientInner {
    client: Client,
    api_key: String,
    base_url: String,
}

impl LlmClient {
    pub fn new(api_key: String) -> Self {
        let inner = LlmClientInner {
            client: Client::builder()
                .timeout(Duration::from_secs(120))
                .build()
                .expect("Failed to create HTTP client"),
            api_key,
            base_url: "https://openrouter.ai/api/v1".to_string(),
        };
        Self {
            inner: Arc::new(inner),
        }
    }

    /// Fetch available models from OpenRouter
    pub async fn fetch_models(&self) -> Result<Vec<crate::config::ModelInfo>> {
        let response = self
            .inner
            .client
            .get(format!("{}/models", self.inner.base_url))
            .header("Authorization", format!("Bearer {}", self.inner.api_key))
            .send()
            .await
            .context("Failed to fetch models")?;

        if !response.status().is_success() {
            let err = response.text().await.unwrap_or_default();
            anyhow::bail!("Failed to fetch models: {}", err);
        }

        let model_list: ModelListResponse = response
            .json()
            .await
            .context("Failed to parse models response")?;
        Ok(model_list.data)
    }

    /// Stream chat completion with SSE
    pub async fn stream_chat(
        &self,
        request: ChatCompletionRequest,
        tx: tokio::sync::mpsc::UnboundedSender<crate::app::AppEvent>,
        request_id: u64,
    ) -> Result<ChatCompletionResponse> {
        let mut request = request;
        request.stream = true;

        let response = self
            .inner
            .client
            .post(format!("{}/chat/completions", self.inner.base_url))
            .header("Authorization", format!("Bearer {}", self.inner.api_key))
            .header(CONTENT_TYPE, "application/json")
            .json(&request)
            .send()
            .await
            .context("Failed to send chat request")?;

        if !response.status().is_success() {
            let err = response.text().await.unwrap_or_default();
            let _ = tx.send(crate::app::AppEvent::LlmRequestFailed(request_id, err.clone()));
            anyhow::bail!("Chat request failed: {}", err);
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
                                    // Find or create tool call entry
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
                content: String::new(), // Content was streamed via LlmChunk events
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