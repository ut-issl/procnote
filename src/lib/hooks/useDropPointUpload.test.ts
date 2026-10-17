import { expect, test } from "vite-plus/test";
import { formatExpiryCountdown, parsePollError } from "./useDropPointUpload.js";

const expiresAt = "2026-10-17T12:00:00Z";
const at = (iso: string) => Date.parse(iso);

test.each([
  ["2026-10-17T11:58:55Z", "1:05 remaining"],
  ["2026-10-17T10:59:59Z", "1:00:01 remaining"],
  ["2026-10-17T11:59:59.500Z", "0:01 remaining"],
  ["2026-10-17T12:00:00Z", "expired"],
  ["2026-10-17T12:05:00Z", "expired"],
])("formatExpiryCountdown at %s", (now, expected) => {
  expect(formatExpiryCountdown(expiresAt, at(now))).toBe(expected);
});

test("formatExpiryCountdown rejects unparsable timestamps", () => {
  expect(formatExpiryCountdown("not a date", 0)).toBe("countdown unavailable");
});

test("parsePollError keeps structured errors", () => {
  expect(parsePollError({ kind: "retryable", message: "timeout" })).toEqual({
    kind: "retryable",
    message: "timeout",
  });
});

test.each([["boom"], [{ kind: 1, message: "x" }], [null]])(
  "parsePollError treats %j as fatal",
  (error) => {
    expect(parsePollError(error).kind).toBe("fatal");
  },
);
