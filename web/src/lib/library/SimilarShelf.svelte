<script>
	import CoverThumbnail from '$lib/components/CoverThumbnail.svelte';
	import Shelf from '$lib/components/ui/Shelf.svelte';
	import ScoreBadge from '$lib/components/ScoreBadge.svelte';
	import { media } from '$lib/api.js';
	import { cardHref, coverId } from '$lib/cards.js';

	let { items } = $props();
</script>

<Shelf title="More like this" subtitle="closest matches" collapsible={false}>
	{#each items as s (s.type + ':' + s.id)}
		<a class="simcard" href={cardHref(s)} title={s.name} draggable="false">
			<CoverThumbnail src={media.thumbnail(coverId(s), s.cover_version)} alt={s.name}>
				<ScoreBadge score={s.score} />
			</CoverThumbnail>
			<p class="simtitle">{s.name}</p>
		</a>
	{/each}
</Shelf>

<style>
	.simcard {
		flex: 0 0 auto;
		width: 150px;
		display: block;
	}
	.simcard :global(.cover) {
		transition:
			transform var(--ease),
			border-color var(--ease);
	}
	.simcard:hover :global(.cover) {
		transform: translateY(-3px);
		border-color: var(--accent);
	}
	.simtitle {
		margin: var(--space-2) 0 0;
		font-size: 0.72rem;
		line-height: 1.3;
		color: var(--text);
		display: -webkit-box;
		-webkit-line-clamp: 2;
		-webkit-box-orient: vertical;
		overflow: hidden;
	}
</style>
