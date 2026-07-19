<script>
	import { get } from 'svelte/store';
	import { goto, afterNavigate } from '$app/navigation';
	import { kindLabel } from '$lib/kinds.js';
	import { seriesHint } from '$lib/seriesHint.js';
	import { navStack, pushNav, popNav, setNav, markPop, consumePop } from '$lib/navStack.js';
	import Loading from '$lib/components/ui/Loading.svelte';
	import ArtworkBackdrop from '$lib/components/ArtworkBackdrop.svelte';

	let {
		title = '',
		ready = false,
		loading = false,
		error = null,
		artwork = '',
		defaultBackTarget = '/',
		defaultBackLabel = 'Library',
		backOverride = null,
		escapeEnabled = true,
		banner,
		topActions,
		aside,
		detail,
	} = $props();

	const APP_PAGES = new Set(['/upload', '/login']);
	const isReaderPath = (p) =>
		p.startsWith('/reader/') || (p.startsWith('/source/') && p.endsWith('/read'));

	afterNavigate((nav) => {
		if (nav.type === 'popstate') return;
		if (consumePop()) return;
		const from = nav.from?.url;
		if (!from) return;
		const p = from.pathname;
		if (p.startsWith('/item/') || p.startsWith('/series/')) {
			pushNav(p + from.search);
		} else if (!isReaderPath(p) && !APP_PAGES.has(p)) {
			setNav([p + from.search]);
		}
	});

	const backPath = $derived($navStack.length ? $navStack[$navStack.length - 1] : null);
	const effBack = $derived(backOverride ?? backPath);
	const backTarget = $derived(effBack ?? defaultBackTarget ?? '/');
	const backReady = $derived(effBack != null || ready);
	const backLabel = $derived.by(() => {
		if (!backReady) return '';
		if (effBack == null) return defaultBackLabel;
		if (effBack.startsWith('/item/')) return 'Back';
		if (effBack.startsWith('/source/')) return 'Back';
		if (effBack.startsWith('/series/')) {
			const h = $seriesHint;
			return h && effBack === `/series/${h.id}` ? h.title : 'Series';
		}
		if (effBack === '/' || effBack.startsWith('/?')) return 'Library';
		const base = effBack.split('?')[0];
		if (base.endsWith('/browse')) return 'Browse';
		if (base === '/for-you') return 'Library';
		const kindBase = base.replace(/\/(for-you|tags)$/, '');
		return kindLabel(decodeURIComponent(kindBase.slice(1)));
	});

	function goBack() {
		const target = backTarget;
		if (!backOverride && get(navStack).length) {
			popNav();
			if (target.startsWith('/item/') || target.startsWith('/series/')) markPop();
		}
		goto(target);
	}
	function onBackClick(e) {
		if (e.metaKey || e.ctrlKey || e.shiftKey || e.button !== 0) return;
		e.preventDefault();
		goBack();
	}
	function onKey(e) {
		if (e.key === 'Escape' && escapeEnabled) goBack();
	}
	$effect(() => {
		window.addEventListener('keydown', onKey);
		return () => window.removeEventListener('keydown', onKey);
	});
</script>

<div class="preview" class:artwork={!!artwork}>
	{#if artwork}<ArtworkBackdrop src={artwork} />{/if}
	<header class="topbar">
		<a class="back" href={backTarget} onclick={onBackClick}>{backReady ? `← ${backLabel}` : '←'}</a>
		<span class="toptitle">{title}</span>
		<span class="topacts">{@render topActions?.()}</span>
	</header>

	<div class="page">
		{@render banner?.()}
		{#if loading && !ready}
			<Loading />
		{:else if error}
			<p class="err">{error}</p>
		{:else if ready}
			<div class="layout">
				<aside class="side">{@render aside?.()}</aside>
				<section class="detail">{@render detail?.()}</section>
			</div>
		{/if}
	</div>
</div>

<style>
	.preview {
		position: relative;
		isolation: isolate;
		min-height: 100vh;
		background: var(--bg);
	}
	.topbar {
		position: sticky;
		top: 0;
		z-index: var(--z-header);
		display: grid;
		grid-template-columns: 1fr minmax(0, auto) 1fr;
		align-items: center;
		gap: var(--space-4);
		padding: calc(var(--space-3) + env(safe-area-inset-top, 0px)) var(--space-5) var(--space-3);
		background: color-mix(in srgb, var(--bg) 88%, transparent);
		backdrop-filter: blur(8px);
		border-bottom: 1px solid var(--border);
	}
	.preview.artwork .topbar {
		background: color-mix(in srgb, var(--bg) 84%, transparent);
	}
	.topacts {
		justify-self: end;
		display: inline-flex;
		align-items: center;
		position: relative;
	}
	.toptitle {
		text-align: center;
		font-size: 0.9rem;
		color: var(--text);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.page {
		position: relative;
		z-index: 1;
		padding: var(--space-4) var(--space-5);
	}
	.back {
		justify-self: start;
		color: var(--muted);
		font-size: 0.9rem;
		white-space: nowrap;
	}
	.back:hover {
		color: var(--text);
	}
	.err {
		padding: var(--space-5) 0;
		color: #e0566f;
	}
	.layout {
		display: flex;
		align-items: stretch;
		gap: var(--space-6);
		margin-top: calc(-1 * var(--space-4));
	}
	.side {
		flex: 0 0 clamp(225px, 16%, 250px);
		display: flex;
		flex-direction: column;
		gap: var(--space-3);
		padding-top: var(--space-2);
		align-self: flex-start;
		position: sticky;
		top: 2.55rem;
		max-height: calc(100vh - 3.05rem);
		overflow-y: auto;
		scrollbar-width: none;
	}
	.side::-webkit-scrollbar {
		display: none;
	}
	.detail {
		flex: 1;
		min-width: 0;
		border-left: 1px solid var(--border);
		padding-left: var(--space-6);
		padding-top: var(--space-4);
	}
	@media (max-width: 720px) {
		.layout {
			flex-direction: column;
			margin-top: 0;
			gap: var(--space-3);
		}
		.side {
			padding-top: 0;
			width: 100%;
			max-width: 100%;
			position: static;
			max-height: none;
			overflow: visible;
		}
		.detail {
			border-left: none;
			padding-left: 0;
			padding-top: 0;
		}
	}
</style>
