import type { MouseEvent } from "react";
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
import AttachmentField from "./AttachmentField.js";
import CheckboxItem from "./CheckboxItem.js";
import InputField from "./InputField.js";
import NoteEditor from "./NoteEditor.js";
import styles from "./StepBody.module.css";

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
    await logWarn(`[StepBody] failed to open link ${anchor.href}: ${e}`);
  }
}

/**
 * A step's content (prose, checkboxes, inputs, attachments) and notes, wired
 * to execution actions. Shared by the list view's StepCard and the focus view.
 */
export default function StepBody({
  stepSummary,
  executionId,
  executionActive = false,
  dropPointEnabled = false,
  noteInput = true,
  onaction,
  ondropimported,
}: {
  stepSummary: StepSummary;
  executionId: string;
  executionActive?: boolean;
  dropPointEnabled?: boolean;
  /** Show the note input under the notes (the focus view has its own). */
  noteInput?: boolean;
  onaction: (action: ExecutionAction) => Promise<boolean> | boolean;
  ondropimported?: (summary: ExecutionSummary) => void;
}) {
  const isInteractable = stepSummary.status === "present" && executionActive;
  const stepId = stepSummary.id;

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

  function unassignAttachment(inputId: string, path: string) {
    void onaction({ action: "unassign_attachment", step_id: stepId, input_id: inputId, path });
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

  return (
    <>
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
                        onunassign={
                          isInteractable ? (path) => unassignAttachment(inputId, path) : undefined
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
          disabled={!isInteractable || !noteInput}
          onadd={addNote}
          onrevert={executionActive ? removeNote : undefined}
        />
      </div>
    </>
  );
}
