<script>
	import Dropdown from '$lib/components/ui/Dropdown.svelte';

	let {
		field = $bindable('added_at'),
		order = $bindable('desc'),
		fields = [
			{ value: 'added_at', label: 'Date added', short: 'Date' },
			{ value: 'title', label: 'Title' },
			{ value: 'creator', label: 'Creator' },
			{ value: 'rating', label: 'Rating' },
			{ value: 'page_count', label: 'Length' },
		],
	} = $props();

	function toggleOrder() {
		order = order === 'asc' ? 'desc' : 'asc';
	}
</script>

<div class="sort">
	<span class="label">Sort</span>

	<div class="picker">
		<Dropdown bind:value={field} options={fields} />
	</div>

	{#if field !== 'relevance'}
		<button
			class="order"
			onclick={toggleOrder}
			title={order === 'asc' ? 'Ascending' : 'Descending'}
		>
			<svg
				viewBox="0 0 24 24"
				fill="none"
				stroke="currentColor"
				stroke-width="2"
				stroke-linecap="round"
				stroke-linejoin="round"
			>
				{#if order === 'asc'}
					<path d="M12 19V5M5 12l7-7 7 7" />
				{:else}
					<path d="M12 5v14M5 12l7 7 7-7" />
				{/if}
			</svg>
			<span>{order === 'asc' ? 'Ascending' : 'Descending'}</span>
		</button>
	{/if}
</div>

<style>
	.sort {
		display: flex;
		align-items: center;
		gap: var(--space-3);
		flex-wrap: wrap;
		row-gap: var(--space-2);
		min-width: 0;
	}
	.label {
		font-size: 0.7rem;
		text-transform: uppercase;
		letter-spacing: 0.15em;
		color: var(--muted);
	}
	.picker {
		flex: 0 0 auto;
		min-width: 7rem;
	}
	@media (max-width: 400px) {
		.picker {
			min-width: 0;
		}
	}
	.order {
		display: inline-flex;
		align-items: center;
		gap: var(--space-2);
		border-radius: var(--radius-sm);
		color: var(--muted);
		font-size: 0.85rem;
	}
	.order:hover {
		color: var(--text);
	}
	.order svg {
		width: 0.85rem;
		height: 0.85rem;
	}
	@media (max-width: 640px) {
		.sort {
			gap: var(--space-2);
		}
		.label {
			display: none;
		}
		.order span {
			display: none;
		}
		.order {
			padding-inline: var(--space-2);
		}
	}
</style>
