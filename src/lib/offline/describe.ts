// A recorded event in words, for showing conflicts: what the operator did,
// with step, checkbox, and input names taken from the execution.
import type { EventRecord, ExecutionSummary } from "#lib/types/index.js";

function stepHeading(summary: ExecutionSummary, stepId: string): string {
  return summary.steps.find((step) => step.id === stepId)?.heading ?? stepId;
}

function checkboxText(summary: ExecutionSummary, checkboxId: string): string {
  for (const step of summary.steps) {
    for (const item of step.content) {
      if (item.type === "Checkbox" && item.id === checkboxId) return item.text;
    }
  }
  return checkboxId;
}

function inputLabel(summary: ExecutionSummary, stepId: string, inputId: string): string {
  const step = summary.steps.find((s) => s.id === stepId);
  for (const item of step?.content ?? []) {
    if (item.type !== "InputBlock") continue;
    const input = item.inputs.find((i) => i.definition.id === inputId);
    if (input) return input.definition.label;
  }
  return inputId;
}

export function describeEvent(record: EventRecord, summary: ExecutionSummary): string {
  switch (record.type) {
    case "checkbox_toggled":
      return `${record.checked ? "Checked" : "Unchecked"} “${checkboxText(summary, record.checkbox_id)}”`;
    case "input_recorded":
      return `Recorded ${inputLabel(summary, record.step_id, record.input_id)}: ${record.value}${record.unit ? ` ${record.unit}` : ""}`;
    case "input_cleared":
      return `Cleared ${inputLabel(summary, record.step_id, record.input_id)} (${record.reason})`;
    case "step_skipped":
      return `Skipped “${stepHeading(summary, record.step_id)}” (${record.reason})`;
    case "step_unskipped":
      return `Unskipped “${stepHeading(summary, record.step_id)}”`;
    case "step_added":
      return `Added step “${record.heading}”`;
    case "note_added":
      return `Note: ${record.text}`;
    case "note_removed":
      return `Removed a note (${record.reason})`;
    case "attachments_added":
      return `Attached ${record.attachments.map((a) => a.filename).join(", ")} to ${inputLabel(summary, record.step_id, record.input_id)}`;
    case "execution_completed":
      return `Completed as ${record.status}`;
    case "execution_aborted":
      return `Aborted (${record.reason})`;
    case "execution_reopened":
      return `Reopened (${record.reason})`;
    case "execution_renamed":
      return `Renamed to “${record.name}”`;
    default:
      return record.type.replaceAll("_", " ");
  }
}
