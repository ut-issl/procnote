<script lang="ts">
    import type { StepContent } from "$lib/types";
    import { isNonComposingEnter } from "$lib/utils/keyboard";
    import Modal from "./Modal.svelte";

    let {
        steps,
        onconfirm,
        oncancel,
    }: {
        steps: { id: string; heading: string }[];
        onconfirm: (
            stepId: string,
            heading: string,
            content: StepContent[],
            afterStepId?: string,
        ) => void;
        oncancel: () => void;
    } = $props();

    let heading = $state("");
    let description = $state("");
    let afterStepId = $state("");

    function submit() {
        if (!heading.trim()) return;
        const stepId = `dyn-step-${crypto.randomUUID().slice(0, 8)}`;
        const content: StepContent[] = description.trim()
            ? [{ type: "Prose", text: description.trim() }]
            : [];
        onconfirm(
            stepId,
            heading.trim(),
            content,
            afterStepId || undefined,
        );
    }
</script>

<Modal {oncancel}>
    <h3>Add Step</h3>
    <p class="hint">Add a new step to the procedure execution.</p>
    <label class="field">
        <span class="field-label">Step Heading</span>
        <!-- svelte-ignore a11y_autofocus -->
        <input
            type="text"
            bind:value={heading}
            placeholder="e.g. Additional Verification"
            autofocus
            onkeydown={(e) => {
                if (isNonComposingEnter(e)) submit();
            }}
        />
    </label>
    <label class="field">
        <span class="field-label">Description (optional)</span>
        <textarea
            bind:value={description}
            placeholder="Describe what this step involves..."
            rows="2"
        ></textarea>
    </label>
    <label class="field">
        <span class="field-label">Insert After (optional)</span>
        <select bind:value={afterStepId}>
            <option value="">End of procedure</option>
            {#each steps as s}
                <option value={s.id}>{s.heading}</option>
            {/each}
        </select>
    </label>
    <div class="modal-actions">
        <button class="btn btn-secondary" onclick={oncancel}>Cancel</button>
        <button
            class="btn btn-primary"
            onclick={submit}
            disabled={!heading.trim()}
        >
            Add Step
        </button>
    </div>
</Modal>

<style>
    .hint {
        margin: 0 0 16px;
        font-size: 13px;
        color: #888;
    }
</style>
