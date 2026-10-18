// The collab service against the database at PROCNOTE_TEST_DATABASE_URL, in a
// schema of its own. The tests pass without running when it is unset, except
// on CI, where it must be set.
import { createHmac, randomUUID } from "node:crypto";
import { readFileSync } from "node:fs";
import { HocuspocusProvider } from "@hocuspocus/provider";
import type { Server } from "@hocuspocus/server";
import pg from "pg";
import { afterAll, beforeAll, describe, expect, it } from "vite-plus/test";
import * as Y from "yjs";
import { METADATA_MAP } from "../src/lib/blocknote/blocks.js";
import type { ProcedureTemplate } from "../src/lib/types/index.js";
import { createEditor, documentToTemplate } from "./document.js";
import { COLLAB_PATH, createCollabServer } from "./server.js";

const DATABASE_URL = process.env.PROCNOTE_TEST_DATABASE_URL;
if (!DATABASE_URL && process.env.CI)
  throw new Error("PROCNOTE_TEST_DATABASE_URL must be set on CI");

const SECRET = "collab-test-secret";
const MIGRATION = new URL(
  "../crates/procnote-server/migrations/0002_templates.sql",
  import.meta.url,
);

const TEMPLATE: ProcedureTemplate = {
  metadata: {
    id: "COLLAB-1",
    title: "Collab Test",
    version: "1",
    equipment: [],
    requirement_traces: [],
  },
  steps: [{ heading: "Only step", content: [{ type: "Checkbox", text: "Done", checked: false }] }],
};

/** A token as procnote-server's collab::issue_token makes them. */
function token(doc: string, exp = Date.now() / 1000 + 60, secret = SECRET): string {
  const payload = Buffer.from(JSON.stringify({ doc, sub: "tester@example.ac.jp", exp })).toString(
    "base64url",
  );
  const signature = createHmac("sha256", secret).update(payload).digest("base64url");
  return `${payload}.${signature}`;
}

async function waitFor<T>(check: () => Promise<T | undefined>): Promise<T> {
  for (let attempt = 0; attempt < 100; attempt++) {
    const value = await check();
    if (value !== undefined) return value;
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  throw new Error("timed out");
}

describe.skipIf(!DATABASE_URL)("collab service", () => {
  const schema = `collab_test_${randomUUID().replaceAll("-", "")}`;
  let admin: pg.Pool;
  let pool: pg.Pool;
  let server: Server;
  let base: string;

  beforeAll(async () => {
    admin = new pg.Pool({ connectionString: DATABASE_URL });
    await admin.query(`CREATE SCHEMA ${schema}`);
    pool = new pg.Pool({ connectionString: DATABASE_URL, options: `-c search_path=${schema}` });
    await pool.query(readFileSync(MIGRATION, "utf8"));
    server = createCollabServer(
      { secret: SECRET, port: 0, address: "127.0.0.1", debounce: 50 },
      pool,
    );
    await server.listen();
    base = `127.0.0.1:${server.address.port}`;
  });

  afterAll(async () => {
    await server?.destroy();
    await pool?.end();
    await admin?.query(`DROP SCHEMA ${schema} CASCADE`);
    await admin?.end();
  });

  async function newTemplate(): Promise<string> {
    const id = randomUUID();
    await pool.query("INSERT INTO templates (id) VALUES ($1)", [id]);
    return id;
  }

  function seed(id: string, auth = `Bearer ${SECRET}`) {
    return fetch(`http://${base}/internal/templates/${id}`, {
      method: "POST",
      headers: { Authorization: auth, "Content-Type": "application/json" },
      body: JSON.stringify(TEMPLATE),
    });
  }

  function connect(id: string, accessToken: string) {
    const document = new Y.Doc();
    let failed: string | undefined;
    const provider = new HocuspocusProvider({
      url: `ws://${base}${COLLAB_PATH}`,
      name: id,
      document,
      token: accessToken,
      onAuthenticationFailed: ({ reason }) => {
        failed = reason;
      },
    });
    return { document, provider, failed: () => failed };
  }

  it("serves a seeded template to editors and stores their changes", async () => {
    const id = await newTemplate();
    expect((await seed(id)).status).toBe(200);

    const editor = connect(id, token(id));
    await waitFor(async () => (editor.provider.isSynced ? true : undefined));
    const { template, issues } = await documentToTemplate(createEditor(), editor.document);
    expect(issues).toEqual([]);
    expect(template.steps[0]?.heading).toBe("Only step");

    editor.document.getMap(METADATA_MAP).set("title", "Renamed");
    const stored = await waitFor(async () => {
      const result = await pool.query("SELECT metadata FROM templates WHERE id = $1", [id]);
      return result.rows[0]?.metadata?.title === "Renamed" ? result.rows[0].metadata : undefined;
    });
    editor.provider.destroy();
    expect(stored).toMatchObject({ id: "COLLAB-1", title: "Renamed" });
  });

  it("rejects tokens that are forged, expired, or for another document", async () => {
    const id = await newTemplate();
    await seed(id);

    for (const bad of [
      token(id, undefined, "wrong-secret"),
      token(id, Date.now() / 1000 - 1),
      token(randomUUID()),
    ]) {
      const editor = connect(id, bad);
      await waitFor(async () => editor.failed());
      expect(editor.document.getMap(METADATA_MAP).size).toBe(0);
      editor.provider.destroy();
    }
  });

  it("tells procnote-server what a document describes and what keeps it from running", async () => {
    const id = await newTemplate();
    await seed(id);
    const get = () =>
      fetch(`http://${base}/internal/templates/${id}/template`, {
        headers: { Authorization: `Bearer ${SECRET}` },
      }).then((response) => response.json());

    const runnable = await get();
    const editor = connect(id, token(id));
    await waitFor(async () => (editor.provider.isSynced ? true : undefined));
    editor.document.getMap(METADATA_MAP).set("title", "");
    // Give the change time to reach the service.
    await new Promise((resolve) => setTimeout(resolve, 200));
    const broken = await get();
    editor.provider.destroy();

    expect(runnable).toEqual({ template: TEMPLATE, issues: [] });
    expect(broken.issues).toEqual(["The procedure has no title."]);
  });

  it("seeds only with the shared secret, and only once", async () => {
    const id = await newTemplate();

    expect((await seed(id, "Bearer wrong")).status).toBe(401);
    expect((await seed(id)).status).toBe(200);
    expect((await seed(id)).status).toBe(409);
  });
});
