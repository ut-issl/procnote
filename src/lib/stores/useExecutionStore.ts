import { useSyncExternalStore } from "react";
import { executionStore, type ExecutionStoreState } from "./execution.js";

/** Subscribe a React component to the shared execution store. */
export function useExecutionStore(): ExecutionStoreState {
  return useSyncExternalStore(executionStore.subscribe, executionStore.getState);
}
