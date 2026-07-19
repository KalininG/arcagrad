<script>
	let { value = null, onset, onclear, readonly = false, label = true, busy = false } = $props();

	let hover = $state(0);
	const shown = $derived(hover || value || 0);

	function pick(n) {
		if (readonly || busy) return;
		if (n === value) onclear?.();
		else onset?.(n);
	}
	function fillOf(s) {
		const halves = shown - (s - 1) * 2;
		return halves >= 2 ? 2 : halves === 1 ? 1 : 0;
	}
	const STAR =
		'M12 2.2l2.94 5.96 6.58.96-4.76 4.64 1.12 6.55L12 17.77 6.12 20.86l1.12-6.55L2.48 9.12l6.58-.96z';
	const stars = [1, 2, 3, 4, 5];
</script>

<div class="rating" class:readonly>
	{#if label}<span class="rlabel">Rating</span>{/if}
	<div class="stars" role="radiogroup" aria-label="Rating" onmouseleave={() => (hover = 0)}>
		{#each stars as s (s)}
			{@const fill = fillOf(s)}
			<span class="star">
				<svg class="glyph base" viewBox="0 0 24 24" aria-hidden="true"><path d={STAR} /></svg>
				<svg
					class="glyph fill"
					viewBox="0 0 24 24"
					aria-hidden="true"
					style={`clip-path: inset(0 ${100 - fill * 50}% 0 0)`}
				>
					<path d={STAR} />
				</svg>
				{#if !readonly}
					<button
						type="button"
						class="zone left"
						disabled={busy}
						aria-label={`Rate ${(2 * s - 1) / 2} stars`}
						title={`${(2 * s - 1) / 2}`}
						onmouseenter={() => (hover = 2 * s - 1)}
						onclick={() => pick(2 * s - 1)}
					></button>
					<button
						type="button"
						class="zone right"
						disabled={busy}
						aria-label={`Rate ${s} star${s > 1 ? 's' : ''}`}
						title={`${s}`}
						onmouseenter={() => (hover = 2 * s)}
						onclick={() => pick(2 * s)}
					></button>
				{/if}
			</span>
		{/each}
	</div>
</div>

<style>
	.rating {
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: var(--space-2);
	}
	.rlabel {
		font-size: 0.7rem;
		text-transform: uppercase;
		letter-spacing: 0.15em;
		color: var(--muted);
	}
	.stars {
		display: inline-flex;
		gap: 0.25rem;
	}
	.star {
		position: relative;
		display: inline-flex;
		width: 1.5rem;
		height: 1.5rem;
	}
	.glyph {
		width: 1.5rem;
		height: 1.5rem;
		fill: currentColor;
	}
	.glyph.base {
		color: color-mix(in srgb, var(--muted) 45%, transparent);
	}
	.glyph.fill {
		position: absolute;
		inset: 0;
		color: #e8b923;
	}
	.zone {
		all: unset;
		position: absolute;
		top: 0;
		height: 100%;
		width: 50%;
		cursor: pointer;
		z-index: 1;
	}
	.zone.left {
		left: 0;
	}
	.zone.right {
		right: 0;
	}
	.rating:not(.readonly) .star:hover {
		transform: scale(1.12);
		transition: transform var(--ease);
	}
	.zone:disabled {
		cursor: default;
	}
	.rating.readonly .star {
		cursor: default;
	}
</style>
