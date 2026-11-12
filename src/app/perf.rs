//! Lightweight performance instrumentation for finding event-loop stalls.
//!
//! Enabled when the environment variable `VOIDRECON_PERF_LOG` points at a file
//! (e.g. `/tmp/voidrecon_perf.log`). Every recorded sample is a single line:
//! `<unix_ms> <label> <duration_micros>`
//!
//! The logger never blocks the event loop for long: it appends with
//! `std::fs::OpenOptions` + `write_all` and is only used on the slow path
//! (durations above the threshold) or when explicitly forced.

use std::io::Write;
use std::sync::OnceLock;
use std::time::Instant;

const DEFAULT_LOG_PATH: &str = "/tmp/voidrecon_perf.log";
const SLOW_THRESHOLD_MICROS: u128 = 8_000; // 8 ms

fn log_path() -> &'static str {
    static PATH: OnceLock<String> = OnceLock::new();
    PATH.get_or_init(|| {
        std::env::var("VOIDRECON_PERF_LOG").unwrap_or_else(|_| DEFAULT_LOG_PATH.to_string())
    })
}

fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("VOIDRECON_PERF").is_some())
}

/// Returns a guard that logs the elapsed time when dropped (if above threshold).
pub struct Timer {
    label: &'static str,
    start: Instant,
    force: bool,
}

impl Timer {
    pub fn new(label: &'static str) -> Self {
        Self {
            label,
            start: Instant::now(),
            force: false,
        }
    }

    pub fn forced(label: &'static str) -> Self {
        Self {
            label,
            start: Instant::now(),
            force: true,
        }
    }

    pub fn elapsed_micros(&self) -> u128 {
        self.start.elapsed().as_micros()
    }
}

impl Drop for Timer {
    fn drop(&mut self) {
        let micros = self.elapsed_micros();
        if !enabled() {
            return;
        }
        if !self.force && micros < SLOW_THRESHOLD_MICROS {
            return;
        }
        let ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_path())
        {
            let _ = writeln!(file, "{} {} {}", ms, self.label, micros);
        }
    }
}

/// Log an explicit event (e.g. "spawn_blocking_start" / "spawn_blocking_done").
pub fn mark(label: &str) {
    if !enabled() {
        return;
    }
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path())
    {
        let _ = writeln!(file, "{} MARK {}", ms, label);
    }
}
