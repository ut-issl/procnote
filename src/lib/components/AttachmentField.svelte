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
    import { inferContentType } from "$lib/utils/mime";
    import { confirm as confirmDialog, open } from "@tauri-apps/plugin-dialog";
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
        onremovefile,
        onclear,
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
        onremovefile?: (path: string) => void;
        onclear?: () => void;
        onstartdrop?: () => Promise<AttachmentDropPointSessionSummary>;
        onpolldrop?: (sessionId: string) => Promise<AttachmentDropPointStatus>;
        onimportdrop?: (sessionId: string) => Promise<void>;
        oncanceldrop?: (sessionId: string) => Promise<void>;
    } = $props();

    let selectedFiles = $state<AttachmentSource[]>([]);
    let remotePhase = $state<RemotePhase>("idle");
    let remoteSession = $state<AttachmentDropPointSessionSummary | null>(null);
    let remoteStatus = $state<AttachmentDropPointStatus | null>(null);
    let remoteError = $state<string | null>(null);
    let pollTimer: number | null = null;
    let remoteRunId = 0;

    let isRecorded = $derived(attachments.length > 0);
    let hasSelectedFiles = $derived(selectedFiles.length > 0);
    let canUseDropPoint = $derived(
        dropPointEnabled && !!onstartdrop && !!onpolldrop && !!onimportdrop && !!oncanceldrop,
    );

    onDestroy(() => {
        stopPolling();
    });

    function filenameFromPath(path: string): string {
        return path.split(/[/\\]/).pop() ?? path;
    }

    async function pickFiles() {
        const result = await open({
            multiple: true,
            directory: false,
            title: definition.label,
        });
        if (!result) return;

        const paths = Array.isArray(result) ? result : [result];
        selectedFiles = [
            ...selectedFiles,
            ...paths.map((path) => {
                const filename = filenameFromPath(path);
                return {
                    filename,
                    path,
                    content_type: inferContentType(filename),
                };
            }),
        ];
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
        remoteRunId += 1;
        remotePhase = "idle";
        remoteSession = null;
        remoteStatus = null;
        remoteError = null;
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
                    Scan to Upload
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
        <h3>Scan to Upload</h3>
        {#if remoteSession}
            <div class="qr-code">{@html remoteSession.qr_svg}</div>
            <p class="drop-url">{remoteSession.qr_url}</p>
            <p class="expiry">Expires at {formatTimestamp(remoteSession.expires_at)}</p>
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

    .remote-error {
        color: #c62828;
        font-weight: 600;
    }
</style>
