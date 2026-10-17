// Verification of the collab tokens procnote-server issues
// (crates/procnote-server/src/collab.rs): `base64url(claims).base64url(HMAC-SHA256)`.
import { createHmac, timingSafeEqual } from "node:crypto";

export interface CollabClaims {
  /** The document the token grants access to. */
  doc: string;
  /** Who the server authenticated (an email), or "" without authentication. */
  sub: string;
  /** Expiry, in seconds since the Unix epoch. */
  exp: number;
}

function isClaims(value: unknown): value is CollabClaims {
  if (typeof value !== "object" || value === null) return false;
  const claims = value as Record<string, unknown>;
  return (
    typeof claims.doc === "string" &&
    typeof claims.sub === "string" &&
    typeof claims.exp === "number"
  );
}

/**
 * The claims of `token` if it is signed with `secret`, unexpired at `now`
 * (seconds since the Unix epoch), and for `documentName`; throws otherwise.
 */
export function verifyToken(
  secret: string,
  token: string,
  documentName: string,
  now: number,
): CollabClaims {
  const [payload, signature, ...rest] = token.split(".");
  if (!payload || !signature || rest.length > 0) throw new Error("malformed token");
  const expected = createHmac("sha256", secret).update(payload).digest();
  const actual = Buffer.from(signature, "base64url");
  if (actual.length !== expected.length || !timingSafeEqual(actual, expected)) {
    throw new Error("invalid token signature");
  }
  const claims: unknown = JSON.parse(Buffer.from(payload, "base64url").toString("utf8"));
  if (!isClaims(claims)) throw new Error("malformed token claims");
  if (claims.exp <= now) throw new Error("token expired");
  if (claims.doc !== documentName) throw new Error("token is for another document");
  return claims;
}
