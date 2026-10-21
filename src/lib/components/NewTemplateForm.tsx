import { useState } from "react";
import { scoped } from "#lib/utils/cx.js";
import styles from "./NewTemplateForm.module.css";

const c = scoped(styles);

/**
 * Ask for a new template's procedure ID and title. The ID can't be changed
 * later, nor used by another template (archived ones included).
 */
export default function NewTemplateForm({
  submitLabel,
  initialTitle = "",
  onSubmit,
  onCancel,
}: {
  submitLabel: string;
  initialTitle?: string;
  /** Create the template; rejects with the reason it wasn't. */
  onSubmit: (procedureId: string, title: string) => Promise<void>;
  onCancel: () => void;
}) {
  const [procedureId, setProcedureId] = useState("");
  const [title, setTitle] = useState(initialTitle);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function submit() {
    setBusy(true);
    setError(null);
    try {
      await onSubmit(procedureId.trim(), title.trim());
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <form
      className={c("form")}
      onSubmit={(e) => {
        e.preventDefault();
        void submit();
      }}
    >
      <label className={c("field")}>
        Procedure ID
        <input
          value={procedureId}
          onChange={(e) => setProcedureId(e.target.value)}
          placeholder="e.g. TVT-002"
          required
          autoFocus
        />
      </label>
      <label className={c("field")}>
        Title
        <input value={title} onChange={(e) => setTitle(e.target.value)} required />
      </label>
      <div className={c("actions")}>
        <button
          type="submit"
          className="btn btn-primary"
          disabled={busy || !procedureId.trim() || !title.trim()}
        >
          {submitLabel}
        </button>
        <button type="button" className="btn btn-secondary" onClick={onCancel}>
          Cancel
        </button>
      </div>
      <p className={c("hint")}>The procedure ID can't be changed later.</p>
      {error && <p className={c("error")}>{error}</p>}
    </form>
  );
}
