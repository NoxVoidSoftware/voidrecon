# voidrecon Architecture

## Module boundaries

See `AGENT.md`. Summary:

- `app/` owns `AppState`, `AppEvent`, main event loop, all state mutations.
- `config/` owns config.json, model cache, API keys.
- `db/` owns all SQL, schema, queries.
- `llm/` owns API calls, SSE parsing, context building, tool schemas. No DB access; receives context as structs.
- `tools/` owns tool execution (DB/filesystem/process) and registry.
- `pty/` owns PTY spawn, resize, vt100 parsing, widget.
- `ui/` owns rendering only; never mutates state.

## State flow

Spawned tasks communicate via `mpsc<AppEvent>`. All mutations happen in the main event loop.

## Adding an LLM provider

Providers are OpenAI-compatible and described by a `ProviderSpec` (`src/llm/providers/`).

1. Create `src/llm/providers/<name>.rs` with a unit struct implementing `ProviderSpec`
   (name, `:provider` aliases, default base URL; optionally API key policy, custom URL
   support, extra headers, chat-model filter).
2. Add one line to the `providers!` list in `src/llm/providers/mod.rs`.

The list generates `ProviderKind`, `ALL_PROVIDERS` and the kind → spec lookup, so the
setup modal, model picker, `:provider` command and HTTP client pick the provider up
without further changes. The variant name becomes the snake_case id stored in
`config.json`; do not rename existing variants or display names.
