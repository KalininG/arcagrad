<script>
	let {
		onClose,
		title = '',
		subtitle = '',
		label = undefined,
		width = 'min(600px, 100%)',
		busy = false,
		closeOnEscape = true,
		header,
		children,
	} = $props();

	function onKey(e) {
		if (e.key === 'Escape' && !busy && closeOnEscape) onClose?.();
	}
</script>

<svelte:window onkeydown={onKey} />

<!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
<div class="overlay" onclick={() => !busy && onClose?.()}>
	<div
		class="modal"
		role="dialog"
		aria-modal="true"
		aria-label={label ?? title}
		style="--modal-w:{width}"
		onclick={(e) => e.stopPropagation()}
	>
		<header>
			{#if header}
				{@render header()}
			{:else}
				<div class="htext">
					{#if title}<h2>{title}</h2>{/if}
					{#if subtitle}<p class="subtitle">{subtitle}</p>{/if}
				</div>
			{/if}
			<button class="close" type="button" aria-label="Close" onclick={() => onClose?.()}>×</button>
		</header>
		{@render children?.()}
	</div>
</div>

<style>
	.overlay {
		position: fixed;
		inset: 0;
		z-index: 100;
		display: flex;
		align-items: center;
		justify-content: center;
		padding: var(--space-4);
		background: rgba(0, 0, 0, 0.55);
	}
	.modal {
		width: var(--modal-w);
		max-height: 90vh;
		overflow-y: auto;
		display: flex;
		flex-direction: column;
		background: var(--surface);
		border: 1px solid var(--border);
		border-radius: var(--radius);
		box-shadow: var(--shadow-lg);
	}
	header {
		display: flex;
		align-items: flex-start;
		justify-content: space-between;
		gap: var(--space-4);
		padding: var(--space-4) var(--space-5);
		border-bottom: 1px solid var(--border);
	}
	.htext {
		min-width: 0;
	}
	h2 {
		margin: 0;
		font-family: var(--font-display);
		font-size: 1.15rem;
	}
	.subtitle {
		margin: 0.25rem 0 0;
		color: var(--muted);
		font-size: 0.8rem;
	}
	.close {
		all: unset;
		cursor: pointer;
		color: var(--muted);
		font-size: 1.25rem;
		line-height: 1;
		padding: 0 var(--space-2);
	}
	.close:hover {
		color: var(--text);
	}
</style>
