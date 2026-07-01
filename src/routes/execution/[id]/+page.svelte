<script lang="ts">
    import { page } from "$app/state";
    import { onMount } from "svelte";
    import { goto } from "$app/navigation";
    import { executionStore } from "$lib/stores/execution.svelte";
    import * as api from "$lib/api/commands";
    import type { ExecutionAction, ExecutionSummary, StepContent } from "$lib/types";
    import { formatTimestamp } from "$lib/utils/format";
    import { isNonComposingEnter } from "$lib/utils/keyboard";
    import StepCard from "$lib/components/StepCard.svelte";
    import AddStepDialog from "$lib/components/AddStepDialog.svelte";
    import Modal from "$lib/components/Modal.svelte";
    import { revealItemInDir } from "@tauri-apps/plugin-opener";

    let showAddStepDialog = $state(false);
    let showCompleteDialog = $state(false);
    let showAbortDialog = $state(false);
    let abortReason = $state("");
    let editingName = $state(false);
    let editNameValue = $state("");
    let dropPointEnabled = $state(false);

    let executionId = $derived(page.params.id ?? "");

    onMount(async () => {
        dropPointEnabled = await api.isDropPointConfigured();
        if (executionId) {
            await executionStore.load(executionId);
        }
    });

    let summary = $derived(executionStore.summary);
    let isActive = $derived(summary?.status === "active");
    let isFinished = $derived(executionStore.isFinished);

    let skippedSteps = $derived(
        summary?.steps.filter((s) => s.status === "skipped").length ?? 0,
    );
    let totalSteps = $derived(summary?.steps.length ?? 0);

    let stepRefs = $derived(summary?.steps.map((s) => ({ id: s.id, heading: s.heading })) ?? []);

    // Find the latest finish event (completed or aborted) for its timestamp.
    let finishEvent = $derived(
        summary?.event_history
            .filter(
                (e) =>
                    e.event_type === "execution_completed" ||
                    e.event_type === "execution_aborted",
            )
            .at(-1),
    );

    async function handleAction(action: ExecutionAction) {
        await executionStore.act(action);
    }

    function handleDropPointImported(nextSummary: ExecutionSummary) {
        executionStore.setSummary(nextSummary);
    }

    async function addStep(
        stepId: string,
        heading: string,
        content: StepContent[],
        afterStepId?: string,
    ) {
        await executionStore.act({
            action: "add_step",
            step_id: stepId,
            heading,
            content,
            after_step_id: afterStepId,
        });
        showAddStepDialog = false;
    }

    async function completeExecution(status: "pass" | "fail") {
        await executionStore.act({ action: "complete", status });
        showCompleteDialog = false;
    }

    async function abortExecution() {
        if (!abortReason.trim()) return;
        await executionStore.act({
            action: "abort",
            reason: abortReason.trim(),
        });
        showAbortDialog = false;
        abortReason = "";
    }

    function startEditingName() {
        editNameValue = summary?.name ?? summary?.procedure_id ?? "";
        editingName = true;
    }

    async function saveName() {
        const trimmed = editNameValue.trim();
        if (trimmed && trimmed !== summary?.name) {
            await executionStore.act({
                action: "rename_execution",
                name: trimmed,
            });
        }
        editingName = false;
    }

    function cancelEditName() {
        editingName = false;
    }

    function goHome() {
        executionStore.reset();
        goto("/");
    }

    async function openDirectory() {
        if (summary?.execution_dir) {
            await revealItemInDir(summary.execution_dir);
        }
    }
</script>

<div class="execution-page">
    {#if executionStore.loading && !summary}
        <p class="loading">Loading execution...</p>
    {:else if executionStore.error && !summary}
        <div class="error-panel">
            <p>{executionStore.error}</p>
            <button class="btn btn-secondary" onclick={goHome}
                >Back to Home</button
            >
        </div>
    {:else if summary}
        <div class="execution-header">
            <div class="header-left">
                <button class="btn-back" onclick={goHome}>&larr; Back</button>
                <div class="header-info">
                    {#if editingName}
                        <!-- svelte-ignore a11y_autofocus -->
                        <input
                            class="name-input"
                            type="text"
                            bind:value={editNameValue}
                            onblur={saveName}
                            onkeydown={(e) => {
                                if (isNonComposingEnter(e)) saveName();
                                if (e.key === "Escape") cancelEditName();
                            }}
                            autofocus
                        />
                    {:else}
                        <button
                            class="execution-name"
                            onclick={startEditingName}
                            title="Click to rename"
                        >
                            {summary.name ?? summary.procedure_id}
                        </button>
                    {/if}
                    <span class="procedure-meta">
                        {summary.procedure_title} ({summary.procedure_id}) v{summary.procedure_version}
                    </span>
                    <button
                        class="execution-dir"
                        onclick={openDirectory}
                        title="Open in Finder"
                    >
                        {summary.execution_dir}
                    </button>
                </div>
            </div>
            <div class="header-right">
                <span
                    class="execution-status"
                    class:status-active={isActive}
                    class:status-pass={summary.status === "pass"}
                    class:status-fail={summary.status === "fail"}
                    class:status-aborted={summary.status === "aborted"}
                >
                    {summary.status}
                </span>
                <span class="progress">{totalSteps} steps</span>
            </div>
        </div>

        {#if executionStore.error}
            <div class="error-bar">{executionStore.error}</div>
        {/if}

        {#if isActive}
            <div class="toolbar">
                <button
                    class="btn btn-secondary"
                    onclick={() => (showAddStepDialog = true)}
                >
                    + Add Step
                </button>
                <div class="toolbar-spacer"></div>
                <button
                    class="btn btn-success"
                    onclick={() => (showCompleteDialog = true)}
                >
                    Complete
                </button>
                <button
                    class="btn btn-danger"
                    onclick={() => (showAbortDialog = true)}
                >
                    Abort
                </button>
            </div>
        {/if}

        {#if isFinished}
            <div
                class="finish-banner"
                class:pass={summary.status === "pass"}
                class:fail={summary.status === "fail"}
                class:aborted={summary.status === "aborted"}
            >
                <span>
                    Execution {summary.status === "pass"
                        ? "passed"
                        : summary.status === "fail"
                          ? "failed"
                          : "aborted"}
                    {#if finishEvent}
                        at {formatTimestamp(finishEvent.at)}
                    {/if}
                    &mdash; {totalSteps} steps, {skippedSteps} skipped
                </span>
                <button
                    class="btn btn-undo"
                    onclick={() =>
                        handleAction({
                            action: "reopen_execution",
                            reason: "Reopened by operator",
                        })}
                >
                    Reopen Execution
                </button>
            </div>
        {/if}

        <div class="steps">
            {#each summary.steps as stepSummary}
                <StepCard
                    {stepSummary}
                    executionId={summary.execution_id}
                    executionActive={isActive ?? false}
                    {dropPointEnabled}
                    onaction={handleAction}
                    ondropimported={handleDropPointImported}
                />
            {/each}
        </div>
    {/if}
</div>

{#if showAddStepDialog}
    <AddStepDialog
        steps={stepRefs}
        onconfirm={addStep}
        oncancel={() => (showAddStepDialog = false)}
    />
{/if}

{#if showCompleteDialog}
    <Modal oncancel={() => (showCompleteDialog = false)}>
        <h3>Complete Execution</h3>
        <p>Mark this execution as:</p>
        <div class="modal-actions">
            <button
                class="btn btn-secondary"
                onclick={() => (showCompleteDialog = false)}>Cancel</button
            >
            <button
                class="btn btn-danger"
                onclick={() => completeExecution("fail")}>Fail</button
            >
            <button
                class="btn btn-success"
                onclick={() => completeExecution("pass")}>Pass</button
            >
        </div>
    </Modal>
{/if}

{#if showAbortDialog}
    <Modal
        oncancel={() => {
            showAbortDialog = false;
            abortReason = "";
        }}
    >
        <h3>Abort Execution</h3>
        <p>This will permanently mark the execution as aborted.</p>
        <label class="field">
            <span class="field-label">Reason</span>
            <textarea
                bind:value={abortReason}
                placeholder="Why is the execution being aborted?"
                rows="3"
            ></textarea>
        </label>
        <div class="modal-actions">
            <button
                class="btn btn-secondary"
                onclick={() => {
                    showAbortDialog = false;
                    abortReason = "";
                }}>Cancel</button
            >
            <button
                class="btn btn-danger"
                onclick={abortExecution}
                disabled={!abortReason.trim()}>Abort</button
            >
        </div>
    </Modal>
{/if}

<style>
    .execution-page {
        display: flex;
        flex-direction: column;
        gap: 16px;
    }

    .loading {
        color: #666;
        font-style: italic;
    }

    .error-panel {
        display: flex;
        flex-direction: column;
        align-items: flex-start;
        gap: 12px;
        color: #c0392b;
    }

    .error-bar {
        padding: 8px 12px;
        background: #fce4ec;
        color: #c62828;
        border-radius: 4px;
        font-size: 13px;
    }

    .execution-header {
        display: flex;
        justify-content: space-between;
        align-items: flex-start;
        gap: 16px;
    }

    .header-left {
        display: flex;
        align-items: flex-start;
        gap: 12px;
    }

    .btn-back {
        padding: 4px 8px;
        background: none;
        border: 1px solid #ccc;
        border-radius: 4px;
        font: inherit;
        font-size: 13px;
        cursor: pointer;
        color: #555;
        margin-top: 2px;
    }

    .btn-back:hover {
        background: #f0f0f0;
    }

    .header-info {
        display: flex;
        flex-direction: column;
        gap: 2px;
    }

    .execution-name {
        margin: 0;
        padding: 0;
        font-size: 18px;
        font-weight: 700;
        font-family: inherit;
        cursor: pointer;
        background: none;
        border: none;
        border-bottom: 1px dashed transparent;
        text-align: left;
        color: inherit;
    }

    .execution-name:hover {
        border-bottom-color: #aaa;
    }

    .name-input {
        font-size: 18px;
        font-weight: 700;
        font-family: inherit;
        border: 1px solid #1a1a2e;
        border-radius: 4px;
        padding: 2px 6px;
        outline: none;
        min-width: 200px;
    }

    .procedure-meta {
        font-size: 13px;
        color: #888;
    }

    .execution-dir {
        font-size: 12px;
        font-family: monospace;
        color: #888;
        background: none;
        border: none;
        padding: 0;
        cursor: pointer;
        text-align: left;
    }

    .execution-dir:hover {
        color: #1a73e8;
        text-decoration: underline;
    }

    .header-right {
        display: flex;
        align-items: center;
        gap: 12px;
        flex-shrink: 0;
    }

    .execution-status {
        font-size: 12px;
        font-weight: 600;
        padding: 3px 10px;
        border-radius: 12px;
        text-transform: uppercase;
        background: #eee;
        color: #666;
    }

    .status-active {
        background: #e8f5e9;
        color: #2e7d32;
    }

    .status-pass {
        background: #e0f2f1;
        color: #00695c;
    }

    .status-fail {
        background: #fce4ec;
        color: #c62828;
    }

    .status-aborted {
        background: #fff3e0;
        color: #e65100;
    }

    .progress {
        font-size: 13px;
        color: #888;
    }

    .toolbar {
        display: flex;
        gap: 8px;
        padding: 12px 0;
        border-bottom: 1px solid #eee;
    }

    .toolbar-spacer {
        flex: 1;
    }

    .finish-banner {
        display: flex;
        align-items: center;
        justify-content: center;
        gap: 12px;
        padding: 12px 16px;
        border-radius: 6px;
        font-weight: 600;
        font-size: 14px;
        text-align: center;
    }

    .finish-banner.pass {
        background: #e0f2f1;
        color: #00695c;
    }

    .finish-banner.fail {
        background: #fce4ec;
        color: #c62828;
    }

    .finish-banner.aborted {
        background: #fff3e0;
        color: #e65100;
    }

    .steps {
        display: flex;
        flex-direction: column;
        gap: 12px;
    }

    /* Buttons */
    .btn {
        padding: 6px 16px;
        border-radius: 4px;
        font: inherit;
        font-size: 13px;
        font-weight: 600;
        cursor: pointer;
        border: 1px solid transparent;
    }

    .btn:disabled {
        opacity: 0.5;
        cursor: not-allowed;
    }

    .btn-secondary {
        background: #fff;
        color: #333;
        border-color: #ccc;
    }

    .btn-secondary:hover {
        background: #f5f5f5;
    }

    .btn-success {
        background: #2e7d32;
        color: #fff;
    }

    .btn-success:hover {
        background: #1b5e20;
    }

    .btn-danger {
        background: #c62828;
        color: #fff;
    }

    .btn-danger:hover:not(:disabled) {
        background: #b71c1c;
    }

    .btn-undo {
        background: #fff;
        color: #6a1b9a;
        border-color: #ce93d8;
    }

    .btn-undo:hover {
        background: #f3e5f5;
    }
</style>
