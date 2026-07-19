<script module>
	const cache = new Map();
	const CACHE_LIMIT = 128;
</script>

<script>
	import { paletteFromImage } from '$lib/artworkPalette.js';

	let { src = '' } = $props();
	let colors = $state([]);

	$effect(() => {
		const url = src;
		let live = true;
		colors = [];
		if (!url || typeof Image === 'undefined') return;

		const known = cache.get(url);
		const pending = known ? Promise.resolve(known) : paletteFromImage(url);
		pending
			.then((palette) => {
				if (!known && palette.length) {
					cache.set(url, palette);
					if (cache.size > CACHE_LIMIT) cache.delete(cache.keys().next().value);
				}
				if (live) colors = palette;
			})
			.catch(() => {});
		return () => (live = false);
	});

	const coverLayer = $derived(
		src ? `url("${src.replaceAll('\\', '\\\\').replaceAll('"', '\\"')}")` : 'none',
	);
	const paletteLayers = $derived(
		colors
			.map(
				(c, i) =>
					`radial-gradient(circle at ${c.x.toFixed(1)}% ${c.y.toFixed(1)}%, ` +
					`rgb(${c.r} ${c.g} ${c.b} / ${i === 0 ? 0.95 : 0.82}) 0%, ` +
					`rgb(${c.r} ${c.g} ${c.b} / 0.52) 24%, transparent 62%)`,
			)
			.join(', '),
	);
	const field = $derived(paletteLayers ? `${paletteLayers}, ${coverLayer}` : coverLayer);
	const paletteLuminance = $derived.by(() => {
		const population = colors.reduce((sum, c) => sum + c.population, 0);
		if (!population) return 1;
		return (
			colors.reduce(
				(sum, c) => sum + ((0.2126 * c.r + 0.7152 * c.g + 0.0722 * c.b) / 255) * c.population,
				0,
			) / population
		);
	});
	const darkBoost = $derived(Math.max(0, Math.min(1, (0.48 - paletteLuminance) / 0.32)));
	const fieldOpacity = $derived((0.58 + darkBoost * 0.26).toFixed(3));
	const fieldSaturation = $derived((0.9 + darkBoost * 0.25).toFixed(3));
</script>

<div class="backdrop" aria-hidden="true">
	<div
		class="field"
		style:background={field}
		style:--field-opacity={fieldOpacity}
		style:--field-saturation={fieldSaturation}
	></div>
	<div class="wash"></div>
</div>

<style>
	.backdrop {
		position: fixed;
		z-index: 0;
		inset: 0 0 auto;
		height: min(720px, 82vh);
		overflow: hidden;
		pointer-events: none;
		opacity: 1;
	}
	.field {
		position: absolute;
		inset: -18%;
		filter: blur(62px) saturate(var(--field-saturation));
		transform: scale(1.16);
		background-color: var(--bg);
		background-size: cover;
		background-position: center;
		opacity: var(--field-opacity);
		transition:
			opacity 220ms ease,
			filter 220ms ease;
	}
	.wash {
		position: absolute;
		inset: 0;
		background:
			linear-gradient(
				to bottom,
				color-mix(in srgb, var(--bg) 68%, transparent) 0%,
				color-mix(in srgb, var(--bg) 81%, transparent) 42%,
				var(--bg) 100%
			),
			linear-gradient(to right, color-mix(in srgb, var(--bg) 44%, transparent), transparent 45%);
	}

	@media (prefers-reduced-transparency: reduce), (prefers-contrast: more) {
		.backdrop {
			display: none;
		}
	}
</style>
