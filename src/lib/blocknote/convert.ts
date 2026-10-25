// Conversion between BlockNote documents and procnote's ProcedureTemplate.
//
// Pure functions: the Markdown <-> block conversion for prose is injected
// (in the app it is BlockNote's own, in tests a stand-in), so this module has
// no editor or DOM dependency.
//
// Mapping (migration plan §5, option (a)):
// - top-level `step` block (heading as inline content) -> Step; its child
//   blocks are the step's content
// - `checkListItem` (nested children = deeper nesting levels) -> Checkbox
// - `procnoteInput` (props only) -> InputDefinition; consecutive ones form one
//   InputBlock
// - `table` with input (`` `input:<id>` ``) or checkbox (`[ ]`) cells -> Table
// - any other run of blocks -> one Prose item, serialized to Markdown
import type {
  ExpectedValue,
  InputDefinition,
  InputType,
  ProcedureMetadata,
  ProcedureTemplate,
  Step,
  StepContent,
  TableAlignment,
  TableCell,
} from "#lib/types/index.js";
import { columnAlignments } from "./tables.js";

/** The structural subset of a BlockNote block this module reads and writes. */
export interface DocBlock {
  id?: string;
  type: string;
  props?: Record<string, unknown>;
  content?: unknown;
  children?: DocBlock[];
}

export interface MarkdownConverter {
  toBlocks(markdown: string): DocBlock[];
  fromBlocks(blocks: DocBlock[]): string;
}

export interface ConversionIssue {
  blockId?: string;
  message: string;
}

export const INPUT_TYPES = ["measurement", "text", "selection", "attachment"] as const;
export const EXPECTED_KINDS = ["none", "range", "exact"] as const;

/** Props of a `procnoteInput` block. BlockNote props are primitives only. */
export interface InputBlockProps {
  inputId: string;
  label: string;
  inputType: InputType;
  unit: string;
  /** One option per line. */
  options: string;
  expectedKind: (typeof EXPECTED_KINDS)[number];
  expectedMin: number;
  expectedMax: number;
  expectedExact: string;
}

// --- Template -> blocks ---

export function inputToProps(input: InputDefinition): InputBlockProps {
  // Absent optional fields arrive as `null` in JSON from the backend, though
  // the generated types declare them as optional; treat both alike.
  const expected = input.expected ?? undefined;
  return {
    inputId: input.id,
    label: input.label,
    inputType: input.type,
    unit: input.unit ?? "",
    options: input.options.join("\n"),
    expectedKind:
      expected === undefined ? "none" : typeof expected === "string" ? "exact" : "range",
    expectedMin: typeof expected === "object" ? expected.min : 0,
    expectedMax: typeof expected === "object" ? expected.max : 0,
    expectedExact: typeof expected === "string" ? expected : "",
  };
}

/** Rebuild the checklist tree from flat checkboxes with nesting levels. */
function checklistBlocks(checkboxes: Extract<StepContent, { type: "Checkbox" }>[]): DocBlock[] {
  const roots: DocBlock[] = [];
  const stack: { level: number; block: DocBlock }[] = [];
  for (const checkbox of checkboxes) {
    const level = checkbox.nesting_level ?? 0;
    const block: DocBlock = {
      type: "checkListItem",
      props: { checked: checkbox.checked },
      content: checkbox.text,
      children: [],
    };
    while (stack.length > 0 && (stack.at(-1)?.level ?? 0) >= level) stack.pop();
    const parent = stack.at(-1);
    if (parent) parent.block.children?.push(block);
    else roots.push(block);
    stack.push({ level, block });
  }
  return roots;
}

function stepChildren(content: StepContent[], md: MarkdownConverter): DocBlock[] {
  const blocks: DocBlock[] = [];
  let pendingChecklist: Extract<StepContent, { type: "Checkbox" }>[] = [];
  const flushChecklist = () => {
    blocks.push(...checklistBlocks(pendingChecklist));
    pendingChecklist = [];
  };
  for (const item of content) {
    if (item.type === "Checkbox") {
      pendingChecklist.push(item);
      continue;
    }
    flushChecklist();
    if (item.type === "Prose") {
      blocks.push(...md.toBlocks(item.text));
    } else if (item.type === "Table") {
      blocks.push(...md.toBlocks(tableMarkdown(item)));
    } else {
      blocks.push(
        ...item.inputs.map((input) => ({
          type: "procnoteInput",
          props: { ...inputToProps(input) },
          children: [],
        })),
      );
    }
  }
  flushChecklist();
  return blocks;
}

export function templateToBlocks(template: ProcedureTemplate, md: MarkdownConverter): DocBlock[] {
  return template.steps.map((step) => ({
    type: "step",
    content: step.heading,
    children: stepChildren(step.content, md),
  }));
}

type TableItem = Extract<StepContent, { type: "Table" }>;

/** A Table item as a Markdown pipe table, as procnote-core writes it. */
export function tableMarkdown(table: TableItem): string {
  const line = (cells: string[]) => `| ${cells.join(" | ")} |`;
  const delimiter: Record<TableAlignment, string> = {
    none: "---",
    left: ":--",
    center: ":-:",
    right: "--:",
  };
  const cell = (c: TableCell) => {
    switch (c.type) {
      case "Text":
        return c.text;
      case "Input":
        return `\`input:${c.input_id}\``;
      case "Checkbox":
        return c.checked ? "[x]" : "[ ]";
    }
  };
  return [
    line(table.header),
    line(table.alignments.map((alignment) => delimiter[alignment])),
    ...table.rows.map((row) => line(row.map(cell))),
  ].join("\n");
}

// --- Blocks -> template ---

/** Plain text of a block's inline content (styles and link targets dropped). */
export function inlineText(content: unknown): string {
  if (typeof content === "string") return content;
  if (!Array.isArray(content)) return "";
  return content
    .map((node: unknown) => {
      if (typeof node !== "object" || node === null) return "";
      const n = node as { type?: string; text?: unknown; content?: unknown };
      if (n.type === "text" && typeof n.text === "string") return n.text;
      if (n.type === "link") return inlineText(n.content);
      return "";
    })
    .join("");
}

function str(value: unknown): string {
  return typeof value === "string" ? value : "";
}

function num(value: unknown): number {
  return typeof value === "number" ? value : Number(value);
}

export function propsToInput(
  props: Record<string, unknown>,
  blockId: string | undefined,
): { input: InputDefinition; issues: ConversionIssue[] } {
  const issues: ConversionIssue[] = [];
  const issue = (message: string) => issues.push({ blockId, message });

  const rawType = str(props.inputType);
  const type: InputType = (INPUT_TYPES as readonly string[]).includes(rawType)
    ? (rawType as InputType)
    : "text";
  const id = str(props.inputId).trim();
  const label = str(props.label).trim();
  const unit = str(props.unit).trim();
  const options = str(props.options)
    .split("\n")
    .map((option) => option.trim())
    .filter(Boolean);

  if (!id) issue("Input has no ID.");
  if (!label) issue(`Input "${id || "?"}" has no label.`);
  if (type === "selection" && options.length === 0) {
    issue(`Selection input "${id || "?"}" has no options.`);
  }

  let expected: ExpectedValue | undefined;
  switch (props.expectedKind) {
    case "range": {
      const min = num(props.expectedMin);
      const max = num(props.expectedMax);
      if (!Number.isFinite(min) || !Number.isFinite(max) || min > max) {
        issue(`Input "${id || "?"}" has an invalid expected range.`);
      } else {
        expected = { min, max };
      }
      break;
    }
    case "exact": {
      const exact = str(props.expectedExact).trim();
      if (exact) expected = exact;
      else issue(`Input "${id || "?"}" expects an exact value but none is given.`);
      break;
    }
    default:
      break;
  }

  const input: InputDefinition = {
    id,
    label,
    type,
    options,
    ...(unit ? { unit } : {}),
    ...(expected !== undefined ? { expected } : {}),
  };
  return { input, issues };
}

const INPUT_CELL_PREFIX = "input:";

/** Inline content of a table cell (a `tableCell`, or bare inline content). */
function cellContent(cell: unknown): unknown {
  if (Array.isArray(cell)) return cell;
  return typeof cell === "object" && cell !== null
    ? (cell as { content?: unknown }).content
    : undefined;
}

/** The input ID of a cell holding only the inline code `input:<id>`. */
function cellInputId(content: unknown): string | undefined {
  if (!Array.isArray(content) || content.length !== 1) return undefined;
  const node = content[0] as { type?: string; text?: unknown; styles?: { code?: unknown } };
  if (node.type !== "text" || node.styles?.code !== true || typeof node.text !== "string") {
    return undefined;
  }
  return node.text.startsWith(INPUT_CELL_PREFIX)
    ? node.text.slice(INPUT_CELL_PREFIX.length).trim()
    : undefined;
}

/** Backslash-escape the pipes not escaped yet, so a cell stays one cell. */
function escapePipes(text: string): string {
  let out = "";
  let previous = "";
  for (const char of text) {
    if (char === "|" && previous !== "\\") out += "\\";
    out += char;
    previous = char;
  }
  return out;
}

function cellMarkdown(content: unknown, md: MarkdownConverter): string {
  if (!inlineText(content).trim()) return "";
  return escapePipes(md.fromBlocks([{ type: "paragraph", content }]).trim());
}

function bodyCell(content: unknown, md: MarkdownConverter): TableCell {
  const inputId = cellInputId(content);
  if (inputId !== undefined) return { type: "Input", input_id: inputId };
  const text = inlineText(content).trim();
  if (text === "[ ]") return { type: "Checkbox", checked: false };
  if (text === "[x]" || text === "[X]") return { type: "Checkbox", checked: true };
  return { type: "Text", text: cellMarkdown(content, md) };
}

/**
 * A table block as a Table item, or undefined when it has no input or
 * checkbox cells (it is then prose). Which cells are inputs or checkboxes is
 * read from the blocks, never from converted Markdown, so the answer is the
 * same in both passes of an asynchronous conversion.
 */
function tableItem(block: DocBlock, md: MarkdownConverter): TableItem | undefined {
  const rows = (block.content as { rows?: { cells?: unknown[] }[] } | undefined)?.rows ?? [];
  const [header, ...body] = rows.map((row) => (row.cells ?? []).map(cellContent));
  const fillable = body.some((row) =>
    row.some((content) => {
      const text = inlineText(content).trim();
      return cellInputId(content) !== undefined || ["[ ]", "[x]", "[X]"].includes(text);
    }),
  );
  if (!header || !fillable) return undefined;
  return {
    type: "Table",
    alignments: columnAlignments(block).map((alignment) =>
      alignment === "left" ? "none" : alignment,
    ),
    header: header.map((content) => cellMarkdown(content, md)),
    rows: body.map((row) => row.map((content) => bodyCell(content, md))),
  };
}

function convertStepChildren(
  children: DocBlock[],
  md: MarkdownConverter,
  issues: ConversionIssue[],
): StepContent[] {
  const content: StepContent[] = [];
  let prose: DocBlock[] = [];

  const flushProse = () => {
    if (prose.length === 0) return;
    const text = md.fromBlocks(prose).trim();
    if (text) content.push({ type: "Prose", text });
    prose = [];
  };

  const pushChecklist = (block: DocBlock, level: number) => {
    content.push({
      type: "Checkbox",
      text: inlineText(block.content),
      checked: block.props?.checked === true,
      ...(level > 0 ? { nesting_level: level } : {}),
    });
    for (const child of block.children ?? []) {
      if (child.type === "checkListItem") {
        pushChecklist(child, level + 1);
      } else {
        issues.push({
          blockId: child.id,
          message: "Only checklist items can be nested under a checklist item.",
        });
      }
    }
  };

  for (const block of children) {
    switch (block.type) {
      case "checkListItem":
        flushProse();
        pushChecklist(block, 0);
        break;
      case "procnoteInput": {
        flushProse();
        const { input, issues: inputIssues } = propsToInput(block.props ?? {}, block.id);
        issues.push(...inputIssues);
        const last = content.at(-1);
        if (last?.type === "InputBlock") last.inputs.push(input);
        else content.push({ type: "InputBlock", inputs: [input] });
        if (block.children?.length) {
          issues.push({ blockId: block.id, message: "Inputs cannot have nested blocks." });
        }
        break;
      }
      case "step":
        flushProse();
        issues.push({ blockId: block.id, message: "Steps cannot be nested inside steps." });
        break;
      case "table": {
        const table = tableItem(block, md);
        if (!table) {
          prose.push(block);
          break;
        }
        flushProse();
        content.push(table);
        break;
      }
      default:
        prose.push(block);
    }
  }
  flushProse();
  return content;
}

function checkUniqueInputIds(steps: Step[], issues: ConversionIssue[]) {
  const seen = new Set<string>();
  for (const step of steps) {
    for (const item of step.content) {
      if (item.type !== "InputBlock") continue;
      for (const input of item.inputs) {
        if (input.id && seen.has(input.id)) {
          issues.push({ message: `Input ID "${input.id}" is used more than once.` });
        }
        seen.add(input.id);
      }
    }
  }
}

export function blocksToTemplate(
  blocks: DocBlock[],
  metadata: ProcedureMetadata,
  md: MarkdownConverter,
): { template: ProcedureTemplate; issues: ConversionIssue[] } {
  const issues: ConversionIssue[] = [];
  const steps: Step[] = [];

  for (const block of blocks) {
    if (block.type !== "step") {
      // An empty trailing paragraph is what the editor leaves after the last block.
      const isEmptyParagraph =
        block.type === "paragraph" && !inlineText(block.content).trim() && !block.children?.length;
      if (!isEmptyParagraph) {
        issues.push({
          blockId: block.id,
          message: "Content outside a step is not part of the procedure.",
        });
      }
      continue;
    }
    const heading = inlineText(block.content).trim();
    if (!heading) issues.push({ blockId: block.id, message: "Step has no heading." });
    steps.push({ heading, content: convertStepChildren(block.children ?? [], md, issues) });
  }

  if (steps.length === 0) issues.push({ message: "The procedure has no steps." });
  checkUniqueInputIds(steps, issues);
  return { template: { metadata, steps }, issues };
}

/** Problems with metadata as stored in a document (fields may be missing or mistyped). */
export function metadataIssues(metadata: Record<string, unknown>): ConversionIssue[] {
  const required = [
    ["id", "ID"],
    ["title", "title"],
    ["version", "version"],
  ] as const;
  return required
    .filter(([key]) => typeof metadata[key] !== "string" || !metadata[key].trim())
    .map(([, name]) => ({ message: `The procedure has no ${name}.` }));
}

// --- Editor structure ---

/**
 * The outermost step that contains block `id` (not counting the block
 * itself), or undefined when the block is not inside a step.
 */
export function enclosingStep(
  id: string,
  parentOf: (id: string) => DocBlock | undefined,
): DocBlock | undefined {
  let outermost: DocBlock | undefined;
  for (let parent = parentOf(id); parent?.id; parent = parentOf(parent.id)) {
    if (parent.type === "step") outermost = parent;
  }
  return outermost;
}

// --- Comparison ---

/**
 * Paths (like `steps[1].content[0].text`) where two JSON values differ.
 * `null` and a missing field count as equal, as they do in the backend.
 */
export function diffPaths(a: unknown, b: unknown, path = ""): string[] {
  if (Object.is(a, b) || (a == null && b == null)) return [];
  if (Array.isArray(a) && Array.isArray(b)) {
    const length = Math.max(a.length, b.length);
    return Array.from({ length }, (_, i) => diffPaths(a[i], b[i], `${path}[${i}]`)).flat();
  }
  if (
    a &&
    b &&
    typeof a === "object" &&
    typeof b === "object" &&
    !Array.isArray(a) &&
    !Array.isArray(b)
  ) {
    const keys = new Set([...Object.keys(a), ...Object.keys(b)]);
    return [...keys].flatMap((key) =>
      diffPaths(
        (a as Record<string, unknown>)[key],
        (b as Record<string, unknown>)[key],
        path ? `${path}.${key}` : key,
      ),
    );
  }
  return [path || "(root)"];
}
