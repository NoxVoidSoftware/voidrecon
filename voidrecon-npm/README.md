# voidrecon

Production-grade interactive TUI terminal for security auditing, CLI workflows, and LLM-assisted terminal operations.

## Installation

```bash
# Run directly (no install needed)
npx voidrecon@latest

# Or install globally
npm i -g voidrecon
voidrecon
```

## What it does

`voidrecon` is a native Rust terminal application with:

- **Interactive TUI** — Built with ratatui/crossterm, runs in any terminal
- **Security auditing** — Port scanning, service enumeration, vulnerability checks
- **LLM-assisted workflows** — Natural language → commands, context-aware suggestions
- **Session management** — Persistent sessions, history, bookmarks
- **PTY support** — Full terminal emulation for nested commands

## How it works

This npm package is a lightweight wrapper that:

1. Detects your platform (Linux x64/ARM64, macOS x64/ARM64, Windows x64)
2. Downloads the prebuilt binary from [GitHub Releases](https://github.com/NoxVoidSoftware/voidrecon/releases)
3. Caches it in `~/.cache/voidrecon/<version>/`
4. Executes the native binary with full stdio forwarding

## Requirements

- Node.js ≥ 18 (for native `fetch` and `tar`/`unzipper`)

## Binaries

Prebuilt for:
- Linux x86_64 / ARM64
- macOS x86_64 / ARM64 (Apple Silicon)
- Windows x86_64

## Source

Main repository: https://github.com/NoxVoidSoftware/voidrecon

## License

MIT OR Apache-2.0