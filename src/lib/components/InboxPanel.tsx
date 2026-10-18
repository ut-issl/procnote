import { useEffect, useState } from "react";
import * as api from "#lib/api/commands.js";
import { assignTargets } from "#lib/inbox.js";
import type { AttachmentState, ExecutionAction, ExecutionSummary } from "#lib/types/index.js";
import { scoped } from "#lib/utils/cx.js";
import { formatTimestamp } from "#lib/utils/format.js";
import styles from "./InboxPanel.module.css";

const c = scoped(styles);

function InboxItem({
  file,
  executionId,
  targets,
  disabled,
  onassign,
}: {
  file: AttachmentState;
  executionId: string;
  targets: ReturnType<typeof assignTargets>;
  disabled: boolean;
  onassign: (target: { step_id: string; input_id: string }) => void;
}) {
  const [preview, setPreview] = useState<string | null>(null);
  useEffect(() => {
    if (!file.content_type.startsWith("image/")) return;
    let cancelled = false;
    api.getAttachmentPreviewDataUrl(executionId, file.path).then(
      (url) => !cancelled && setPreview(url),
      () => {},
    );
    return () => {
      cancelled = true;
    };
  }, [executionId, file.path, file.content_type]);

  return (
    <li className={c("item")}>
      {preview ? (
        <img className={c("thumb")} src={preview} alt="" />
      ) : (
        <span className={c("thumb", "placeholder")}>{file.filename.split(".").pop()}</span>
      )}
      <span className={c("info")}>
        <span className={c("name")} title={file.filename}>
          {file.filename}
        </span>
        {file.at && <span className="timestamp">{formatTimestamp(file.at)}</span>}
      </span>
      <select
        aria-label={`Assign ${file.filename}`}
        value=""
        disabled={disabled || targets.length === 0}
        onChange={(e) => {
          const target = targets[Number(e.target.value)];
          if (target) onassign(target);
        }}
      >
        <option value="">{targets.length ? "Assign to…" : "No attachment inputs"}</option>
        {targets.map((target, index) => (
          <option key={`${target.step_id}/${target.input_id}`} value={index}>
            {target.label}
          </option>
        ))}
      </select>
    </li>
  );
}

/** Files received through the upload link, waiting to be assigned to inputs. */
export default function InboxPanel({
  summary,
  executionActive,
  onaction,
}: {
  summary: ExecutionSummary;
  executionActive: boolean;
  onaction: (action: ExecutionAction) => Promise<boolean>;
}) {
  if (summary.inbox.length === 0) return null;
  const targets = assignTargets(summary);
  return (
    <section className={c("inbox")} aria-label="Inbox">
      <h3 className={c("heading")}>
        Inbox <span className={c("count")}>{summary.inbox.length} unassigned</span>
      </h3>
      <ul className={c("list")}>
        {summary.inbox.map((file) => (
          <InboxItem
            key={file.path}
            file={file}
            executionId={summary.execution_id}
            targets={targets}
            disabled={!executionActive}
            onassign={(target) =>
              void onaction({ action: "assign_attachment", ...target, path: file.path })
            }
          />
        ))}
      </ul>
    </section>
  );
}
