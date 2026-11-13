# voidrecon

**An interactive terminal workspace for security assessments and LLM-assisted CLI workflows.**

voidrecon combines a terminal UI, local shell tabs, assessment-session data, and optional LLM integrations in one Rust application. Use it to keep targets, findings, command history, and evidence together while working in an authorized environment.

> **Early-stage project:** voidrecon is under active development. Features and interfaces may change.

## Features

- **Terminal UI** with a workspace, target sidebar, chat, and additional local shell tabs.
- **Assessment sessions** with separate SQLite databases, chat history, and workspace files.
- **Target and finding management**, including ports, metadata, notes, severity, and confidence.
- **LLM-assisted workflows** that can use assessment context and suggest commands.
- **Interactive approvals** for suggested command execution and sensitive actions such as saving files or starting listeners.
- **Multiple model providers:** OpenRouter, NVIDIA NIM, OpenAI, Ollama, and custom OpenAI-compatible endpoints.
- **Model discovery and selection** from the configured provider.

## Requirements

- Rust stable toolchain (install with [rustup](https://rustup.rs/)).
- A terminal that supports a TUI and alternate screen.
- An API key for hosted providers. Ollama can be used locally without an API key; custom endpoints may or may not require one.
- Any external CLI tools you intend to use in your shell or assessment workflows must be installed separately.

## Build and run

```sh
git clone https://github.com/ostapyrih/voidrecon.git
cd voidrecon
cargo run
```

To build an optimized binary:

```sh
cargo build --release
```

The binary will be available at `target/release/voidrecon` (or `voidrecon.exe` on Windows).

GitHub Releases are configured to publish builds for x86_64 Linux, aarch64 Linux, Intel and Apple Silicon macOS, and x86_64 Windows. Check the [Releases](https://github.com/ostapyrih/voidrecon/releases) page for available downloads.

## First launch

1. Start voidrecon with `cargo run` or launch the downloaded binary.
2. In the workspace, open provider setup with `:provider` and choose a provider.
3. Enter an API key if the provider requires one. For Ollama or a custom endpoint, configure the endpoint as needed in the provider setup.
4. Open the model picker with `F2` or `:models` and choose a model.
5. Type a shell command in shell mode, or switch to LLM query mode with `Ctrl+L`.

Provider configuration is saved to `~/.config/voidrecon/config.json`. Sessions and their data are stored under `~/.config/voidrecon/sessions/`.

## Keyboard shortcuts

Press `F1`, `Alt+H`, or `Ctrl+H` to open the in-app help.

| Shortcut | Action |
| --- | --- |
| `F1` / `Alt+H` / `Ctrl+H` | Toggle help |
| `F2` | Open model picker |
| `F3` | Open a new shell tab |
| `F4` / `Alt+M` | Toggle mouse mode |
| `Tab` | Complete commands and paths in the workspace prompt; shell tabs use native shell completion |
| `Shift+Tab` | Cycle backward through workspace panes and shell tabs |
| `Esc` | Return to the workspace or dismiss the active view |
| `Ctrl+L` | Enter LLM query mode |
| `Ctrl+C` | Abort a running command or LLM request |
| `Ctrl+Q` | Quit |
| `[` | Toggle the target sidebar |

Mouse mode is off by default so the terminal can handle native text selection and copy. Enable it with `F4` or `Alt+M` to use mouse scrolling and tab clicks.

## Workspace commands

Enter these in shell mode:

| Command | Action |
| --- | --- |
| `:targets` | List recorded targets |
| `:findings` | Show findings for the active target |
| `:findings N` | Show findings for target `N` |
| `:note <text>` | Add a note to the active target |
| `:models` | Open the model picker |
| `:provider` | Configure or switch the LLM provider |
| `:sessions` | Switch or create an assessment session |
| `:mouse [on\|off]` | Toggle mouse mode or set its state |
| `:newtab` | Open a shell tab |
| `:closeshell` | Close the current shell tab |
| `:exploit` | Enter exploit mode for the active target |
| `:quit` | Quit |

In LLM query mode, type your question and press `Enter`. You can also prefix a question with `?` in shell mode. When the assistant suggests a command, review it and choose **run**, **edit**, or **skip** in the UI.

## Data and privacy

- Configuration, including provider API keys, is stored locally in `~/.config/voidrecon/config.json`.
- Each session has its own `session.db`, `session.json`, chat history, and `workspace/` directory beneath `~/.config/voidrecon/sessions/<session-name>/`.
- When you use a hosted LLM provider, prompts and the assessment context included in them are sent to that provider. Review the provider's data policies before use.
- Commands run in your local environment. Review suggested commands and only use voidrecon on systems and networks you are authorized to assess.

## Development

Format the code, run tests, and build:

```sh
cargo fmt --check
cargo test
cargo build
```

Contributions, bug reports, and feature requests are welcome. For changes, please include a clear description and tests where practical.

## License

Licensed under either of:

- [Apache License, Version 2.0](https://www.apache.org/licenses/LICENSE-2.0)
- [MIT license](https://opensource.org/licenses/MIT)

at your option.
