<script>
	import { onMount, onDestroy, untrack } from 'svelte';
	import { get } from 'svelte/store';
	import { goto, replaceState } from '$app/navigation';
	import { page as pageStore } from '$app/stores';
	import { items as itemsApi, media } from '$lib/api.js';
	import { isGuest } from '$lib/session.js';
	import ReaderChrome from './ReaderChrome.svelte';

	let { id = null, detail = null, source = null } = $props();

	let title = $state('');
	let pageCount = $state(0);
	let current = $state(0);
	let seriesCtx = $state(null);
	let immersive = $state(false);
	let topHover = $state(false);
	const TOP_REVEAL = 64;

	const pageUrl = (n) => (source ? source.pageUrl(n) : media.page(id, n, detail?.version));
	const progressPct = $derived(pageCount ? ((current + 1) / pageCount) * 100 : 0);

	// Swap between two decoded layers to avoid WebKit blanking between pages.
	const PX = 'data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7';
	let s0 = $state(PX);
	let s1 = $state(PX);
	let p0 = $state(-1);
	let p1 = $state(-1);
	let shown = $state(0);
	let img0, img1;
	const imgAt = (i) => (i === 0 ? img0 : img1);
	const pageAt = (i) => (i === 0 ? p0 : p1);
	function setLayer(i, url, page) {
		if (i === 0) {
			s0 = url;
			p0 = page;
		} else {
			s1 = url;
			p1 = page;
		}
	}

	let showSpinner = $state(true);
	let spinnerTimer;
	let navToken = 0;

	function finishSwap(i, token) {
		if (token !== navToken) return;
		shown = i;
		clearTimeout(spinnerTimer);
		showSpinner = false;
		preloadAround(current);
	}
	function swapTo(n, token) {
		if (pageAt(shown) === n) {
			finishSwap(shown, token);
			return;
		}
		const hide = shown ^ 1;
		const url = pageUrl(n);
		if (pageAt(hide) !== n) setLayer(hide, url, n);
		const el = imgAt(hide);
		if (el && el.getAttribute('src') === url && el.complete && el.naturalWidth > 0) {
			finishSwap(hide, token);
			return;
		}
		clearTimeout(spinnerTimer);
		spinnerTimer = setTimeout(() => {
			if (token === navToken) showSpinner = true;
		}, 100);
	}

	const PRELOAD_RADIUS = 2;
	const preloadCache = new Map();
	let preloadToken = 0;

	async function preloadAround(c) {
		if (!pageCount) return;
		const keep = new Set();
		for (let d = -PRELOAD_RADIUS; d <= PRELOAD_RADIUS; d++) {
			const i = c + d;
			if (i >= 0 && i < pageCount) keep.add(pageUrl(i));
		}
		for (const url of preloadCache.keys()) if (!keep.has(url)) preloadCache.delete(url);

		const order = [];
		for (let d = 1; d <= PRELOAD_RADIUS; d++) {
			if (c + d < pageCount) order.push(c + d);
			if (c - d >= 0) order.push(c - d);
		}
		const token = ++preloadToken;
		for (const i of order) {
			if (token !== preloadToken) return;
			const url = pageUrl(i);
			if (preloadCache.has(url)) continue;
			const img = new Image();
			img.decoding = 'async';
			img.src = url;
			preloadCache.set(url, img);
			try {
				await img.decode();
			} catch {
				/* ignored */
			}
		}
	}

	function navigate(delta) {
		const dest = current + delta;
		if (dest >= pageCount) {
			if (seriesCtx?.next_leaf_id != null) goToLeaf(seriesCtx.next_leaf_id);
			else goBack();
			return;
		}
		if (dest < 0) {
			if (seriesCtx?.prev_leaf_id != null) goToLeaf(seriesCtx.prev_leaf_id);
			return;
		}

		const token = ++navToken;
		current = dest;
		reportProgress();
		swapTo(dest, token);
	}

	const next = () => navigate(1);
	const prev = () => navigate(-1);

	async function onLayerLoad(i) {
		const el = imgAt(i);
		try {
			await Promise.race([
				el?.decode?.() ?? Promise.resolve(),
				new Promise((r) => setTimeout(r, 250)),
			]);
		} catch {
			/* ignored */
		}
		if (pageAt(i) === current) finishSwap(i, navToken);
	}

	function onLayerError(i) {
		if (pageAt(i) === current) {
			clearTimeout(spinnerTimer);
			showSpinner = false;
		}
	}

	async function goBack() {
		await flushProgress();
		goto(source ? source.backHref : `/item/${id}`);
	}
	async function goToLeaf(leafId) {
		if (leafId == null) return;
		await flushProgress();
		goto(`/reader/${leafId}`);
	}

	let progressTimer;
	let pendingPage = null;
	function reportProgress() {
		pendingPage = current;
		clearTimeout(progressTimer);
		progressTimer = setTimeout(flushProgress, 600);
	}
	function flushProgress() {
		clearTimeout(progressTimer);
		if (source || pendingPage == null || get(isGuest)) return Promise.resolve();
		const p = pendingPage;
		pendingPage = null;
		return itemsApi.saveProgress(id, p, { keepalive: true }).catch(() => {});
	}

	const SWIPE_THRESHOLD = 50;
	const TAP_TOLERANCE = 10;
	let containerEl;
	let downX = 0;
	let downY = 0;
	let downTime = 0;
	let moved = false;

	function onPointerDown(e) {
		downX = e.clientX;
		downY = e.clientY;
		downTime = Date.now();
		moved = false;
	}
	function onPointerMove(e) {
		if (
			Math.abs(e.clientX - downX) > TAP_TOLERANCE ||
			Math.abs(e.clientY - downY) > TAP_TOLERANCE
		) {
			moved = true;
		}
		if (immersive) topHover = e.clientY < TOP_REVEAL;
	}
	function onPointerLeave() {
		topHover = false;
	}
	function onPointerUp(e) {
		if (e.target.closest('button, a')) return;
		const dx = e.clientX - downX;
		const dy = e.clientY - downY;
		if (Math.abs(dx) > SWIPE_THRESHOLD && Math.abs(dx) > Math.abs(dy)) {
			if (dx < 0) next();
			else prev();
			return;
		}
		if (!moved && Date.now() - downTime < 500) handleTap(e.clientX);
	}
	function handleTap(clientX) {
		const r = containerEl.getBoundingClientRect();
		const rel = (clientX - r.left) / r.width;
		if (rel < 0.33) prev();
		else if (rel > 0.67) next();
		else {
			immersive = !immersive;
			topHover = false;
		}
	}

	function onKey(e) {
		if (e.key === 'ArrowRight' || e.key === ' ') {
			e.preventDefault();
			next();
		} else if (e.key === 'ArrowLeft') {
			prev();
		} else if (e.key === 'Escape') {
			goBack();
		}
	}

	function resetBuffer() {
		shown = 0;
		p1 = -1;
		s1 = PX;
		p0 = current;
		s0 = pageUrl(current);
	}

	function initReader() {
		showSpinner = true;
		clearTimeout(spinnerTimer);
		navToken++;
		preloadToken++;
		preloadCache.clear();
		pendingPage = null;
		clearTimeout(progressTimer);

		if (source) {
			title = source.title ?? '';
			pageCount = source.pageCount ?? 0;
			seriesCtx = null;
			current = Math.max(0, Math.min(source.start ?? 0, Math.max(0, pageCount - 1)));
			resetBuffer();
			return;
		}
		if (!detail) return;
		title = detail.name;
		pageCount = detail.page_count ?? 0;
		seriesCtx = detail.series ?? null;
		const q = parseInt(new URLSearchParams(location.search).get('page') ?? '', 10);
		const resume = Number.isFinite(q) ? q - 1 : (detail.progress ?? 0);
		current = Math.max(0, Math.min(resume, Math.max(0, pageCount - 1)));
		resetBuffer();
		reportProgress();
	}

	$effect(() => {
		detail;
		source;
		untrack(() => initReader());
	});

	$effect(() => {
		const p = current;
		if (!pageCount) return;
		untrack(() => {
			const url = new URL(location.href);
			if (url.searchParams.get('page') === String(p + 1)) return;
			url.searchParams.set('page', String(p + 1));
			try {
				replaceState(url, get(pageStore).state ?? {});
			} catch {
				/* ignored */
			}
		});
	});

	onMount(() => {
		window.addEventListener('keydown', onKey);
		window.addEventListener('pagehide', flushProgress);
	});

	onDestroy(() => {
		window.removeEventListener('keydown', onKey);
		window.removeEventListener('pagehide', flushProgress);
		clearTimeout(spinnerTimer);
		flushProgress();
	});
</script>

<div
	class="reader"
	class:immersive
	bind:this={containerEl}
	onpointerdown={onPointerDown}
	onpointermove={onPointerMove}
	onpointerup={onPointerUp}
	onpointerleave={onPointerLeave}
>
	<ReaderChrome
		{title}
		{progressPct}
		showProgress={!!pageCount}
		{seriesCtx}
		onback={goBack}
		onprev={() => goToLeaf(seriesCtx?.prev_leaf_id)}
		onnext={() => goToLeaf(seriesCtx?.next_leaf_id)}
		headerOverlay={immersive}
		headerRevealed={topHover}
		progressOverlay={immersive}
	>
		{#snippet counter()}{pageCount ? current + 1 : 0} / {pageCount}{/snippet}

		{#snippet children()}
			<div class="stage">
				{#if showSpinner}
					<div class="center spinner-wrap"><div class="spinner"></div></div>
				{/if}
				<img
					class="layer"
					class:show={shown === 0}
					src={s0}
					alt={p0 >= 0 ? `Page ${p0 + 1}` : ''}
					decoding="async"
					draggable="false"
					bind:this={img0}
					onload={() => onLayerLoad(0)}
					onerror={() => onLayerError(0)}
				/>
				<img
					class="layer"
					class:show={shown === 1}
					src={s1}
					alt={p1 >= 0 ? `Page ${p1 + 1}` : ''}
					decoding="async"
					draggable="false"
					bind:this={img1}
					onload={() => onLayerLoad(1)}
					onerror={() => onLayerError(1)}
				/>
			</div>
		{/snippet}
	</ReaderChrome>
</div>

<style>
	.reader {
		position: fixed;
		inset: 0;
		background: #000;
		padding-bottom: env(safe-area-inset-bottom, 0px);
		overflow: hidden;
		touch-action: none;
		user-select: none;
		display: flex;
		flex-direction: column;
	}
	.stage {
		position: relative;
		flex: 1;
		min-height: 0;
		overflow: hidden;
	}
	.layer {
		position: absolute;
		inset: 0;
		width: 100%;
		height: 100%;
		object-fit: contain;
		display: block;
		opacity: 0;
		will-change: opacity;
		transform: translateZ(0);
	}
	.layer.show {
		opacity: 1;
	}

	.center {
		position: absolute;
		inset: 0;
		display: grid;
		place-items: center;
	}
	.spinner-wrap {
		z-index: 15;
	}
	.spinner {
		width: 2rem;
		height: 2rem;
		border: 3px solid var(--surface);
		border-top-color: var(--accent);
		border-radius: 50%;
		animation: spin 0.8s linear infinite;
	}
</style>
