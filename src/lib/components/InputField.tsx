import { useEffect, useState } from "react";
import type { InputDefinition, InputState } from "#lib/types/index.js";
import { scoped } from "#lib/utils/cx.js";
import { formatTimestamp } from "#lib/utils/format.js";
import { isNonComposingEnter } from "#lib/utils/keyboard.js";
import TrashIcon from "./TrashIcon.js";
import styles from "./InputField.module.css";

const c = scoped(styles);

function expectedText(definition: InputDefinition): string | null {
  if (!definition.expected) return null;
  if (typeof definition.expected === "string") {
    return `Expected: ${definition.expected}`;
  }
  return `Expected: ${definition.expected.min} - ${definition.expected.max}${definition.unit ? " " + definition.unit : ""}`;
}

export default function InputField({
  definition,
  recorded,
  disabled = false,
  onrecord,
  onrevert,
}: {
  definition: InputDefinition;
  recorded?: InputState;
  disabled?: boolean;
  onrecord: (inputId: string, value: string, unit?: string) => void;
  onrevert?: () => void;
}) {
  const [inputValue, setInputValue] = useState(recorded?.value ?? "");
  useEffect(() => setInputValue(recorded?.value ?? ""), [recorded?.value]);

  function submit(value: string = inputValue) {
    const val = value.trim();
    if (!val) return;
    onrecord(definition.id, val, definition.unit);
  }

  const isRecorded = !!recorded;
  const inputId = `input-${definition.id}`;
  const expected = expectedText(definition);

  return (
    <div className={c("input-field", isRecorded && "recorded")}>
      <div className={c("input-header")}>
        <label className={c("input-label")} htmlFor={inputId}>
          {definition.label}
        </label>
        {expected && <span className={c("expected")}>{expected}</span>}
      </div>
      <div className={c("input-row")}>
        {definition.type === "selection" ? (
          <select
            id={inputId}
            value={inputValue}
            disabled={disabled || isRecorded}
            onChange={(e) => {
              setInputValue(e.target.value);
              submit(e.target.value);
            }}
          >
            <option value="">Select...</option>
            {definition.options?.map((opt) => (
              <option value={opt} key={opt}>
                {opt}
              </option>
            ))}
          </select>
        ) : (
          <input
            id={inputId}
            type={definition.type === "measurement" ? "number" : "text"}
            value={inputValue}
            onChange={(e) => setInputValue(e.target.value)}
            disabled={disabled || isRecorded}
            placeholder={definition.type === "measurement" ? "0.0" : "Enter value"}
            onKeyDown={(e) => {
              if (isNonComposingEnter(e.nativeEvent)) submit();
            }}
          />
        )}
        {definition.unit && <span className={c("unit")}>{definition.unit}</span>}
        {!isRecorded && definition.type !== "selection" && (
          <button
            className={c("btn-record")}
            onClick={() => submit()}
            disabled={disabled || !inputValue.trim()}
          >
            Record
          </button>
        )}
        {isRecorded && (
          <>
            <span className={c("recorded-badge")}>Recorded</span>
            {recorded?.at && <span className="timestamp">{formatTimestamp(recorded.at)}</span>}
            {onrevert && (
              <button className={c("btn-delete")} title="Delete recorded value" onClick={onrevert}>
                <TrashIcon />
              </button>
            )}
          </>
        )}
      </div>
    </div>
  );
}
