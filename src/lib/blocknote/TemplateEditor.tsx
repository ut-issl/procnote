import "@blocknote/mantine/style.css";
import { filterSuggestionItems, insertOrUpdateBlockForSlashMenu } from "@blocknote/core";
import { BlockNoteView } from "@blocknote/mantine";
import {
  type DefaultReactSuggestionItem,
  getDefaultReactSlashMenuItems,
  SuggestionMenuController,
  useCreateBlockNote,
} from "@blocknote/react";
import { useCallback, useEffect, useMemo, useState } from "react";
import type { ProcedureTemplate } from "#lib/types/index.js";
import { scoped } from "#lib/utils/cx.js";
import {
  blocksToTemplate,
  type ConversionIssue,
  diffPaths,
  type DocBlock,
  enclosingStep,
  type MarkdownConverter,
  templateToBlocks,
} from "./convert.js";
import { type ProcnoteEditor, procnoteSchema } from "./schema.js";
import styles from "./TemplateEditor.module.css";

const c = scoped(styles);

type Blocks = Parameters<ProcnoteEditor["replaceBlocks"]>[1];

/** Prose conversion backed by BlockNote's own (lossy) Markdown support. */
function markdownConverter(editor: ProcnoteEditor): MarkdownConverter {
  return {
    toBlocks: (markdown) => editor.tryParseMarkdownToBlocks(markdown) as unknown as DocBlock[],
    fromBlocks: (blocks) => editor.blocksToMarkdownLossy(blocks as unknown as Blocks),
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

/**
 * Prototype template editor: edits a template in BlockNote and shows, live,
 * the ProcedureTemplate it converts back to, any structural issues, and how
 * it differs from the template that was loaded. Nothing is saved.
 */
export default function TemplateEditor({ template }: { template: ProcedureTemplate }) {
  const editor = useCreateBlockNote({ schema: procnoteSchema });
  const md = useMemo(() => markdownConverter(editor), [editor]);
  const [result, setResult] = useState<{
    template: ProcedureTemplate;
    issues: ConversionIssue[];
  } | null>(null);

  const recompute = useCallback(() => {
    setResult(blocksToTemplate(editor.document as unknown as DocBlock[], template.metadata, md));
  }, [editor, md, template.metadata]);

  useEffect(() => {
    editor.replaceBlocks(editor.document, templateToBlocks(template, md) as unknown as Blocks);
    recompute();
  }, [editor, md, template, recompute]);

  const differences = result ? diffPaths(template, result.template) : [];

  return (
    <div className={c("layout")}>
      <div className={c("editor")}>
        <BlockNoteView editor={editor} theme="light" slashMenu={false} onChange={recompute}>
          <SuggestionMenuController
            triggerCharacter="/"
            getItems={async (query) =>
              filterSuggestionItems(
                [...procnoteSlashItems(editor), ...getDefaultReactSlashMenuItems(editor)],
                query,
              )
            }
          />
        </BlockNoteView>
      </div>
      <aside className={c("panel")}>
        <section>
          <h3>Issues</h3>
          {result && result.issues.length === 0 ? (
            <p className={c("ok")}>No issues.</p>
          ) : (
            <ul className={c("issues")}>
              {result?.issues.map((issue, i) => (
                <li key={i}>{issue.message}</li>
              ))}
            </ul>
          )}
        </section>
        <section>
          <h3>Compared with the loaded template</h3>
          {differences.length === 0 ? (
            <p className={c("ok")} data-testid="roundtrip-identical">
              Identical.
            </p>
          ) : (
            <ul className={c("diff")} data-testid="roundtrip-diff">
              {differences.slice(0, 30).map((path) => (
                <li key={path}>
                  <code>{path}</code>
                </li>
              ))}
              {differences.length > 30 && <li>… {differences.length - 30} more</li>}
            </ul>
          )}
        </section>
        <section>
          <h3>ProcedureTemplate</h3>
          <pre className={c("json")}>
            {result && JSON.stringify(result.template.steps, null, 2)}
          </pre>
        </section>
      </aside>
    </div>
  );
}
