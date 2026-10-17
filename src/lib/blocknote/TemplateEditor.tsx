import "@blocknote/mantine/style.css";
import { BlockNoteView } from "@blocknote/mantine";
import { useCreateBlockNote } from "@blocknote/react";
import { useCallback, useEffect, useMemo, useState } from "react";
import type { ProcedureTemplate } from "#lib/types/index.js";
import { scoped } from "#lib/utils/cx.js";
import {
  blocksToTemplate,
  type ConversionIssue,
  diffPaths,
  type DocBlock,
  templateToBlocks,
} from "./convert.js";
import {
  type Blocks,
  IssuesSection,
  markdownConverter,
  ProcnoteSlashMenu,
  TemplateJsonSection,
} from "./editorParts.js";
import { procnoteSchema } from "./schema.js";
import styles from "./TemplateEditor.module.css";

const c = scoped(styles);

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
          <ProcnoteSlashMenu editor={editor} />
        </BlockNoteView>
      </div>
      <aside className={c("panel")}>
        <IssuesSection issues={result?.issues} />
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
        <TemplateJsonSection template={result?.template} />
      </aside>
    </div>
  );
}
