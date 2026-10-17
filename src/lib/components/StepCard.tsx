import { useState } from "react";
import type { ExecutionAction, ExecutionSummary, StepSummary } from "#lib/types/index.js";
import { scoped } from "#lib/utils/cx.js";
import { formatTimestamp } from "#lib/utils/format.js";
import { isNonComposingEnter } from "#lib/utils/keyboard.js";
import StepBody from "./StepBody.js";
import styles from "./StepCard.module.css";

const c = scoped(styles);

export default function StepCard({
  stepSummary,
  executionId,
  executionActive = false,
  dropPointEnabled = false,
  onaction,
  ondropimported,
}: {
  stepSummary: StepSummary;
  executionId: string;
  executionActive?: boolean;
  dropPointEnabled?: boolean;
  onaction: (action: ExecutionAction) => Promise<boolean> | boolean;
  ondropimported?: (summary: ExecutionSummary) => void;
}) {
  const isPresent = stepSummary.status === "present";
  const isSkipped = stepSummary.status === "skipped";
  const stepId = stepSummary.id;

  const [showSkipDialog, setShowSkipDialog] = useState(false);
  const [skipReason, setSkipReason] = useState("");

  async function confirmSkip() {
    if (!skipReason.trim()) return;
    const skipped = await onaction({
      action: "skip_step",
      step_id: stepId,
      reason: skipReason.trim(),
    });
    if (skipped) {
      setShowSkipDialog(false);
      setSkipReason("");
    }
  }

  function cancelSkip() {
    setShowSkipDialog(false);
    setSkipReason("");
  }

  function unskipStep() {
    void onaction({ action: "unskip_step", step_id: stepId, reason: "Unskipped by operator" });
  }

  return (
    <div className={c("step-card", isSkipped && "skipped")}>
      <div className={c("step-header")}>
        <div className={c("step-status-indicator")} />
        <h3 className={c("step-heading")}>{stepSummary.heading}</h3>
        {stepSummary.status_at && (
          <span className="timestamp">{formatTimestamp(stepSummary.status_at)}</span>
        )}
        {isSkipped && <span className={c("step-status-badge")}>skipped</span>}
      </div>

      <StepBody
        stepSummary={stepSummary}
        executionId={executionId}
        executionActive={executionActive}
        dropPointEnabled={dropPointEnabled}
        onaction={onaction}
        ondropimported={ondropimported}
      />

      {executionActive && (
        <div className={c("step-actions")}>
          {isPresent && (
            <button className="btn btn-muted" onClick={() => setShowSkipDialog(true)}>
              Skip
            </button>
          )}
          {isSkipped && (
            <button className={c("btn", "btn-undo")} onClick={unskipStep}>
              Undo Skip
            </button>
          )}
        </div>
      )}

      {showSkipDialog && (
        <div className={c("skip-dialog")}>
          <label className={c("field")}>
            <span className={c("field-label")}>Reason for skipping</span>
            <input
              type="text"
              value={skipReason}
              onChange={(e) => setSkipReason(e.target.value)}
              placeholder="Enter reason..."
              autoFocus
              onKeyDown={(e) => {
                if (isNonComposingEnter(e.nativeEvent)) void confirmSkip();
              }}
            />
          </label>
          <div className={c("skip-actions")}>
            <button className="btn btn-muted" onClick={cancelSkip}>
              Cancel
            </button>
            <button className="btn btn-warn" onClick={confirmSkip} disabled={!skipReason.trim()}>
              Skip Step
            </button>
          </div>
        </div>
      )}
    </div>
  );
}
