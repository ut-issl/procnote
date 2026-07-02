<script lang="ts">
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import { warn } from '@tauri-apps/plugin-log';
	import type { TemplateSummary, ExecutionSummary } from '$lib/types';
	import * as api from '$lib/api/commands';
	import { executionStore } from '$lib/stores/execution.svelte';
	import { formatTimestamp } from '$lib/utils/format';
	import TemplateList from '$lib/components/TemplateList.svelte';

	let templates: TemplateSummary[] = $state([]);
	let executions: ExecutionSummary[] = $state([]);
	let loadingTemplates = $state(true);
	let loadingExecutions = $state(true);
	let error: string | null = $state(null);
	let selectedProcedure: string = $state("all");

	let procedures = $derived(
		[...new Map(executions.map(e => [e.procedure_id, e.procedure_title])).entries()]
			.map(([id, title]) => ({ id, title }))
			.sort((a, b) => a.title.localeCompare(b.title))
	);

	// Sort executions by started_at desc (most recent first). Executions without a
	// started_at fall back to the empty string so they sort last.
	let sortedExecutions = $derived(
		[...executions].sort((a, b) => (b.started_at ?? '').localeCompare(a.started_at ?? ''))
	);

	let filteredExecutions = $derived(
		selectedProcedure === "all"
			? sortedExecutions
			: sortedExecutions.filter(e => e.procedure_id === selectedProcedure)
	);

	onMount(async () => {
		try {
			templates = await api.listTemplates();
		} catch (e) {
			error = String(e);
		} finally {
			loadingTemplates = false;
		}

		try {
			executions = await api.listExecutions();
		} catch (e) {
			warn(`[home] listExecutions failed: ${e}`);
		} finally {
			loadingExecutions = false;
		}
	});

	async function handleStart(template: TemplateSummary) {
		const started = await executionStore.start(template.path);
		if (started && executionStore.summary) {
			goto(`/execution/${executionStore.summary.execution_id}`);
		}
	}

	function resumeExecution(exec: ExecutionSummary) {
		goto(`/execution/${exec.execution_id}`);
	}
</script>

<div class="home">
	<section class="section">
		<h2 class="section-title">Procedure Templates</h2>
		{#if executionStore.error}
			<p class="error">{executionStore.error}</p>
		{/if}
		{#if loadingTemplates}
			<p class="loading">Loading templates...</p>
		{:else if error}
			<p class="error">{error}</p>
		{:else if templates.length === 0}
			<p class="empty">No procedure templates found. Place <code>.md</code> files in the procedures directory.</p>
		{:else}
			<TemplateList {templates} onstart={handleStart} />
		{/if}
	</section>

	{#if !loadingExecutions && executions.length > 0}
		<section class="section">
			<div class="section-header">
				<h2 class="section-title">Recent Executions</h2>
				{#if procedures.length > 1}
					<select bind:value={selectedProcedure} class="procedure-filter">
						<option value="all">All procedures</option>
						{#each procedures as proc}
							<option value={proc.id}>{proc.title} ({proc.id})</option>
						{/each}
					</select>
				{/if}
			</div>
			<div class="execution-list">
				{#each filteredExecutions as exec}
					<button class="execution-card" class:execution-active={exec.status === 'active'} onclick={() => resumeExecution(exec)}>
						<div class="exec-header">
							<span class="exec-name">{exec.name ?? exec.procedure_id}</span>
							<span class="exec-status" class:status-active={exec.status === 'active'} class:status-pass={exec.status === 'pass'} class:status-fail={exec.status === 'fail'} class:status-aborted={exec.status === 'aborted'}>
								{exec.status}
							</span>
						</div>
						<div class="exec-meta">
							<span>{exec.procedure_title} ({exec.procedure_id}) v{exec.procedure_version}</span>
							{#if exec.finished_at && exec.started_at}
								<span class="exec-time">{formatTimestamp(exec.started_at)} — {formatTimestamp(exec.finished_at)}</span>
							{:else if exec.started_at}
								<span class="exec-time">Started {formatTimestamp(exec.started_at)}</span>
							{/if}
						</div>
					</button>
				{/each}
			</div>
		</section>
	{/if}
</div>

<style>
	.home {
		display: flex;
		flex-direction: column;
		gap: 32px;
	}

	.section-header {
		display: flex;
		justify-content: space-between;
		align-items: center;
		margin-bottom: 16px;
	}

	.section-header .section-title {
		margin: 0;
	}

	.section-title {
		font-size: 16px;
		font-weight: 600;
		margin: 0 0 16px;
		color: #333;
	}

	.procedure-filter {
		font-size: 13px;
		padding: 4px 8px;
		border: 1px solid #ccc;
		border-radius: 4px;
		background: #fff;
		color: #333;
	}

	.loading, .empty {
		color: #666;
		font-style: italic;
	}

	.error {
		color: #c0392b;
	}

	.execution-list {
		display: flex;
		flex-direction: column;
		gap: 8px;
	}

	.execution-card {
		display: block;
		width: 100%;
		text-align: left;
		padding: 12px 16px;
		background: #fff;
		border: 1px solid #ddd;
		border-radius: 6px;
		cursor: pointer;
		font: inherit;
	}

	.execution-card.execution-active {
		border-left: 3px solid #2e7d32;
		background: #f9fdf9;
	}

	.execution-card:hover {
		border-color: #aaa;
		background: #fafafa;
	}

	.exec-header {
		display: flex;
		justify-content: space-between;
		align-items: center;
		margin-bottom: 4px;
	}

	.exec-name {
		font-weight: 600;
	}

	.exec-status {
		font-size: 12px;
		font-weight: 600;
		padding: 2px 8px;
		border-radius: 10px;
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

	.exec-meta {
		display: flex;
		gap: 16px;
		font-size: 12px;
		color: #888;
	}

	.exec-time {
		color: #999;
	}
</style>
