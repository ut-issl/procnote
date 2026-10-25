import { type ChangeEvent, type CSSProperties, useEffect, useRef, useState } from "react";
import { renderInlineMarkdown } from "#lib/markdown.js";
import { isOutOfRange } from "#lib/run.js";
import type {
  InputDefinitionSummary,
  StepContentSummary,
  TableAlignment,
} from "#lib/types/index.js";
import { scoped } from "#lib/utils/cx.js";
import { formatTimestamp } from "#lib/utils/format.js";
import { isNonComposingEnter } from "#lib/utils/keyboard.js";
import TrashIcon from "./TrashIcon.js";
import styles from "./TableBlock.module.css";

const c = scoped(styles);

type TableContent = Extract<StepContentSummary, { type: "Table" }>;

function alignStyle(alignment: TableAlignment | undefined): CSSProperties | undefined {
  return alignment && alignment !== "none" ? { textAlign: alignment } : undefined;
}

function expectedHint(input: InputDefinitionSummary): string | undefined {
  const { expected, unit } = input.definition;
  // Absent in the generated type, but `null` in JSON from the backend.
  if (expected == null) return undefined;
  if (typeof expected === "string") return expected;
  return `${expected.min} – ${expected.max}${unit ? ` ${unit}` : ""}`;
}

/** An input cell: a compact field until recorded, then the value. */
function InputCell({
  input,
  disabled,
  onrecord,
  onrevert,
}: {
  input: InputDefinitionSummary;
  disabled: boolean;
  onrecord: (inputId: string, value: string, unit?: string) => void;
  onrevert?: (inputId: string) => void;
}) {
  const { definition, recorded } = input;
  const [value, setValue] = useState(recorded?.value ?? "");
  useEffect(() => setValue(recorded?.value ?? ""), [recorded?.value]);
  // Submit once until the recorded value arrives (or the record is cleared).
  const submitted = useRef(false);
  useEffect(() => {
    submitted.current = false;
  }, [recorded]);
  const hint = expectedHint(input);

  function submit(next: string = value) {
    const trimmed = next.trim();
    if (!trimmed || submitted.current) return;
    submitted.current = true;
    onrecord(definition.id, trimmed, definition.unit);
  }

  if (recorded) {
    const unit = recorded.unit ?? definition.unit;
    return (
      <span
        className={c(
          "recorded",
          isOutOfRange(definition.expected, recorded.value) && "out-of-range",
        )}
        title={[
          definition.label,
          hint && `Expected: ${hint}`,
          recorded.at && `Recorded ${formatTimestamp(recorded.at)}`,
        ]
          .filter(Boolean)
          .join("\n")}
      >
        <span className={c("value")}>
          {recorded.value}
          {unit && ` ${unit}`}
        </span>
        {onrevert && (
          <button
            className={c("btn-delete")}
            title="Delete recorded value"
            onClick={() => onrevert(definition.id)}
          >
            <TrashIcon />
          </button>
        )}
      </span>
    );
  }
  if (disabled) return <span className={c("empty")}>—</span>;

  return (
    <span className={c("field")}>
      {definition.type === "selection" ? (
        <select
          aria-label={definition.label}
          value={value}
          onChange={(e) => {
            setValue(e.target.value);
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
        <>
          <input
            aria-label={definition.label}
            type={definition.type === "measurement" ? "number" : "text"}
            value={value}
            placeholder={hint ?? definition.label}
            title={hint ? `${definition.label} (expected: ${hint})` : definition.label}
            onChange={(e) => setValue(e.target.value)}
            onKeyDown={(e) => {
              if (isNonComposingEnter(e.nativeEvent)) submit();
            }}
          />
          {definition.unit && <span className={c("unit")}>{definition.unit}</span>}
          {value.trim() && (
            <button className={c("btn-record")} onClick={() => submit()}>
              Record
            </button>
          )}
        </>
      )}
    </span>
  );
}

/** A checkbox cell; reflects the click at once and falls back if rejected. */
function CheckboxCell({
  id,
  checked: recordedChecked,
  at,
  disabled,
  ontoggle,
}: {
  id: string;
  checked: boolean;
  at?: string;
  disabled: boolean;
  ontoggle: (checkboxId: string, checked: boolean) => Promise<boolean> | boolean;
}) {
  const [checked, setChecked] = useState(recordedChecked);
  useEffect(() => setChecked(recordedChecked), [recordedChecked]);

  async function handleChange(event: ChangeEvent<HTMLInputElement>) {
    const next = event.currentTarget.checked;
    setChecked(next);
    if (!(await ontoggle(id, next))) setChecked(recordedChecked);
  }

  return (
    <input
      type="checkbox"
      className={c("checkbox")}
      checked={checked}
      disabled={disabled}
      title={at ? `Toggled ${formatTimestamp(at)}` : undefined}
      onChange={handleChange}
    />
  );
}

/** A Markdown table whose input and checkbox cells are filled during execution. */
export default function TableBlock({
  table,
  disabled,
  onrecord,
  onrevert,
  ontoggle,
}: {
  table: TableContent;
  disabled: boolean;
  onrecord: (inputId: string, value: string, unit?: string) => void;
  onrevert?: (inputId: string) => void;
  ontoggle: (checkboxId: string, checked: boolean) => Promise<boolean> | boolean;
}) {
  return (
    <div className={c("table-wrap")}>
      <table className={c("table")}>
        <thead>
          <tr>
            {table.header.map((text, column) => (
              <th
                key={column}
                style={alignStyle(table.alignments[column])}
                dangerouslySetInnerHTML={{ __html: renderInlineMarkdown(text) }}
              />
            ))}
          </tr>
        </thead>
        <tbody>
          {table.rows.map((row, rowIndex) => (
            <tr key={rowIndex}>
              {row.map((cell, column) => {
                const style = alignStyle(table.alignments[column]);
                switch (cell.type) {
                  case "Text":
                    return (
                      <td
                        key={column}
                        style={style}
                        dangerouslySetInnerHTML={{ __html: renderInlineMarkdown(cell.text) }}
                      />
                    );
                  case "Input":
                    return (
                      <td key={column} style={style}>
                        <InputCell
                          input={cell.input}
                          disabled={disabled}
                          onrecord={onrecord}
                          onrevert={onrevert}
                        />
                      </td>
                    );
                  case "Checkbox":
                    return (
                      <td key={column} style={style}>
                        <CheckboxCell
                          id={cell.id}
                          checked={cell.checked}
                          at={cell.at}
                          disabled={disabled}
                          ontoggle={ontoggle}
                        />
                      </td>
                    );
                }
              })}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
