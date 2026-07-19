<script>
	import { onMount, onDestroy } from 'svelte';
	import { get } from 'svelte/store';
	import { goto } from '$app/navigation';
	import { items as itemsApi, media, ApiError } from '$lib/api.js';
	import { isGuest } from '$lib/session.js';
	import Loading from '$lib/components/ui/Loading.svelte';
	import ReaderChrome from './ReaderChrome.svelte';

	let { id, detail, openContents = false } = $props();

	let title = $state('');
	let spine = $state([]);
	let chapter = $state(0);
	let page = $state(0);
	let pages = $state(1);
	let loading = $state(true);
	let error = $state(null);
	let seriesCtx = $state(null);
	let frameReady = $state(false);
	let pageCounts = $state([]);
	let toc = $state([]);
	let showToc = $state(false);

	let nextTarget = null;
	let iframeEl = null,
		frameDoc = null,
		frameBody = null,
		colStyleEl = null,
		pageW = 0;
	let measureFrameEl = null;
	let measuredKey = '';
	let measureToken = 0;

	const total = $derived(spine.length);
	const entry = $derived(spine[chapter] ?? null);
	const overallPct = $derived(
		total ? ((chapter + (pages ? Math.min(page + 1, pages) / pages : 0)) / total) * 100 : 0,
	);
	const atStart = $derived(chapter <= 0 && page <= 0 && seriesCtx?.prev_leaf_id == null);

	const totalPages = $derived(pageCounts.reduce((a, b) => a + (b || 0), 0));
	const measured = $derived(
		spine.length > 0 && pageCounts.length === spine.length && pageCounts.every((c) => c > 0),
	);
	const globalPage = $derived.by(() => {
		const before = pageCounts.slice(0, chapter).reduce((a, b) => a + (b || 0), 0);
		const span = pageCounts[chapter] || pages || 1;
		return before + Math.min(page, Math.max(0, span - 1)) + 1;
	});
	const progressPct = $derived(
		measured && totalPages ? (globalPage / totalPages) * 100 : overallPct,
	);
	function spineStart(idx) {
		let s = 0;
		for (let i = 0; i < idx; i++) s += pageCounts[i] || 0;
		return s;
	}
	function tocGlobalPage(t) {
		return spineStart(t.spineIndex) + (t.fragPage || 0) + 1;
	}
	const hasToc = $derived(new Set(toc.map((t) => `${t.spineIndex}#${t.fragment ?? ''}`)).size >= 2);
	const tocEntries = $derived(
		measured && hasToc ? toc.map((t) => ({ ...t, gp: tocGlobalPage(t) })) : [],
	);
	const currentTocIdx = $derived.by(() => {
		let idx = -1;
		for (let i = 0; i < tocEntries.length; i++) {
			if (tocEntries[i].gp <= globalPage) idx = i;
			else break;
		}
		return idx;
	});
	const pagesLeftInChapter = $derived.by(() => {
		if (currentTocIdx < 0) return null;
		const next = tocEntries[currentTocIdx + 1];
		const nextGp = next ? next.gp : totalPages + 1;
		return Math.max(0, nextGp - globalPage);
	});

	// Blob documents keep the iframe same-origin; <base> routes their relative assets.
	const blobCache = new Map();
	function clearBlobCache() {
		for (const e of blobCache.values()) if (e.url) URL.revokeObjectURL(e.url);
		blobCache.clear();
	}
	function docUrl(href) {
		let e = blobCache.get(href);
		if (e) return e.promise;
		e = { url: null, promise: null };
		e.promise = (async () => {
			const url = new URL(media.epubResource(href), window.location.origin).toString();
			const res = await fetch(url);
			if (!res.ok) throw new Error(`spine doc ${res.status}`);
			const ctype = (res.headers.get('content-type') || 'application/xhtml+xml')
				.split(';')[0]
				.trim();
			const text = await res.text();
			const head = /<head[^>]*>/i.exec(text);
			if (!head) return url;
			const baseHref = url.slice(0, url.lastIndexOf('/') + 1);
			const html =
				text.slice(0, head.index + head[0].length) +
				`<base href="${baseHref}"/>` +
				text.slice(head.index + head[0].length);
			e.url = URL.createObjectURL(new Blob([html], { type: ctype }));
			return e.url;
		})();
		blobCache.set(href, e);
		return e.promise;
	}

	let src = $state('');
	$effect(() => {
		const href = entry?.href;
		frameReady = false;
		src = '';
		if (!href) return;
		let live = true;
		docUrl(href)
			.then((u) => {
				if (live) src = u;
			})
			.catch((e) => {
				if (live) error = e?.message ?? String(e);
			});
		return () => (live = false);
	});

	async function loadManifest() {
		const myId = id;
		loading = true;
		error = null;
		spine = [];
		chapter = 0;
		page = 0;
		pageCounts = [];
		toc = [];
		showToc = false;
		measuredKey = '';
		measureToken++;
		clearBlobCache();
		seriesCtx = detail?.series ?? null;
		title = detail?.name ?? '';
		try {
			const m = await itemsApi.manifest(myId);
			if (myId !== id) return;
			spine = m.readingOrder ?? m.reading_order ?? [];
			title = detail?.name || m.metadata?.title || 'Untitled';
			toc = (m.toc ?? [])
				.map((t) => {
					const hash = t.href.indexOf('#');
					const path = hash < 0 ? t.href : t.href.slice(0, hash);
					const fragment = hash < 0 ? null : t.href.slice(hash + 1);
					let spineIndex = spine.findIndex((s) => s.href === path);
					if (spineIndex < 0) {
						const fn = path.split('/').pop();
						spineIndex = spine.findIndex((s) => s.href.split('/').pop() === fn);
					}
					return {
						href: t.href,
						title: t.title,
						level: t.level ?? 0,
						spineIndex,
						fragment,
						fragPage: 0,
					};
				})
				.filter((t) => t.spineIndex >= 0);
			showToc = openContents && toc.length > 0;
			const loc = detail?.progress_locator ?? {};
			chapter = spine.length ? Math.min(Math.max(0, loc.chapter | 0), spine.length - 1) : 0;
			nextTarget = Math.max(0, loc.page | 0);
			if (!spine.length) error = 'This book has no readable content.';
		} catch (e) {
			if (myId !== id) return;
			if (!(e instanceof ApiError && e.status === 401)) error = e?.message ?? String(e);
		} finally {
			if (myId === id) loading = false;
		}
	}

	const READING_THEME = `
		:root { color-scheme: light; }
		html:focus, body:focus, html:focus-visible, body:focus-visible { outline: none; }
		p, div, li { orphans: 2; widows: 2; }
		img, svg, video { max-width: 100% !important; height: auto; }
		table, pre { max-width: 100% !important; }
	`;
	const PADV = 44;
	const READ_MAX_COL = 720;
	function columnCss(w, h) {
		const margin = Math.max(24, Math.round((w - READ_MAX_COL) / 2));
		const gap = margin * 2;
		const colW = Math.max(120, Math.round(w - gap));
		return `
			html { margin:0; padding:0; height:${h}px; overflow:hidden; background:#fbfbf9; }
			body {
				margin:0 !important;
				box-sizing:border-box;
				height:${h}px !important;
				max-height:${h}px !important;
				padding:${PADV}px ${margin}px !important;
				color:#1a1a1a;
				line-height:1.6;
				column-width:${colW}px !important;
				column-gap:${gap}px !important;
				column-fill:auto !important;
				-webkit-column-width:${colW}px !important;
				-webkit-column-gap:${gap}px !important;
				column-count:auto !important;
				width:auto !important;
				transition: transform .2s ease;
			}
			img, svg, video { max-height:${Math.max(80, h - PADV * 2)}px !important; }
		`;
	}

	function applyPage(animate = true) {
		if (!frameBody) return;
		if (!animate) frameBody.style.transition = 'none';
		frameBody.style.transform = `translateX(${-page * pageW}px)`;
		if (!animate) {
			void frameBody.offsetWidth;
			frameBody.style.transition = 'transform .2s ease';
		}
		reportProgress();
	}

	function relayoutWhenSized(target, attempts = 30) {
		// A newly keyed iframe can fire load before it has layout dimensions.
		const el = iframeEl;
		if (!el) return;
		if (el.clientWidth && el.clientHeight) {
			relayout(target);
			return;
		}
		if (attempts <= 0) return;
		requestAnimationFrame(() => {
			if (iframeEl === el) relayoutWhenSized(target, attempts - 1);
		});
	}

	function relayout(target) {
		if (!frameDoc || !frameBody || !iframeEl) return;
		const w = iframeEl.clientWidth;
		const h = iframeEl.clientHeight;
		if (!w || !h) return;
		const frac = pages > 1 ? page / (pages - 1) : 0;
		pageW = w;
		if (!colStyleEl) {
			colStyleEl = frameDoc.createElement('style');
			frameDoc.head?.appendChild(colStyleEl);
		}
		colStyleEl.textContent = columnCss(w, h);
		pages = Math.max(1, Math.round(frameBody.scrollWidth / pageW));
		if (target === 'last') page = pages - 1;
		else if (typeof target === 'number') page = Math.min(Math.max(0, target), pages - 1);
		else page = Math.round(frac * (pages - 1));
		applyPage(false);
		maybeMeasure();
	}

	function maybeMeasure() {
		if (!iframeEl || !measureFrameEl || !total) return;
		const w = iframeEl.clientWidth;
		const h = iframeEl.clientHeight;
		if (!w || !h) return;
		const key = `${w}x${h}`;
		if (key === measuredKey) return;
		measuredKey = key;
		measureFrameEl.style.width = `${w}px`;
		measureFrameEl.style.height = `${h}px`;
		measureAll(w, h);
	}

	async function measureAll(w, h) {
		// A hidden iframe measures every spine entry with the active reader geometry.
		const token = ++measureToken;
		pageCounts = new Array(total).fill(0);
		for (let i = 0; i < total; i++) {
			if (token !== measureToken) return;
			const frags = toc.filter((t) => t.spineIndex === i && t.fragment).map((t) => t.fragment);
			const res = await measureSpine(spine[i]?.href, w, h, frags);
			if (token !== measureToken) return;
			pageCounts[i] = res.count;
			for (const t of toc) {
				if (t.spineIndex === i) t.fragPage = t.fragment ? (res.anchors[t.fragment] ?? 0) : 0;
			}
			await new Promise((r) => requestAnimationFrame(() => r()));
		}
		if (token === measureToken) reportProgress();
	}

	function measureSpine(href, w, h, fragments) {
		return new Promise((resolve) => {
			const frame = measureFrameEl;
			const fallback = { count: 1, anchors: {} };
			if (!frame || !href) return resolve(fallback);
			let settled = false;
			const finish = (r) => {
				if (settled) return;
				settled = true;
				clearTimeout(guard);
				frame.onload = null;
				resolve(r);
			};
			const guard = setTimeout(() => finish(fallback), 5000);
			frame.onload = async () => {
				try {
					const doc = frame.contentDocument;
					const body = doc?.body;
					if (!doc || !body) return finish(fallback);
					const st = doc.createElement('style');
					st.textContent = READING_THEME + columnCss(w, h);
					doc.head?.appendChild(st);
					if (doc.fonts?.ready) await doc.fonts.ready.catch(() => {});
					if (settled) return;
					const count = Math.max(1, Math.round(body.scrollWidth / w));
					const bodyLeft = body.getBoundingClientRect().left;
					const anchors = {};
					for (const frag of fragments) {
						const el =
							doc.getElementById(frag) || doc.querySelector(`a[name="${CSS.escape(frag)}"]`);
						const x = el ? el.getBoundingClientRect().left - bodyLeft : 0;
						anchors[frag] = Math.max(0, Math.min(count - 1, Math.round(x / w)));
					}
					finish({ count, anchors });
				} catch {
					finish(fallback);
				}
			};
			docUrl(href).then(
				(u) => {
					if (!settled) frame.src = u;
				},
				() => finish(fallback),
			);
		});
	}

	let saveTimer;
	let pending = null;
	function reportProgress() {
		if (loading || !total) return;
		const active = currentTocIdx >= 0 ? tocEntries[currentTocIdx] : null;
		const nextToc = active ? tocEntries[currentTocIdx + 1] : null;
		const activeSpan = active ? Math.max(1, (nextToc?.gp ?? totalPages + 1) - active.gp) : 1;
		const activeProgress = active
			? Math.min(Math.max((globalPage - active.gp) / activeSpan, 0), 1)
			: 0;
		pending = {
			chapter,
			page,
			value: progressPct / 100,
			toc: active ? { href: active.href, title: active.title, progress: activeProgress } : null,
		};
		clearTimeout(saveTimer);
		saveTimer = setTimeout(flushProgress, 500);
	}
	function flushProgress() {
		clearTimeout(saveTimer);
		if (pending == null || !total || get(isGuest)) return Promise.resolve();
		const { chapter: ch, page: pg, value, toc: activeToc } = pending;
		pending = null;
		return itemsApi
			.saveProgress(
				id,
				{ value, locator: { chapter: ch, page: pg, ...(activeToc ? { toc: activeToc } : {}) } },
				{ keepalive: true },
			)
			.catch(() => {});
	}

	function next() {
		if (page < pages - 1) {
			page++;
			applyPage();
		} else nextChapter(0);
	}
	function prev() {
		if (page > 0) {
			page--;
			applyPage();
		} else prevChapter('last');
	}
	function nextChapter(target = 0) {
		if (chapter < total - 1) {
			nextTarget = target;
			chapter++;
		} else if (seriesCtx?.next_leaf_id != null) goToLeaf(seriesCtx.next_leaf_id);
		else goBack();
	}
	function prevChapter(target = 0) {
		if (chapter > 0) {
			nextTarget = target;
			chapter--;
		} else if (seriesCtx?.prev_leaf_id != null) goToLeaf(seriesCtx.prev_leaf_id);
	}
	function jumpTo(t) {
		showToc = false;
		if (t.spineIndex === chapter) {
			page = Math.min(Math.max(0, t.fragPage || 0), pages - 1);
			applyPage();
		} else {
			nextTarget = t.fragPage || 0;
			chapter = t.spineIndex;
		}
	}
	async function goBack() {
		await flushProgress();
		goto(`/item/${id}`);
	}
	async function goToLeaf(leafId) {
		if (leafId == null) return;
		await flushProgress();
		goto(`/reader/${leafId}`);
	}

	function onFrameLoad(e) {
		iframeEl = e.currentTarget;
		frameDoc = null;
		frameBody = null;
		colStyleEl = null;
		try {
			frameDoc = iframeEl.contentDocument;
			frameBody = frameDoc?.body ?? null;
			if (frameDoc && frameBody) {
				const base = frameDoc.createElement('style');
				base.textContent = READING_THEME;
				frameDoc.head?.appendChild(base);
				frameDoc.addEventListener('keydown', onKey);
				const target = nextTarget;
				nextTarget = null;
				relayoutWhenSized(target ?? 0);
			}
		} catch {
			/* ignored */
		}
		frameReady = true;
	}

	function onKey(e) {
		if (showToc && e.key !== 'Escape') return;
		if (e.key === 'ArrowRight' || e.key === 'PageDown') {
			e.preventDefault();
			next();
		} else if (e.key === 'ArrowLeft' || e.key === 'PageUp') {
			e.preventDefault();
			prev();
		} else if (e.key === ' ' || e.key === 'Spacebar') {
			e.preventDefault();
			if (e.shiftKey) prev();
			else next();
		} else if (e.key === 'Escape') {
			if (showToc) showToc = false;
			else goBack();
		}
	}

	let resizeTimer;
	function onResize() {
		clearTimeout(resizeTimer);
		resizeTimer = setTimeout(() => relayout(), 120);
	}

	$effect(() => {
		id;
		loadManifest();
	});

	onMount(() => {
		window.addEventListener('keydown', onKey);
		window.addEventListener('pagehide', flushProgress);
		window.addEventListener('resize', onResize);
	});
	onDestroy(() => {
		window.removeEventListener('keydown', onKey);
		window.removeEventListener('pagehide', flushProgress);
		window.removeEventListener('resize', onResize);
		clearTimeout(saveTimer);
		clearTimeout(resizeTimer);
		measureToken++;
		clearBlobCache();
		flushProgress();
	});
</script>

<div class="reader">
	<ReaderChrome
		{title}
		{progressPct}
		showProgress={!!total}
		{seriesCtx}
		onback={goBack}
		onprev={() => goToLeaf(seriesCtx?.prev_leaf_id)}
		onnext={() => goToLeaf(seriesCtx?.next_leaf_id)}
	>
		{#snippet leadActions()}
			{#if hasToc}
				<button
					class="volnav toc-btn"
					class:active={showToc}
					title="Contents"
					aria-label="Contents"
					aria-expanded={showToc}
					onclick={() => (showToc = !showToc)}
				>
					<svg
						viewBox="0 0 24 24"
						fill="none"
						stroke="currentColor"
						stroke-width="2"
						stroke-linecap="round"
						stroke-linejoin="round"
						><path d="M8 6h13M8 12h13M8 18h13M3 6h.01M3 12h.01M3 18h.01" /></svg
					>
				</button>
			{/if}
		{/snippet}

		{#snippet counter()}{#if measured}{globalPage} / {totalPages}{/if}{/snippet}

		{#snippet children()}
			{#if showToc}
				<button class="toc-backdrop" aria-label="Close contents" onclick={() => (showToc = false)}
				></button>
				<nav class="toc-panel" aria-label="Table of contents">
					<div class="toc-head">Contents</div>
					<ul>
						{#each tocEntries.length ? tocEntries : toc as t, i (i)}
							<li>
								<button
									class="toc-row"
									class:current={tocEntries.length && i === currentTocIdx}
									style={`padding-left:${0.9 + (t.level || 0) * 1.1}rem`}
									onclick={() => jumpTo(t)}
								>
									<span class="toc-label">{t.title}</span>
									{#if tocEntries.length}<span class="toc-page">{t.gp}</span>{/if}
								</button>
							</li>
						{/each}
					</ul>
				</nav>
			{/if}

			<div class="stage">
				{#if loading}
					<div class="center"><Loading label="" /></div>
				{:else if error}
					<div class="center"><div class="errbox">Couldn't open this book: {error}</div></div>
				{:else if src}
					{#key src}
						<iframe
							class="doc"
							class:ready={frameReady}
							{src}
							title={`${title} — section ${chapter + 1}`}
							sandbox="allow-same-origin"
							onload={onFrameLoad}
						></iframe>
					{/key}

					<button
						class="edge left"
						onclick={prev}
						class:hidden={atStart}
						aria-label="Previous page"
					>
						<svg
							viewBox="0 0 24 24"
							fill="none"
							stroke="currentColor"
							stroke-width="2"
							stroke-linecap="round"
							stroke-linejoin="round"><path d="M15 18l-6-6 6-6" /></svg
						>
					</button>
					<button class="edge right" onclick={next} aria-label="Next page">
						<svg
							viewBox="0 0 24 24"
							fill="none"
							stroke="currentColor"
							stroke-width="2"
							stroke-linecap="round"
							stroke-linejoin="round"><path d="M9 18l6-6-6-6" /></svg
						>
					</button>
				{/if}

				<iframe
					class="measure"
					bind:this={measureFrameEl}
					title=""
					aria-hidden="true"
					tabindex="-1"
					sandbox="allow-same-origin"
				></iframe>

				{#if pagesLeftInChapter != null}
					<div class="chapfoot">
						{pagesLeftInChapter}
						{pagesLeftInChapter === 1 ? 'page' : 'pages'} left in chapter
					</div>
				{/if}
			</div>
		{/snippet}
	</ReaderChrome>
</div>

<style>
	.reader {
		position: fixed;
		inset: 0;
		background: var(--bg);
		overflow: hidden;
		display: flex;
		flex-direction: column;
		padding-bottom: env(safe-area-inset-bottom, 0px);
	}
	.stage {
		position: relative;
		flex: 1;
		min-height: 0;
		background: #fbfbf9;
	}
	.doc {
		width: 100%;
		height: 100%;
		border: none;
		display: block;
		background: #fbfbf9;
		opacity: 0;
		transition: opacity 0.12s ease;
	}
	.doc.ready {
		opacity: 1;
	}

	.measure {
		position: absolute;
		inset: 0;
		width: 100%;
		height: 100%;
		border: none;
		opacity: 0;
		pointer-events: none;
		z-index: 0;
	}
	.doc:focus,
	.doc:focus-visible {
		outline: none;
	}

	.edge {
		position: absolute;
		top: 0;
		bottom: 0;
		width: clamp(2.5rem, 7%, 5rem);
		display: flex;
		align-items: center;
		border: none;
		background: transparent;
		color: var(--muted);
		opacity: 0.35;
		cursor: pointer;
		transition:
			opacity var(--ease),
			color var(--ease);
		z-index: 10;
	}
	.edge:hover {
		opacity: 1;
		color: var(--text);
	}
	.edge.left {
		left: 0;
		justify-content: flex-start;
		padding-left: var(--space-3);
	}
	.edge.right {
		right: 0;
		justify-content: flex-end;
		padding-right: var(--space-3);
	}
	.edge.hidden {
		opacity: 0;
		pointer-events: none;
	}
	.edge svg {
		width: 1.9rem;
		height: 1.9rem;
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

	.toc-btn.active {
		color: var(--text);
	}
	.toc-backdrop {
		position: fixed;
		inset: 0;
		background: transparent;
		border: none;
		padding: 0;
		z-index: 25;
		cursor: default;
	}
	.toc-panel {
		position: absolute;
		top: calc(var(--space-3) + env(safe-area-inset-top, 0px) + 2.4rem);
		left: var(--space-4);
		z-index: 26;
		width: min(22rem, calc(100vw - 2 * var(--space-4)));
		max-height: min(75vh, 40rem);
		overflow-y: auto;
		background: color-mix(in srgb, var(--surface) 98%, transparent);
		border: 1px solid var(--border);
		border-radius: 14px;
		box-shadow: 0 16px 48px rgba(0, 0, 0, 0.4);
		padding: 0.4rem;
		backdrop-filter: blur(12px);
	}
	.toc-head {
		text-align: center;
		font-size: 0.75rem;
		letter-spacing: 0.06em;
		text-transform: uppercase;
		color: var(--muted);
		padding: 0.5rem 0 0.6rem;
		border-bottom: 1px solid var(--border);
		margin-bottom: 0.3rem;
	}
	.toc-panel ul {
		list-style: none;
		margin: 0;
		padding: 0;
	}
	.toc-row {
		display: flex;
		align-items: baseline;
		gap: 0.75rem;
		width: 100%;
		background: transparent;
		border: none;
		border-radius: 9px;
		padding: 0.5rem 0.8rem;
		color: var(--text);
		font-size: 0.9rem;
		text-align: left;
		cursor: pointer;
	}
	.toc-row:hover {
		background: var(--surface-2);
	}
	.toc-row.current {
		background: var(--surface-2);
		font-weight: 600;
	}
	.toc-label {
		flex: 1 1 auto;
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.toc-page {
		flex: 0 0 auto;
		color: var(--muted);
		font-variant-numeric: tabular-nums;
		font-size: 0.82rem;
	}

	.chapfoot {
		position: absolute;
		right: var(--space-5);
		bottom: 6px;
		font-size: 0.72rem;
		color: var(--muted);
		font-variant-numeric: tabular-nums;
		z-index: 15;
		pointer-events: none;
	}
	.center {
		position: absolute;
		inset: 0;
		display: grid;
		place-items: center;
	}
	.muted {
		color: var(--muted);
	}
	.errbox {
		border: 1px solid rgba(255, 128, 136, 0.4);
		background: rgba(255, 128, 136, 0.1);
		color: #ff8088;
		padding: 0.75rem 1rem;
		border-radius: 10px;
		max-width: 80%;
		text-align: center;
	}
</style>
