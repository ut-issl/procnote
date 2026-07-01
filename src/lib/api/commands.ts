import { invoke } from "@tauri-apps/api/core";
import type {
  AttachmentDropPointSessionSummary,
  AttachmentDropPointStatus,
  ExecutionAction,
  ExecutionSummary,
  ProcedureTemplate,
  TemplateSummary,
} from "$lib/types";

export async function listTemplates(): Promise<TemplateSummary[]> {
  return invoke("list_templates");
}

export async function loadTemplate(path: string): Promise<ProcedureTemplate> {
  return invoke("load_template", { path });
}

export async function startExecution(templatePath: string): Promise<ExecutionSummary> {
  return invoke("start_execution", { templatePath });
}

export async function recordAction(
  executionId: string,
  action: ExecutionAction,
): Promise<ExecutionSummary> {
  return invoke("record_action", { executionId, action });
}

export async function getExecutionState(executionId: string): Promise<ExecutionSummary> {
  return invoke("get_execution_state", { executionId });
}

export async function listExecutions(): Promise<ExecutionSummary[]> {
  return invoke("list_executions");
}

export async function isDropPointConfigured(): Promise<boolean> {
  return invoke("is_drop_point_configured");
}

export async function startAttachmentDropPointSession(
  executionId: string,
  stepId: string,
  inputId: string,
): Promise<AttachmentDropPointSessionSummary> {
  return invoke("start_attachment_drop_point_session", { executionId, stepId, inputId });
}

export async function pollAttachmentDropPointSession(
  sessionId: string,
): Promise<AttachmentDropPointStatus> {
  return invoke("poll_attachment_drop_point_session", { sessionId });
}

export async function importAttachmentDropPointUpload(
  executionId: string,
  stepId: string,
  inputId: string,
  sessionId: string,
): Promise<ExecutionSummary> {
  return invoke("import_attachment_drop_point_upload", {
    executionId,
    stepId,
    inputId,
    sessionId,
  });
}

export async function cancelAttachmentDropPointSession(sessionId: string): Promise<void> {
  return invoke("cancel_attachment_drop_point_session", { sessionId });
}
