import { useCallback, useEffect, useState } from "react";
import * as api from "#lib/api/commands.js";
import { describeEvent } from "#lib/offline/describe.js";
import { localExecutions } from "#lib/offline/index.js";
import type { LocalExecution } from "#lib/offline/localExecutions.js";
import { newActionMeta } from "#lib/operator.js";
import { isDesktop } from "#lib/platform.js";
import type { Conflict, ExecutionSummary } from "#lib/types/index.js";
import { scoped } from "#lib/utils/cx.js";
import { formatTimestamp } from "#lib/utils/format.js";
import styles from "./ConflictsPanel.module.css";

const c = scoped(styles);

/**
 * Recordings that didn't make it into the execution: events that reached the
 * server after the execution had moved on (conflicts), and events this
 * device couldn't send (rejected). They really happened, so they stay until
 * someone re-records what still matters and dismisses them.
 */
export default function ConflictsPanel({ summary }: { summary: ExecutionSummary }) {
  const id = summary.execution_id;
  const [conflicts, setConflicts] = useState<Conflict[]>([]);
  const [rejected, setRejected] = useState<LocalExecution["rejected"]>([]);
  const [error, setError] = useState<string | null>(null);

  const reload = useCallback(async () => {
    if (isDesktop) {
      setRejected(await api.listRejected(id).catch(() => []));
    } else if (localExecutions) {
      setRejected(await localExecutions.rejected(id));
    } else {
      return;
    }
    try {
      setConflicts(await api.listConflicts(id));
    } catch {
      // Unreachable or not registered yet: nothing new to show.
    }
  }, [id]);

  // Reload whenever the execution changes (a sync may have brought conflicts).
  useEffect(() => {
    void reload();
  }, [reload, summary.seq]);

  async function dismiss(eventId: string) {
    try {
      setConflicts(await api.resolveConflict(id, eventId, newActionMeta()));
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  }

  const open = conflicts.filter((conflict) => !conflict.resolved);
  if ((!isDesktop && !localExecutions) || (open.length === 0 && rejected.length === 0)) return null;

  return (
    <section className={c("conflicts")}>
      <h3>Needs review</h3>
      <p className={c("hint")}>
        These were recorded but not added to the execution, because it had changed by the time they
        arrived. Record again what still applies, then dismiss.
      </p>
      <ul>
        {open.map((conflict) => (
          <li key={conflict.record.event_id}>
            <span className={c("what")}>{describeEvent(conflict.record, summary)}</span>
            <span className={c("meta")}>
              {conflict.record.recorded_by ?? "someone"} at {formatTimestamp(conflict.record.at)} —{" "}
              {conflict.reason}
            </span>
            {conflict.record.event_id && (
              <button
                className="btn btn-secondary"
                onClick={() => dismiss(conflict.record.event_id as string)}
              >
                Dismiss
              </button>
            )}
          </li>
        ))}
        {rejected.map(({ record, reason }) => (
          <li key={record.event_id ?? reason}>
            <span className={c("what")}>{describeEvent(record, summary)}</span>
            <span className={c("meta")}>Couldn't be sent from this device: {reason}</span>
          </li>
        ))}
      </ul>
      {error && <p className={c("error")}>{error}</p>}
    </section>
  );
}
