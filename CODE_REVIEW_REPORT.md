# Codebase Review Report

- **Date:** 2026-07-02
- **Reviewed at:** commit `a924b4e` (main)
- **Scope:** `crates/procnote-core/`, `src-tauri/` (including `drop_point/`), `src/` (frontend), Tauri configuration
- **Method:** Full read of all non-generated source files (~10k lines). Parser findings were additionally verified by running probe inputs through `parse_template`; high-severity claims were independently re-verified against the code.

Severity reflects impact on this app's threat model: a local-first desktop tool whose event log is the system of record, with one network-facing subsystem (DropPoint) and webview-rendered Markdown.

---

## Summary of highest-priority items

| # | Severity | Area | Finding |
|---|----------|------|---------|
| S1 | High | Tauri config | CSP disabled (`"csp": null`), turning any webview injection into full IPC access |
| S2 | High | IPC commands | `load_template` / `start_execution` / `AddAttachment` accept arbitrary filesystem paths from the frontend |
| B1 | High | Template parser | Nested task lists mangle checkbox text, drop checkboxes, and duplicate content as prose |
| B2 | High | Execution engine | Steps containing a pre-checked `- [x]` template checkbox can never be skipped |
| B3 | High | Windows support | `sync_dir` fails unconditionally on Windows — starting an execution or recording the first action errors |
| S3 | Medium | DropPoint | Server-supplied `drop_link` is rendered into the QR without origin validation, defeating E2E encryption |
| B4 | Medium | Frontend | Last-write-wins race in `executionStore.act()` can display stale state |
| B5 | Medium | Persistence | Executions are looked up by 8-char UUID prefix, first match wins |

---

## 1. Security findings

### S1. Content Security Policy is disabled — High

`src-tauri/tauri.conf.json:23` sets `"csp": null`. The app renders template-author-controlled Markdown (via `marked` + DOMPurify) and backend-supplied SVG. DOMPurify is a single point of failure; without a CSP there is no second layer, and any HTML/script injection in the webview gains full access to every IPC command — directly weaponizing S2. Configure a restrictive CSP (Tauri injects nonces automatically for its own scripts).

### S2. IPC commands accept arbitrary filesystem paths — High

Per Tauri's threat model, IPC inputs are attacker-controlled once the webview is compromised (see S1). Execution IDs are properly typed (`ExecutionId` wraps a `Uuid`), but raw paths are not validated:

- `src-tauri/src/commands/template.rs:90-92` — `load_template(path)` reads any file on disk.
- `src-tauri/src/commands/execution.rs:599-629` + `persistence/execution_store.rs:50-55` — `start_execution(template_path)` reads any file **and creates `.executions/…` directories, a `template.md` snapshot, and `events.jsonl` next to it** — an arbitrary-directory write primitive.
- `src-tauri/src/action.rs:63-74` + `persistence/execution_store.rs:241,306` + `attachment_store.rs:31` — `AddAttachment { path }` copies any readable file into the execution's `attachments/` dir, after which `get_attachment_preview_data_url` will base64-encode it (the `image/*` check trusts the attacker-supplied `content_type` stored in the event) — an arbitrary-file-read/exfiltration primitive.

Neither command checks containment in `state.procedures_dir`, which `list_templates` correctly scopes to. Attaching user-picked files is a feature, but the picked path should come from (or be validated against) the trusted dialog plugin rather than accepted as a raw IPC string.

### S3. DropPoint: server-supplied `drop_link` not origin-checked — Medium

`src-tauri/src/commands/drop_point.rs:268-282` parses the server's `drop_link` for syntax only and embeds it in the QR code. A compromised or malicious DropPoint server can return `drop_link: "https://evil.example/drop"`; the phone then loads the attacker's page, which simply never performs the client-side encryption and receives the files in plaintext. This defeats the E2E-encryption design whose whole point is that the server never sees plaintext. Require the `drop_link` origin to match the configured `base_url`.

### S4. DropPoint pickup downloads unbounded bodies into memory — Medium

`src-tauri/src/drop_point/client.rs:184` (`response.bytes().await?.to_vec()`) enforces neither `config.max_bytes` nor a `Content-Length` cap; decryption and payload splitting then allocate ~3× the body size (`crypto.rs:105`, `manifest.rs:95`). A hostile server (or anyone who photographed the QR) can OOM the desktop app mid-procedure. `ensure_success` (`client.rs:215`) also buffers unbounded error bodies. Cap via `Content-Length` plus a streamed read against a hard limit.

### S5. Attachment filenames are not sanitized on the write side — Medium

`src-tauri/src/persistence/attachment_store.rs:44-49` builds `{sha256_7}-{filename}` with no validation. Traversal is blocked only *accidentally* (the hash prefix glues onto the first path segment, so `../x` fails with ENOENT). Consequences today: a legitimate filename containing `/` (or `\` on Windows) fails with a bewildering error; the raw name is baked into the event's immutable `relative_path`, which the read-side containment check (`execution.rs:531-558`, which is a textbook single-`Component::Normal` + canonicalize check) then rejects — a recorded attachment that can never be previewed. DropPoint's `sanitize_filename` (`drop_point/manifest.rs:150-173`) is stricter but still admits trailing dots (Windows strips them on create, so the on-disk name diverges from the logged `relative_path`) and Unicode bidi/format characters such as U+202E (category `Cf`, not caught by `is_control()`) enabling extension spoofing in the UI. Apply one shared single-component sanitizer on every write path, mirroring the read side.

### S6. Attachment paths in events are replayed with zero validation — Medium

`crates/procnote-core/src/event/types.rs:9-14` and `engine.rs:341-368,701-721` accept arbitrary `path` strings into state. Since the storage layout is explicitly git-friendly (logs may be synced/pulled from a shared repo), a tampered `events.jsonl` containing `AttachmentAdded { path: "../../…" }` replays successfully, and safety depends on every filesystem consumer sanitizing before joining. Reject absolute paths and `..` components at the event boundary — the single source of truth.

### S7. DropPoint: server-supplied `drop_point_id` spliced into URL paths unencoded — Low

`src-tauri/src/drop_point/client.rs:159,173,195,202-205` interpolate the ID from the server's create response into URL paths with no encoding or character validation; `../`, `?`, and `#` can steer subsequent bearer-authenticated GET/DELETE calls to other same-origin paths. Validate the ID as an opaque token or build URLs with `path_segments_mut()`.

### S8. DropPoint: `http://` base URLs accepted — Low

`src-tauri/src/drop_point/client.rs:63-75` never requires `https`, so `api_token` and `pickup_token` can travel in cleartext. Payload confidentiality survives (E2E), but the tokens allow drop-point creation/deletion/pickup replay. Enforce `https` (optionally exempting localhost).

### S9. Markdown links navigate the webview away from the app — Low (security/UX)

`src/lib/components/StepCard.svelte:68-72,228` — sanitized `<a href>` links survive DOMPurify, and nothing intercepts clicks; in a Tauri webview a plain anchor click replaces the app UI with the external site, losing in-progress unrecorded input. Route link clicks through `@tauri-apps/plugin-opener` (already a dependency).

### S10. Minor hardening items — Low

- `{@html remoteSession.qr_svg}` (`AttachmentField.svelte:459`) is the app's single unsanitized `{@html}` sink. Safe today (the `qrcode` crate emits only `<rect>` geometry), but its safety rests on a dependency's implementation detail; pass it through DOMPurify's SVG profile.
- `image/svg+xml` passes the `preview_content_type` filter (`execution.rs:517-529`). Harmless inside `<img>`, but with CSP disabled it becomes scriptable if the data URL is ever rendered in an `<object>` or new tab.
- Recipient private keys and derived AES keys are never zeroized and are copied on every session `get` (`drop_point/crypto.rs:54-58,213-219`, `session.rs:6-15`); wrap in `zeroize::Zeroizing`.
- The DH shared-secret all-zero check (`crypto.rs:89`) short-circuits; use `subtle::ConstantTimeEq`. (Practically negligible — small-order rejection only.)
- `opener:allow-reveal-item-in-dir` is granted with no scope in `capabilities/default.json`, allowing the frontend to reveal any path in the OS file manager. The capability set is otherwise commendably minimal.

**Verified sound (no finding):** TLS via rustls + platform verifier with verification on; AES-GCM with AAD, HKDF salt binding both public keys, per-session ephemeral keys (no cross-session replay), strict multipart envelope validation with a 1 MiB cap; multipart header injection unreachable (reqwest `HeaderValue` rejects CR/LF); attachment preview path containment (`resolve_attachment_file_path`) is correct; no regex anywhere in the codebase, per project convention.

---

## 2. Bugs

### Core: template parser

**B1. Nested task lists are mangled, lost, and duplicated — High.**
`parser.rs:294-318` (`collect_task_text`) breaks on the *first* `End(TagEnd::Item)` — for an item containing a nested list, that's the child's end. Verified with probe inputs: `- [ ] parent` / `  - [ ] nested` parses to one checkbox `"parent checkboxnested checkbox"` (texts concatenated, nested checkbox gone) **plus** a `Prose` block duplicating the entire list, because `Start(Tag::List)` events consumed inside `collect_task_text` never increment `list_depth`, so the outer `End(List)` resets `prose_start` to before the list. The existing `test_nested_pure_checkboxes` only asserts `text.contains(...)`, masking this.

**B2. Content before the first `## ` heading is silently discarded — Medium.**
`parser.rs:199-207` gates prose tracking on `current_heading.is_some()`. Verified: an intro paragraph between frontmatter and the first H2 vanishes without warning; a template with no H2 at all "successfully" parses to zero steps.

**B3. A regular bullet with a nested pure task list is torn apart — Medium.**
`parser.rs:133-153` — the backward scan finds the *nearest* preceding `Start(Tag::List)` (the nested one), not the enclosing top-level list the comment claims. Verified: `- prose item` / `  - [ ] nested checkbox` yields `Prose { "- prose item" }` plus an interactive checkbox ripped out of its parent, contradicting the mixed-list-is-prose rule enforced at the top level.

**B4. `split_frontmatter` is fragile substring matching — Medium (smell/bug).**
`parser.rs:28-44` — the opener accepts `----`/`---junk` (trailing text becomes YAML), and `find("\n---")` matches any line merely *starting* with `---`, including one inside a YAML block scalar, truncating the frontmatter early. A line-based scanner requiring the delimiter line to be exactly `---` fits the project's "proper data and logic structures" convention.

**B5. `is_pure_task_list` can underflow `depth` — Low.**
`parser.rs:240-241` uses `depth -= 1` where the main loop uses `saturating_sub`; a misfired backward scan panics in debug builds.

### Core: execution engine

**B6. Steps with a pre-checked `- [x]` template checkbox can never be skipped — High.**
`engine.rs:1068-1076` treats any `checked: true` checkbox as user-captured data, but the parser legitimately emits `checked: true` for `- [x]` template items (confirmed at `parser.rs:124-159`). `skip_step` on such a step fails with `StepHasCapturedData` despite zero operator actions; the only workaround is unchecking the box, which itself pollutes the log. Compare against the template's initial checkbox state instead.

**B7. `InputRecorded` performs no input-definition validation — Medium.**
`engine.rs:281-301` never checks `input_id` exists in the step's `InputBlock` or is scalar-typed, while the attachment path enforces both (`require_attachment_input`). Consequences: typo'd IDs are silently accepted and invisible in UI forms; a scalar value and attachments can coexist under one `input_id`, after which further attachment adds fail with `InputAlreadyRecorded`. (The engine's own tests currently rely on this laxity, e.g. `engine.rs:1210-1212`.)

**B8. `RecordedInput.label` stores the input ID, not the label — Medium.**
`engine.rs:293-300` fills `label: input_id.clone()` even though the real label exists in the step's `InputDefinition`s; any consumer of `RecordedInput.label` displays `"current-draw"` instead of "Measure current draw".

**B9. `after_step_id` referencing a nonexistent step silently appends at the end — Low.**
`engine.rs:224-231` — a typo'd `after_step_id` succeeds and misplaces the step with no feedback; in a safety-procedure tool, misordering deserves an error.

**B10. Checkboxes in dynamically added steps are untoggleable if `id` is `None` — Low.**
`start()` assigns checkbox IDs (`engine.rs:444-461`) but `add_step_event` (`engine.rs:498-527`) accepts raw `StepContent` without assigning or validating IDs; `CheckboxToggled` only matches `id: Some(_)` (`engine.rs:264-276`), so such checkboxes are permanently stuck.

### Core: event log

**B11. Corrupt fields on known event types are misreported as "unknown event type … this is a bug" — Medium.**
`event/log.rs:88-100` — any valid-JSON line that fails `Event` deserialization lands in the `UnknownEventType` branch, including known types with malformed fields, and the message asserts "this is a bug" even for the documented newer-minor-version case. Distinguish "type tag not in the enum" from "type known, payload invalid".

**B12. `append_event` can create a log with no `LogMeta` and never fsyncs — Low.**
`event/log.rs:33-44` happily creates a fresh file with any event (producing a log `read_log` later rejects with `MissingLogMeta` — the crate's own `test_creates_parent_dirs` does exactly this) and never flushes/syncs. The Tauri shell's `append_durable` covers production use; the core API still doesn't enforce the invariant it validates on read.

### Tauri shell

**B13. `sync_dir` cannot work on Windows — High.**
`persistence/event_log.rs:133-135` — `File::open` on a directory fails with `PermissionDenied` on Windows (std doesn't set `FILE_FLAG_BACKUP_SEMANTICS`). `sync_dir` is on the mandatory path of `create_execution`, first lock-file creation, first durable append, and attachment writes — yet `tauri.conf.json` bundles NSIS with `targets: "all"`. On Windows, starting an execution or recording the first action fails unconditionally.

**B14. Execution lookup by 8-char UUID prefix, first match wins — Medium.**
`commands/execution.rs:463-484` (duplicated at `execution_store.rs:340-361`) matches directories by `-{first 8 hex chars}` suffix in nondeterministic `read_dir` order across all procedures. A prefix collision (~50% birthday odds around 77k executions, or trivially via a hand-crafted directory name) makes `record_action` lock and append events to the *wrong* execution's log — silent corruption if both share a template.

**B15. One unreadable `.executions` dir aborts the whole execution search — Medium.**
Same functions: `std::fs::read_dir(&exec_base).ok()?` returns `None` from the entire function instead of skipping the procedure, so a single permissions problem makes *every other* execution "not found", with the underlying error swallowed unlogged.

**B16. Crash-leftover temp dirs appear as ghost executions — Medium.**
`execution_store.rs:64-91` stages into `.{name}.tmp-{uuid}` before rename; `list_executions` (`execution.rs:695-721`) accepts any dir containing `events.jsonl` with no temp-name filtering or cleanup. A crash between write and rename leaves a "ghost" execution that lists forever but can never be opened (the suffix match never finds it).

**B17. Durable append happens before `state.apply` validation — Medium.**
`execution_store.rs:126-129` (and 166-169) append the event durably, *then* apply it. Any divergence between the `*_event` builder's validation and `apply` — exactly the invariant CLAUDE.md says each new variant must maintain — durably commits an event that `from_events` will refuse to replay, rendering the execution permanently unreadable (and silently skipped by `list_executions`). Applying before appending removes the failure mode at zero cost.

**B18. Committed action reported as failure if `unlock()` errors — Low.**
`event_log.rs:49-52` propagates an `unlock` error after `f()` durably appended; the user's retry then duplicates the event. Log the unlock error instead (dropping the file releases the lock anyway).

**B19. Attachment bytes written before event validation — Low.**
`execution_store.rs:162-165,240-262` copy files into `attachments/` before building/validating/appending the event; a bad `step_id` or a finished execution leaves orphaned files nothing ever GCs. Validate the candidate event first. (Same ordering issue on the DropPoint import path.)

**B20. Unlocked readers can silently miss the newest event — Low.**
`get_execution_state`/`list_executions` read without the lock while `read_log` tolerates a truncated final line with only a `log::warn!`; after a crash, the final user action silently disappears with no UI signal.

**B21. `summarize` reports the nil UUID for degenerate logs — Low.**
`execution.rs:342` uses `state.execution_id.unwrap_or_default()`, producing a listing entry that can never be looked up, while sibling code `expect`s the same invariant (`execution.rs:585,609-620`) and would panic the command thread instead of returning `Err`.

### DropPoint

**B22. Session lifecycle leaks — Medium.**
`drop_point/session.rs:17-47`, `commands/drop_point.rs:55-100`: starting a new session for the same input doesn't close the previous one (the old remote drop point keeps accepting drops until server TTL); sessions have no expiry; and being memory-only, a crash loses the `pickup_token`, stranding encrypted data on the server — which also cuts against the project's own "filesystem over in-memory, for crash safety" convention.

**B23. Error paths leak the created remote drop point — Low.**
`commands/drop_point.rs:73-99` — after `create_drop_point()` succeeds, failures in QR rendering or the `u32::try_from(created.max_bytes)` conversion return `Err` without `close()`; the `max_bytes` case fires *after* `sessions.insert`, leaving a live session the frontend can never reach or cancel. Do fallible conversions first; close on subsequent errors.

**B24. Concurrent imports of the same session double-record attachments — Low.**
`session.rs:31-38` returns a clone and removal happens only after commit (`drop_point.rs:136-180`); two overlapping import calls both pass the lookup and both append `AttachmentsAdded`. Atomically remove up front and re-insert on failure. The frontend compounds this: the 1.5s poll interval has no in-flight guard (`AttachmentField.svelte:211-214`), so a slow poll response lets two `"ready"` results each trigger `importRemoteUpload`.

**B25. `u64 → u32` DTO truncation hard-fails for values ≥ 4 GiB — Low.**
`commands/drop_point.rs:97-98,118-119` — a server with `max_bytes` or `encrypted_size` ≥ 2³² makes session start or every poll error permanently. Use `u64` in the DTO.

### Frontend

**B26. Last-write-wins race in `executionStore.act()` — Medium.**
`lib/stores/execution.svelte.ts:43-55` — no queueing or in-flight guard; the last IPC response to *arrive* wins, not the last action *sent*. Rapidly toggling two checkboxes can leave the UI showing the older snapshot even though the log is correct. Sequence actions or discard stale responses.

**B27. Unkeyed `{#each summary.steps}` bleeds component state across steps — Medium.**
`routes/execution/[id]/+page.svelte:248` (and nested `{#each block.inputs}` at `StepCard.svelte:237`) — inserting a step shifts components by index, so un-recorded draft text in `InputField`/`NoteEditor` and open skip-dialog state jump to a *different* step's card. Key by `(stepSummary.id)`.

**B28. Failed `start()` can navigate to a stale previous execution — Medium.**
`routes/+page.svelte:54-59` — `executionStore.start()` swallows errors into `store.error` and leaves the previous `summary` intact; the header home link never calls `reset()`. A failed template start then navigates to the *previous* execution, and the failure is never rendered on the home page.

**B29. Execution page goes blank if `isDropPointConfigured()` rejects — Medium.**
`routes/execution/[id]/+page.svelte:25-30` — no try/catch; a rejection also prevents `executionStore.load()` from running, and with `summary === null, loading === false, error === null` none of the template branches render. The two awaits are also needlessly serialized.

**B30. Optimistic checkbox DOM diverges from store on failure — Low.**
`CheckboxItem.svelte:19-24` — the native checkbox toggles; if `record_action` fails the `checked` prop never changes, so Svelte never resets the DOM: UI shows checked, log says unchecked. Also `checkbox.id ?? ""` silently sends an empty `checkbox_id` instead of refusing.

**B31. Dialogs close and discard input even when the action failed — Low.**
`routes/execution/[id]/+page.svelte:62-91` — `act()` never throws, so the abort/add-step dialogs always close and clear the typed reason; on a disk error the user must retype everything.

**B32. Enter + blur race double-fires rename — Low.**
`routes/execution/[id]/+page.svelte:98-107,142-152` — Enter starts the async save; a blur during the await passes the `trimmed !== summary?.name` guard again, appending two `rename_execution` events.

**B33. Route param changes don't reload the execution — Low (latent).**
`executionId` is `$derived` but only read in `onMount`; navigating `/execution/A → /execution/B` would show A's data. No such navigation exists in the current UI, but it's a trap.

**B34. All image previews cleared and refetched over IPC on every action — Low (performance).**
`AttachmentField.svelte:81-91` — every `act()` replaces `summary` wholesale, so the preview `$effect` reruns: thumbnails flash blank and every image is re-read, base64-encoded, and re-shipped over IPC on every checkbox toggle anywhere in the execution.

**B35. Home page shows time-of-day only — Low.**
`lib/utils/format.ts:5-12` renders `HH:MM:SS`; "Recent Executions" spanning multiple days is ambiguous.

---

## 3. Code smells

- **Copy-pasted `find_execution_dir`** between `commands/execution.rs:464-484` and `persistence/execution_store.rs:341-361` — bugs B14/B15 must be fixed twice. Likewise `record_action` vs `record_attachment_bytes_batch` (`execution_store.rs:105-178`) duplicate the entire find/lock/read/replay/append/apply transaction; extract a `with_log_transaction(execution_id, |state, dir| -> Event)` helper. `EventLog::new(log_path.clone())` is constructed redundantly inside the closures.
- **Redundant hashing and whole-file buffering:** `attachment_store.rs:31,70-79` reads the whole attachment into RAM, writes it, then re-reads the whole file to re-verify a hash of bytes it already held; previews (`execution.rs:568-569`) base64 entire files on the IPC thread. Multi-GB attachments triple peak memory.
- **Duplicate event vocabularies still actively written:** `AttachmentRemoved`/`AttachmentsCleared` and `AttachmentAdded`/`AttachmentsAdded` have identical semantics (`engine.rs:341-389`); log compatibility requires *reading* both forever, but the crate still exposes both *write* paths, minting new duplicates.
- **`LogMeta` special-cased twice:** `from_events` skips it and `apply` also no-ops it (`engine.rs:156-164,398`) — the `apply` arm silently succeeding in any state contradicts the "every variant defines its behavior explicitly" rule.
- **Inconsistent predicates:** `note_exists` scans all steps but `remove_note` skips skipped steps (`engine.rs:945-968`) — dead defensive divergence today, a duplicate-yet-unremovable note if the skip rules ever change.
- **Per-instance Markdown machinery:** 19 `hljs.registerLanguage` calls and `new Marked(...)` run per `StepCard` instance, and `renderMarkdown` re-parses + re-sanitizes every prose block on every summary refresh (`StepCard.svelte:1-66,228`). Move to `<script module>` and memoize.
- **Dead code:** `loadTemplate` in `lib/api/commands.ts:15-17` has no callers; `ExecutionStore.isActive` is unused (the page re-derives it); `executionActive={isActive ?? false}` null-coalesces a value that is always boolean.
- **Fire-and-forget plugin calls** (`open()`, `confirmDialog`, `revealItemInDir`) have no catch — failures surface only as console unhandled rejections.
- **`NoteEditor` reverts by index** and the parent maps back to the id (`NoteEditor.svelte:16,38`, `StepCard.svelte:280-285`) — pass the id it already holds.
- **Byte-for-byte duplicated status-badge/button CSS** across `+page.svelte`, `execution/[id]/+page.svelte`, `StepCard.svelte`, `AddStepDialog.svelte`; the project already has a `:global()` shared-style pattern in `+layout.svelte`.
- **Panicking `expect`s on IPC-handler paths** (`execution.rs:585,609-620,671`) encode real invariants but crash the command thread instead of returning `Err`.
- **Misleading doc comment:** `step_order` claims to hold "step headings" but holds step IDs (`engine.rs:130-131`).
- **`read_log` buffers the whole file twice** (`event/log.rs:53-56`) — raw lines plus parsed events; combined with full replay on every action, a bloated log degrades every UI interaction. The tail-tolerance check needs only one line of lookahead.
- Each DropPoint command constructs a fresh `DropPointClient` (new reqwest connection pool per call).

---

## 4. Conventions verified clean

- **No regex** anywhere in the reviewed code (core, shell, DropPoint, frontend) — the project convention is satisfied; `split_frontmatter` (B4) uses substring search and is flagged for fragility, not regex use.
- **No `unwrap`/`expect` on untrusted input** in core non-test code; test modules carry explicit `#[expect(clippy::unwrap_used)]`.
- **No `any`** in the frontend; props are fully typed against the ts-rs generated types.
- The `events.jsonl` **write path is genuinely solid** where it counts: exclusive advisory lock around the full read/replay/validate/append transaction, fsync + parent-dir sync, temp-dir + rename publication. The concurrent-append race is correctly prevented (modulo B14 targeting the wrong log and B13 on Windows).
- **Timer cleanup** in `AttachmentField` is correct; stale async work is invalidated via run IDs.
- DropPoint **crypto design is sound** overall: contributory-behavior check, HKDF salt binding both public keys, distinct info strings and AAD, protocol pinning, OsRng keygen. The lack of sender authentication is inherent to the QR design and worth documenting explicitly.

## 5. Suggested priorities

1. **Now:** S1 (CSP), S2 (path validation on IPC), B13 (Windows fsync) — S1+S2 together are the only remote-ish compromise path, and B13 blocks a bundled platform outright.
2. **Next:** B1/B2/B3 (parser correctness — silently corrupted or lost template content in the system of record), B6 (unskippable steps), B17 (append-before-validate can brick an execution), B14 (prefix collision), S3/S4 (hostile DropPoint server).
3. **Then:** the medium frontend races (B26–B29), session lifecycle (B22), filename sanitization (S5/S6), and the dedup/cleanup smells.
