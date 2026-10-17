import { lexer, type Tokens } from "marked";
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

describe("tables in prose", () => {
  it("keep their column alignment through a Y.Doc", async () => {
    const editor = createEditor();
    const doc = new Y.Doc();
    const table = "| a | b | c |\n| :-- | --: | :-: |\n| 1 | 2 | 3 |";
    const template: ProcedureTemplate = {
      ...TEMPLATE,
      steps: [{ heading: "Table", content: [{ type: "Prose", text: table }] }],
    };

    Y.applyUpdate(doc, await templateToUpdate(editor, template));
    const { template: back } = await documentToTemplate(editor, doc);

    const prose = back.steps[0]?.content[0];
    const tables = lexer(prose?.type === "Prose" ? prose.text : "").filter(
      (token): token is Tokens.Table => token.type === "table",
    );
    expect(tables.map((token) => token.align)).toEqual([[null, "right", "center"]]);
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
