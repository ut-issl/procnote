import type { TemplateSummary } from "#lib/types/index.js";
import { scoped } from "#lib/utils/cx.js";
import styles from "./TemplateList.module.css";

const c = scoped(styles);

export default function TemplateList({
  templates,
  onstart,
}: {
  templates: TemplateSummary[];
  onstart: (t: TemplateSummary) => void;
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
          <button className={c("btn-start")} onClick={() => onstart(template)}>
            Start
          </button>
        </div>
      ))}
    </div>
  );
}
