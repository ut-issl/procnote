// The template catalog as the home page and the template page show it.
import type { CatalogEntry } from "#lib/api/commands.js";

/** Where a procedure's template page is. */
export function templatePagePath(procedureId: string): string {
  return `/templates/${encodeURIComponent(procedureId)}`;
}

export interface Badge {
  text: string;
  /** Something to look at: the shared template and the workspace file disagree. */
  warning?: boolean;
}

/** Where a procedure's template lives, and whether its copies agree. */
export function catalogBadges(entry: CatalogEntry): Badge[] {
  if (!entry.shared) return [{ text: "Workspace file" }];
  if (!entry.file) return [{ text: "Server only" }];
  return entry.differs_from_file
    ? [{ text: "Edited on server" }, { text: "Differs from file", warning: true }]
    : [{ text: "Edited on server" }];
}
