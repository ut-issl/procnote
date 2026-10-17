// The operator using this browser or desktop app: a name they enter and a
// device ID generated once. Both are sent with every recorded action. Kept in
// localStorage (best effort): without it they last for this page only.
import { isDesktop } from "#lib/platform.js";
import type { ActionMeta } from "#lib/types/index.js";

const NAME_KEY = "procnote.operatorName";
const DEVICE_KEY = "procnote.deviceId";

function load(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

function save(key: string, value: string) {
  try {
    localStorage.setItem(key, value);
  } catch {
    // Storage unavailable; keep the value in memory only.
  }
}

let name = load(NAME_KEY) ?? "";
const device =
  load(DEVICE_KEY) ??
  (() => {
    const id = `${isDesktop ? "desktop" : "browser"}-${crypto.randomUUID().slice(0, 8)}`;
    save(DEVICE_KEY, id);
    return id;
  })();

const listeners = new Set<() => void>();

export function operatorName(): string {
  return name;
}

export function setOperatorName(next: string) {
  name = next.trim();
  save(NAME_KEY, name);
  for (const listener of listeners) listener();
}

export function subscribeOperatorName(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

/** Metadata for one new action: who, which device, and a fresh event ID. */
export function newActionMeta(): ActionMeta {
  return { event_id: crypto.randomUUID(), recorded_by: name || undefined, device };
}
