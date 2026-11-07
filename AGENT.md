# AGENT.md — voidrecon Development Rules

You are a Rust developer building **voidrecon** for **NoxVoid Software**.

---

## Mindset

You are building a tool that will be used in real penetration testing sessions.
Crashes, freezes, and corrupted terminal state are not acceptable.
When in doubt — be defensive, not clever.

---

## Code rules

**Errors**
- Never use `.unwrap()` or `.expect()` outside of `main.rs` startup or tests
- All fallible operations return `anyhow::Result`
- Errors that occur at runtime (network, DB, PTY) are converted to `AppEvent::Notification` and shown as banners — they never crash the app

**Async**
- Never block the tokio runtime — no `std::thread::sleep`, no blocking I/O on async tasks
- PTY reads run in dedicated `tokio::task::spawn_blocking` tasks
- LLM SSE stream is consumed in a spawned async task; each chunk fires `AppEvent::LlmChunk`

**Terminal safety**
- `TermGuard` must be the first thing initialized in `main` — its `Drop` calls `disable_raw_mode()` and `LeaveAlternateScreen` unconditionally
- A `std::panic::set_hook` must call the same cleanup before printing the panic message
- Never leave the terminal in raw mode under any circumstance

**State**
- All state lives in `AppState` — no global statics, no `Arc<Mutex<T>>` scattered around
- All mutations to `AppState` happen in the main event loop, never from spawned tasks directly
- Spawned tasks communicate only through the `mpsc` channel via `AppEvent` variants

**Database**
- All DB access goes through `db/` module functions — no inline SQL elsewhere
- Every `INSERT` to `targets` or `findings` fires `AppEvent::SidebarReload` after the write
- Truncate at write time: `commands.output` → first 250 + last 250 chars; `findings.description` → 200 chars

**LLM context**
- Context is stateless — rebuilt from DB on every request
- `chat_history` is append-only log, never read back into prompts
- Context is always scoped to `active_target_id` — never mix data from different targets

---

## Module boundaries

Each module owns its concern completely — do not reach across:

| Module | Owns | Never does |
| --- | --- | --- |
| `db/` | All SQL, schema init, query functions | Knows nothing about UI or LLM |
| `llm/` | API calls, SSE parsing, context build, tool schemas | Never touches DB directly — receives context as a struct |
| `pty/` | PTY spawn, resize, vt100 parsing, ratatui widget | Never touches DB or LLM |
| `ui/` | All ratatui rendering, layout, widget render functions | Never mutates AppState directly |
| `app/` | AppState, AppEvent, main event loop, all state mutations | Coordinates modules, owns no business logic |
| `config/` | config.json read/write, model cache | No UI, no DB |

---

## UI rules

- Every panel that shows growing content must have `scroll_offset` + `at_bottom` tracking
- Auto-scroll to bottom only when `at_bottom == true`; preserve position when user has scrolled up
- The sidebar reloads from DB on every `AppEvent::SidebarReload` — never use stale in-memory state
- Input mode (`Shell` / `LlmQuery` / `EditCommand`) is always visible in the footer
- Notifications auto-dismiss after 5 seconds; max 3 visible; oldest dropped on overflow

---

## Git discipline

- One commit per completed module — not per file, not per feature
- Commit message format: `module: what and why` (e.g. `db: add schema init and target query functions`)
- Never commit with `cargo build` warnings present
- Never commit with `todo!()` or `unimplemented!()` macros left in production paths

---

## What done means

A module is done when:
1. `cargo build --release` passes with zero warnings
2. Its public functions are tested with at least one integration test
3. It is wired into the event loop and manually verified to work end-to-end