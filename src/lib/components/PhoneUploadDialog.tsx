import DOMPurify from "dompurify";
import QRCode from "qrcode";
import { useEffect, useState } from "react";
import * as api from "#lib/api/commands.js";
import { isLoopbackHost } from "#lib/inbox.js";
import { scoped } from "#lib/utils/cx.js";
import Modal from "./Modal.js";
import styles from "./PhoneUploadDialog.module.css";

const c = scoped(styles);

/**
 * Shows the execution's upload link as a QR code. Phones that open it can
 * send files to the inbox until the execution finishes or the link is stopped.
 */
export default function PhoneUploadDialog({
  executionId,
  onclose,
}: {
  executionId: string;
  onclose: () => void;
}) {
  const [url, setUrl] = useState<string | null>(null);
  const [qrSvg, setQrSvg] = useState("");
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const link = await api.createUploadLink(executionId);
        const svg = await QRCode.toString(link, { type: "svg", margin: 1 });
        if (cancelled) return;
        setUrl(link);
        setQrSvg(DOMPurify.sanitize(svg, { USE_PROFILES: { svg: true } }));
      } catch (e) {
        if (!cancelled) setError(String(e));
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [executionId]);

  async function stop() {
    try {
      await api.revokeUploadLink(executionId);
      onclose();
    } catch (e) {
      setError(String(e));
    }
  }

  return (
    <Modal oncancel={onclose}>
      <h3>Upload from a phone</h3>
      <p>
        Scan to open the upload page. Photos and files sent there go to this execution's inbox, and
        you assign them to steps here. The link works until the execution finishes or you stop it.
      </p>
      {isLoopbackHost(window.location.hostname) && (
        <p className={c("warning")}>
          This page is open as {window.location.hostname}, which other devices cannot reach. Open
          procnote via this computer's network address to use the link from a phone.
        </p>
      )}
      {error && <p className={c("error")}>{error}</p>}
      {url && (
        <>
          <div className={c("qr")} dangerouslySetInnerHTML={{ __html: qrSvg }} />
          <p className={c("url")}>{url}</p>
        </>
      )}
      <div className="modal-actions">
        <button className="btn btn-secondary" onClick={stop} disabled={!url}>
          Stop link
        </button>
        <button className="btn btn-primary" onClick={onclose}>
          Done
        </button>
      </div>
    </Modal>
  );
}
