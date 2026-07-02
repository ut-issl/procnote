import { debug, error as logError } from "@tauri-apps/plugin-log";
import * as api from "$lib/api/commands";
import type { ExecutionAction, ExecutionSummary } from "$lib/types";

class ExecutionStore {
  summary: ExecutionSummary | null = $state(null);
  loading = $state(false);
  error: string | null = $state(null);

  get isFinished() {
    const s = this.summary?.status;
    return s === "pass" || s === "fail" || s === "aborted";
  }

  async start(templatePath: string) {
    this.loading = true;
    this.error = null;
    try {
      this.summary = await api.startExecution(templatePath);
    } catch (e) {
      this.error = String(e);
    } finally {
      this.loading = false;
    }
  }

  async load(executionId: string) {
    this.loading = true;
    this.error = null;
    try {
      this.summary = await api.getExecutionState(executionId);
    } catch (e) {
      this.error = String(e);
    } finally {
      this.loading = false;
    }
  }

  async act(action: ExecutionAction) {
    if (!this.summary) return;
    this.error = null;
    try {
      debug(`[ExecutionStore] act: ${JSON.stringify(action)}`);
      const result = await api.recordAction(this.summary.execution_id, action);
      debug(`[ExecutionStore] act result: steps=${result.steps.length} status=${result.status}`);
      this.summary = result;
    } catch (e) {
      logError(`[ExecutionStore] act error: ${e}`);
      this.error = String(e);
    }
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
