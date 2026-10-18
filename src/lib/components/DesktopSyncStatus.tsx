import { useCallback, useEffect, useRef, useState } from "react";
import * as api from "#lib/api/commands.js";
import type { SyncStatus } from "#lib/types/index.js";
import { scoped } from "#lib/utils/cx.js";
import { formatTimestamp } from "#lib/utils/format.js";
import styles from "./SyncStatus.module.css";

const c = scoped(styles);
const POLL_MS = 3000;
const SERVER_KEY = "procnote.syncServer";

function rememberedServer(): string {
  try {
    return localStorage.getItem(SERVER_KEY) ?? "";
  } catch {
    return "";
  }
}

/**
 * The desktop app's connection to a procnote server: connect the workspace
 * (sign in through the browser), see what's not synced yet, sync now, or
 * disconnect. Desktop only.
 */
export default function DesktopSyncStatus() {
  const [status, setStatus] = useState<SyncStatus | null>(null);
  const [open, setOpen] = useState(false);
  const [server, setServer] = useState(rememberedServer);
  const [device, setDevice] = useState("procnote desktop");
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const panel = useRef<HTMLDivElement>(null);

  const refresh = useCallback(() => {
    api.getSyncStatus().then(setStatus, () => undefined);
  }, []);

  useEffect(() => {
    refresh();
    const timer = window.setInterval(refresh, POLL_MS);
    return () => window.clearInterval(timer);
  }, [refresh]);

  useEffect(() => {
    if (!open) return;
    const close = (event: MouseEvent) => {
      if (!panel.current?.contains(event.target as Node)) setOpen(false);
    };
    document.addEventListener("mousedown", close);
    return () => document.removeEventListener("mousedown", close);
  }, [open]);

  async function run(label: string, action: () => Promise<SyncStatus>) {
    setBusy(label);
    setError(null);
    try {
      setStatus(await action());
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(null);
    }
  }

  function connect() {
    try {
      localStorage.setItem(SERVER_KEY, server.trim());
    } catch {
      // Remembering the address is only a convenience.
    }
    void run("Waiting for the browser…", () => api.connectServer(server.trim(), device.trim()));
  }

  if (!status) return null;
  const connected = status.connected;
  const label = !connected
    ? "Not connected"
    : status.problem === "unauthorized"
      ? "Connect again"
      : status.waiting > 0
        ? `${status.waiting} not synced${status.problem === "unreachable" ? " (offline)" : ""}`
        : "Synced";

  return (
    <div className={c("wrapper")} ref={panel}>
      <button
        className={c(
          "sync",
          (!connected || status.problem === "unreachable") && "offline",
          connected && status.waiting > 0 && "waiting",
        )}
        onClick={() => setOpen(!open)}
      >
        {label}
      </button>
      {open && (
        <div className={c("panel")} role="dialog" aria-label="Server">
          {connected ? (
            <>
              <p>
                This workspace syncs with <strong>{connected.server}</strong>
                {connected.email ? ` as ${connected.email}` : ""} ({connected.device}). Executions
                started here are kept in this folder and sent to the server when it can be reached.
              </p>
              <p>
                {status.waiting === 0
                  ? "Everything recorded here is on the server."
                  : `${status.waiting} recorded change(s) are kept here, waiting for the server.`}
                {status.last_synced_at && ` Last synced ${formatTimestamp(status.last_synced_at)}.`}
              </p>
              {status.message && <p className={c("warning")}>{status.message}</p>}
              <div className={c("buttons")}>
                <button
                  className="btn btn-secondary"
                  disabled={busy !== null}
                  onClick={() => run("Syncing…", api.syncNow)}
                >
                  Sync now
                </button>
                {status.problem === "unauthorized" && (
                  <button className="btn btn-primary" disabled={busy !== null} onClick={connect}>
                    Connect again
                  </button>
                )}
                <button
                  className="btn btn-secondary"
                  disabled={busy !== null}
                  onClick={() => run("Disconnecting…", api.disconnectServer)}
                >
                  Disconnect
                </button>
              </div>
            </>
          ) : (
            <>
              <p>
                Connect this workspace to a procnote server to share executions. They stay in this
                folder and work offline; they sync whenever the server can be reached.
              </p>
              <label className="field">
                <span className="field-label">Server</span>
                <input
                  value={server}
                  placeholder="https://procnote.example.ac.jp"
                  onChange={(e) => setServer(e.target.value)}
                />
              </label>
              <label className="field">
                <span className="field-label">This device</span>
                <input value={device} onChange={(e) => setDevice(e.target.value)} />
              </label>
              <div className={c("buttons")}>
                <button
                  className="btn btn-primary"
                  disabled={busy !== null || !server.trim() || !device.trim()}
                  onClick={connect}
                >
                  Connect…
                </button>
              </div>
            </>
          )}
          {busy && <p className={c("message")}>{busy}</p>}
          {error && <p className={c("warning")}>{error}</p>}
        </div>
      )}
    </div>
  );
}
