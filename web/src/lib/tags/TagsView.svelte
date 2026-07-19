<script>
	import { get } from 'svelte/store';
	import { tags as tagsApi } from '$lib/api.js';
	import { isGuest } from '$lib/session.js';
	import { kindLabel } from '$lib/kinds.js';
	import { tagHref } from '$lib/tags.js';
	import Pagination from '$lib/components/ui/Pagination.svelte';
	import Loading from '$lib/components/ui/Loading.svelte';
	import LibraryTabs from '$lib/library/LibraryTabs.svelte';
	import PageHeader from '$lib/components/ui/PageHeader.svelte';
	import { gridColumns, rowAlignedPageSize } from '$lib/grid.js';

	let { kind } = $props();

	let all = $state([]);
	let favs = $state([]);
	let loading = $state(true);
	let error = $state(null);

	let query = $state('');
	let sort = $state('used');

	async function load(k) {
		loading = true;
		error = null;
		all = [];
		favs = [];
		try {
			const [a, f] = await Promise.all([
				tagsApi.list(k),
				get(isGuest) ? Promise.resolve([]) : tagsApi.favorites(k),
			]);
			all = a;
			favs = f;
		} catch (e) {
			error = e?.message ?? String(e);
		} finally {
			loading = false;
		}
	}
	$effect(() => {
		load(kind);
	});

	const unique = $derived(all.length);
	const totalUses = $derived(all.reduce((s, t) => s + (t.count || 0), 0));
	const fmtUses = (n) => (n >= 1000 ? `${(n / 1000).toFixed(1).replace(/\.0$/, '')}k` : String(n));

	const filtered = $derived.by(() => {
		const q = query.trim().toLowerCase();
		const list = q ? all.filter((t) => t.value.toLowerCase().includes(q)) : all.slice();
		if (sort === 'az') list.sort((x, y) => x.value.localeCompare(y.value));
		return list;
	});

	let gridCols = $state(3);
	const PAGE_SIZE = $derived(rowAlignedPageSize(gridCols, 100, 10, 50));
	let pageNum = $state(1);
	const pageCount = $derived(Math.max(1, Math.ceil(filtered.length / PAGE_SIZE)));
	const pageItems = $derived(filtered.slice((pageNum - 1) * PAGE_SIZE, pageNum * PAGE_SIZE));
	$effect(() => {
		query;
		sort;
		pageNum = 1;
	});
	$effect(() => {
		if (pageNum > pageCount) pageNum = pageCount;
	});

	const CLOUD_MAX = 44;
	const cloud = $derived.by(() => {
		const tasteful = favs.filter((t) => t.namespace !== 'language');
		if (!tasteful.length) return [];
		const top = tasteful.slice(0, CLOUD_MAX);
		const max = top[0].count;
		const min = top[top.length - 1].count;
		const span = Math.max(1, max - min);
		const sized = top.map((t) => {
			const f = Math.sqrt((t.count - min) / span);
			return {
				...t,
				size: 0.85 + f * 1.55,
				tier: f > 0.62 ? 'hot' : f > 0.28 ? 'warm' : 'cool',
			};
		});
		const out = [];
		sized.forEach((t, i) => (i % 2 === 0 ? out.push(t) : out.unshift(t)));
		return out;
	});
</script>

<div class="page">
	<PageHeader title={kindLabel(kind)} />
	<LibraryTabs {kind} active="tags" />

	{#if error}
		<p class="err">{error}</p>
	{/if}

	<section class="cloudwrap">
		<div class="cloudhead">
			<p class="eyebrow accent">Your favorite tags</p>
			<p class="cloudsub">from archives you've finished or favorited</p>
		</div>
		{#if cloud.length}
			<div class="cloud">
				{#each cloud as t (t.namespace + ':' + t.value)}
					<a
						class="cloudtag {t.tier}"
						style={`font-size:${t.size}rem`}
						href={tagHref(kind, t)}
						title={`${t.value} · ${t.count}`}>{t.value}</a
					>
				{/each}
			</div>
		{:else if !loading}
			<p class="empty">Favorite or finish some archives — your most-read tags show up here.</p>
		{/if}
	</section>

	<section class="allwrap">
		<div class="allhead">
			<p class="eyebrow">All tags</p>
			<span class="stats">{unique} unique · {fmtUses(totalUses)} uses</span>
			<div class="searchbox">
				<svg
					class="sico"
					viewBox="0 0 24 24"
					fill="none"
					stroke="currentColor"
					stroke-width="2"
					stroke-linecap="round"
					stroke-linejoin="round"
					aria-hidden="true"
				>
					<circle cx="11" cy="11" r="7" />
					<path d="M21 21l-4.3-4.3" />
				</svg>
				<input
					placeholder="Search tags…"
					bind:value={query}
					autocapitalize="off"
					spellcheck="false"
				/>
			</div>
			<div class="sortseg" role="group" aria-label="Sort tags">
				<button class:on={sort === 'used'} onclick={() => (sort = 'used')} type="button"
					>Most used</button
				>
				<button class:on={sort === 'az'} onclick={() => (sort = 'az')} type="button">A–Z</button>
			</div>
		</div>

		{#if loading}
			<Loading />
		{:else if filtered.length}
			<div class="taggrid" use:gridColumns={(n) => (gridCols = n)}>
				{#each pageItems as t (t.namespace + ':' + t.value)}
					<a class="tagchip" href={tagHref(kind, t)} title={`${t.namespace}:${t.value}`}>
						<span class="tv">{t.value}</span>
						<span class="tc">{t.count}</span>
					</a>
				{/each}
			</div>
			{#if pageCount > 1}
				<div class="pager">
					<Pagination jump page={pageNum} {pageCount} onnavigate={(p) => (pageNum = p)} />
				</div>
			{/if}
		{:else}
			<p class="muted pad">{query ? `No tags match “${query}”.` : 'No tags yet in this kind.'}</p>
		{/if}
	</section>
</div>

<style>
	.page {
		padding: var(--space-6) clamp(var(--space-4), 5vw, var(--space-8));
	}
	@media (max-width: 1200px) {
		.page {
			padding-inline: clamp(var(--space-4), 5vw, var(--space-6));
		}
	}
	.eyebrow {
		margin: 0 0 var(--space-1);
		font-size: 0.7rem;
		text-transform: uppercase;
		letter-spacing: 0.18em;
		color: var(--muted);
	}
	.eyebrow.accent {
		color: var(--accent);
	}
	.err {
		color: #ff8088;
	}
	.muted {
		color: var(--muted);
	}
	.pad {
		padding: var(--space-4) 0;
	}

	.cloudwrap {
		border: 1px solid var(--border);
		border-radius: var(--radius-lg);
		background: color-mix(in srgb, var(--surface) 55%, transparent);
		padding: var(--space-3) var(--space-5) var(--space-6);
		margin-bottom: var(--space-6);
	}
	.cloudhead {
		border-bottom: 1px solid var(--border);
		padding-bottom: var(--space-3);
		margin-bottom: var(--space-5);
	}
	.cloudsub {
		margin: 0;
		font-size: 0.8rem;
		color: var(--muted);
	}
	.cloud {
		display: flex;
		flex-wrap: wrap;
		align-items: baseline;
		justify-content: center;
		gap: 0.25em 0.7em;
		max-width: 40rem;
		margin: 0 auto;
		padding: var(--space-3) var(--space-2) var(--space-2);
	}
	.cloudtag {
		font-family: var(--font-display);
		line-height: 1.05;
		transition:
			color var(--ease),
			transform var(--ease);
		white-space: nowrap;
	}
	.cloudtag.cool {
		color: var(--muted);
	}
	.cloudtag.warm {
		color: var(--text);
	}
	.cloudtag.hot {
		color: var(--accent);
	}
	.cloudtag:hover {
		color: var(--accent);
		transform: translateY(-1px);
	}
	.empty {
		margin: 0;
		padding: var(--space-4) 0;
		text-align: center;
		color: var(--muted);
		font-size: 0.9rem;
	}

	.allhead {
		display: flex;
		align-items: center;
		gap: var(--space-3);
		flex-wrap: wrap;
		margin-bottom: var(--space-4);
	}
	.allhead .eyebrow {
		margin: 0;
		flex: 0 0 auto;
	}
	.stats {
		flex: 0 0 auto;
		font-size: 0.78rem;
		color: var(--muted);
		font-variant-numeric: tabular-nums;
	}
	.searchbox {
		position: relative;
		flex: 1 1 12rem;
		min-width: 0;
		display: flex;
		align-items: center;
	}
	.sico {
		position: absolute;
		left: 0.7rem;
		width: 1rem;
		height: 1rem;
		color: var(--muted);
		pointer-events: none;
	}
	.searchbox input {
		width: 100%;
		padding-left: 2.1rem;
	}
	.sortseg {
		flex: 0 0 auto;
		display: inline-flex;
		border: 1px solid var(--border);
		border-radius: var(--radius);
		overflow: hidden;
	}
	.sortseg button {
		all: unset;
		cursor: pointer;
		padding: var(--space-2) var(--space-4);
		font-size: 0.82rem;
		color: var(--muted);
		transition:
			background var(--ease),
			color var(--ease);
	}
	.sortseg button:hover {
		color: var(--text);
	}
	.sortseg button.on {
		background: var(--accent-soft);
		color: var(--accent);
	}
	@media (max-width: 560px) {
		.sortseg {
			flex: 1 1 100%;
		}
		.sortseg button {
			flex: 1 1 0;
			text-align: center;
		}
	}

	.taggrid {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(min(100%, 15rem), 1fr));
		gap: var(--space-2);
	}
	.tagchip {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--space-2);
		padding: var(--space-3);
		border: 1px solid var(--border);
		border-radius: var(--radius);
		background: var(--surface);
		transition:
			border-color var(--ease),
			background var(--ease);
	}
	.tagchip:hover {
		border-color: var(--accent);
		background: var(--surface-2);
	}
	.tv {
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		font-size: 0.9rem;
		color: var(--text);
	}
	.tc {
		flex: 0 0 auto;
		font-variant-numeric: tabular-nums;
		font-size: 0.75rem;
		color: var(--muted);
	}
	.pager {
		display: flex;
		justify-content: center;
		margin-top: var(--space-4);
	}
</style>
