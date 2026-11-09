//! Application state for voidrecon

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use ratatui::text::Line;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::Instant;
use tokio::sync::mpsc;

use crate::app::events::AppEvent;
use crate::config::Config;
use crate::db::{Database, Target, Finding};
use crate::llm::{ChatMessage, LlmClient, LlmRequestState, LlmSuggestion};
use crate::pty::tab::PtyTab;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputMode {
    Shell,
    LlmQuery,
    EditCommand,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotifLevel {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone)]
pub struct Notification {
    pub message: String,
    pub level: NotifLevel,
    pub created_at: Instant,
}

#[derive(Debug, Clone)]
pub struct SidebarNode {
    pub target: Target,
    pub ports: Vec<crate::db::models::PortInfo>,
    pub expanded: bool,
    pub findings_by_severity: Vec<(String, Vec<Finding>)>,
}

#[derive(Debug, Clone)]
pub enum ApprovalKind {
    ReverseShellListener {
        lhost: String,
        lport: u16,
        shell_type: String,
    },
    ExecuteExploit {
        script_name: String,
        command_line: String,
    },
    SaveWorkspaceFile {
        folder: String,
        filename: String,
        description: String,
        content: String,
    },
}

#[derive(Debug, Clone)]
pub struct ExploitApproval {
    pub target_id: i64,
    pub target_ip: String,
    pub kind: ApprovalKind,
    pub tool_call: crate::llm::types::ToolCall,
}

/// Session metadata
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SessionMeta {
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub last_activity: DateTime<Utc>,
    pub active_target_id: Option<i64>,
}

impl SessionMeta {
    pub fn new(name: String) -> Self {
        let now = Utc::now();
        Self {
            name,
            created_at: now,
            last_activity: now,
            active_target_id: None,
        }
    }

    pub fn sessions_dir() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".config")
            .join("voidrecon")
            .join("sessions")
    }

    pub fn path(&self) -> PathBuf {
        Self::sessions_dir().join(&self.name)
    }

    pub fn db_path(&self) -> PathBuf {
        self.path().join("session.db")
    }

    pub fn meta_path(&self) -> PathBuf {
        self.path().join("session.json")
    }

    pub fn workspace_path(&self) -> PathBuf {
        self.path().join("workspace")
    }

    pub fn chat_history_path(&self) -> PathBuf {
        self.path().join("chat_history.json")
    }

    pub fn load_chat_history(&self) -> Vec<ChatMessage> {
        let p = self.chat_history_path();
        if p.exists() {
            if let Ok(content) = std::fs::read_to_string(&p) {
                if let Ok(msgs) = serde_json::from_str::<Vec<ChatMessage>>(&content) {
                    return msgs;
                }
            }
        }
        Vec::new()
    }

    pub fn save_chat_history(&self, messages: &[ChatMessage]) -> Result<()> {
        if self.name.is_empty() || self.name == "None" {
            return Ok(());
        }
        let p = self.chat_history_path();
        let content = serde_json::to_string_pretty(messages)?;
        std::fs::write(p, content)?;
        Ok(())
    }

    pub fn list_sessions() -> Vec<SessionMeta> {
        let dir = Self::sessions_dir();
        if !dir.exists() {
            return Vec::new();
        }
        let mut list = Vec::new();
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let meta_path = entry.path().join("session.json");
                if meta_path.exists() {
                    if let Ok(meta) = Self::load(&meta_path) {
                        list.push(meta);
                    }
                }
            }
        }
        list.sort_by(|a, b| b.last_activity.cmp(&a.last_activity));
        list
    }
}

pub struct App {
    pub config: Config,
    pub db: Database,
    pub llm_client: LlmClient,
    
    // Session info
    pub session_meta: SessionMeta,
    
    // UI state
    pub ui_state: UiState,
    pub input_mode: InputMode,
    pub input_buffer: String,
    pub command_history: VecDeque<String>,
    pub history_index: Option<usize>,
    
    // Targets & sidebar
    pub sidebar_nodes: Vec<SidebarNode>,
    pub active_target_id: Option<i64>,
    
    // PTY tabs
    pub tabs: Vec<PtyTab>,
    pub active_tab: usize,
    pub next_tab_id: usize,
    
    // LLM state
    pub llm_request_id: u64,
    pub llm_request_state: LlmRequestState,
    pub llm_response_buffer: String,
    pub pending_llm_request: Option<String>,
    pub pending_tool_calls: Vec<crate::llm::types::ToolCall>,
    pub post_exec_mode: bool,
    pub llm_abort_handle: Option<tokio::task::AbortHandle>,
    
    // Suggestion panel
    pub suggestion: Option<LlmSuggestion>,
    pub suggestion_visible: bool,
    
    // Chat history (for display only)
    pub chat_log: Vec<ChatMessage>,
    pub cached_chat_lines: Vec<Line<'static>>,
    
    // Notifications
    pub notifications: Vec<Notification>,
    
    // Event channel
    pub event_tx: mpsc::UnboundedSender<AppEvent>,
    pub event_rx: mpsc::UnboundedReceiver<AppEvent>,
    
    // Terminal size & layout
    pub terminal_size: (u16, u16),
    
    // Exploit approval prompt
    pub pending_exploit_approval: Option<ExploitApproval>,

    // Background workspace command execution (Tab 0)
    pub running_command: Option<(u64, String, Instant)>,
    pub running_command_output: Vec<String>,
    pub running_command_raw: String,
    pub command_stdin_tx: Option<tokio::sync::mpsc::UnboundedSender<String>>,
    pub command_abort_handle: Option<tokio::task::AbortHandle>,

    // Mouse mode
    pub mouse_capture_enabled: bool,

    // Timing
    pub last_tick: Instant,
}

impl App {
    pub async fn new() -> Result<Self> {
        // Load or create config
        let config = Config::load().context("Failed to load config")?;
        
        // Initialize LLM client for configured provider (OpenRouter, OpenAI, Ollama, Custom)
        let llm_client = LlmClient::from_config(&config).context("Failed to initialize LLM client")?;
        
        // Initialize with in-memory database and placeholder session until user selects or creates a session
        let db = Database::in_memory().context("Failed to initialize temporary database")?;
        let session_meta = SessionMeta::new("None".to_string());
        let session_list = SessionMeta::list_sessions();
        
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        
        // Spawn tick event generator
        let tx_tick = event_tx.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_millis(50));
            loop {
                interval.tick().await;
                if tx_tick.send(AppEvent::Tick).is_err() {
                    break;
                }
            }
        });
        
        // Spawn crossterm event reader
        let tx_keys = event_tx.clone();
        tokio::spawn(async move {
            let mut reader = crossterm::event::EventStream::new();
            use futures_util::StreamExt;
            while let Some(event) = reader.next().await {
                if let Ok(crossterm::event::Event::Key(key)) = event {
                    // Discard Release events to prevent duplicate inputs and flickering on Windows
                    if key.kind != crossterm::event::KeyEventKind::Release {
                        if tx_keys.send(AppEvent::Key(key)).is_err() {
                            break;
                        }
                    }
                } else if let Ok(crossterm::event::Event::Mouse(mouse)) = event {
                    if tx_keys.send(AppEvent::Mouse(mouse)).is_err() {
                        break;
                    }
                } else if let Ok(crossterm::event::Event::Resize(cols, rows)) = event {
                    if tx_keys.send(AppEvent::Resize(cols, rows)).is_err() {
                        break;
                    }
                }
            }
        });
        
        let prompt_api_key = config.provider.requires_api_key() && config.api_key.is_empty();

        let mut app = Self {
            config,
            db,
            llm_client,
            session_meta,
            ui_state: UiState {
                show_session_picker: true,
                is_initial_session_select: true,
                session_picker_selected: 0,
                session_list,
                session_naming_mode: false,
                session_name_input: String::new(),
                show_api_key_prompt: prompt_api_key,
                ..UiState::default()
            },
            input_mode: InputMode::Shell,
            input_buffer: String::new(),
            command_history: VecDeque::with_capacity(1000),
            history_index: None,
            sidebar_nodes: Vec::new(),
            active_target_id: None,
            tabs: Vec::new(), // Tab 0 is always Workspace & LLM; tabs holds Tab 1..N (Shell tabs)
            active_tab: 0,
            next_tab_id: 1,
            llm_request_id: 0,
            llm_request_state: LlmRequestState::Idle,
            llm_response_buffer: String::new(),
            pending_llm_request: None,
            pending_tool_calls: Vec::new(),
            post_exec_mode: false,
            llm_abort_handle: None,
            suggestion: None,
            suggestion_visible: false,
            chat_log: Vec::new(),
            cached_chat_lines: Vec::new(),
            notifications: Vec::new(),
            event_tx,
            event_rx,
            terminal_size: crossterm::terminal::size().unwrap_or((80, 24)),
            pending_exploit_approval: None,
            running_command: None,
            running_command_output: Vec::new(),
            running_command_raw: String::new(),
            command_stdin_tx: None,
            command_abort_handle: None,
            mouse_capture_enabled: false,
            last_tick: Instant::now(),
        };

        // Dynamic model discovery on startup (fetch live models from provider if cache is empty or stale)
        if app.config.is_cache_stale() || app.config.models_cache.is_empty() {
            app.trigger_fetch_models();
        }
        
        // Load sidebar
        app.reload_sidebar().await?;
        
        Ok(app)
    }
    
    pub fn next_request_id(&mut self) -> u64 {
        self.llm_request_id += 1;
        self.llm_request_id
    }

    pub fn cancel_llm_request(&mut self) {
        if let Some(abort) = self.llm_abort_handle.take() {
            abort.abort();
        }
        self.next_request_id();
        self.llm_request_state = LlmRequestState::Idle;
        self.pending_tool_calls.clear();
        self.llm_response_buffer.clear();
        self.append_chat_message(ChatMessage {
            role: "system".to_string(),
            content: "⚠️ LLM generation cancelled by user (Ctrl+C)".to_string(),
            tool_calls: None,
            tool_call_id: None,
        });
        self.ui_state.log_at_bottom = true;
        self.add_notification("LLM generation cancelled".to_string(), NotifLevel::Info);
    }

    pub fn save_chat_history(&self) {
        let _ = self.session_meta.save_chat_history(&self.chat_log);
    }

    pub fn rebuild_cached_chat_lines(&mut self) {
        self.cached_chat_lines.clear();
        for msg in &self.chat_log {
            let lines = crate::ui::layout::format_chat_message(msg, &self.config.active_model);
            self.cached_chat_lines.extend(lines);
        }
    }

    pub fn append_chat_message(&mut self, msg: ChatMessage) {
        let lines = crate::ui::layout::format_chat_message(&msg, &self.config.active_model);
        self.cached_chat_lines.extend(lines);
        self.chat_log.push(msg);
        self.ui_state.log_at_bottom = true;
        self.save_chat_history();
    }

    pub async fn delete_session(&mut self, session_name: &str) -> Result<()> {
        let is_current = self.session_meta.name == session_name;
        if is_current {
            // Drop connection to session.db before removing folder on Windows
            self.db = Database::in_memory()?;
            self.session_meta = SessionMeta::new("None".to_string());
            self.chat_log.clear();
            self.cached_chat_lines.clear();
            self.tabs.clear();
            self.sidebar_nodes.clear();
            self.active_target_id = None;
        }

        let dir = SessionMeta::sessions_dir().join(session_name);
        if dir.exists() {
            std::fs::remove_dir_all(&dir)?;
        }

        self.ui_state.session_list = SessionMeta::list_sessions();
        let total_rows = 1 + self.ui_state.session_list.len();
        if self.ui_state.session_picker_selected >= total_rows {
            self.ui_state.session_picker_selected = total_rows.saturating_sub(1);
        }
        self.add_notification(format!("Deleted session: {}", session_name), NotifLevel::Info);
        Ok(())
    }

    pub async fn switch_session(&mut self, session_name: String) -> Result<()> {
        // Save current session chat history before switching
        self.save_chat_history();

        let meta_path = SessionMeta::sessions_dir().join(&session_name).join("session.json");
        let session_meta = if meta_path.exists() {
            let mut meta = SessionMeta::load(&meta_path)?;
            meta.last_activity = Utc::now();
            let _ = meta.save();
            meta
        } else {
            let meta = SessionMeta::new(session_name);
            std::fs::create_dir_all(&meta.path())?;
            meta.save()?;
            meta
        };

        // Ensure session workspace subdirectories always exist
        let ws = session_meta.workspace_path();
        let _ = std::fs::create_dir_all(ws.join("outputs"));
        let _ = std::fs::create_dir_all(ws.join("evidence"));
        let _ = std::fs::create_dir_all(ws.join("artifacts"));
        let _ = std::fs::create_dir_all(ws.join("exploits"));

        self.db = Database::new(&session_meta.db_path())?;
        self.chat_log = session_meta.load_chat_history();
        self.rebuild_cached_chat_lines();
        self.session_meta = session_meta;
        self.tabs.clear();
        self.active_tab = 0;
        self.active_target_id = None;
        self.ui_state.log_at_bottom = true;
        self.reload_sidebar().await?;
        self.ui_state.session_list = SessionMeta::list_sessions();
        self.add_notification(format!("Active session: {}", self.session_meta.name), NotifLevel::Info);
        Ok(())
    }
}

impl Drop for App {
    fn drop(&mut self) {
        self.save_chat_history();
    }
}

impl SessionMeta {
    pub fn save(&self) -> Result<()> {
        let content = serde_json::to_string_pretty(self)?;
        std::fs::write(self.meta_path(), content)?;
        Ok(())
    }
    
    pub fn load(path: &PathBuf) -> Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let meta: SessionMeta = serde_json::from_str(&content)?;
        Ok(meta)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowFocus {
    ChatLog,
    Input,
    Sidebar,
}

impl Default for WindowFocus {
    fn default() -> Self {
        WindowFocus::Input
    }
}

#[derive(Debug, Default)]
pub struct UiState {
    pub should_quit: bool,
    pub focus: WindowFocus,
    pub show_help: bool,
    pub show_model_picker: bool,
    pub show_session_picker: bool,
    pub is_initial_session_select: bool,
    pub session_naming_mode: bool,
    pub session_name_input: String,
    pub session_delete_confirm: Option<String>,
    pub session_picker_selected: usize,
    pub session_list: Vec<SessionMeta>,
    pub show_api_key_prompt: bool,
    pub api_key_input: String,
    pub provider_picker_selected: usize,
    pub model_picker_search: String,
    pub model_picker_free_only: bool,
    pub model_picker_selected: usize,
    pub model_picker_provider: crate::config::ProviderKind,
    pub tick_count: usize,
    pub sidebar_collapsed: bool,
    pub sidebar_selected: usize,
    pub log_scroll: usize,
    pub log_at_bottom: bool,
    pub input_cursor: usize,
}