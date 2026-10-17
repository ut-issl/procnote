import { invoke } from "@tauri-apps/api/core";
import { isDesktop } from "#lib/platform.js";
import type {
  ActionMeta,
  AttachmentDropPointSessionSummary,
  AttachmentDropPointStatus,
  AttachmentSource,
  ExecutionAction,
  ExecutionSummary,
  ProcedureTemplate,
  TemplateSummary,
} from "#lib/types/index.js";

// In the desktop app commands go over Tauri IPC; in a browser they go to the
// procnote-server HTTP API on the same origin. Both reject with a string
// message on failure.

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const response = await fetch(`/api${path}`, init);
  const body: unknown = await response.json().catch(() => null);
  if (!response.ok) {
    const message =
      body && typeof body === "object" && "error" in body && typeof body.error === "string"
        ? body.error
        : `HTTP ${response.status}`;
    throw message;
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

export async function listTemplates(): Promise<TemplateSummary[]> {
  if (isDesktop) return invoke("list_templates");
  return request("/templates");
}

export async function getTemplate(templatePath: string): Promise<ProcedureTemplate> {
  if (isDesktop) return invoke("get_template", { templatePath });
  return request(`/templates/content?${new URLSearchParams({ path: templatePath })}`);
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
export function watchExecution(executionId: string, onSeq: (seq: number) => void): () => void {
  if (isDesktop) return () => {};
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

export async function createUploadLink(executionId: string): Promise<string> {
  const { token } = await postJson<{ token: string }>(
    `/executions/${encodeURIComponent(executionId)}/upload-link`,
    {},
  );
  return token;
}

export async function revokeUploadLink(executionId: string): Promise<void> {
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
export async function pickAttachmentSources(title: string): Promise<AttachmentSource[]> {
  if (isDesktop) return invoke("pick_attachment_sources", { title });
  const files = await chooseBrowserFiles();
  if (files.length === 0) return [];
  const form = new FormData();
  for (const file of files) form.append("file", file, file.name);
  return request("/uploads", { method: "POST", body: form });
}

function chooseBrowserFiles(): Promise<File[]> {
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
