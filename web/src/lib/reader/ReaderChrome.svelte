<script>
	let {
		title = '',
		progressPct = 0,
		showProgress = true,
		seriesCtx = null,
		onback,
		onprev,
		onnext,
		counter,
		leadActions,
		children,
		headerOverlay = false,
		headerRevealed = false,
		headerHidden = false,
		progressOverlay = false,
		progressHidden = false,
	} = $props();
</script>

<header class:overlay={headerOverlay} class:revealed={headerRevealed} class:hidden={headerHidden}>
	<div class="side">
		<button class="back" onclick={onback}>← Back</button>
		{@render leadActions?.()}
		{#if seriesCtx?.prev_leaf_id != null}
			<button class="volnav" title="Previous volume" aria-label="Previous volume" onclick={onprev}>
				<svg viewBox="0 0 24 24" fill="currentColor"><path d="M6 5h2v14H6zM20 5v14L9 12z" /></svg>
			</button>
		{/if}
	</div>
	<span class="title">{title}</span>
	<div class="side right">
		{#if seriesCtx?.number_disp}<span class="vol">{seriesCtx.number_disp}</span>{/if}
		<span class="counter">{@render counter?.()}</span>
		{#if seriesCtx?.next_leaf_id != null}
			<button class="volnav" title="Next volume" aria-label="Next volume" onclick={onnext}>
				<svg viewBox="0 0 24 24" fill="currentColor"><path d="M16 5h2v14h-2zM4 5l11 7-11 7z" /></svg
				>
			</button>
		{/if}
	</div>
</header>

{@render children?.()}

{#if showProgress}
	<div class="progress" class:overlay={progressOverlay} class:hidden={progressHidden}>
		<div class="bar" style={`width:${progressPct}%`}></div>
	</div>
{/if}

<style>
	header {
		display: flex;
		align-items: center;
		gap: 0.75rem;
		padding: calc(var(--space-3) + env(safe-area-inset-top, 0px)) var(--space-5) var(--space-3);
		background: color-mix(in srgb, var(--surface) 92%, transparent);
		backdrop-filter: blur(8px);
		border-bottom: 1px solid var(--border);
		z-index: 20;
	}
	header.overlay {
		position: absolute;
		top: 0;
		left: 0;
		right: 0;
		transform: translateY(-110%);
		transition: transform 0.25s ease;
	}
	header.overlay.revealed {
		transform: translateY(0);
	}
	header.hidden {
		display: none;
	}
	.back {
		background: transparent;
		border: none;
		padding: 0;
		color: var(--muted);
		font-size: 0.9rem;
		white-space: nowrap;
		cursor: pointer;
	}
	.back:hover {
		color: var(--text);
	}
	.side {
		flex: 1 1 0;
		flex-shrink: 0;
		display: flex;
		align-items: center;
		min-width: max-content;
		gap: 0.6rem;
	}
	.side.right {
		justify-content: flex-end;
	}
	.title {
		flex: 0 1 auto;
		min-width: 0;
		text-align: center;
		font-size: 0.9rem;
		color: var(--text);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}
	.counter {
		font-variant-numeric: tabular-nums;
		font-size: 0.8rem;
		color: var(--muted);
	}
	.vol {
		font-size: 0.78rem;
		color: var(--text);
		white-space: nowrap;
	}
	.volnav {
		flex: 0 0 auto;
		display: inline-flex;
		align-items: center;
		justify-content: center;
		padding: 0;
		background: transparent;
		border: none;
		color: var(--muted);
		cursor: pointer;
	}
	.volnav:hover {
		color: var(--text);
	}
	.volnav svg {
		width: 1.05rem;
		height: 1.05rem;
	}

	.progress {
		height: 4px;
		background: var(--surface-2);
		z-index: 20;
	}
	.progress.overlay {
		position: absolute;
		bottom: 0;
		left: 0;
		right: 0;
		transform: translateY(110%);
	}
	.progress.hidden {
		display: none;
	}
	.bar {
		height: 100%;
		background: var(--accent);
		transition: width 0.2s ease;
	}
</style>
