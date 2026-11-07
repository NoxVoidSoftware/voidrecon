//! Application state for voidrecon

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
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
    pub expanded: bool,
    pub findings_by_severity: Vec<(String, Vec<Finding>)>,
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

    pub fn path(&self) -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".audit-tui")
            .join("sessions")
            .join(&self.name)
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
    pub sidebar_scroll: usize,
    
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
    pub awaiting_tool_results: bool,
    pub post_exec_mode: bool,
    
    // Suggestion panel
    pub suggestion: Option<LlmSuggestion>,
    pub suggestion_visible: bool,
    
    // Chat history (for display only)
    pub chat_log: Vec<ChatMessage>,
    
    // Notifications
    pub notifications: Vec<Notification>,
    
    // Event channel
    pub event_tx: mpsc::UnboundedSender<AppEvent>,
    pub event_rx: mpsc::UnboundedReceiver<AppEvent>,
    
    // Timing
    pub last_tick: Instant,
}

impl App {
    pub async fn new() -> Result<Self> {
        // Load or create config
        let config = Config::load().context("Failed to load config")?;
        
        // If no API key, we'll prompt for it in the UI
        let llm_client = LlmClient::new(config.api_key.clone());
        
        // For now create a default session
        let session_name = format!("{}_scan", chrono::Local::now().format("%Y-%m-%d"));
        let session_meta = SessionMeta::new(session_name);
        
        // Create session directory and workspace
        std::fs::create_dir_all(&session_meta.path())?;
        std::fs::create_dir_all(session_meta.workspace_path().join("outputs"))?;
        std::fs::create_dir_all(session_meta.workspace_path().join("evidence"))?;
        std::fs::create_dir_all(session_meta.workspace_path().join("artifacts"))?;
        
        // Save session metadata
        session_meta.save()?;
        
        let db = Database::new(&session_meta.db_path()).context("Failed to open database")?;
        
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
                    // Only forward Press events to prevent duplicate inputs and flickering on Windows
                    if key.kind == crossterm::event::KeyEventKind::Press {
                        if tx_keys.send(AppEvent::Key(key)).is_err() {
                            break;
                        }
                    }
                } else if let Ok(crossterm::event::Event::Resize(cols, rows)) = event {
                    if tx_keys.send(AppEvent::Resize(cols, rows)).is_err() {
                        break;
                    }
                }
            }
        });
        
        let prompt_api_key = config.api_key.is_empty();

        let mut app = Self {
            config,
            db,
            llm_client,
            session_meta,
            ui_state: UiState {
                show_api_key_prompt: prompt_api_key,
                ..UiState::default()
            },
            input_mode: InputMode::Shell,
            input_buffer: String::new(),
            command_history: VecDeque::with_capacity(1000),
            history_index: None,
            sidebar_nodes: Vec::new(),
            active_target_id: None,
            sidebar_scroll: 0,
            tabs: Vec::new(), // Tab 0 is always Workspace & LLM; tabs holds Tab 1..N (Shell tabs)
            active_tab: 0,
            next_tab_id: 1,
            llm_request_id: 0,
            llm_request_state: LlmRequestState::Idle,
            llm_response_buffer: String::new(),
            pending_llm_request: None,
            pending_tool_calls: Vec::new(),
            awaiting_tool_results: false,
            post_exec_mode: false,
            suggestion: None,
            suggestion_visible: false,
            chat_log: Vec::new(),
            notifications: Vec::new(),
            event_tx,
            event_rx,
            last_tick: Instant::now(),
        };

        // Dynamic model discovery on startup (fetch OpenRouter models if cache is older than 24 hours)
        if app.config.is_cache_stale() {
            let tx = app.event_tx.clone();
            tokio::spawn(async move {
                let client = reqwest::Client::builder()
                    .timeout(std::time::Duration::from_secs(10))
                    .build();
                if let Ok(client) = client {
                    if let Ok(res) = client.get("https://openrouter.ai/api/v1/models").send().await {
                        if let Ok(list) = res.json::<crate::llm::types::ModelListResponse>().await {
                            let _ = tx.send(AppEvent::ModelsLoaded(list.data));
                        }
                    }
                }
            });
        }
        
        // Load sidebar
        app.reload_sidebar().await?;
        
        Ok(app)
    }
    
    pub fn next_request_id(&mut self) -> u64 {
        self.llm_request_id += 1;
        self.llm_request_id
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

#[derive(Debug, Default)]
pub struct UiState {
    pub should_quit: bool,
    pub show_help: bool,
    pub show_model_picker: bool,
    pub show_api_key_prompt: bool,
    pub api_key_input: String,
    pub model_picker_search: String,
    pub model_picker_free_only: bool,
    pub model_picker_selected: usize,
    pub sidebar_collapsed: bool,
    pub sidebar_selected: usize,
    pub log_scroll: usize,
    pub log_at_bottom: bool,
    pub chat_scroll: usize,
    pub chat_at_bottom: bool,
    pub input_cursor: usize,
}