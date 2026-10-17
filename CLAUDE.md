# Instructions for Claude

- This project is a procedure execution tool called `procnote`.
- This project is not yet published. So backward compatibility is not a concern. Make the simplest possible implementation.
- `procnote` uses filesystem-based storage with append-only JSONL event logs.
- Use durable storage (the filesystem, or the server's database) and avoid in-memory caches as much as possible, to ensure crash safety and avoid cache invalidation issues. The desktop app keeps the git-friendly directory layout, and the database can be exported back to it.
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
- **`crates/procnote-wasm/`** builds the execution logic (`procnote-core` and `procnote-app` without its `fs` feature) to WebAssembly for the browser, so actions can be recorded offline with the same state machine as the server. `pnpm run build:wasm` (`just build-wasm`, also run by `pnpm build`) compiles it and generates `src/lib/wasm/pkg/` (git-ignored) with the `wasm-bindgen` CLI, which must match the crate's version; `src/lib/wasm/core.ts` wraps it. Type checking and frontend tests need it built.
- **`collab/`** is the Node collab service (Hocuspocus) for collaborative template editing on the server; see below.
- **`crates/procnote-launcher/`** is a Tauri-free console adapter outside those domain layers. It owns `procnote [WORKSPACE]`, prints help/version synchronously, and detaches the packaged GUI. It must never depend on `procnote-tauri`, Tauri, or GUI/dialog libraries.

### Core domain (`procnote-core`)

- `Event` enum (internally tagged `"type"` for JSON) — 13 variants covering execution lifecycle, step transitions, data capture, and revert markers.
- `ExecutionState::apply()` is the single source of truth for state mutations.
- `ExecutionState::from_events()` reconstructs state by replaying the event log in order; `LogMeta` is ignored by the state machine.
- Each log line is an `EventRecord`: the event plus optional `EventMeta` (`event_id`, `recorded_by`, `device`) on the same JSON object. The state machine only sees the event; `ExecutionStore` uses `event_id` to ignore retried actions.
- User-visible reversals are modeled as typed domain events (`InputCleared`, `StepUnskipped`, `ExecutionReopened`, etc.), not generic event-log reverts.
- `ExecutionState::apply()` exhaustively handles every event variant, so new event types must define their state-machine behavior explicitly.
- Procedure templates are Markdown files with YAML frontmatter, parsed by `template/parser.rs` using `pulldown-cmark`.

### Application layer (`procnote-app`)

- The filesystem storage (`persistence/`, which needs `fs2` file locks) is behind the default `fs` feature, so the rest builds for WebAssembly. Pure attachment helpers (naming, hashing) live in `attachment.rs`.
- Operations take `procedures_dir: &Path` explicitly. Each procedure is a subdirectory containing `template.md` and `.executions/`.
- `summarize()` converts `ExecutionState` + raw events into `ExecutionSummary` DTO for the frontend, building timestamp maps from the replayed event stream.
- Execution storage is behind the `ExecutionRepository` trait (`repository.rs`); `ExecutionStore` is the filesystem implementation. What to record is decided by pure functions there (`decide_action`, `decide_receive`), and `record` applies the decision atomically, skipping retried `event_id`s.
- Every `record_action` call re-reads and replays the full event log from storage (no in-memory cache, by design).
- Attachments are stored as `attachments/{sha256_7}-{filename}` inside the execution directory.

### Tauri shell (`src-tauri`)

- `AppState` holds the `procedures_dir` path, DropPoint configuration, and the attachment grants issued by the native file picker.
- Attachment actions are authorized against those grants before being passed to `procnote_app::execution::record_action`.

### API server (`procnote-server`)

- Routes under `/api` mirror the IPC commands; other paths serve the built frontend (`--static-dir`) with an SPA fallback.
- Authentication is Cloudflare Access (Google login, allowlist managed in Access); procnote has no login of its own. With `--access-team-domain` and `--access-aud`, every `/api` request except upload links must carry a valid Access application token (`Cf-Access-Jwt-Assertion` header, else the `CF_Authorization` cookie). `access.rs` verifies RS256 signature (keys from `<team>/cdn-cgi/access/certs`, refetched on unknown `kid`), `aud`, `iss`, and `exp`, and the token's `email` overrides any client-sent recorder. Without those flags the server refuses to start unless `--insecure-no-auth` is given (local development; `just dev-server` / `just serve` pass it). Deploy bound to `127.0.0.1` behind Cloudflare Tunnel, with an Access Bypass for `/upload/*`, `/api/upload-links/*`, and the static assets the phone page loads.
- `GET /api/executions/{id}/live` is a WebSocket sending `{"seq": n}` on connect and after each write through this server. `seq` (`ExecutionSummary.seq`, `ExecutionState::applied_events`) counts recorded events; the frontend store refetches only when a `seq` is ahead of its state and never replaces its state with an older one. Writes by another process (e.g. the desktop app on the same directory) are not announced.
- Browsers upload attachments to `/api/uploads` first and receive single-use upload tokens. Attachment actions may only reference files by such tokens, mirroring the desktop's file picker grants, so clients can never make the server read arbitrary paths.
- Per-execution upload links (`/api/executions/{id}/upload-link`, `/api/upload-links/{token}/...`) let a phone send files without logging in. They land in the execution's inbox (`AttachmentReceived`) and are assigned on the execution screen (`AttachmentAssigned`, back with `AttachmentUnassigned`); nothing is tied to a guessed "current step". Link tokens are files in `--state-dir`, never in the procedures directory. The phone page is `/upload/:token`.
- Executions are stored in the workspace directory by default, or in PostgreSQL with `--database-url` (`PostgresStore` in `postgres.rs`; schema in `migrations/`, applied on start). There each event-log line is an `events` row and attachment content goes to an object store (`object_store`) under `sha256/{hash}`: `--attachment-dir` locally, or `--attachment-bucket` for an S3-compatible bucket such as Cloudflare R2, configured by `AWS_*` variables. Templates are still read from the workspace. `ExecutionRepository` is synchronous, so the store must be called from blocking threads. `procnote-storage import <WORKSPACE>` copies `.executions/` into the database (read-only on the source, skips executions already there) and `procnote-storage export <WORKSPACE>` writes the directory layout back; both take the same `--database-url` and attachment flags. Postgres tests need `PROCNOTE_TEST_DATABASE_URL` (`just dev-db`); they are required on CI.
- Devices that record offline sync through `GET /api/executions/{id}/events?after=n` (log lines from position n), `POST /api/executions/{id}/events` (pushed log lines with their own `event_id` and device time, plus upload tokens for attachment content), and `PUT /api/executions/{id}` (an execution the device started itself). `procnote_app::sync` decides each pushed event: recorded if it still applies, a duplicate if its `event_id` is known, a **conflict** if it no longer applies (kept beside the log — `conflicts.jsonl` or the `conflicts` table — for a person to resolve via `/api/executions/{id}/conflicts`, never dropped), or rejected if malformed or missing content. There are no leases (checkouts) yet.
- `just dev-server` + `just dev-frontend` for development (Vite proxies `/api`), `just serve` to build and serve.

### Collab service (`collab/`)

- Node service (Hocuspocus) for collaborative template editing on the server: each shared template is a Yjs document in PostgreSQL's `templates` table (created by procnote-server's migrations). The server's `templates` rows are the source of truth; workspace `template.md` files are what shared templates start from ("Edit together").
- The document holds the blocks in the `blocks` XML fragment and the `ProcedureMetadata` fields in the `metadata` map (`src/lib/blocknote/blocks.ts`). Block configs and the block ⇄ `ProcedureTemplate` conversion (`convert.ts`) are shared with the browser; `collab/schema.ts` builds the same schema without React.
- procnote-server creates templates (`POST /api/collab/templates`, which seeds the document through the collab service's internal API) and issues short-lived HMAC tokens (`/api/collab/templates/{id}/token`, `collab.rs`) that the collab service checks in `onAuthenticate`, so Cloudflare Access is verified only in Rust. Both share `PROCNOTE_COLLAB_SECRET`; the server enables collab with `--collab-secret` (needs `--database-url`).
- Editors connect to `/collab` on the same origin: Vite proxies it in development, Cloudflare Tunnel routes it to the service (port 5182) in production. The internal API (`/internal/...`) must not be routed publicly.
- Executions start from a shared template as it is at that moment (`POST /api/collab/templates/{id}/executions`): the collab service converts the document (`GET /internal/templates/{id}/template`, with the issues that block execution), and the server starts the execution filed under the procedure ID, keeping the template as `template.json` (a `TemplateSnapshot::Json`) instead of `template.md`. Snapshots are archival; execution state comes from the events.
- Shared templates go back to procnote Markdown with `procnote_core::template::to_markdown` (the inverse of `parse_template`, checked by parsing its output back; content the format can't hold, such as a `##` heading inside prose, is an error): `GET /api/collab/templates/{id}/markdown` (the editor's "Download template.md") and `procnote-storage export-templates <WORKSPACE>` (writes `<procedure ID>/template.md`, `--overwrite` to replace). Prose goes through BlockNote's lossy Markdown conversion; table column alignment is carried across by `tables.ts`, but prose formatting (spacing, list markers) may be rewritten.
- Bundled with `pnpm run build:collab` (`vp pack`); `just dev-collab` builds and runs it. Its tests (`collab/*.test.ts`) run with `pnpm test`; the database ones need `PROCNOTE_TEST_DATABASE_URL`.

### Frontend (`src/`)

- `lib/api/commands.ts` talks to Tauri IPC in the desktop app and to the HTTP API in a browser. Host services (logging, confirm dialogs, opening URLs) go through `lib/platform.ts`; components must not import Tauri plugins directly.

- In a browser, recording is local-first (`lib/offline/`): `LocalExecutions` (the store's `ExecutionApi`) keeps each opened or started execution in IndexedDB (`idb-keyval`) as server-confirmed log lines plus pending ones, turns actions into events with the WASM core, shows the result at once, and syncs (register, push, pull) whenever the server can be reached — at startup, on `online`, every 30 s, after each action. Unreachable (`isUnreachable`) means the request helper threw `UnreachableError`: `fetch` failed, or a gateway (e.g. Cloudflare when the origin is down) answered with 502/503/504/52x without procnote-server's `{"error"}` body; server refusals are strings. Executions can start offline from templates kept on the device (`keepTemplates`, `keepSharedTemplates`). Conflicts and rejected events show in `ConflictsPanel`; the header shows what is not synced (`SyncStatus`). Attached files are kept in IndexedDB (`local:{sha256}` sources), previewed from there, uploaded on sync, and sent with their events as upload tokens. Shared template documents are kept with `y-indexeddb`, so the editor opens and edits offline. `static/sw.js` (registered only in production browser builds) caches the app shell: `/assets/*` cache-first, navigations network-first with the cached `index.html` as fallback; `/api`, `/collab`, and `/upload` always use the network. The desktop app keeps using its backend directly.
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
│           │                       #   (template.json for executions of shared templates)
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
