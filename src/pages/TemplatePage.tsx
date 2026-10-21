import { lazy, Suspense, useCallback, useEffect, useState } from "react";
import { useNavigate, useParams } from "react-router";
import * as api from "#lib/api/commands.js";
import NewTemplateForm from "#lib/components/NewTemplateForm.js";
import { TemplateBadges } from "#lib/components/TemplateList.js";
import { fetchOrKept } from "#lib/offline/index.js";
import { confirmWarning } from "#lib/platform.js";
import { executionStore } from "#lib/stores/execution.js";
import { useExecutionStore } from "#lib/stores/useExecutionStore.js";
import { templatePagePath } from "#lib/templates.js";
import type { ProcedureTemplate } from "#lib/types/index.js";
import { scoped } from "#lib/utils/cx.js";
import styles from "./TemplatePage.module.css";

const c = scoped(styles);

// BlockNote is large; load it only on this page.
const CollabTemplateEditor = lazy(() => import("#lib/blocknote/CollabTemplateEditor.js"));
const TemplateView = lazy(() => import("#lib/blocknote/TemplateView.js"));

/**
 * A procedure's template, where its executions start. Where templates can be
 * edited together, this is the shared template's editor (created from the
 * workspace file on first opening); elsewhere, the workspace file read-only.
 */
export default function TemplatePage() {
  const navigate = useNavigate();
  const procedureId = useParams().procedureId ?? "";
  const store = useExecutionStore();
  const [catalog, setCatalog] = useState<api.TemplateCatalog | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [sharedId, setSharedId] = useState<string | null>(null);
  // Why the shared template couldn't be opened (e.g. offline): the file is shown instead.
  const [openError, setOpenError] = useState<string | null>(null);
  const [user, setUser] = useState<string | null>(null);
  const [duplicating, setDuplicating] = useState(false);

  const loadCatalog = useCallback(() => {
    fetchOrKept("template-catalog", api.getTemplateCatalog).then(setCatalog, (e) =>
      setError(String(e)),
    );
  }, []);

  useEffect(() => {
    setCatalog(null);
    setError(null);
    setSharedId(null);
    setOpenError(null);
    setDuplicating(false);
    loadCatalog();
  }, [procedureId, loadCatalog]);

  const entry = catalog?.entries.find((e) => e.procedure_id === procedureId);
  const collab = catalog?.collab ?? false;
  const listedSharedId = entry?.shared?.id ?? null;
  const file = entry?.file ?? null;

  useEffect(() => {
    if (!collab) return;
    if (listedSharedId) {
      setSharedId(listedSharedId);
      return;
    }
    if (!file) return;
    let cancelled = false;
    api.openSharedTemplate(file).then(
      (id) => {
        if (cancelled) return;
        setSharedId(id);
        // It may have just been created: list it as the shared template it now is.
        loadCatalog();
      },
      (e) => !cancelled && setOpenError(String(e)),
    );
    return () => {
      cancelled = true;
    };
  }, [collab, listedSharedId, file, loadCatalog]);

  useEffect(() => {
    if (!collab) return;
    let cancelled = false;
    api.getMe().then((me) => !cancelled && setUser(me.email ?? ""));
    return () => {
      cancelled = true;
    };
  }, [collab]);

  async function startShared() {
    if (!sharedId) return;
    const started = await executionStore.startShared(sharedId);
    if (started) navigate(`/execution/${started.execution_id}`);
  }

  async function startFile() {
    if (!file) return;
    const started = await executionStore.start(file);
    if (started) navigate(`/execution/${started.execution_id}`);
  }

  async function archive() {
    if (!sharedId || !entry) return;
    const confirmed = await confirmWarning(
      `Archive "${entry.title}"? It will no longer be listed, edited, or executed, and its procedure ID ${entry.procedure_id} can't be used again. Past executions are kept.`,
      { title: "Archive template", okLabel: "Archive", cancelLabel: "Cancel" },
    );
    if (!confirmed) return;
    try {
      await api.archiveTemplate(sharedId);
      navigate("/");
    } catch (e) {
      setError(String(e));
    }
  }

  async function resetFromFile() {
    if (!sharedId || !entry) return;
    const confirmed = await confirmWarning(
      `Replace "${entry.title}" with the workspace's template.md? Changes made here are lost, also for everyone editing it now.`,
      { title: "Reset from file", okLabel: "Reset", cancelLabel: "Cancel" },
    );
    if (!confirmed) return;
    try {
      await api.resetTemplateFromFile(sharedId);
      loadCatalog();
    } catch (e) {
      setError(String(e));
    }
  }

  const editing = collab && sharedId !== null;

  return (
    <div className={c("page")}>
      <div className={c("header")}>
        <button className="btn btn-secondary" onClick={() => navigate("/")}>
          &larr; Back
        </button>
        <div className={c("heading")}>
          <h2 className={c("title")}>{entry?.title || procedureId}</h2>
          {entry && (
            <span className={c("meta")}>
              {entry.procedure_id} &middot; v{entry.version} <TemplateBadges entry={entry} />
            </span>
          )}
        </div>
        <div className={c("actions")}>
          {editing && (
            <>
              <button className="btn btn-secondary" onClick={() => setDuplicating(true)}>
                Duplicate
              </button>
              {file && (
                <button className="btn btn-secondary" onClick={resetFromFile}>
                  Reset from file
                </button>
              )}
              <button className="btn btn-danger" onClick={archive}>
                Archive
              </button>
            </>
          )}
          {!editing && file && (
            <button className="btn btn-primary" onClick={startFile}>
              Start execution
            </button>
          )}
        </div>
      </div>
      {duplicating && entry && sharedId && (
        <NewTemplateForm
          submitLabel="Duplicate"
          initialTitle={`${entry.title} (copy)`}
          onCancel={() => setDuplicating(false)}
          onSubmit={async (newId, title) => {
            await api.duplicateTemplate(sharedId, newId, title);
            navigate(templatePagePath(newId));
          }}
        />
      )}
      {store.error && <p className={c("error")}>{store.error}</p>}
      {error && <p className={c("error")}>{error}</p>}
      {catalog && !entry && <p className={c("error")}>No template for procedure {procedureId}.</p>}
      {openError && (
        <p className={c("error")}>
          The shared template couldn't be opened ({openError}); showing the workspace file.
        </p>
      )}
      {editing && user !== null && (
        <Suspense fallback={<p>Loading editor...</p>}>
          <CollabTemplateEditor key={sharedId} id={sharedId} user={user} onStart={startShared} />
        </Suspense>
      )}
      {!editing && file && (!collab || openError) && <FileTemplate path={file} />}
    </div>
  );
}

/** A workspace template file, read-only. */
function FileTemplate({ path }: { path: string }) {
  const [template, setTemplate] = useState<ProcedureTemplate | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    setTemplate(null);
    setError(null);
    fetchOrKept(`template-content:${path}`, () => api.getTemplate(path)).then(
      (loaded) => !cancelled && setTemplate(loaded),
      (e) => !cancelled && setError(String(e)),
    );
    return () => {
      cancelled = true;
    };
  }, [path]);

  if (error) return <p className={c("error")}>{error}</p>;
  if (!template) return <p>Loading template...</p>;
  return (
    <Suspense fallback={<p>Loading template...</p>}>
      <TemplateView template={template} />
    </Suspense>
  );
}
