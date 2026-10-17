import { logDebug, logError } from "#lib/platform.js";
import * as commands from "#lib/api/commands.js";
import { newActionMeta } from "#lib/operator.js";
import type { ActionMeta, ExecutionAction, ExecutionSummary } from "#lib/types/index.js";

export interface ExecutionStoreState {
  summary: ExecutionSummary | null;
  loading: boolean;
  error: string | null;
}

/** Backend operations the store needs; injectable for tests. */
export interface ExecutionApi {
  startExecution(templatePath: string, meta: ActionMeta): Promise<ExecutionSummary>;
  startSharedExecution(templateId: string, meta: ActionMeta): Promise<ExecutionSummary>;
  getExecutionState(executionId: string): Promise<ExecutionSummary>;
  recordAction(
    executionId: string,
    action: ExecutionAction,
    meta: ActionMeta,
  ): Promise<ExecutionSummary>;
  watchExecution(executionId: string, onSeq: (seq: number) => void): () => void;
}

export function isFinished(summary: ExecutionSummary | null): boolean {
  const s = summary?.status;
  return s === "pass" || s === "fail" || s === "aborted";
}

/**
 * Framework-independent store for the execution being viewed.
 *
 * State is replaced (never mutated) on every change so UI bindings can compare
 * snapshots by identity. Actions are queued so they reach the backend in the
 * order the operator performed them.
 */
export class ExecutionStore {
  #state: ExecutionStoreState = { summary: null, loading: false, error: null };
  #listeners = new Set<() => void>();
  #actionQueue: Promise<boolean> = Promise.resolve(true);
  #refreshing = false;
  #refreshAgain = false;
  readonly #api: ExecutionApi;
  readonly #meta: () => ActionMeta;

  /** `meta` supplies who is acting, with a fresh event ID per call. */
  constructor(api: ExecutionApi = commands, meta: () => ActionMeta = newActionMeta) {
    this.#api = api;
    this.#meta = meta;
  }

  getState = (): ExecutionStoreState => this.#state;

  subscribe = (listener: () => void): (() => void) => {
    this.#listeners.add(listener);
    return () => this.#listeners.delete(listener);
  };

  #set(patch: Partial<ExecutionStoreState>) {
    this.#state = { ...this.#state, ...patch };
    for (const listener of this.#listeners) listener();
  }

  async start(templatePath: string): Promise<ExecutionSummary | null> {
    return this.#fetchSummary(() => this.#api.startExecution(templatePath, this.#meta()));
  }

  /** Start an execution of a shared (collaboratively edited) template. */
  async startShared(templateId: string): Promise<ExecutionSummary | null> {
    return this.#fetchSummary(() => this.#api.startSharedExecution(templateId, this.#meta()));
  }

  async load(executionId: string): Promise<ExecutionSummary | null> {
    return this.#fetchSummary(() => this.#api.getExecutionState(executionId));
  }

  async #fetchSummary(fetch: () => Promise<ExecutionSummary>): Promise<ExecutionSummary | null> {
    this.#set({ loading: true, error: null, summary: null });
    try {
      const summary = await fetch();
      this.#set({ summary, loading: false });
      return summary;
    } catch (e) {
      this.#set({ error: String(e), loading: false });
      return null;
    }
  }

  /**
   * Take a summary from the backend unless it is older than the one held:
   * responses can arrive out of order with live updates.
   */
  #accept(next: ExecutionSummary) {
    const current = this.#state.summary;
    if (current && current.execution_id === next.execution_id && next.seq < current.seq) return;
    this.#set({ summary: next });
  }

  /** Refetch the current execution; overlapping requests are coalesced. */
  async refresh(): Promise<void> {
    if (this.#refreshing) {
      this.#refreshAgain = true;
      return;
    }
    this.#refreshing = true;
    try {
      do {
        this.#refreshAgain = false;
        const current = this.#state.summary;
        if (!current) return;
        try {
          this.#accept(await this.#api.getExecutionState(current.execution_id));
        } catch (e) {
          void logError(`[ExecutionStore] refresh error: ${e}`);
        }
      } while (this.#refreshAgain);
    } finally {
      this.#refreshing = false;
    }
  }

  /**
   * Follow changes other clients make to `executionId`. Returns a function
   * that stops following.
   */
  watch(executionId: string): () => void {
    return this.#api.watchExecution(executionId, (seq) => {
      const current = this.#state.summary;
      if (current?.execution_id === executionId && seq > current.seq) void this.refresh();
    });
  }

  async act(action: ExecutionAction): Promise<boolean> {
    const run = async (): Promise<boolean> => {
      const current = this.#state.summary;
      if (!current) return false;
      this.#set({ error: null });
      try {
        void logDebug(`[ExecutionStore] act: ${JSON.stringify(action)}`);
        const result = await this.#api.recordAction(current.execution_id, action, this.#meta());
        void logDebug(
          `[ExecutionStore] act result: steps=${result.steps.length} status=${result.status}`,
        );
        this.#accept(result);
        return true;
      } catch (e) {
        void logError(`[ExecutionStore] act error: ${e}`);
        this.#set({ error: String(e) });
        return false;
      }
    };

    const queued = this.#actionQueue.then(run, run);
    this.#actionQueue = queued.catch(() => false);
    return queued;
  }

  setSummary(summary: ExecutionSummary) {
    this.#set({ summary, error: null });
  }

  setError(error: string | null) {
    this.#set({ error });
  }

  reset() {
    this.#set({ summary: null, loading: false, error: null });
  }
}

export const executionStore = new ExecutionStore();
