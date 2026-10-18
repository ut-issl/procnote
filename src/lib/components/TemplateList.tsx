import type { TemplateSummary } from "#lib/types/index.js";
import { scoped } from "#lib/utils/cx.js";
import styles from "./TemplateList.module.css";

const c = scoped(styles);

export default function TemplateList({
  templates,
  onstart,
  onedit,
  onshare,
}: {
  templates: TemplateSummary[];
  onstart: (t: TemplateSummary) => void;
  onedit?: (t: TemplateSummary) => void;
  /** Copy the template into a shared, collaboratively edited one. */
  onshare?: (t: TemplateSummary) => void;
}) {
  return (
    <div className={c("template-list")}>
      {templates.map((template) => (
        <div className={c("template-card")} key={template.path}>
          <div className={c("template-info")}>
            <span className={c("template-title")}>{template.title}</span>
            <span className={c("template-meta")}>
              {template.id} &middot; v{template.version}
            </span>
          </div>
          {onedit && (
            <button className={c("btn-edit")} onClick={() => onedit(template)}>
              Edit (prototype)
            </button>
          )}
          {onshare && (
            <button className={c("btn-edit")} onClick={() => onshare(template)}>
              Edit together
            </button>
          )}
          <button className={c("btn-start")} onClick={() => onstart(template)}>
            Start
          </button>
        </div>
      ))}
    </div>
  );
}
