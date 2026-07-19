<script>
	import { onMount, onDestroy } from 'svelte';

	let {
		title = '',
		count = 1,
		cooldown = 3,
		busy = false,
		heading: headingOverride = '',
		message = '',
		verb = 'Delete',
		busyLabel = '',
		onConfirm,
		onClose,
	} = $props();

	let remaining = $state(cooldown);
	let timer;
	const ready = $derived(remaining <= 0);
	const noun = $derived(count === 1 ? 'archive' : 'archives');
	const heading = $derived(
		headingOverride || (title ? 'Delete this archive?' : `Delete ${count} ${noun}?`),
	);
	const doing = $derived(busyLabel || `${verb.replace(/e$/, '')}ing…`);

	function onKey(e) {
		if (e.key === 'Escape' && !busy) onClose?.();
	}

	onMount(() => {
		timer = setInterval(() => {
			remaining -= 1;
			if (remaining <= 0) {
				remaining = 0;
				clearInterval(timer);
			}
		}, 1000);
		window.addEventListener('keydown', onKey);
	});
	onDestroy(() => {
		clearInterval(timer);
		window.removeEventListener('keydown', onKey);
	});
</script>

<div class="overlay" onclick={() => !busy && onClose?.()}>
	<div
		class="modal"
		role="alertdialog"
		aria-modal="true"
		aria-label={heading}
		onclick={(e) => e.stopPropagation()}
	>
		<div class="head">
			<span class="warn" aria-hidden="true">
				<svg
					viewBox="0 0 24 24"
					fill="none"
					stroke="currentColor"
					stroke-width="2"
					stroke-linecap="round"
					stroke-linejoin="round"
				>
					<path
						d="M10.29 3.86 1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z"
					/>
					<path d="M12 9v4" />
					<path d="M12 17h.01" />
				</svg>
			</span>
			<h2>{heading}</h2>
		</div>

		<p class="body">
			{#if message}
				{message}
			{:else if title}
				“{title}” will be <strong>permanently removed</strong> from the server. This cannot be undone.
			{:else}
				<strong>{count} {noun}</strong> will be <strong>permanently removed</strong> from the server.
				This cannot be undone.
			{/if}
		</p>

		<div class="foot">
			<button class="ghost" type="button" onclick={() => onClose?.()} disabled={busy}>Cancel</button
			>
			<button class="danger" type="button" onclick={() => onConfirm?.()} disabled={!ready || busy}>
				{busy ? doing : ready ? verb : `${verb} (${remaining})`}
			</button>
		</div>
	</div>
</div>

<style>
	.overlay {
		position: fixed;
		inset: 0;
		z-index: 100;
		display: grid;
		place-items: center;
		padding: var(--space-5);
		background: rgba(0, 0, 0, 0.55);
		backdrop-filter: blur(2px);
	}
	.modal {
		width: 100%;
		max-width: 30rem;
		background: var(--surface);
		border: 1px solid var(--border);
		border-radius: var(--radius-lg);
		box-shadow: var(--shadow-lg);
		padding: var(--space-5);
	}
	.head {
		display: flex;
		align-items: center;
		gap: var(--space-3);
		margin-bottom: var(--space-3);
	}
	.warn {
		flex: 0 0 auto;
		display: inline-flex;
		align-items: center;
		justify-content: center;
		width: 2.2rem;
		height: 2.2rem;
		border-radius: 50%;
		color: #e0566f;
		background: rgba(224, 86, 111, 0.14);
		border: 1px solid rgba(224, 86, 111, 0.4);
	}
	.warn svg {
		width: 1.2rem;
		height: 1.2rem;
	}
	h2 {
		margin: 0;
		font-family: var(--font-display);
		font-weight: 700;
		font-size: 1.25rem;
	}
	.body {
		margin: 0 0 var(--space-5);
		color: var(--muted);
		line-height: 1.55;
		font-size: 0.92rem;
	}
	.body strong {
		color: #e0566f;
		font-weight: 600;
	}
	.foot {
		display: flex;
		justify-content: flex-end;
		gap: var(--space-3);
	}
	.ghost,
	.danger {
		all: unset;
		cursor: pointer;
		padding: 0.5rem 1rem;
		border-radius: var(--radius-sm);
		font-size: 0.9rem;
		font-weight: 600;
		text-align: center;
		transition:
			background var(--ease),
			border-color var(--ease),
			color var(--ease),
			opacity var(--ease);
	}
	.ghost {
		border: 1px solid var(--border);
		color: var(--text);
	}
	.ghost:hover {
		background: var(--surface-2);
	}
	.danger {
		min-width: 6.5rem;
		background: #d24a5f;
		color: #fff;
	}
	.danger:hover:not(:disabled) {
		background: #e0566f;
	}
	.danger:disabled {
		cursor: not-allowed;
		opacity: 0.55;
	}
	.ghost:disabled {
		cursor: not-allowed;
		opacity: 0.55;
	}
</style>
