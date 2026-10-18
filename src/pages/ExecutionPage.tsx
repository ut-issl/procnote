import { useEffect, useState } from "react";
import { useNavigate, useParams } from "react-router";
import * as api from "#lib/api/commands.js";
import AddStepDialog from "#lib/components/AddStepDialog.js";
import ConflictsPanel from "#lib/components/ConflictsPanel.js";
import InboxPanel from "#lib/components/InboxPanel.js";
import Modal from "#lib/components/Modal.js";
import PhoneUploadDialog from "#lib/components/PhoneUploadDialog.js";
import RunView from "#lib/components/RunView.js";
import StepCard from "#lib/components/StepCard.js";
import { isDesktop } from "#lib/platform.js";
import { executionStore, isFinished } from "#lib/stores/execution.js";
import { useExecutionStore } from "#lib/stores/useExecutionStore.js";
import type { ExecutionAction, StepContent } from "#lib/types/index.js";
import { scoped } from "#lib/utils/cx.js";
import { formatTimestamp } from "#lib/utils/format.js";
import { isNonComposingEnter } from "#lib/utils/keyboard.js";
import styles from "./ExecutionPage.module.css";

const c = scoped(styles);

function handleAction(action: ExecutionAction): Promise<boolean> {
  return executionStore.act(action);
}

type Layout = "focus" | "list";
const LAYOUT_KEY = "procnote.executionLayout";

/** The execution layout, remembered per browser (best effort). */
function useLayout(): [Layout, (layout: Layout) => void] {
  const [layout, setLayout] = useState<Layout>(() => {
    try {
      return localStorage.getItem(LAYOUT_KEY) === "list" ? "list" : "focus";
    } catch {
      return "focus";
    }
  });
  function update(next: Layout) {
    setLayout(next);
    try {
      localStorage.setItem(LAYOUT_KEY, next);
    } catch {
      // Storage may be unavailable; the choice then lasts for this page only.
    }
  }
  return [layout, update];
}

export default function ExecutionPage() {
  const navigate = useNavigate();
  const executionId = useParams().id ?? "";
  const { summary, loading, error } = useExecutionStore();

  const [showAddStepDialog, setShowAddStepDialog] = useState(false);
  const [showCompleteDialog, setShowCompleteDialog] = useState(false);
  const [showAbortDialog, setShowAbortDialog] = useState(false);
  const [abortReason, setAbortReason] = useState("");
  const [editingName, setEditingName] = useState(false);
  const [editNameValue, setEditNameValue] = useState("");
  const [savingName, setSavingName] = useState(false);
  const [dropPointEnabled, setDropPointEnabled] = useState(false);
  const [layout, setLayout] = useLayout();
  const [showPhoneUpload, setShowPhoneUpload] = useState(false);

  useEffect(() => {
    if (!executionId) return;
    let cancelled = false;
    void (async () => {
      const [dropPointResult] = await Promise.allSettled([
        api.isDropPointConfigured(),
        executionStore.load(executionId),
      ]);
      if (cancelled) return;
      setDropPointEnabled(dropPointResult.status === "fulfilled" ? dropPointResult.value : false);
    })();
    return () => {
      cancelled = true;
    };
  }, [executionId]);

  // Show what other operators record on this execution as it happens.
  useEffect(() => {
    if (!executionId) return;
    return executionStore.watch(executionId);
  }, [executionId]);

  const isActive = summary?.status === "active";
  const finished = isFinished(summary);
  const skippedSteps = summary?.steps.filter((s) => s.status === "skipped").length ?? 0;
  const totalSteps = summary?.steps.length ?? 0;
  const stepRefs = summary?.steps.map((s) => ({ id: s.id, heading: s.heading })) ?? [];

  async function addStep(
    stepId: string,
    heading: string,
    content: StepContent[],
    afterStepId?: string,
  ) {
    if (
      await executionStore.act({
        action: "add_step",
        step_id: stepId,
        heading,
        content,
        after_step_id: afterStepId,
      })
    ) {
      setShowAddStepDialog(false);
    }
  }

  async function completeExecution(status: "pass" | "fail") {
    if (await executionStore.act({ action: "complete", status })) {
      setShowCompleteDialog(false);
    }
  }

  function closeAbortDialog() {
    setShowAbortDialog(false);
    setAbortReason("");
  }

  async function abortExecution() {
    if (!abortReason.trim()) return;
    if (await executionStore.act({ action: "abort", reason: abortReason.trim() })) {
      closeAbortDialog();
    }
  }

  function startEditingName() {
    setEditNameValue(summary?.name ?? summary?.procedure_id ?? "");
    setEditingName(true);
  }

  async function saveName() {
    if (savingName) return;
    const trimmed = editNameValue.trim();
    if (!trimmed || trimmed === summary?.name) {
      setEditingName(false);
      return;
    }
    setSavingName(true);
    const saved = await executionStore.act({ action: "rename_execution", name: trimmed });
    setSavingName(false);
    if (saved) setEditingName(false);
  }

  function goHome() {
    executionStore.reset();
    navigate("/");
  }

  async function openDirectory() {
    if (!summary?.execution_id) return;
    try {
      await api.revealExecutionDir(summary.execution_id);
    } catch (e) {
      executionStore.setError(String(e));
    }
  }

  return (
    <>
      <div className={c("execution-page")}>
        {loading && !summary ? (
          <p className={c("loading")}>Loading execution...</p>
        ) : error && !summary ? (
          <div className={c("error-panel")}>
            <p>{error}</p>
            <button className="btn btn-secondary" onClick={goHome}>
              Back to Home
            </button>
          </div>
        ) : (
          summary && (
            <>
              <div className={c("execution-header")}>
                <div className={c("header-left")}>
                  <button className={c("btn-back")} onClick={goHome}>
                    &larr; Back
                  </button>
                  <div className={c("header-info")}>
                    {editingName ? (
                      <input
                        className={c("name-input")}
                        type="text"
                        value={editNameValue}
                        onChange={(e) => setEditNameValue(e.target.value)}
                        onBlur={saveName}
                        onKeyDown={(e) => {
                          if (isNonComposingEnter(e.nativeEvent)) void saveName();
                          if (e.key === "Escape") setEditingName(false);
                        }}
                        autoFocus
                      />
                    ) : (
                      <button
                        className={c("execution-name")}
                        onClick={startEditingName}
                        title="Click to rename"
                      >
                        {summary.name ?? summary.procedure_id}
                      </button>
                    )}
                    <span className={c("procedure-meta")}>
                      {summary.procedure_title} ({summary.procedure_id}) v
                      {summary.procedure_version}
                    </span>
                    {summary.execution_dir &&
                      (isDesktop ? (
                        <button
                          className={c("execution-dir")}
                          onClick={openDirectory}
                          title="Open in Finder"
                        >
                          {summary.execution_dir}
                        </button>
                      ) : (
                        <span className={c("execution-dir")}>{summary.execution_dir}</span>
                      ))}
                  </div>
                </div>
                <div className={c("header-right")}>
                  <span
                    className={c(
                      "execution-status",
                      isActive && "status-active",
                      summary.status === "pass" && "status-pass",
                      summary.status === "fail" && "status-fail",
                      summary.status === "aborted" && "status-aborted",
                    )}
                  >
                    {summary.status}
                  </span>
                  <span className={c("progress")}>{totalSteps} steps</span>
                  <div className={c("layout-toggle")} role="group" aria-label="Layout">
                    {(["focus", "list"] as const).map((option) => (
                      <button
                        key={option}
                        className={c(layout === option && "active")}
                        aria-pressed={layout === option}
                        onClick={() => setLayout(option)}
                      >
                        {option === "focus" ? "Focus" : "List"}
                      </button>
                    ))}
                  </div>
                </div>
              </div>

              {error && <div className={c("error-bar")}>{error}</div>}

              {isActive && (
                <div className={c("toolbar")}>
                  <button className="btn btn-secondary" onClick={() => setShowAddStepDialog(true)}>
                    + Add Step
                  </button>
                  {!isDesktop && (
                    <button className="btn btn-secondary" onClick={() => setShowPhoneUpload(true)}>
                      Phone upload
                    </button>
                  )}
                  <div className={c("toolbar-spacer")} />
                  <button className="btn btn-success" onClick={() => setShowCompleteDialog(true)}>
                    Complete
                  </button>
                  <button className="btn btn-danger" onClick={() => setShowAbortDialog(true)}>
                    Abort
                  </button>
                </div>
              )}

              {finished && (
                <div
                  className={c(
                    "finish-banner",
                    summary.status === "pass" && "pass",
                    summary.status === "fail" && "fail",
                    summary.status === "aborted" && "aborted",
                  )}
                >
                  <span>
                    Execution{" "}
                    {summary.status === "pass"
                      ? "passed"
                      : summary.status === "fail"
                        ? "failed"
                        : "aborted"}
                    {summary.finished_at && ` at ${formatTimestamp(summary.finished_at)}`} &mdash;{" "}
                    {totalSteps} steps, {skippedSteps} skipped
                  </span>
                  <button
                    className="btn btn-undo"
                    onClick={() =>
                      handleAction({ action: "reopen_execution", reason: "Reopened by operator" })
                    }
                  >
                    Reopen Execution
                  </button>
                </div>
              )}

              <ConflictsPanel summary={summary} />

              <InboxPanel summary={summary} executionActive={isActive} onaction={handleAction} />

              {layout === "focus" ? (
                <RunView
                  summary={summary}
                  executionActive={isActive}
                  dropPointEnabled={dropPointEnabled}
                  onaction={handleAction}
                  ondropimported={(next) => executionStore.setSummary(next)}
                />
              ) : (
                <div className={c("steps")}>
                  {summary.steps.map((stepSummary) => (
                    <StepCard
                      key={stepSummary.id}
                      stepSummary={stepSummary}
                      executionId={summary.execution_id}
                      executionActive={isActive}
                      dropPointEnabled={dropPointEnabled}
                      onaction={handleAction}
                      ondropimported={(next) => executionStore.setSummary(next)}
                    />
                  ))}
                </div>
              )}
            </>
          )
        )}
      </div>

      {showPhoneUpload && summary && (
        <PhoneUploadDialog
          executionId={summary.execution_id}
          onclose={() => setShowPhoneUpload(false)}
        />
      )}

      {showAddStepDialog && (
        <AddStepDialog
          steps={stepRefs}
          onconfirm={addStep}
          oncancel={() => setShowAddStepDialog(false)}
        />
      )}

      {showCompleteDialog && (
        <Modal oncancel={() => setShowCompleteDialog(false)}>
          <h3>Complete Execution</h3>
          {summary && summary.inbox.length > 0 && (
            <p className={c("inbox-warning")}>
              {summary.inbox.length} file{summary.inbox.length === 1 ? " is" : "s are"} still in the
              inbox and not assigned to any step.
            </p>
          )}
          <p>Mark this execution as:</p>
          <div className="modal-actions">
            <button className="btn btn-secondary" onClick={() => setShowCompleteDialog(false)}>
              Cancel
            </button>
            <button className="btn btn-danger" onClick={() => completeExecution("fail")}>
              Fail
            </button>
            <button className="btn btn-success" onClick={() => completeExecution("pass")}>
              Pass
            </button>
          </div>
        </Modal>
      )}

      {showAbortDialog && (
        <Modal oncancel={closeAbortDialog}>
          <h3>Abort Execution</h3>
          <p>This will permanently mark the execution as aborted.</p>
          <label className="field">
            <span className="field-label">Reason</span>
            <textarea
              value={abortReason}
              onChange={(e) => setAbortReason(e.target.value)}
              placeholder="Why is the execution being aborted?"
              rows={3}
            />
          </label>
          <div className="modal-actions">
            <button className="btn btn-secondary" onClick={closeAbortDialog}>
              Cancel
            </button>
            <button
              className="btn btn-danger"
              onClick={abortExecution}
              disabled={!abortReason.trim()}
            >
              Abort
            </button>
          </div>
        </Modal>
      )}
    </>
  );
}
