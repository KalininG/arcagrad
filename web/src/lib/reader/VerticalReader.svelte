<script>
	import { onMount, onDestroy, tick, untrack } from 'svelte';
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

	const pageUrl = (n) => (source ? source.pageUrl(n) : media.page(id, n, detail?.version));
	const progressPct = $derived(pageCount ? ((current + 1) / pageCount) * 100 : 0);

	let wStart = $state(0);
	let wEnd = $state(-1);
	const pages = $derived(
		Array.from({ length: Math.max(0, wEnd - wStart + 1) }, (_, i) => wStart + i),
	);
	let dims = $state({});

	let scroller = $state(null);
	let pageEls = $state({});
	let session = 0;
	let ready = $state(false);
	let lastScrollTs = 0;

	const APPEND_BATCH = 3;
	const NEAR_TOP = 2.5;
	const NEAR_BOTTOM = 1.5;
	const PREFILL_ABOVE = 2;
	const MAX_WINDOW = 40;

	const decodeCache = new Map();

	let scrollRaf = 0;
	function onScroll() {
		lastScrollTs = performance.now();
		if (scrollRaf) return;
		scrollRaf = requestAnimationFrame(() => {
			scrollRaf = 0;
			if (!scroller) return;
			maybeExtend();
			trackCurrent();
		});
	}

	function trackCurrent() {
		const mid = scroller.getBoundingClientRect().top + scroller.clientHeight * 0.4;
		let best = null;
		for (const n of pages) {
			const el = pageEls[n];
			if (!el) continue;
			if (!dims[n]) continue;
			if (el.getBoundingClientRect().top <= mid) best = n;
			else break;
		}
		if (best != null && best !== current) {
			current = best;
			reportProgress();
		}
	}

	function nearTop() {
		return scroller.scrollTop < scroller.clientHeight * NEAR_TOP;
	}
	function nearBottom() {
		return (
			scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight <
			scroller.clientHeight * NEAR_BOTTOM
		);
	}

	let extending = false;
	let retryTimer;
	async function maybeExtend() {
		if (extending) return;
		extending = true;
		try {
			if (nearBottom() && wEnd < pageCount - 1) {
				await appendBelow();
			}
			while (nearTop() && wStart > 0) {
				const active = performance.now() - lastScrollTs < 120;
				const pinned = scroller.scrollTop < scroller.clientHeight * 0.25;
				if (active && !pinned) {
					ensureDecoded(wStart - 1);
					clearTimeout(retryTimer);
					retryTimer = setTimeout(maybeExtend, 150);
					break;
				}
				await prependAbove();
			}
		} finally {
			extending = false;
		}
	}

	async function appendBelow() {
		const mySession = session;
		const newEnd = Math.min(pageCount - 1, wEnd + APPEND_BATCH);
		wEnd = newEnd;
		await tick();
		if (mySession !== session) return;
		pruneTop();
	}

	const decodesInFlight = new Map();
	function ensureDecoded(n) {
		if (dims[n]) return Promise.resolve();
		const inflight = decodesInFlight.get(n);
		if (inflight) return inflight;
		const mySession = session;
		const url = pageUrl(n);
		const img = new Image();
		img.decoding = 'async';
		img.src = url;
		const p = img
			.decode()
			.catch(() => {})
			.then(() => {
				decodesInFlight.delete(n);
				if (mySession !== session) return;
				if (img.naturalWidth > 0) {
					dims[n] = [img.naturalWidth, img.naturalHeight];
					decodeCache.set(url, img);
				}
			});
		decodesInFlight.set(n, p);
		return p;
	}

	async function prependAbove() {
		const mySession = session;
		const n = wStart - 1;
		await ensureDecoded(n);
		if (mySession !== session) return;

		// Preserve the visible position when decoded pages are inserted above it.
		const prevTop = scroller.scrollTop;
		const prevH = scroller.scrollHeight;
		wStart = n;
		await tick();
		if (mySession !== session) return;
		scroller.scrollTop = prevTop + (scroller.scrollHeight - prevH);
		pruneBottom();
	}

	async function prefillAbove(count) {
		const mySession = session;
		for (let i = 0; i < count && wStart > 0; i++) {
			if (mySession !== session || extending) return;
			extending = true;
			try {
				await prependAbove();
			} finally {
				extending = false;
			}
		}
	}

	function pruneTop() {
		if (wEnd - wStart + 1 <= MAX_WINDOW) return;
		const prevTop = scroller.scrollTop;
		const prevH = scroller.scrollHeight;
		wStart = wEnd - MAX_WINDOW + 1;
		tick().then(() => {
			scroller.scrollTop = prevTop + (scroller.scrollHeight - prevH);
		});
	}
	function pruneBottom() {
		if (wEnd - wStart + 1 <= MAX_WINDOW) return;
		wEnd = wStart + MAX_WINDOW - 1;
	}

	function onPageImgLoad(n, img) {
		if (img?.naturalWidth > 0) dims[n] = [img.naturalWidth, img.naturalHeight];
		decodeCache.delete(img?.currentSrc ?? '');
		decodeCache.delete(pageUrl(n));
	}

	function onWheel(e) {
		if (e.deltaY < 0 && scroller && scroller.scrollTop === 0) maybeExtend();
	}
	let touchY = 0;
	function onTouchStart(e) {
		touchY = e.touches[0]?.clientY ?? 0;
	}
	function onTouchMove(e) {
		const y = e.touches[0]?.clientY ?? 0;
		if (y > touchY && scroller && scroller.scrollTop === 0) maybeExtend();
		touchY = y;
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

	async function goBack() {
		await flushProgress();
		goto(source ? source.backHref : `/item/${id}`);
	}
	async function goToLeaf(leafId) {
		if (leafId == null) return;
		await flushProgress();
		goto(`/reader/${leafId}`);
	}

	let downX = 0;
	let downY = 0;
	function onPointerDown(e) {
		downX = e.clientX;
		downY = e.clientY;
	}
	function onPointerUp(e) {
		if (e.target.closest('button, a')) return;
		if (Math.abs(e.clientX - downX) < 10 && Math.abs(e.clientY - downY) < 10) {
			immersive = !immersive;
		}
	}

	function onKey(e) {
		if (e.key === 'Escape') {
			goBack();
		} else if (e.key === ' ' || e.key === 'PageDown' || e.key === 'ArrowRight') {
			e.preventDefault();
			scroller?.scrollBy({ top: scroller.clientHeight * 0.85, behavior: 'smooth' });
		} else if (e.key === 'PageUp' || e.key === 'ArrowLeft') {
			e.preventDefault();
			scroller?.scrollBy({ top: -scroller.clientHeight * 0.85, behavior: 'smooth' });
		}
	}

	async function initReader() {
		session++;
		pendingPage = null;
		clearTimeout(progressTimer);
		dims = {};
		pageEls = {};
		decodeCache.clear();
		decodesInFlight.clear();
		clearTimeout(retryTimer);

		if (source) {
			title = source.title ?? '';
			pageCount = source.pageCount ?? 0;
			seriesCtx = null;
			current = Math.max(0, Math.min(source.start ?? 0, Math.max(0, pageCount - 1)));
		} else {
			title = detail?.name ?? '';
			pageCount = detail?.page_count ?? 0;
			seriesCtx = detail?.series ?? null;
			const q = parseInt(new URLSearchParams(location.search).get('page') ?? '', 10);
			const resume = Number.isFinite(q) ? q - 1 : (detail?.progress ?? 0);
			current = Math.max(0, Math.min(resume, Math.max(0, pageCount - 1)));
		}
		wStart = current;
		wEnd = Math.min(pageCount - 1, current + APPEND_BATCH);

		const mySession = session;
		ready = false;
		await ensureDecoded(current);
		if (mySession !== session) return;
		await tick();
		if (scroller) scroller.scrollTop = 0;
		ready = true;
		reportProgress();
		prefillAbove(PREFILL_ABOVE);
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
		if (scrollRaf) cancelAnimationFrame(scrollRaf);
		clearTimeout(retryTimer);
		decodeCache.clear();
		flushProgress();
	});
</script>

<div class="reader" class:immersive onpointerdown={onPointerDown} onpointerup={onPointerUp}>
	<ReaderChrome
		{title}
		{progressPct}
		showProgress={!!pageCount}
		{seriesCtx}
		onback={goBack}
		onprev={() => goToLeaf(seriesCtx?.prev_leaf_id)}
		onnext={() => goToLeaf(seriesCtx?.next_leaf_id)}
		headerHidden={immersive}
		progressHidden={immersive}
	>
		{#snippet counter()}{pageCount ? current + 1 : 0} / {pageCount}{/snippet}

		{#snippet children()}
			{#if !ready}
				<div class="boot"><div class="spinner"></div></div>
			{/if}
			<div
				class="scroll"
				class:booting={!ready}
				bind:this={scroller}
				onscroll={onScroll}
				onwheel={onWheel}
				ontouchstart={onTouchStart}
				ontouchmove={onTouchMove}
			>
				<div class="column">
					{#each pages as n (n)}
						<div
							class="page"
							data-page={n}
							bind:this={pageEls[n]}
							style={dims[n] ? `aspect-ratio: ${dims[n][0]} / ${dims[n][1]}` : ''}
						>
							<img
								src={pageUrl(n)}
								alt={`Page ${n + 1}`}
								decoding="async"
								draggable="false"
								onload={(e) => onPageImgLoad(n, e.currentTarget)}
							/>
						</div>
					{/each}

					{#if wEnd >= pageCount - 1}
						<div class="endcap">
							{#if seriesCtx?.next_leaf_id != null}
								<button class="endbtn primary" onclick={() => goToLeaf(seriesCtx.next_leaf_id)}>
									Next volume →
								</button>
							{/if}
							<button class="endbtn" onclick={goBack}>Back to item</button>
						</div>
					{/if}
				</div>
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
		display: flex;
		flex-direction: column;
		user-select: none;
	}

	.scroll {
		flex: 1;
		min-height: 0;
		overflow-y: auto;
		overscroll-behavior: contain;
		touch-action: pan-y;
		overflow-anchor: none;
	}
	.scroll.booting {
		visibility: hidden;
	}
	.boot {
		position: absolute;
		inset: 0;
		display: grid;
		place-items: center;
		z-index: 10;
	}
	.spinner {
		width: 2rem;
		height: 2rem;
		border: 3px solid var(--surface);
		border-top-color: var(--accent);
		border-radius: 50%;
		animation: spin 0.8s linear infinite;
	}
	.column {
		margin: 0 auto;
		width: min(100%, 720px);
	}
	.page {
		min-height: 40px;
	}
	.page img {
		display: block;
		width: 100%;
		height: auto;
	}

	.endcap {
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: var(--space-2);
		padding: var(--space-6, 3rem) var(--space-4);
	}
	.endbtn {
		padding: var(--space-2) var(--space-4);
		border: 1px solid var(--border);
		border-radius: var(--radius, 6px);
		background: var(--surface);
		color: var(--text);
		font-size: 0.9rem;
		cursor: pointer;
	}
	.endbtn.primary {
		background: var(--accent);
		border-color: var(--accent);
		color: #fff;
		font-weight: 600;
	}
	.endbtn:hover {
		filter: brightness(1.1);
	}
</style>
