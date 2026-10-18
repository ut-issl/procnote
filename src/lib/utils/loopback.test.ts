import { describe, expect, test } from "vite-plus/test";
import { isLoopbackCallback } from "./loopback.js";

describe("isLoopbackCallback", () => {
  test.each([
    ["http://127.0.0.1:53211/callback", true],
    ["http://localhost:8080/", true],
    ["http://[::1]:9000/cb", true],
    ["https://127.0.0.1/cb", false],
    ["http://127.0.0.1.attacker.example/cb", false],
    ["http://attacker.example/?x=http://127.0.0.1", false],
    ["http://user@127.0.0.1/cb", false],
    ["javascript:alert(1)", false],
    ["not a url", false],
  ])("%s", (callback, allowed) => {
    expect(isLoopbackCallback(callback)).toBe(allowed);
  });
});
