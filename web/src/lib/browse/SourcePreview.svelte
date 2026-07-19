<script>
	import { goto } from '$app/navigation';
	import { untrack } from 'svelte';
	import {
		plugins as pluginsApi,
		downloads as downloadsApi,
		jobs as jobsApi,
		library as libraryApi,
		items as itemsApi,
		media,
		ApiError,
	} from '$lib/api.js';
	import { loadCover } from '$lib/browse/coverwarm.js';
	import { refreshKinds, kindLabel, kindHref } from '$lib/kinds.js';
	import { currentUser } from '$lib/session.js';
	import { ensureTagCounts } from '$lib/tagstats.js';
	import PreviewShell from '$lib/components/PreviewShell.svelte';
	import ReadingModeToggle from '$lib/components/ReadingModeToggle.svelte';
	import TagRows from '$lib/components/TagRows.svelte';
	import PreviewTitleMetadata from '$lib/components/PreviewTitleMetadata.svelte';
	import Description from '$lib/components/Description.svelte';
	import CoverThumbnail from '$lib/components/CoverThumbnail.svelte';
	import TypographicCover from '$lib/components/TypographicCover.svelte';
	import FavCount from '$lib/components/FavCount.svelte';
	import PageThumbGrid from '$lib/components/PageThumbGrid.svelte';
	import ChapterGrid from '$lib/components/ChapterGrid.svelte';
	import Pagination from '$lib/components/ui/Pagination.svelte';
	import Comments from '$lib/components/Comments.svelte';
	import { rowAlignedPageSize } from '$lib/grid.js';
	import { beginDownload, finishDownload } from '$lib/downloads.js';
	import { readingTimeLabel } from '$lib/format.js';

	let { plugin, reference, kind, back = '' } = $props();

	const isAdmin = $derived($currentUser?.role === 'admin');
	const backOverride = $derived(back || null);

	let meta = $state(null);
	let pages = $state([]);
	let loading = $state(true);
	let error = $state(null);
	let downloading = $state(false);

	const coverUrl = $derived(meta?.cover_url || pages[0]?.thumb_url || '');
	const pageCount = $derived(meta?.page_count ?? (pages.length || null));
	const favorites = $derived(meta?.favorites ?? null);
	const titleTags = $derived.by(() => {
		const tags = meta?.tags ?? [];
		const language = meta?.language?.trim();
		if (!language || tags.some((tag) => tag.namespace === 'language')) return tags;
		return [...tags, { namespace: 'language', value: language, qualifier: 'none' }];
	});
	const titleCount = $derived(pageCount ? `${pageCount} pages` : '');
	const titleReadingTime = $derived(pageCount ? readingTimeLabel(pageCount / 3) : '');
	const proxiedCover = $derived(coverUrl ? media.pluginImage(plugin, coverUrl) : '');
	const coverAuthor = $derived(meta?.tags?.find((t) => t.namespace === 'creator')?.value ?? '');

	const canRead = $derived(meta?.capabilities?.includes('read') ?? false);
	const canDownload = $derived(meta?.capabilities?.includes('download') ?? false);

	let readMode = $state('paged');
	const modeKey = $derived(`arca:src-read-mode:${plugin}`);
	function initReadMode() {
		let stored = null;
		try {
			stored = localStorage.getItem(modeKey);
		} catch {
			/* ignored */
		}
		readMode =
			stored === 'paged' || stored === 'vertical' ? stored : (meta?.reading_mode ?? 'paged');
	}
	function setReadMode(m) {
		readMode = m;
		try {
			localStorage.setItem(modeKey, m);
		} catch {
			/* ignored */
		}
	}
	const canPickMode = $derived(canRead && (pages.length > 0 || (meta?.chapters?.length ?? 0) > 0));

	$effect(() => {
		plugin;
		reference;
		load();
	});
	async function load() {
		loading = true;
		error = null;
		meta = null;
		pages = [];
		downloadedId = null;
		try {
			const [d] = await Promise.all([pluginsApi.item(plugin, reference), ensureTagCounts()]);
			meta = d;
			initReadMode();
			if (d?.capabilities?.includes('read') && !d?.chapters?.length) loadPages();
		} catch (e) {
			if (!(e instanceof ApiError && e.status === 401)) error = e?.message ?? String(e);
		} finally {
			loading = false;
		}
	}

	let downloadedId = $state(null);
	async function download() {
		if (!isAdmin) {
			error = 'Adding to the library requires an admin account.';
			return;
		}
		if (downloading) return;
		downloading = true;
		error = null;
		const key = `srcdl-${plugin}-${reference}`;
		beginDownload(key, meta?.title ?? String(reference), 'Adding to library…');
		try {
			const r = await downloadsApi.create(plugin, { ref: reference, kind, wait: true });
			let st = r?.state ?? null;
			let result = r?.result ?? null;
			const jobId = r?.job_id;
			const deadline = Date.now() + 10 * 60 * 1000;
			while (st !== 'done' && st !== 'failed' && jobId != null && Date.now() < deadline) {
				const j = await jobsApi.get(jobId, { wait: true }).catch(() => null);
				if (j) {
					st = j.state ?? st;
					result = j.result ?? result;
				} else {
					await new Promise((res) => setTimeout(res, 1500));
				}
			}
			if (st === 'failed') {
				finishDownload(key, false, result?.error ?? 'Download failed');
				return;
			}
			if (st !== 'done') {
				finishDownload(key, false, 'Still downloading on the server — check back soon');
				return;
			}
			refreshKinds();
			const id = result?.id;
			if (id != null) {
				downloadedId = id;
				finishDownload(key, true, 'Tap to open in library', { href: `/item/${id}` });
			} else {
				finishDownload(key, true, 'Added to library');
			}
		} catch (e) {
			finishDownload(key, false, e?.message ?? String(e));
		} finally {
			downloading = false;
		}
	}

	const hasPages = $derived(pages.length > 0);
	async function loadPages() {
		pages = [];
		thumbPage = 1;
		try {
			const d = await pluginsApi.pages(plugin, reference);
			pages = d.pages ?? [];
		} catch {
			/* ignored */
		}
	}
	function read(n = 0) {
		const q = new URLSearchParams({ page: String(n + 1), mode: readMode });
		if (kind) q.set('kind', kind);
		goto(`/source/${encodeURIComponent(plugin)}/${encodeURIComponent(reference)}/read?${q}`, {
			state: { title: meta?.title ?? '' },
		});
	}
	function readChapter(c) {
		if (!c?.reference) return;
		const q = new URLSearchParams({ page: '1', back: reference, mode: readMode });
		if (kind) q.set('kind', kind);
		const chLabel = c.title ?? (c.number ? `Ch. ${c.number}` : 'Chapter');
		goto(`/source/${encodeURIComponent(plugin)}/${encodeURIComponent(c.reference)}/read?${q}`, {
			state: { title: `${meta?.title ?? ''} · ${chLabel}`.trim() },
		});
	}

	let ownedMatch = $state(null);
	let matchDone = $state(false);
	let matchToken = 0;
	async function runMatch() {
		ownedMatch = null;
		matchDone = false;
		const token = ++matchToken;
		try {
			if (!meta) return;
			if (proxiedCover) await loadCover(proxiedCover);
			if (token !== matchToken) return;
			let verdict;
			try {
				[verdict] = await libraryApi.match([
					{
						source_url: meta.source_url,
						cover_url: coverUrl,
						page_count: meta.page_count ?? pageCount,
					},
				]);
			} catch {
				return;
			}
			if (token !== matchToken) return;
			const id = verdict?.owned_item_id ?? verdict?.likely_item_id;
			if (id == null) return;
			const d = await itemsApi.detail(id).catch(() => null);
			if (token !== matchToken || !d) return;
			ownedMatch = {
				id,
				name: d.name,
				cover_version: d.version,
				exact: verdict.owned_item_id != null,
			};
		} finally {
			if (token === matchToken) matchDone = true;
		}
	}
	$effect.pre(() => {
		meta;
		proxiedCover;
		untrack(() => runMatch());
	});
	const showLoading = $derived(loading || (meta != null && !error && !matchDone));

	const backTarget = $derived(
		kind ? `${kindHref(kind)}/browse?src=${encodeURIComponent(plugin)}` : '/',
	);
	const backLabel = $derived(kind ? kindLabel(kind) : 'Browse');

	const THUMB_TARGET = 30;
	let thumbCols = $state(6);
	const THUMBS_PER_PAGE = $derived(rowAlignedPageSize(thumbCols, THUMB_TARGET, 3, 5));
	let thumbPage = $state(1);
	const thumbPageCount = $derived(Math.max(1, Math.ceil(pages.length / THUMBS_PER_PAGE)));
	const thumbStart = $derived((thumbPage - 1) * THUMBS_PER_PAGE);
	const thumbEnd = $derived(Math.min(thumbStart + THUMBS_PER_PAGE, pages.length));
	$effect(() => {
		if (thumbPage > thumbPageCount) thumbPage = thumbPageCount;
	});
	function goThumbPage(p) {
		thumbPage = p;
	}
	const pageCells = $derived(
		pages.slice(thumbStart, thumbEnd).map((pg) => ({
			key: pg.number,
			src: media.pluginImage(plugin, pg.thumb_url),
			label: pg.number,
		})),
	);
</script>

<PreviewShell
	title={meta?.title ?? ''}
	artwork={proxiedCover}
	loading={showLoading}
	{error}
	ready={meta != null && matchDone}
	{backOverride}
	defaultBackTarget={backTarget}
	defaultBackLabel={backLabel}
>
	{#snippet banner()}
		{#if !ownedMatch?.exact}
			<div class="notice">
				<svg
					class="ico"
					viewBox="0 0 24 24"
					fill="none"
					stroke="currentColor"
					stroke-width="2"
					stroke-linecap="round"
					stroke-linejoin="round"
				>
					<circle cx="12" cy="12" r="10" />
					<path d="M12 16v-4M12 8h.01" />
				</svg>
				<div>
					<strong>Not in your library</strong>
					<p>
						You're viewing this directly from {plugin}. Favoriting, rating, editing metadata, and
						saving offline need the archive on your server first.
					</p>
				</div>
			</div>
		{/if}

		{#if ownedMatch}
			<div class="ownmatch">
				{#if ownedMatch.exact}
					<p class="omhead">Same archive in your library:</p>
				{:else}
					<p class="omhead likely">
						<svg
							viewBox="0 0 24 24"
							fill="none"
							stroke="currentColor"
							stroke-width="2"
							stroke-linecap="round"
							stroke-linejoin="round"
						>
							<path d="M12 2 2 7l10 5 10-5-10-5zM2 17l10 5 10-5M2 12l10 5 10-5" />
						</svg>
						Looks like one you may already have:
					</p>
				{/if}
				<a class="omcard" class:owned={ownedMatch.exact} href={`/item/${ownedMatch.id}`}>
					<span class="omcover">
						<CoverThumbnail
							src={media.thumbnail(ownedMatch.id, ownedMatch.cover_version)}
							alt={ownedMatch.name}
						/>
					</span>
					<span class="ominfo">
						<span class="omtitle">{ownedMatch.name}</span>
						<span class="omopen">Open in your library →</span>
					</span>
				</a>
			</div>
		{/if}
	{/snippet}

	{#snippet aside()}
		<CoverThumbnail src={proxiedCover} alt={meta.title} eager>
			{#snippet fallback()}
				<TypographicCover title={meta.title} author={coverAuthor} />
			{/snippet}
		</CoverThumbnail>

		{#if hasPages}
			<button class="read" onclick={() => read(0)}>
				<svg
					viewBox="0 0 24 24"
					fill="none"
					stroke="currentColor"
					stroke-width="2"
					stroke-linecap="round"
					stroke-linejoin="round"
				>
					<path
						d="M2 6s2-1 5-1 5 1 5 1v13s-2-1-5-1-5 1-5 1zM12 6s2-1 5-1 5 1 5 1v13s-2-1-5-1-5 1-5 1z"
					/>
				</svg>
				Read
			</button>
		{:else if meta.chapters?.[0]?.reference}
			<button class="read" onclick={() => readChapter(meta.chapters[0])}>
				<svg
					viewBox="0 0 24 24"
					fill="none"
					stroke="currentColor"
					stroke-width="2"
					stroke-linecap="round"
					stroke-linejoin="round"
				>
					<path
						d="M2 6s2-1 5-1 5 1 5 1v13s-2-1-5-1-5 1-5 1zM12 6s2-1 5-1 5 1 5 1v13s-2-1-5-1-5 1-5 1z"
					/>
				</svg>
				Read from beginning
			</button>
		{/if}

		{#if canPickMode}
			<ReadingModeToggle mode={readMode} onchange={setReadMode} />
		{/if}

		{#if downloadedId != null || ownedMatch?.exact}
			<a
				class="dl openlib"
				class:primary={!hasPages && !meta.chapters?.length}
				href={`/item/${downloadedId ?? ownedMatch.id}`}
			>
				<svg
					viewBox="0 0 24 24"
					fill="none"
					stroke="currentColor"
					stroke-width="2"
					stroke-linecap="round"
					stroke-linejoin="round"
				>
					<path d="M6.5 2H20v20H6.5A2.5 2.5 0 0 1 4 19.5v-15A2.5 2.5 0 0 1 6.5 2z" />
					<path d="M4 19.5A2.5 2.5 0 0 1 6.5 17H20" />
				</svg>
				Open in library
			</a>
		{:else if isAdmin && canDownload}
			<button
				class="dl"
				class:primary={!hasPages && !meta.chapters?.length}
				onclick={download}
				disabled={downloading}
			>
				<svg
					viewBox="0 0 24 24"
					fill="none"
					stroke="currentColor"
					stroke-width="2"
					stroke-linecap="round"
					stroke-linejoin="round"
				>
					<path d="M12 3v12M7 10l5 5 5-5M5 21h14" />
				</svg>
				{downloading ? 'Downloading…' : 'Download to library'}
			</button>
		{/if}

		{#if meta.source_url}
			<a class="ext" href={meta.source_url} target="_blank" rel="noopener noreferrer">
				<svg
					viewBox="0 0 24 24"
					fill="none"
					stroke="currentColor"
					stroke-width="2"
					stroke-linecap="round"
					stroke-linejoin="round"
				>
					<path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6" />
					<path d="M15 3h6v6M10 14 21 3" />
				</svg>
				Open in browser
			</a>
		{/if}
	{/snippet}

	{#snippet detail()}
		<div class="titlecopy">
			<h1 class="title preview-title">{meta.title}</h1>
			<PreviewTitleMetadata
				tags={titleTags}
				{kind}
				primaryCount={titleCount}
				readingTime={titleReadingTime}
			/>
		</div>
		<div class="badges">
			{#if favorites != null}<span class="badge"><FavCount count={favorites} /></span>{/if}
			<span class="badge muted">{plugin} #{reference}</span>
		</div>

		<Description text={meta.description} />

		<TagRows tags={meta.tags ?? []} {kind} hiddenNamespaces={['creator', 'language']} />

		{#if meta.chapters?.length}
			<ChapterGrid
				chapters={meta.chapters}
				thumbSrc={() => proxiedCover}
				showProgress={false}
				onopen={(_, c) => readChapter(c)}
			/>
		{/if}

		{#if hasPages}
			<div class="pageshead">
				<h2 class="section">Pages</h2>
				{#if thumbPageCount > 1}
					<Pagination simple page={thumbPage} pageCount={thumbPageCount} onnavigate={goThumbPage} />
				{/if}
			</div>
			<PageThumbGrid
				pages={pageCells}
				oncols={(n) => (thumbCols = n)}
				onopen={(p) => read(p.key - 1)}
			/>
		{/if}

		{#if meta.comments?.length}
			<div class="commentswrap">
				<Comments comments={meta.comments} />
			</div>
		{/if}
	{/snippet}
</PreviewShell>

<style>
	.notice {
		display: flex;
		align-items: flex-start;
		gap: var(--space-3);
		margin-bottom: var(--space-5);
		padding: var(--space-4) var(--space-5);
		border: 1px solid color-mix(in srgb, #e8b923 45%, var(--border));
		border-radius: var(--radius);
		background: color-mix(in srgb, #e8b923 8%, var(--surface));
	}
	.notice .ico {
		flex: 0 0 auto;
		width: 1.15rem;
		height: 1.15rem;
		margin-top: 0.1rem;
		color: #e8b923;
	}
	.notice strong {
		display: block;
		font-size: 0.92rem;
		margin-bottom: 0.15rem;
	}
	.notice p {
		margin: 0;
		font-size: 0.85rem;
		color: var(--muted);
		line-height: 1.5;
	}

	.read,
	.dl {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		gap: var(--space-2);
		padding: var(--space-3);
	}
	a.dl {
		border: 1px solid var(--border);
		border-radius: var(--radius);
		color: var(--text);
		font-size: 0.95rem;
	}
	a.dl:hover {
		border-color: var(--accent);
	}
	.read,
	.dl.primary {
		background: var(--accent);
		border-color: var(--accent);
		color: #fff;
		font-weight: 600;
	}
	.read:hover,
	.dl.primary:hover:not(:disabled) {
		filter: brightness(1.1);
		border-color: var(--accent);
	}
	.dl:hover:not(:disabled) {
		border-color: var(--accent);
	}
	.dl:disabled {
		opacity: 0.6;
		cursor: not-allowed;
	}
	.read svg,
	.dl svg {
		width: 1rem;
		height: 1rem;
	}

	.pageshead {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--space-3);
		margin: var(--space-5) 0 var(--space-3);
		padding-top: var(--space-3);
		border-top: 1px solid var(--border);
	}
	.section {
		margin: 0;
		font-size: 0.9rem;
		font-weight: 600;
	}
	.commentswrap {
		margin-top: var(--space-6);
		padding-top: var(--space-5);
		border-top: 1px solid var(--border);
	}
	.ext {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		gap: var(--space-2);
		padding: var(--space-3);
		border: 1px solid var(--border);
		border-radius: var(--radius-sm);
		color: var(--muted);
		font-size: 0.9rem;
	}
	.ext:hover {
		border-color: var(--accent);
		color: var(--text);
	}
	.ext svg {
		width: 1rem;
		height: 1rem;
	}

	.titlecopy {
		margin-bottom: var(--space-3);
	}
	.badges {
		display: flex;
		flex-wrap: wrap;
		gap: var(--space-2);
		margin-bottom: var(--space-5);
	}
	.badge {
		display: inline-flex;
		align-items: center;
		gap: 0.3rem;
		padding: 0.15rem 0.55rem;
		border: 1px solid var(--border);
		border-radius: var(--radius-sm);
		background: var(--surface);
		font-size: 0.75rem;
		color: var(--text);
		font-variant-numeric: tabular-nums;
	}
	.badge.muted {
		color: var(--muted);
	}

	.ownmatch {
		margin-bottom: var(--space-5);
	}
	.omhead {
		display: flex;
		align-items: center;
		gap: 0.4rem;
		margin-bottom: var(--space-2);
		font-size: 0.78rem;
		color: #6ee7b7;
	}
	.omhead.likely {
		color: #7dd3fc;
	}
	.omhead svg {
		width: 0.9rem;
		height: 0.9rem;
	}
	.omcard {
		display: flex;
		align-items: center;
		gap: var(--space-3);
		padding: var(--space-3);
		border: 1px solid var(--border);
		border-radius: var(--radius);
		background: var(--surface);
		min-width: 0;
		transition: border-color var(--ease);
	}
	.omcard:hover {
		border-color: var(--accent);
	}
	.omcard.owned:hover {
		border-color: #34d399;
	}
	.omcard:not(.owned) {
		border-color: rgba(14, 165, 233, 0.55);
	}
	.omcard:not(.owned):hover {
		border-color: #38bdf8;
	}
	.omcard:not(.owned) .omopen {
		color: #38bdf8;
	}
	.omcover {
		flex: 0 0 auto;
		width: 3.5rem;
	}
	.ominfo {
		min-width: 0;
		display: flex;
		flex-direction: column;
		gap: 0.25rem;
	}
	.omtitle {
		font-size: 0.9rem;
		color: var(--text);
		display: -webkit-box;
		-webkit-line-clamp: 2;
		-webkit-box-orient: vertical;
		overflow: hidden;
	}
	.omopen {
		font-size: 0.72rem;
		font-weight: 500;
		color: var(--accent);
		transition: transform var(--ease);
	}
	.omcard:hover .omopen {
		transform: translateX(2px);
	}
</style>
