import "@blocknote/mantine/style.css";
import { withCollaboration } from "@blocknote/core/yjs";
import { BlockNoteView } from "@blocknote/mantine";
import { useCreateBlockNote } from "@blocknote/react";
import { HocuspocusProvider } from "@hocuspocus/provider";
import { useCallback, useEffect, useMemo, useState } from "react";
import * as Y from "yjs";
import { getCollabToken } from "#lib/api/commands.js";
import type { ProcedureMetadata, ProcedureTemplate } from "#lib/types/index.js";
import { scoped } from "#lib/utils/cx.js";
import { DOCUMENT_FRAGMENT, METADATA_MAP } from "./blocks.js";
import {
  blocksToTemplate,
  type ConversionIssue,
  type DocBlock,
  metadataIssues,
} from "./convert.js";
import {
  IssuesSection,
  markdownConverter,
  ProcnoteSlashMenu,
  TemplateJsonSection,
} from "./editorParts.js";
import { procnoteSchema } from "./schema.js";
import styles from "./TemplateEditor.module.css";

const c = scoped(styles);

/** Where the collab service is reached: /collab on this origin. */
function collabUrl(): string {
  const scheme = window.location.protocol === "https:" ? "wss" : "ws";
  return `${scheme}://${window.location.host}/collab`;
}

/** A stable cursor color per user. */
function colorOf(name: string): string {
  let hash = 0;
  for (const char of name) hash = (hash * 31 + (char.codePointAt(0) ?? 0)) >>> 0;
  return `hsl(${hash % 360} 70% 45%)`;
}

interface Session {
  doc: Y.Doc;
  provider: HocuspocusProvider;
}

/**
 * Collaborative template editor: the template's Y.Doc, shared through the
 * collab service. Changes are saved as they are made.
 */
export default function CollabTemplateEditor({
  id,
  user,
  onStart,
}: {
  id: string;
  user: string;
  /** Start an execution of the template as it is now. */
  onStart: () => void;
}) {
  const [session, setSession] = useState<Session | null>(null);
  const [synced, setSynced] = useState(false);
  const [status, setStatus] = useState("connecting");
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const doc = new Y.Doc();
    const provider = new HocuspocusProvider({
      url: collabUrl(),
      name: id,
      document: doc,
      // Fetched on every (re)connect, since tokens are short-lived.
      token: () => getCollabToken(id),
      onStatus: ({ status }) => setStatus(status),
      onSynced: () => setSynced(true),
      onAuthenticationFailed: ({ reason }) => setError(`Not allowed to edit: ${reason}`),
    });
    setSession({ doc, provider });
    return () => {
      provider.destroy();
      doc.destroy();
      setSession(null);
      setSynced(false);
    };
  }, [id]);

  return (
    <div className={c("collab")}>
      <p className={c("connection", status === "connected" && "connected")}>
        {error ?? (status === "connected" ? "Connected — changes are saved as you type" : status)}
      </p>
      {session && synced ? (
        <SharedEditor session={session} user={user} onStart={onStart} />
      ) : (
        !error && <p>Loading template...</p>
      )}
    </div>
  );
}

/** The metadata map's fields, re-read whenever someone changes them. */
function useMetadata(map: Y.Map<unknown>): Record<string, unknown> {
  const [metadata, setMetadata] = useState(() => map.toJSON());
  useEffect(() => {
    const update = () => setMetadata(map.toJSON());
    map.observe(update);
    update();
    return () => map.unobserve(update);
  }, [map]);
  return metadata;
}

/** A metadata field that edits locally and commits to the shared map on blur or Enter. */
function MetadataField({
  label,
  value,
  onCommit,
}: {
  label: string;
  value: string;
  onCommit: (value: string) => void;
}) {
  const [draft, setDraft] = useState(value);
  useEffect(() => setDraft(value), [value]);
  const commit = () => {
    if (draft !== value) onCommit(draft);
  };
  return (
    <label className={c("meta-field")}>
      {label}
      <input
        value={draft}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => {
          if (e.key === "Enter") commit();
        }}
      />
    </label>
  );
}

const METADATA_FIELDS = [
  ["title", "Title"],
  ["id", "ID"],
  ["version", "Version"],
  ["author", "Author"],
] as const;

function SharedEditor({
  session,
  user,
  onStart,
}: {
  session: Session;
  user: string;
  onStart: () => void;
}) {
  const { doc, provider } = session;
  const metadataMap = useMemo(() => doc.getMap<unknown>(METADATA_MAP), [doc]);
  const metadata = useMetadata(metadataMap);
  const editor = useCreateBlockNote(
    withCollaboration({
      schema: procnoteSchema,
      collaboration: {
        // BlockNote only uses the provider's awareness (for cursors).
        provider: { awareness: provider.awareness ?? undefined },
        fragment: doc.getXmlFragment(DOCUMENT_FRAGMENT),
        user: { name: user || "Anonymous", color: colorOf(user) },
      },
    }),
    [doc, provider],
  );
  const md = useMemo(() => markdownConverter(editor), [editor]);
  const [result, setResult] = useState<{
    template: ProcedureTemplate;
    issues: ConversionIssue[];
  } | null>(null);

  const recompute = useCallback(() => {
    const converted = blocksToTemplate(
      editor.document as unknown as DocBlock[],
      metadata as unknown as ProcedureMetadata,
      md,
    );
    setResult({
      template: converted.template,
      issues: [...metadataIssues(metadata), ...converted.issues],
    });
  }, [editor, md, metadata]);

  useEffect(recompute, [recompute]);

  return (
    <div className={c("layout")}>
      <div className={c("editor")}>
        <div className={c("meta")}>
          {METADATA_FIELDS.map(([key, label]) => (
            <MetadataField
              key={key}
              label={label}
              value={typeof metadata[key] === "string" ? metadata[key] : ""}
              onCommit={(value) => {
                if (key === "author" && !value.trim()) metadataMap.delete(key);
                else metadataMap.set(key, value);
              }}
            />
          ))}
        </div>
        <BlockNoteView editor={editor} theme="light" slashMenu={false} onChange={recompute}>
          <ProcnoteSlashMenu editor={editor} />
        </BlockNoteView>
      </div>
      <aside className={c("panel")}>
        <section>
          <button
            className="btn btn-primary"
            disabled={!result || result.issues.length > 0}
            onClick={onStart}
            title={result?.issues.length ? "Fix the issues below first" : undefined}
          >
            Start execution
          </button>
        </section>
        <IssuesSection issues={result?.issues} />
        <TemplateJsonSection template={result?.template} />
      </aside>
    </div>
  );
}
