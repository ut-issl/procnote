// The collab service: Hocuspocus serving template documents to editors at
// /collab, stored in PostgreSQL's `templates` table (created by
// procnote-server's migrations), plus an internal API for procnote-server.
import { timingSafeEqual } from "node:crypto";
import type { IncomingMessage, ServerResponse } from "node:http";
import { Database } from "@hocuspocus/extension-database";
import { Server } from "@hocuspocus/server";
import type pg from "pg";
import * as Y from "yjs";
import type { ProcedureTemplate } from "../src/lib/types/index.js";
import { metadataIssues } from "../src/lib/blocknote/convert.js";
import {
  createEditor,
  documentMetadata,
  documentToTemplate,
  type Editor,
  isEmptyDocument,
  keepProcedureId,
  templateToUpdate,
} from "./document.js";
import { verifyToken } from "./token.js";

/** Where editors connect. */
export const COLLAB_PATH = "/collab";
const INTERNAL_TEMPLATES = "/internal/templates/";

export interface CollabServerConfig {
  /** Shared with procnote-server: signs tokens and guards the internal API. */
  secret: string;
  port: number;
  address: string;
  /** Milliseconds to wait for more changes before storing (Hocuspocus `debounce`). */
  debounce?: number;
}

export interface ConnectionContext {
  /** The authenticated user (an email), or "" without authentication. */
  user: string;
}

/** An error the internal API reports with a status code. */
class HttpError extends Error {
  constructor(
    readonly status: number,
    message: string,
  ) {
    super(message);
  }
}

function pathOf(request: IncomingMessage): string {
  return new URL(request.url ?? "/", "http://localhost").pathname;
}

function hasSecret(request: IncomingMessage, secret: string): boolean {
  const given = Buffer.from(request.headers.authorization ?? "");
  const expected = Buffer.from(`Bearer ${secret}`);
  return given.length === expected.length && timingSafeEqual(given, expected);
}

async function readJson(request: IncomingMessage): Promise<unknown> {
  const chunks: Buffer[] = [];
  for await (const chunk of request) chunks.push(chunk as Buffer);
  return JSON.parse(Buffer.concat(chunks).toString("utf8"));
}

function respond(response: ServerResponse, status: number, body: unknown) {
  response.writeHead(status, { "Content-Type": "application/json" });
  response.end(JSON.stringify(body));
}

export function createCollabServer(config: CollabServerConfig, pool: pg.Pool): Server {
  const editor = createEditor();

  const server: Server = new Server({
    name: "procnote-collab",
    port: config.port,
    address: config.address,
    quiet: true,
    debounce: config.debounce ?? 2000,
    extensions: [
      new Database({
        fetch: async ({ documentName }) => {
          const result = await pool.query<{ state: Buffer | null }>(
            "SELECT state FROM templates WHERE id = $1",
            [documentName],
          );
          const row = result.rows[0];
          if (!row) throw new Error(`no template ${documentName}`);
          return row.state ? new Uint8Array(row.state) : null;
        },
        store: async ({ documentName, state, document }) => {
          await pool.query(
            "UPDATE templates SET state = $2, metadata = $3, updated_at = now() WHERE id = $1",
            [documentName, state, documentMetadata(document)],
          );
        },
      }),
    ],

    async afterLoadDocument({ documentName, document }) {
      const result = await pool.query<{ procedure_id: string }>(
        "SELECT procedure_id FROM templates WHERE id = $1",
        [documentName],
      );
      const row = result.rows[0];
      if (!row) throw new Error(`no template ${documentName}`);
      keepProcedureId(document, row.procedure_id);
    },

    async onUpgrade({ request, socket }) {
      if (pathOf(request) !== COLLAB_PATH) {
        socket.destroy();
        // A falsy rejection stops the upgrade without an error.
        throw null;
      }
    },

    async onAuthenticate({ token, documentName }): Promise<ConnectionContext> {
      const claims = verifyToken(config.secret, token, documentName, Date.now() / 1000);
      return { user: claims.sub };
    },

    async onRequest({ request, response }) {
      try {
        if (!hasSecret(request, config.secret)) throw new HttpError(401, "unauthorized");
        const path = pathOf(request);
        if (!path.startsWith(INTERNAL_TEMPLATES)) throw new HttpError(404, "not found");
        const [id = "", resource, ...rest] = path.slice(INTERNAL_TEMPLATES.length).split("/");
        if (request.method === "POST" && resource === undefined) {
          // POST /internal/templates/{id}: seed an empty document.
          const template = (await readJson(request)) as ProcedureTemplate;
          await seed(server, id, await templateToUpdate(editor, template));
          respond(response, 200, { id });
        } else if (request.method === "GET" && resource === "template" && rest.length === 0) {
          // GET /internal/templates/{id}/template: what the document describes now.
          respond(response, 200, await currentTemplate(server, editor, id));
        } else {
          throw new HttpError(404, "not found");
        }
      } catch (error) {
        const status = error instanceof HttpError ? error.status : 400;
        respond(response, status, {
          error: String(error instanceof Error ? error.message : error),
        });
      }
      // A falsy rejection tells Hocuspocus the request has been answered.
      throw null;
    },
  });
  return server;
}

/**
 * The template a document describes now, and why it can't be executed (an
 * empty list when it can).
 */
async function currentTemplate(server: Server, editor: Editor, id: string) {
  const connection = await server.hocuspocus.openDirectConnection(id, {
    user: "procnote-server",
  } satisfies ConnectionContext);
  try {
    const doc = connection.document;
    if (!doc) throw new Error(`template ${id} could not be opened`);
    const { template, issues } = await documentToTemplate(editor, doc);
    return {
      template,
      issues: [...metadataIssues(documentMetadata(doc)), ...issues].map((issue) => issue.message),
    };
  } finally {
    await connection.disconnect();
  }
}

/** Fill an empty template document with its first content, and store it. */
async function seed(server: Server, id: string, update: Uint8Array) {
  const connection = await server.hocuspocus.openDirectConnection(id, {
    user: "procnote-server",
  } satisfies ConnectionContext);
  try {
    let empty = true;
    await connection.transact((doc) => {
      empty = isEmptyDocument(doc);
      if (empty) Y.applyUpdate(doc, update);
    });
    if (!empty) throw new HttpError(409, `template ${id} already has content`);
  } finally {
    await connection.disconnect();
  }
}
