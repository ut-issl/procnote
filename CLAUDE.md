# Instructions for Claude

- This project is a procedure execution tool called `procnote`.
- This project is not yet published. So backward compatibility is not a concern. Make the simplest possible implementation.
- `procnote` uses filesystem-based storage with append-only JSONL event logs.
- Use filesystem and avoid in-memory caches as much as possible, to ensure crash safety, git-friendliness, and avoid cache invalidation issues.
- Don't rely on regex for determining something. Implement proper data and logic structures instead.
  - Using regex is a code smell in this project where backward compatibility is not a concern.

## Architecture

- Event-sourced Tauri 2 desktop app: Rust backend + Svelte 5 frontend.
- Three layers with strict dependency direction:
  1. **`crates/procnote-core/`** — Pure Rust domain logic. No Tauri dependency. Contains event types, execution state machine, and template parser.
  2. **`src-tauri/`** — Tauri shell. Bridges core to desktop via IPC commands. Owns serialization DTOs (`ExecutionSummary`, `StepSummary`, etc.) and filesystem I/O.
  3. **`src/`** — SvelteKit + Svelte 5 frontend. Uses runes (`$state`, `$derived`, `$props`), not stores.

### Core domain (`procnote-core`)

- `Event` enum (internally tagged `"type"` for JSON) — 13 variants covering execution lifecycle, step transitions, data capture, and revert markers.
- `ExecutionState::apply()` is the single source of truth for state mutations.
- `ExecutionState::from_events()` reconstructs state by replaying the event log in order; `LogMeta` is ignored by the state machine.
- User-visible reversals are modeled as typed domain events (`InputCleared`, `StepUnskipped`, `ExecutionReopened`, etc.), not generic event-log reverts.
- `ExecutionState::apply()` exhaustively handles every event variant, so new event types must define their state-machine behavior explicitly.
- Procedure templates are Markdown files with YAML frontmatter, parsed by `template/parser.rs` using `pulldown-cmark`.

### Tauri shell (`src-tauri`)

- `AppState` holds a single `procedures_dir` path. Each procedure is a subdirectory containing `template.md` and `.executions/`.
- `summarize()` converts `ExecutionState` + raw events into `ExecutionSummary` DTO for the frontend, building timestamp maps from the replayed event stream.
- Every `record_action` call re-reads and replays the full event log from disk (no in-memory cache, by design).
- Attachments are stored as `attachments/{sha256_7}-{filename}` inside the execution directory.

### Frontend (`src/`)

- `executionStore` (in `lib/stores/execution.svelte.ts`) is a Svelte 5 runes-based reactive store wrapping `ExecutionSummary`.
- TypeScript types in `lib/types/generated/` are auto-generated from Rust via `ts-rs`. Regenerate with `cargo test --workspace export_bindings_`. CI enforces they stay in sync.
- `chrono` is a dependency of `procnote-core` but NOT of `procnote-tauri`; timestamps cross the IPC boundary as ISO 8601 strings.

### Storage layout

```text
procedures/
├── <procedure-name>/
│   ├── template.md         # Procedure template (Markdown + YAML frontmatter)
│   └── .executions/
│       └── {YYYYMMDD}T{HHMMSS}-{uuid_8}/
│           ├── events.jsonl        # Append-only event log
│           ├── template.md         # Snapshot of procedure template at execution start
│           └── attachments/
│               └── {sha256_7}-{filename}
```

### Event log schema evolution rules

The `events.jsonl` format must remain backward-compatible within a major version. See `EVENT_LOG_COMPAT.md` for the full design.

- **Never rename or remove fields** on existing events.
- **Never change field types.**
- **Never rename event `"type"` values.**
- **All new fields must be `Option<T>` + `#[serde(default)]`.**
- Adding a new event variant or new optional field bumps the **minor** version.
- Violating any rule above requires a **major** version bump.
- Forward compatibility is not supported — older app versions reject logs from newer versions.
- Log files are **never modified** after creation; the code adapts, not the data.

## Development

- `just dev` — runs the Tauri dev server (passes the workspace directory explicitly).
- `cargo test --workspace` — runs all Rust tests (46+ tests across core).
- `npx svelte-check` — TypeScript type checking for frontend.
- `biome` — frontend linting/formatting.
- Pre-commit hooks are configured.
- **Package manager:** `pnpm` (not `npm`). Use `pnpm add` to install dependencies, `pnpm remove` to uninstall.

### Logging

- Tauri log plugin outputs to stdout, the log directory, and the webview console.
- **Log file location:** `~/Library/Logs/com.github.shunichironomura.procnote/procnote.log`
- In debug mode, the log level is `Debug`. Use `log::info!`, `log::debug!`, `log::warn!`, etc. in Rust backend code.
- Read logs with `tail -f ~/Library/Logs/com.github.shunichironomura.procnote/procnote.log`.

## Misc

- The discussions logs can be found in the `.local` directory. Note that some of the documents are ideas that have been discarded later.
