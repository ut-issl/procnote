import { useCallback, useEffect, useRef, useState } from "react";
import type {
  AttachmentDropPointSessionSummary,
  AttachmentDropPointStatus,
} from "#lib/types/index.js";

export type RemotePhase =
  | "idle"
  | "creating"
  | "open"
  | "importing"
  | "closed"
  | "expired"
  | "failed"
  | "canceling";

export interface DropPointHandlers {
  onstartdrop?: () => Promise<AttachmentDropPointSessionSummary>;
  onpolldrop?: (sessionId: string) => Promise<AttachmentDropPointStatus>;
  onimportdrop?: (sessionId: string) => Promise<void>;
  oncanceldrop?: (sessionId: string) => Promise<void>;
}

export function formatExpiryCountdown(expiresAt: string, nowMs: number): string {
  const expiresMs = Date.parse(expiresAt);
  if (!Number.isFinite(expiresMs)) return "countdown unavailable";

  const remainingSeconds = Math.max(0, Math.ceil((expiresMs - nowMs) / 1000));
  if (remainingSeconds === 0) return "expired";

  const hours = Math.floor(remainingSeconds / 3600);
  const minutes = Math.floor((remainingSeconds % 3600) / 60);
  const seconds = remainingSeconds % 60;
  const paddedSeconds = String(seconds).padStart(2, "0");
  if (hours === 0) return `${minutes}:${paddedSeconds} remaining`;

  const paddedMinutes = String(minutes).padStart(2, "0");
  return `${hours}:${paddedMinutes}:${paddedSeconds} remaining`;
}

export function remoteMessage(phase: RemotePhase): string {
  switch (phase) {
    case "creating":
      return "Creating upload session...";
    case "open":
      return "Scan once, then keep sending files from the same page. Waiting for files...";
    case "importing":
      return "Files received. Importing attachments...";
    case "closed":
      return "Upload session closed.";
    case "expired":
      return "Upload session expired.";
    case "failed":
      return "Upload session failed.";
    case "canceling":
      return "Cancelling upload session...";
    case "idle":
      return "";
  }
}

export function parsePollError(error: unknown): { kind: string; message: string } {
  if (typeof error === "object" && error !== null) {
    const value = error as Record<string, unknown>;
    if (typeof value.kind === "string" && typeof value.message === "string") {
      return { kind: value.kind, message: value.message };
    }
  }
  return { kind: "fatal", message: String(error) };
}

/**
 * Drive one DropPoint (QR code upload) session: create it, poll for uploads
 * with backoff, import received files, and cancel.
 *
 * Every async step captures a run ID and gives up when a newer run (cancel or
 * restart) has begun, so stale responses never overwrite current state.
 */
export function useDropPointUpload(handlers: DropPointHandlers) {
  const [phase, setPhase] = useState<RemotePhase>("idle");
  const [session, setSessionState] = useState<AttachmentDropPointSessionSummary | null>(null);
  const [status, setStatus] = useState<AttachmentDropPointStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [countdownNow, setCountdownNow] = useState(Date.now());

  const handlersRef = useRef(handlers);
  handlersRef.current = handlers;
  const sessionRef = useRef<AttachmentDropPointSessionSummary | null>(null);
  const pollTimer = useRef<number | null>(null);
  const pollBackoffMs = useRef(1000);
  const countdownTimer = useRef<number | null>(null);
  const runId = useRef(0);
  const pollInFlightRunId = useRef<number | null>(null);

  const setSession = useCallback((next: AttachmentDropPointSessionSummary | null) => {
    sessionRef.current = next;
    setSessionState(next);
  }, []);

  const stopPolling = useCallback(() => {
    if (pollTimer.current !== null) {
      window.clearTimeout(pollTimer.current);
      pollTimer.current = null;
    }
  }, []);

  const stopExpiryCountdown = useCallback(() => {
    if (countdownTimer.current !== null) {
      window.clearInterval(countdownTimer.current);
      countdownTimer.current = null;
    }
  }, []);

  const startExpiryCountdown = useCallback(() => {
    stopExpiryCountdown();
    setCountdownNow(Date.now());
    countdownTimer.current = window.setInterval(() => setCountdownNow(Date.now()), 1000);
  }, [stopExpiryCountdown]);

  useEffect(
    () => () => {
      stopPolling();
      stopExpiryCountdown();
    },
    [stopPolling, stopExpiryCountdown],
  );

  // pollRemoteOnce and importRemoteUpload schedule each other, so they are
  // reached through a ref to the latest closures.
  const pollRef = useRef<() => Promise<void>>(async () => {});

  const schedulePoll = useCallback((delayMs: number) => {
    if (!sessionRef.current || pollTimer.current !== null) return;
    const jitteredDelay = Math.max(250, Math.round(delayMs * (0.8 + Math.random() * 0.4)));
    pollTimer.current = window.setTimeout(() => {
      pollTimer.current = null;
      void pollRef.current();
    }, jitteredDelay);
  }, []);

  const importRemoteUpload = useCallback(
    async (sessionId: string) => {
      const onimportdrop = handlersRef.current.onimportdrop;
      if (!onimportdrop) return;
      const currentRun = runId.current;
      setPhase("importing");
      setError(null);
      try {
        await onimportdrop(sessionId);
        if (currentRun !== runId.current) return;
        setPhase("open");
        schedulePoll(1500);
      } catch (e) {
        if (currentRun !== runId.current) return;
        setPhase("failed");
        setError(String(e));
      }
    },
    [schedulePoll],
  );

  pollRef.current = async () => {
    const current = sessionRef.current;
    const { onpolldrop, onimportdrop } = handlersRef.current;
    if (!current || !onpolldrop) return;
    const currentRun = runId.current;
    if (pollInFlightRunId.current === currentRun) return;
    pollInFlightRunId.current = currentRun;
    try {
      const next = await onpolldrop(current.session_id);
      if (currentRun !== runId.current) return;
      setStatus(next);
      setError(null);
      pollBackoffMs.current = 1000;
      if (next.status !== "open") {
        // Terminal polling may have recovered local receipts into the log.
        await onimportdrop?.(current.session_id);
        if (currentRun !== runId.current) return;
      }
      switch (next.status) {
        case "open":
          setPhase("open");
          if (next.pending_submissions > 0 || next.needs_import) {
            await importRemoteUpload(current.session_id);
          } else {
            schedulePoll(1500);
          }
          break;
        case "closed":
          stopPolling();
          setPhase("closed");
          break;
        case "expired":
          stopPolling();
          setPhase("expired");
          break;
        case "failed":
          stopPolling();
          setPhase("failed");
          setError("DropPoint reported a terminal internal failure.");
          break;
        default:
          stopPolling();
          setPhase("failed");
          setError(`Unsupported DropPoint session status: ${next.status}`);
          break;
      }
    } catch (e) {
      if (currentRun !== runId.current) return;
      const pollError = parsePollError(e);
      if (pollError.kind === "retryable") {
        setError(`Temporary polling failure: ${pollError.message}`);
        schedulePoll(pollBackoffMs.current);
        pollBackoffMs.current = Math.min(pollBackoffMs.current * 2, 15000);
        return;
      }
      stopPolling();
      if (pollError.kind === "terminal") {
        try {
          await onimportdrop?.(current.session_id);
        } catch (refreshError) {
          pollError.message = String(refreshError);
        }
        if (currentRun !== runId.current) return;
      }
      setPhase("failed");
      setError(pollError.message);
    } finally {
      if (pollInFlightRunId.current === currentRun) {
        pollInFlightRunId.current = null;
      }
    }
  };

  const startPolling = useCallback(() => {
    stopPolling();
    pollBackoffMs.current = 1000;
    void pollRef.current();
  }, [stopPolling]);

  const start = useCallback(async () => {
    const { onstartdrop, oncanceldrop } = handlersRef.current;
    if (!onstartdrop) return;
    stopPolling();
    stopExpiryCountdown();
    setPhase("creating");
    setSession(null);
    setStatus(null);
    setError(null);
    const currentRun = ++runId.current;
    try {
      const created = await onstartdrop();
      if (currentRun !== runId.current) {
        try {
          await oncanceldrop?.(created.session_id);
        } catch {
          // The user already cancelled this stale session locally.
        }
        return;
      }
      setSession(created);
      setPhase("open");
      startExpiryCountdown();
      startPolling();
    } catch (e) {
      setPhase("failed");
      setError(String(e));
    }
  }, [setSession, startExpiryCountdown, startPolling, stopExpiryCountdown, stopPolling]);

  const retry = useCallback(() => {
    if (!sessionRef.current) return;
    setError(null);
    setPhase("open");
    startPolling();
  }, [startPolling]);

  const reset = useCallback(() => {
    stopPolling();
    stopExpiryCountdown();
    runId.current += 1;
    setPhase("idle");
    setSession(null);
    setStatus(null);
    setError(null);
  }, [setSession, stopExpiryCountdown, stopPolling]);

  const cancel = useCallback(async () => {
    stopPolling();
    const sessionId = sessionRef.current?.session_id;
    runId.current += 1;
    setPhase("canceling");
    try {
      const oncanceldrop = handlersRef.current.oncanceldrop;
      if (sessionId && oncanceldrop) {
        await oncanceldrop(sessionId);
      }
      reset();
    } catch (e) {
      setPhase("failed");
      setError(`Could not stop receiving. Try again: ${String(e)}`);
    }
  }, [reset, stopPolling]);

  return { phase, session, status, error, countdownNow, start, retry, cancel };
}
