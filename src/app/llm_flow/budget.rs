//! Token budgeting: size estimation, truncation, and fitting a request into
//! the model's context window.

use anyhow::{bail, Context, Result};
use std::collections::HashSet;

use crate::app::state::{App, NotifLevel};
use crate::llm::{ChatMessage, LlmRequestState};
use crate::tools::get_tool_definitions;

impl App {
    pub(super) fn model_context_length(&self) -> usize {
        self.config
            .context_limit_for(self.config.provider, &self.config.active_model) as usize
    }

    pub(super) fn output_token_budget(&self, preferred: u32) -> u32 {
        preferred.min((self.model_context_length() / 4).max(128) as u32)
    }

    pub(super) fn input_token_budget(&self, output_tokens: u32) -> usize {
        self.model_context_length()
            .saturating_sub(output_tokens as usize)
    }

    pub(super) fn estimate_text_tokens(text: &str) -> usize {
        let ascii = text.chars().filter(|ch| ch.is_ascii()).count();
        let non_ascii = text.len().saturating_sub(ascii);
        ascii / 3 + non_ascii + 8
    }

    pub(super) fn estimate_message_tokens(message: &ChatMessage) -> usize {
        let mut tokens = Self::estimate_text_tokens(&message.role)
            + Self::estimate_text_tokens(&message.content)
            + 4;
        if let Some(tool_calls) = &message.tool_calls {
            if let Ok(serialized) = serde_json::to_string(tool_calls) {
                tokens += Self::estimate_text_tokens(&serialized);
            }
        }
        if let Some(tool_call_id) = &message.tool_call_id {
            tokens += Self::estimate_text_tokens(tool_call_id);
        }
        tokens
    }

    /// Keep the beginning and end of long text at UTF-8 safe boundaries.
    /// Command output kept per chat-history message before it is excerpted.
    pub(crate) const CHAT_OUTPUT_BYTES: usize = 16_000;
    /// Command output included in the post-execution analysis request.
    pub(super) const ANALYSIS_OUTPUT_BYTES: usize = 12_000;
    const SAVED_OUTPUT_PREFIX: &'static str = "[Full output saved: ";

    /// Prefixes long command output with the file holding its full text.
    pub(crate) fn output_with_saved_path(output: &str, path: &str) -> String {
        format!(
            "{}{}; {} lines]\n{}",
            Self::SAVED_OUTPUT_PREFIX,
            path,
            output.lines().count(),
            output
        )
    }

    /// Saved-output path from a header written by [`Self::output_with_saved_path`].
    pub(super) fn saved_output_path(content: &str) -> Option<&str> {
        let rest = content.strip_prefix(Self::SAVED_OUTPUT_PREFIX)?;
        let header = rest.lines().next()?;
        header.split(';').next().map(str::trim)
    }

    /// Shortens command output by whole lines, keeping the beginning and the
    /// (usually more important) end. The marker states exactly which lines are
    /// missing and, when the full output was saved, how to read them, so the
    /// model never mistakes an excerpt for the complete output.
    pub fn output_excerpt(text: &str, max_bytes: usize, full_path: Option<&str>) -> String {
        if text.len() <= max_bytes {
            return text.to_string();
        }
        let lines: Vec<&str> = text.lines().collect();
        let total = lines.len();
        let content_budget = max_bytes.saturating_sub(400);
        let head_budget = content_budget / 3;
        let tail_budget = content_budget - head_budget;

        let mut head = 0;
        let mut used = 0;
        while head < total && used + lines[head].len() < head_budget {
            used += lines[head].len() + 1;
            head += 1;
        }
        let mut tail = 0;
        used = 0;
        while tail < total - head && used + lines[total - 1 - tail].len() < tail_budget {
            used += lines[total - 1 - tail].len() + 1;
            tail += 1;
        }
        if head + tail == 0 {
            // A few enormous lines: fall back to a byte cut.
            return Self::truncate_bytes(text, max_bytes);
        }

        let omitted_from = head + 1;
        let omitted_to = total - tail;
        let hint = match full_path {
            Some(path) => format!(
                " Full output: {}. Read the omitted part with read_workspace_file (path, start_line={}) or search it with pattern.",
                path, omitted_from
            ),
            None => String::new(),
        };
        format!(
            "{}\n[... lines {}-{} of {} omitted.{} ...]\n{}",
            lines[..head].join("\n"),
            omitted_from,
            omitted_to,
            total,
            hint,
            lines[total - tail..].join("\n")
        )
    }

    pub(super) fn truncate_bytes(text: &str, max_bytes: usize) -> String {
        if text.len() <= max_bytes {
            return text.to_string();
        }
        let marker = "\n...[truncated]...\n";
        let content_budget = max_bytes.saturating_sub(marker.len());
        let head_end = Self::utf8_boundary_before(text, content_budget * 2 / 3);
        let tail_start =
            Self::utf8_boundary_after(text, text.len().saturating_sub(content_budget / 3));
        format!("{}{}{}", &text[..head_end], marker, &text[tail_start..])
    }

    pub(super) fn estimate_chat_tokens(chat_log: &[ChatMessage]) -> usize {
        chat_log.iter().map(Self::estimate_message_tokens).sum()
    }

    /// Approximate token cost of the tool schemas sent with every request.
    pub(super) fn tool_definitions_tokens() -> Result<usize> {
        let serialized = serde_json::to_string(&get_tool_definitions())
            .context("Failed to estimate LLM tool schema size")?;
        Ok(Self::estimate_text_tokens(&serialized))
    }

    pub(super) fn quote_untrusted(text: &str) -> String {
        format!("{text:?}")
    }

    pub(super) fn remove_incomplete_tool_cycles(messages: Vec<ChatMessage>) -> Vec<ChatMessage> {
        let mut complete = Vec::with_capacity(messages.len());
        let mut index = 0;
        while index < messages.len() {
            let message = &messages[index];
            let Some(calls) = message
                .tool_calls
                .as_ref()
                .filter(|_| message.role == "assistant")
            else {
                if message.role != "tool" {
                    complete.push(message.clone());
                }
                index += 1;
                continue;
            };

            let mut expected: HashSet<&str> = calls.iter().map(|call| call.id.as_str()).collect();
            let mut cycle = vec![message.clone()];
            index += 1;
            while index < messages.len() && messages[index].role == "tool" {
                if let Some(id) = messages[index].tool_call_id.as_deref() {
                    if expected.remove(id) {
                        cycle.push(messages[index].clone());
                    }
                }
                index += 1;
            }
            if expected.is_empty() {
                complete.extend(cycle);
            }
        }
        complete
    }

    pub(super) fn drop_oldest_history_unit(history: &mut Vec<ChatMessage>) {
        if history.is_empty() {
            return;
        }
        let first = history.remove(0);
        if let Some(calls) = first.tool_calls {
            let mut pending: HashSet<String> = calls.into_iter().map(|call| call.id).collect();
            while history
                .first()
                .map(|message| message.role == "tool")
                .unwrap_or(false)
            {
                let result = history.remove(0);
                if let Some(id) = result.tool_call_id {
                    pending.remove(&id);
                }
                if pending.is_empty() {
                    break;
                }
            }
        }
    }

    pub(super) fn split_latest_turn(history: &mut Vec<ChatMessage>) -> Vec<ChatMessage> {
        let latest_query = history.iter().rposition(|message| {
            message.role == "user" && message.content.starts_with("User query:")
        });
        let latest_tool_cycle = history
            .iter()
            .rposition(|message| message.role == "assistant" && message.tool_calls.is_some());
        let latest_message = history.len().saturating_sub(1);
        let turn_start = latest_query.or(latest_tool_cycle).unwrap_or(latest_message);
        history.split_off(turn_start)
    }

    pub(super) fn fit_request_messages(
        &self,
        system_prompt: String,
        history: Vec<ChatMessage>,
        trailing_messages: Vec<ChatMessage>,
        output_tokens: u32,
    ) -> Result<Vec<ChatMessage>> {
        let tool_tokens = Self::tool_definitions_tokens()?;
        let input_budget = self.input_token_budget(output_tokens);
        let mut history = Self::remove_incomplete_tool_cycles(history);
        let mut trailing_messages = Self::remove_incomplete_tool_cycles(trailing_messages);
        let trailing_tokens = Self::estimate_chat_tokens(&trailing_messages);
        let mut system_prompt = system_prompt;
        let available_system_tokens = input_budget.saturating_sub(tool_tokens + trailing_tokens);
        if Self::estimate_text_tokens(&system_prompt) > available_system_tokens {
            system_prompt = Self::fit_system_prompt(&system_prompt, available_system_tokens)?;
        }
        let system_message = ChatMessage::system(system_prompt);
        let fixed_tokens =
            Self::estimate_message_tokens(&system_message) + tool_tokens + trailing_tokens;
        if fixed_tokens > input_budget {
            bail!(
                "LLM context is too small for the system prompt, tools, and current input (estimated {} tokens; input budget {}). Reduce session context or select a model with a larger context window.",
                fixed_tokens,
                input_budget
            );
        }

        while fixed_tokens + Self::estimate_chat_tokens(&history) > input_budget
            && !history.is_empty()
        {
            Self::drop_oldest_history_unit(&mut history);
        }
        if fixed_tokens + Self::estimate_chat_tokens(&history) > input_budget {
            bail!(
                "LLM request exceeds the estimated input token budget after removing older history"
            );
        }

        let mut messages = Vec::with_capacity(history.len() + trailing_messages.len() + 1);
        messages.push(system_message);
        messages.extend(history);
        messages.append(&mut trailing_messages);
        Ok(messages)
    }

    pub(super) fn request_messages_or_notify(
        &mut self,
        system_prompt: String,
        history: Vec<ChatMessage>,
        trailing_messages: Vec<ChatMessage>,
        output_tokens: u32,
    ) -> Option<Vec<ChatMessage>> {
        match self.fit_request_messages(system_prompt, history, trailing_messages, output_tokens) {
            Ok(messages) => {
                let tool_tokens = match Self::tool_definitions_tokens() {
                    Ok(tokens) => tokens,
                    Err(error) => {
                        self.llm.request_state = LlmRequestState::Failed;
                        self.add_notification(
                            format!("Failed to estimate LLM tool schema size: {}", error),
                            NotifLevel::Error,
                        );
                        return None;
                    }
                };
                self.llm.pending_context_tokens = Some(
                    Self::estimate_chat_tokens(&messages)
                        .saturating_add(tool_tokens)
                        .saturating_add(output_tokens as usize),
                );
                self.llm.pending_context_limit_key =
                    Some(crate::config::Config::context_limit_key(
                        self.config.provider,
                        &self.config.active_model,
                    ));
                Some(messages)
            }
            Err(error) => {
                self.llm.request_state = LlmRequestState::Failed;
                self.add_notification(
                    format!("LLM request was not sent: {}", error),
                    NotifLevel::Error,
                );
                None
            }
        }
    }

    pub(super) fn fit_system_prompt(prompt: &str, max_tokens: usize) -> Result<String> {
        let mut max_bytes = max_tokens.saturating_mul(3);
        loop {
            let candidate = Self::truncate_system_prompt(prompt, max_bytes)?;
            let candidate_tokens = Self::estimate_text_tokens(&candidate);
            if candidate_tokens <= max_tokens {
                return Ok(candidate);
            }
            if max_bytes == 0 {
                bail!("The model context is too small to fit the required system instructions");
            }
            let reduced = max_bytes.saturating_mul(max_tokens) / candidate_tokens;
            max_bytes = if reduced >= max_bytes {
                max_bytes - 1
            } else {
                reduced
            };
        }
    }

    pub(super) fn truncate_system_prompt(prompt: &str, max_bytes: usize) -> Result<String> {
        const MARKER: &str = "\n\n[Session details omitted to fit the model context. Use the database tools to retrieve omitted details.]\n\n";
        if prompt.len() <= max_bytes {
            return Ok(prompt.to_string());
        }
        let section_start = prompt
            .find("\n\n## Session & Targets")
            .unwrap_or(prompt.len());
        let policy = &prompt[..section_start];
        let available = max_bytes.saturating_sub(policy.len() + MARKER.len());
        if available < 256 {
            bail!(
                "The model context is too small to fit the required system instructions and tools (system instructions need at least {} bytes).",
                policy.len() + MARKER.len() + 256
            );
        }
        let context = &prompt[section_start..];
        let head_bytes = available / 2;
        let tail_bytes = available - head_bytes;
        let head_end = Self::utf8_boundary_after(context, head_bytes);
        let tail_start =
            Self::utf8_boundary_before(context, context.len().saturating_sub(tail_bytes));
        Ok(format!(
            "{}{}{}{}{}",
            policy,
            MARKER,
            &context[..head_end],
            "\n[...]\n",
            &context[tail_start..]
        ))
    }

    pub(super) fn utf8_boundary_before(text: &str, index: usize) -> usize {
        let mut boundary = index.min(text.len());
        while boundary > 0 && !text.is_char_boundary(boundary) {
            boundary -= 1;
        }
        boundary
    }

    pub(super) fn utf8_boundary_after(text: &str, index: usize) -> usize {
        let mut boundary = index.min(text.len());
        while boundary < text.len() && !text.is_char_boundary(boundary) {
            boundary += 1;
        }
        boundary
    }

    pub(super) fn is_context_length_error(error: &str) -> bool {
        let error = error.to_ascii_lowercase();
        [
            "maximum context length",
            "max context length",
            "context length",
            "context_length_exceeded",
            "context window",
            "too many tokens",
            "prompt is too long",
            "input is too long",
        ]
        .iter()
        .any(|phrase| error.contains(phrase))
    }

    pub(super) fn context_limit_from_error(error: &str) -> Option<usize> {
        let lower = error.to_ascii_lowercase();
        for marker in [
            "maximum context length is",
            "max context length is",
            "maximum context window of",
            "context window of",
        ] {
            let Some(index) = lower.find(marker) else {
                continue;
            };
            let number = error[index + marker.len()..]
                .trim_start()
                .chars()
                .take_while(|character| character.is_ascii_digit() || *character == ',')
                .filter(|character| *character != ',')
                .collect::<String>();
            if let Ok(limit) = number.parse() {
                return Some(limit);
            }
        }
        None
    }
}
