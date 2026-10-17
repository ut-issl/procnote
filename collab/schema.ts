// The procnote BlockNote schema for reading and writing template documents in
// Node. Block configs are shared with the browser's schema
// (src/lib/blocknote/schema.tsx), so both see the same documents. Rendering is
// only a plain element: documents are never displayed here, and the Markdown
// conversions run inside server-util's jsdom.
import { BlockNoteSchema, createBlockSpec, defaultBlockSpecs } from "@blocknote/core";
import { inputBlockConfig, stepBlockConfig } from "../src/lib/blocknote/blocks.js";

export const serverSchema = BlockNoteSchema.create({
  blockSpecs: {
    ...defaultBlockSpecs,
    step: createBlockSpec(stepBlockConfig, {
      render: () => {
        const dom = document.createElement("div");
        return { dom, contentDOM: dom };
      },
    })(),
    procnoteInput: createBlockSpec(inputBlockConfig, {
      render: () => ({ dom: document.createElement("div") }),
    })(),
  },
});
