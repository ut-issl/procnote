// Desktop (Tauri) and browser implementations of host services.
// Components use these instead of importing Tauri plugins directly so the same
// UI runs in both environments.
import { isTauri } from "@tauri-apps/api/core";
import { confirm as tauriConfirm } from "@tauri-apps/plugin-dialog";
import {
  debug as tauriDebug,
  error as tauriError,
  warn as tauriWarn,
} from "@tauri-apps/plugin-log";
import { openUrl } from "@tauri-apps/plugin-opener";

export const isDesktop = isTauri();

export async function logDebug(message: string): Promise<void> {
  if (isDesktop) return tauriDebug(message);
  console.debug(message);
}

export async function logWarn(message: string): Promise<void> {
  if (isDesktop) return tauriWarn(message);
  console.warn(message);
}

export async function logError(message: string): Promise<void> {
  if (isDesktop) return tauriError(message);
  console.error(message);
}

export interface ConfirmOptions {
  title: string;
  okLabel: string;
  cancelLabel: string;
}

/** Ask the user to confirm a destructive action. */
export async function confirmWarning(message: string, options: ConfirmOptions): Promise<boolean> {
  if (isDesktop) return tauriConfirm(message, { ...options, kind: "warning" });
  return window.confirm(message);
}

export async function openExternalUrl(url: string): Promise<void> {
  if (isDesktop) return openUrl(url);
  window.open(url, "_blank", "noopener,noreferrer");
}
