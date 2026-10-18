import { useEffect, useRef, useState } from "react";
import { localExecutions } from "#lib/offline/index.js";
import type { OfflineBackup, SyncProblem } from "#lib/offline/localExecutions.js";
import { scoped } from "#lib/utils/cx.js";
import styles from "./SyncStatus.module.css";

const c = scoped(styles);

function download(name: string, text: string) {
  const url = URL.createObjectURL(new Blob([text], { type: "application/json" }));
  const link = document.createElement("a");
  link.href = url;
  link.download = name;
  link.click();
  URL.revokeObjectURL(url);
}

function chooseFile(): Promise<File | undefined> {
  return new Promise((resolve) => {
    const input = document.createElement("input");
    input.type = "file";
    input.accept = "application/json,.json";
    input.addEventListener("change", () => resolve(input.files?.[0]));
    input.addEventListener("cancel", () => resolve(undefined));
    input.click();
  });
}

/**
 * What is still only on this device, and what to do about it: sign in again
 * when that's what blocks syncing, a warning when the browser may delete
 * what it keeps, and a backup file of the unsynced records. Browser only.
 */
export default function SyncStatus() {
  const [waiting, setWaiting] = useState(0);
  const [problem, setProblem] = useState<SyncProblem>(null);
  const [online, setOnline] = useState(() => navigator.onLine);
  const [persisted, setPersisted] = useState<boolean | null>(null);
  const [open, setOpen] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  const panel = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!localExecutions) return;
    const update = () => {
      void localExecutions?.waitingCount().then(setWaiting);
      setProblem(localExecutions?.syncProblem() ?? null);
    };
    const goOnline = () => setOnline(true);
    const goOffline = () => setOnline(false);
    update();
    void navigator.storage?.persisted?.().then(setPersisted);
    const stop = localExecutions.onWaitingChange(update);
    window.addEventListener("online", goOnline);
    window.addEventListener("offline", goOffline);
    return () => {
      stop();
      window.removeEventListener("online", goOnline);
      window.removeEventListener("offline", goOffline);
    };
  }, []);

  useEffect(() => {
    if (!open) return;
    const close = (event: MouseEvent) => {
      if (!panel.current?.contains(event.target as Node)) setOpen(false);
    };
    document.addEventListener("mousedown", close);
    return () => document.removeEventListener("mousedown", close);
  }, [open]);

  if (!localExecutions) return null;
  const local = localExecutions;

  async function saveBackup() {
    const backup = await local.backup();
    download(
      `procnote-unsynced-${backup.created_at.slice(0, 19).replaceAll(":", "")}.json`,
      JSON.stringify(backup),
    );
    setMessage(`Saved ${backup.executions.length} execution(s) to a file.`);
  }

  async function restoreBackup() {
    const file = await chooseFile();
    if (!file) return;
    try {
      const count = await local.restore(JSON.parse(await file.text()) as OfflineBackup);
      setMessage(`Restored ${count} execution(s); they sync like any other records.`);
    } catch (e) {
      setMessage(`Couldn't restore: ${String(e)}`);
    }
  }

  const label =
    waiting === 0
      ? online
        ? "Synced"
        : "Offline — synced"
      : problem === "login"
        ? `${waiting} not synced — sign in`
        : `${waiting} not synced${online ? "" : " (offline)"}`;

  return (
    <div className={c("wrapper")} ref={panel}>
      <button
        className={c(
          "sync",
          !online && "offline",
          waiting > 0 && "waiting",
          persisted === false && "unprotected",
        )}
        onClick={() => setOpen(!open)}
      >
        {persisted === false && waiting > 0 && "⚠ "}
        {label}
      </button>
      {open && (
        <div className={c("panel")} role="dialog" aria-label="Sync">
          <p>
            {waiting === 0
              ? "Everything recorded here is on the server."
              : `${waiting} recorded change(s) are kept on this device and will be sent when the server can be reached.`}
          </p>
          {problem === "login" && waiting > 0 && (
            <p className={c("action")}>
              The server needs you to sign in again before it takes them.{" "}
              <button className="btn btn-primary" onClick={() => window.location.reload()}>
                Sign in
              </button>
            </p>
          )}
          {persisted === false && (
            <p className={c("warning")}>
              This browser hasn't promised to keep this site's data: it may delete it when space
              runs low. Don't clear browsing data, and save a backup before long offline work.
            </p>
          )}
          <div className={c("buttons")}>
            <button className="btn btn-secondary" onClick={saveBackup} disabled={waiting === 0}>
              Save backup of unsynced records
            </button>
            <button className="btn btn-secondary" onClick={restoreBackup}>
              Restore from backup…
            </button>
          </div>
          {message && <p className={c("message")}>{message}</p>}
        </div>
      )}
    </div>
  );
}
