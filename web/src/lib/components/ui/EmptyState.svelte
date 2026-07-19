<script>
	let { title = '', message = '', icon, action, wide = false } = $props();
</script>

<div class="empty" class:wide>
	{#if icon}<div class="ico">{@render icon()}</div>{/if}
	{#if title}<h2>{title}</h2>{/if}
	{#if message}<p>{message}</p>{/if}
	{#if action}<div class="action">{@render action()}</div>{/if}
</div>

<style>
	.empty {
		display: flex;
		flex-direction: column;
		align-items: center;
		text-align: center;
		gap: var(--space-3);
		max-width: 34rem;
		margin: var(--space-8) auto;
		padding: var(--space-8) var(--space-6);
		border: 1px dashed var(--border);
		border-radius: var(--radius-lg);
		background: var(--surface);
	}
	.empty.wide {
		max-width: none;
		margin: 0;
	}
	.ico {
		color: var(--muted);
		opacity: 0.7;
	}
	.ico :global(svg) {
		width: 2.4rem;
		height: 2.4rem;
	}
	h2 {
		margin: 0;
		font-family: var(--font-display);
		font-weight: 700;
		font-size: 1.25rem;
	}
	p {
		margin: 0;
		color: var(--muted);
		line-height: 1.6;
		font-size: 0.92rem;
	}
	.action {
		margin-top: var(--space-2);
	}
</style>
