import "@blocknote/mantine/style.css";
import { BlockNoteView } from "@blocknote/mantine";
import { useCreateBlockNote } from "@blocknote/react";
import { useEffect, useMemo } from "react";
import type { ProcedureTemplate } from "#lib/types/index.js";
import { scoped } from "#lib/utils/cx.js";
import { templateToBlocks } from "./convert.js";
import { type Blocks, markdownConverter } from "./editorParts.js";
import { procnoteSchema } from "./schema.js";
import styles from "./TemplateEditor.module.css";

const c = scoped(styles);

/** A template shown as the editor shows it, read-only (where it can't be edited together). */
export default function TemplateView({ template }: { template: ProcedureTemplate }) {
  const editor = useCreateBlockNote({ schema: procnoteSchema });
  const md = useMemo(() => markdownConverter(editor), [editor]);

  useEffect(() => {
    editor.replaceBlocks(editor.document, templateToBlocks(template, md) as unknown as Blocks);
  }, [editor, md, template]);

  return (
    // inert: the procnote blocks' own form fields ignore `editable`.
    <div className={c("editor")} inert>
      <BlockNoteView editor={editor} theme="light" editable={false} />
    </div>
  );
}
