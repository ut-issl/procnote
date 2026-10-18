import { describe, expect, it } from "vite-plus/test";
import { verifyToken } from "./token.js";

// Issued by procnote-server's collab::issue_token (see KNOWN_TOKEN there).
const KNOWN_TOKEN =
  "eyJkb2MiOiJkb2MtMSIsInN1YiI6Im9wZXJhdG9yQGV4YW1wbGUuYWMuanAiLCJleHAiOjQxMDI0NDQ4MDB9.6_FwxfiXy8ujUSIeVjceHXnMRdduoe4fENSCDCEtyyg";
const NOW = 1_800_000_000;

describe("verifyToken", () => {
  it("accepts a token the API server issued", () => {
    expect(verifyToken("test-secret", KNOWN_TOKEN, "doc-1", NOW)).toEqual({
      doc: "doc-1",
      sub: "operator@example.ac.jp",
      exp: 4_102_444_800,
    });
  });

  it.each([
    ["another secret", "other-secret", KNOWN_TOKEN, "doc-1", NOW],
    ["another document", "test-secret", KNOWN_TOKEN, "doc-2", NOW],
    ["an expired token", "test-secret", KNOWN_TOKEN, "doc-1", 4_102_444_800],
    ["a tampered payload", "test-secret", `x${KNOWN_TOKEN}`, "doc-1", NOW],
    ["garbage", "test-secret", "not-a-token", "doc-1", NOW],
  ])("rejects %s", (_, secret, token, doc, now) => {
    expect(() => verifyToken(secret, token, doc, now)).toThrow();
  });
});
