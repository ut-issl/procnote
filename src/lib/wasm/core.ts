// procnote's execution logic (crates/procnote-wasm) for the browser: the same
// state machine as the server, so actions can be recorded while offline.
// Call `loadCore()` once before the other functions.
import init, {
  attachmentRecord as attachmentRecordJson,
  decide as decideJson,
  initSync,
  start as startJson,
  summarize as summarizeJson,
} from "./pkg/procnote_wasm.js";
import type {
  AttachmentRecord,
  Event,
  EventRecord,
  ExecutionAction,
  ExecutionSummary,
  ProcedureTemplate,
} from "#lib/types/index.js";

let loading: Promise<unknown> | null = null;

/** Load the WebAssembly module (fetched next to the bundle). */
export function loadCore(): Promise<unknown> {
  loading ??= init();
  return loading;
}

/** Load the module from its bytes, where it can't be fetched (tests). */
export function loadCoreFrom(bytes: BufferSource): void {
  initSync({ module: bytes });
  loading = Promise.resolve();
}

/** The summary the UI shows for an execution's log lines. */
export function summarize(records: EventRecord[]): ExecutionSummary {
  return JSON.parse(summarizeJson(JSON.stringify(records))) as ExecutionSummary;
}

/**
 * The events `action` records on top of `records`, timed by this device's
 * clock. Throws (with the reason) when the action isn't allowed now.
 * `files` are the attachments an `add_attachments` action adds.
 */
export function decide(
  records: EventRecord[],
  action: ExecutionAction,
  files: AttachmentRecord[] = [],
): Event[] {
  return JSON.parse(
    decideJson(JSON.stringify(records), JSON.stringify(action), JSON.stringify(files)),
  ) as Event[];
}

/** The events that start an execution of `template`, with a new execution ID. */
export function start(template: ProcedureTemplate): Event[] {
  return JSON.parse(startJson(JSON.stringify(template))) as Event[];
}

/** How a file is recorded: `attachments/{sha256_7}-{sanitized name}`, as the server stores it. */
export function attachmentRecord(filename: string, sha256: string): AttachmentRecord {
  return JSON.parse(attachmentRecordJson(filename, sha256)) as AttachmentRecord;
}
