<script>
	let { mode = 'paged', disabled = false, onchange } = $props();
</script>

<div class="rmode">
	<div class="rmode-seg" role="group" aria-label="Reading mode">
		<button
			type="button"
			class:active={mode !== 'vertical'}
			{disabled}
			onclick={() => onchange?.('paged')}
		>
			Paged
		</button>
		<button
			type="button"
			class:active={mode === 'vertical'}
			{disabled}
			onclick={() => onchange?.('vertical')}
		>
			Vertical
		</button>
	</div>
</div>

<style>
	.rmode {
		display: flex;
		flex-direction: column;
	}
	.rmode-seg {
		display: flex;
		border: 1px solid var(--border);
		border-radius: var(--radius, 6px);
		overflow: hidden;
	}
	.rmode-seg button {
		flex: 1 1 0;
		padding: calc(var(--space-2) + 1px) var(--space-2);
		background: var(--surface);
		border: none;
		border-right: 1px solid var(--border);
		border-radius: 0;
		color: var(--muted);
		font-size: 0.9rem;
		cursor: pointer;
	}
	.rmode-seg button:last-child {
		border-right: none;
	}
	.rmode-seg button:hover:not(.active) {
		color: var(--text);
	}
	.rmode-seg button.active {
		background: var(--accent);
		color: #fff;
		font-weight: 600;
	}
	.rmode-seg button:disabled {
		opacity: 0.7;
		cursor: default;
	}
</style>
