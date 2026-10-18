// Local-first execution recording for the browser: every execution the
// browser has opened or started is kept on the device (IndexedDB), actions
// are turned into events here by the WebAssembly core and shown at once, and
// the events are synced with the server whenever it can be reached. While the
// server is unreachable everything keeps working; nothing is lost.
//
// What the server refuses later because the execution had moved on is kept
// there as a conflict (see procnote_app::sync), not dropped here.
import { LoginRequiredError, type PushResult, UnreachableError } from "#lib/api/commands.js";
import type {
  ActionMeta,
  AttachmentRecord,
  AttachmentSource,
  Event,
  EventRecord,
  ExecutionAction,
  ExecutionSummary,
  ProcedureTemplate,
} from "#lib/types/index.js";
import type { ExecutionApi } from "#lib/stores/execution.js";

/** Where local copies are kept (IndexedDB in the app, a Map in tests). */
export interface KeyValueStore {
  get<T>(key: string): Promise<T | undefined>;
  set(key: string, value: unknown): Promise<void>;
  del(key: string): Promise<void>;
  entries<T>(): Promise<[string, T][]>;
}

/** The server's sync API (`lib/api/commands.ts`). */
export interface SyncServer {
  pullEvents(
    executionId: string,
    after: number,
  ): Promise<{ records: EventRecord[]; count: number }>;
  pushEvents(
    executionId: string,
    events: EventRecord[],
    uploads: string[],
  ): Promise<{ results: PushResult[] }>;
  uploadFiles(files: { name: string; blob: Blob }[]): Promise<AttachmentSource[]>;
  registerExecution(
    executionId: string,
    procedure: string,
    template: ProcedureTemplate,
    events: EventRecord[],
  ): Promise<ExecutionSummary>;
  getTemplate(templatePath: string): Promise<ProcedureTemplate>;
  getSharedTemplate(templateId: string): Promise<ProcedureTemplate>;
}

/** The execution logic (`lib/wasm/core.ts`). */
export interface Core {
  ready(): Promise<unknown>;
  summarize(records: EventRecord[]): ExecutionSummary;
  decide(records: EventRecord[], action: ExecutionAction, files?: AttachmentRecord[]): Event[];
  start(template: ProcedureTemplate): Event[];
  attachmentRecord(filename: string, sha256: string): AttachmentRecord;
}

/** A file attached on this device, kept until the server has its content. */
interface KeptFile {
  name: string;
  type: string;
  data: ArrayBuffer;
}

/** Why the last sync didn't get through, if it didn't. */
export type SyncProblem = "unreachable" | "login" | null;

/** An execution as kept on this device. */
export interface LocalExecution {
  id: string;
  /** Log lines the server has confirmed, in its order (starting with LogMeta). */
  records: EventRecord[];
  /** Recorded here and not yet accepted by the server, in order. */
  pending: EventRecord[];
  /** Pushed but refused as malformed: kept so nothing recorded is lost. */
  rejected: { record: EventRecord; reason: string }[];
  /**
   * Set while the execution exists only here (started offline); `records`
   * then holds its starting events.
   */
  unregistered?: { procedure: string; template: ProcedureTemplate };
}

/**
 * The server could not be reached (not a refusal): the request helper's
 * `UnreachableError`, or a `fetch` that failed outright.
 */
export function isUnreachable(error: unknown): boolean {
  return error instanceof UnreachableError || error instanceof TypeError;
}

const EXECUTION_KEY = "execution:";
const TEMPLATE_KEY = "template:";
const SHARED_TEMPLATE_KEY = "shared-template:";
const FILE_KEY = "file:";
/** The path of an attachment source kept on this device: `local:{sha256}`. */
const LOCAL_FILE = "local:";

function sha256Hex(data: ArrayBuffer): Promise<string> {
  return crypto.subtle
    .digest("SHA-256", data)
    .then((digest) =>
      [...new Uint8Array(digest)].map((b) => b.toString(16).padStart(2, "0")).join(""),
    );
}

/** The attachment records an event adds. */
function attachmentsOf(record: EventRecord): AttachmentRecord[] {
  return record.type === "attachments_added" ? record.attachments : [];
}

const PREVIEWABLE = ["image/png", "image/jpeg", "image/gif", "image/webp", "image/bmp"];

function withMeta(events: Event[], meta: ActionMeta): EventRecord[] {
  return events.map((event, index) => ({
    ...event,
    // One action may record several events; each needs its own ID.
    event_id: index === 0 && meta.event_id ? meta.event_id : crypto.randomUUID(),
    recorded_by: meta.recorded_by ?? undefined,
    device: meta.device ?? undefined,
  }));
}

/** The procedure directory of a workspace template path (`<dir>/template.md`). */
function procedureOf(templatePath: string): string {
  const parts = templatePath.replaceAll("\\", "/").split("/").filter(Boolean);
  return parts.at(-2) ?? "procedure";
}

/** Unsynced recordings written to a file, to survive losing this browser's storage. */
export interface OfflineBackup {
  format: "procnote-offline-backup";
  version: 1;
  /** ISO 8601 timestamp. */
  created_at: string;
  executions: LocalExecution[];
  /** Content of attached files, by SHA-256. */
  files: Record<string, { name: string; type: string; base64: string }>;
}

function eventIds(records: EventRecord[]): Set<string> {
  return new Set(records.flatMap((record) => (record.event_id ? [record.event_id] : [])));
}

/**
 * A kept execution with a backed-up copy merged in: recordings missing here
 * are added back, and nothing is doubled (events are matched by event ID;
 * the server deduplicates the same way).
 */
export function mergeBackup(
  kept: LocalExecution | undefined,
  backup: LocalExecution,
): LocalExecution {
  if (!kept) return backup;
  const known = eventIds([...kept.records, ...kept.pending]);
  const knownRejected = eventIds(kept.rejected.map((r) => r.record));
  return {
    ...kept,
    pending: [
      ...kept.pending,
      ...backup.pending.filter((record) => !record.event_id || !known.has(record.event_id)),
    ],
    rejected: [
      ...kept.rejected,
      ...backup.rejected.filter((r) => !r.record.event_id || !knownRejected.has(r.record.event_id)),
    ],
  };
}

function toBase64(data: ArrayBuffer): string {
  const bytes = new Uint8Array(data);
  let binary = "";
  for (let i = 0; i < bytes.length; i += 0x8000) {
    binary += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  }
  return btoa(binary);
}

function fromBase64(text: string): ArrayBuffer {
  const binary = atob(text);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
  return bytes.buffer;
}

export class LocalExecutions implements ExecutionApi {
  readonly #store: KeyValueStore;
  readonly #server: SyncServer;
  readonly #online: ExecutionApi;
  readonly #core: Core;
  /** Per-execution queue, so syncing and recording never interleave. */
  #locks = new Map<string, Promise<unknown>>();
  #listeners = new Set<(executionId: string, summary: ExecutionSummary) => void>();
  #pendingListeners = new Set<() => void>();
  #problem: SyncProblem = null;

  /** `online` is the plain server API, used when the server can be reached. */
  constructor(store: KeyValueStore, server: SyncServer, online: ExecutionApi, core: Core) {
    this.#store = store;
    this.#server = server;
    this.#online = online;
    this.#core = core;
  }

  #exclusive<T>(executionId: string, run: () => Promise<T>): Promise<T> {
    const previous = this.#locks.get(executionId) ?? Promise.resolve();
    const next = previous.then(run, run);
    this.#locks.set(
      executionId,
      next.catch(() => undefined),
    );
    return next;
  }

  async #load(executionId: string): Promise<LocalExecution | undefined> {
    return this.#store.get<LocalExecution>(EXECUTION_KEY + executionId);
  }

  async #save(local: LocalExecution): Promise<void> {
    await this.#store.set(EXECUTION_KEY + local.id, local);
    for (const listener of this.#pendingListeners) listener();
  }

  #summary(local: LocalExecution): ExecutionSummary {
    return this.#core.summarize([...local.records, ...local.pending]);
  }

  #notify(local: LocalExecution) {
    const summary = this.#summary(local);
    for (const listener of this.#listeners) listener(local.id, summary);
  }

  /** A fresh local copy of an execution the server has. */
  async #fetch(executionId: string): Promise<LocalExecution> {
    const { records } = await this.#server.pullEvents(executionId, 0);
    const local: LocalExecution = { id: executionId, records, pending: [], rejected: [] };
    await this.#save(local);
    return local;
  }

  // --- ExecutionApi ---

  async getExecutionState(executionId: string): Promise<ExecutionSummary> {
    await this.#core.ready();
    const local = await this.#exclusive(executionId, async () => {
      const kept = await this.#load(executionId);
      if (!kept) return this.#fetch(executionId);
      return this.#syncLocked(kept).catch((error: unknown) => {
        if (!isUnreachable(error)) throw error;
        return kept;
      });
    });
    return this.#summary(local);
  }

  async recordAction(
    executionId: string,
    action: ExecutionAction,
    meta: ActionMeta,
  ): Promise<ExecutionSummary> {
    const sources =
      action.action === "add_attachments"
        ? action.files
        : action.action === "add_attachment"
          ? [action]
          : [];
    if (sources.some((source) => !source.path.startsWith(LOCAL_FILE))) {
      // Files already uploaded (by upload token) are attached on the server.
      const summary = await this.#online.recordAction(executionId, action, meta);
      await this.#exclusive(executionId, async () => {
        const kept = await this.#load(executionId);
        if (kept) await this.#syncLocked(kept);
      });
      return summary;
    }
    await this.#core.ready();
    const files = sources.map((source) =>
      this.#core.attachmentRecord(source.filename, source.path.slice(LOCAL_FILE.length)),
    );
    await this.#core.ready();
    const local = await this.#exclusive(executionId, async () => {
      const kept = (await this.#load(executionId)) ?? (await this.#fetch(executionId));
      const events = this.#core.decide([...kept.records, ...kept.pending], action, files);
      const next = { ...kept, pending: [...kept.pending, ...withMeta(events, meta)] };
      await this.#save(next);
      return next;
    });
    this.#syncSoon(executionId);
    return this.#summary(local);
  }

  async startExecution(templatePath: string, meta: ActionMeta): Promise<ExecutionSummary> {
    try {
      const summary = await this.#online.startExecution(templatePath, meta);
      await this.#exclusive(summary.execution_id, () => this.#fetch(summary.execution_id));
      return summary;
    } catch (error) {
      if (!isUnreachable(error)) throw error;
    }
    const template = await this.#store.get<ProcedureTemplate>(TEMPLATE_KEY + templatePath);
    if (!template) throw "This template isn't saved on this device, so it can't start offline.";
    return this.#startHere(procedureOf(templatePath), template, meta);
  }

  async startSharedExecution(templateId: string, meta: ActionMeta): Promise<ExecutionSummary> {
    try {
      const summary = await this.#online.startSharedExecution(templateId, meta);
      await this.#exclusive(summary.execution_id, () => this.#fetch(summary.execution_id));
      return summary;
    } catch (error) {
      if (!isUnreachable(error)) throw error;
    }
    const template = await this.#store.get<ProcedureTemplate>(SHARED_TEMPLATE_KEY + templateId);
    if (!template) throw "This template isn't saved on this device, so it can't start offline.";
    return this.#startHere(template.metadata.id, template, meta);
  }

  /** Start an execution on this device; it is registered with the server on sync. */
  async #startHere(
    procedure: string,
    template: ProcedureTemplate,
    meta: ActionMeta,
  ): Promise<ExecutionSummary> {
    await this.#core.ready();
    const records = withMeta(this.#core.start(template), meta);
    const started = records[0];
    if (started?.type !== "execution_started") throw "starting produced no execution";
    const local: LocalExecution = {
      id: started.execution_id,
      records,
      pending: [],
      rejected: [],
      unregistered: { procedure, template },
    };
    await this.#exclusive(local.id, () => this.#save(local));
    this.#syncSoon(local.id);
    return this.#summary(local);
  }

  /** Local summaries go to `onSummary`; the server's announcements trigger a sync. */
  watchExecution(
    executionId: string,
    _onSeq: (seq: number) => void,
    onSummary?: (summary: ExecutionSummary) => void,
  ): () => void {
    const listener = (id: string, summary: ExecutionSummary) => {
      if (id === executionId) onSummary?.(summary);
    };
    this.#listeners.add(listener);
    // The server announces its log length; pull when it has more than we hold.
    const stopRemote = this.#online.watchExecution(executionId, () => {
      void this.sync(executionId).catch(() => undefined);
    });
    return () => {
      this.#listeners.delete(listener);
      stopRemote();
    };
  }

  // --- Sync ---

  /** Bring one execution in step with the server. Throws when unreachable. */
  sync(executionId: string): Promise<LocalExecution | undefined> {
    return this.#exclusive(executionId, async () => {
      const kept = await this.#load(executionId);
      return kept && this.#syncLocked(kept);
    });
  }

  async #syncLocked(kept: LocalExecution): Promise<LocalExecution> {
    let local = kept;
    if (local.unregistered) {
      await this.#server.registerExecution(
        local.id,
        local.unregistered.procedure,
        local.unregistered.template,
        local.records,
      );
      // The server's log (with its LogMeta) replaces the starting events.
      const { records } = await this.#server.pullEvents(local.id, 0);
      local = { ...local, records, unregistered: undefined };
      await this.#save(local);
    }
    if (local.pending.length > 0) {
      const uploads = await this.#uploadKeptFiles(local.pending);
      const { results } = await this.#server.pushEvents(local.id, local.pending, uploads);
      const rejected = local.pending.flatMap((record, index) => {
        const result = results[index];
        return result?.outcome === "rejected" ? [{ record, reason: result.reason }] : [];
      });
      // The server has the content of everything it took (conflicts included).
      for (const record of local.pending) {
        if (rejected.some((r) => r.record === record)) continue;
        for (const file of attachmentsOf(record)) await this.#store.del(FILE_KEY + file.sha256);
      }
      local = { ...local, pending: [], rejected: [...local.rejected, ...rejected] };
      await this.#save(local);
    }
    const { records, count } = await this.#server.pullEvents(local.id, local.records.length);
    if (records.length > 0 || count !== local.records.length) {
      local = {
        ...local,
        records:
          count === local.records.length + records.length
            ? [...local.records, ...records]
            : (await this.#server.pullEvents(local.id, 0)).records,
      };
      await this.#save(local);
    }
    await this.#core.ready();
    this.#notify(local);
    return local;
  }

  #syncSoon(executionId: string) {
    this.sync(executionId).then(
      () => this.#setProblem(null),
      (error: unknown) => {
        if (error instanceof LoginRequiredError) this.#setProblem("login");
        else if (isUnreachable(error)) this.#setProblem("unreachable");
        else console.warn(`[sync] ${executionId}: ${String(error)}`);
      },
    );
  }

  #setProblem(problem: SyncProblem) {
    if (this.#problem === problem) return;
    this.#problem = problem;
    for (const listener of this.#pendingListeners) listener();
  }

  /** Why the last sync didn't get through (null when it did). */
  syncProblem(): SyncProblem {
    return this.#problem;
  }

  /** Upload the kept content of files the events attach; returns upload tokens. */
  async #uploadKeptFiles(records: EventRecord[]): Promise<string[]> {
    const files: { name: string; blob: Blob }[] = [];
    const seen = new Set<string>();
    for (const attachment of records.flatMap(attachmentsOf)) {
      if (seen.has(attachment.sha256)) continue;
      seen.add(attachment.sha256);
      const kept = await this.#store.get<KeptFile>(FILE_KEY + attachment.sha256);
      if (kept) files.push({ name: attachment.filename, blob: new Blob([kept.data]) });
    }
    if (files.length === 0) return [];
    return (await this.#server.uploadFiles(files)).map((source) => source.path);
  }

  // --- Files attached on this device ---

  /** Keep chosen files on this device; their sources refer to them as `local:{sha256}`. */
  async keepFiles(files: File[]): Promise<AttachmentSource[]> {
    const sources: AttachmentSource[] = [];
    for (const file of files) {
      const data = await file.arrayBuffer();
      const sha256 = await sha256Hex(data);
      const kept: KeptFile = { name: file.name, type: file.type, data };
      await this.#store.set(FILE_KEY + sha256, kept);
      sources.push({ filename: file.name, path: LOCAL_FILE + sha256, content_type: file.type });
    }
    return sources;
  }

  /**
   * A data URL previewing an attachment whose content is still only on this
   * device, or undefined when the server has it (or it isn't an image).
   */
  async preview(executionId: string, path: string): Promise<string | null | undefined> {
    const local = await this.#load(executionId);
    const attachment = local?.pending.flatMap(attachmentsOf).find((a) => a.path === path);
    if (!attachment) return undefined;
    const kept = await this.#store.get<KeptFile>(FILE_KEY + attachment.sha256);
    if (!kept) return undefined;
    if (!PREVIEWABLE.includes(attachment.content_type)) return null;
    return `data:${attachment.content_type};base64,${toBase64(kept.data)}`;
  }

  /** Sync every execution with something waiting; quietly gives up when unreachable. */
  async syncAll(): Promise<void> {
    for (const local of await this.waiting()) this.#syncSoon(local.id);
  }

  /** Local copies with events not yet on the server. */
  async waiting(): Promise<LocalExecution[]> {
    const entries = await this.#store.entries<LocalExecution>();
    return entries
      .filter(([key]) => key.startsWith(EXECUTION_KEY))
      .map(([, local]) => local)
      .filter((local) => local.pending.length > 0 || local.unregistered !== undefined);
  }

  /** How many recorded things are waiting to reach the server. */
  async waitingCount(): Promise<number> {
    return (await this.waiting()).reduce(
      (count, local) =>
        count + local.pending.length + (local.unregistered ? local.records.length : 0),
      0,
    );
  }

  /** Summaries of every execution kept here (for listing while offline). */
  async summaries(): Promise<ExecutionSummary[]> {
    await this.#core.ready();
    const entries = await this.#store.entries<LocalExecution>();
    return entries
      .filter(([key]) => key.startsWith(EXECUTION_KEY))
      .map(([, local]) => this.#summary(local));
  }

  // --- Backups of what isn't synced ---

  /** Everything not yet on the server (and refused events), with attached files. */
  async backup(now = new Date()): Promise<OfflineBackup> {
    const executions = (await this.#store.entries<LocalExecution>())
      .filter(([key]) => key.startsWith(EXECUTION_KEY))
      .map(([, local]) => local)
      .filter(
        (local) =>
          local.pending.length > 0 || local.unregistered !== undefined || local.rejected.length > 0,
      );
    const files: OfflineBackup["files"] = {};
    const attached = executions.flatMap((local) => [
      ...local.pending.flatMap(attachmentsOf),
      ...local.rejected.flatMap((r) => attachmentsOf(r.record)),
    ]);
    for (const attachment of attached) {
      const kept = await this.#store.get<KeptFile>(FILE_KEY + attachment.sha256);
      if (kept)
        files[attachment.sha256] = {
          name: kept.name,
          type: kept.type,
          base64: toBase64(kept.data),
        };
    }
    return {
      format: "procnote-offline-backup",
      version: 1,
      created_at: now.toISOString(),
      executions,
      files,
    };
  }

  /** Put a backup's recordings back on this device; they sync like any others. */
  async restore(backup: OfflineBackup): Promise<number> {
    if (backup.format !== "procnote-offline-backup" || backup.version !== 1) {
      throw "This file isn't a procnote backup of unsynced records.";
    }
    for (const [sha256, file] of Object.entries(backup.files)) {
      const kept: KeptFile = { name: file.name, type: file.type, data: fromBase64(file.base64) };
      await this.#store.set(FILE_KEY + sha256, kept);
    }
    for (const execution of backup.executions) {
      await this.#exclusive(execution.id, async () =>
        this.#save(mergeBackup(await this.#load(execution.id), execution)),
      );
    }
    await this.syncAll();
    return backup.executions.length;
  }

  /** Events of an execution the server refused as malformed. */
  async rejected(executionId: string): Promise<LocalExecution["rejected"]> {
    return (await this.#load(executionId))?.rejected ?? [];
  }

  /** Be told whenever what's waiting, or why it's waiting, changes. */
  onWaitingChange(listener: () => void): () => void {
    this.#pendingListeners.add(listener);
    return () => this.#pendingListeners.delete(listener);
  }

  // --- Templates kept for starting offline ---

  /** Keep the workspace templates' contents on this device. */
  async keepTemplates(templatePaths: string[]): Promise<void> {
    for (const path of templatePaths) {
      await this.#store.set(TEMPLATE_KEY + path, await this.#server.getTemplate(path));
    }
  }

  /** Keep the shared templates (those that can run) on this device. */
  async keepSharedTemplates(templateIds: string[]): Promise<void> {
    for (const id of templateIds) {
      try {
        await this.#store.set(SHARED_TEMPLATE_KEY + id, await this.#server.getSharedTemplate(id));
      } catch (error) {
        // A template with issues can't run; keep whatever version was kept before.
        if (isUnreachable(error)) throw error;
      }
    }
  }

  /** A value kept for listing while offline. */
  async kept<T>(key: string): Promise<T | undefined> {
    return this.#store.get<T>(`list:${key}`);
  }

  async keep(key: string, value: unknown): Promise<void> {
    await this.#store.set(`list:${key}`, value);
  }
}
