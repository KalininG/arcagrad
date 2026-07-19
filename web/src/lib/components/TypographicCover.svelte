<script>
	let { title = '', author = '' } = $props();

	const hue = $derived.by(() => {
		let h = 0;
		for (const ch of title) h = (h * 31 + ch.charCodeAt(0)) >>> 0;
		return h % 360;
	});
</script>

<div class="tcover" style={`--h:${hue}`} role="img" aria-label={title}>
	<span class="ttitle">{title}</span>
	<span class="trule" aria-hidden="true"></span>
	{#if author}<span class="tauthor">{author}</span>{/if}
</div>

<style>
	.tcover {
		position: absolute;
		inset: 0;
		container-type: inline-size;
		display: flex;
		flex-direction: column;
		align-items: center;
		text-align: center;
		padding: 16% 12% 12%;
		background:
			radial-gradient(120% 90% at 50% 0%, hsl(var(--h) 34% 30% / 0.85), transparent 62%),
			linear-gradient(180deg, hsl(var(--h) 28% 20%), hsl(var(--h) 30% 11%) 82%);
	}
	.tcover::before {
		content: '';
		position: absolute;
		inset: 7px;
		border: 1px solid hsl(var(--h) 30% 72% / 0.28);
		border-radius: 4px;
		pointer-events: none;
	}
	.ttitle {
		font-family: var(--font-display);
		font-weight: 700;
		font-size: clamp(0.72rem, 9cqw, 1.05rem);
		line-height: 1.22;
		color: hsl(var(--h) 38% 88%);
		text-wrap: balance;
		display: -webkit-box;
		-webkit-line-clamp: 5;
		-webkit-box-orient: vertical;
		overflow: hidden;
		margin-top: 6%;
	}
	.trule {
		width: 26px;
		height: 1px;
		background: hsl(var(--h) 35% 70% / 0.55);
		margin-top: 10%;
		flex: 0 0 auto;
	}
	.tauthor {
		margin-top: auto;
		font-size: 0.6rem;
		letter-spacing: 0.14em;
		text-transform: uppercase;
		color: hsl(var(--h) 24% 68%);
		display: -webkit-box;
		-webkit-line-clamp: 2;
		-webkit-box-orient: vertical;
		overflow: hidden;
	}
</style>
