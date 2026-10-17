// Pure helpers for the focused run view. Step completion is derived from the
// recorded state (there is no "step completed" event): a present step is
// complete when every checkbox is checked and every input has a value or an
// attachment.
import type { ExpectedValue, InputDefinitionSummary, StepSummary } from "#lib/types/index.js";

export interface StepProgress {
  checkedBoxes: number;
  totalBoxes: number;
  filledInputs: number;
  totalInputs: number;
  /** Checkboxes and inputs still to do. */
  remaining: number;
  complete: boolean;
}

export function stepInputs(step: StepSummary): InputDefinitionSummary[] {
  return step.content.flatMap((block) => (block.type === "InputBlock" ? block.inputs : []));
}

export function isInputFilled(input: InputDefinitionSummary): boolean {
  return input.definition.type === "attachment" ? input.attachments.length > 0 : !!input.recorded;
}

export function stepProgress(step: StepSummary): StepProgress {
  const boxes = step.content.filter((block) => block.type === "Checkbox");
  const inputs = stepInputs(step);
  const checkedBoxes = boxes.filter((box) => box.checked).length;
  const filledInputs = inputs.filter(isInputFilled).length;
  const remaining = boxes.length - checkedBoxes + (inputs.length - filledInputs);
  return {
    checkedBoxes,
    totalBoxes: boxes.length,
    filledInputs,
    totalInputs: inputs.length,
    remaining,
    complete: remaining === 0,
  };
}

/** Whether a step still needs work: present and not complete. */
export function isOpen(step: StepSummary): boolean {
  return step.status === "present" && !stepProgress(step).complete;
}

/** Whether a recorded value falls outside the expected value. */
export function isOutOfRange(expected: ExpectedValue | undefined, value: string): boolean {
  if (expected === undefined || value.trim() === "") return false;
  if (typeof expected === "string") return value.trim() !== expected;
  const n = Number(value);
  return !Number.isFinite(n) || n < expected.min || n > expected.max;
}

/** The step to focus by default: the first open step, else the last step. */
export function defaultFocus(steps: StepSummary[]): string | undefined {
  return (steps.find(isOpen) ?? steps.at(-1))?.id;
}

/** The step before `stepId` in order, if any. */
export function previousStep(steps: StepSummary[], stepId: string): string | undefined {
  const index = steps.findIndex((s) => s.id === stepId);
  return index > 0 ? steps[index - 1]?.id : undefined;
}

/**
 * Where "next" goes from `stepId`: the next open step after it, wrapping to
 * earlier open steps, else simply the following step.
 */
export function nextStep(steps: StepSummary[], stepId: string): string | undefined {
  const index = steps.findIndex((s) => s.id === stepId);
  const after = steps.slice(index + 1);
  const before = steps.slice(0, Math.max(index, 0));
  return (after.find(isOpen) ?? before.find(isOpen) ?? after[0])?.id;
}

export interface RunProgress {
  complete: number;
  skipped: number;
  total: number;
}

export function runProgress(steps: StepSummary[]): RunProgress {
  return {
    complete: steps.filter((s) => s.status === "present" && stepProgress(s).complete).length,
    skipped: steps.filter((s) => s.status === "skipped").length,
    total: steps.length,
  };
}
