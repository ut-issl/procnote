import { useEffect, useState } from "react";
import { localExecutions } from "#lib/offline/index.js";
import { scoped } from "#lib/utils/cx.js";
import styles from "./SyncStatus.module.css";

const c = scoped(styles);

/**
 * How many recorded things are still only on this device, so nobody closes
 * the browser thinking they're on the server. Browser only.
 */
export default function SyncStatus() {
  const [waiting, setWaiting] = useState(0);
  const [online, setOnline] = useState(() => navigator.onLine);

  useEffect(() => {
    if (!localExecutions) return;
    const update = () => void localExecutions?.waitingCount().then(setWaiting);
    const goOnline = () => setOnline(true);
    const goOffline = () => setOnline(false);
    update();
    const stop = localExecutions.onWaitingChange(update);
    window.addEventListener("online", goOnline);
    window.addEventListener("offline", goOffline);
    return () => {
      stop();
      window.removeEventListener("online", goOnline);
      window.removeEventListener("offline", goOffline);
    };
  }, []);

  if (!localExecutions) return null;
  if (waiting === 0) {
    return (
      <span className={c("sync", !online && "offline")} title="Everything is on the server">
        {online ? "Synced" : "Offline — synced"}
      </span>
    );
  }
  return (
    <span
      className={c("sync", "waiting")}
      title="Kept on this device and sent when the server can be reached. Don't clear this browser's data."
    >
      {waiting} not synced{online ? "" : " (offline)"}
    </span>
  );
}
