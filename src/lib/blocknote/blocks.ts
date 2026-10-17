// Block configs of procnote's BlockNote schema, without any rendering, so the
// browser (React, schema.tsx) and the collab service (Node, collab/schema.ts)
// build the same ProseMirror schema and read the same Yjs documents.
import { EXPECTED_KINDS, INPUT_TYPES } from "./convert.js";

/** A procedure step: its heading is the inline content, its children the step content. */
export const stepBlockConfig = { type: "step", propSchema: {}, content: "inline" } as const;

export const inputPropSchema = {
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

/** One input field; consecutive ones form an InputBlock. */
export const inputBlockConfig = {
  type: "procnoteInput",
  propSchema: inputPropSchema,
  content: "none",
} as const;

// Names inside each template's Y.Doc.

/** The XML fragment holding the template's blocks. */
export const DOCUMENT_FRAGMENT = "blocks";
/** The map holding the template's ProcedureMetadata fields. */
export const METADATA_MAP = "metadata";
