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
