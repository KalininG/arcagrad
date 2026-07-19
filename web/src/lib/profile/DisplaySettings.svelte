<script>
	import { theme, applyTheme } from '$lib/theme.js';
	import { accent, applyAccent, resetAccent, ACCENTS, DEFAULT_ACCENT } from '$lib/accent.js';
</script>

<div class="display-grid">
	<section class="group">
		<div class="row">
			<div class="meta">
				<h2>Theme</h2>
				<p class="desc">Switch between the dark and light palette.</p>
			</div>
			<div class="seg" role="group" aria-label="Theme">
				<button
					class="segbtn"
					class:on={$theme === 'light'}
					onclick={() => applyTheme('light')}
					type="button">Light</button
				>
				<button
					class="segbtn"
					class:on={$theme === 'dark'}
					onclick={() => applyTheme('dark')}
					type="button">Dark</button
				>
			</div>
		</div>
	</section>

	<section class="group block">
		<div class="blockhead">
			<div class="meta">
				<h2>Accent color</h2>
				<p class="desc">Used for highlights, links, and the active state throughout the app.</p>
			</div>
			{#if $accent !== DEFAULT_ACCENT}
				<button class="reset" type="button" onclick={resetAccent}>Reset to default</button>
			{/if}
		</div>
		<div class="swatches">
			{#each ACCENTS as a (a.id)}
				<button
					class="swatch"
					class:on={$accent === a.value}
					type="button"
					onclick={() => applyAccent(a.value)}
					aria-pressed={$accent === a.value}
					title={a.label}
				>
					<span class="chip" style={`background:${a.value}`}></span>
					<span class="name">{a.label}</span>
				</button>
			{/each}
		</div>
	</section>
</div>

<style>
	.display-grid {
		display: grid;
		gap: var(--space-3);
	}
	.group {
		border: 1px solid var(--border);
		border-radius: var(--radius);
		background: var(--surface);
		padding: 0 var(--space-5);
	}
	.row {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--space-5);
		padding: var(--space-4) 0;
		flex-wrap: wrap;
	}
	.meta h2 {
		margin: 0 0 0.2rem;
		font-size: 1rem;
		font-weight: 600;
	}
	.desc {
		margin: 0;
		font-size: 0.82rem;
		color: var(--muted);
	}
	.seg {
		display: inline-flex;
		flex: 0 0 auto;
		border: 1px solid var(--border);
		border-radius: var(--radius-sm);
		overflow: hidden;
	}
	.segbtn {
		all: unset;
		cursor: pointer;
		padding: var(--space-2) var(--space-4);
		font-size: 0.88rem;
		color: var(--muted);
		transition:
			background var(--ease),
			color var(--ease);
	}
	.segbtn + .segbtn {
		border-left: 1px solid var(--border);
	}
	.segbtn:not(.on):hover {
		background: var(--surface-2);
		color: var(--text);
	}
	.segbtn.on {
		background: var(--accent);
		color: #fff;
	}
	.block {
		padding: var(--space-5);
	}
	.blockhead {
		display: flex;
		align-items: flex-start;
		justify-content: space-between;
		gap: var(--space-4);
		margin-bottom: var(--space-5);
	}
	.reset {
		all: unset;
		flex: 0 0 auto;
		cursor: pointer;
		color: var(--muted);
		font-size: 0.85rem;
	}
	.reset:hover {
		color: var(--accent);
	}
	.swatches {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(108px, 1fr));
		gap: var(--space-3);
	}
	.swatch {
		all: unset;
		cursor: pointer;
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: var(--space-2);
	}
	.swatch .chip {
		width: 100%;
		height: 2.6rem;
		border-radius: var(--radius);
		border: 1px solid rgba(255, 255, 255, 0.1);
		transition:
			transform var(--ease),
			box-shadow var(--ease);
	}
	.swatch:hover .chip {
		transform: translateY(-1px);
	}
	.swatch.on .chip {
		box-shadow:
			0 0 0 2px var(--surface),
			0 0 0 4px var(--text);
	}
	.swatch .name {
		font-size: 0.78rem;
		color: var(--muted);
	}
	.swatch.on .name {
		color: var(--text);
	}
</style>
