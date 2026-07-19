<script>
	import { page } from '$app/stores';
	import { items as itemsApi, ApiError } from '$lib/api.js';
	import PagedReader from '$lib/reader/PagedReader.svelte';
	import VerticalReader from '$lib/reader/VerticalReader.svelte';
	import ReflowableReader from '$lib/reader/ReflowableReader.svelte';

	let id = $derived($page.params.id);
	let detail = $state(null);
	let loading = $state(true);
	let error = $state(null);

	const engine = $derived.by(() => {
		if (!detail) return null;
		if (detail.reader === 'reflowable' || detail.modality === 'reflowable') {
			return 'reflowable';
		}
		return detail.reading_mode === 'vertical' ? 'vertical' : 'paged';
	});
	const openContents = $derived($page.url.searchParams.get('contents') === '1');

	async function load(myId) {
		loading = true;
		error = null;
		detail = null;
		try {
			const d = await itemsApi.detail(myId);
			if (myId !== id) return;
			detail = d;
		} catch (e) {
			if (myId !== id) return;
			if (!(e instanceof ApiError && e.status === 401)) error = e.message ?? String(e);
		} finally {
			if (myId === id) loading = false;
		}
	}

	$effect(() => {
		id;
		load(id);
	});
</script>

{#if loading}
	<div class="boot"><div class="spinner"></div></div>
{:else if error}
	<div class="boot"><div class="errbox">Couldn't open this item: {error}</div></div>
{:else if engine === 'reflowable'}
	<ReflowableReader {id} {detail} {openContents} />
{:else if engine === 'vertical'}
	<VerticalReader {id} {detail} />
{:else if detail}
	<PagedReader {id} {detail} />
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
