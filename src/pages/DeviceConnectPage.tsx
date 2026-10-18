import { useCallback, useEffect, useState } from "react";
import { useSearchParams } from "react-router";
import * as api from "#lib/api/commands.js";
import { formatTimestamp } from "#lib/utils/format.js";
import { isLoopbackCallback } from "#lib/utils/loopback.js";

/**
 * Connect a desktop app: the app opens this page (behind Cloudflare Access)
 * with `callback`, `state`, and `device`; confirming issues a device token
 * for the signed-in user and hands it to the app on this machine. Also lists
 * the user's connected devices, to revoke them.
 */
export default function DeviceConnectPage() {
  const [params] = useSearchParams();
  const callback = params.get("callback") ?? "";
  const state = params.get("state") ?? "";
  const device = params.get("device") ?? "";
  const connecting = callback !== "";
  const [me, setMe] = useState<api.Me | null>(null);
  const [devices, setDevices] = useState<api.DeviceToken[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [done, setDone] = useState(false);

  const reload = useCallback(() => {
    api.listDeviceTokens().then(setDevices, (e) => setError(String(e)));
  }, []);

  useEffect(() => {
    void api.getMe().then(setMe);
    reload();
  }, [reload]);

  async function connect() {
    try {
      const issued = await api.issueDeviceToken(device);
      const target = new URL(callback);
      target.searchParams.set("state", state);
      target.searchParams.set("token", issued.token);
      target.searchParams.set("email", issued.email);
      setDone(true);
      window.location.assign(target.toString());
    } catch (e) {
      setError(String(e));
    }
  }

  async function revoke(id: string) {
    try {
      await api.revokeDeviceToken(id);
      reload();
    } catch (e) {
      setError(String(e));
    }
  }

  return (
    <div style={{ maxWidth: 640, display: "flex", flexDirection: "column", gap: 16 }}>
      <h2 style={{ margin: 0 }}>Desktop app</h2>
      {connecting &&
        (!isLoopbackCallback(callback) || !state || !device.trim() ? (
          <p style={{ color: "#c0392b" }}>
            This link doesn't come from a procnote desktop app on this computer, so no token is
            issued.
          </p>
        ) : done ? (
          <p>Connected. You can go back to the desktop app.</p>
        ) : (
          <section>
            <p>
              Connect <strong>{device}</strong> to this server
              {me?.email ? (
                <>
                  {" "}
                  as <strong>{me.email}</strong>
                </>
              ) : null}
              ? It will sync executions in its workspace, recorded as you, until you revoke it
              below.
            </p>
            <button className="btn btn-primary" onClick={connect}>
              Connect
            </button>
          </section>
        ))}
      <section>
        <h3 style={{ fontSize: 15 }}>Your connected devices</h3>
        {devices.length === 0 ? (
          <p>None.</p>
        ) : (
          <ul style={{ paddingLeft: 18 }}>
            {devices.map((token) => (
              <li key={token.id} style={{ marginBottom: 6 }}>
                {token.device} — connected {formatTimestamp(token.created_at)}{" "}
                <button className="btn btn-secondary" onClick={() => revoke(token.id)}>
                  Revoke
                </button>
              </li>
            ))}
          </ul>
        )}
      </section>
      {error && <p style={{ color: "#c0392b" }}>{error}</p>}
    </div>
  );
}
