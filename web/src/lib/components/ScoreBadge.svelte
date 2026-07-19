<script>
	let { score } = $props();

	const fmt = (s) => (s != null ? (s * 10).toFixed(1) : '');
	function color(s) {
		const t = Math.max(0, Math.min(1, s ?? 0));
		return `hsl(${t * 130} 58% 42%)`;
	}
</script>

{#if score != null}
	<span class="score" style={`background:${color(score)}`}>{fmt(score)}</span>
{/if}

<style>
	.score {
		position: absolute;
		top: 0.375rem;
		right: 0.375rem;
		display: inline-flex;
		align-items: center;
		justify-content: center;
		height: 1.5rem;
		min-width: 1.5rem;
		padding: 0 0.375rem;
		border-radius: 9999px;
		color: #fff;
		font-size: 0.625rem;
		font-weight: 700;
		line-height: 1;
		box-shadow:
			0 0 0 1px rgba(0, 0, 0, 0.2),
			0 4px 6px -1px rgba(0, 0, 0, 0.4),
			0 2px 4px -2px rgba(0, 0, 0, 0.4);
	}
</style>
