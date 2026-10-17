import { expect, test } from "vite-plus/test";
import type { ExecutionSummary, InputDefinitionSummary, StepSummary } from "#lib/types/index.js";
import { assignTargets, isLoopbackHost } from "./inbox.js";

const input = (id: string, type: "attachment" | "measurement"): InputDefinitionSummary => ({
  definition: { id, label: `Label ${id}`, type, options: [] },
  attachments: [],
});

const step = (id: string, status: string, inputs: InputDefinitionSummary[]): StepSummary => ({
  id,
  heading: `Heading ${id}`,
  status,
  notes: [],
  content: [
    { type: "Prose", text: "x" },
    { type: "InputBlock", inputs },
  ],
});

test("assignTargets lists attachment inputs of present steps", () => {
  const summary = {
    steps: [
      step("s1", "present", [input("photo", "attachment"), input("volts", "measurement")]),
      step("s2", "skipped", [input("skipped-photo", "attachment")]),
      step("s3", "present", [input("log", "attachment")]),
    ],
  } as ExecutionSummary;

  expect(assignTargets(summary)).toEqual([
    { step_id: "s1", input_id: "photo", label: "Heading s1 — Label photo" },
    { step_id: "s3", input_id: "log", label: "Heading s3 — Label log" },
  ]);
});

test.each([
  ["localhost", true],
  ["127.0.0.1", true],
  ["[::1]", true],
  ["192.168.1.20", false],
  ["procnote.local", false],
])("isLoopbackHost(%s) is %s", (host, expected) => {
  expect(isLoopbackHost(host)).toBe(expected);
});
