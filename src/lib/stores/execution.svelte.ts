import { debug, error as logError } from "@tauri-apps/plugin-log";
import * as api from "$lib/api/commands";
import type { ExecutionAction, ExecutionSummary } from "$lib/types";

class ExecutionStore {
  summary: ExecutionSummary | null = $state(null);
  loading = $state(false);
  error: string | null = $state(null);
  #actionQueue: Promise<boolean> = Promise.resolve(true);

  get isActive() {
    return this.summary?.status === "active";
  }

  get isFinished() {
    const s = this.summary?.status;
    return s === "pass" || s === "fail" || s === "aborted";
  }

  async start(templatePath: string): Promise<boolean> {
    this.loading = true;
    this.error = null;
    this.summary = null;
    try {
      this.summary = await api.startExecution(templatePath);
      return true;
    } catch (e) {
      this.error = String(e);
      return false;
    } finally {
      this.loading = false;
    }
  }

  async load(executionId: string): Promise<boolean> {
    this.loading = true;
    this.error = null;
    this.summary = null;
    try {
      this.summary = await api.getExecutionState(executionId);
      return true;
    } catch (e) {
      this.error = String(e);
      return false;
    } finally {
      this.loading = false;
    }
  }

  async act(action: ExecutionAction): Promise<boolean> {
    const run = async (): Promise<boolean> => {
      if (!this.summary) return false;
      this.error = null;
      try {
        debug(`[ExecutionStore] act: ${JSON.stringify(action)}`);
        const result = await api.recordAction(this.summary.execution_id, action);
        debug(`[ExecutionStore] act result: steps=${result.steps.length} status=${result.status}`);
        this.summary = result;
        return true;
      } catch (e) {
        logError(`[ExecutionStore] act error: ${e}`);
        this.error = String(e);
        return false;
      }
    };

    const queued = this.#actionQueue.then(run, run);
    this.#actionQueue = queued.catch(() => false);
    return queued;
  }

  setSummary(summary: ExecutionSummary) {
    this.summary = summary;
    this.error = null;
  }

  reset() {
    this.summary = null;
    this.loading = false;
    this.error = null;
  }
}

export const executionStore = new ExecutionStore();
