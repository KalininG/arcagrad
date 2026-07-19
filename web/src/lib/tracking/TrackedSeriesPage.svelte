<script>
	import { onMount } from 'svelte';
	import { series as seriesApi, upcoming as upcomingApi, media } from '$lib/api.js';
	import { kindLabel } from '$lib/kinds.js';
	import Loading from '$lib/components/ui/Loading.svelte';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';
	import TrackerModal from '$lib/components/TrackerModal.svelte';
	import ScheduleFooter from '$lib/components/ScheduleFooter.svelte';
	import { relativeTime } from '$lib/format.js';

	let rows = $state([]);
	let loading = $state(true);
	let error = $state('');
	let search = $state('');
	let filtersOpen = $state(false);
	let source = $state('all');
	let status = $state('all');
	let editTarget = $state(null);
	let refreshing = $state(false);

	onMount(load);

	async function load() {
		try {
			rows = (await seriesApi.allTrackers()) ?? [];
			error = '';
		} catch (e) {
			error = e?.message ?? String(e);
		} finally {
			loading = false;
		}
	}

	async function refreshAll() {
		if (refreshing) return;
		refreshing = true;
		try {
			await upcomingApi.refresh();
			error = '';
		} catch (e) {
			error = e?.message ?? String(e);
		} finally {
			refreshing = false;
		}
	}

	function closeEditor() {
		editTarget = null;
		loading = true;
		load();
	}

	const sources = $derived.by(() => {
		const seen = new Map();
		for (const row of rows) seen.set(row.plugin_id, row.plugin_name);
		return [...seen.entries()].sort((a, b) => a[1].localeCompare(b[1]));
	});
	const shown = $derived(
		rows.filter((row) => {
			const query = search.trim().toLocaleLowerCase();
			return (
				(!query ||
					row.title.toLocaleLowerCase().includes(query) ||
					row.plugin_name.toLocaleLowerCase().includes(query)) &&
				(source === 'all' || row.plugin_id === source) &&
				(status === 'all' || row.status === status)
			);
		}),
	);
	const attention = $derived(shown.filter((row) => row.status !== 'active'));
	const active = $derived(shown.filter((row) => row.status === 'active'));
	const lastCheckedAt = $derived(
		rows.reduce((latest, row) => Math.max(latest, row.last_checked_at ?? 0), 0) || null,
	);
	const nextCheckAt = $derived.by(() => {
		const next = new Date();
		next.setHours(3, 0, 0, 0);
		if (next.getTime() <= Date.now()) next.setDate(next.getDate() + 1);
		return next.toISOString();
	});

	const volumeLabel = (row) => `${row.leaf_count} ${row.leaf_count === 1 ? 'item' : 'items'}`;
	const checkedLabel = (ts) => (ts ? `Checked ${relativeTime(ts)}` : 'Not checked yet');
	const releaseLabel = (row) => {
		if (!row.next_release_date) return 'Nothing announced';
		const [year, month, day] = row.next_release_date.split('-').map(Number);
		const date = new Intl.DateTimeFormat(undefined, { month: 'short', day: 'numeric' }).format(
			new Date(year, month - 1, day),
		);
		return `${row.next_label ?? 'Release'} · ${date}`;
	};
</script>

<div class="tracked-page">
	<header class="content-head">
		<div>
			<h2>Tracked series</h2>
			<p>Every series supplying release dates to your calendar.</p>
		</div>
		<div class="controls">
			<label class="searchbox">
				<svg viewBox="0 0 24 24" aria-hidden="true"
					><circle cx="11" cy="11" r="7" /><path d="m20 20-4-4" /></svg
				>
				<input
					type="search"
					bind:value={search}
					placeholder="Search tracked series"
					aria-label="Search tracked series"
				/>
			</label>
			<button
				class:active={filtersOpen || source !== 'all' || status !== 'all'}
				type="button"
				onclick={() => (filtersOpen = !filtersOpen)}
			>
				<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M4 5h16l-6 7v6l-4 2v-8Z" /></svg>
				Filter
			</button>
		</div>
	</header>

	{#if filtersOpen}
		<div class="filters">
			<label
				>Source<select bind:value={source}
					><option value="all">All sources</option>{#each sources as [id, name] (id)}<option
							value={id}>{name}</option
						>{/each}</select
				></label
			>
			<label
				>Status<select bind:value={status}
					><option value="all">All statuses</option><option value="active">Active</option><option
						value="pending">Pending</option
					><option value="error">Needs attention</option></select
				></label
			>
			{#if source !== 'all' || status !== 'all'}<button
					type="button"
					onclick={() => {
						source = 'all';
						status = 'all';
					}}>Clear</button
				>{/if}
		</div>
	{/if}

	{#if error}<p class="error">{error}</p>{/if}
	{#if loading}
		<Loading />
	{:else if !rows.length}
		<EmptyState
			title="No tracked series"
			message="Open a series and choose Track releases to connect it to a calendar source."
		/>
	{:else if !shown.length}
		<div class="no-results">
			<strong>No tracked series match</strong>
			<p>Try clearing the search or filters.</p>
		</div>
	{:else}
		{#if attention.length}
			<section class="group attention-group">
				<header>
					<span class="status-dot warning"></span>
					<h3>Needs attention</h3>
					<small>{attention.length}</small>
				</header>
				<div class="tracker-list">
					{#each attention as row (`${row.series_id}:${row.plugin_id}`)}
						<article class="tracker-row">
							<a class="cover" href={`/series/${row.series_id}`}
								>{#if row.cover_item_id}<img
										src={media.thumbnail(row.cover_item_id, row.cover_version)}
										alt=""
									/>{:else}<span>{row.title.slice(0, 2).toUpperCase()}</span>{/if}</a
							>
							<div class="identity">
								<a href={`/series/${row.series_id}`}>{row.title}</a><span
									>{kindLabel(row.kind)} · {volumeLabel(row)}</span
								>{#if row.last_error}<small class="problem"
										><svg viewBox="0 0 24 24"
											><path d="M12 3 2 21h20L12 3Z" /><path d="M12 9v5m0 3h.01" /></svg
										>{row.last_error}</small
									>{:else}<small class="pending">Waiting for its first calendar check</small>{/if}
							</div>
							<div class="provider">
								<img src={media.pluginIcon(row.plugin_id)} alt="" /><span
									><strong>{row.plugin_name}</strong><small
										>{checkedLabel(row.last_checked_at)}</small
									></span
								>
							</div>
							<div class="next">
								<small>Next release</small><strong>{releaseLabel(row)}</strong>
							</div>
							<button class="manage" type="button" onclick={() => (editTarget = row)}>Manage</button
							>
						</article>
					{/each}
				</div>
			</section>
		{/if}

		{#if active.length}
			<section class="group">
				<header>
					<span class="status-dot success"></span>
					<h3>Active</h3>
					<small>{active.length}</small>
				</header>
				<div class="tracker-list">
					{#each active as row (`${row.series_id}:${row.plugin_id}`)}
						<article class="tracker-row">
							<a class="cover" href={`/series/${row.series_id}`}
								>{#if row.cover_item_id}<img
										src={media.thumbnail(row.cover_item_id, row.cover_version)}
										alt=""
									/>{:else}<span>{row.title.slice(0, 2).toUpperCase()}</span>{/if}</a
							>
							<div class="identity">
								<a href={`/series/${row.series_id}`}>{row.title}</a><span
									>{kindLabel(row.kind)} · {volumeLabel(row)}</span
								>
							</div>
							<div class="provider">
								<img src={media.pluginIcon(row.plugin_id)} alt="" /><span
									><strong>{row.plugin_name}</strong><small
										>{checkedLabel(row.last_checked_at)}</small
									></span
								>
							</div>
							<div class="next">
								<small>Next release</small><strong class:quiet={!row.next_release_date}
									>{releaseLabel(row)}</strong
								>
							</div>
							<button class="manage" type="button" onclick={() => (editTarget = row)}>Manage</button
							>
						</article>
					{/each}
				</div>
			</section>
		{/if}
	{/if}

	<ScheduleFooter
		updatedLabel="Calendars updated"
		updatedAt={lastCheckedAt}
		nextAt={nextCheckAt}
		actionLabel="Refresh all"
		busy={refreshing}
		onaction={refreshAll}
	/>
</div>

{#if editTarget}
	<TrackerModal seriesId={editTarget.series_id} kind={editTarget.kind} onClose={closeEditor} />
{/if}

<style>
	.tracked-page {
		min-width: 0;
	}
	.content-head {
		display: flex;
		align-items: flex-end;
		justify-content: space-between;
		gap: var(--space-4);
		margin-bottom: var(--space-4);
	}
	.content-head h2 {
		margin: 0;
		font-family: var(--font-display);
		font-size: 1.2rem;
	}
	.content-head p {
		margin: 0.25rem 0 0;
		color: var(--muted);
		font-size: 0.8rem;
	}
	.controls {
		display: flex;
		align-items: center;
		gap: var(--space-2);
	}
	.controls > button {
		display: inline-flex;
		align-items: center;
		gap: var(--space-2);
		min-height: 2.55rem;
	}
	.controls > button.active {
		color: var(--accent);
		border-color: color-mix(in srgb, var(--accent) 55%, var(--border));
		background: var(--accent-soft);
	}
	.controls svg,
	.searchbox svg {
		width: 1rem;
		height: 1rem;
		fill: none;
		stroke: currentColor;
		stroke-width: 1.8;
		stroke-linecap: round;
		stroke-linejoin: round;
	}
	.searchbox {
		position: relative;
	}
	.searchbox svg {
		position: absolute;
		top: 50%;
		left: 0.75rem;
		color: var(--muted);
		transform: translateY(-50%);
		pointer-events: none;
	}
	.searchbox input {
		width: min(18rem, 30vw);
		min-height: 2.55rem;
		padding: 0.55rem 0.75rem 0.55rem 2.25rem;
		color: var(--text);
		background: var(--surface);
		border: 1px solid var(--border);
		border-radius: var(--radius);
		font: inherit;
		font-size: 0.8rem;
	}
	.searchbox input:focus {
		outline: none;
		border-color: var(--accent);
	}
	.filters {
		display: flex;
		align-items: flex-end;
		gap: var(--space-3);
		margin-bottom: var(--space-4);
		padding: var(--space-3);
		border: 1px solid var(--border);
		border-radius: var(--radius);
		background: var(--surface);
	}
	.filters label {
		display: flex;
		min-width: 10rem;
		flex-direction: column;
		gap: 0.3rem;
		color: var(--muted);
		font-size: 0.68rem;
		text-transform: uppercase;
		letter-spacing: 0.08em;
	}
	.filters select {
		padding: 0.45rem 1.6rem 0.45rem 0.65rem;
		color: var(--text);
		background-color: var(--surface-2);
		border: 1px solid var(--border);
		border-radius: var(--radius-sm);
		font: inherit;
		font-size: 0.78rem;
		text-transform: none;
		letter-spacing: normal;
	}
	.filters button {
		margin-left: auto;
		font-size: 0.75rem;
	}
	.error,
	.problem {
		color: var(--bad, #e0566f);
	}
	.no-results {
		padding: var(--space-8);
		border: 1px solid var(--border);
		border-radius: var(--radius);
		text-align: center;
	}
	.no-results p {
		margin: 0.3rem 0 0;
		color: var(--muted);
		font-size: 0.8rem;
	}
	.group {
		margin-bottom: var(--space-4);
	}
	.group > header {
		display: flex;
		align-items: center;
		gap: var(--space-2);
		padding: 0 var(--space-1) var(--space-2);
	}
	.group > header h3 {
		margin: 0;
		font-size: 0.86rem;
		font-weight: 600;
	}
	.group > header small {
		color: var(--muted);
		font-size: 0.68rem;
	}
	.status-dot {
		width: 0.48rem;
		height: 0.48rem;
		border-radius: 50%;
	}
	.status-dot.success {
		background: var(--good, #5fae7e);
	}
	.status-dot.warning {
		background: #d9a13d;
	}
	.tracker-list {
		overflow: hidden;
		border: 1px solid var(--border);
		border-radius: var(--radius);
		background: color-mix(in srgb, var(--surface) 45%, transparent);
	}
	.tracker-row {
		display: grid;
		grid-template-columns: 3.1rem minmax(10rem, 1.3fr) minmax(9rem, 0.8fr) minmax(9rem, 0.7fr) auto;
		align-items: center;
		gap: var(--space-3);
		padding: 0.65rem 0.8rem;
		border-bottom: 1px solid var(--border);
	}
	.tracker-row:last-child {
		border-bottom: 0;
	}
	.cover {
		position: relative;
		width: 3.1rem;
		aspect-ratio: var(--cover-aspect);
		display: grid;
		place-items: center;
		overflow: hidden;
		border-radius: var(--radius-sm);
		background: var(--surface-2);
		color: var(--muted);
		font-family: var(--font-display);
	}
	.cover img {
		position: absolute;
		inset: 0;
		width: 100%;
		height: 100%;
		object-fit: cover;
	}
	.identity,
	.provider,
	.provider span,
	.next {
		min-width: 0;
		display: flex;
		flex-direction: column;
		gap: 0.16rem;
	}
	.identity > a {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		font-family: var(--font-display);
		font-size: 0.94rem;
		font-weight: 600;
	}
	.identity > span,
	.provider small,
	.next small {
		color: var(--muted);
		font-size: 0.68rem;
	}
	.problem {
		display: flex;
		align-items: center;
		gap: 0.3rem;
		margin-top: 0.1rem;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		font-size: 0.68rem;
	}
	.problem svg {
		flex: none;
		width: 0.75rem;
		height: 0.75rem;
		fill: none;
		stroke: currentColor;
		stroke-width: 1.8;
		stroke-linecap: round;
		stroke-linejoin: round;
	}
	.pending {
		color: #d9a13d;
		font-size: 0.68rem;
	}
	.provider {
		flex-direction: row;
		align-items: center;
		gap: var(--space-2);
	}
	.provider img {
		width: 2rem;
		height: 2rem;
		flex: none;
		object-fit: cover;
		border-radius: var(--radius-sm);
		background: var(--surface-2);
	}
	.provider strong,
	.next strong {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		font-size: 0.75rem;
		font-weight: 500;
	}
	.next strong.quiet {
		color: var(--muted);
		font-weight: 400;
	}
	.manage {
		padding: 0.4rem 0.65rem;
		color: var(--muted);
		font-size: 0.72rem;
	}
	.manage:hover {
		color: var(--text);
		border-color: var(--accent);
	}
	@media (max-width: 900px) {
		.tracker-row {
			grid-template-columns: 3.1rem minmax(0, 1fr) auto;
		}
		.provider,
		.next {
			grid-column: 2;
		}
		.manage {
			grid-column: 3;
			grid-row: 1;
		}
	}
	@media (max-width: 640px) {
		.content-head {
			align-items: flex-start;
			flex-direction: column;
		}
		.controls,
		.searchbox,
		.searchbox input {
			width: 100%;
		}
		.controls > button {
			flex: none;
		}
		.filters {
			align-items: stretch;
			flex-direction: column;
		}
		.filters label {
			min-width: 0;
		}
		.filters button {
			width: 100%;
			margin-left: 0;
		}
	}
</style>
