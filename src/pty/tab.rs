//! PTY tab management for voidrecon

use anyhow::{Context, Result};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, SlavePty, native_pty_system};
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::sync::mpsc;
use vt100::Parser;

use crate::app::AppEvent;

/// Terminal tab kind
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalKind {
    LocalShell,
    Listener,
    Command,
}

impl TerminalKind {
    pub fn as_str(self) -> &'static str {
        match self {
            TerminalKind::LocalShell => "shell",
            TerminalKind::Listener => "listener",
            TerminalKind::Command => "command",
        }
    }
}

/// Bytes of raw command output kept per tab (the tail is kept when exceeded).
const TRANSCRIPT_LIMIT: usize = 256 * 1024;

/// Raw output of a tab: the newest bytes plus a running byte count, so
/// readers can ask for "everything after offset N".
#[derive(Debug, Default)]
pub struct Transcript {
    bytes: Vec<u8>,
    /// Bytes ever appended; the offset just past the newest byte.
    total: u64,
    last_output: Option<Instant>,
}

/// Output appended after a read offset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptSlice {
    /// New output with terminal escapes removed.
    pub text: String,
    /// Offset to pass next time to get only newer output.
    pub end: u64,
    /// New bytes that were already dropped from the kept tail.
    pub lost_bytes: u64,
}

impl Transcript {
    pub fn append(&mut self, data: &[u8]) {
        self.bytes.extend_from_slice(data);
        self.total += data.len() as u64;
        self.last_output = Some(Instant::now());
        if self.bytes.len() > TRANSCRIPT_LIMIT {
            let excess = self.bytes.len() - TRANSCRIPT_LIMIT;
            self.bytes.drain(..excess);
        }
    }

    pub fn total(&self) -> u64 {
        self.total
    }

    pub fn last_output(&self) -> Option<Instant> {
        self.last_output
    }

    /// Everything appended after `offset`.
    pub fn since(&self, offset: u64) -> TranscriptSlice {
        let kept_start = self.total - self.bytes.len() as u64;
        let start = offset.clamp(kept_start, self.total);
        let skip = usize::try_from(start - kept_start).unwrap_or(self.bytes.len());
        let raw = String::from_utf8_lossy(&self.bytes[skip.min(self.bytes.len())..]);
        TranscriptSlice {
            text: crate::ui::text::sanitize_terminal_output(&raw).trim().to_string(),
            end: self.total,
            lost_bytes: start - offset.min(start),
        }
    }
}

pub type SharedTranscript = Arc<Mutex<Transcript>>;

fn append_transcript(transcript: &Mutex<Transcript>, data: &[u8]) {
    if let Ok(mut transcript) = transcript.lock() {
        transcript.append(data);
    }
}

/// A PTY terminal tab
pub struct PtyTab {
    /// Internal key that routes PTY events; never reused.
    pub id: usize,
    /// Number shown in the tab bar and used by the LLM ("Tab 1"). The lowest
    /// free number is reused, so it stays stable while the tab is open.
    pub number: usize,
    /// Full raw output (screen text only holds what is visible), newest bytes kept.
    pub transcript: SharedTranscript,
    /// Transcript offset up to which the LLM has read this tab's output.
    pub llm_read_offset: u64,
    pub label: String,
    /// Full command line run in this tab (none for a plain shell).
    pub command: Option<String>,
    pub started: Instant,
    /// Process id of the tab's top process.
    pub pid: Option<u32>,
    pub kind: TerminalKind,
    pub exit_code: Option<i32>,
    pub master: Box<dyn MasterPty + Send>,
    pub _slave: Option<Box<dyn SlavePty + Send>>,
    pub child: Arc<Mutex<Box<dyn Child + Send + Sync>>>,
    pub parser: Arc<Mutex<Parser>>,
    pub writer: Box<dyn Write + Send>,
    pub _reader_thread: Option<std::thread::JoinHandle<()>>,
    pub _watcher_thread: Option<std::thread::JoinHandle<()>>,
}

impl PtyTab {
    fn build_shell_command() -> CommandBuilder {
        #[cfg(windows)]
        let shell = std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_string());
        #[cfg(not(windows))]
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "bash".to_string());

        let mut cmd = CommandBuilder::new(shell);
        // Inherit all environment variables so child processes (like cmd.exe) have access
        // to SystemRoot, PATH, etc. to prevent error 0xc0000142 on Windows
        for (k, v) in std::env::vars() {
            cmd.env(k, v);
        }
        cmd
    }

    /// Create a new local shell tab
    pub fn new_shell(
        id: usize,
        tx: mpsc::UnboundedSender<AppEvent>,
        size: (u16, u16),
    ) -> Result<Self> {
        let (cols, rows) = size;
        let pty_system = native_pty_system();
        let pty_pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("Failed to open PTY")?;

        let cmd = Self::build_shell_command();

        let child = pty_pair
            .slave
            .spawn_command(cmd)
            .context("Failed to spawn shell")?;

        let mut reader = pty_pair.master.try_clone_reader().context("Failed to clone reader")?;
        let writer = pty_pair.master.take_writer().context("Failed to take writer")?;

        let parser = Arc::new(Mutex::new(Parser::new(rows, cols, 0)));

        let parser_clone = parser.clone();
        let transcript: SharedTranscript = Arc::default();
        let transcript_clone = transcript.clone();
        let tx_clone = tx.clone();
        let tab_id = id;

        // Use dedicated thread for blocking read to avoid blocking tokio runtime
        let reader_thread = std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break, // EOF
                    Ok(n) => {
                        let data = &buf[..n];
                        // Feed to vt100 parser
                        if let Ok(mut p) = parser_clone.lock() {
                            p.process(data);
                        }
                        append_transcript(&transcript_clone, data);
                        // Send to app event loop
                        if tx_clone.send(AppEvent::PtyData(tab_id, data.to_vec())).is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        eprintln!("PTY read error: {}", e);
                        break;
                    }
                }
            }
        });

        let pid = child.process_id();
        let child_arc = Arc::new(Mutex::new(child));
        let child_clone = child_arc.clone();
        let tx_exit = tx.clone();

        // Watcher thread to promptly detect when process terminates (e.g. user typed 'exit')
        let watcher_thread = std::thread::spawn(move || {
            loop {
                std::thread::sleep(std::time::Duration::from_millis(100));
                if let Ok(mut c) = child_clone.lock() {
                    match c.try_wait() {
                        Ok(Some(status)) => {
                            let _ = tx_exit.send(AppEvent::PtyExited(tab_id, status.exit_code() as i32));
                            break;
                        }
                        Ok(None) => continue,
                        Err(_) => {
                            let _ = tx_exit.send(AppEvent::PtyExited(tab_id, 0));
                            break;
                        }
                    }
                } else {
                    break;
                }
            }
        });

        Ok(Self {
            id,
            label: format!("Shell {}", id),
            kind: TerminalKind::LocalShell,
            exit_code: None,
            number: id,
            transcript,
            llm_read_offset: 0,
            command: None,
            started: Instant::now(),
            pid,
            master: pty_pair.master,
            _slave: Some(pty_pair.slave),
            child: child_arc,
            parser,
            writer,
            _reader_thread: Some(reader_thread),
            _watcher_thread: Some(watcher_thread),
        })
    }

    /// Create a new reverse shell listener tab
    pub fn new_listener(
        id: usize,
        lport: u16,
        tx: mpsc::UnboundedSender<AppEvent>,
        size: (u16, u16),
    ) -> Result<Self> {
        let (cols, rows) = size;
        let pty_system = native_pty_system();
        let pty_pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("Failed to open PTY for listener")?;

        // Try nc, ncat, socat, python, powershell in order
        let mut builder: Option<CommandBuilder> = None;

        for listener in ["nc", "ncat", "socat"] {
            if which::which(listener).is_ok() {
                let mut b = CommandBuilder::new(listener);
                for (k, v) in std::env::vars() {
                    b.env(k, v);
                }
                match listener {
                    "nc" | "ncat" => {
                        b.arg("-lvnp");
                        b.arg(lport.to_string());
                    }
                    "socat" => {
                        b.arg(format!("TCP-LISTEN:{},reuseaddr,fork", lport));
                        b.arg("EXEC:bash,pty,stderr,setsid,sigint,sane");
                    }
                    _ => {}
                }
                builder = Some(b);
                break;
            }
        }

        if builder.is_none() {
            // Check for python3 or python
            let py = if which::which("python3").is_ok() {
                Some("python3")
            } else if which::which("python").is_ok() {
                Some("python")
            } else {
                None
            };

            if let Some(py_bin) = py {
                let script = format!(
                    "import socket, sys, threading\n\
                     s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)\n\
                     s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)\n\
                     s.bind(('0.0.0.0', {port}))\n\
                     s.listen(1)\n\
                     print('[*] Python Reverse Shell Listener active on 0.0.0.0:{port}', flush=True)\n\
                     print('[*] Waiting for incoming connection...', flush=True)\n\
                     conn, addr = s.accept()\n\
                     print(f'[+] Shell connection received from {{addr[0]}}:{{addr[1]}}!', flush=True)\n\
                     def rx():\n\
                         while True:\n\
                             try:\n\
                                 d = conn.recv(1024)\n\
                                 if not d: break\n\
                                 sys.stdout.buffer.write(d)\n\
                                 sys.stdout.buffer.flush()\n\
                             except: break\n\
                     threading.Thread(target=rx, daemon=True).start()\n\
                     while True:\n\
                         try:\n\
                             d = sys.stdin.buffer.read(1)\n\
                             if not d: break\n\
                             conn.sendall(d)\n\
                         except: break\n",
                    port = lport
                );
                let mut b = CommandBuilder::new(py_bin);
                for (k, v) in std::env::vars() {
                    b.env(k, v);
                }
                b.arg("-u");
                b.arg("-c");
                b.arg(script);
                builder = Some(b);
            }
        }

        if builder.is_none() {
            // Fallback to powershell / pwsh on Windows
            let ps = if which::which("pwsh").is_ok() {
                Some("pwsh")
            } else if which::which("powershell").is_ok() {
                Some("powershell")
            } else {
                None
            };

            if let Some(ps_bin) = ps {
                let ps_cmd = format!(
                    "$port = {port}; \
                     Write-Host '[*] Reverse shell listener active on 0.0.0.0:'$port; \
                     Write-Host '[*] Waiting for connection...'; \
                     $listener = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Any, $port); \
                     $listener.Start(); \
                     $client = $listener.AcceptTcpClient(); \
                     Write-Host ('[+] Shell connection received from ' + $client.Client.RemoteEndPoint); \
                     $stream = $client.GetStream(); \
                     $buf = New-Object byte[] 1024; \
                     [System.Threading.Tasks.Task]::Run({{ while(($r = $stream.Read($buf, 0, $buf.Length)) -gt 0){{ [Console]::OpenStandardOutput().Write($buf, 0, $r) }} }}); \
                     while($true){{ if([Console]::KeyAvailable){{ $k = [Console]::ReadKey($true); [byte[]]$b = [System.Text.Encoding]::ASCII.GetBytes($k.KeyChar); $stream.Write($b, 0, $b.Length) }} Start-Sleep -Milliseconds 20 }}",
                    port = lport
                );
                let mut b = CommandBuilder::new(ps_bin);
                for (k, v) in std::env::vars() {
                    b.env(k, v);
                }
                b.arg("-NoProfile");
                b.arg("-ExecutionPolicy");
                b.arg("Bypass");
                b.arg("-Command");
                b.arg(ps_cmd);
                builder = Some(b);
            }
        }

        let builder = builder.context("No listener executable found (nc, ncat, socat, python, powershell)")?;

        let child = pty_pair
            .slave
            .spawn_command(builder)
            .context("Failed to spawn listener")?;

        let mut reader = pty_pair.master.try_clone_reader().context("Failed to clone reader")?;
        let writer = pty_pair.master.take_writer().context("Failed to take writer")?;

        let parser = Arc::new(Mutex::new(Parser::new(rows, cols, 0)));

        let parser_clone = parser.clone();
        let transcript: SharedTranscript = Arc::default();
        let transcript_clone = transcript.clone();
        let tx_clone = tx.clone();
        let tab_id = id;

        let reader_thread = std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        let data = &buf[..n];
                        if let Ok(mut p) = parser_clone.lock() {
                            p.process(data);
                        }
                        append_transcript(&transcript_clone, data);
                        if tx_clone.send(AppEvent::PtyData(tab_id, data.to_vec())).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        let pid = child.process_id();
        let child_arc = Arc::new(Mutex::new(child));
        let child_clone = child_arc.clone();
        let tx_exit = tx.clone();
        let watcher_thread = std::thread::spawn(move || {
            loop {
                std::thread::sleep(std::time::Duration::from_millis(100));
                if let Ok(mut c) = child_clone.lock() {
                    match c.try_wait() {
                        Ok(Some(status)) => {
                            let _ = tx_exit.send(AppEvent::PtyExited(tab_id, status.exit_code() as i32));
                            break;
                        }
                        Ok(None) => continue,
                        Err(_) => {
                            let _ = tx_exit.send(AppEvent::PtyExited(tab_id, 0));
                            break;
                        }
                    }
                } else {
                    break;
                }
            }
        });

        Ok(Self {
            id,
            label: format!("Listener: :{}", lport),
            kind: TerminalKind::Listener,
            exit_code: None,
            number: id,
            transcript,
            llm_read_offset: 0,
            command: Some(format!("listener on port {}", lport)),
            started: Instant::now(),
            pid,
            master: pty_pair.master,
            _slave: Some(pty_pair.slave),
            child: child_arc,
            parser,
            writer,
            _reader_thread: Some(reader_thread),
            _watcher_thread: Some(watcher_thread),
        })
    }

    /// Run one approved command in its own PTY tab.
    pub fn new_command(
        id: usize,
        command: &str,
        workspace: &std::path::Path,
        tx: mpsc::UnboundedSender<AppEvent>,
        size: (u16, u16),
    ) -> Result<Self> {
        let (cols, rows) = size;
        let pty_system = native_pty_system();
        let pty_pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("Failed to open PTY for command")?;

        let mut cmd = Self::build_shell_command();
        #[cfg(windows)]
        cmd.args(["/C", command]);
        #[cfg(not(windows))]
        cmd.args(["-lc", command]);
        cmd.cwd(workspace);

        let child = pty_pair
            .slave
            .spawn_command(cmd)
            .context("Failed to spawn interactive command")?;
        let mut reader = pty_pair
            .master
            .try_clone_reader()
            .context("Failed to clone reader")?;
        let writer = pty_pair
            .master
            .take_writer()
            .context("Failed to take writer")?;
        let parser = Arc::new(Mutex::new(Parser::new(rows, cols, 0)));
        let parser_clone = parser.clone();
        let transcript: SharedTranscript = Arc::default();
        let transcript_clone = transcript.clone();
        let tx_clone = tx.clone();
        let tab_id = id;
        let reader_thread = std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if let Ok(mut parser) = parser_clone.lock() {
                            parser.process(&buf[..n]);
                        }
                        append_transcript(&transcript_clone, &buf[..n]);
                        if tx_clone
                            .send(AppEvent::PtyData(tab_id, buf[..n].to_vec()))
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err(error) => {
                        eprintln!("PTY read error: {}", error);
                        break;
                    }
                }
            }
        });

        let pid = child.process_id();
        let child_arc = Arc::new(Mutex::new(child));
        let child_clone = child_arc.clone();
        let tx_exit = tx;
        let watcher_thread = std::thread::spawn(move || loop {
            std::thread::sleep(std::time::Duration::from_millis(100));
            if let Ok(mut child) = child_clone.lock() {
                match child.try_wait() {
                    Ok(Some(status)) => {
                        let _ = tx_exit.send(AppEvent::PtyExited(
                            tab_id,
                            status.exit_code() as i32,
                        ));
                        break;
                    }
                    Ok(None) => continue,
                    Err(_) => {
                        let _ = tx_exit.send(AppEvent::PtyExited(tab_id, 0));
                        break;
                    }
                }
            } else {
                break;
            }
        });

        let label = command
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(36)
            .collect::<String>();
        Ok(Self {
            id,
            label: format!("Command: {}", label),
            kind: TerminalKind::Command,
            exit_code: None,
            number: id,
            transcript,
            llm_read_offset: 0,
            command: Some(command.to_string()),
            started: Instant::now(),
            pid,
            master: pty_pair.master,
            _slave: Some(pty_pair.slave),
            child: child_arc,
            parser,
            writer,
            _reader_thread: Some(reader_thread),
            _watcher_thread: Some(watcher_thread),
        })
    }

    /// Write data to the PTY
    pub fn write(&mut self, data: &[u8]) -> Result<()> {
        self.writer.write_all(data).context("Failed to write to PTY")?;
        self.writer.flush().context("Failed to flush PTY")?;
        Ok(())
    }

    /// Human-readable process state: `running` or `exited (<code>)`.
    pub fn state_label(&self) -> String {
        self.exit_code
            .map_or_else(|| "running".to_string(), |code| format!("exited ({code})"))
    }

    /// Full command output captured so far, with terminal escapes removed.
    pub fn transcript_text(transcript: &Mutex<Transcript>) -> String {
        transcript
            .lock()
            .map(|transcript| transcript.since(0).text)
            .unwrap_or_default()
    }

    /// Output the LLM has not read yet and when the tab last printed anything.
    pub fn unread_output(&self) -> (TranscriptSlice, Option<Instant>) {
        self.transcript
            .lock()
            .map(|transcript| (transcript.since(self.llm_read_offset), transcript.last_output()))
            .unwrap_or_else(|_| {
                let empty = TranscriptSlice { text: String::new(), end: self.llm_read_offset, lost_bytes: 0 };
                (empty, None)
            })
    }

    /// Marks everything printed so far as read by the LLM.
    pub fn mark_read(&mut self) {
        if let Ok(transcript) = self.transcript.lock() {
            self.llm_read_offset = transcript.total();
        }
    }

    /// When the tab last printed anything.
    pub fn last_output(&self) -> Option<Instant> {
        self.transcript.lock().ok().and_then(|transcript| transcript.last_output())
    }

    /// Bytes printed since the LLM last read this tab.
    pub fn unread_bytes(&self) -> u64 {
        self.transcript
            .lock()
            .map(|transcript| transcript.total().saturating_sub(self.llm_read_offset))
            .unwrap_or(0)
    }

    /// Read the currently visible terminal screen as plain text.
    pub fn screen_text(&self) -> String {
        let Ok(parser) = self.parser.lock() else {
            return String::new();
        };
        parser.screen().contents().trim().to_string()
    }

    /// Resize the PTY and vt100 parser
    pub fn resize(&mut self, cols: u16, rows: u16) -> Result<()> {
        let _ = self.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        });
        if let Ok(mut parser) = self.parser.lock() {
            parser.set_size(rows, cols);
        }
        Ok(())
    }
}

impl Drop for PtyTab {
    fn drop(&mut self) {
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
        }
        // Close this handle to the slave side before joining the reader thread.
        // Otherwise the parent keeps the PTY open and the master read never
        // returns EOF/EIO, so `join()` below would block forever.
        self._slave.take();
        if let Some(handle) = self._watcher_thread.take() {
            let _ = handle.join();
        }
        if let Some(handle) = self._reader_thread.take() {
            let _ = handle.join();
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transcript_returns_only_output_after_offset() {
        let mut transcript = Transcript::default();
        transcript.append(b"first line\n");
        let first = transcript.since(0);
        assert_eq!(first.text, "first line");

        assert_eq!(transcript.since(first.end).text, "", "nothing new yet");
        transcript.append(b"\x1b[32mconnect from 10.10.14.2\x1b[0m\n");
        let second = transcript.since(first.end);
        assert_eq!(second.text, "connect from 10.10.14.2");
        assert_eq!(second.lost_bytes, 0);
    }

    #[test]
    fn transcript_reports_bytes_dropped_from_the_tail() {
        let mut transcript = Transcript::default();
        transcript.append(&vec![b'a'; TRANSCRIPT_LIMIT]);
        transcript.append(b"tail");
        let slice = transcript.since(0);
        assert_eq!(slice.lost_bytes, 4);
        assert!(slice.text.ends_with("tail"));
        assert_eq!(slice.end, TRANSCRIPT_LIMIT as u64 + 4);
    }
}
