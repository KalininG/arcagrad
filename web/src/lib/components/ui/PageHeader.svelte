<script>
	let { title = '', wordmark = 'Arcagrad', actions } = $props();
</script>

<header class="pagehead">
	<div>
		<p class="wordmark">{wordmark}</p>
		<h1>{title}</h1>
	</div>
	{#if actions}
		<div class="actions">{@render actions()}</div>
	{/if}
</header>

<style>
	.pagehead {
		margin-bottom: var(--space-6);
		display: flex;
		align-items: flex-start;
		justify-content: space-between;
		gap: var(--space-4);
		flex-wrap: wrap;
	}
	.wordmark {
		margin: 0 0 0.15rem;
		font-size: 0.7rem;
		text-transform: uppercase;
		letter-spacing: 0.3em;
		color: var(--muted);
	}
	h1 {
		margin: 0;
		font-family: var(--font-display);
		font-weight: 700;
		font-size: 1.75rem;
		line-height: 1;
	}
	.actions {
		display: flex;
		flex-direction: column;
		align-items: flex-end;
		gap: var(--space-2);
	}
</style>
