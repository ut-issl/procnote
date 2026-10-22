// Table column alignment across BlockNote's Markdown conversion, which drops
// it: reading Markdown, the delimiter row's alignment (`:--`, `--:`, `:-:`)
// is put on the table blocks' cells (`textAlignment`); writing, the delimiter
// rows of BlockNote's output are rewritten from the header cells' alignment.
import { lexer, type Token, type Tokens } from "marked";
import type { DocBlock } from "./convert.js";

type Alignment = "left" | "center" | "right";

interface TableContent {
  rows?: { cells?: unknown[] }[];
}

function isTable(token: Token): token is Tokens.Table {
  return token.type === "table";
}

function cellWithAlignment(cell: unknown, alignment: Alignment): unknown {
  if (typeof cell !== "object" || cell === null) return cell;
  const props = (cell as { props?: Record<string, unknown> }).props ?? {};
  return { ...cell, props: { ...props, textAlignment: alignment } };
}

/** Blocks parsed from `markdown`, with its tables' column alignment on their cells. */
export function readTableAlignments(markdown: string, blocks: DocBlock[]): DocBlock[] {
  const tables = lexer(markdown).filter(isTable);
  let next = 0;
  return blocks.map((block) => {
    if (block.type !== "table") return block;
    const align = tables[next++]?.align;
    const content = block.content as TableContent | undefined;
    if (!align || !content?.rows) return block;
    return {
      ...block,
      content: {
        ...content,
        rows: content.rows.map((row) => ({
          ...row,
          cells: row.cells?.map((cell, column) => cellWithAlignment(cell, align[column] ?? "left")),
        })),
      },
    };
  });
}

/** The column alignment of a table block, from its header row. */
export function columnAlignments(block: DocBlock): Alignment[] {
  const header = (block.content as TableContent | undefined)?.rows?.[0]?.cells ?? [];
  return header.map((cell) => {
    const alignment =
      typeof cell === "object" && cell !== null
        ? (cell as { props?: { textAlignment?: unknown } }).props?.textAlignment
        : undefined;
    return alignment === "center" || alignment === "right" ? alignment : "left";
  });
}

function delimiterRow(alignments: Alignment[]): string {
  const cells = alignments.map((alignment) =>
    alignment === "center" ? ":---:" : alignment === "right" ? "---:" : "---",
  );
  return `| ${cells.join(" | ")} |`;
}

/** `markdown` written from `blocks`, with the delimiter rows of its tables set from the blocks. */
export function writeTableAlignments(markdown: string, blocks: DocBlock[]): string {
  const tables = blocks.filter((block) => block.type === "table");
  let next = 0;
  // Top-level tokens' raw text concatenates back to the source.
  return lexer(markdown)
    .map((token) => {
      if (!isTable(token)) return token.raw;
      const block = tables[next++];
      const alignments = block ? columnAlignments(block) : [];
      if (alignments.length !== token.header.length) return token.raw;
      const [header, , ...rest] = token.raw.split("\n");
      return [header, delimiterRow(alignments), ...rest].join("\n");
    })
    .join("");
}
