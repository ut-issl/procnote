<script lang="ts">
    import type { InputDefinition, InputState } from "$lib/types";
    import { formatTimestamp } from "$lib/utils/format";
    import { isNonComposingEnter } from "$lib/utils/keyboard";
    import TrashIcon from "./TrashIcon.svelte";

    let {
        definition,
        recorded,
        disabled = false,
        onrecord,
        onrevert,
    }: {
        definition: InputDefinition;
        recorded?: InputState;
        disabled?: boolean;
        onrecord: (inputId: string, value: string, unit?: string) => void;
        onrevert?: () => void;
    } = $props();

    let inputValue = $state("");
    $effect(() => {
        inputValue = recorded?.value ?? "";
    });

    function submit() {
        const val = String(inputValue).trim();
        if (!val) return;
        onrecord(definition.id, val, definition.unit);
    }

    let expectedText = $derived.by(() => {
        if (!definition.expected) return null;
        if (typeof definition.expected === "string") {
            return `Expected: ${definition.expected}`;
        }
        return `Expected: ${definition.expected.min} - ${definition.expected.max}${definition.unit ? " " + definition.unit : ""}`;
    });

    let isRecorded = $derived(!!recorded);
    let inputId = $derived(`input-${definition.id}`);
</script>

<div class="input-field" class:recorded={isRecorded}>
    <div class="input-header">
        <label class="input-label" for={inputId}>{definition.label}</label>
        {#if expectedText}
            <span class="expected">{expectedText}</span>
        {/if}
    </div>
    <div class="input-row">
        {#if definition.type === "selection"}
            <select
                id={inputId}
                bind:value={inputValue}
                disabled={disabled || isRecorded}
                onchange={submit}
            >
                <option value="">Select...</option>
                {#each definition.options as opt}
                    <option value={opt}>{opt}</option>
                {/each}
            </select>
        {:else}
            <input
                id={inputId}
                type={definition.type === "measurement"
                    ? "number"
                    : "text"}
                bind:value={inputValue}
                disabled={disabled || isRecorded}
                placeholder={definition.type === "measurement"
                    ? "0.0"
                    : "Enter value"}
                onkeydown={(e) => {
                    if (isNonComposingEnter(e)) submit();
                }}
            />
        {/if}
        {#if definition.unit}
            <span class="unit">{definition.unit}</span>
        {/if}
        {#if !isRecorded && definition.type !== "selection"}
            <button
                class="btn-record"
                onclick={submit}
                disabled={disabled || !String(inputValue).trim()}
            >
                Record
            </button>
        {/if}
        {#if isRecorded}
            <span class="recorded-badge">Recorded</span>
            {#if recorded?.at}
                <span class="timestamp">{formatTimestamp(recorded.at)}</span>
            {/if}
            {#if onrevert}
                <button class="btn-delete" title="Delete recorded value" onclick={onrevert}>
                    <TrashIcon />
                </button>
            {/if}
        {/if}
    </div>
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
    }

    .input-label {
        font-size: 12px;
        font-weight: 600;
        color: #555;
    }

    .expected {
        font-size: 11px;
        color: #888;
    }

    .input-row {
        display: flex;
        align-items: center;
        gap: 8px;
    }

    .input-row input,
    .input-row select {
        flex: 1;
        padding: 6px 10px;
        border: 1px solid #ccc;
        border-radius: 4px;
        font: inherit;
        font-size: 13px;
    }

    .input-row input:focus,
    .input-row select:focus {
        outline: none;
        border-color: #1a1a2e;
        box-shadow: 0 0 0 2px rgba(26, 26, 46, 0.15);
    }

    .input-row input:disabled,
    .input-row select:disabled {
        background: #eee;
    }

    .unit {
        font-size: 12px;
        color: #666;
        white-space: nowrap;
    }

    .btn-record {
        padding: 6px 12px;
        background: #1a1a2e;
        color: #fff;
        border: none;
        border-radius: 4px;
        font: inherit;
        font-size: 12px;
        font-weight: 600;
        cursor: pointer;
        white-space: nowrap;
    }

    .btn-record:hover:not(:disabled) {
        background: #16213e;
    }

    .btn-record:disabled {
        opacity: 0.4;
        cursor: not-allowed;
    }

    .recorded-badge {
        font-size: 11px;
        font-weight: 600;
        color: #2e7d32;
        white-space: nowrap;
    }
</style>
