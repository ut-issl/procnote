// The browser's local-first recording, wired to IndexedDB, the server, and
// the WebAssembly core. The desktop app records to its own files and has none.
import { createStore, entries, get, set } from "idb-keyval";
import * as commands from "#lib/api/commands.js";
import { isDesktop } from "#lib/platform.js";
import * as core from "#lib/wasm/core.js";
import { isUnreachable, type KeyValueStore, LocalExecutions } from "./localExecutions.js";

function indexedDbStore(): KeyValueStore {
  const store = createStore("procnote", "offline");
  return {
    get: (key) => get(key, store),
    set: (key, value) => set(key, value, store),
    entries: () => entries(store),
  };
}

export const localExecutions: LocalExecutions | null = isDesktop
  ? null
  : new LocalExecutions(indexedDbStore(), commands, commands, {
      ready: core.loadCore,
      summarize: core.summarize,
      decide: core.decide,
      start: core.start,
    });

/**
 * Fetch something from the server and keep it, or use what was kept when the
 * server can't be reached.
 */
export async function fetchOrKept<T>(key: string, fetch: () => Promise<T>): Promise<T> {
  if (!localExecutions) return fetch();
  try {
    const value = await fetch();
    await localExecutions.keep(key, value);
    return value;
  } catch (error) {
    const kept = isUnreachable(error) ? await localExecutions.kept<T>(key) : undefined;
    if (kept === undefined) throw error;
    return kept;
  }
}

/**
 * Keep recordings safe and moving: ask the browser not to evict our storage,
 * load the execution logic, and sync now, whenever the network comes back,
 * and every half minute.
 */
export function startSyncing(): void {
  if (!localExecutions) return;
  void navigator.storage?.persist?.();
  // Load the execution logic now, while the module can still be fetched.
  void core.loadCore();
  const syncAll = () => void localExecutions?.syncAll();
  syncAll();
  window.addEventListener("online", syncAll);
  window.setInterval(syncAll, 30_000);
}
