# Instructions for Claude

- This project is a procedure execution tool called `procnote`.
- This project is not yet published. So backward compatibility is not a concern. Make the simplest possible implementation.
- `procnote` uses filesystem-based storage with append-only JSONL event logs.
- Use filesystem and avoid in-memory caches as much as possible, to ensure crash safety, git-friendliness, and avoid cache invalidation issues.
- Don't rely on regex for determining something. Implement proper data and logic structures instead.
  - Using regex is a code smell in this project where backward compatibility is not a concern.

## Architecture

- Event-sourced Tauri 2 desktop app (also usable from a browser via `procnote-server`): Rust backend + React frontend.
- Four layers with strict dependency direction:
  1. **`crates/procnote-core/`** — Pure Rust domain logic. No Tauri dependency. Contains event types, execution state machine, and template parser.
  2. **`crates/procnote-app/`** — Tauri-free application layer shared by the desktop shell and the future server. Owns serialization DTOs (`ExecutionSummary`, `StepSummary`, etc.), `ExecutionAction` → event mapping, and filesystem I/O (`persistence/`). Must never depend on Tauri.
  3. **`src-tauri/`** — Thin Tauri shell. Exposes `procnote-app` operations as IPC commands and owns desktop-only concerns: the native file picker and its attachment grants, revealing directories, and DropPoint.
  4. **`src/`** — React + Vite SPA frontend (react-router), shared by the desktop app and the browser.
- **`crates/procnote-server/`** is an axum HTTP shell over `procnote-app`, parallel to `src-tauri`, for using the same UI from a browser. It must never depend on Tauri.
- **`crates/procnote-launcher/`** is a Tauri-free console adapter outside those domain layers. It owns `procnote [WORKSPACE]`, prints help/version synchronously, and detaches the packaged GUI. It must never depend on `procnote-tauri`, Tauri, or GUI/dialog libraries.

### Core domain (`procnote-core`)

- `Event` enum (internally tagged `"type"` for JSON) — 13 variants covering execution lifecycle, step transitions, data capture, and revert markers.
- `ExecutionState::apply()` is the single source of truth for state mutations.
- `ExecutionState::from_events()` reconstructs state by replaying the event log in order; `LogMeta` is ignored by the state machine.
- User-visible reversals are modeled as typed domain events (`InputCleared`, `StepUnskipped`, `ExecutionReopened`, etc.), not generic event-log reverts.
- `ExecutionState::apply()` exhaustively handles every event variant, so new event types must define their state-machine behavior explicitly.
- Procedure templates are Markdown files with YAML frontmatter, parsed by `template/parser.rs` using `pulldown-cmark`.

### Application layer (`procnote-app`)

- Operations take `procedures_dir: &Path` explicitly. Each procedure is a subdirectory containing `template.md` and `.executions/`.
- `summarize()` converts `ExecutionState` + raw events into `ExecutionSummary` DTO for the frontend, building timestamp maps from the replayed event stream.
- Every `record_action` call re-reads and replays the full event log from disk (no in-memory cache, by design).
- Attachments are stored as `attachments/{sha256_7}-{filename}` inside the execution directory.

### Tauri shell (`src-tauri`)

- `AppState` holds the `procedures_dir` path, DropPoint configuration, and the attachment grants issued by the native file picker.
- Attachment actions are authorized against those grants before being passed to `procnote_app::execution::record_action`.

### API server (`procnote-server`)

- Routes under `/api` mirror the IPC commands; other paths serve the built frontend (`--static-dir`) with an SPA fallback.
- No authentication yet: it listens on `127.0.0.1` by default and is meant for trusted networks only.
- Browsers upload attachments to `/api/uploads` first and receive single-use upload tokens. Attachment actions may only reference files by such tokens, mirroring the desktop's file picker grants, so clients can never make the server read arbitrary paths.
- `just dev-server` + `just dev-frontend` for development (Vite proxies `/api`), `just serve` to build and serve.

### Frontend (`src/`)

- `lib/api/commands.ts` talks to Tauri IPC in the desktop app and to the HTTP API in a browser. Host services (logging, confirm dialogs, opening URLs) go through `lib/platform.ts`; components must not import Tauri plugins directly.

- `executionStore` (in `lib/stores/execution.ts`) is a framework-independent store wrapping `ExecutionSummary`; components subscribe via `useExecutionStore()` (`useSyncExternalStore`). Keep it free of React so later stages can add sync logic there.
- Component styles are CSS Modules (`*.module.css`). `scoped(styles)` from `lib/utils/cx.ts` emits both the raw class name (so global rules in `src/global.css` such as `.btn` apply) and the module-scoped one.
- Frontend tests use `vite-plus/test` (Vitest) and run with `pnpm test`.
- TypeScript types in `lib/types/generated/` are auto-generated from Rust via `ts-rs`. Regenerate with `cargo test --workspace export_bindings_`. CI enforces they stay in sync.
- Timestamps cross the IPC boundary as ISO 8601 strings.

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
- `pnpm check` — TypeScript type checking for frontend (`tsc --noEmit`).
- `pnpm test` — frontend unit tests.
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
