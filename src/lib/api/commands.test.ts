import { afterEach, describe, expect, test, vi } from "vite-plus/test";
import { isUnreachable } from "#lib/offline/localExecutions.js";
import { listTemplates, UnreachableError } from "./commands.js";

function respond(status: number, body: unknown) {
  vi.stubGlobal(
    "fetch",
    vi.fn(async () => new Response(JSON.stringify(body), { status })),
  );
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("reaching the server", () => {
  test("a failed fetch means the server can't be reached", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => {
        throw new TypeError("Failed to fetch");
      }),
    );

    const error = await listTemplates().catch((e: unknown) => e);

    expect(error).toBeInstanceOf(UnreachableError);
    expect(isUnreachable(error)).toBe(true);
  });

  test("a gateway answering for the server means it can't be reached", async () => {
    respond(530, null);

    expect(isUnreachable(await listTemplates().catch((e: unknown) => e))).toBe(true);
  });

  test("procnote-server's own errors are refusals, whatever the status", async () => {
    respond(502, { error: "collab service is unreachable" });

    const error = await listTemplates().catch((e: unknown) => e);

    expect(error).toBe("collab service is unreachable");
    expect(isUnreachable(error)).toBe(false);
  });
});
