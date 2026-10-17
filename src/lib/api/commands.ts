import { invoke } from "@tauri-apps/api/core";
import { isDesktop } from "#lib/platform.js";
import type {
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

export async function startExecution(templatePath: string): Promise<ExecutionSummary> {
  if (isDesktop) return invoke("start_execution", { templatePath });
  return postJson("/executions", { template_path: templatePath });
}

export async function recordAction(
  executionId: string,
  action: ExecutionAction,
): Promise<ExecutionSummary> {
  if (isDesktop) return invoke("record_action", { executionId, action });
  return postJson(`/executions/${encodeURIComponent(executionId)}/actions`, action);
}

export async function getExecutionState(executionId: string): Promise<ExecutionSummary> {
  if (isDesktop) return invoke("get_execution_state", { executionId });
  return request(`/executions/${encodeURIComponent(executionId)}`);
}

export async function listExecutions(): Promise<ExecutionSummary[]> {
  if (isDesktop) return invoke("list_executions");
  return request("/executions");
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
