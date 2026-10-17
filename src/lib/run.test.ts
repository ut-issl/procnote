import { describe, expect, test } from "vite-plus/test";
import type { InputDefinitionSummary, StepSummary } from "#lib/types/index.js";
import {
  defaultFocus,
  isOutOfRange,
  nextStep,
  previousStep,
  runProgress,
  stepProgress,
} from "./run.js";

function input(
  id: string,
  overrides: Partial<InputDefinitionSummary> & { type?: "measurement" | "attachment" } = {},
): InputDefinitionSummary {
  const { type = "measurement", ...rest } = overrides;
  return { definition: { id, label: id, type, options: [] }, attachments: [], ...rest };
}

function step(
  id: string,
  { checked = [] as boolean[], inputs = [] as InputDefinitionSummary[], status = "present" } = {},
): StepSummary {
  return {
    id,
    heading: id,
    status,
    notes: [],
    content: [
      ...checked.map((c, i) => ({
        type: "Checkbox" as const,
        id: `${id}/cb-${i}`,
        text: "item",
        checked: c,
        nesting_level: 0,
      })),
      ...(inputs.length ? [{ type: "InputBlock" as const, inputs }] : []),
    ],
  };
}

const recorded = { label: "v", value: "1" };
const attachment = {
  filename: "a.png",
  path: "attachments/a",
  content_type: "image/png",
  sha256: "x",
};

describe("stepProgress", () => {
  test("counts checkboxes, recorded values, and attachments", () => {
    const progress = stepProgress(
      step("s", {
        checked: [true, false],
        inputs: [
          input("v", { recorded }),
          input("w"),
          input("photo", { type: "attachment", attachments: [attachment] }),
        ],
      }),
    );

    expect(progress).toEqual({
      checkedBoxes: 1,
      totalBoxes: 2,
      filledInputs: 2,
      totalInputs: 3,
      remaining: 2,
      complete: false,
    });
  });

  test("a step with nothing to do is complete", () => {
    expect(stepProgress(step("s")).complete).toBe(true);
  });
});

test.each([
  [undefined, "5", false],
  [{ min: 100, max: 150 }, "120", false],
  [{ min: 100, max: 150 }, "151", true],
  [{ min: 100, max: 150 }, "abc", true],
  [{ min: 100, max: 150 }, "", false],
  ["PASS", "PASS", false],
  ["PASS", "FAIL", true],
])("isOutOfRange(%j, %j) is %s", (expected, value, out) => {
  expect(isOutOfRange(expected, value)).toBe(out);
});

describe("navigation", () => {
  const steps = [
    step("done", { checked: [true] }),
    step("open", { checked: [false] }),
    step("skipped", { checked: [false], status: "skipped" }),
    step("later", { checked: [false] }),
  ];

  test("defaultFocus picks the first open step", () => {
    expect(defaultFocus(steps)).toBe("open");
  });

  test("defaultFocus falls back to the last step when all are done", () => {
    expect(defaultFocus([step("a"), step("b")])).toBe("b");
  });

  test("nextStep skips completed and skipped steps", () => {
    expect(nextStep(steps, "open")).toBe("later");
  });

  test("nextStep wraps to earlier open steps", () => {
    expect(nextStep(steps, "later")).toBe("open");
  });

  test("nextStep moves to the following step when nothing is open", () => {
    expect(nextStep([step("a"), step("b")], "a")).toBe("b");
    expect(nextStep([step("a"), step("b")], "b")).toBeUndefined();
  });

  test("previousStep goes back one step in order", () => {
    expect(previousStep(steps, "skipped")).toBe("open");
    expect(previousStep(steps, "done")).toBeUndefined();
  });
});

test("runProgress counts complete and skipped steps", () => {
  expect(
    runProgress([step("a"), step("b", { checked: [false] }), step("c", { status: "skipped" })]),
  ).toEqual({ complete: 1, skipped: 1, total: 3 });
});
