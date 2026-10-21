import { Link } from "react-router";
import type { CatalogEntry } from "#lib/api/commands.js";
import { catalogBadges, templatePagePath } from "#lib/templates.js";
import { scoped } from "#lib/utils/cx.js";
import { formatTimestamp } from "#lib/utils/format.js";
import styles from "./TemplateList.module.css";

const c = scoped(styles);

/** The procedures' templates, each opening its template page (where executions start). */
export default function TemplateList({ entries }: { entries: CatalogEntry[] }) {
  return (
    <div className={c("template-list")}>
      {entries.map((entry) => (
        <Link
          className={c("template-card")}
          key={entry.procedure_id}
          to={templatePagePath(entry.procedure_id)}
        >
          <div className={c("template-info")}>
            <span className={c("template-title")}>{entry.title || "Untitled"}</span>
            <span className={c("template-meta")}>
              {entry.procedure_id} &middot; v{entry.version}
              {entry.shared && <> &middot; updated {formatTimestamp(entry.shared.updated_at)}</>}
            </span>
          </div>
          <TemplateBadges entry={entry} />
        </Link>
      ))}
    </div>
  );
}

export function TemplateBadges({ entry }: { entry: CatalogEntry }) {
  return (
    <span className={c("badges")}>
      {catalogBadges(entry).map((badge) => (
        <span key={badge.text} className={c("badge", badge.warning && "badge-warning")}>
          {badge.text}
        </span>
      ))}
    </span>
  );
}
