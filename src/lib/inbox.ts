import type { ExecutionSummary } from "#lib/types/index.js";

export interface AssignTarget {
  step_id: string;
  input_id: string;
  /** "Step heading — input label", for a picker. */
  label: string;
}

/** Attachment inputs that inbox files can be assigned to, in step order. */
export function assignTargets(summary: ExecutionSummary): AssignTarget[] {
  return summary.steps
    .filter((step) => step.status === "present")
    .flatMap((step) =>
      step.content.flatMap((block) =>
        block.type === "InputBlock"
          ? block.inputs
              .filter((input) => input.definition.type === "attachment")
              .map((input) => ({
                step_id: step.id,
                input_id: input.definition.id,
                label: `${step.heading} — ${input.definition.label}`,
              }))
          : [],
      ),
    );
}

/** Whether the page is served from an address other devices cannot reach. */
export function isLoopbackHost(hostname: string): boolean {
  return (
    hostname === "localhost" ||
    hostname === "::1" ||
    hostname === "[::1]" ||
    hostname.startsWith("127.")
  );
}
