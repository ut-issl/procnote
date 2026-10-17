// BlockNote schema for procnote templates: the default blocks plus a `step`
// block and a `procnoteInput` block. See convert.ts for how they map to
// ProcedureTemplate. All BlockNote-specific code stays under lib/blocknote/.
import { BlockNoteSchema, defaultBlockSpecs } from "@blocknote/core";
import { createReactBlockSpec } from "@blocknote/react";
import { useEffect, useState } from "react";
import { scoped } from "#lib/utils/cx.js";
import { EXPECTED_KINDS, INPUT_TYPES } from "./convert.js";
import styles from "./schema.module.css";

const c = scoped(styles);

export const createStepBlock = createReactBlockSpec(
  { type: "step", propSchema: {}, content: "inline" },
  {
    // A step owns its content: Enter in the heading starts the content, an
    // empty last line leaves the step (to add the next one), and content
    // can't be outdented out of it.
    experimental_keyboard: {
      enter: "into-children",
      emptyChildEnter: "exit-at-end",
      childrenCanOutdent: false,
    },
    render: ({ contentRef }) => (
      <div className={c("step")}>
        <span className={c("step-label")} contentEditable={false}>
          Step
        </span>
        <div className={c("step-heading")} ref={contentRef} />
      </div>
    ),
  },
);

const inputPropSchema = {
  inputId: { default: "" },
  label: { default: "" },
  inputType: { default: "text", values: INPUT_TYPES },
  unit: { default: "" },
  /** One option per line (props cannot hold arrays). */
  options: { default: "" },
  expectedKind: { default: "none", values: EXPECTED_KINDS },
  expectedMin: { default: 0 },
  expectedMax: { default: 0 },
  expectedExact: { default: "" },
} as const;

/** A text field that edits locally and commits to the block on blur. */
function PropField({
  value,
  onCommit,
  placeholder,
  multiline = false,
  className,
}: {
  value: string;
  onCommit: (value: string) => void;
  placeholder: string;
  multiline?: boolean;
  className?: string;
}) {
  const [draft, setDraft] = useState(value);
  useEffect(() => setDraft(value), [value]);
  const commit = () => {
    if (draft !== value) onCommit(draft);
  };
  return multiline ? (
    <textarea
      className={className}
      value={draft}
      placeholder={placeholder}
      rows={3}
      onChange={(e) => setDraft(e.target.value)}
      onBlur={commit}
    />
  ) : (
    <input
      className={className}
      value={draft}
      placeholder={placeholder}
      onChange={(e) => setDraft(e.target.value)}
      onBlur={commit}
      onKeyDown={(e) => {
        if (e.key === "Enter") commit();
      }}
    />
  );
}

export const createInputBlock = createReactBlockSpec(
  { type: "procnoteInput", propSchema: inputPropSchema, content: "none" },
  {
    render: ({ block, editor }) => {
      const props = block.props;
      const update = (patch: Partial<typeof props>) =>
        editor.updateBlock(block, { type: "procnoteInput", props: patch });
      return (
        <div className={c("input-block")} contentEditable={false}>
          <div className={c("input-row")}>
            <span className={c("input-badge")}>Input</span>
            <PropField
              className={c("field-label")}
              value={props.label}
              placeholder="Label"
              onCommit={(label) => update({ label })}
            />
            <select
              value={props.inputType}
              onChange={(e) =>
                update({ inputType: e.target.value as (typeof INPUT_TYPES)[number] })
              }
            >
              {INPUT_TYPES.map((t) => (
                <option key={t} value={t}>
                  {t}
                </option>
              ))}
            </select>
          </div>
          <div className={c("input-row")}>
            <label className={c("field")}>
              ID
              <PropField
                value={props.inputId}
                placeholder="input-id"
                onCommit={(inputId) => update({ inputId })}
              />
            </label>
            {props.inputType === "measurement" && (
              <label className={c("field")}>
                Unit
                <PropField
                  value={props.unit}
                  placeholder="mA"
                  onCommit={(unit) => update({ unit })}
                />
              </label>
            )}
            <label className={c("field")}>
              Expected
              <select
                value={props.expectedKind}
                onChange={(e) =>
                  update({ expectedKind: e.target.value as (typeof EXPECTED_KINDS)[number] })
                }
              >
                {EXPECTED_KINDS.map((k) => (
                  <option key={k} value={k}>
                    {k}
                  </option>
                ))}
              </select>
            </label>
            {props.expectedKind === "range" && (
              <>
                <PropField
                  className={c("narrow")}
                  value={String(props.expectedMin)}
                  placeholder="min"
                  onCommit={(v) => update({ expectedMin: Number(v) })}
                />
                <span>–</span>
                <PropField
                  className={c("narrow")}
                  value={String(props.expectedMax)}
                  placeholder="max"
                  onCommit={(v) => update({ expectedMax: Number(v) })}
                />
              </>
            )}
            {props.expectedKind === "exact" && (
              <PropField
                value={props.expectedExact}
                placeholder="value"
                onCommit={(expectedExact) => update({ expectedExact })}
              />
            )}
          </div>
          {props.inputType === "selection" && (
            <label className={c("field", "options")}>
              Options (one per line)
              <PropField
                multiline
                value={props.options}
                placeholder={"PASS\nFAIL"}
                onCommit={(options) => update({ options })}
              />
            </label>
          )}
        </div>
      );
    },
  },
);

export const procnoteSchema = BlockNoteSchema.create({
  blockSpecs: {
    ...defaultBlockSpecs,
    step: createStepBlock(),
    procnoteInput: createInputBlock(),
  },
});

export type ProcnoteEditor = typeof procnoteSchema.BlockNoteEditor;
