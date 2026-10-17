// Entry point of the collab service. Configuration comes from the environment:
// PROCNOTE_DATABASE_URL and PROCNOTE_COLLAB_SECRET (both shared with
// procnote-server), and optionally PROCNOTE_COLLAB_PORT (default 5182) and
// PROCNOTE_COLLAB_ADDRESS (default 127.0.0.1).
import pg from "pg";
import { COLLAB_PATH, createCollabServer } from "./server.js";

function required(name: string): string {
  const value = process.env[name];
  if (!value) throw new Error(`${name} must be set`);
  return value;
}

const pool = new pg.Pool({ connectionString: required("PROCNOTE_DATABASE_URL") });
const server = createCollabServer(
  {
    secret: required("PROCNOTE_COLLAB_SECRET"),
    port: Number(process.env.PROCNOTE_COLLAB_PORT ?? 5182),
    address: process.env.PROCNOTE_COLLAB_ADDRESS ?? "127.0.0.1",
  },
  pool,
);
await server.listen();
console.log(
  `procnote collab service: editors connect to ${COLLAB_PATH} on port ${server.address.port}`,
);
