<script lang="ts">
    import { onDestroy } from "svelte";
    import type {
        AttachmentDropPointSessionSummary,
        AttachmentDropPointStatus,
        AttachmentSource,
        AttachmentState,
        InputDefinition,
    } from "$lib/types";
    import { formatTimestamp } from "$lib/utils/format";
    import { confirm as confirmDialog } from "@tauri-apps/plugin-dialog";
    import Modal from "./Modal.svelte";
    import TrashIcon from "./TrashIcon.svelte";

    type RemotePhase =
        | "idle"
        | "creating"
        | "open"
        | "receiving"
        | "importing"
        | "closed"
        | "expired"
        | "failed"
        | "canceling";

    let {
        definition,
        attachments = [],
        disabled = false,
        dropPointEnabled = false,
        onattach,
        onpick,
        onremovefile,
        onclear,
        onpreview,
        onstartdrop,
        onpolldrop,
        onimportdrop,
        oncanceldrop,
    }: {
        definition: InputDefinition;
        attachments?: AttachmentState[];
        disabled?: boolean;
        dropPointEnabled?: boolean;
        onattach: (files: AttachmentSource[]) => void;
        onpick?: () => Promise<AttachmentSource[]>;
        onremovefile?: (path: string) => void;
        onclear?: () => void;
        onpreview?: (path: string) => Promise<string | null>;
        onstartdrop?: () => Promise<AttachmentDropPointSessionSummary>;
        onpolldrop?: (sessionId: string) => Promise<AttachmentDropPointStatus>;
        onimportdrop?: (sessionId: string) => Promise<void>;
        oncanceldrop?: (sessionId: string) => Promise<void>;
    } = $props();

    let selectedFiles = $state<AttachmentSource[]>([]);
    let previewUrls = $state<Record<string, string>>({});
    let previewRunId = 0;
    let remotePhase = $state<RemotePhase>("idle");
    let remoteSession = $state<AttachmentDropPointSessionSummary | null>(null);
    let remoteStatus = $state<AttachmentDropPointStatus | null>(null);
    let remoteError = $state<string | null>(null);
    let countdownNow = $state(Date.now());
    let pollTimer: number | null = null;
    let countdownTimer: number | null = null;
    let remoteRunId = 0;

    let isRecorded = $derived(attachments.length > 0);
    let hasSelectedFiles = $derived(selectedFiles.length > 0);
    let canUseDropPoint = $derived(
        dropPointEnabled && !!onstartdrop && !!onpolldrop && !!onimportdrop && !!oncanceldrop,
    );
    let expiryCountdown = $derived(
        remoteSession ? formatExpiryCountdown(remoteSession.expires_at, countdownNow) : null,
    );

    onDestroy(() => {
        stopPolling();
        stopExpiryCountdown();
    });

    $effect(() => {
        const previewLoader = onpreview;
        const runId = ++previewRunId;
        previewUrls = {};
        if (!previewLoader) return;

        const imageAttachments = attachments.filter(isImageAttachment);
        for (const file of imageAttachments) {
            void loadPreview(previewLoader, file, runId);
        }
    });

    function normalizedContentType(contentType: string): string {
        return contentType.split(";")[0]?.trim().toLowerCase() ?? "";
    }

    function isImageAttachment(file: AttachmentState): boolean {
        return normalizedContentType(file.content_type).startsWith("image/");
    }

    async function loadPreview(
        previewLoader: (path: string) => Promise<string | null>,
        file: AttachmentState,
        runId: number,
    ) {
        try {
            const dataUrl = await previewLoader(file.path);
            if (runId === previewRunId && dataUrl) {
                previewUrls[file.path] = dataUrl;
            }
        } catch {
            // Thumbnail previews are best-effort; keep the attachment usable without one.
        }
    }

    async function pickFiles() {
        if (!onpick) return;
        const files = await onpick();
        if (files.length === 0) return;
        selectedFiles = [...selectedFiles, ...files];
    }

    function attachSelected() {
        if (selectedFiles.length === 0) return;
        onattach(selectedFiles);
        selectedFiles = [];
    }

    function removeSelected(index: number) {
        selectedFiles = selectedFiles.filter((_, i) => i !== index);
    }

    function clearSelected() {
        selectedFiles = [];
    }

    async function confirmRemoveFile(file: AttachmentState) {
        if (!onremovefile) return;
        const ok = await confirmDialog(`Remove ${file.filename}?`, {
            title: "Remove attachment",
            kind: "warning",
            okLabel: "Remove",
            cancelLabel: "Cancel",
        });
        if (ok) onremovefile(file.path);
    }

    async function confirmClearAll() {
        if (!onclear) return;
        const ok = await confirmDialog(
            `Remove all ${attachments.length} attachments from ${definition.label}?`,
            {
                title: "Remove all attachments",
                kind: "warning",
                okLabel: "Remove all",
                cancelLabel: "Cancel",
            },
        );
        if (ok) onclear();
    }

    async function startRemoteUpload() {
        if (!canUseDropPoint || !onstartdrop) return;
        stopPolling();
        stopExpiryCountdown();
        remotePhase = "creating";
        remoteSession = null;
        remoteStatus = null;
        remoteError = null;
        const runId = ++remoteRunId;
        try {
            const session = await onstartdrop();
            if (runId !== remoteRunId) {
                try {
                    await oncanceldrop?.(session.session_id);
                } catch {
                    // The user already cancelled this stale session locally.
                }
                return;
            }
            remoteSession = session;
            remotePhase = "open";
            startExpiryCountdown();
            startPolling();
        } catch (e) {
            remotePhase = "failed";
            remoteError = String(e);
        }
    }

    function startPolling() {
        stopPolling();
        void pollRemoteOnce();
        pollTimer = window.setInterval(() => void pollRemoteOnce(), 1500);
    }

    function stopPolling() {
        if (pollTimer !== null) {
            window.clearInterval(pollTimer);
            pollTimer = null;
        }
    }

    function startExpiryCountdown() {
        stopExpiryCountdown();
        countdownNow = Date.now();
        countdownTimer = window.setInterval(() => {
            countdownNow = Date.now();
        }, 1000);
    }

    function stopExpiryCountdown() {
        if (countdownTimer !== null) {
            window.clearInterval(countdownTimer);
            countdownTimer = null;
        }
    }

    async function pollRemoteOnce() {
        if (!remoteSession || !onpolldrop) return;
        const session = remoteSession;
        const runId = remoteRunId;
        try {
            const status = await onpolldrop(session.session_id);
            if (runId !== remoteRunId) return;
            remoteStatus = status;
            switch (status.status) {
                case "open":
                    remotePhase = "open";
                    break;
                case "receiving":
                    remotePhase = "receiving";
                    break;
                case "ready":
                    stopPolling();
                    await importRemoteUpload(session.session_id);
                    break;
                case "closed":
                    stopPolling();
                    remotePhase = "closed";
                    break;
                case "expired":
                    stopPolling();
                    remotePhase = "expired";
                    break;
                default:
                    stopPolling();
                    remotePhase = "failed";
                    remoteError = `DropPoint session status: ${status.status}`;
                    break;
            }
        } catch (e) {
            stopPolling();
            remotePhase = "failed";
            remoteError = String(e);
        }
    }

    async function importRemoteUpload(sessionId: string) {
        if (!onimportdrop) return;
        const runId = remoteRunId;
        remotePhase = "importing";
        remoteError = null;
        try {
            await onimportdrop(sessionId);
            if (runId !== remoteRunId) return;
            resetRemoteUpload();
        } catch (e) {
            if (runId !== remoteRunId) return;
            remotePhase = "failed";
            remoteError = String(e);
        }
    }

    async function retryRemoteImport() {
        if (!remoteSession) return;
        await importRemoteUpload(remoteSession.session_id);
    }

    async function cancelRemoteUpload() {
        stopPolling();
        const sessionId = remoteSession?.session_id;
        remotePhase = "canceling";
        try {
            if (sessionId && oncanceldrop) {
                await oncanceldrop(sessionId);
            }
        } finally {
            resetRemoteUpload();
        }
    }

    function resetRemoteUpload() {
        stopPolling();
        stopExpiryCountdown();
        remoteRunId += 1;
        remotePhase = "idle";
        remoteSession = null;
        remoteStatus = null;
        remoteError = null;
    }

    function formatExpiryCountdown(expiresAt: string, nowMs: number): string {
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

    function remoteMessage(): string {
        switch (remotePhase) {
            case "creating":
                return "Creating upload session...";
            case "open":
                return "Scan with your phone. Waiting for files...";
            case "receiving":
                return "Upload in progress...";
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
</script>

<div class="input-field" class:recorded={isRecorded}>
    <div class="input-header">
        <span class="input-label">{definition.label}</span>
        {#if isRecorded}
            <span class="recorded-badge">{attachments.length} attached</span>
        {/if}
    </div>

    {#if isRecorded}
        <div class="attachment-list">
            {#each attachments as file}
                <div class="attachment-row">
                    {#if isImageAttachment(file) && previewUrls[file.path]}
                        <img
                            class="attachment-thumbnail"
                            src={previewUrls[file.path]}
                            alt=""
                            title={file.filename}
                        />
                    {/if}
                    <span class="filename" title={file.path}>{file.filename}</span>
                    <span class="hash">{file.sha256.slice(0, 7)}</span>
                    {#if file.at}
                        <span class="timestamp">{formatTimestamp(file.at)}</span>
                    {/if}
                    {#if onremovefile}
                        <button
                            class="btn-delete"
                            title="Remove attachment"
                            onclick={() => confirmRemoveFile(file)}
                            disabled={disabled}
                        >
                            <TrashIcon />
                        </button>
                    {/if}
                </div>
            {/each}
        </div>
    {/if}

    {#if hasSelectedFiles}
        <div class="attachment-list pending-list">
            {#each selectedFiles as file, index}
                <div class="attachment-row">
                    <span class="filename" title={file.path}>{file.filename}</span>
                    <button
                        class="btn-clear"
                        onclick={() => removeSelected(index)}
                        disabled={disabled}
                    >
                        Remove
                    </button>
                </div>
            {/each}
        </div>
        <div class="input-row row-actions">
            <button class="btn-record" onclick={attachSelected} disabled={disabled}>
                Attach {selectedFiles.length} {selectedFiles.length === 1 ? "File" : "Files"}
            </button>
            <button class="btn-choose" onclick={pickFiles} disabled={disabled}>
                Add More
            </button>
            <button class="btn-clear" onclick={clearSelected} disabled={disabled}>
                Clear
            </button>
        </div>
    {:else}
        <div class="input-row row-actions">
            <button class="btn-choose" onclick={pickFiles} disabled={disabled}>
                {isRecorded ? "Add Files" : "Choose Files"}
            </button>
            {#if canUseDropPoint}
                <button class="btn-drop" onclick={startRemoteUpload} disabled={disabled}>
                    Upload via QR Code
                </button>
            {/if}
            {#if isRecorded && onclear && attachments.length > 1}
                <button class="btn-clear" onclick={confirmClearAll} disabled={disabled}>
                    Remove All
                </button>
            {/if}
        </div>
    {/if}
</div>

{#if remotePhase !== "idle"}
    <Modal oncancel={cancelRemoteUpload}>
        <h3>Upload via QR Code</h3>
        {#if remoteSession}
            <div class="drop-name-card">
                <span class="drop-name-label">Confirm upload page</span>
                <p>
                    After scanning, confirm the upload page shows <strong>{remoteSession.display_name}</strong>.
                </p>
            </div>
            <div class="qr-code">{@html remoteSession.qr_svg}</div>
            <p class="drop-url">{remoteSession.qr_url}</p>
            <p class="expiry">
                Expires at {formatTimestamp(remoteSession.expires_at)}
                {#if expiryCountdown}
                    <span class="expiry-countdown">({expiryCountdown})</span>
                {/if}
            </p>
        {/if}
        <p>{remoteMessage()}</p>
        {#if remoteStatus?.encrypted_size}
            <p>Encrypted size: {remoteStatus.encrypted_size} bytes</p>
        {/if}
        {#if remoteError}
            <p class="remote-error">{remoteError}</p>
        {/if}
        <div class="modal-actions">
            {#if remotePhase === "failed" && remoteStatus?.status === "ready"}
                <button class="btn-record" onclick={retryRemoteImport}>Retry Import</button>
            {/if}
            <button class="btn-clear" onclick={cancelRemoteUpload} disabled={remotePhase === "canceling"}>
                {remotePhase === "canceling" ? "Cancelling..." : "Cancel"}
            </button>
        </div>
    </Modal>
{/if}

<style>
    .input-field {
        padding: 8px 12px;
        background: #f8f9fa;
        border: 1px solid #e0e0e0;
        border-radius: 4px;
    }

    .input-field.recorded {
        background: #e8f5e9;
        border-color: #c8e6c9;
    }

    .input-header {
        display: flex;
        justify-content: space-between;
        align-items: baseline;
        margin-bottom: 6px;
        gap: 8px;
    }

    .input-label {
        font-size: 12px;
        font-weight: 600;
        color: #555;
    }

    .input-row,
    .attachment-row {
        display: flex;
        align-items: center;
        gap: 8px;
    }

    .row-actions {
        margin-top: 8px;
    }

    .attachment-list {
        display: flex;
        flex-direction: column;
        gap: 4px;
    }

    .pending-list {
        margin-top: 8px;
        padding-top: 8px;
        border-top: 1px dashed #c8e6c9;
    }

    .attachment-row {
        min-height: 28px;
    }

    .attachment-thumbnail {
        width: 32px;
        height: 32px;
        flex-shrink: 0;
        border: 1px solid #c8e6c9;
        border-radius: 4px;
        background: #fff;
        object-fit: cover;
    }

    .filename {
        flex: 1;
        font-size: 13px;
        color: #333;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }

    .btn-choose,
    .btn-record,
    .btn-drop,
    .btn-clear {
        padding: 6px 12px;
        border: none;
        border-radius: 4px;
        font: inherit;
        font-size: 12px;
        font-weight: 600;
        cursor: pointer;
        white-space: nowrap;
    }

    .btn-choose,
    .btn-record,
    .btn-drop {
        background: #1a1a2e;
        color: #fff;
    }

    .btn-choose:hover:not(:disabled),
    .btn-record:hover:not(:disabled),
    .btn-drop:hover:not(:disabled) {
        background: #16213e;
    }

    .btn-drop {
        background: #00695c;
    }

    .btn-drop:hover:not(:disabled) {
        background: #004d40;
    }

    .btn-clear {
        background: #fff;
        color: #666;
        border: 1px solid #ccc;
    }

    .btn-clear:hover:not(:disabled) {
        background: #f5f5f5;
    }

    .btn-choose:disabled,
    .btn-record:disabled,
    .btn-drop:disabled,
    .btn-clear:disabled {
        opacity: 0.4;
        cursor: not-allowed;
    }

    .hash {
        font-size: 11px;
        font-family: monospace;
        color: #888;
        white-space: nowrap;
    }

    .timestamp {
        font-size: 11px;
        color: #888;
        white-space: nowrap;
    }

    .recorded-badge {
        font-size: 11px;
        font-weight: 600;
        color: #2e7d32;
        white-space: nowrap;
    }

    .btn-delete {
        display: flex;
        align-items: center;
        justify-content: center;
        width: 24px;
        height: 24px;
        padding: 0;
        border: none;
        border-radius: 4px;
        background: transparent;
        color: #888;
        cursor: pointer;
        flex-shrink: 0;
    }

    .btn-delete:hover:not(:disabled) {
        background: #ffcdd2;
        color: #c62828;
    }

    .btn-delete:disabled {
        opacity: 0.4;
        cursor: not-allowed;
    }

    .drop-name-card {
        padding: 12px;
        border: 1px solid #b2dfdb;
        border-radius: 6px;
        background: #e0f2f1;
        color: #004d40;
    }

    .drop-name-card p {
        margin: 4px 0 0;
        font-size: 14px;
    }

    .drop-name-label {
        font-size: 11px;
        font-weight: 700;
        letter-spacing: 0.04em;
        text-transform: uppercase;
    }

    .qr-code {
        display: flex;
        justify-content: center;
        margin: 16px 0;
    }

    .qr-code :global(svg) {
        width: 240px;
        height: 240px;
        border: 1px solid #eee;
    }

    .drop-url {
        max-width: 460px;
        overflow-wrap: anywhere;
        font-family: monospace;
        font-size: 11px;
        color: #666;
    }

    .expiry {
        font-size: 12px;
        color: #666;
    }

    .expiry-countdown {
        margin-left: 6px;
        color: #00695c;
        font-family: monospace;
        font-weight: 700;
        white-space: nowrap;
    }

    .remote-error {
        color: #c62828;
        font-weight: 600;
    }
</style>
