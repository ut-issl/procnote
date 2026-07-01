<script lang="ts">
    import type { AttachmentSource, AttachmentState, InputDefinition } from "$lib/types";
    import { formatTimestamp } from "$lib/utils/format";
    import { inferContentType } from "$lib/utils/mime";
    import { confirm as confirmDialog, open } from "@tauri-apps/plugin-dialog";
    import TrashIcon from "./TrashIcon.svelte";

    let {
        definition,
        attachments = [],
        disabled = false,
        onattach,
        onremovefile,
        onclear,
    }: {
        definition: InputDefinition;
        attachments?: AttachmentState[];
        disabled?: boolean;
        onattach: (files: AttachmentSource[]) => void;
        onremovefile?: (path: string) => void;
        onclear?: () => void;
    } = $props();

    let selectedFiles = $state<AttachmentSource[]>([]);

    let isRecorded = $derived(attachments.length > 0);
    let hasSelectedFiles = $derived(selectedFiles.length > 0);

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
        {#if onclear && attachments.length > 1}
            <div class="input-row row-actions">
                <button class="btn-clear" onclick={confirmClearAll} disabled={disabled}>
                    Remove All
                </button>
            </div>
        {/if}
    {:else if hasSelectedFiles}
        <div class="attachment-list">
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
        <div class="input-row">
            <button class="btn-choose" onclick={pickFiles} disabled={disabled}>
                Choose Files
            </button>
        </div>
    {/if}
</div>

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
    .btn-record {
        background: #1a1a2e;
        color: #fff;
    }

    .btn-choose:hover:not(:disabled),
    .btn-record:hover:not(:disabled) {
        background: #16213e;
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
</style>
