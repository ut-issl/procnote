import { readFileSync } from "node:fs";
import { beforeAll, describe, expect, test } from "vite-plus/test";
import type { EventRecord, ProcedureTemplate } from "#lib/types/index.js";
import { attachmentRecord, decide, loadCoreFrom, start, summarize } from "./core.js";

const TEMPLATE: ProcedureTemplate = {
  metadata: { id: "W-1", title: "Wasm", version: "1", equipment: [], requirement_traces: [] },
  steps: [
    {
      heading: "Only step",
      content: [
        { type: "Checkbox", text: "Ready", checked: false },
        {
          type: "InputBlock",
          inputs: [{ id: "photo", label: "Photo", type: "attachment", options: [] }],
        },
      ],
    },
  ],
};

beforeAll(() => {
  loadCoreFrom(readFileSync(new URL("./pkg/procnote_wasm_bg.wasm", import.meta.url)));
});

describe("the execution core in WebAssembly", () => {
  test("runs an execution as the server would", () => {
    const log: EventRecord[] = start(TEMPLATE);
    log.push(
      ...decide(log, {
        action: "toggle_checkbox",
        step_id: "step-0",
        checkbox_id: "step-0/cb-0",
        checked: true,
      }),
    );
    const photo = attachmentRecord("bench photo.jpg", "a".repeat(64));
    log.push(
      ...decide(
        log,
        { action: "add_attachments", step_id: "step-0", input_id: "photo", files: [] },
        [photo],
      ),
    );

    const summary = summarize(log);

    expect(summary.procedure_id).toBe("W-1");
    expect(summary.steps[0]?.content[0]).toMatchObject({ type: "Checkbox", checked: true });
    expect(photo).toEqual({
      filename: "bench photo.jpg",
      path: "attachments/aaaaaaa-bench photo.jpg",
      content_type: "image/jpeg",
      sha256: "a".repeat(64),
    });
    expect(summary.steps[0]?.content[1]).toMatchObject({
      type: "InputBlock",
      inputs: [{ attachments: [{ path: photo.path }] }],
    });
  });

  test("refuses actions the state machine doesn't allow", () => {
    const log: EventRecord[] = start(TEMPLATE);

    expect(() => decide(log, { action: "skip_step", step_id: "missing", reason: "x" })).toThrow();
  });
});
