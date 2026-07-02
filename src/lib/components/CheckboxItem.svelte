<script lang="ts">
    import type { StepContentSummary } from "$lib/types";
    import { formatTimestamp } from "$lib/utils/format";

    type CheckboxContent = Extract<StepContentSummary, { type: "Checkbox" }>;

    let {
        checkbox,
        disabled = false,
        ontoggle,
    }: {
        checkbox: CheckboxContent;
        disabled?: boolean;
        ontoggle: (checkboxId: string, checked: boolean) => Promise<boolean> | boolean;
    } = $props();

    async function handleChange(event: Event) {
        const input = event.currentTarget as HTMLInputElement;
        if (!checkbox.id) {
            input.checked = checkbox.checked;
            return;
        }
        const toggled = await ontoggle(checkbox.id, input.checked);
        if (!toggled) {
            input.checked = checkbox.checked;
        }
    }
</script>

<label class="checkbox-item" class:checked={checkbox.checked} class:disabled>
    <input
        type="checkbox"
        checked={checkbox.checked}
        {disabled}
        onchange={handleChange}
    />
    <span class="checkbox-text">{checkbox.text}</span>
    {#if checkbox.at}
        <span class="timestamp">{formatTimestamp(checkbox.at)}</span>
    {/if}
</label>

<style>
    .checkbox-item {
        display: flex;
        align-items: flex-start;
        gap: 8px;
        padding: 6px 0;
        cursor: pointer;
        font-size: 13px;
    }

    .checkbox-item.disabled {
        cursor: default;
        opacity: 0.6;
    }

    .checkbox-item input[type="checkbox"] {
        margin-top: 2px;
        accent-color: #1a1a2e;
    }

    .checkbox-text {
        flex: 1;
    }

    .checked .checkbox-text {
        text-decoration: line-through;
        color: #888;
    }
</style>
