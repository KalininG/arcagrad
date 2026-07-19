<script>
	import { goto } from '$app/navigation';
	import { media } from '$lib/api.js';
	import CoverThumbnail from '$lib/components/CoverThumbnail.svelte';
	import TypographicCover from '$lib/components/TypographicCover.svelte';
	import FavCount from '$lib/components/FavCount.svelte';

	let { pluginId, item, match = null, onpreview, actions } = $props();

	const ownedId = $derived(match?.owned_item_id ?? null);
	const likelyId = $derived(
		!ownedId && match?.likely_item_id != null ? match.likely_item_id : null,
	);
	const coverSrc = $derived(item.cover_url ? media.pluginImage(pluginId, item.cover_url) : '');
</script>

<div class="bcard" class:owned={ownedId != null} class:likely={likelyId != null}>
	<button
		class="bcover"
		type="button"
		title={ownedId != null ? 'Open in your library' : 'View details'}
		onclick={() => (ownedId != null ? goto(`/item/${ownedId}`) : onpreview?.(item))}
	>
		<CoverThumbnail src={coverSrc} alt={item.title}>
			{#snippet fallback()}
				<TypographicCover title={item.title} author={item.subtitle} />
			{/snippet}
			{#if item.favorites != null}
				<span class="fav"><FavCount count={item.favorites} /></span>
			{:else if item.rating != null}
				<span class="rating">★ {item.rating.toFixed(1)}</span>
			{/if}
			{#if item.page_count}<span class="pages">{item.page_count}p</span>{/if}
		</CoverThumbnail>
	</button>
	{#if ownedId != null}
		<a class="own owned" href={`/item/${ownedId}`} title="Open in your library">
			<svg
				viewBox="0 0 24 24"
				fill="none"
				stroke="currentColor"
				stroke-width="3"
				stroke-linecap="round"
				stroke-linejoin="round"><path d="M20 6 9 17l-5-5" /></svg
			>
			Owned
		</a>
	{:else if likelyId != null}
		<a
			class="own likely"
			href={`/item/${likelyId}`}
			title="Likely the same work — open in your library">likely owned</a
		>
	{/if}
	<p class="btitle" title={item.title}>{item.title}</p>
	{#if item.subtitle}<p class="bsubtitle" title={item.subtitle}>{item.subtitle}</p>{/if}
	{#if actions}<span class="cardacts">{@render actions()}</span>{/if}
</div>

<style>
	.bcard {
		display: block;
		min-width: 0;
		position: relative;
	}
	.bcard.owned :global(.cover) {
		border-color: rgba(16, 185, 129, 0.7);
		box-shadow:
			0 0 0 2px rgba(16, 185, 129, 0.4),
			var(--shadow);
	}
	.bcard.likely :global(.cover) {
		border-color: rgba(14, 165, 233, 0.7);
		box-shadow:
			0 0 0 2px rgba(14, 165, 233, 0.4),
			var(--shadow);
	}
	.bcard.owned .btitle {
		color: #6ee7b7;
	}
	.bcard.likely .btitle {
		color: #7dd3fc;
	}
	.bcover {
		all: unset;
		display: block;
		cursor: pointer;
		width: 100%;
	}
	.bcover:disabled {
		cursor: default;
	}
	.bcover :global(.cover) {
		transition:
			transform var(--ease),
			border-color var(--ease),
			box-shadow var(--ease);
	}
	.bcover:hover :global(.cover) {
		transform: translateY(-2px);
	}
	.bcard:not(.owned):not(.likely) .bcover:hover :global(.cover) {
		border-color: var(--accent);
	}
	.own {
		position: absolute;
		top: 0.375rem;
		right: 0.375rem;
		z-index: 3;
		display: inline-flex;
		align-items: center;
		gap: 0.2rem;
		padding: 0.12rem 0.45rem;
		border-radius: 9999px;
		font-size: 0.62rem;
		font-weight: 600;
		color: #fff;
		box-shadow: 0 2px 6px rgba(0, 0, 0, 0.35);
		text-decoration: none;
	}
	.own.owned {
		background: rgba(16, 185, 129, 0.92);
	}
	.own.likely {
		background: rgba(14, 165, 233, 0.92);
	}
	.own svg {
		width: 0.7rem;
		height: 0.7rem;
	}
	.own:hover {
		filter: brightness(1.08);
	}
	.btitle {
		margin: var(--space-2) 0 0;
		font-size: 0.8rem;
		line-height: 1.3;
		color: var(--text);
		display: -webkit-box;
		-webkit-line-clamp: 2;
		-webkit-box-orient: vertical;
		overflow: hidden;
		transition: color var(--ease);
	}
	.bsubtitle {
		margin: 0.1rem 0 0;
		font-size: 0.72rem;
		line-height: 1.25;
		color: var(--muted);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.fav {
		position: absolute;
		top: 0.375rem;
		left: 0.375rem;
		padding: 0.1rem 0.4rem;
		border-radius: 9999px;
		background: rgba(0, 0, 0, 0.66);
		backdrop-filter: blur(4px);
		color: #fff;
		font-size: 0.62rem;
		font-weight: 600;
	}
	.rating {
		position: absolute;
		top: 0.375rem;
		left: 0.375rem;
		padding: 0.1rem 0.4rem;
		border-radius: 9999px;
		background: rgba(0, 0, 0, 0.66);
		backdrop-filter: blur(4px);
		color: #ffd54a;
		font-size: 0.62rem;
		font-weight: 600;
		font-variant-numeric: tabular-nums;
	}
	.pages {
		position: absolute;
		bottom: 0.375rem;
		right: 0.375rem;
		padding: 0.05rem 0.35rem;
		border-radius: var(--radius-sm);
		background: rgba(0, 0, 0, 0.66);
		color: #fff;
		font-size: 0.62rem;
		font-variant-numeric: tabular-nums;
	}
	.cardacts {
		display: grid;
		gap: var(--space-1);
		margin-top: var(--space-1);
	}
	.cardacts :global(.btn) {
		width: 100%;
	}
</style>
