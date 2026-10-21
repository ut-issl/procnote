import { invoke } from "@tauri-apps/api/core";
import { isDesktop } from "#lib/platform.js";
import type {
  ActionMeta,
  AttachmentDropPointSessionSummary,
  AttachmentDropPointStatus,
  AttachmentSource,
  Conflict,
  EventRecord,
  ExecutionAction,
  ExecutionSummary,
  ProcedureTemplate,
  PushOutcome,
  SyncStatus,
  TemplateSummary,
} from "#lib/types/index.js";

// In the desktop app commands go over Tauri IPC; in a browser they go to the
// procnote-server HTTP API on the same origin. Both reject with a string
// message on failure.

/**
 * The server can't be reached: the network is down, or a gateway in front of
 * it (Cloudflare Tunnel, a proxy) answered for it.
 */
export class UnreachableError extends Error {}

/**
 * The server can't be used until this browser signs in again: Cloudflare
 * Access redirected the request to its login, or the server refused the
 * missing or expired token. Nothing is lost; it is unreachable until then.
 */
export class LoginRequiredError extends UnreachableError {}

/** Gateway errors, which mean the server itself didn't answer. */
const GATEWAY_STATUSES = new Set([502, 503, 504, 520, 521, 522, 523, 524, 525, 526, 527, 530]);

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  let response: Response;
  try {
    // procnote's API never redirects; a redirect is Access sending us to log in.
    response = await fetch(`/api${path}`, { redirect: "manual", ...init });
  } catch (error) {
    throw new UnreachableError(String(error));
  }
  if (response.type === "opaqueredirect") throw new LoginRequiredError("sign in required");
  const body: unknown = await response.json().catch(() => null);
  if (!response.ok) {
    const serverMessage =
      body && typeof body === "object" && "error" in body && typeof body.error === "string"
        ? body.error
        : null;
    if (response.status === 401) throw new LoginRequiredError(serverMessage ?? "sign in required");
    // procnote-server explains its errors; a bare gateway error came from in front of it.
    if (serverMessage === null && GATEWAY_STATUSES.has(response.status)) {
      throw new UnreachableError(`HTTP ${response.status}`);
    }
    throw serverMessage ?? `HTTP ${response.status}`;
  }
  return body as T;
}

function postJson<T>(path: string, body: unknown): Promise<T> {
  return request(path, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
}

function desktopOnly(command: string): never {
  throw `${command} is only available in the desktop app`;
}

/** Who the server says the caller is. */
export interface Me {
  /** `"access"` when Cloudflare Access authenticates users, else `"none"`. */
  auth: "access" | "none";
  email: string | null;
}

/**
 * The signed-in user, if the server knows one. The desktop app and pages
 * outside the login (the phone upload page) get no email.
 */
export async function getMe(): Promise<Me> {
  if (isDesktop) return { auth: "none", email: null };
  try {
    return await request<Me>("/me");
  } catch {
    return { auth: "none", email: null };
  }
}

export async function listTemplates(): Promise<TemplateSummary[]> {
  if (isDesktop) return invoke("list_templates");
  return request("/templates");
}

export async function getTemplate(templatePath: string): Promise<ProcedureTemplate> {
  if (isDesktop) return invoke("get_template", { templatePath });
  return request(`/templates/content?${new URLSearchParams({ path: templatePath })}`);
}

/** A procedure's template: a workspace file, a shared (collaboratively edited) one, or both. */
export interface CatalogEntry {
  procedure_id: string;
  title: string;
  version: string;
  /** Path of the workspace's `template.md`, if there is one. */
  file: string | null;
  /** The shared template, if there is one. */
  shared: { id: string; updated_at: string } | null;
  /** Whether the shared template no longer describes what the workspace file does. */
  differs_from_file: boolean;
}

export interface TemplateCatalog {
  /** Whether templates can be edited together here (never in the desktop app). */
  collab: boolean;
  entries: CatalogEntry[];
}

/** Every procedure's template, one entry per procedure ID. */
export async function getTemplateCatalog(): Promise<TemplateCatalog> {
  if (!isDesktop) return request("/template-catalog");
  const files: TemplateSummary[] = await invoke("list_templates");
  const entries = files
    .map((file) => ({
      procedure_id: file.id,
      title: file.title,
      version: file.version,
      file: file.path,
      shared: null,
      differs_from_file: false,
    }))
    .sort((a, b) => a.title.localeCompare(b.title) || a.procedure_id.localeCompare(b.procedure_id));
  return { collab: false, entries };
}

/**
 * The shared template of the workspace template's procedure, created from
 * the file when there is none yet; returns its ID.
 */
export async function openSharedTemplate(templatePath: string): Promise<string> {
  if (isDesktop) desktopOnly("openSharedTemplate");
  const opened = await postJson<{ id: string }>("/collab/templates", {
    template_path: templatePath,
  });
  return opened.id;
}

/** A new shared template with no steps yet; returns its ID. */
export async function createBlankTemplate(procedureId: string, title: string): Promise<string> {
  if (isDesktop) desktopOnly("createBlankTemplate");
  const created = await postJson<{ id: string }>("/collab/templates/blank", {
    procedure_id: procedureId,
    title,
  });
  return created.id;
}

/** A copy of shared template `id` under a new procedure ID; returns the copy's ID. */
export async function duplicateTemplate(
  id: string,
  procedureId: string,
  title: string,
): Promise<string> {
  if (isDesktop) desktopOnly("duplicateTemplate");
  const created = await postJson<{ id: string }>(
    `/collab/templates/${encodeURIComponent(id)}/duplicate`,
    { procedure_id: procedureId, title },
  );
  return created.id;
}

/** Archive shared template `id`. Its procedure ID stays taken. */
export async function archiveTemplate(id: string): Promise<void> {
  if (isDesktop) desktopOnly("archiveTemplate");
  await request(`/collab/templates/${encodeURIComponent(id)}/archive`, { method: "POST" });
}

/** Replace shared template `id` with its procedure's workspace file. */
export async function resetTemplateFromFile(id: string): Promise<void> {
  if (isDesktop) desktopOnly("resetTemplateFromFile");
  await request(`/collab/templates/${encodeURIComponent(id)}/reset`, { method: "POST" });
}

/** Start an execution of shared template `id` as it is now. */
export async function startSharedExecution(
  id: string,
  meta: ActionMeta,
): Promise<ExecutionSummary> {
  if (isDesktop) desktopOnly("startSharedExecution");
  return postJson(`/collab/templates/${encodeURIComponent(id)}/executions`, { meta });
}

/** Shared template `id` as it is now, if it can be executed. */
export function getSharedTemplate(id: string): Promise<ProcedureTemplate> {
  return request(`/collab/templates/${encodeURIComponent(id)}/template`);
}

/** Shared template `id` as procnote Markdown (`template.md`). */
export async function getSharedTemplateMarkdown(id: string): Promise<string> {
  const response = await fetch(`/api/collab/templates/${encodeURIComponent(id)}/markdown`);
  if (!response.ok) {
    const body: unknown = await response.json().catch(() => null);
    throw body && typeof body === "object" && "error" in body && typeof body.error === "string"
      ? body.error
      : `HTTP ${response.status}`;
  }
  return response.text();
}

/** A short-lived token for editing template `id` through the collab service. */
export async function getCollabToken(id: string): Promise<string> {
  const { token } = await postJson<{ token: string }>(
    `/collab/templates/${encodeURIComponent(id)}/token`,
    {},
  );
  return token;
}

export async function startExecution(
  templatePath: string,
  meta: ActionMeta,
): Promise<ExecutionSummary> {
  if (isDesktop) return invoke("start_execution", { templatePath, meta });
  return postJson("/executions", { template_path: templatePath, meta });
}

export async function recordAction(
  executionId: string,
  action: ExecutionAction,
  meta: ActionMeta,
): Promise<ExecutionSummary> {
  if (isDesktop) return invoke("record_action", { executionId, action, meta });
  return postJson(`/executions/${encodeURIComponent(executionId)}/actions`, { action, meta });
}

export async function getExecutionState(executionId: string): Promise<ExecutionSummary> {
  if (isDesktop) return invoke("get_execution_state", { executionId });
  return request(`/executions/${encodeURIComponent(executionId)}`);
}

export async function listExecutions(): Promise<ExecutionSummary[]> {
  if (isDesktop) return invoke("list_executions");
  return request("/executions");
}

/**
 * Call `onSeq` with the execution's event count now and after every change
 * recorded through the server, reconnecting as needed. Returns a function
 * that stops watching. The desktop app has a single writer, so it does not
 * watch.
 */
/** How often the desktop app looks for changes a background sync brought in. */
const DESKTOP_WATCH_INTERVAL_MS = 5000;

export function watchExecution(
  executionId: string,
  onSeq: (seq: number) => void,
  onSummary?: (summary: ExecutionSummary) => void,
): () => void {
  if (isDesktop) {
    // The desktop app's own writes return their summary; this picks up what
    // syncing with a server pulled in.
    let last = "";
    const timer = window.setInterval(() => {
      getExecutionState(executionId).then(
        (summary) => {
          const json = JSON.stringify(summary);
          if (last && json !== last) onSummary?.(summary);
          last = json;
        },
        () => undefined,
      );
    }, DESKTOP_WATCH_INTERVAL_MS);
    return () => window.clearInterval(timer);
  }
  let socket: WebSocket | null = null;
  let retryTimer: number | undefined;
  let retryDelayMs = 1000;
  let stopped = false;

  const connect = () => {
    const scheme = window.location.protocol === "https:" ? "wss" : "ws";
    socket = new WebSocket(
      `${scheme}://${window.location.host}/api/executions/${encodeURIComponent(executionId)}/live`,
    );
    socket.addEventListener("open", () => {
      retryDelayMs = 1000;
    });
    socket.addEventListener("message", (event) => {
      const data: unknown = JSON.parse(String(event.data));
      if (data && typeof data === "object" && "seq" in data && typeof data.seq === "number") {
        onSeq(data.seq);
      }
    });
    socket.addEventListener("close", () => {
      if (stopped) return;
      retryTimer = window.setTimeout(connect, retryDelayMs);
      retryDelayMs = Math.min(retryDelayMs * 2, 10_000);
    });
  };

  connect();
  return () => {
    stopped = true;
    window.clearTimeout(retryTimer);
    socket?.close();
  };
}

// --- Upload links (browser only; the desktop app uses DropPoint) ---

export async function getUploadLink(executionId: string): Promise<string | null> {
  const { token } = await request<{ token: string | null }>(
    `/executions/${encodeURIComponent(executionId)}/upload-link`,
  );
  return token;
}

/**
 * The execution's phone upload link, created if it has none, as the URL a
 * phone opens. The desktop app gets it from the server it syncs with.
 */
export async function createUploadLink(executionId: string): Promise<string> {
  if (isDesktop) return invoke("create_upload_link", { executionId });
  const { token } = await postJson<{ token: string }>(
    `/executions/${encodeURIComponent(executionId)}/upload-link`,
    {},
  );
  return `${window.location.origin}/upload/${token}`;
}

export async function revokeUploadLink(executionId: string): Promise<void> {
  if (isDesktop) return invoke("revoke_upload_link", { executionId });
  await request(`/executions/${encodeURIComponent(executionId)}/upload-link`, {
    method: "DELETE",
  });
}

export interface UploadTarget {
  name: string;
  procedure_title: string;
  active: boolean;
}

/** What an upload link points at, as a phone may see it. */
export function getUploadTarget(token: string): Promise<UploadTarget> {
  return request(`/upload-links/${encodeURIComponent(token)}`);
}

/** Send files to an execution's inbox through its upload link. */
export function uploadToLink(
  token: string,
  files: File[],
  meta: ActionMeta,
): Promise<{ received: number; skipped: number }> {
  const form = new FormData();
  if (meta.recorded_by) form.append("recorded_by", meta.recorded_by);
  if (meta.device) form.append("device", meta.device);
  for (const file of files) form.append("file", file, file.name);
  return request(`/upload-links/${encodeURIComponent(token)}/files`, {
    method: "POST",
    body: form,
  });
}

// --- Device tokens for desktop apps (browser only) ---

export interface DeviceToken {
  id: string;
  email: string;
  device: string;
  /** ISO 8601 timestamp. */
  created_at: string;
}

/** Issue a token for a desktop app to sync as the signed-in user. */
export function issueDeviceToken(device: string): Promise<DeviceToken & { token: string }> {
  return postJson("/device-tokens", { device });
}

export function listDeviceTokens(): Promise<DeviceToken[]> {
  return request("/device-tokens");
}

export async function revokeDeviceToken(id: string): Promise<void> {
  const response = await fetch(`/api/device-tokens/${encodeURIComponent(id)}`, {
    method: "DELETE",
  });
  if (!response.ok) throw `HTTP ${response.status}`;
}

// --- Syncing the desktop app with a server (desktop only) ---

export function getSyncStatus(): Promise<SyncStatus> {
  return invoke("get_sync_status");
}

/** Connect the workspace: the browser opens the server's connect page, and this resolves once confirmed there. */
export function connectServer(server: string, device: string): Promise<SyncStatus> {
  return invoke("connect_server", { server, device });
}

export function disconnectServer(): Promise<SyncStatus> {
  return invoke("disconnect_server");
}

export function syncNow(): Promise<SyncStatus> {
  return invoke("sync_now");
}

/** Whether an execution syncs with the server (null when it doesn't; true once the server has it). */
export async function executionLink(executionId: string): Promise<boolean | null> {
  if (!isDesktop) return null;
  return invoke("execution_link", { executionId });
}

/** The server's executions not kept in this workspace yet. */
export function listServerExecutions(): Promise<ExecutionSummary[]> {
  return invoke("list_server_executions");
}

/** Keep a copy of a server execution in this workspace; it syncs from then on. */
export function openServerExecution(executionId: string): Promise<ExecutionSummary> {
  return invoke("open_server_execution", { executionId });
}

/** Events of a linked execution the server refused as malformed. */
export function listRejected(
  executionId: string,
): Promise<{ record: EventRecord; reason: string }[]> {
  return invoke("list_rejected", { executionId });
}

// --- Sync for offline recording (browser only) ---

/** Log lines of an execution from position `after` on, and how many there are. */
export function pullEvents(
  executionId: string,
  after: number,
): Promise<{ records: EventRecord[]; count: number }> {
  return request(`/executions/${encodeURIComponent(executionId)}/events?after=${after}`);
}

export type PushResult = PushOutcome & { event_id: string | null };

/** Push log lines recorded on this device, with upload tokens for their attachment content. */
export function pushEvents(
  executionId: string,
  events: EventRecord[],
  uploads: string[],
): Promise<{ results: PushResult[] }> {
  return postJson(`/executions/${encodeURIComponent(executionId)}/events`, { events, uploads });
}

/** Register an execution this device started itself. */
export function registerExecution(
  executionId: string,
  procedure: string,
  template: ProcedureTemplate,
  events: EventRecord[],
): Promise<ExecutionSummary> {
  return request(`/executions/${encodeURIComponent(executionId)}`, {
    method: "PUT",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ procedure, template, events }),
  });
}

/** Events that reached the server after the execution had moved on. */
export function listConflicts(executionId: string): Promise<Conflict[]> {
  if (isDesktop) return invoke("list_conflicts", { executionId });
  return request(`/executions/${encodeURIComponent(executionId)}/conflicts`);
}

export function resolveConflict(
  executionId: string,
  eventId: string,
  meta: ActionMeta,
): Promise<Conflict[]> {
  if (isDesktop) return invoke("resolve_conflict", { executionId, eventId });
  return postJson(
    `/executions/${encodeURIComponent(executionId)}/conflicts/${encodeURIComponent(eventId)}/resolve`,
    { meta },
  );
}

export async function getAttachmentPreviewDataUrl(
  executionId: string,
  path: string,
): Promise<string | null> {
  if (isDesktop) return invoke("get_attachment_preview_data_url", { executionId, path });
  const query = new URLSearchParams({ path });
  return request(`/executions/${encodeURIComponent(executionId)}/attachment-preview?${query}`);
}

/**
 * Let the user choose files to attach.
 *
 * In a browser the chosen files are uploaded right away, and the returned
 * sources reference them by upload token instead of a filesystem path.
 */
/** Upload files to the server; their sources carry single-use upload tokens as paths. */
export function uploadFiles(files: { name: string; blob: Blob }[]): Promise<AttachmentSource[]> {
  const form = new FormData();
  for (const file of files) form.append("file", file.blob, file.name);
  return request("/uploads", { method: "POST", body: form });
}

export async function pickAttachmentSources(title: string): Promise<AttachmentSource[]> {
  if (isDesktop) return invoke("pick_attachment_sources", { title });
  const files = await chooseBrowserFiles();
  if (files.length === 0) return [];
  const form = new FormData();
  for (const file of files) form.append("file", file, file.name);
  return request("/uploads", { method: "POST", body: form });
}

/** Let the user choose files with the browser's file dialog. */
export function chooseBrowserFiles(): Promise<File[]> {
  return new Promise((resolve) => {
    const input = document.createElement("input");
    input.type = "file";
    input.multiple = true;
    input.addEventListener("change", () => resolve([...(input.files ?? [])]));
    input.addEventListener("cancel", () => resolve([]));
    input.click();
  });
}

export async function revealExecutionDir(executionId: string): Promise<void> {
  if (isDesktop) return invoke("reveal_execution_dir", { executionId });
  desktopOnly("Revealing the execution directory");
}

export async function isDropPointConfigured(): Promise<boolean> {
  if (isDesktop) return invoke("is_drop_point_configured");
  return false;
}

export async function startAttachmentDropPointSession(
  executionId: string,
  stepId: string,
  inputId: string,
): Promise<AttachmentDropPointSessionSummary> {
  if (!isDesktop) desktopOnly("DropPoint");
  return invoke("start_attachment_drop_point_session", { executionId, stepId, inputId });
}

export async function pollAttachmentDropPointSession(
  sessionId: string,
): Promise<AttachmentDropPointStatus> {
  if (!isDesktop) desktopOnly("DropPoint");
  return invoke("poll_attachment_drop_point_session", { sessionId });
}

export async function importAttachmentDropPointUpload(
  executionId: string,
  stepId: string,
  inputId: string,
  sessionId: string,
): Promise<ExecutionSummary> {
  if (!isDesktop) desktopOnly("DropPoint");
  return invoke("import_attachment_drop_point_upload", {
    executionId,
    stepId,
    inputId,
    sessionId,
  });
}

export async function cancelAttachmentDropPointSession(sessionId: string): Promise<void> {
  if (!isDesktop) desktopOnly("DropPoint");
  return invoke("cancel_attachment_drop_point_session", { sessionId });
}
