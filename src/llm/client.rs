//! Multi-provider LLM client for voidrecon
//! Integrates OpenRouter, OpenAI, Ollama, and Custom OpenAI-compatible endpoints with SSE streaming.

use anyhow::{Context, Result};
use reqwest::{Client, header::CONTENT_TYPE};
use std::sync::Arc;
use std::time::Duration;

use crate::config::{Config, ModelInfo, ModelPricing, ProviderKind};
use crate::llm::providers::{LlmProvider, Provider};
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

pub fn parse_retry_delay(headers: &reqwest::header::HeaderMap, response_body: &str) -> Duration {
    // 1. Check Retry-After header
    if let Some(retry_after) = headers.get(reqwest::header::RETRY_AFTER) {
        if let Ok(val) = retry_after.to_str() {
            if let Ok(secs) = val.trim().parse::<u64>() {
                return Duration::from_secs(secs.clamp(1, 60));
            }
            if let Ok(secs_f) = val.trim().parse::<f64>() {
                if secs_f > 0.0 {
                    return Duration::from_millis((secs_f * 1000.0).clamp(1000.0, 60000.0) as u64);
                }
            }
        }
    }

    // 2. Check JSON error body
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(response_body) {
        if let Some(secs) = json.get("retry_after").and_then(|v| v.as_f64()).or_else(|| {
            json.get("error").and_then(|e| e.get("retry_after")).and_then(|v| v.as_f64())
        }) {
            if secs > 0.0 {
                return Duration::from_millis((secs * 1000.0).clamp(1000.0, 60000.0) as u64);
            }
        }
    }

    // 3. Search in text for regex-like patterns: "try again in X", "retry after X", "retry in X"
    let lower = response_body.to_lowercase();
    for pattern in ["try again in ", "retry after ", "retry in "] {
        if let Some(pos) = lower.find(pattern) {
            let after = &lower[pos + pattern.len()..];
            let num_str: String = after
                .chars()
                .take_while(|c| c.is_ascii_digit() || *c == '.')
                .collect();
            if let Ok(secs) = num_str.parse::<f64>() {
                if secs > 0.0 {
                    return Duration::from_millis((secs * 1000.0).clamp(1000.0, 60000.0) as u64);
                }
            }
        }
    }

    // Default: 10 seconds (as requested by user)
    Duration::from_secs(10)
}

impl LlmClient {
    pub fn new(provider: Provider) -> Result<Self> {
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .build()
            .context("Failed to create HTTP client")?;
        let inner = LlmClientInner { client, provider };
        Ok(Self { inner: Arc::new(inner) })
    }

    pub fn from_config(config: &Config) -> Result<Self> {
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
            .get(self.inner.provider.models_endpoint())
            .timeout(Duration::from_secs(15));

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
                    let context_length = item
                        .get("context_length")
                        .or_else(|| item.get("context_window"))
                        .or_else(|| item.get("max_context_length"))
                        .and_then(|v| v.as_u64())
                        .map(|n| n as u32);
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

        // Filter out non-chat models (embeddings, reward, detectors, etc.)
        if self.inner.provider.kind() == ProviderKind::NvidiaNim {
            models.retain(|m| {
                let id = m.id.to_lowercase();
                !id.contains("embed")
                    && !id.contains("reward")
                    && !id.contains("detector")
                    && !id.contains("calibration")
                    && !id.contains("nvclip")
            });
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

        let mut retry_count = 0;
        let response = loop {
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

            let resp = match req.json(&request).send().await {
                Ok(resp) => resp,
                Err(e) => {
                    let err_msg = if e.is_connect() {
                        format!("Connection failed to {}: {}", self.inner.provider.name(), e)
                    } else if e.is_timeout() {
                        format!("Request timed out connecting to {}", self.inner.provider.name())
                    } else {
                        format!("HTTP error: {}", e)
                    };
                    let _ = tx.send(crate::app::AppEvent::LlmRequestFailed(request_id, err_msg.clone()));
                    anyhow::bail!(err_msg);
                }
            };

            let status = resp.status();
            if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                retry_count += 1;
                let headers = resp.headers().clone();
                let raw_err = resp.text().await.unwrap_or_default();
                let wait_duration = parse_retry_delay(&headers, &raw_err);
                let wait_secs = wait_duration.as_secs_f32();

                let retry_msg = format!(
                    "\n[⚠️ HTTP 429 Rate Limited from {}. Retrying in {:.1}s (attempt #{})... Press Ctrl+C to cancel]\n",
                    self.inner.provider.name(),
                    wait_secs,
                    retry_count
                );
                let _ = tx.send(crate::app::AppEvent::LlmChunk(request_id, retry_msg));

                tokio::time::sleep(wait_duration).await;
                continue;
            }

            if !status.is_success() {
                let raw_err = resp.text().await.unwrap_or_default();
                let detail = if let Ok(json) = serde_json::from_str::<serde_json::Value>(&raw_err) {
                    json.get("detail")
                        .or_else(|| json.get("message"))
                        .or_else(|| json.get("error").and_then(|e| e.get("message")))
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .unwrap_or(raw_err)
                } else {
                    raw_err
                };
                let err_msg = format!("HTTP {} from {}: {}", status, self.inner.provider.name(), detail);
                let _ = tx.send(crate::app::AppEvent::LlmRequestFailed(request_id, err_msg.clone()));
                anyhow::bail!(err_msg);
            }

            break resp;
        };

        let _ = tx.send(crate::app::AppEvent::LlmRequestStarted(request_id));

        let mut stream = response.bytes_stream();
        let mut tool_calls_accumulated: Vec<crate::llm::types::ToolCall> = Vec::new();
        let mut line_buffer = String::new();

        use futures_util::StreamExt;
        while let Some(chunk_result) = stream.next().await {
            let chunk = match chunk_result {
                Ok(c) => c,
                Err(e) => {
                    let _ = tx.send(crate::app::AppEvent::LlmRequestFailed(request_id, format!("Stream chunk error: {}", e)));
                    break;
                }
            };
            line_buffer.push_str(&String::from_utf8_lossy(&chunk));

            while let Some(newline_pos) = line_buffer.find('\n') {
                let line: String = line_buffer.drain(..=newline_pos).collect();
                let trimmed = line.trim();
                if let Some(data) = trimmed.strip_prefix("data:") {
                    let data = data.trim();
                    if data == "[DONE]" {
                        break;
                    }
                    if let Ok(stream_chunk) = serde_json::from_str::<StreamChunk>(data) {
                        for choice in stream_chunk.choices {
                            // 1. Content delta
                            if let Some(content) = &choice.delta.content {
                                if !content.is_empty() {
                                    let _ = tx.send(crate::app::AppEvent::LlmChunk(request_id, content.clone()));
                                }
                            }
                            // 2. Reasoning delta (DeepSeek R1 / V4.1)
                            if let Some(reasoning) = &choice.delta.reasoning_content {
                                if !reasoning.is_empty() {
                                    let _ = tx.send(crate::app::AppEvent::LlmChunk(request_id, reasoning.clone()));
                                }
                            }
                            // 3. Tool call delta
                            if let Some(tool_calls) = &choice.delta.tool_calls {
                                for tc in tool_calls {
                                    let idx = tc.index.unwrap_or_else(|| {
                                        if tool_calls_accumulated.is_empty() { 0 } else { tool_calls_accumulated.len() - 1 }
                                    });
                                    if idx >= tool_calls_accumulated.len() {
                                        tool_calls_accumulated.push(crate::llm::types::ToolCall {
                                            id: tc.id.clone().unwrap_or_default(),
                                            call_type: tc.call_type.clone().unwrap_or_else(|| "function".to_string()),
                                            function: crate::llm::types::ToolFunction {
                                                name: tc.function.as_ref().and_then(|f| f.name.clone()).unwrap_or_default(),
                                                arguments: tc.function.as_ref().and_then(|f| f.arguments.clone()).unwrap_or_default(),
                                            },
                                        });
                                    } else {
                                        if let Some(id) = &tc.id {
                                            if !id.is_empty() {
                                                tool_calls_accumulated[idx].id = id.clone();
                                            }
                                        }
                                        if let Some(func) = &tc.function {
                                            if let Some(name) = &func.name {
                                                if !name.is_empty() {
                                                    tool_calls_accumulated[idx].function.name.push_str(name);
                                                }
                                            }
                                            if let Some(args) = &func.arguments {
                                                tool_calls_accumulated[idx].function.arguments.push_str(args);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    } else if let Ok(err_val) = serde_json::from_str::<serde_json::Value>(data) {
                        if let Some(err_obj) = err_val.get("error") {
                            let msg = err_obj.get("message")
                                .and_then(|m| m.as_str())
                                .unwrap_or("Server error in stream");
                            let _ = tx.send(crate::app::AppEvent::LlmRequestFailed(request_id, format!("Provider error: {}", msg)));
                            return Ok(ChatCompletionResponse {
                                id: "stream_error".to_string(),
                                choices: vec![],
                                usage: None,
                            });
                        }
                    }
                }
            }
        }

        // Dispatch tool calls if any accumulated
        if !tool_calls_accumulated.is_empty() {
            for tc in &tool_calls_accumulated {
                let _ = tx.send(crate::app::AppEvent::LlmToolCall(request_id, tc.clone()));
            }
        }
        let _ = tx.send(crate::app::AppEvent::LlmDone(request_id));

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

    /// Non-streaming completion used for internal tasks such as history compaction.
    pub async fn complete_chat(&self, mut request: ChatCompletionRequest) -> Result<String> {
        request.stream = false;

        let mut req = self
            .inner
            .client
            .post(self.inner.provider.chat_endpoint())
            .header(CONTENT_TYPE, "application/json")
            .timeout(Duration::from_secs(120));

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

        let resp = req
            .json(&request)
            .send()
            .await
            .context("Failed to send completion request")?;

        let status = resp.status();
        let body = resp.text().await.context("Failed to read completion response")?;

        if !status.is_success() {
            let detail = if let Ok(json) = serde_json::from_str::<serde_json::Value>(&body) {
                json.get("detail")
                    .or_else(|| json.get("message"))
                    .or_else(|| json.get("error").and_then(|e| e.get("message")))
                    .and_then(|v| v.as_str())
                    .unwrap_or(&body)
                    .to_string()
            } else {
                body
            };
            anyhow::bail!("HTTP {} from {}: {}", status, self.inner.provider.name(), detail);
        }

        let parsed: ChatCompletionResponse =
            serde_json::from_str(&body).context("Failed to parse completion response")?;

        Ok(parsed
            .choices
            .into_iter()
            .next()
            .map(|c| c.message.content)
            .unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_retry_delay_from_header() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(reqwest::header::RETRY_AFTER, "5".parse().unwrap());
        let duration = parse_retry_delay(&headers, "");
        assert_eq!(duration, Duration::from_secs(5));
    }

    #[test]
    fn test_parse_retry_delay_from_json() {
        let headers = reqwest::header::HeaderMap::new();
        let body = r#"{"error": {"retry_after": 7.5}}"#;
        let duration = parse_retry_delay(&headers, body);
        assert_eq!(duration, Duration::from_millis(7500));
    }

    #[test]
    fn test_parse_retry_delay_from_text() {
        let headers = reqwest::header::HeaderMap::new();
        let body = "Rate limit reached. Try again in 4.2s.";
        let duration = parse_retry_delay(&headers, body);
        assert_eq!(duration, Duration::from_millis(4200));
    }

    #[test]
    fn test_parse_retry_delay_default_10s() {
        let headers = reqwest::header::HeaderMap::new();
        let body = "Too many requests";
        let duration = parse_retry_delay(&headers, body);
        assert_eq!(duration, Duration::from_secs(10));
    }
}
