<script>
	let { title, subtitle = '', collapsible = true, storageKey = '', children } = $props();

	function initialCollapsed() {
		if (!storageKey || typeof localStorage === 'undefined') return false;
		return localStorage.getItem(storageKey) === '1';
	}
	let collapsed = $state(initialCollapsed());

	function toggle() {
		collapsed = !collapsed;
		if (storageKey && typeof localStorage !== 'undefined') {
			localStorage.setItem(storageKey, collapsed ? '1' : '0');
		}
	}

	let track = $state(null);
	let dragging = $state(false);
	let startX = 0;
	let startScroll = 0;
	let moved = false;

	function onPointerDown(e) {
		if (e.pointerType !== 'mouse' || e.button !== 0) return;
		dragging = true;
		moved = false;
		startX = e.clientX;
		startScroll = track.scrollLeft;
	}
	function onPointerMove(e) {
		if (!dragging) return;
		const dx = e.clientX - startX;
		if (Math.abs(dx) > 4) moved = true;
		track.scrollLeft = startScroll - dx;
	}
	function endDrag() {
		dragging = false;
	}
	function onClickCapture(e) {
		if (moved) {
			e.stopPropagation();
			e.preventDefault();
			moved = false;
		}
	}
</script>

<section class="shelf">
	{#if collapsible}
		<button class="head" onclick={toggle} aria-expanded={!collapsed}>
			<svg class="tri" class:open={!collapsed} viewBox="0 0 16 16" aria-hidden="true">
				<path
					d="M6 4l5 4-5 4"
					fill="none"
					stroke="currentColor"
					stroke-width="2"
					stroke-linecap="round"
					stroke-linejoin="round"
				/>
			</svg>
			<span>{title}</span>
		</button>
	{:else}
		<div class="head static">
			<span class="label">{title}</span>
			{#if subtitle}<span class="sub">{subtitle}</span>{/if}
		</div>
	{/if}

	{#if !collapsible || !collapsed}
		<!-- svelte-ignore a11y_no_static_element_interactions -->
		<div
			class="track"
			class:dragging
			bind:this={track}
			onpointerdown={onPointerDown}
			onpointermove={onPointerMove}
			onpointerup={endDrag}
			onpointercancel={endDrag}
			onpointerleave={endDrag}
			onclickcapture={onClickCapture}
		>
			{@render children?.()}
		</div>
	{/if}
</section>

<style>
	.shelf {
		margin-bottom: var(--space-5);
	}
	.head {
		all: unset;
		display: inline-flex;
		align-items: center;
		gap: var(--space-2);
		cursor: pointer;
		margin-bottom: var(--space-3);
		font-weight: 600;
		font-size: 1.05rem;
		color: var(--text);
	}
	.head:not(.static):hover {
		color: var(--accent);
	}
	.head.static {
		display: flex;
		align-items: baseline;
		gap: var(--space-3);
		cursor: default;
	}
	.label {
		font-size: 0.78rem;
		font-weight: 700;
		text-transform: uppercase;
		letter-spacing: 0.12em;
		color: var(--accent);
	}
	.sub {
		font-size: 0.8rem;
		color: var(--muted);
	}
	.tri {
		width: 0.85rem;
		height: 0.85rem;
		color: var(--muted);
		transition: transform var(--ease);
	}
	.tri.open {
		transform: rotate(90deg);
	}
	.track {
		display: flex;
		gap: var(--space-4);
		overflow-x: auto;
		overflow-y: hidden;
		padding-bottom: var(--space-2);
		cursor: grab;
		scrollbar-width: none;
	}
	.track::-webkit-scrollbar {
		display: none;
	}
	.track.dragging {
		cursor: grabbing;
		user-select: none;
	}
</style>
