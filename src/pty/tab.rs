//! PTY tab management for voidrecon

use anyhow::{Context, Result};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, SlavePty, native_pty_system};
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
}

/// A PTY terminal tab
pub struct PtyTab {
    pub id: usize,
    pub label: String,
    pub kind: TerminalKind,
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

        let cmd = Self::build_shell_command();

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
            // Send exit event
            let _ = tx_clone.send(AppEvent::PtyExited(tab_id, 0));
        });

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

        let parser = Arc::new(Mutex::new(Parser::new(24, 80, 0)));

        let parser_clone = parser.clone();
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
                        if tx_clone.send(AppEvent::PtyData(tab_id, data.to_vec())).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            let _ = tx_clone.send(AppEvent::PtyExited(tab_id, 0));
        });

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
        if let Some(handle) = self._watcher_thread.take() {
            let _ = handle.join();
        }
        if let Some(handle) = self._reader_thread.take() {
            let _ = handle.join();
        }
    }
}