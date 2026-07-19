<script>
	let { label, children } = $props();

	let open = $state(false);
	$effect(() => {
		if (!open) return;
		const close = () => (open = false);
		document.addEventListener('click', close);
		return () => document.removeEventListener('click', close);
	});
	function close() {
		open = false;
	}
</script>

<button
	class="cog"
	class:open
	type="button"
	aria-label={label}
	aria-haspopup="menu"
	aria-expanded={open}
	onclick={(e) => {
		e.stopPropagation();
		open = !open;
	}}
>
	<svg
		viewBox="0 0 24 24"
		fill="none"
		stroke="currentColor"
		stroke-width="1.8"
		stroke-linecap="round"
		stroke-linejoin="round"
		><circle cx="12" cy="12" r="3" /><path
			d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 1 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 1 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 1 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"
		/></svg
	>
</button>
{#if open}
	<div class="cogmenu" role="menu">
		{@render children(close)}
	</div>
{/if}

<style>
	.cog {
		all: unset;
		cursor: pointer;
		width: 32px;
		height: 32px;
		display: grid;
		place-items: center;
		border-radius: var(--radius-sm);
		color: var(--muted);
		transition:
			color var(--ease),
			background var(--ease);
		margin-block: -6px;
	}
	.cog:hover,
	.cog.open {
		color: var(--text);
		background: var(--surface-2);
	}
	.cog svg {
		width: 1.15rem;
		height: 1.15rem;
	}
	.cogmenu {
		position: absolute;
		top: calc(100% + 8px);
		right: 0;
		min-width: 200px;
		background: var(--surface);
		border: 1px solid var(--border);
		border-radius: var(--radius);
		box-shadow: var(--shadow-lg);
		padding: 6px;
		z-index: calc(var(--z-header) + 1);
	}
	.cogmenu :global(.mitem) {
		all: unset;
		display: flex;
		align-items: center;
		gap: 10px;
		width: 100%;
		box-sizing: border-box;
		padding: 8px 10px;
		border-radius: var(--radius-sm);
		font-size: 0.88rem;
		cursor: pointer;
	}
	.cogmenu :global(.mitem:hover) {
		background: var(--surface-2);
	}
	.cogmenu :global(.mitem svg) {
		width: 1rem;
		height: 1rem;
		color: var(--muted);
		flex: 0 0 auto;
	}
	.cogmenu :global(.mitem-danger:hover) {
		background: rgba(224, 86, 111, 0.1);
		color: #e0566f;
	}
	.cogmenu :global(.mitem-danger:hover svg) {
		color: #e0566f;
	}
</style>
