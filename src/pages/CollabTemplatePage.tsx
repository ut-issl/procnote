import { lazy, Suspense, useEffect, useState } from "react";
import { useNavigate, useParams } from "react-router";
import * as api from "#lib/api/commands.js";
import { executionStore } from "#lib/stores/execution.js";
import { useExecutionStore } from "#lib/stores/useExecutionStore.js";

// BlockNote is large; load it only on this page.
const CollabTemplateEditor = lazy(() => import("#lib/blocknote/CollabTemplateEditor.js"));

export default function CollabTemplatePage() {
  const navigate = useNavigate();
  const id = useParams().id ?? "";
  const [user, setUser] = useState<string | null>(null);
  const store = useExecutionStore();

  async function start() {
    const started = await executionStore.startShared(id);
    if (started) navigate(`/execution/${started.execution_id}`);
  }

  useEffect(() => {
    let cancelled = false;
    api.getMe().then((me) => !cancelled && setUser(me.email ?? ""));
    return () => {
      cancelled = true;
    };
  }, []);

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 12 }}>
      <div style={{ display: "flex", alignItems: "baseline", gap: 12 }}>
        <button className="btn btn-secondary" onClick={() => navigate("/")}>
          &larr; Back
        </button>
        <h2 style={{ margin: 0, fontSize: 18 }}>Shared template</h2>
      </div>
      {store.error && <p style={{ color: "#c0392b", margin: 0 }}>{store.error}</p>}
      {user !== null && (
        <Suspense fallback={<p>Loading editor...</p>}>
          <CollabTemplateEditor key={id} id={id} user={user} onStart={start} />
        </Suspense>
      )}
    </div>
  );
}
