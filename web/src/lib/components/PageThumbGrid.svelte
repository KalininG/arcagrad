<script>
	import { gridColumns } from '$lib/grid.js';
	import CoverThumbnail from './CoverThumbnail.svelte';

	let { pages = [], onopen, oncols } = $props();
</script>

<div class="pagegrid fluid-grid" use:gridColumns={(n) => oncols?.(n)}>
	{#each pages as p (p.key)}
		<button class="pagecard" type="button" title={`Page ${p.label}`} onclick={() => onopen?.(p)}>
			<CoverThumbnail src={p.src} alt={`Page ${p.label}`}>
				<span class="pagenum">{p.label}</span>
			</CoverThumbnail>
		</button>
	{/each}
</div>

<style>
	.pagegrid {
		--min-cols: 3;
		--max-cols: 12;
		--col-target: 160px;
		--grid-gap: var(--space-2);
	}
	.pagecard {
		all: unset;
		cursor: pointer;
		display: block;
	}
	.pagecard :global(.cover) {
		transition:
			transform var(--ease),
			border-color var(--ease);
	}
	.pagecard:hover :global(.cover) {
		transform: translateY(-2px);
		border-color: var(--accent);
	}
	.pagenum {
		position: absolute;
		left: 0;
		right: 0;
		bottom: 0;
		padding: 0.1rem 0;
		text-align: center;
		background: rgba(0, 0, 0, 0.75);
		font-size: 0.7rem;
		color: #fff;
		font-variant-numeric: tabular-nums;
	}
</style>
