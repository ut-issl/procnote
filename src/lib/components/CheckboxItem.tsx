import { type ChangeEvent, type CSSProperties, useEffect, useState } from "react";
import type { StepContentSummary } from "#lib/types/index.js";
import { scoped } from "#lib/utils/cx.js";
import { formatTimestamp } from "#lib/utils/format.js";
import styles from "./CheckboxItem.module.css";

const c = scoped(styles);

type CheckboxContent = Extract<StepContentSummary, { type: "Checkbox" }>;

function nestingIndent(level: number): string {
  const safeLevel = Math.min(Math.max(Math.trunc(level), 0), 12);
  return `${safeLevel * 20}px`;
}

export default function CheckboxItem({
  checkbox,
  disabled = false,
  ontoggle,
}: {
  checkbox: CheckboxContent;
  disabled?: boolean;
  ontoggle: (checkboxId: string, checked: boolean) => Promise<boolean> | boolean;
}) {
  // Reflect the click immediately and fall back to the recorded state if the
  // toggle is rejected.
  const [checked, setChecked] = useState(checkbox.checked);
  useEffect(() => setChecked(checkbox.checked), [checkbox.checked]);

  async function handleChange(event: ChangeEvent<HTMLInputElement>) {
    if (!checkbox.id) return;
    const next = event.currentTarget.checked;
    setChecked(next);
    const toggled = await ontoggle(checkbox.id, next);
    if (!toggled) setChecked(checkbox.checked);
  }

  return (
    <label
      className={c("checkbox-item", checkbox.checked && "checked", disabled && "disabled")}
      style={{ "--checkbox-indent": nestingIndent(checkbox.nesting_level) } as CSSProperties}
    >
      <input type="checkbox" checked={checked} disabled={disabled} onChange={handleChange} />
      <span className={c("checkbox-text")}>{checkbox.text}</span>
      {checkbox.at && <span className="timestamp">{formatTimestamp(checkbox.at)}</span>}
    </label>
  );
}
