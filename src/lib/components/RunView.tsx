import { useEffect, useRef, useState } from "react";
import {
  defaultFocus,
  isOutOfRange,
  nextStep,
  previousStep,
  runProgress,
  stepCheckboxes,
  stepInputs,
  stepProgress,
} from "#lib/run.js";
import type {
  ExecutionAction,
  ExecutionSummary,
  InputDefinitionSummary,
  StepSummary,
} from "#lib/types/index.js";
import { scoped } from "#lib/utils/cx.js";
import { isNonComposingEnter } from "#lib/utils/keyboard.js";
import { executionStore } from "#lib/stores/execution.js";
import StepBody from "./StepBody.js";
import styles from "./RunView.module.css";

const c = scoped(styles);

type StepState = "done" | "open" | "skipped";

function stepState(step: StepSummary): StepState {
  if (step.status === "skipped") return "skipped";
  return stepProgress(step).complete ? "done" : "open";
}

function inputValueText(input: InputDefinitionSummary): string {
  if (input.definition.type === "attachment") {
    const n = input.attachments.length;
    return n === 0 ? "―" : `${n} file${n === 1 ? "" : "s"}`;
  }
  if (!input.recorded) return "―";
  const unit = input.recorded.unit ?? input.definition.unit;
  return unit ? `${input.recorded.value} ${unit}` : input.recorded.value;
}

/** One row of the left overview: status, heading, and one-line summaries. */
function StepOverview({
  step,
  number,
  focused,
  onfocus,
}: {
  step: StepSummary;
  number: number;
  focused: boolean;
  onfocus: () => void;
}) {
  const [expanded, setExpanded] = useState(false);
  const rowRef = useRef<HTMLDivElement>(null);
  const state = stepState(step);
  const progress = stepProgress(step);
  const checkboxes = stepCheckboxes(step);

  useEffect(() => {
    if (focused) rowRef.current?.scrollIntoView({ block: "nearest", behavior: "smooth" });
  }, [focused]);

  return (
    <div ref={rowRef} className={c("overview-item", focused && "focused", state)}>
      <div className={c("overview-head")}>
        <button
          className={c("disclosure")}
          onClick={() => setExpanded((v) => !v)}
          aria-expanded={expanded}
          aria-label={expanded ? "Collapse details" : "Expand details"}
          disabled={checkboxes.length === 0 && step.notes.length === 0}
        >
          {expanded ? "▾" : "▸"}
        </button>
        <button className={c("overview-title")} onClick={onfocus}>
          <span className={c("state-dot")} />
          <span className={c("overview-number")}>{number}.</span>
          <span className={c("overview-heading")}>{step.heading}</span>
          <span className={c("overview-count")}>
            {state === "skipped"
              ? "skipped"
              : state === "done"
                ? "✓"
                : `${progress.remaining} left`}
          </span>
        </button>
      </div>
      <div className={c("overview-summary")}>
        {stepInputs(step).map((input) => {
          const value = input.recorded?.value ?? "";
          return (
            <div
              key={input.definition.id}
              className={c(
                "summary-line",
                isOutOfRange(input.definition.expected, value) && "out-of-range",
              )}
            >
              <span className={c("summary-label")}>{input.definition.label}</span>
              <span className={c("summary-value")}>{inputValueText(input)}</span>
            </div>
          );
        })}
        {step.notes.length > 0 && (
          <div className={c("summary-line", "muted")}>
            {step.notes.length} note{step.notes.length === 1 ? "" : "s"}
          </div>
        )}
      </div>
      {expanded && (
        <div className={c("overview-details")}>
          {checkboxes.map((box, index) => (
            <div
              key={box.id ?? index}
              className={c("detail-checkbox", box.checked && "checked")}
              style={{ paddingLeft: `${Math.min(box.nesting_level, 12) * 14}px` }}
            >
              {box.checked ? "☑" : "☐"} {box.text}
            </div>
          ))}
          {step.notes.map((note) => (
            <div key={note.id} className={c("detail-note")}>
              {note.text}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

/**
 * Two-pane run screen: an overview of every step on the left and the step
 * being worked on, with a pinned record footer, on the right.
 */
export default function RunView({
  summary,
  executionActive,
  dropPointEnabled,
  onaction,
  ondropimported,
}: {
  summary: ExecutionSummary;
  executionActive: boolean;
  dropPointEnabled: boolean;
  onaction: (action: ExecutionAction) => Promise<boolean>;
  ondropimported: (summary: ExecutionSummary) => void;
}) {
  const steps = summary.steps;
  const [focusId, setFocusId] = useState<string>();
  const [comment, setComment] = useState("");
  const [hint, setHint] = useState<string | null>(null);
  const bodyRef = useRef<HTMLDivElement>(null);

  const currentId = steps.some((s) => s.id === focusId) ? focusId : defaultFocus(steps);
  // Pin the default so finishing the focused step never moves focus by itself.
  useEffect(() => {
    if (currentId !== undefined && currentId !== focusId) setFocusId(currentId);
  }, [currentId, focusId]);
  const index = steps.findIndex((s) => s.id === currentId);
  const current = steps[index];
  const progress = runProgress(steps);
  const allDone = progress.complete + progress.skipped === progress.total;

  function focus(stepId: string | undefined) {
    if (!stepId) return;
    setFocusId(stepId);
    setComment("");
    setHint(null);
    bodyRef.current?.scrollTo({ top: 0 });
  }

  async function addCommentNote(step: StepSummary): Promise<boolean> {
    const text = comment.trim();
    if (!text) return true;
    return onaction({ action: "add_note", text, step_id: step.id });
  }

  async function goNext() {
    if (!current) return;
    if (executionActive && current.status === "present" && !(await addCommentNote(current))) {
      return;
    }
    focus(nextStep(steps, current.id) ?? current.id);
  }

  async function skip() {
    if (!current) return;
    const reason = comment.trim();
    if (!reason) {
      setHint("Enter a reason in the comment box to skip this step.");
      return;
    }
    if (await onaction({ action: "skip_step", step_id: current.id, reason })) {
      focus(nextStep(steps, current.id) ?? current.id);
    } else {
      setHint(`Could not skip: ${executionStore.getState().error ?? "unknown error"}`);
    }
  }

  function unskip() {
    if (!current) return;
    void onaction({ action: "unskip_step", step_id: current.id, reason: "Unskipped by operator" });
  }

  const currentProgress = current ? stepProgress(current) : null;
  const currentState = current ? stepState(current) : null;

  return (
    <div className={c("run")}>
      <div className={c("run-progress")}>
        <div className={c("bar")}>
          <div
            className={c("bar-fill")}
            style={{
              width: `${progress.total ? ((progress.complete + progress.skipped) / progress.total) * 100 : 0}%`,
            }}
          />
        </div>
        <span className={c("bar-count")}>
          {progress.complete} / {progress.total} steps done
          {progress.skipped > 0 && ` · ${progress.skipped} skipped`}
        </span>
        {executionActive && allDone && (
          <span className={c("all-done")}>All steps are done — use Complete to finish.</span>
        )}
      </div>

      <div className={c("panes")}>
        <nav className={c("overview")} aria-label="Steps">
          {steps.map((step, i) => (
            <StepOverview
              key={step.id}
              step={step}
              number={i + 1}
              focused={step.id === currentId}
              onfocus={() => focus(step.id)}
            />
          ))}
        </nav>

        {current && (
          <section className={c("focus-card", currentState)} aria-label="Current step">
            <div className={c("focus-top")}>
              <div className={c("breadcrumb")}>
                Step {index + 1} of {steps.length}
                {currentState === "skipped" && (
                  <span className={c("chip", "chip-skipped")}>skipped</span>
                )}
                {currentState === "done" && <span className={c("chip", "chip-done")}>done</span>}
                {currentState === "open" && currentProgress && (
                  <span className={c("chip")}>{currentProgress.remaining} left</span>
                )}
              </div>
              <h2 className={c("focus-heading")}>{current.heading}</h2>
            </div>

            <div ref={bodyRef} className={c("focus-body")}>
              <StepBody
                key={current.id}
                stepSummary={current}
                executionId={summary.execution_id}
                executionActive={executionActive}
                dropPointEnabled={dropPointEnabled}
                noteInput={false}
                onaction={onaction}
                ondropimported={ondropimported}
              />
            </div>

            <div className={c("focus-footer")}>
              {executionActive && current.status === "present" && (
                <input
                  className={c("comment-box")}
                  value={comment}
                  onChange={(e) => {
                    setComment(e.target.value);
                    setHint(null);
                  }}
                  placeholder="Add a note, or a reason to skip (Enter: next)"
                  onKeyDown={(e) => {
                    if (isNonComposingEnter(e.nativeEvent)) void goNext();
                  }}
                />
              )}
              {hint && <p className={c("hint")}>{hint}</p>}
              <div className={c("footer-actions")}>
                <button
                  className="btn btn-secondary"
                  onClick={() => focus(previousStep(steps, current.id))}
                  disabled={index <= 0}
                >
                  ← Previous
                </button>
                <div className={c("spacer")} />
                {executionActive && current.status === "present" && (
                  <button className="btn btn-warn" onClick={skip}>
                    Skip
                  </button>
                )}
                {executionActive && current.status === "skipped" && (
                  <button className="btn btn-undo" onClick={unskip}>
                    Undo Skip
                  </button>
                )}
                <button
                  className={currentState === "done" ? "btn btn-success" : "btn btn-primary"}
                  onClick={goNext}
                  disabled={nextStep(steps, current.id) === undefined && !comment.trim()}
                >
                  {currentState === "done" ? "✓ Done — next" : "Next →"}
                </button>
              </div>
            </div>
          </section>
        )}
      </div>
    </div>
  );
}
