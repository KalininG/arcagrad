<script>
	import { relativeTime } from '$lib/format.js';

	let {
		updatedLabel = 'Updated',
		updatedAt = null,
		nextLabel = 'Next update',
		nextAt = null,
		actionLabel = 'Refresh now',
		busyLabel = 'Queuing…',
		busy = false,
		showAction = true,
		onaction,
	} = $props();

	const ago = (value) =>
		value ? relativeTime(typeof value === 'number' ? value : new Date(value)) : 'not yet';

	function until(value) {
		if (!value) return 'not scheduled';
		const time = typeof value === 'number' ? value * 1000 : new Date(value).getTime();
		const hours = Math.max(0, Math.round((time - Date.now()) / 3_600_000));
		if (hours < 1) return 'soon';
		return `in ${hours} ${hours === 1 ? 'hour' : 'hours'}`;
	}
</script>

<footer class="schedule-footer">
	<div>
		<span>{updatedLabel} {ago(updatedAt)}</span>
		<span>{nextLabel} {until(nextAt)}</span>
	</div>
	{#if showAction}
		<button onclick={onaction} disabled={busy} type="button">
			<svg viewBox="0 0 24 24"
				><path
					d="M20 6v5h-5M4 18v-5h5M6.1 9a7 7 0 0 1 11.2-2.4L20 11M4 13l2.7 4.4A7 7 0 0 0 17.9 15"
				/></svg
			>
			{busy ? busyLabel : actionLabel}
		</button>
	{/if}
</footer>

<style>
	.schedule-footer {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--space-4);
		margin: var(--space-6) calc(-1 * var(--space-6)) 0;
		padding: var(--space-4) var(--space-6);
		border-top: 1px solid var(--border);
		color: var(--muted);
		font-size: 0.7rem;
	}
	.schedule-footer div {
		display: flex;
		gap: var(--space-5);
	}
	.schedule-footer button {
		display: inline-flex;
		align-items: center;
		gap: 0.45rem;
		font-size: 0.72rem;
	}
	.schedule-footer svg {
		width: 1.05rem;
		height: 1.05rem;
		fill: none;
		stroke: currentColor;
		stroke-width: 1.8;
		stroke-linecap: round;
		stroke-linejoin: round;
	}
	@media (max-width: 900px) {
		.schedule-footer {
			margin-inline: calc(-1 * var(--space-5));
			padding-inline: var(--space-5);
		}
	}
	@media (max-width: 640px) {
		.schedule-footer,
		.schedule-footer div {
			align-items: flex-start;
			flex-direction: column;
		}
		.schedule-footer button {
			width: 100%;
			justify-content: center;
		}
	}
</style>
