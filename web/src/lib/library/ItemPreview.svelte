<script>
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import { items as itemsApi, media, ApiError } from '$lib/api.js';
	import { refreshTagCounts } from '$lib/tagstats.js';
	import { currentUser, resolveGuestSession } from '$lib/session.js';
	import { kindHref, kindLabel, refreshKinds } from '$lib/kinds.js';
	import CoverThumbnail from '$lib/components/CoverThumbnail.svelte';
	import StarRating from '$lib/components/ui/StarRating.svelte';
	import PreviewShell from '$lib/components/PreviewShell.svelte';
	import ReadingModeToggle from '$lib/components/ReadingModeToggle.svelte';
	import TagRows from '$lib/components/TagRows.svelte';
	import Description from '$lib/components/Description.svelte';
	import Pagination from '$lib/components/ui/Pagination.svelte';
	import MetadataModal from '$lib/components/MetadataModal.svelte';
	import EditMetadataModal from '$lib/components/EditMetadataModal.svelte';
	import ProgressMeter from '$lib/components/ui/ProgressMeter.svelte';
	import DeleteConfirm from '$lib/components/ui/DeleteConfirm.svelte';
	import IdentifyModal from '$lib/components/IdentifyModal.svelte';
	import Comments from '$lib/components/Comments.svelte';
	import PageThumbGrid from '$lib/components/PageThumbGrid.svelte';
	import ChapterGrid from '$lib/components/ChapterGrid.svelte';
	import ReflowableOverview from '$lib/components/ReflowableOverview.svelte';
	import PreviewTitleMetadata from '$lib/components/PreviewTitleMetadata.svelte';
	import CogMenu from '$lib/library/CogMenu.svelte';
	import SimilarShelf from '$lib/library/SimilarShelf.svelte';
	import { rowAlignedPageSize } from '$lib/grid.js';
	import { beginDownload, finishDownload } from '$lib/downloads.js';
	import {
		WORDS_PER_MINUTE,
		readingTimeLabel as readingLabel,
		compactWords as compactNum,
		fileSizeLabel,
		formatAdded,
	} from '$lib/format.js';
	import { loadSimilarCards } from '$lib/cards.js';

	let { id } = $props();

	let meta = $state(null);
	let loading = $state(true);
	let error = $state(null);
	let favBusy = $state(false);
	const isAdmin = $derived($currentUser?.role === 'admin');
	const guest = $derived($currentUser?.role === 'guest');
	let showMeta = $state(false);
	let showDelete = $state(false);
	let deleting = $state(false);
	let showEdit = $state(false);
	let showIdentify = $state(false);
	function onEdited(fresh) {
		if (fresh) meta = fresh;
		refreshTagCounts();
		loadSimilar(id);
	}

	const THUMB_TARGET = 30;
	let thumbCols = $state(6);
	const THUMBS_PER_PAGE = $derived(rowAlignedPageSize(thumbCols, THUMB_TARGET, 3, 5));
	let thumbPage = $state(1);
	const thumbPageCount = $derived(Math.max(1, Math.ceil(pageCount / THUMBS_PER_PAGE)));
	const thumbStart = $derived((thumbPage - 1) * THUMBS_PER_PAGE);
	const thumbEnd = $derived(Math.min(thumbStart + THUMBS_PER_PAGE, pageCount));
	$effect(() => {
		if (thumbPage > thumbPageCount) thumbPage = thumbPageCount;
	});

	function goThumbPage(p) {
		thumbPage = p;
	}

	const displayTitle = $derived(
		meta?.series?.number_disp ? `${meta.name} ${meta.series.number_disp}` : (meta?.name ?? ''),
	);

	const pageCount = $derived(meta?.page_count ?? 0);
	const chapterCount = $derived(meta?.chapters?.filter((c) => c.number).length ?? 0);
	const PAGES_PER_MINUTE = 3;
	const wordCount = $derived(meta?.word_count ?? 0);
	const readingMinutes = $derived(
		wordCount ? wordCount / WORDS_PER_MINUTE : pageCount ? pageCount / PAGES_PER_MINUTE : 0,
	);
	const showThumbs = $derived(meta?.modality !== 'reflowable');
	const completed = $derived(
		meta && meta.progress != null && pageCount > 0 && meta.progress + 1 >= pageCount,
	);
	const started = $derived(!!meta && (meta.progress != null || meta.progress_locator != null));
	const reflowFraction = $derived(
		meta?.modality === 'reflowable' ? Math.min(Math.max(meta.progress_value ?? 0, 0), 1) : 0,
	);
	const reflowPercent = $derived(Math.round(reflowFraction * 100));
	const reflowDone = $derived(meta?.modality === 'reflowable' && reflowFraction >= 0.98);
	const reflowPositionLabel = $derived.by(() => {
		if (meta?.progress_locator?.toc?.title) return meta.progress_locator.toc.title;
		if (meta?.progress_locator?.chapter != null)
			return `Section ${Number(meta.progress_locator.chapter) + 1}`;
		return 'Progress';
	});
	const titleCount = $derived(
		meta?.modality === 'reflowable'
			? wordCount
				? `${compactNum(wordCount)} words`
				: ''
			: pageCount
				? `${pageCount} pages${chapterCount ? ` · ${chapterCount} chapters` : ''}`
				: '',
	);
	const titleReadingTime = $derived(readingMinutes ? readingLabel(readingMinutes) : '');
	const downloadDetails = $derived(
		[meta?.format, fileSizeLabel(meta?.size_bytes)].filter(Boolean).join(' · '),
	);

	$effect(() => {
		loadDetail(id);
		loadSimilar(id);
	});

	let similar = $state([]);
	async function loadSimilar(itemId) {
		similar = [];
		try {
			similar = await loadSimilarCards(itemsApi.similar, itemId);
		} catch {
			/* ignored */
		}
	}
	async function loadDetail(itemId) {
		loading = true;
		error = null;
		meta = null;
		thumbPage = 1;
		try {
			meta = await itemsApi.detail(itemId);
		} catch (e) {
			if (e instanceof ApiError && e.status === 401) return;
			error = e.message ?? String(e);
		} finally {
			loading = false;
		}
	}

	function onMetaUpdated(fresh) {
		if (fresh) meta = fresh;
		refreshTagCounts();
		loadSimilar(id);
	}

	async function toggleFavorite() {
		if (!meta || favBusy) return;
		favBusy = true;
		const want = !meta.favorited;
		try {
			await (want ? itemsApi.favorite(meta.id) : itemsApi.unfavorite(meta.id));
			meta = { ...meta, favorited: want };
		} catch (e) {
			error = e.message ?? String(e);
		} finally {
			favBusy = false;
		}
	}

	let ratingBusy = $state(false);
	async function applyRating(next) {
		if (!meta || ratingBusy) return;
		const prev = meta.rating ?? null;
		meta = { ...meta, rating: next };
		ratingBusy = true;
		try {
			await (next == null ? itemsApi.clearRating(meta.id) : itemsApi.setRating(meta.id, next));
		} catch (e) {
			meta = { ...meta, rating: prev };
			error = e.message ?? String(e);
		} finally {
			ratingBusy = false;
		}
	}

	async function confirmDelete() {
		if (!meta || deleting) return;
		deleting = true;
		try {
			const goneKind = meta.kind;
			await itemsApi.remove(meta.id);
			refreshKinds();
			goto(goneKind ? kindHref(goneKind) : '/');
		} catch (e) {
			error = e.message ?? String(e);
			deleting = false;
			showDelete = false;
		}
	}

	function read(startPage) {
		const q = startPage != null ? `?page=${startPage + 1}` : '';
		goto(`/reader/${id}${q}`);
	}

	const tauriInvoke = globalThis.__TAURI__?.core?.invoke;
	const clientDownloadsDisabled = !!tauriInvoke;
	let downloaded = $state(false);
	let dlBusy = $state(false);
	$effect(() => {
		downloaded = false;
		if (tauriInvoke && meta?.id != null) {
			tauriInvoke('item_downloaded', { id: meta.id })
				.then((v) => (downloaded = !!v))
				.catch(() => {});
		}
	});
	async function downloadOffline() {
		if (!meta || dlBusy || clientDownloadsDisabled) return;
		const key = `item-${meta.id}`;
		if (tauriInvoke) {
			if (downloaded) {
				dlBusy = true;
				try {
					await tauriInvoke('remove_download', { id: meta.id });
					downloaded = false;
				} catch {
					/* ignored */
				} finally {
					dlBusy = false;
				}
				return;
			}
			dlBusy = true;
			beginDownload(key, meta.name);
			try {
				await tauriInvoke('download_item', { id: meta.id, name: meta.name });
				downloaded = true;
				finishDownload(key, true);
			} catch (e) {
				finishDownload(key, false, typeof e === 'string' ? e : (e?.message ?? 'Download failed'));
			} finally {
				dlBusy = false;
			}
		} else {
			const a = document.createElement('a');
			a.href = media.download(meta.id);
			a.download = '';
			document.body.appendChild(a);
			a.click();
			a.remove();
		}
	}

	let modeBusy = $state(false);
	async function applyReadingMode(mode) {
		if (!meta || modeBusy || (meta.reading_mode ?? 'paged') === mode) return;
		const prev = meta.reading_mode ?? 'paged';
		meta = { ...meta, reading_mode: mode };
		modeBusy = true;
		try {
			await itemsApi.setReadingMode(meta.id, mode);
		} catch {
			meta = { ...meta, reading_mode: prev };
		} finally {
			modeBusy = false;
		}
	}

	const sources = $derived(meta?.sources ?? []);
	const beforeRows = $derived(
		meta?.series
			? [
					{
						label: 'Series',
						chips: [
							{
								text: meta.series.title,
								href: `/series/${meta.series.id}`,
							},
						],
					},
				]
			: [],
	);
	const afterRows = $derived([
		...(sources.length
			? [
					{
						label: 'Source',
						chips: sources.map((s) => ({ text: s.url, href: s.url, external: true, wrap: true })),
					},
				]
			: []),
		...(formatAdded(meta?.added_at)
			? [{ label: 'Added', chips: [{ text: formatAdded(meta.added_at) }] }]
			: []),
	]);

	const defaultBackTarget = $derived(
		meta?.series?.id != null ? `/series/${meta.series.id}` : meta?.kind ? kindHref(meta.kind) : '/',
	);
	const defaultBackLabel = $derived(
		meta?.series?.id != null
			? (meta.series.title ?? 'Series')
			: meta?.kind
				? kindLabel(meta.kind)
				: 'Library',
	);
	onMount(resolveGuestSession);
</script>

<PreviewShell
	title={displayTitle}
	artwork={meta ? media.thumbnail(meta.id, meta.version) : ''}
	{loading}
	{error}
	ready={meta != null}
	{defaultBackTarget}
	{defaultBackLabel}
	escapeEnabled={!showMeta && !showDelete && !showEdit && !showIdentify}
>
	{#snippet topActions()}
		{#if isAdmin && meta}
			<CogMenu label="Item actions">
				{#snippet children(close)}
					<button
						class="mitem"
						type="button"
						role="menuitem"
						onclick={() => {
							close();
							showEdit = true;
						}}
					>
						<svg
							viewBox="0 0 24 24"
							fill="none"
							stroke="currentColor"
							stroke-width="2"
							stroke-linecap="round"
							stroke-linejoin="round"
							><path d="M12 20h9" /><path d="M16.5 3.5a2.12 2.12 0 0 1 3 3L7 19l-4 1 1-4Z" /></svg
						>
						Edit metadata…
					</button>
					{#if showThumbs}
						<button
							class="mitem"
							type="button"
							role="menuitem"
							onclick={() => {
								close();
								showIdentify = true;
							}}
						>
							<svg
								viewBox="0 0 24 24"
								fill="none"
								stroke="currentColor"
								stroke-width="2"
								stroke-linecap="round"
								stroke-linejoin="round"
								><circle cx="11" cy="11" r="7" /><path d="m21 21-4.3-4.3" /></svg
							>
							Identify…
						</button>
					{/if}
					<button
						class="mitem mitem-danger"
						type="button"
						role="menuitem"
						onclick={() => {
							close();
							showDelete = true;
						}}
					>
						<svg
							viewBox="0 0 24 24"
							fill="none"
							stroke="currentColor"
							stroke-width="2"
							stroke-linecap="round"
							stroke-linejoin="round"
							><path d="M3 6h18" /><path
								d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"
							/><path d="M10 11v6M14 11v6" /></svg
						>
						Delete from server…
					</button>
				{/snippet}
			</CogMenu>
		{/if}
	{/snippet}
	{#snippet readingMode()}
		{#if meta.modality !== 'reflowable' && !guest}
			<ReadingModeToggle
				mode={meta.reading_mode ?? 'paged'}
				disabled={modeBusy}
				onchange={applyReadingMode}
			/>
		{/if}
	{/snippet}
	{#snippet itemProgress()}
		{#if !guest && meta.modality === 'reflowable'}
			<ProgressMeter
				label={reflowPositionLabel}
				doneLabel="Completed"
				done={reflowDone}
				fraction={reflowFraction}
				valueLabel={`${reflowPercent}%`}
			/>
		{:else if !guest && pageCount}
			<ProgressMeter
				label="Progress"
				doneLabel="Completed"
				done={completed}
				current={(meta.progress ?? -1) + 1}
				total={pageCount}
			/>
		{/if}
	{/snippet}
	{#snippet aside()}
		<div class="mobilehero">
			<div class="mh-cover">
				<CoverThumbnail src={media.thumbnail(meta.id, meta.version)} alt={meta.name} eager />
			</div>
			<div class="mh-body">
				<h1 class="mh-title preview-title">{displayTitle}</h1>
				<PreviewTitleMetadata
					tags={meta.tags ?? []}
					kind={meta.kind}
					primaryCount={titleCount}
					readingTime={titleReadingTime}
					format={meta.format}
					publisher={meta.publisher}
					compact
				/>
				{#if !guest}
					<StarRating
						value={meta.rating ?? null}
						busy={ratingBusy}
						onset={applyRating}
						onclear={() => applyRating(null)}
						label={false}
					/>
				{/if}
				{@render itemProgress()}
				<button class="read" onclick={() => read(completed ? 0 : undefined)}>
					{completed ? 'Read again' : started ? 'Continue' : 'Read'}
				</button>
			</div>
		</div>
		<div class="mobileactions">
			{#if meta.modality !== 'reflowable' && !guest}
				<button
					class="mact mact-mode"
					type="button"
					disabled={modeBusy}
					onclick={() =>
						applyReadingMode((meta.reading_mode ?? 'paged') === 'vertical' ? 'paged' : 'vertical')}
					title="Reading mode — tap to switch"
				>
					<span>{(meta.reading_mode ?? 'paged') === 'vertical' ? 'Vertical' : 'Paged'}</span>
				</button>
			{/if}
			{#if !guest}
				<button
					class="mact mact-fav"
					class:on={meta.favorited}
					disabled={favBusy}
					onclick={toggleFavorite}
					title={meta.favorited ? 'Favorited' : 'Favorite'}
				>
					<svg
						viewBox="0 0 24 24"
						fill={meta.favorited ? 'currentColor' : 'none'}
						stroke="currentColor"
						stroke-width="2"
					>
						<path
							d="M20.84 4.61a5.5 5.5 0 0 0-7.78 0L12 5.67l-1.06-1.06a5.5 5.5 0 0 0-7.78 7.78L12 21.23l8.84-8.84a5.5 5.5 0 0 0 0-7.78z"
						/>
					</svg>
					<span class="mact-label">{meta.favorited ? 'Favorited' : 'Favorite'}</span>
				</button>
			{/if}
			{#if isAdmin}
				<button
					class="mact mact-meta"
					type="button"
					onclick={() => (showMeta = true)}
					title="Add metadata"
				>
					<svg
						viewBox="0 0 24 24"
						fill="none"
						stroke="currentColor"
						stroke-width="2"
						stroke-linecap="round"><path d="M12 5v14M5 12h14" /></svg
					>
					<span class="mact-label">Metadata</span>
				</button>
			{/if}
			{#if !clientDownloadsDisabled}
				<button
					class="mact"
					type="button"
					disabled={dlBusy}
					onclick={downloadOffline}
					title={tauriInvoke && downloaded
						? 'Remove the downloaded copy from this device'
						: `Download${downloadDetails ? ` · ${downloadDetails}` : ''}`}
				>
					{#if tauriInvoke && downloaded}
						<svg
							viewBox="0 0 24 24"
							fill="none"
							stroke="currentColor"
							stroke-width="2"
							stroke-linecap="round"
							stroke-linejoin="round"
						>
							<path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" />
							<path d="M9 8l6 6M15 8l-6 6" />
						</svg>
						<span class="mact-label">Remove</span>
					{:else}
						<svg
							viewBox="0 0 24 24"
							fill="none"
							stroke="currentColor"
							stroke-width="2"
							stroke-linecap="round"
							stroke-linejoin="round"
						>
							<path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" />
							<path d="M7 10l5 5 5-5" />
							<path d="M12 15V3" />
						</svg>
						<span class="mact-label">Download</span>
					{/if}
				</button>
			{/if}
		</div>

		<div class="desktopaside">
			<div class="desktopcover">
				<CoverThumbnail src={media.thumbnail(meta.id, meta.version)} alt={meta.name} eager />
			</div>

			{@render itemProgress()}

			<button class="read" onclick={() => read(completed ? 0 : undefined)}>
				{completed ? 'Read again' : started ? 'Continue' : 'Read'}
			</button>

			{@render readingMode()}

			{#if !guest}
				<button class="fav" class:on={meta.favorited} disabled={favBusy} onclick={toggleFavorite}>
					<svg
						viewBox="0 0 24 24"
						fill={meta.favorited ? 'currentColor' : 'none'}
						stroke="currentColor"
						stroke-width="2"
					>
						<path
							d="M20.84 4.61a5.5 5.5 0 0 0-7.78 0L12 5.67l-1.06-1.06a5.5 5.5 0 0 0-7.78 7.78L12 21.23l8.84-8.84a5.5 5.5 0 0 0 0-7.78z"
						/>
					</svg>
					{meta.favorited ? 'Favorited' : 'Favorite'}
				</button>

				<StarRating
					value={meta.rating ?? null}
					busy={ratingBusy}
					onset={applyRating}
					onclear={() => applyRating(null)}
				/>
			{/if}

			{#if !clientDownloadsDisabled}
				<button class="dlbtn" type="button" disabled={dlBusy} onclick={downloadOffline}>
					{#if tauriInvoke && downloaded}
						<span class="dlprimary">
							<svg
								viewBox="0 0 24 24"
								fill="none"
								stroke="currentColor"
								stroke-width="2"
								stroke-linecap="round"
								stroke-linejoin="round"
							>
								<path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" />
								<path d="M9 8l6 6M15 8l-6 6" />
							</svg>
							Remove download
						</span>
					{:else}
						<span class="dlprimary">
							<svg
								viewBox="0 0 24 24"
								fill="none"
								stroke="currentColor"
								stroke-width="2"
								stroke-linecap="round"
								stroke-linejoin="round"
							>
								<path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" />
								<path d="M7 10l5 5 5-5" />
								<path d="M12 15V3" />
							</svg>
							Download
						</span>
						{#if downloadDetails}<span class="dlmeta">{downloadDetails}</span>{/if}
					{/if}
				</button>
			{/if}
		</div>
	{/snippet}

	{#snippet detail()}
		<div class="titlerow">
			<div class="titlecopy mediatitle">
				<h1 class="preview-title">{displayTitle}</h1>
				<PreviewTitleMetadata
					tags={meta.tags ?? []}
					kind={meta.kind}
					primaryCount={titleCount}
					readingTime={titleReadingTime}
					format={meta.format}
					publisher={meta.publisher}
				/>
			</div>
			{#if isAdmin}
				<button class="addmeta" type="button" onclick={() => (showMeta = true)}>
					<svg
						viewBox="0 0 24 24"
						fill="none"
						stroke="currentColor"
						stroke-width="2"
						stroke-linecap="round"><path d="M12 5v14M5 12h14" /></svg
					>
					Add Metadata
				</button>
			{/if}
		</div>
		<Description text={meta.description} />

		<TagRows
			tags={meta.tags ?? []}
			kind={meta.kind}
			before={beforeRows}
			after={afterRows}
			hiddenNamespaces={['creator', 'language']}
		/>

		{#if pageCount && showThumbs}
			{#if meta.chapters?.length}
				<ChapterGrid
					chapters={meta.chapters}
					itemId={meta.id}
					version={meta.version}
					progress={meta.progress}
					onopen={(startPage) => read(startPage)}
				/>
			{:else}
				<div class="pageshead">
					<h2 class="section">Pages</h2>
					{#if thumbPageCount > 1}
						<Pagination
							simple
							page={thumbPage}
							pageCount={thumbPageCount}
							onnavigate={goThumbPage}
						/>
					{/if}
				</div>
				<PageThumbGrid
					pages={Array.from({ length: thumbEnd - thumbStart }, (_, i) => {
						const n = thumbStart + i;
						return { key: n, src: media.pageThumbnail(meta.id, n, meta.version), label: n + 1 };
					})}
					oncols={(n) => (thumbCols = n)}
					onopen={(p) => read(p.key)}
				/>
			{/if}
		{:else if meta.modality === 'reflowable'}
			<ReflowableOverview
				itemId={meta.id}
				progressValue={meta.progress_value}
				locator={meta.progress_locator}
				lastReadAt={meta.last_read_at}
				{wordCount}
				tags={meta.tags ?? []}
			/>
		{/if}

		{#if similar.length}
			<div class="similar">
				<SimilarShelf items={similar} />
			</div>
		{/if}

		{#if meta.comments?.length}
			<div class="commentswrap">
				<Comments comments={meta.comments} itemId={meta.id} pageCount={meta.page_count} />
			</div>
		{/if}
	{/snippet}
</PreviewShell>

{#if showMeta && meta}
	<MetadataModal
		itemId={meta.id}
		kind={meta.kind}
		{pageCount}
		sources={meta.sources ?? []}
		onClose={() => (showMeta = false)}
		onUpdated={onMetaUpdated}
	/>
{/if}

{#if showEdit && meta}
	<EditMetadataModal {meta} onClose={() => (showEdit = false)} onUpdated={onEdited} />
{/if}

{#if showDelete && meta}
	<DeleteConfirm
		title={displayTitle}
		busy={deleting}
		onConfirm={confirmDelete}
		onClose={() => (showDelete = false)}
	/>
{/if}

{#if showIdentify && meta}
	<IdentifyModal
		itemId={meta.id}
		kind={meta.kind}
		{pageCount}
		onClose={() => (showIdentify = false)}
		onScraped={onEdited}
	/>
{/if}

<style>
	.mobilehero,
	.mobileactions {
		display: none;
	}
	.desktopaside {
		display: contents;
	}
	.desktopcover {
		width: 100%;
	}
	@media (max-width: 720px) {
		.pageshead,
		.similar,
		.commentswrap {
			margin-left: 0;
			padding-left: 0;
			margin-right: 0;
			padding-right: 0;
		}
	}
	.read {
		background: var(--accent);
		border-color: var(--accent);
		color: #fff;
		font-weight: 600;
		padding: var(--space-2) var(--space-3);
	}
	.read:hover {
		filter: brightness(1.1);
		border-color: var(--accent);
	}
	.fav {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		gap: var(--space-2);
		padding: var(--space-2) var(--space-3);
	}
	.dlbtn {
		display: flex;
		flex-direction: column;
		align-items: center;
		justify-content: center;
		gap: 0.2rem;
		padding: 0.6rem var(--space-3);
	}
	.dlprimary {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		gap: var(--space-2);
		font-weight: 600;
	}
	.dlmeta {
		color: var(--muted);
		font-size: 0.7rem;
		font-variant-numeric: tabular-nums;
	}
	.dlbtn svg {
		width: 1rem;
		height: 1rem;
	}
	.dlbtn:disabled {
		opacity: 0.6;
		cursor: default;
	}
	.fav svg {
		width: 1rem;
		height: 1rem;
	}
	.fav.on {
		border-color: #e0566f;
		background: rgba(224, 86, 111, 0.12);
		color: #e0566f;
	}

	.titlerow {
		display: flex;
		align-items: flex-start;
		justify-content: space-between;
		gap: var(--space-4);
		flex-wrap: nowrap;
	}
	.titlecopy {
		flex: 1 1 auto;
		min-width: 0;
	}
	.addmeta {
		flex: 0 0 auto;
		display: inline-flex;
		align-items: center;
		gap: var(--space-2);
		padding: var(--space-2) var(--space-3);
		font-size: 0.82rem;
		font-weight: 500;
		color: var(--text);
		background: var(--surface-2);
		border: 1px solid var(--border);
		border-radius: var(--radius-sm);
		cursor: pointer;
		transition:
			border-color 0.15s ease,
			background 0.15s ease,
			color 0.15s ease;
	}
	.addmeta:hover {
		border-color: var(--accent);
		color: var(--accent);
		background: color-mix(in srgb, var(--accent) 12%, var(--surface-2));
	}
	.addmeta svg {
		width: 0.9rem;
		height: 0.9rem;
	}
	.titlerow h1 {
		flex: 1 1 auto;
	}

	.pageshead {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--space-3);
		margin-bottom: var(--space-3);
		padding-top: var(--space-3);
		border-top: 1px solid var(--border);
		margin-left: calc(-1 * var(--space-6));
		padding-left: var(--space-6);
		margin-right: calc(-1 * var(--space-5));
		padding-right: var(--space-5);
	}
	.section {
		margin: 0;
		font-size: 0.9rem;
		font-weight: 600;
	}

	.similar,
	.commentswrap {
		margin-top: var(--space-6);
		padding-top: var(--space-5);
		border-top: 1px solid var(--border);
		margin-left: calc(-1 * var(--space-6));
		padding-left: var(--space-6);
		margin-right: calc(-1 * var(--space-5));
		padding-right: var(--space-5);
	}

	@media (max-width: 720px) {
		.desktopaside {
			display: none;
		}
		.titlerow {
			display: none;
		}
		.mobilehero {
			display: flex;
			align-items: flex-start;
			gap: var(--space-4);
		}
		.mh-cover {
			flex: 0 0 auto;
			width: clamp(110px, 33vw, 150px);
		}
		.mh-body {
			flex: 1 1 auto;
			min-width: 0;
			display: flex;
			flex-direction: column;
			gap: var(--space-3);
		}
		.read {
			padding-block: 0.4rem;
		}
		.mh-title {
			font-size: 1.15rem;
			line-height: 1.2;
		}
		.mh-body :global(.rating) {
			align-items: flex-start;
		}
		.mh-body :global(.star svg) {
			width: 1.2rem;
			height: 1.2rem;
		}
		.mobileactions {
			display: flex;
			flex-wrap: nowrap;
			gap: var(--space-2);
			margin: var(--space-2) calc(-1 * var(--space-5)) 0;
			padding: var(--space-2) var(--space-5);
			border-top: 1px solid var(--border);
			border-bottom: 1px solid var(--border);
		}
		.mact {
			flex: 1 1 0;
			min-width: 0;
			overflow: hidden;
			display: inline-flex;
			align-items: center;
			justify-content: center;
			gap: var(--space-2);
			padding: var(--space-1) var(--space-3);
			font-size: 0.85rem;
			white-space: nowrap;
		}
		.mact svg {
			width: 1rem;
			height: 1rem;
			flex: 0 0 auto;
		}
		.mact-fav.on {
			border-color: #e0566f;
			background: rgba(224, 86, 111, 0.12);
			color: #e0566f;
		}
	}
	@media (max-width: 720px) {
		.mobileactions .mact-label {
			display: none;
		}
		.mact-mode {
			color: var(--text);
			font-weight: 600;
		}
	}
</style>
