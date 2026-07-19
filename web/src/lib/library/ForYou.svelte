<script>
	import { items as itemsApi, media, series as seriesApi } from '$lib/api.js';
	import { cardHref, coverId, loadSimilarCards } from '$lib/cards.js';
	import { kindLabel } from '$lib/kinds.js';
	import CoverThumbnail from '$lib/components/CoverThumbnail.svelte';
	import ScoreBadge from '$lib/components/ScoreBadge.svelte';
	import Loading from '$lib/components/ui/Loading.svelte';
	import PageHeader from '$lib/components/ui/PageHeader.svelte';
	import Shelf from '$lib/components/ui/Shelf.svelte';
	import LibraryTabs from '$lib/library/LibraryTabs.svelte';

	let { kind = null } = $props();

	const heading = $derived(kind ? kindLabel(kind) : 'Library');
	const thumb = (id, v) => media.thumbnail(id, v);

	let recommended = $state([]);
	let seedRows = $state([]);
	let loading = $state(true);

	$effect(() => {
		let live = true;
		(async () => {
			try {
				const [data, fin] = await Promise.all([
					itemsApi.recommendations(15, kind || undefined),
					itemsApi.finished(5, kind || undefined),
				]);
				if (!live) return;
				recommended = data.items ?? [];
				const seeds = fin.items ?? [];
				const rows = await Promise.all(
					seeds.map(async (seed) => {
						try {
							const apiFn = seed.type === 'series' ? seriesApi.similar : itemsApi.similar;
							return { seed, cards: await loadSimilarCards(apiFn, seed.id) };
						} catch {
							return { seed, cards: [] };
						}
					}),
				);
				if (live) seedRows = rows.filter((r) => r.cards.length);
			} catch {
				/* ignored */
			} finally {
				if (live) loading = false;
			}
		})();
		return () => (live = false);
	});
</script>

<div class="page">
	<PageHeader title={heading} />
	<LibraryTabs {kind} active="foryou" />

	{#if loading}
		<Loading />
	{:else if recommended.length || seedRows.length}
		{#if recommended.length}
			<Shelf title="For You" storageKey="arca:shelf:recommendations">
				{#each recommended as a (a.type + ':' + a.id)}
					<a class="shelfcard" href={cardHref(a)} draggable="false" title={a.name}>
						<CoverThumbnail src={thumb(coverId(a), a.cover_version)} alt={a.name} eager>
							<ScoreBadge score={a.score} />
						</CoverThumbnail>
						<p class="cardtitle">{a.name}</p>
					</a>
				{/each}
			</Shelf>
		{/if}

		{#each seedRows as row (row.seed.type + ':' + row.seed.id)}
			<Shelf
				title={`Because you recently finished ${row.seed.name}`}
				storageKey={`arca:shelf:seed:${row.seed.type}:${row.seed.id}`}
			>
				{#each row.cards as a (a.type + ':' + a.id)}
					<a class="shelfcard" href={cardHref(a)} draggable="false" title={a.name}>
						<CoverThumbnail src={thumb(coverId(a), a.cover_version)} alt={a.name} eager>
							<ScoreBadge score={a.score} />
						</CoverThumbnail>
						<p class="cardtitle">{a.name}</p>
					</a>
				{/each}
			</Shelf>
		{/each}
	{:else}
		<p class="empty">
			Nothing to recommend yet — finish or favorite a few
			{kind ? kindLabel(kind).toLowerCase() : 'items'} and picks will show up here.
		</p>
	{/if}
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
	.shelfcard {
		flex: 0 0 auto;
		width: 150px;
		-webkit-user-drag: none;
	}
	.cardtitle {
		margin: var(--space-2) 0 0;
		font-size: 0.72rem;
		line-height: 1.3;
		color: var(--text);
		display: -webkit-box;
		-webkit-line-clamp: 2;
		-webkit-box-orient: vertical;
		overflow: hidden;
	}
	.empty {
		color: var(--muted);
		font-size: 0.9rem;
	}
</style>
