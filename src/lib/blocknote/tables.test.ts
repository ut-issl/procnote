import { lexer, type Tokens } from "marked";
import { describe, expect, test } from "vite-plus/test";
import type { DocBlock } from "./convert.js";
import { readTableAlignments, writeTableAlignments } from "./tables.js";

const MARKDOWN = "Intro.\n\n| a | b | c |\n| :-- | --: | :-: |\n| 1 | 2 | 3 |\n\nAfter.\n";

/** A table block as BlockNote parses one: every cell left-aligned. */
function table(rows: number, columns: number, alignment = "left"): DocBlock {
  return {
    type: "table",
    content: {
      type: "tableContent",
      rows: Array.from({ length: rows }, () => ({
        cells: Array.from({ length: columns }, () => ({
          type: "tableCell",
          content: [],
          props: { textAlignment: alignment, colspan: 1 },
        })),
      })),
    },
  };
}

function cellAlignments(block: DocBlock | undefined): unknown[][] {
  const content = block?.content as
    | { rows: { cells: { props: { textAlignment: unknown } }[] }[] }
    | undefined;
  return (content?.rows ?? []).map((row) => row.cells.map((cell) => cell.props.textAlignment));
}

function tableAligns(markdown: string): (string | null)[][] {
  return lexer(markdown)
    .filter((token): token is Tokens.Table => token.type === "table")
    .map((token) => token.align);
}

describe("table alignment", () => {
  test("is read from the delimiter row onto every cell of its column", () => {
    const blocks = readTableAlignments(MARKDOWN, [
      { type: "paragraph", content: "Intro." },
      table(2, 3),
      { type: "paragraph", content: "After." },
    ]);

    expect(cellAlignments(blocks[1])).toEqual([
      ["left", "right", "center"],
      ["left", "right", "center"],
    ]);
    expect(blocks[0]).toEqual({ type: "paragraph", content: "Intro." });
  });

  test("is written into the delimiter row, leaving the rest as it is", () => {
    const written = "Intro.\n\n| a | b | c |\n| - | - | - |\n| 1 | 2 | 3 |\n\nAfter.\n";
    const blocks = readTableAlignments(MARKDOWN, [table(2, 3)]);

    const result = writeTableAlignments(written, blocks);

    expect(tableAligns(result)).toEqual([[null, "right", "center"]]);
    expect(result.startsWith("Intro.\n\n| a | b | c |\n")).toBe(true);
    expect(result.endsWith("| 1 | 2 | 3 |\n\nAfter.\n")).toBe(true);
  });

  test("leaves tables it can't match alone", () => {
    const written = "| a | b |\n| - | - |\n| 1 | 2 |\n";

    expect(writeTableAlignments(written, [table(2, 3, "right")])).toBe(written);
    expect(writeTableAlignments(written, [])).toBe(written);
    expect(readTableAlignments("no tables", [table(1, 2)])).toEqual([table(1, 2)]);
  });
});
