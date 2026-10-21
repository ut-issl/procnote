// Pieces shared by the collaborative template editor and the read-only
// template view: Markdown conversion, the slash menu, and the panel showing
// what the document converts to.
import { filterSuggestionItems, insertOrUpdateBlockForSlashMenu } from "@blocknote/core";
import {
  type DefaultReactSuggestionItem,
  getDefaultReactSlashMenuItems,
  SuggestionMenuController,
} from "@blocknote/react";
import type { ProcedureTemplate } from "#lib/types/index.js";
import { scoped } from "#lib/utils/cx.js";
import {
  type ConversionIssue,
  type DocBlock,
  enclosingStep,
  type MarkdownConverter,
} from "./convert.js";
import type { ProcnoteEditor } from "./schema.js";
import { readTableAlignments, writeTableAlignments } from "./tables.js";
import styles from "./TemplateEditor.module.css";

const c = scoped(styles);

export type Blocks = Parameters<ProcnoteEditor["replaceBlocks"]>[1];

/**
 * Prose conversion backed by BlockNote's own (lossy) Markdown support, with
 * table column alignment carried across (tables.ts).
 */
export function markdownConverter(editor: ProcnoteEditor): MarkdownConverter {
  return {
    toBlocks: (markdown) =>
      readTableAlignments(
        markdown,
        editor.tryParseMarkdownToBlocks(markdown) as unknown as DocBlock[],
      ),
    fromBlocks: (blocks) =>
      writeTableAlignments(editor.blocksToMarkdownLossy(blocks as unknown as Blocks), blocks),
  };
}

function procnoteSlashItems(editor: ProcnoteEditor): DefaultReactSuggestionItem[] {
  // Steps only exist at the top level, so don't offer one inside a step.
  const cursorBlock = editor.getTextCursorPosition().block;
  const insideStep =
    enclosingStep(cursorBlock.id, (id) => editor.getParentBlock(id) as DocBlock | undefined) !==
    undefined;
  const stepItem: DefaultReactSuggestionItem = {
    title: "Procedure step",
    subtext: "A step of the procedure; put its content inside (indented)",
    aliases: ["step", "section"],
    group: "Procedure",
    onItemClick: () => {
      insertOrUpdateBlockForSlashMenu(editor, { type: "step" });
    },
  };
  return [
    ...(insideStep ? [] : [stepItem]),
    {
      title: "Input field",
      subtext: "A value the operator records during execution",
      aliases: ["input", "measurement", "field"],
      group: "Procedure",
      onItemClick: () => {
        insertOrUpdateBlockForSlashMenu(editor, {
          type: "procnoteInput",
          props: { inputId: `input-${crypto.randomUUID().slice(0, 6)}`, inputType: "measurement" },
        });
      },
    },
  ];
}

/** The slash menu with procnote's blocks first. Use with `slashMenu={false}`. */
export function ProcnoteSlashMenu({ editor }: { editor: ProcnoteEditor }) {
  return (
    <SuggestionMenuController
      triggerCharacter="/"
      getItems={async (query) =>
        filterSuggestionItems(
          [...procnoteSlashItems(editor), ...getDefaultReactSlashMenuItems(editor)],
          query,
        )
      }
    />
  );
}

export function IssuesSection({ issues }: { issues: ConversionIssue[] | undefined }) {
  return (
    <section>
      <h3>Issues</h3>
      {issues && issues.length === 0 ? (
        <p className={c("ok")}>No issues.</p>
      ) : (
        <ul className={c("issues")}>
          {issues?.map((issue, i) => (
            <li key={i}>{issue.message}</li>
          ))}
        </ul>
      )}
    </section>
  );
}

export function TemplateJsonSection({ template }: { template: ProcedureTemplate | undefined }) {
  return (
    <section>
      <h3>ProcedureTemplate</h3>
      <pre className={c("json")}>{template && JSON.stringify(template.steps, null, 2)}</pre>
    </section>
  );
}
