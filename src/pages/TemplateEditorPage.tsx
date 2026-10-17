import { lazy, Suspense, useEffect, useState } from "react";
import { useNavigate, useSearchParams } from "react-router";
import * as api from "#lib/api/commands.js";
import type { ProcedureTemplate } from "#lib/types/index.js";

// BlockNote is large; load it only on this page.
const TemplateEditor = lazy(() => import("#lib/blocknote/TemplateEditor.js"));

export default function TemplateEditorPage() {
  const navigate = useNavigate();
  const path = useSearchParams()[0].get("path") ?? "";
  const [template, setTemplate] = useState<ProcedureTemplate | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    setTemplate(null);
    setError(null);
    api.getTemplate(path).then(
      (loaded) => !cancelled && setTemplate(loaded),
      (e) => !cancelled && setError(String(e)),
    );
    return () => {
      cancelled = true;
    };
  }, [path]);

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 12 }}>
      <div style={{ display: "flex", alignItems: "baseline", gap: 12 }}>
        <button className="btn btn-secondary" onClick={() => navigate("/")}>
          &larr; Back
        </button>
        <h2 style={{ margin: 0, fontSize: 18 }}>{template?.metadata.title ?? "Template"}</h2>
        <span style={{ color: "#e65100", fontSize: 12, fontWeight: 600 }}>
          Prototype editor — changes are not saved
        </span>
      </div>
      {error && <p style={{ color: "#c0392b" }}>{error}</p>}
      {template && (
        <Suspense fallback={<p>Loading editor...</p>}>
          <TemplateEditor key={path} template={template} />
        </Suspense>
      )}
    </div>
  );
}
