<script>
	import { page } from '$app/stores';
	import { plugins as pluginsApi, media, ApiError } from '$lib/api.js';
	import PagedReader from '$lib/reader/PagedReader.svelte';
	import VerticalReader from '$lib/reader/VerticalReader.svelte';

	const plugin = $derived(decodeURIComponent($page.params.plugin));
	const reference = $derived(decodeURIComponent($page.params.ref));
	const kind = $derived($page.url.searchParams.get('kind') ?? '');
	const back = $derived($page.url.searchParams.get('back') ?? '');
	const start = $derived(
		Math.max(0, (parseInt($page.url.searchParams.get('page') ?? '1', 10) || 1) - 1),
	);
	const title = $derived($page.state?.title ?? '');

	let pages = $state([]);
	let loading = $state(true);
	let error = $state(null);
	let vertical = $state(false);

	$effect(() => {
		plugin;
		reference;
		load();
	});
	async function load() {
		loading = true;
		error = null;
		try {
			const modeParam = new URLSearchParams(location.search).get('mode');
			const needList = modeParam !== 'paged' && modeParam !== 'vertical';
			const [d, list] = await Promise.all([
				pluginsApi.pages(plugin, reference),
				needList ? pluginsApi.list().catch(() => []) : Promise.resolve(null),
			]);
			pages = d.pages ?? [];
			if (modeParam === 'vertical') vertical = true;
			else if (modeParam === 'paged') vertical = false;
			else vertical = (list?.find((p) => p.id === plugin)?.reading_mode ?? 'paged') === 'vertical';
			if (!pages.length) error = 'This source has no readable pages.';
		} catch (e) {
			if (!(e instanceof ApiError && e.status === 401)) error = e?.message ?? String(e);
		} finally {
			loading = false;
		}
	}

	const backHref = $derived(
		`/source/${encodeURIComponent(plugin)}/${encodeURIComponent(back || reference)}${kind ? `?kind=${encodeURIComponent(kind)}` : ''}`,
	);
	const source = $derived(
		pages.length
			? {
					pageUrl: (n) => media.pluginImage(plugin, pages[n].image_url),
					pageCount: pages.length,
					title,
					start,
					backHref,
				}
			: null,
	);
</script>

{#if loading}
	<div class="boot"><div class="spinner"></div></div>
{:else if error}
	<div class="boot"><div class="errbox">{error}</div></div>
{:else if source && vertical}
	<VerticalReader {source} />
{:else if source}
	<PagedReader {source} />
{/if}

<style>
	.boot {
		position: fixed;
		inset: 0;
		background: var(--bg);
		display: grid;
		place-items: center;
	}
	.spinner {
		width: 2rem;
		height: 2rem;
		border: 3px solid var(--surface);
		border-top-color: var(--accent);
		border-radius: 50%;
		animation: spin 0.8s linear infinite;
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
