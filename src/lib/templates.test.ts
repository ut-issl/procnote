import { expect, test } from "vite-plus/test";
import type { CatalogEntry } from "#lib/api/commands.js";
import { catalogBadges, templatePagePath } from "./templates.js";

const entry = (overrides: Partial<CatalogEntry>): CatalogEntry => ({
  procedure_id: "TVT-001",
  title: "Thermal Vacuum Test",
  version: "1",
  file: null,
  shared: null,
  differs_from_file: false,
  ...overrides,
});

const shared = { id: "4b4a", updated_at: "2026-10-22T00:00:00Z" };

test.each([
  [{ file: "/p/TVT/template.md" }, ["Workspace file"]],
  [{ shared }, ["Server only"]],
  [{ file: "/p/TVT/template.md", shared }, ["Edited on server"]],
  [
    { file: "/p/TVT/template.md", shared, differs_from_file: true },
    ["Edited on server", "Differs from file"],
  ],
])("badges of %j", (overrides, expected) => {
  expect(catalogBadges(entry(overrides)).map((badge) => badge.text)).toEqual(expected);
});

test("template pages are addressed by procedure ID", () => {
  expect(templatePagePath("A/B 1")).toBe("/templates/A%2FB%201");
});
