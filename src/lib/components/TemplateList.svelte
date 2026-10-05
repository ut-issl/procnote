<script lang="ts">
    import type { TemplateSummary } from "#lib/types/index.js";

    let {
        templates,
        onstart,
    }: { templates: TemplateSummary[]; onstart: (t: TemplateSummary) => void } =
        $props();
</script>

<div class="template-list">
    {#each templates as template}
        <div class="template-card">
            <div class="template-info">
                <span class="template-title">{template.title}</span>
                <span class="template-meta"
                    >{template.id} &middot; v{template.version}</span
                >
            </div>
            <button class="btn-start" onclick={() => onstart(template)}>
                Start
            </button>
        </div>
    {/each}
</div>

<style>
    .template-list {
        display: flex;
        flex-direction: column;
        gap: 8px;
    }

    .template-card {
        display: flex;
        justify-content: space-between;
        align-items: center;
        padding: 12px 16px;
        background: #fff;
        border: 1px solid #ddd;
        border-radius: 6px;
    }

    .template-card:hover {
        border-color: #bbb;
    }

    .template-info {
        display: flex;
        flex-direction: column;
        gap: 2px;
    }

    .template-title {
        font-weight: 600;
        font-size: 14px;
    }

    .template-meta {
        font-size: 12px;
        color: #888;
    }

    .btn-start {
        padding: 6px 16px;
        background: #1a1a2e;
        color: #fff;
        border: none;
        border-radius: 4px;
        font: inherit;
        font-size: 13px;
        font-weight: 600;
        cursor: pointer;
    }

    .btn-start:hover {
        background: #16213e;
    }
</style>
