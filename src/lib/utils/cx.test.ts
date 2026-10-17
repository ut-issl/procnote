import { expect, test } from "vite-plus/test";
import { scoped } from "./cx.js";

test("scoped emits global and module names and skips falsy entries", () => {
  const c = scoped({ card: "card_x1", active: "active_x2" });

  expect(c("card", false, "btn", null, undefined, "", "active")).toBe(
    "card card_x1 btn active active_x2",
  );
});
