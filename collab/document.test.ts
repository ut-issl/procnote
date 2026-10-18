import { describe, expect, it } from "vite-plus/test";
import * as Y from "yjs";
import { diffPaths } from "../src/lib/blocknote/convert.js";
import type { ProcedureTemplate } from "../src/lib/types/index.js";
import {
  createEditor,
  documentToTemplate,
  isEmptyDocument,
  templateToUpdate,
  withAsyncMarkdown,
} from "./document.js";

const TEMPLATE: ProcedureTemplate = {
  metadata: {
    id: "DOC-001",
    title: "Document Test",
    version: "1.0",
    equipment: [],
    requirement_traces: ["REQ-1"],
  },
  steps: [
    {
      heading: "Prepare",
      content: [
        { type: "Prose", text: "Check the **power** supply." },
        { type: "Checkbox", text: "Power on", checked: false },
        { type: "Checkbox", text: "Wait", checked: false, nesting_level: 1 },
      ],
    },
    {
      heading: "Measure",
      content: [
        {
          type: "InputBlock",
          inputs: [
            {
              id: "voltage",
              label: "Voltage",
              type: "measurement",
              unit: "V",
              expected: { min: 1, max: 2 },
              options: [],
            },
            {
              id: "result",
              label: "Result",
              type: "selection",
              options: ["PASS", "FAIL"],
            },
          ],
        },
      ],
    },
  ],
};

describe("template documents", () => {
  it("round-trip a template through a Y.Doc", async () => {
    const editor = createEditor();
    const doc = new Y.Doc();
    expect(isEmptyDocument(doc)).toBe(true);

    Y.applyUpdate(doc, await templateToUpdate(editor, TEMPLATE));
    const { template, issues } = await documentToTemplate(editor, doc);

    expect(isEmptyDocument(doc)).toBe(false);
    expect(issues).toEqual([]);
    expect(diffPaths(TEMPLATE, template)).toEqual([]);
  });
});

describe("withAsyncMarkdown", () => {
  it("gives the second pass the asynchronous results", async () => {
    const result = await withAsyncMarkdown(
      {
        toBlocks: async (text) => [{ type: "paragraph", content: text.toUpperCase() }],
        fromBlocks: async (blocks) => blocks.map((b) => String(b.content)).join("|"),
      },
      (md) => [md.toBlocks("a"), md.fromBlocks([{ type: "paragraph", content: "x" }])],
    );

    expect(result).toEqual([[{ type: "paragraph", content: "A" }], "x"]);
  });
});
