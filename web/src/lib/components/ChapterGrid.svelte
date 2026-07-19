<script>
	import CoverThumbnail from './CoverThumbnail.svelte';
	import Pagination from './ui/Pagination.svelte';
	import PageThumbGrid from './PageThumbGrid.svelte';
	import { media } from '$lib/api.js';

	let {
		chapters = [],
		itemId,
		version,
		progress = null,
		onopen,
		thumbSrc = null,
		showProgress = true,
	} = $props();

	const thumbFor = (c) =>
		thumbSrc ? thumbSrc(c) : media.pageThumbnail(itemId, c.start_page, version);

	const PER_PAGE = 12;
	let asc = $state(true);
	let page = $state(1);

	let previewing = $state(null);
	const previewPages = $derived(
		previewing
			? Array.from({ length: previewing.page_count }, (_, k) => {
					const n = previewing.start_page + k;
					return { key: n, src: media.pageThumbnail(itemId, n, version), label: n + 1 };
				})
			: [],
	);

	const cur = $derived(progress ?? -1);
	const totalPages = $derived(
		chapters.reduce((m, c) => Math.max(m, c.start_page + c.page_count), 0),
	);
	const completed = $derived(cur >= 0 && totalPages > 0 && cur + 1 >= totalPages);
	const currentIdx = $derived(
		completed || cur < 0
			? -1
			: chapters.findIndex((c) => cur >= c.start_page && cur < c.start_page + c.page_count),
	);

	function statusOf(i) {
		if (completed) return 'read';
		if (currentIdx < 0) return 'unread';
		if (i < currentIdx) return 'read';
		if (i === currentIdx) return 'reading';
		return 'unread';
	}
	function label(c) {
		return c.title ?? c.number?.replace(/^Ch\.\s*/, 'Chapter ') ?? 'Chapter';
	}

	const numberedCount = $derived(chapters.filter((c) => c.number).length);
	const readCount = $derived(chapters.filter((c, i) => c.number && statusOf(i) === 'read').length);

	const rows = $derived(chapters.map((c, i) => ({ c, i, status: statusOf(i) })));
	const ordered = $derived(asc ? rows : [...rows].reverse());
	const pageCount = $derived(Math.max(1, Math.ceil(ordered.length / PER_PAGE)));
	const slice = $derived(ordered.slice((page - 1) * PER_PAGE, page * PER_PAGE));
	$effect(() => {
		if (page > pageCount) page = pageCount;
	});

	let landedFor = $state(null);
	$effect(() => {
		if (landedFor === itemId) return;
		landedFor = itemId;
		previewing = null;
		const pos = ordered.findIndex((r) => r.i === currentIdx);
		page = pos >= 0 ? Math.floor(pos / PER_PAGE) + 1 : 1;
	});
</script>

{#if previewing}
	<div class="chhead pvhead">
		<button class="ctl" type="button" onclick={() => (previewing = null)}>
			<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M15 18l-6-6 6-6" /></svg>
			Chapters
		</button>
		<div class="pvtitle">
			<b>{label(previewing)}</b>
			<span
				>pages {previewing.start_page + 1}–{previewing.start_page + previewing.page_count} · {previewing.page_count}
				pages</span
			>
		</div>
		<button class="ctl read" type="button" onclick={() => onopen?.(previewing.start_page)}>
			<svg viewBox="0 0 24 24" fill="currentColor" stroke="none" aria-hidden="true"
				><path d="M8 5v14l11-7z" /></svg
			>
			Read
		</button>
	</div>
	<PageThumbGrid pages={previewPages} onopen={(p) => onopen?.(p.key)} />
{:else}
	<div class="chhead">
		<h2 class="section">
			Chapters
			{#if showProgress}
				<span class="readcount"
					>{completed ? numberedCount : readCount} of {numberedCount} read</span
				>
			{:else}
				<span class="readcount">{numberedCount} chapters</span>
			{/if}
		</h2>
		<div class="controls">
			<button
				class="ctl"
				type="button"
				onclick={() => (asc = !asc)}
				title={asc ? 'Newest first' : 'Oldest first'}
			>
				<svg viewBox="0 0 24 24" aria-hidden="true"
					><path d="M7 4v16m0 0 3-3m-3 3-3-3M17 20V4m0 0 3 3m-3-3-3 3" /></svg
				>
				{asc ? `1 → ${numberedCount}` : `${numberedCount} → 1`}
			</button>
		</div>
	</div>

	<div class="chgrid">
		{#each slice as { c, i, status } (i)}
			<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
			<div
				class="chcell {status}"
				class:readonly={!onopen}
				role={onopen ? 'button' : undefined}
				tabindex={onopen ? 0 : undefined}
				title={!onopen
					? label(c)
					: thumbSrc
						? `Read ${label(c)}`
						: `${label(c)} · read from page ${c.start_page + 1}`}
				onclick={() => onopen?.(c.start_page, c)}
				onkeydown={(e) => {
					if (onopen && (e.key === 'Enter' || e.key === ' ')) {
						e.preventDefault();
						onopen(c.start_page, c);
					}
				}}
			>
				<div class="thumb">
					<CoverThumbnail src={thumbFor(c)} alt={label(c)} />
				</div>
				<span class="name">{label(c)}</span>
				<span class="state">
					{#if status === 'reading'}
						<span class="reading">page {cur - c.start_page + 1} / {c.page_count}</span>
					{:else if status === 'read'}
						<svg class="check" viewBox="0 0 24 24" aria-hidden="true"
							><path d="M5 12l5 5L20 7" /></svg
						>
					{:else}
						<span class="pages">{c.page_count} pages</span>
					{/if}
				</span>
				{#if onopen && !thumbSrc}
					<button
						class="peek"
						type="button"
						aria-label="Preview pages"
						onclick={(e) => {
							e.stopPropagation();
							previewing = c;
						}}
					>
						<svg viewBox="0 0 24 24" aria-hidden="true"
							><rect x="3" y="3" width="7" height="7" rx="1" /><rect
								x="14"
								y="3"
								width="7"
								height="7"
								rx="1"
							/><rect x="3" y="14" width="7" height="7" rx="1" /><rect
								x="14"
								y="14"
								width="7"
								height="7"
								rx="1"
							/></svg
						>
					</button>
				{/if}
			</div>
		{/each}
	</div>

	{#if pageCount > 1}
		<div class="chpager">
			<Pagination simple {page} {pageCount} onnavigate={(p) => (page = p)} />
		</div>
	{/if}
{/if}

<style>
	.chhead {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--space-3);
		margin-bottom: var(--space-3);
		padding-top: var(--space-3);
		border-top: 1px solid var(--border);
		margin-left: calc(-1 * var(--space-6));
		padding-left: var(--space-6);
		margin-right: calc(-1 * var(--space-5));
		padding-right: var(--space-5);
	}
	.section {
		margin: 0;
		font-size: 0.9rem;
		font-weight: 600;
		display: flex;
		align-items: baseline;
		gap: var(--space-2);
	}
	.readcount {
		font-weight: 400;
		font-size: 0.8rem;
		color: var(--muted);
		font-variant-numeric: tabular-nums;
	}
	.controls {
		display: flex;
		gap: var(--space-2);
	}
	.ctl {
		display: inline-flex;
		align-items: center;
		gap: 0.35rem;
		background: var(--surface-2);
		border: 1px solid var(--border);
		color: var(--text, inherit);
		border-radius: var(--radius);
		padding: 0.35rem 0.6rem;
		font-size: 0.78rem;
		cursor: pointer;
		font-variant-numeric: tabular-nums;
		white-space: nowrap;
	}
	.ctl:hover {
		border-color: var(--accent);
	}
	.ctl svg {
		width: 14px;
		height: 14px;
		fill: none;
		stroke: currentColor;
		stroke-width: 2;
		stroke-linecap: round;
		stroke-linejoin: round;
	}

	.chgrid {
		display: grid;
		grid-template-columns: repeat(2, minmax(0, 1fr));
		gap: var(--space-2);
	}
	@media (max-width: 640px) {
		.chgrid {
			grid-template-columns: 1fr;
		}
	}
	.chcell {
		box-sizing: border-box;
		display: flex;
		align-items: center;
		gap: var(--space-3);
		padding: var(--space-2);
		border: 1px solid var(--border);
		border-radius: var(--radius);
		background: var(--surface);
		cursor: pointer;
		transition:
			border-color var(--ease),
			background var(--ease);
	}
	.chcell:hover {
		border-color: var(--accent);
	}
	.chcell:focus-visible {
		outline: 2px solid var(--accent);
		outline-offset: 2px;
	}
	.chcell.readonly {
		cursor: default;
	}
	.chcell.readonly:hover {
		border-color: var(--border);
	}
	.peek {
		flex: none;
		width: 30px;
		height: 30px;
		padding: 0;
		display: grid;
		place-items: center;
		border: 1px solid transparent;
		border-radius: 8px;
		background: none;
		color: var(--muted);
		cursor: pointer;
		opacity: 0.55;
		transition:
			opacity var(--ease),
			color var(--ease),
			border-color var(--ease);
	}
	.peek:focus:not(:focus-visible) {
		outline: none;
	}
	.chcell:hover .peek {
		opacity: 1;
		border-color: var(--border);
	}
	.peek:hover {
		color: var(--text);
		border-color: var(--accent);
	}
	.peek svg {
		width: 16px;
		height: 16px;
		fill: none;
		stroke: currentColor;
		stroke-width: 1.8;
	}
	.pvtitle {
		flex: 1;
		min-width: 0;
		display: flex;
		flex-direction: column;
		gap: 1px;
	}
	.pvtitle b {
		font-size: 0.9rem;
		font-weight: 600;
	}
	.pvtitle span {
		font-size: 0.78rem;
		color: var(--muted);
		font-variant-numeric: tabular-nums;
	}
	.ctl.read {
		background: var(--accent);
		border-color: var(--accent);
		color: #fff;
		font-weight: 600;
	}
	.ctl.read:hover {
		filter: brightness(1.08);
		border-color: var(--accent);
	}
	.ctl.read svg {
		stroke: none;
		fill: currentColor;
	}
	.thumb {
		width: 40px;
		flex: none;
	}
	.name {
		flex: 1;
		min-width: 0;
		font-size: 0.9rem;
		font-weight: 500;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.state {
		flex: none;
		display: flex;
		align-items: center;
	}
	.pages {
		font-size: 0.8rem;
		color: var(--muted);
		font-variant-numeric: tabular-nums;
	}
	.check {
		width: 16px;
		height: 16px;
		fill: none;
		stroke: var(--muted);
		stroke-width: 2;
		stroke-linecap: round;
		stroke-linejoin: round;
	}
	.reading {
		font-size: 0.8rem;
		font-weight: 600;
		color: var(--accent);
		font-variant-numeric: tabular-nums;
	}

	.chcell.read .name {
		color: var(--muted);
		font-weight: 400;
	}
	.chcell.reading {
		background: var(--accent-soft);
		border-color: var(--accent);
	}
	.chcell.reading .name {
		color: var(--accent);
	}

	.chpager {
		display: flex;
		justify-content: center;
		margin-top: var(--space-3);
	}
</style>
