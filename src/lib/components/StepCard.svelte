<script lang="ts">
    import DOMPurify from "dompurify";
    import { Marked } from "marked";
    import { markedHighlight } from "marked-highlight";
    import hljs from "highlight.js/lib/core";

    import python from "highlight.js/lib/languages/python";
    import rust from "highlight.js/lib/languages/rust";
    import bash from "highlight.js/lib/languages/bash";
    import yaml from "highlight.js/lib/languages/yaml";
    import json from "highlight.js/lib/languages/json";
    import javascript from "highlight.js/lib/languages/javascript";
    import typescript from "highlight.js/lib/languages/typescript";
    import toml from "highlight.js/lib/languages/ini";
    import sql from "highlight.js/lib/languages/sql";
    import xml from "highlight.js/lib/languages/xml";
    import css_lang from "highlight.js/lib/languages/css";
    import markdown_lang from "highlight.js/lib/languages/markdown";

    hljs.registerLanguage("python", python);
    hljs.registerLanguage("rust", rust);
    hljs.registerLanguage("bash", bash);
    hljs.registerLanguage("shell", bash);
    hljs.registerLanguage("sh", bash);
    hljs.registerLanguage("yaml", yaml);
    hljs.registerLanguage("yml", yaml);
    hljs.registerLanguage("json", json);
    hljs.registerLanguage("javascript", javascript);
    hljs.registerLanguage("js", javascript);
    hljs.registerLanguage("typescript", typescript);
    hljs.registerLanguage("ts", typescript);
    hljs.registerLanguage("toml", toml);
    hljs.registerLanguage("sql", sql);
    hljs.registerLanguage("xml", xml);
    hljs.registerLanguage("html", xml);
    hljs.registerLanguage("css", css_lang);
    hljs.registerLanguage("markdown", markdown_lang);
    hljs.registerLanguage("md", markdown_lang);

    import "highlight.js/styles/atom-one-light.css";

    import type { ExecutionAction, StepSummary } from "$lib/types";
    import { formatTimestamp } from "$lib/utils/format";
    import { isNonComposingEnter } from "$lib/utils/keyboard";
    import AttachmentField from "./AttachmentField.svelte";
    import CheckboxItem from "./CheckboxItem.svelte";
    import InputField from "./InputField.svelte";
    import NoteEditor from "./NoteEditor.svelte";

    const markedInstance = new Marked(
        markedHighlight({
            langPrefix: "hljs language-",
            highlight(code, lang) {
                const language = hljs.getLanguage(lang) ? lang : "plaintext";
                return hljs.highlight(code, { language }).value;
            },
        }),
    );

    function renderMarkdown(source: string): string {
        return DOMPurify.sanitize(
            markedInstance.parse(source, { async: false }) as string,
        );
    }

    let {
        stepSummary,
        executionActive = false,
        onaction,
    }: {
        stepSummary: StepSummary;
        executionActive?: boolean;
        onaction: (action: ExecutionAction) => void;
    } = $props();

    let isPresent = $derived(stepSummary.status === "present");
    let isSkipped = $derived(stepSummary.status === "skipped");
    let isInteractable = $derived(isPresent && executionActive);

    let showSkipDialog = $state(false);
    let skipReason = $state("");

    function confirmSkip() {
        if (!skipReason.trim()) return;
        onaction({
            action: "skip_step",
            step_id: stepSummary.id,
            reason: skipReason.trim(),
        });
        showSkipDialog = false;
        skipReason = "";
    }

    function toggleCheckbox(checkboxId: string, checked: boolean) {
        onaction({
            action: "toggle_checkbox",
            step_id: stepSummary.id,
            checkbox_id: checkboxId,
            checked,
        });
    }

    function recordInput(inputId: string, value: string, unit?: string) {
        onaction({
            action: "record_input",
            step_id: stepSummary.id,
            input_id: inputId,
            value,
            unit,
        });
    }

    function attachFile(inputId: string, filename: string, path: string, contentType: string) {
        onaction({
            action: "add_attachment",
            step_id: stepSummary.id,
            input_id: inputId,
            filename,
            path,
            content_type: contentType,
        });
    }

    function clearInput(inputId: string) {
        onaction({
            action: "clear_input",
            step_id: stepSummary.id,
            input_id: inputId,
            reason: "Cleared by operator",
        });
    }

    function removeAttachment(inputId: string) {
        onaction({
            action: "remove_attachment",
            step_id: stepSummary.id,
            input_id: inputId,
            reason: "Removed by operator",
        });
    }

    function addNote(text: string) {
        onaction({
            action: "add_note",
            text,
            step_id: stepSummary.id,
        });
    }

    function removeNote(noteId: string) {
        onaction({
            action: "remove_note",
            note_id: noteId,
            reason: "Removed by operator",
        });
    }

    function unskipStep() {
        onaction({
            action: "unskip_step",
            step_id: stepSummary.id,
            reason: "Unskipped by operator",
        });
    }
</script>

<div
    class="step-card"
    class:skipped={isSkipped}
>
    <div class="step-header">
        <div class="step-status-indicator"></div>
        <h3 class="step-heading">{stepSummary.heading}</h3>
        {#if stepSummary.status_at}
            <span class="timestamp">{formatTimestamp(stepSummary.status_at)}</span>
        {/if}
        {#if isSkipped}
            <span class="step-status-badge">skipped</span>
        {/if}
    </div>

    {#each stepSummary.content as block}
        {#if block.type === "Prose"}
            <div class="step-description">{@html renderMarkdown(block.text)}</div>
        {:else if block.type === "Checkbox"}
            <CheckboxItem
                checkbox={block}
                disabled={!isInteractable}
                ontoggle={toggleCheckbox}
            />
        {:else if block.type === "InputBlock"}
            <div class="step-section">
                {#each block.inputs as input}
                    {@const revertHandler = input.recorded && executionActive
                        ? () =>
                              input.definition.type === "attachment"
                                  ? removeAttachment(input.definition.id)
                                  : clearInput(input.definition.id)
                        : undefined}
                    {#if input.definition.type === "attachment"}
                        <AttachmentField
                            definition={input.definition}
                            recorded={input.recorded}
                            disabled={!isInteractable}
                            onattach={(filename, path, contentType) =>
                                attachFile(input.definition.id, filename, path, contentType)}
                            onrevert={revertHandler}
                        />
                    {:else}
                        <InputField
                            definition={input.definition}
                            recorded={input.recorded}
                            disabled={!isInteractable}
                            onrecord={recordInput}
                            onrevert={revertHandler}
                        />
                    {/if}
                {/each}
            </div>
        {/if}
    {/each}

    <div class="step-section">
        <NoteEditor
            notes={stepSummary.notes}
            disabled={!isInteractable}
            onadd={addNote}
            onrevert={executionActive
                ? (noteIndex) => {
                      const note = stepSummary.notes[noteIndex];
                      if (note) removeNote(note.id);
                  }
                : undefined}
        />
    </div>

    {#if executionActive}
        <div class="step-actions">
            {#if isPresent}
                <button
                    class="btn btn-muted"
                    onclick={() => (showSkipDialog = true)}>Skip</button
                >
            {/if}
            {#if isSkipped}
                <button
                    class="btn btn-undo"
                    onclick={unskipStep}
                >
                    Undo Skip
                </button>
            {/if}
        </div>
    {/if}

    {#if showSkipDialog}
        <div class="skip-dialog">
            <label class="field">
                <span class="field-label">Reason for skipping</span>
                <!-- svelte-ignore a11y_autofocus -->
                <input
                    type="text"
                    bind:value={skipReason}
                    placeholder="Enter reason..."
                    autofocus
                    onkeydown={(e) => {
                        if (isNonComposingEnter(e)) confirmSkip();
                    }}
                />
            </label>
            <div class="skip-actions">
                <button
                    class="btn btn-muted"
                    onclick={() => {
                        showSkipDialog = false;
                        skipReason = "";
                    }}>Cancel</button
                >
                <button
                    class="btn btn-warn"
                    onclick={confirmSkip}
                    disabled={!skipReason.trim()}>Skip Step</button
                >
            </div>
        </div>
    {/if}
</div>

<style>
    .step-card {
        background: #fff;
        border: 1px solid #ddd;
        border-radius: 8px;
        padding: 16px;
        transition: border-color 0.15s;
    }

    .step-card.skipped {
        border-color: #ffe0b2;
        background: #fffdf5;
        opacity: 0.8;
    }

    .step-header {
        display: flex;
        align-items: center;
        gap: 10px;
        margin-bottom: 12px;
    }

    .step-status-indicator {
        width: 10px;
        height: 10px;
        border-radius: 50%;
        background: #ccc;
        flex-shrink: 0;
    }

    .skipped .step-status-indicator {
        background: #e65100;
    }

    .step-heading {
        flex: 1;
        margin: 0;
        font-size: 15px;
        font-weight: 600;
    }

    .step-description {
        margin: 0 0 4px;
        font-size: 14px;
        color: #444;
        line-height: 1.5;
    }

    .step-description :global(p) {
        margin: 0 0 0.5em;
    }

    .step-description :global(p:last-child) {
        margin-bottom: 0;
    }

    .step-description :global(a) {
        color: #1a73e8;
    }

    .step-description :global(code) {
        background: #f0f0f0;
        padding: 1px 4px;
        border-radius: 3px;
        font-size: 13px;
    }

    .step-description :global(pre) {
        border-radius: 4px;
        overflow-x: auto;
        margin: 0.5em 0;
        font-size: 13px;
        line-height: 1.4;
    }

    .step-description :global(pre code) {
        padding: 0;
        border-radius: 0;
    }

    .step-description :global(pre code.hljs) {
        padding: 12px;
        border-radius: 4px;
    }

    .step-description :global(pre code:not(.hljs)) {
        background: #f0f0f0;
        padding: 12px;
        display: block;
    }

    .step-status-badge {
        font-size: 11px;
        font-weight: 600;
        text-transform: uppercase;
        color: #888;
    }

    .skipped .step-status-badge {
        color: #e65100;
    }

    .step-section {
        margin-top: 12px;
        display: flex;
        flex-direction: column;
        gap: 6px;
    }

    .step-actions {
        display: flex;
        gap: 8px;
        margin-top: 16px;
        padding-top: 12px;
        border-top: 1px solid #eee;
    }

    .btn {
        padding: 6px 16px;
        border-radius: 4px;
        font: inherit;
        font-size: 13px;
        font-weight: 600;
        cursor: pointer;
        border: 1px solid transparent;
    }

    .btn-muted {
        background: #fff;
        color: #666;
        border-color: #ccc;
    }

    .btn-muted:hover {
        background: #f5f5f5;
    }

    .btn-undo {
        background: #fff;
        color: #6a1b9a;
        border-color: #ce93d8;
        margin-left: auto;
    }

    .btn-undo:hover {
        background: #f3e5f5;
    }

    .btn-warn {
        background: #e65100;
        color: #fff;
    }

    .btn-warn:hover:not(:disabled) {
        background: #bf360c;
    }

    .btn-warn:disabled {
        opacity: 0.4;
        cursor: not-allowed;
    }

    .skip-dialog {
        margin-top: 12px;
        padding: 12px;
        background: #fff8e1;
        border: 1px solid #ffe082;
        border-radius: 4px;
    }

    .field {
        display: block;
        margin-bottom: 8px;
    }

    .field-label {
        display: block;
        font-size: 12px;
        font-weight: 600;
        margin-bottom: 4px;
        color: #555;
    }

    .field input {
        width: 100%;
        padding: 6px 10px;
        border: 1px solid #ccc;
        border-radius: 4px;
        font: inherit;
        font-size: 13px;
    }

    .field input:focus {
        outline: none;
        border-color: #1a1a2e;
        box-shadow: 0 0 0 2px rgba(26, 26, 46, 0.15);
    }

    .skip-actions {
        display: flex;
        justify-content: flex-end;
        gap: 8px;
    }
</style>
