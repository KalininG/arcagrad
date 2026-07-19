<script>
	let {
		label,
		doneLabel = null,
		done = false,
		current = null,
		total = null,
		fraction = null,
		valueLabel = null,
	} = $props();
	const fill = $derived(Math.min(Math.max(fraction ?? (total ? current / total : 0), 0), 1));
	const displayValue = $derived(
		valueLabel ?? (current != null && total != null ? `${current} / ${total}` : ''),
	);
</script>

<div class="progress">
	<div class="prow" class:done>
		<span>{done && doneLabel ? doneLabel : label}</span>
		<span>{displayValue}</span>
	</div>
	<div class="track">
		<div class="fill" class:done style={`width:${fill * 100}%`}></div>
	</div>
</div>

<style>
	.progress {
		display: flex;
		flex-direction: column;
		gap: var(--space-2);
	}
	.prow {
		display: flex;
		justify-content: space-between;
		align-items: baseline;
		font-size: 0.74rem;
		font-weight: 600;
		color: var(--muted);
	}
	.prow.done {
		color: var(--good);
	}
	.track {
		height: 8px;
		border-radius: 9999px;
		overflow: hidden;
		background: var(--surface-2);
	}
	.fill {
		height: 100%;
		border-radius: 9999px;
		background: var(--accent);
		transition: width 0.3s ease;
	}
	.fill.done {
		background: var(--good);
	}
</style>
