import { useEffect, useMemo, useRef, useState } from "react";
import DOMPurify from "dompurify";
import type {
  AttachmentDropPointSessionSummary,
  AttachmentDropPointStatus,
  AttachmentSource,
  AttachmentState,
  InputDefinition,
} from "#lib/types/index.js";
import {
  formatExpiryCountdown,
  remoteMessage,
  useDropPointUpload,
} from "#lib/hooks/useDropPointUpload.js";
import { confirmWarning, logWarn } from "#lib/platform.js";
import { scoped } from "#lib/utils/cx.js";
import { formatTimestamp } from "#lib/utils/format.js";
import Modal from "./Modal.js";
import TrashIcon from "./TrashIcon.js";
import styles from "./AttachmentField.module.css";

const c = scoped(styles);

function normalizedContentType(contentType: string): string {
  return contentType.split(";")[0]?.trim().toLowerCase() ?? "";
}

function isImageAttachment(file: AttachmentState): boolean {
  return normalizedContentType(file.content_type).startsWith("image/");
}

/** Load thumbnails for image attachments, keeping already-loaded ones. */
function usePreviewUrls(
  attachments: AttachmentState[],
  onpreview: ((path: string) => Promise<string | null>) | undefined,
): Record<string, string> {
  const [previewUrls, setPreviewUrls] = useState<Record<string, string>>({});
  const previewUrlsRef = useRef(previewUrls);
  previewUrlsRef.current = previewUrls;
  const onpreviewRef = useRef(onpreview);
  onpreviewRef.current = onpreview;
  const hasLoader = !!onpreview;

  useEffect(() => {
    const previewLoader = onpreviewRef.current;
    if (!previewLoader) {
      setPreviewUrls({});
      return;
    }
    let cancelled = false;
    const imageAttachments = attachments.filter(isImageAttachment);
    const imagePaths = new Set(imageAttachments.map((file) => file.path));
    setPreviewUrls((current) => {
      const retained = Object.fromEntries(
        Object.entries(current).filter(([path]) => imagePaths.has(path)),
      );
      return Object.keys(retained).length === Object.keys(current).length ? current : retained;
    });
    for (const file of imageAttachments) {
      if (previewUrlsRef.current[file.path]) continue;
      void previewLoader(file.path)
        .then((dataUrl) => {
          if (cancelled || !dataUrl) return;
          setPreviewUrls((current) =>
            current[file.path] === dataUrl ? current : { ...current, [file.path]: dataUrl },
          );
        })
        .catch(() => {
          // Thumbnail previews are best-effort; keep the attachment usable without one.
        });
    }
    return () => {
      cancelled = true;
    };
  }, [attachments, hasLoader]);

  return previewUrls;
}

export default function AttachmentField({
  definition,
  attachments = [],
  disabled = false,
  dropPointEnabled = false,
  onattach,
  onpick,
  onremovefile,
  onunassign,
  onclear,
  onpreview,
  onstartdrop,
  onpolldrop,
  onimportdrop,
  oncanceldrop,
}: {
  definition: InputDefinition;
  attachments?: AttachmentState[];
  disabled?: boolean;
  dropPointEnabled?: boolean;
  onattach: (files: AttachmentSource[]) => void;
  onpick?: () => Promise<AttachmentSource[]>;
  onremovefile?: (path: string) => void;
  /** Return a file to the execution's inbox. */
  onunassign?: (path: string) => void;
  onclear?: () => void;
  onpreview?: (path: string) => Promise<string | null>;
  onstartdrop?: () => Promise<AttachmentDropPointSessionSummary>;
  onpolldrop?: (sessionId: string) => Promise<AttachmentDropPointStatus>;
  onimportdrop?: (sessionId: string) => Promise<void>;
  oncanceldrop?: (sessionId: string) => Promise<void>;
}) {
  const [selectedFiles, setSelectedFiles] = useState<AttachmentSource[]>([]);
  const previewUrls = usePreviewUrls(attachments, onpreview);
  const remote = useDropPointUpload({ onstartdrop, onpolldrop, onimportdrop, oncanceldrop });

  const isRecorded = attachments.length > 0;
  const hasSelectedFiles = selectedFiles.length > 0;
  const canUseDropPoint =
    dropPointEnabled && !!onstartdrop && !!onpolldrop && !!onimportdrop && !!oncanceldrop;
  const expiryCountdown = remote.session
    ? formatExpiryCountdown(remote.session.expires_at, remote.countdownNow)
    : null;
  const sanitizedQrSvg = useMemo(
    () =>
      remote.session
        ? DOMPurify.sanitize(remote.session.qr_svg, { USE_PROFILES: { svg: true } })
        : "",
    [remote.session],
  );

  async function pickFiles() {
    if (!onpick) return;
    const files = await onpick();
    if (files.length === 0) return;
    setSelectedFiles((current) => [...current, ...files]);
  }

  function attachSelected() {
    if (selectedFiles.length === 0) return;
    onattach(selectedFiles);
    setSelectedFiles([]);
  }

  function removeSelected(index: number) {
    setSelectedFiles((current) => current.filter((_, i) => i !== index));
  }

  async function confirmRemoveFile(file: AttachmentState) {
    if (!onremovefile) return;
    let ok = false;
    try {
      ok = await confirmWarning(`Remove ${file.filename}?`, {
        title: "Remove attachment",
        okLabel: "Remove",
        cancelLabel: "Cancel",
      });
    } catch (e) {
      await logWarn(`[AttachmentField] remove confirmation failed: ${e}`);
      return;
    }
    if (ok) onremovefile(file.path);
  }

  async function confirmClearAll() {
    if (!onclear) return;
    let ok = false;
    try {
      ok = await confirmWarning(
        `Remove all ${attachments.length} attachments from ${definition.label}?`,
        {
          title: "Remove all attachments",
          okLabel: "Remove all",
          cancelLabel: "Cancel",
        },
      );
    } catch (e) {
      await logWarn(`[AttachmentField] clear confirmation failed: ${e}`);
      return;
    }
    if (ok) onclear();
  }

  return (
    <>
      <div className={c("input-field", isRecorded && "recorded")}>
        <div className={c("input-header")}>
          <span className={c("input-label")}>{definition.label}</span>
          {isRecorded && <span className={c("recorded-badge")}>{attachments.length} attached</span>}
        </div>

        {isRecorded && (
          <div className={c("attachment-list")}>
            {attachments.map((file) => (
              <div className={c("attachment-row")} key={file.path}>
                {isImageAttachment(file) && previewUrls[file.path] && (
                  <img
                    className={c("attachment-thumbnail")}
                    src={previewUrls[file.path]}
                    alt=""
                    title={file.filename}
                  />
                )}
                <span className={c("filename")} title={file.path}>
                  {file.filename}
                </span>
                <span className={c("hash")}>{file.sha256.slice(0, 7)}</span>
                {file.at && <span className={c("timestamp")}>{formatTimestamp(file.at)}</span>}
                {onunassign && (
                  <button
                    className={c("btn-unassign")}
                    title="Move back to the inbox"
                    onClick={() => onunassign(file.path)}
                    disabled={disabled}
                  >
                    ↩
                  </button>
                )}
                {onremovefile && (
                  <button
                    className={c("btn-delete")}
                    title="Remove attachment"
                    onClick={() => confirmRemoveFile(file)}
                    disabled={disabled}
                  >
                    <TrashIcon />
                  </button>
                )}
              </div>
            ))}
          </div>
        )}

        {hasSelectedFiles ? (
          <>
            <div className={c("attachment-list", "pending-list")}>
              {selectedFiles.map((file, index) => (
                <div className={c("attachment-row")} key={`${index}-${file.path}`}>
                  <span className={c("filename")} title={file.path}>
                    {file.filename}
                  </span>
                  <button
                    className={c("btn-clear")}
                    onClick={() => removeSelected(index)}
                    disabled={disabled}
                  >
                    Remove
                  </button>
                </div>
              ))}
            </div>
            <div className={c("input-row", "row-actions")}>
              <button className={c("btn-record")} onClick={attachSelected} disabled={disabled}>
                Attach {selectedFiles.length} {selectedFiles.length === 1 ? "File" : "Files"}
              </button>
              <button className={c("btn-choose")} onClick={pickFiles} disabled={disabled}>
                Add More
              </button>
              <button
                className={c("btn-clear")}
                onClick={() => setSelectedFiles([])}
                disabled={disabled}
              >
                Clear
              </button>
            </div>
          </>
        ) : (
          <div className={c("input-row", "row-actions")}>
            <button className={c("btn-choose")} onClick={pickFiles} disabled={disabled}>
              {isRecorded ? "Add Files" : "Choose Files"}
            </button>
            {canUseDropPoint && (
              <button className={c("btn-drop")} onClick={remote.start} disabled={disabled}>
                Upload via QR Code
              </button>
            )}
            {isRecorded && onclear && attachments.length > 1 && (
              <button className={c("btn-clear")} onClick={confirmClearAll} disabled={disabled}>
                Remove All
              </button>
            )}
          </div>
        )}
      </div>

      {remote.phase !== "idle" && (
        <Modal oncancel={remote.cancel}>
          <h3>Upload via QR Code</h3>
          {remote.session && (
            <>
              <div className={c("drop-name-card")}>
                <span className={c("drop-name-label")}>Confirm upload page</span>
                <p>
                  After scanning, confirm the upload page shows{" "}
                  <strong>{remote.session.display_name}</strong>.
                </p>
              </div>
              <div className={c("qr-code")} dangerouslySetInnerHTML={{ __html: sanitizedQrSvg }} />
              <p className={c("drop-url")}>{remote.session.qr_url}</p>
              <p className={c("expiry")}>
                Expires at {formatTimestamp(remote.session.expires_at)}
                {expiryCountdown && (
                  <span className={c("expiry-countdown")}>({expiryCountdown})</span>
                )}
              </p>
            </>
          )}
          <p>{remoteMessage(remote.phase)}</p>
          {!!remote.status?.pending_submissions && (
            <p>Pending submissions: {remote.status.pending_submissions}</p>
          )}
          {remote.error && <p className={c("remote-error")}>{remote.error}</p>}
          <div className="modal-actions">
            {remote.phase === "failed" && remote.session && (
              <button className={c("btn-record")} onClick={remote.retry}>
                Retry receiving
              </button>
            )}
            <button
              className={c("btn-clear")}
              onClick={remote.cancel}
              disabled={remote.phase === "canceling"}
            >
              {remote.phase === "canceling" ? "Stopping..." : "Stop receiving"}
            </button>
          </div>
        </Modal>
      )}
    </>
  );
}
