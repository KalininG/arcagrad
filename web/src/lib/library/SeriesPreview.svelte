<script>
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import { series as seriesApi, media, ApiError } from '$lib/api.js';
	import { currentUser, resolveGuestSession } from '$lib/session.js';
	import { kindHref, kindLabel } from '$lib/kinds.js';
	import { setSeriesHint } from '$lib/seriesHint.js';
	import PreviewShell from '$lib/components/PreviewShell.svelte';
	import ProgressMeter from '$lib/components/ui/ProgressMeter.svelte';
	import StarRating from '$lib/components/ui/StarRating.svelte';
	import TagRows from '$lib/components/TagRows.svelte';
	import PreviewTitleMetadata from '$lib/components/PreviewTitleMetadata.svelte';
	import Description from '$lib/components/Description.svelte';
	import CoverThumbnail from '$lib/components/CoverThumbnail.svelte';
	import MetadataModal from '$lib/components/MetadataModal.svelte';
	import EditMetadataModal from '$lib/components/EditMetadataModal.svelte';
	import TrackerModal from '$lib/components/TrackerModal.svelte';
	import CogMenu from '$lib/library/CogMenu.svelte';
	import SimilarShelf from '$lib/library/SimilarShelf.svelte';
	import { WORDS_PER_MINUTE, readingTimeLabel, compactWords, formatAdded } from '$lib/format.js';
	import { loadSimilarCards } from '$lib/cards.js';

	let { id } = $props();

	let meta = $state(null);
	let loading = $state(true);
	let error = $state(null);
	const isAdmin = $derived($currentUser?.role === 'admin');
	const guest = $derived($currentUser?.role === 'guest');
	let showMeta = $state(false);
	let showEdit = $state(false);
	let showTrackers = $state(false);

	let favBusy = $state(false);
	async function toggleFavorite() {
		if (!meta || favBusy) return;
		favBusy = true;
		const want = !meta.favorited;
		try {
			await (want ? seriesApi.favorite(meta.id) : seriesApi.unfavorite(meta.id));
			meta = { ...meta, favorited: want };
		} catch (e) {
			error = e.message ?? String(e);
		} finally {
			favBusy = false;
		}
	}

	let ratingBusy = $state(false);
	async function rateSeries(next) {
		if (!meta || ratingBusy) return;
		const prev = meta.rating ?? null;
		meta = { ...meta, rating: next };
		ratingBusy = true;
		try {
			await (next == null ? seriesApi.clearRating(meta.id) : seriesApi.setRating(meta.id, next));
		} catch (e) {
			meta = { ...meta, rating: prev };
			error = e.message ?? String(e);
		} finally {
			ratingBusy = false;
		}
	}

	function onMetaUpdated(fresh) {
		if (fresh) meta = fresh;
	}

	async function load() {
		loading = true;
		error = null;
		meta = null;
		try {
			const detail = await seriesApi.get(id);
			meta = detail;
			setSeriesHint(detail.id, detail.title);
		} catch (e) {
			if (!(e instanceof ApiError && e.status === 401)) error = e?.message ?? String(e);
		} finally {
			loading = false;
		}
	}
	let similar = $state([]);
	async function loadSimilar(sid) {
		similar = [];
		try {
			similar = await loadSimilarCards(seriesApi.similar, sid);
		} catch {
			similar = [];
		}
	}
	$effect(() => {
		id;
		load();
		loadSimilar(id);
	});

	const leaves = $derived(meta?.leaves ?? []);
	const readCount = $derived(meta?.read_count ?? 0);
	const totalWords = $derived(leaves.reduce((sum, l) => sum + (l.word_count ?? 0), 0));
	const titleCount = $derived(
		`${leaves.length} ${leaves.length === 1 ? 'volume' : 'volumes'}` +
			(totalWords ? ` · ${compactWords(totalWords)} words` : ''),
	);
	const titleReadingTime = $derived(
		totalWords ? readingTimeLabel(totalWords / WORDS_PER_MINUTE) : '',
	);
	const allRead = $derived(leaves.length > 0 && readCount >= leaves.length);
	const leafReflow = (l) => l.modality === 'reflowable';
	const leafDone = (l) =>
		leafReflow(l)
			? l.value != null && l.value >= 0.98
			: l.progress != null && l.page_count && l.progress + 1 >= l.page_count;
	const leafStarted = (l) => (leafReflow(l) ? l.value != null && l.value > 0 : l.progress != null);
	const leafFraction = (l) =>
		leafReflow(l)
			? l.value != null
				? Math.min(Math.max(l.value, 0), 1)
				: 0
			: l.progress != null && l.page_count
				? Math.min((l.progress + 1) / l.page_count, 1)
				: 0;
	const leafPct = (l) => Math.round(leafFraction(l) * 100);
	const anyStarted = $derived(readCount > 0 || leaves.some(leafStarted));
	const readLabel = $derived(allRead ? 'Read again' : anyStarted ? 'Continue' : 'Read');
	function readSeries() {
		if (meta?.resume_leaf_id != null) goto(`/reader/${meta.resume_leaf_id}`);
	}
	const leafReadLabel = (l) => (leafDone(l) ? 'Read again' : leafStarted(l) ? 'Continue' : 'Read');
	const leafReadHref = (l) =>
		leafDone(l) ? `/reader/${l.item_id}?page=1` : `/reader/${l.item_id}`;

	const sources = $derived(meta?.sources ?? []);
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

	const backTarget = $derived(meta?.kind ? kindHref(meta.kind) : '/');
	const backLabel = $derived(meta?.kind ? kindLabel(meta.kind) : 'Library');

	onMount(resolveGuestSession);
</script>

<PreviewShell
	title={meta?.title ?? ''}
	artwork={meta?.cover_item_id != null ? media.thumbnail(meta.cover_item_id) : ''}
	{loading}
	{error}
	ready={meta != null}
	defaultBackTarget={backTarget}
	defaultBackLabel={backLabel}
	escapeEnabled={!showMeta && !showEdit && !showTrackers}
>
	{#snippet topActions()}
		{#if isAdmin && meta}
			<CogMenu label="Series actions">
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
					<button
						class="mitem"
						type="button"
						role="menuitem"
						onclick={() => {
							close();
							showTrackers = true;
						}}
					>
						<svg
							viewBox="0 0 24 24"
							fill="none"
							stroke="currentColor"
							stroke-width="2"
							stroke-linecap="round"
							stroke-linejoin="round"
							><rect x="3" y="5" width="18" height="16" rx="2" /><path
								d="M16 3v4M8 3v4M3 10h18"
							/></svg
						>
						Track releases…
					</button>
				{/snippet}
			</CogMenu>
		{/if}
	{/snippet}
	{#snippet aside()}
		<div class="mobilehero">
			<div class="mh-cover">
				{#if meta.cover_item_id != null}
					<CoverThumbnail src={media.thumbnail(meta.cover_item_id)} alt={meta.title} eager />
				{/if}
			</div>
			<div class="mh-body">
				<h1 class="mh-title preview-title">{meta.title}</h1>
				<PreviewTitleMetadata
					tags={meta.tags ?? []}
					kind={meta.kind}
					primaryCount={titleCount}
					readingTime={titleReadingTime}
					compact
				/>
				{#if leaves.length && !guest}
					<ProgressMeter
						label="Volumes read"
						done={allRead}
						current={readCount}
						total={leaves.length}
					/>
				{/if}
				<button class="read" onclick={readSeries} disabled={meta.resume_leaf_id == null}
					>{readLabel}</button
				>
			</div>
		</div>
		{#if !guest}
			<div class="mobileactions">
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
			</div>
		{/if}

		<div class="desktopaside">
			<div class="desktopcover">
				{#if meta.cover_item_id != null}
					<CoverThumbnail src={media.thumbnail(meta.cover_item_id)} alt={meta.title} eager />
				{/if}
			</div>
			{#if leaves.length && !guest}
				<ProgressMeter
					label="Volumes read"
					done={allRead}
					current={readCount}
					total={leaves.length}
				/>
			{/if}
			<button class="read" onclick={readSeries} disabled={meta.resume_leaf_id == null}
				>{readLabel}</button
			>
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
					onset={rateSeries}
					onclear={() => rateSeries(null)}
				/>
			{/if}
		</div>
	{/snippet}

	{#snippet detail()}
		<div class="titlerow">
			<div class="titlecopy">
				<h1 class="preview-title">{meta.title}</h1>
				<PreviewTitleMetadata
					tags={meta.tags ?? []}
					kind={meta.kind}
					primaryCount={titleCount}
					readingTime={titleReadingTime}
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
			after={afterRows}
			hiddenNamespaces={['creator', 'language']}
		/>

		<div class="pageshead"><h2 class="section">Volumes</h2></div>
		<div class="pagegrid fluid-grid">
			{#each leaves as leaf (leaf.item_id)}
				<div class="volcard">
					<div class="covwrap">
						<a
							class="vollink"
							href={`/item/${leaf.item_id}`}
							title={leaf.name}
							aria-label={leaf.name}
						>
							<CoverThumbnail src={media.thumbnail(leaf.item_id)} alt={leaf.name}>
								{#if leafDone(leaf)}
									<span class="donecheck" aria-label="Read">
										<svg
											viewBox="0 0 24 24"
											fill="none"
											stroke="currentColor"
											stroke-width="3"
											stroke-linecap="round"
											stroke-linejoin="round"><path d="M5 12l5 5L20 6" /></svg
										>
									</span>
								{:else if leafStarted(leaf)}
									<div class="vbar">
										<div class="vfill" style={`width:${leafFraction(leaf) * 100}%`}></div>
									</div>
								{/if}
							</CoverThumbnail>
						</a>
						<a class="readbanner" href={leafReadHref(leaf)}>
							<svg viewBox="0 0 24 24" fill="currentColor" aria-hidden="true"
								><path d="M8 5v14l11-7z" /></svg
							>
							{leafReadLabel(leaf)}
						</a>
					</div>
					<a class="voltitlelink" href={`/item/${leaf.item_id}`} title={leaf.name}>
						<p class="voltitle" class:finished={leafDone(leaf)}>{leaf.number_disp ?? leaf.name}</p>
						{#if leafReflow(leaf)}
							<p
								class="volprog"
								class:reading={leafStarted(leaf) && !leafDone(leaf)}
								class:complete={leafDone(leaf)}
							>
								{leafPct(leaf)}%
							</p>
						{:else if leaf.page_count}
							<p
								class="volprog"
								class:reading={leaf.progress != null && !leafDone(leaf)}
								class:complete={leafDone(leaf)}
							>
								{(leaf.progress ?? -1) + 1} / {leaf.page_count}
							</p>
						{/if}
					</a>
				</div>
			{/each}
		</div>
		{#if similar.length}
			<div class="similar">
				<SimilarShelf items={similar} />
			</div>
		{/if}
	{/snippet}
</PreviewShell>

{#if showEdit && meta}
	<EditMetadataModal
		{meta}
		mode="series"
		onClose={() => (showEdit = false)}
		onUpdated={onMetaUpdated}
	/>
{/if}

{#if showTrackers && meta}
	<TrackerModal seriesId={meta.id} kind={meta.kind} onClose={() => (showTrackers = false)} />
{/if}

{#if showMeta && meta}
	<MetadataModal
		itemId={meta.id}
		kind={meta.kind}
		sources={meta.sources ?? []}
		scope="series"
		onClose={() => (showMeta = false)}
		onUpdated={onMetaUpdated}
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
	.read {
		background: var(--accent);
		border-color: var(--accent);
		color: #fff;
		font-weight: 600;
		padding: var(--space-2) var(--space-3);
	}

	.fav {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		gap: var(--space-2);
		padding: var(--space-2) var(--space-3);
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
	.read:hover:not(:disabled) {
		filter: brightness(1.1);
		border-color: var(--accent);
	}
	.read:disabled {
		opacity: 0.55;
		cursor: not-allowed;
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
	@media (max-width: 720px) {
		.pageshead {
			margin-left: 0;
			padding-left: 0;
			margin-right: 0;
			padding-right: 0;
		}
	}
	.section {
		margin: 0;
		font-size: 0.9rem;
		font-weight: 600;
	}
	.pagegrid {
		--min-cols: 3;
		--max-cols: 8;
		--col-target: 188px;
		--grid-gap: var(--space-4);
	}
	.volcard {
		display: block;
	}
	.covwrap {
		position: relative;
		border-radius: var(--radius);
		transition:
			transform var(--ease),
			box-shadow var(--ease);
	}
	.vollink {
		display: block;
	}
	.vollink :global(.cover) {
		transition: border-color var(--ease);
	}
	.volcard:hover .covwrap {
		transform: translateY(-4px);
		box-shadow: 0 10px 24px rgba(0, 0, 0, 0.35);
	}
	.volcard:hover :global(.cover) {
		border-color: var(--accent);
	}
	.donecheck {
		position: absolute;
		top: 6px;
		right: 6px;
		z-index: 2;
		display: grid;
		place-items: center;
		width: 1.4rem;
		height: 1.4rem;
		border-radius: 50%;
		background: var(--good);
		color: #fff;
		box-shadow: 0 1px 5px rgba(0, 0, 0, 0.4);
	}
	.donecheck svg {
		width: 0.85rem;
		height: 0.85rem;
	}
	.readbanner {
		position: absolute;
		left: 0;
		right: 0;
		bottom: 0;
		z-index: 2;
		display: flex;
		align-items: center;
		justify-content: center;
		gap: 0.3rem;
		padding: 0.4rem 0.5rem;
		font-size: 0.75rem;
		font-weight: 600;
		color: #fff;
		background: color-mix(in srgb, var(--accent) 92%, transparent);
		border-radius: 0 0 var(--radius) var(--radius);
		opacity: 0;
		transform: translateY(100%);
		transition:
			opacity var(--ease),
			transform var(--ease);
	}
	.readbanner svg {
		width: 0.8rem;
		height: 0.8rem;
	}
	.volcard:hover .readbanner,
	.readbanner:focus-visible {
		opacity: 1;
		transform: translateY(0);
	}
	.readbanner:hover {
		filter: brightness(1.1);
	}
	.voltitlelink {
		display: block;
	}
	.voltitle {
		margin: var(--space-2) 0 0;
		font-size: 0.85rem;
		font-weight: 600;
		color: var(--text);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.voltitle.finished {
		color: var(--good);
	}
	.volprog {
		margin: 0.1rem 0 0;
		font-size: 0.72rem;
		color: var(--muted);
		font-variant-numeric: tabular-nums;
	}
	.volprog.reading {
		color: var(--accent);
	}
	.volprog.complete {
		color: var(--good);
	}
	.vbar {
		position: absolute;
		left: 0;
		right: 0;
		bottom: 0;
		height: 5px;
		background: rgba(0, 0, 0, 0.4);
	}
	.vfill {
		height: 100%;
		background: var(--accent);
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
			gap: var(--space-4);
		}
		.mh-title {
			font-size: 1.3rem;
			line-height: 1.2;
		}
		.mobileactions {
			display: flex;
			flex-wrap: nowrap;
			gap: var(--space-2);
			margin: var(--space-3) 0 0;
			padding: var(--space-3) 0;
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
			padding: var(--space-2) var(--space-3);
			font-size: 0.85rem;
			color: var(--muted);
			white-space: nowrap;
		}
		.mact:hover {
			border-color: var(--accent);
			color: var(--text);
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
	@media (max-width: 380px) {
		.mact-label {
			display: none;
		}
	}

	.similar {
		margin-top: var(--space-6);
		padding-top: var(--space-5);
		border-top: 1px solid var(--border);
	}
</style>
