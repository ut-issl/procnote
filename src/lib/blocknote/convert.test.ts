import { describe, expect, test } from "vite-plus/test";
import type { ProcedureMetadata, ProcedureTemplate } from "#lib/types/index.js";
import {
  blocksToTemplate,
  diffPaths,
  type DocBlock,
  enclosingStep,
  inlineText,
  inputToProps,
  type MarkdownConverter,
  metadataIssues,
  propsToInput,
  tableMarkdown,
  templateToBlocks,
} from "./convert.js";

// Stand-in for BlockNote's Markdown conversion: one paragraph per prose item.
const md: MarkdownConverter = {
  toBlocks: (markdown) => [{ type: "paragraph", content: markdown }],
  fromBlocks: (blocks) => blocks.map((block) => inlineText(block.content)).join("\n\n"),
};

const metadata: ProcedureMetadata = {
  id: "TVT-001",
  title: "Thermal Vacuum Test",
  version: "1.0",
  equipment: [],
  requirement_traces: [],
};

const template: ProcedureTemplate = {
  metadata,
  steps: [
    {
      heading: "Preconditions",
      content: [
        { type: "Checkbox", text: "Chamber pressure < 1e-5 Pa", checked: false },
        { type: "Checkbox", text: "EGSE connected", checked: true },
      ],
    },
    {
      heading: "Step 1: Power On",
      content: [
        { type: "Prose", text: "Connect PSU to DUT J1 connector." },
        { type: "Checkbox", text: "a checkbox item", checked: false },
        { type: "Checkbox", text: "a nested checkbox item", checked: false, nesting_level: 1 },
        { type: "Checkbox", text: "deeper", checked: false, nesting_level: 2 },
        { type: "Checkbox", text: "back at the top", checked: false },
        {
          type: "InputBlock",
          inputs: [
            {
              id: "current-draw",
              label: "Measure current draw",
              type: "measurement",
              unit: "mA",
              options: [],
              expected: { min: 100, max: 150 },
            },
          ],
        },
        { type: "Prose", text: "text also here" },
      ],
    },
    {
      heading: "Step 2: Functional Check",
      content: [
        {
          type: "InputBlock",
          inputs: [
            {
              id: "selftest-result",
              label: "Self-test response",
              type: "selection",
              options: ["PASS", "FAIL", "TIMEOUT"],
              expected: "PASS",
            },
            { id: "selftest-log", label: "Attach self-test log", type: "attachment", options: [] },
          ],
        },
      ],
    },
  ],
};

describe("templateToBlocks", () => {
  test("makes one step block per step with its content as children", () => {
    const blocks = templateToBlocks(template, md);

    expect(blocks.map((b) => [b.type, b.content])).toEqual([
      ["step", "Preconditions"],
      ["step", "Step 1: Power On"],
      ["step", "Step 2: Functional Check"],
    ]);
    expect(blocks[1]?.children?.map((b) => b.type)).toEqual([
      "paragraph",
      "checkListItem",
      "checkListItem",
      "procnoteInput",
      "paragraph",
    ]);
  });

  test("rebuilds checklist nesting from nesting levels", () => {
    const [, step] = templateToBlocks(template, md);
    const first = step?.children?.[1];

    expect(first?.children?.[0]?.content).toBe("a nested checkbox item");
    expect(first?.children?.[0]?.children?.[0]?.content).toBe("deeper");
    expect(step?.children?.[2]?.content).toBe("back at the top");
  });

  test("flattens input definitions into primitive props", () => {
    const [, , step] = templateToBlocks(template, md);

    expect(step?.children?.[0]?.props).toMatchObject({
      inputId: "selftest-result",
      inputType: "selection",
      options: "PASS\nFAIL\nTIMEOUT",
      expectedKind: "exact",
      expectedExact: "PASS",
    });
  });
});

describe("blocksToTemplate", () => {
  test("round-trips a template without changes", () => {
    const { template: back, issues } = blocksToTemplate(
      templateToBlocks(template, md),
      metadata,
      md,
    );

    expect(issues).toEqual([]);
    expect(diffPaths(template, back)).toEqual([]);
  });

  test("reads editor inline content and ignores the trailing empty paragraph", () => {
    const blocks: DocBlock[] = [
      {
        id: "s1",
        type: "step",
        content: [
          { type: "text", text: "Check ", styles: {} },
          {
            type: "link",
            href: "https://example.com",
            content: [{ type: "text", text: "docs", styles: { bold: true } }],
          },
        ],
        children: [],
      },
      { id: "p", type: "paragraph", content: [], children: [] },
    ];

    const { template: result, issues } = blocksToTemplate(blocks, metadata, md);

    expect(result.steps[0]?.heading).toBe("Check docs");
    expect(issues).toEqual([]);
  });

  test("reports structural problems", () => {
    const blocks: DocBlock[] = [
      { id: "intro", type: "paragraph", content: "Before the first step", children: [] },
      {
        id: "s1",
        type: "step",
        content: "",
        children: [
          { id: "inner", type: "step", content: "Nested", children: [] },
          {
            id: "cb",
            type: "checkListItem",
            content: "item",
            children: [{ id: "para", type: "paragraph", content: "note", children: [] }],
          },
        ],
      },
    ];

    const { issues } = blocksToTemplate(blocks, metadata, md);

    expect(issues).toEqual([
      { blockId: "intro", message: "Content outside a step is not part of the procedure." },
      { blockId: "s1", message: "Step has no heading." },
      { blockId: "inner", message: "Steps cannot be nested inside steps." },
      { blockId: "para", message: "Only checklist items can be nested under a checklist item." },
    ]);
  });

  test("reports duplicate input IDs across steps", () => {
    const input = (id: string): DocBlock => ({
      type: "procnoteInput",
      props: { inputId: id, label: "L", inputType: "text", expectedKind: "none" },
    });
    const blocks: DocBlock[] = [
      { type: "step", content: "A", children: [input("x")] },
      { type: "step", content: "B", children: [input("x")] },
    ];

    expect(blocksToTemplate(blocks, metadata, md).issues).toEqual([
      { message: 'Input ID "x" is used more than once.' },
    ]);
  });
});

describe("tables", () => {
  const text = (value: string, styles: Record<string, boolean> = {}) => ({
    type: "text",
    text: value,
    styles,
  });
  const cell = (content: unknown[], textAlignment = "left") => ({
    type: "tableCell",
    content,
    props: { textAlignment },
  });
  const tableBlock = (rows: unknown[][][]): DocBlock => ({
    type: "table",
    content: {
      type: "tableContent",
      rows: rows.map((cells) => ({
        cells: cells.map((content, column) => cell(content, column === 1 ? "right" : "left")),
      })),
    },
  });
  const stepWith = (children: DocBlock[]): DocBlock[] => [{ type: "step", content: "S", children }];

  test("a table with input and checkbox cells becomes a Table item", () => {
    const blocks = stepWith([
      tableBlock([
        [[text("Parameter")], [text("Value")], [text("OK")]],
        [[text("a | b")], [text("input:v", { code: true })], [text("[ ]")]],
        [[], [text("x")], [text("[x]")]],
      ]),
    ]);

    const { template } = blocksToTemplate(blocks, metadata, md);

    expect(template.steps[0]?.content).toEqual([
      {
        type: "Table",
        alignments: ["none", "right", "none"],
        header: ["Parameter", "Value", "OK"],
        rows: [
          [
            { type: "Text", text: "a \\| b" },
            { type: "Input", input_id: "v" },
            { type: "Checkbox", checked: false },
          ],
          [
            { type: "Text", text: "" },
            { type: "Text", text: "x" },
            { type: "Checkbox", checked: true },
          ],
        ],
      },
    ]);
  });

  test("a table without such cells stays prose", () => {
    const blocks = stepWith([tableBlock([[[text("A")]], [[text("input:v")]]])]);

    const tablesAsProse: MarkdownConverter = {
      ...md,
      fromBlocks: (run) => run.map((block) => block.type).join(),
    };

    const { template } = blocksToTemplate(blocks, metadata, tablesAsProse);

    expect(template.steps[0]?.content).toEqual([{ type: "Prose", text: "table" }]);
  });

  test("a Table item is written as procnote Markdown for the editor", () => {
    expect(
      tableMarkdown({
        type: "Table",
        alignments: ["none", "right", "center"],
        header: ["A", "B", "C"],
        rows: [
          [
            { type: "Text", text: "*a*" },
            { type: "Input", input_id: "v" },
            { type: "Checkbox", checked: true },
          ],
        ],
      }),
    ).toBe("| A | B | C |\n| --- | --: | :-: |\n| *a* | `input:v` | [x] |");
  });
});

describe("propsToInput", () => {
  test.each([
    [{ inputId: "", label: "L", inputType: "text" }, "Input has no ID."],
    [{ inputId: "a", label: "", inputType: "text" }, 'Input "a" has no label.'],
    [
      { inputId: "a", label: "L", inputType: "selection", options: "" },
      'Selection input "a" has no options.',
    ],
    [
      {
        inputId: "a",
        label: "L",
        inputType: "measurement",
        expectedKind: "range",
        expectedMin: 5,
        expectedMax: 1,
      },
      'Input "a" has an invalid expected range.',
    ],
    [
      { inputId: "a", label: "L", inputType: "text", expectedKind: "exact", expectedExact: " " },
      'Input "a" expects an exact value but none is given.',
    ],
  ])("validates %j", (props, message) => {
    expect(propsToInput(props, "b").issues).toEqual([{ blockId: "b", message }]);
  });

  test("omits empty unit and expected value", () => {
    const { input } = propsToInput(
      { inputId: "a", label: "L", inputType: "text", unit: " ", expectedKind: "none" },
      undefined,
    );

    expect(input).toEqual({ id: "a", label: "L", type: "text", options: [] });
  });
});

describe("enclosingStep", () => {
  // step-a > checklist > step-b > paragraph
  const parents: Record<string, DocBlock> = {
    checklist: { id: "step-a", type: "step" },
    "step-b": { id: "checklist", type: "checkListItem" },
    paragraph: { id: "step-b", type: "step" },
  };
  const parentOf = (id: string) => parents[id];

  test("finds the outermost step around a block", () => {
    expect(enclosingStep("paragraph", parentOf)?.id).toBe("step-a");
    expect(enclosingStep("step-b", parentOf)?.id).toBe("step-a");
  });

  test("is undefined at the top level", () => {
    expect(enclosingStep("step-a", parentOf)).toBeUndefined();
  });
});

test("inputToProps treats null optional fields from the backend as absent", () => {
  const fromBackend = JSON.parse(
    '{"id":"a","label":"L","type":"text","unit":null,"options":[],"expected":null}',
  );

  expect(inputToProps(fromBackend)).toMatchObject({ unit: "", expectedKind: "none" });
});

test("diffPaths treats null and missing fields as equal", () => {
  expect(diffPaths({ a: 1, unit: null }, { a: 1 })).toEqual([]);
});

test("diffPaths lists differing leaves", () => {
  expect(diffPaths({ a: [1, { b: 2 }], c: "x" }, { a: [1, { b: 3 }, 4], c: "x" })).toEqual([
    "a[1].b",
    "a[2]",
  ]);
});

describe("metadataIssues", () => {
  test.each([
    [{ id: "P-1", title: "T", version: "1" }, []],
    [{ id: "P-1", title: " ", version: "1" }, ["The procedure has no title."]],
    [{ title: "T", version: 1 }, ["The procedure has no ID.", "The procedure has no version."]],
  ])("%o", (metadata, expected) => {
    expect(metadataIssues(metadata).map((issue) => issue.message)).toEqual(expected);
  });
});
