//! PTY tab management for voidrecon

use anyhow::{Context, Result};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use ratatui::style::Style;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use vt100::Parser;

use crate::app::AppEvent;

/// Terminal tab kind
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalKind {
    LocalShell,
    Listener,
    RemoteShell,
}

/// Process state for a terminal tab
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessState {
    Running,
    Completed,
    Failed,
    Cancelled,
    TimedOut,
}

/// A PTY terminal tab
pub struct PtyTab {
    pub id: usize,
    pub label: String,
    pub kind: TerminalKind,
    pub parser: Arc<Mutex<Parser>>,
    pub writer: Box<dyn Write + Send>,
    pub reader_thread: Option<tokio::task::JoinHandle<()>>,
    pub process_state: ProcessState,
    pub exit_code: Option<i32>,
    pub target_id: Option<i64>,
    pub lport: Option<u16>,
    pub shell_type: Option<String>,
}

impl PtyTab {
    /// Create a new local shell tab
    pub fn new_shell(id: usize, tx: mpsc::UnboundedSender<AppEvent>) -> Result<Self> {
        let pty_system = native_pty_system();
        let pty_pair = pty_system
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("Failed to open PTY")?;

        let cmd = CommandBuilder::new("cmd");
        #[cfg(unix)]
        {
            cmd = CommandBuilder::new(std::env::var("SHELL").unwrap_or_else(|_| "bash".to_string()));
        }

        let child = pty_pair
            .slave
            .spawn_command(cmd)
            .context("Failed to spawn shell")?;

        let mut reader = pty_pair.master.try_clone_reader().context("Failed to clone reader")?;
        let writer = pty_pair.master.take_writer().context("Failed to take writer")?;

        let parser = Arc::new(Mutex::new(Parser::new(24, 80, 0)));

        let parser_clone = parser.clone();
        let tx_clone = tx.clone();
        let tab_id = id;

        let reader_thread = tokio::spawn(async move {
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break, // EOF
                    Ok(n) => {
                        let data = &buf[..n];
                        // Feed to vt100 parser
                        if let Ok(mut parser) = parser_clone.lock() {
                            parser.process(data);
                        }
                        // Send to app event loop
                        let _ = tx_clone.send(AppEvent::PtyData(tab_id, data.to_vec()));
                    }
                    Err(e) => {
                        eprintln!("PTY read error: {}", e);
                        break;
                    }
                }
            }
            // Send exit event
            let _ = tx_clone.send(AppEvent::PtyExited(tab_id, 0));
        });

        // Drop slave to allow EOF detection
        drop(pty_pair.slave);
        drop(child);

        Ok(Self {
            id,
            label: format!("Shell {}", id + 1),
            kind: TerminalKind::LocalShell,
            parser,
            writer,
            reader_thread: Some(reader_thread),
            process_state: ProcessState::Running,
            exit_code: None,
            target_id: None,
            lport: None,
            shell_type: None,
        })
    }

    /// Create a new reverse shell listener tab
    pub fn new_listener(
        id: usize,
        lport: u16,
        tx: mpsc::UnboundedSender<AppEvent>,
    ) -> Result<Self> {
        let pty_system = native_pty_system();
        let pty_pair = pty_system
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("Failed to open PTY for listener")?;

        // Try nc, ncat, socat in order
        let listeners = ["nc", "ncat", "socat"];
        let mut cmd = None;
        for listener in listeners {
            if which::which(listener).is_ok() {
                cmd = Some(listener);
                break;
            }
        }
        let listener = cmd.context("No netcat listener found (nc, ncat, socat)")?;

        let mut builder = CommandBuilder::new(listener);
        match listener {
            "nc" | "ncat" => {
                builder.arg("-lvnp");
                builder.arg(lport.to_string());
            }
            "socat" => {
                builder.arg(format!("TCP-LISTEN:{},reuseaddr,fork", lport));
                builder.arg("EXEC:bash,pty,stderr,setsid,sigint,sane");
            }
            _ => {}
        }

        let child = pty_pair
            .slave
            .spawn_command(builder)
            .context("Failed to spawn listener")?;

        let mut reader = pty_pair.master.try_clone_reader().context("Failed to clone reader")?;
        let writer = pty_pair.master.take_writer().context("Failed to take writer")?;

        let parser = Arc::new(Mutex::new(Parser::new(24, 80, 0)));

        let parser_clone = parser.clone();
        let tx_clone = tx.clone();
        let tab_id = id;

        let reader_thread = tokio::spawn(async move {
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        let data = &buf[..n];
                        if let Ok(mut parser) = parser_clone.lock() {
                            parser.process(data);
                        }
                        let _ = tx_clone.send(AppEvent::PtyData(tab_id, data.to_vec()));
                    }
                    Err(_) => break,
                }
            }
            let _ = tx_clone.send(AppEvent::PtyExited(tab_id, 0));
        });

        drop(pty_pair.slave);
        drop(child);

        Ok(Self {
            id,
            label: format!("Listener: :{}", lport),
            kind: TerminalKind::Listener,
            parser,
            writer,
            reader_thread: Some(reader_thread),
            process_state: ProcessState::Running,
            exit_code: None,
            target_id: None,
            lport: Some(lport),
            shell_type: None,
        })
    }

    /// Create a new remote shell tab (for established connections)
    pub fn new_remote_shell(
        id: usize,
        label: String,
        target_id: i64,
        tx: mpsc::UnboundedSender<AppEvent>,
    ) -> Result<Self> {
        let pty_system = native_pty_system();
        let pty_pair = pty_system
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("Failed to open PTY for remote shell")?;

        let cmd = CommandBuilder::new("cmd");
        #[cfg(unix)]
        {
            cmd = CommandBuilder::new(std::env::var("SHELL").unwrap_or_else(|_| "bash".to_string()));
        }

        let child = pty_pair
            .slave
            .spawn_command(cmd)
            .context("Failed to spawn remote shell")?;

        let mut reader = pty_pair.master.try_clone_reader().context("Failed to clone reader")?;
        let writer = pty_pair.master.take_writer().context("Failed to take writer")?;

        let parser = Arc::new(Mutex::new(Parser::new(24, 80, 0)));

        let parser_clone = parser.clone();
        let tx_clone = tx.clone();
        let tab_id = id;

        let reader_thread = tokio::spawn(async move {
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        let data = &buf[..n];
                        if let Ok(mut parser) = parser_clone.lock() {
                            parser.process(data);
                        }
                        let _ = tx_clone.send(AppEvent::PtyData(tab_id, data.to_vec()));
                    }
                    Err(_) => break,
                }
            }
            let _ = tx_clone.send(AppEvent::PtyExited(tab_id, 0));
        });

        drop(pty_pair.slave);
        drop(child);

        Ok(Self {
            id,
            label,
            kind: TerminalKind::RemoteShell,
            parser,
            writer,
            reader_thread: Some(reader_thread),
            process_state: ProcessState::Running,
            exit_code: None,
            target_id: Some(target_id),
            lport: None,
            shell_type: Some("bash".to_string()),
        })
    }

    /// Write data to the PTY
    pub fn write(&mut self, data: &[u8]) -> Result<()> {
        self.writer.write_all(data).context("Failed to write to PTY")?;
        self.writer.flush().context("Failed to flush PTY")?;
        Ok(())
    }

    /// Resize the PTY and vt100 parser
    pub fn resize(&mut self, cols: u16, rows: u16) -> Result<()> {
        if let Ok(mut parser) = self.parser.lock() {
            parser.set_size(rows, cols);
        }
        Ok(())
    }

    /// Get the current process state
    pub fn process_state(&self) -> ProcessState {
        self.process_state
    }

    /// Mark the process as exited
    pub fn mark_exited(&mut self, exit_code: i32) {
        self.process_state = ProcessState::Completed;
        self.exit_code = Some(exit_code);
    }

    /// Get screen content for rendering - uses the parser's screen
    pub fn get_screen_content(&self) -> Vec<(u16, u16, char, Style)> {
        let mut result = Vec::new();
        if let Ok(parser) = self.parser.lock() {
            let screen = parser.screen();
            let contents = screen.contents();
            for (y, line) in contents.lines().enumerate() {
                for (x, ch) in line.chars().enumerate() {
                    result.push((x as u16, y as u16, ch, Style::default()));
                }
            }
        }
        result
    }
}

impl Drop for PtyTab {
    fn drop(&mut self) {
        if let Some(handle) = self.reader_thread.take() {
            handle.abort();
        }
    }
}