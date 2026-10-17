import { describe, expect, test } from "vite-plus/test";
import type { EventRecord, ExecutionSummary } from "#lib/types/index.js";
import { describeEvent } from "./describe.js";

const summary: ExecutionSummary = {
  execution_id: "e",
  procedure_id: "p",
  procedure_title: "P",
  procedure_version: "1",
  status: "active",
  inbox: [],
  seq: 3,
  steps: [
    {
      id: "step-0",
      heading: "Measure",
      status: "present",
      notes: [],
      content: [
        { type: "Checkbox", id: "step-0/cb-0", text: "Power on", checked: false, nesting_level: 0 },
        {
          type: "InputBlock",
          inputs: [
            {
              definition: { id: "v", label: "Voltage", type: "measurement", options: [] },
              attachments: [],
            },
          ],
        },
      ],
    },
  ],
};

const at = "2026-10-18T00:00:00Z";
const execution_id = "e";

describe("describeEvent", () => {
  test.each<[EventRecord, string]>([
    [
      {
        type: "checkbox_toggled",
        at,
        execution_id,
        step_id: "step-0",
        checkbox_id: "step-0/cb-0",
        checked: true,
      },
      "Checked “Power on”",
    ],
    [
      {
        type: "input_recorded",
        at,
        execution_id,
        step_id: "step-0",
        input_id: "v",
        value: "5.1",
        unit: "V",
      },
      "Recorded Voltage: 5.1 V",
    ],
    [
      { type: "step_skipped", at, execution_id, step_id: "step-0", reason: "n/a" },
      "Skipped “Measure” (n/a)",
    ],
    [
      {
        type: "checkbox_toggled",
        at,
        execution_id,
        step_id: "gone",
        checkbox_id: "gone/cb-9",
        checked: false,
      },
      "Unchecked “gone/cb-9”",
    ],
    [
      { type: "attachments_cleared", at, execution_id, step_id: "step-0", input_id: "v" },
      "attachments cleared",
    ],
  ])("%#", (record, text) => {
    expect(describeEvent(record, summary)).toBe(text);
  });
});
