<script>
	let { label = 'Loading…', compact = false } = $props();
</script>

<div class="loading" class:compact role="status" aria-live="polite">
	<span class="ring" aria-hidden="true"></span>
	{#if label}<span class="lbl">{label}</span>{/if}
</div>

<style>
	.loading {
		display: flex;
		flex-direction: column;
		align-items: center;
		justify-content: center;
		gap: var(--space-3);
		min-height: 180px;
		padding: var(--space-5);
		color: var(--muted);
	}
	.loading.compact {
		flex-direction: row;
		gap: var(--space-2);
		min-height: 0;
		padding: var(--space-4) 0;
	}
	.ring {
		width: 1.75rem;
		height: 1.75rem;
		flex: 0 0 auto;
		border: 2.5px solid color-mix(in srgb, var(--accent) 18%, transparent);
		border-top-color: var(--accent);
		border-radius: 50%;
		animation: spin 0.8s linear infinite;
	}
	.compact .ring {
		width: 1rem;
		height: 1rem;
		border-width: 2px;
	}
	.lbl {
		font-size: 0.85rem;
	}
	@media (prefers-reduced-motion: reduce) {
		.ring {
			animation-duration: 2s;
		}
	}
</style>
