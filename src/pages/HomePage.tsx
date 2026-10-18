import { useEffect, useMemo, useState } from "react";
import { useNavigate } from "react-router";
import * as api from "#lib/api/commands.js";
import TemplateList from "#lib/components/TemplateList.js";
import { logWarn } from "#lib/platform.js";
import { executionStore } from "#lib/stores/execution.js";
import { useExecutionStore } from "#lib/stores/useExecutionStore.js";
import type { ExecutionSummary, TemplateSummary } from "#lib/types/index.js";
import { scoped } from "#lib/utils/cx.js";
import { formatTimestamp } from "#lib/utils/format.js";
import styles from "./HomePage.module.css";

const c = scoped(styles);

export default function HomePage() {
  const navigate = useNavigate();
  const store = useExecutionStore();
  const [templates, setTemplates] = useState<TemplateSummary[]>([]);
  const [executions, setExecutions] = useState<ExecutionSummary[]>([]);
  const [loadingTemplates, setLoadingTemplates] = useState(true);
  const [loadingExecutions, setLoadingExecutions] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [selectedProcedure, setSelectedProcedure] = useState("all");
  const [shared, setShared] = useState<api.CollabTemplateSummary[] | null>(null);

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const loaded = await api.listTemplates();
        if (!cancelled) setTemplates(loaded);
      } catch (e) {
        if (!cancelled) setError(String(e));
      } finally {
        if (!cancelled) setLoadingTemplates(false);
      }

      try {
        const loaded = await api.listCollabTemplates();
        if (!cancelled) setShared(loaded);
      } catch (e) {
        void logWarn(`[home] listCollabTemplates failed: ${e}`);
      }

      try {
        const loaded = await api.listExecutions();
        if (!cancelled) setExecutions(loaded);
      } catch (e) {
        void logWarn(`[home] listExecutions failed: ${e}`);
      } finally {
        if (!cancelled) setLoadingExecutions(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  const procedures = useMemo(
    () =>
      [...new Map(executions.map((e) => [e.procedure_id, e.procedure_title])).entries()]
        .map(([id, title]) => ({ id, title }))
        .sort((a, b) => a.title.localeCompare(b.title)),
    [executions],
  );

  // Most recent first; executions without started_at sort last.
  const filteredExecutions = useMemo(() => {
    const sorted = [...executions].sort((a, b) =>
      (b.started_at ?? "").localeCompare(a.started_at ?? ""),
    );
    return selectedProcedure === "all"
      ? sorted
      : sorted.filter((e) => e.procedure_id === selectedProcedure);
  }, [executions, selectedProcedure]);

  async function handleShare(template: TemplateSummary) {
    try {
      const id = await api.createCollabTemplate(template.path);
      navigate(`/templates/shared/${id}`);
    } catch (e) {
      setError(String(e));
    }
  }

  async function handleStartShared(templateId: string) {
    const started = await executionStore.startShared(templateId);
    if (started) navigate(`/execution/${started.execution_id}`);
  }

  async function handleStart(template: TemplateSummary) {
    const started = await executionStore.start(template.path);
    if (started) navigate(`/execution/${started.execution_id}`);
  }

  return (
    <div className={c("home")}>
      <section className={c("section")}>
        <h2 className={c("section-title")}>Procedure Templates</h2>
        {store.error && <p className={c("error")}>{store.error}</p>}
        {loadingTemplates ? (
          <p className={c("loading")}>Loading templates...</p>
        ) : error ? (
          <p className={c("error")}>{error}</p>
        ) : templates.length === 0 ? (
          <p className={c("empty")}>
            No procedure templates found. Place <code>.md</code> files in the procedures directory.
          </p>
        ) : (
          <TemplateList
            templates={templates}
            onstart={handleStart}
            onedit={(t) => navigate(`/templates/edit?${new URLSearchParams({ path: t.path })}`)}
            onshare={shared ? handleShare : undefined}
          />
        )}
      </section>

      {shared && (
        <section className={c("section")}>
          <h2 className={c("section-title")}>Shared Templates</h2>
          {shared.length === 0 ? (
            <p className={c("empty")}>
              No shared templates yet. Use <em>Edit together</em> on a template above.
            </p>
          ) : (
            <div className={c("execution-list")}>
              {shared.map((template) => (
                <div key={template.id} className={c("shared-card")}>
                  <button
                    className={c("execution-card")}
                    onClick={() => navigate(`/templates/shared/${template.id}`)}
                  >
                    <div className={c("exec-header")}>
                      <span className={c("exec-name")}>{template.title || "Untitled"}</span>
                    </div>
                    <div className={c("exec-meta")}>
                      <span>
                        {template.procedure_id} v{template.version}
                      </span>
                      <span className={c("exec-time")}>
                        Updated {formatTimestamp(template.updated_at)}
                      </span>
                    </div>
                  </button>
                  <button
                    className="btn btn-primary"
                    onClick={() => handleStartShared(template.id)}
                  >
                    Start
                  </button>
                </div>
              ))}
            </div>
          )}
        </section>
      )}

      {!loadingExecutions && executions.length > 0 && (
        <section className={c("section")}>
          <div className={c("section-header")}>
            <h2 className={c("section-title")}>Recent Executions</h2>
            {procedures.length > 1 && (
              <select
                value={selectedProcedure}
                onChange={(e) => setSelectedProcedure(e.target.value)}
                className={c("procedure-filter")}
              >
                <option value="all">All procedures</option>
                {procedures.map((proc) => (
                  <option value={proc.id} key={proc.id}>
                    {proc.title} ({proc.id})
                  </option>
                ))}
              </select>
            )}
          </div>
          <div className={c("execution-list")}>
            {filteredExecutions.map((exec) => (
              <button
                key={exec.execution_id}
                className={c("execution-card", exec.status === "active" && "execution-active")}
                onClick={() => navigate(`/execution/${exec.execution_id}`)}
              >
                <div className={c("exec-header")}>
                  <span className={c("exec-name")}>{exec.name ?? exec.procedure_id}</span>
                  <span
                    className={c(
                      "exec-status",
                      exec.status === "active" && "status-active",
                      exec.status === "pass" && "status-pass",
                      exec.status === "fail" && "status-fail",
                      exec.status === "aborted" && "status-aborted",
                    )}
                  >
                    {exec.status}
                  </span>
                </div>
                <div className={c("exec-meta")}>
                  <span>
                    {exec.procedure_title} ({exec.procedure_id}) v{exec.procedure_version}
                  </span>
                  {exec.finished_at && exec.started_at ? (
                    <span className={c("exec-time")}>
                      {formatTimestamp(exec.started_at)} — {formatTimestamp(exec.finished_at)}
                    </span>
                  ) : (
                    exec.started_at && (
                      <span className={c("exec-time")}>
                        Started {formatTimestamp(exec.started_at)}
                      </span>
                    )
                  )}
                </div>
              </button>
            ))}
          </div>
        </section>
      )}
    </div>
  );
}
