// Template documents: a Y.Doc holding a template's blocks (DOCUMENT_FRAGMENT)
// and its ProcedureMetadata fields (METADATA_MAP), converted to and from
// ProcedureTemplate with the browser's converter (convert.ts).
import type { PartialBlock } from "@blocknote/core";
import { ServerBlockNoteEditor } from "@blocknote/server-util";
import * as Y from "yjs";
import { DOCUMENT_FRAGMENT, METADATA_MAP } from "../src/lib/blocknote/blocks.js";
import {
  blocksToTemplate,
  type ConversionIssue,
  type DocBlock,
  type MarkdownConverter,
  templateToBlocks,
} from "../src/lib/blocknote/convert.js";
import { readTableAlignments, writeTableAlignments } from "../src/lib/blocknote/tables.js";
import type { ProcedureMetadata, ProcedureTemplate } from "../src/lib/types/index.js";
import { serverSchema } from "./schema.js";

export function createEditor() {
  return ServerBlockNoteEditor.create({ schema: serverSchema });
}

export type Editor = ReturnType<typeof createEditor>;

/** Markdown conversion that may be asynchronous, as server-util's is. */
export interface AsyncMarkdownConverter {
  toBlocks(markdown: string): Promise<DocBlock[]>;
  fromBlocks(blocks: DocBlock[]): Promise<string>;
}

/**
 * Run `run` (which needs a synchronous converter) with an asynchronous one:
 * a first pass collects what to convert, the conversions run, and a second
 * pass uses their results.
 */
export async function withAsyncMarkdown<T>(
  convert: AsyncMarkdownConverter,
  run: (md: MarkdownConverter) => T,
): Promise<T> {
  const markdown = new Set<string>();
  const blockRuns = new Map<string, DocBlock[]>();
  run({
    toBlocks: (text) => {
      markdown.add(text);
      return [];
    },
    fromBlocks: (blocks) => {
      blockRuns.set(JSON.stringify(blocks), blocks);
      return "";
    },
  });
  const parsed = new Map<string, DocBlock[]>();
  for (const text of markdown) parsed.set(text, await convert.toBlocks(text));
  const serialized = new Map<string, string>();
  for (const [key, blocks] of blockRuns) serialized.set(key, await convert.fromBlocks(blocks));
  return run({
    toBlocks: (text) => parsed.get(text) ?? [],
    fromBlocks: (blocks) => serialized.get(JSON.stringify(blocks)) ?? "",
  });
}

/** BlockNote's Markdown conversion, with table column alignment carried across. */
function markdownOf(editor: Editor): AsyncMarkdownConverter {
  return {
    toBlocks: async (text) =>
      readTableAlignments(text, (await editor.tryParseMarkdownToBlocks(text)) as DocBlock[]),
    fromBlocks: async (blocks) =>
      writeTableAlignments(
        await editor.blocksToMarkdownLossy(
          blocks as PartialBlock<typeof serverSchema.blockSchema>[],
        ),
        blocks,
      ),
  };
}

/** A Yjs update that creates the document of `template` in an empty Y.Doc. */
export async function templateToUpdate(
  editor: Editor,
  template: ProcedureTemplate,
): Promise<Uint8Array> {
  const blocks = await withAsyncMarkdown(markdownOf(editor), (md) =>
    templateToBlocks(template, md),
  );
  const doc = editor.blocksToYDoc(
    blocks as PartialBlock<typeof serverSchema.blockSchema>[],
    DOCUMENT_FRAGMENT,
  );
  const metadata = doc.getMap(METADATA_MAP);
  for (const [key, value] of Object.entries(template.metadata)) {
    if (value !== undefined && value !== null) metadata.set(key, value);
  }
  return Y.encodeStateAsUpdate(doc);
}

/** Whether `doc` has no template content yet. */
export function isEmptyDocument(doc: Y.Doc): boolean {
  return doc.getXmlFragment(DOCUMENT_FRAGMENT).length === 0 && doc.getMap(METADATA_MAP).size === 0;
}

/** The document's metadata fields as stored (not validated). */
export function documentMetadata(doc: Y.Doc): Record<string, unknown> {
  return doc.getMap(METADATA_MAP).toJSON();
}

/** The template a document describes, with any structural issues. */
export async function documentToTemplate(
  editor: Editor,
  doc: Y.Doc,
): Promise<{ template: ProcedureTemplate; issues: ConversionIssue[] }> {
  const blocks = editor.yDocToBlocks(doc, DOCUMENT_FRAGMENT) as DocBlock[];
  const metadata = documentMetadata(doc) as unknown as ProcedureMetadata;
  return withAsyncMarkdown(markdownOf(editor), (md) => blocksToTemplate(blocks, metadata, md));
}
