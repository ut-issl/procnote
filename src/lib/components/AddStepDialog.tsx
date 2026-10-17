import { useState } from "react";
import type { StepContent } from "#lib/types/index.js";
import { scoped } from "#lib/utils/cx.js";
import { isNonComposingEnter } from "#lib/utils/keyboard.js";
import Modal from "./Modal.js";
import styles from "./AddStepDialog.module.css";

const c = scoped(styles);

export default function AddStepDialog({
  steps,
  onconfirm,
  oncancel,
}: {
  steps: { id: string; heading: string }[];
  onconfirm: (
    stepId: string,
    heading: string,
    content: StepContent[],
    afterStepId?: string,
  ) => void;
  oncancel: () => void;
}) {
  const [heading, setHeading] = useState("");
  const [description, setDescription] = useState("");
  const [afterStepId, setAfterStepId] = useState("");

  function submit() {
    if (!heading.trim()) return;
    const stepId = `dyn-step-${crypto.randomUUID().slice(0, 8)}`;
    const content: StepContent[] = description.trim()
      ? [{ type: "Prose", text: description.trim() }]
      : [];
    onconfirm(stepId, heading.trim(), content, afterStepId || undefined);
  }

  return (
    <Modal oncancel={oncancel}>
      <h3>Add Step</h3>
      <p className={c("hint")}>Add a new step to the procedure execution.</p>
      <label className="field">
        <span className="field-label">Step Heading</span>
        <input
          type="text"
          value={heading}
          onChange={(e) => setHeading(e.target.value)}
          placeholder="e.g. Additional Verification"
          autoFocus
          onKeyDown={(e) => {
            if (isNonComposingEnter(e.nativeEvent)) submit();
          }}
        />
      </label>
      <label className="field">
        <span className="field-label">Description (optional)</span>
        <textarea
          value={description}
          onChange={(e) => setDescription(e.target.value)}
          placeholder="Describe what this step involves..."
          rows={2}
        />
      </label>
      <label className="field">
        <span className="field-label">Insert After (optional)</span>
        <select value={afterStepId} onChange={(e) => setAfterStepId(e.target.value)}>
          <option value="">End of procedure</option>
          {steps.map((s) => (
            <option value={s.id} key={s.id}>
              {s.heading}
            </option>
          ))}
        </select>
      </label>
      <div className="modal-actions">
        <button className="btn btn-secondary" onClick={oncancel}>
          Cancel
        </button>
        <button className="btn btn-primary" onClick={submit} disabled={!heading.trim()}>
          Add Step
        </button>
      </div>
    </Modal>
  );
}
