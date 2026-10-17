import { type ChangeEvent, useEffect, useState } from "react";
import { useParams } from "react-router";
import * as api from "#lib/api/commands.js";
import { newActionMeta } from "#lib/operator.js";
import { scoped } from "#lib/utils/cx.js";
import styles from "./UploadPage.module.css";

const c = scoped(styles);

interface Batch {
  id: number;
  names: string[];
  status: "sending" | "done" | "failed";
  message?: string;
}

/**
 * Phone page behind an execution's upload link: send photos or files to the
 * execution's inbox, as often as needed, without logging in. They are
 * assigned to steps on the execution screen.
 */
export default function UploadPage() {
  const token = useParams().token ?? "";
  const [target, setTarget] = useState<api.UploadTarget | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [batches, setBatches] = useState<Batch[]>([]);

  useEffect(() => {
    api.getUploadTarget(token).then(setTarget, (e) => setError(String(e)));
  }, [token]);

  async function send(event: ChangeEvent<HTMLInputElement>) {
    const input = event.currentTarget;
    const files = [...(input.files ?? [])];
    input.value = "";
    if (files.length === 0) return;
    const id = Date.now();
    const update = (patch: Partial<Batch>) =>
      setBatches((all) => all.map((b) => (b.id === id ? { ...b, ...patch } : b)));
    setBatches((all) => [{ id, names: files.map((f) => f.name), status: "sending" }, ...all]);
    try {
      const { received, skipped } = await api.uploadToLink(token, files, newActionMeta());
      update({
        status: "done",
        message: skipped > 0 ? `${received} sent, ${skipped} already there` : `${received} sent`,
      });
    } catch (e) {
      update({ status: "failed", message: String(e) });
    }
  }

  if (error) return <p className={c("error")}>{error}</p>;
  if (!target) return <p className={c("muted")}>Loading...</p>;

  return (
    <div className={c("page")}>
      <p className={c("muted")}>{target.procedure_title}</p>
      <h1 className={c("title")}>{target.name}</h1>
      {target.active ? (
        <>
          <div className={c("actions")}>
            <label className={c("button", "primary")}>
              Take photo
              <input type="file" accept="image/*" capture="environment" hidden onChange={send} />
            </label>
            <label className={c("button")}>
              Choose files
              <input type="file" multiple hidden onChange={send} />
            </label>
          </div>
          <p className={c("muted")}>
            Files go to the execution's inbox. Keep this page open and send more at any time.
          </p>
        </>
      ) : (
        <p className={c("error")}>This execution is finished and no longer takes files.</p>
      )}
      {batches.length > 0 && (
        <ul className={c("batches")}>
          {batches.map((batch) => (
            <li key={batch.id} className={c("batch", batch.status)}>
              <span className={c("names")}>{batch.names.join(", ")}</span>
              <span className={c("status")}>
                {batch.status === "sending" ? "Sending..." : batch.message}
              </span>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
