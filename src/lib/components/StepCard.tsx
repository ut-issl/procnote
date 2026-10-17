import { type MouseEvent, useState } from "react";
import * as api from "#lib/api/commands.js";
import { renderMarkdown } from "#lib/markdown.js";
import { logWarn, openExternalUrl } from "#lib/platform.js";
import type {
  AttachmentSource,
  ExecutionAction,
  ExecutionSummary,
  StepSummary,
} from "#lib/types/index.js";
import { scoped } from "#lib/utils/cx.js";
import { formatTimestamp } from "#lib/utils/format.js";
import { isNonComposingEnter } from "#lib/utils/keyboard.js";
import AttachmentField from "./AttachmentField.js";
import CheckboxItem from "./CheckboxItem.js";
import InputField from "./InputField.js";
import NoteEditor from "./NoteEditor.js";
import styles from "./StepCard.module.css";

const c = scoped(styles);

/** Open links in rendered Markdown outside the app instead of navigating. */
async function handleMarkdownClick(event: MouseEvent<HTMLDivElement>) {
  const target = event.target;
  if (!(target instanceof Element)) return;
  const anchor = target.closest("a[href]");
  if (!(anchor instanceof HTMLAnchorElement)) return;
  event.preventDefault();
  try {
    await openExternalUrl(anchor.href);
  } catch (e) {
    await logWarn(`[StepCard] failed to open link ${anchor.href}: ${e}`);
  }
}

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
  const isInteractable = isPresent && executionActive;
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

  function toggleCheckbox(checkboxId: string, checked: boolean) {
    return onaction({
      action: "toggle_checkbox",
      step_id: stepId,
      checkbox_id: checkboxId,
      checked,
    });
  }

  function recordInput(inputId: string, value: string, unit?: string) {
    void onaction({ action: "record_input", step_id: stepId, input_id: inputId, value, unit });
  }

  function attachFiles(inputId: string, files: AttachmentSource[]) {
    void onaction({ action: "add_attachments", step_id: stepId, input_id: inputId, files });
  }

  function clearInput(inputId: string) {
    void onaction({
      action: "clear_input",
      step_id: stepId,
      input_id: inputId,
      reason: "Cleared by operator",
    });
  }

  function removeAttachmentFile(inputId: string, path: string) {
    void onaction({ action: "remove_attachment_file", step_id: stepId, input_id: inputId, path });
  }

  function clearAttachments(inputId: string) {
    void onaction({ action: "clear_attachments", step_id: stepId, input_id: inputId });
  }

  async function importDropPointUpload(inputId: string, sessionId: string) {
    const summary = await api.importAttachmentDropPointUpload(
      executionId,
      stepId,
      inputId,
      sessionId,
    );
    ondropimported?.(summary);
  }

  function addNote(text: string) {
    void onaction({ action: "add_note", text, step_id: stepId });
  }

  function removeNote(noteId: string) {
    void onaction({ action: "remove_note", note_id: noteId, reason: "Removed by operator" });
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

      {stepSummary.content.map((block, index) => {
        switch (block.type) {
          case "Prose":
            return (
              <div
                key={index}
                className={c("step-description")}
                onClick={handleMarkdownClick}
                dangerouslySetInnerHTML={{ __html: renderMarkdown(block.text) }}
              />
            );
          case "Checkbox":
            return (
              <CheckboxItem
                key={block.id ?? index}
                checkbox={block}
                disabled={!isInteractable}
                ontoggle={toggleCheckbox}
              />
            );
          case "InputBlock":
            return (
              <div key={index} className={c("step-section")}>
                {block.inputs.map((input) => {
                  const inputId = input.definition.id;
                  if (input.definition.type === "attachment") {
                    return (
                      <AttachmentField
                        key={inputId}
                        definition={input.definition}
                        attachments={input.attachments}
                        disabled={!isInteractable}
                        dropPointEnabled={dropPointEnabled && isInteractable}
                        onattach={(files) => attachFiles(inputId, files)}
                        onpick={() => api.pickAttachmentSources(input.definition.label)}
                        onremovefile={
                          isInteractable ? (path) => removeAttachmentFile(inputId, path) : undefined
                        }
                        onclear={isInteractable ? () => clearAttachments(inputId) : undefined}
                        onpreview={(path) => api.getAttachmentPreviewDataUrl(executionId, path)}
                        onstartdrop={() =>
                          api.startAttachmentDropPointSession(executionId, stepId, inputId)
                        }
                        onpolldrop={api.pollAttachmentDropPointSession}
                        onimportdrop={(sessionId) => importDropPointUpload(inputId, sessionId)}
                        oncanceldrop={api.cancelAttachmentDropPointSession}
                      />
                    );
                  }
                  return (
                    <InputField
                      key={inputId}
                      definition={input.definition}
                      recorded={input.recorded}
                      disabled={!isInteractable}
                      onrecord={recordInput}
                      onrevert={
                        input.recorded && executionActive ? () => clearInput(inputId) : undefined
                      }
                    />
                  );
                })}
              </div>
            );
        }
      })}

      <div className={c("step-section")}>
        <NoteEditor
          notes={stepSummary.notes}
          disabled={!isInteractable}
          onadd={addNote}
          onrevert={executionActive ? removeNote : undefined}
        />
      </div>

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
