<script>
	import { onMount } from 'svelte';
	import { page as pageStore } from '$app/stores';
	import { goto } from '$app/navigation';
	import { items as itemsApi, media, ApiError } from '$lib/api.js';
	import CoverThumbnail from '$lib/components/CoverThumbnail.svelte';
	import LibrarySearch from '$lib/library/LibrarySearch.svelte';
	import LibraryTabs from '$lib/library/LibraryTabs.svelte';
	import { parseSearch } from '$lib/librarySearch.js';
	import Shelf from '$lib/components/ui/Shelf.svelte';
	import SortControl from '$lib/components/ui/SortControl.svelte';
	import Pagination from '$lib/components/ui/Pagination.svelte';
	import PageHeader from '$lib/components/ui/PageHeader.svelte';
	import DeleteConfirm from '$lib/components/ui/DeleteConfirm.svelte';
	import { kindLabel, refreshKinds } from '$lib/kinds.js';
	import { gridColumns, rowAlignedPageSize } from '$lib/grid.js';
	import { coverId } from '$lib/cards.js';
	import { relativeTime } from '$lib/format.js';
	import { isGuest } from '$lib/session.js';
	import Loading from '$lib/components/ui/Loading.svelte';

	let { kind = null } = $props();
	const heading = $derived(kind ? kindLabel(kind) : 'Library');

	let selected = $state(new Set());
	const selectionMode = $derived(selected.size > 0);
	let showDelete = $state(false);
	let deleting = $state(false);

	const isSelected = (id) => selected.has(id);
	function toggleSelect(id) {
		const s = new Set(selected);
		s.has(id) ? s.delete(id) : s.add(id);
		selected = s;
	}
	const clearSelection = () => (selected = new Set());

	const LONG_PRESS_MS = 450;
	const MOVE_TOL = 10;
	let lpTimer;
	let lpFired = false;
	let lpDown = null;
	function onCardDown(e, id) {
		if (e.button != null && e.button > 0) return;
		lpFired = false;
		lpDown = { x: e.clientX, y: e.clientY };
		clearTimeout(lpTimer);
		lpTimer = setTimeout(() => {
			lpFired = true;
			toggleSelect(id);
		}, LONG_PRESS_MS);
	}
	function onCardMove(e) {
		if (!lpDown) return;
		if (Math.abs(e.clientX - lpDown.x) > MOVE_TOL || Math.abs(e.clientY - lpDown.y) > MOVE_TOL) {
			clearTimeout(lpTimer);
			lpDown = null;
		}
	}
	function onCardUp() {
		clearTimeout(lpTimer);
		lpDown = null;
	}
	function onCardClick(e, id) {
		if (lpFired) {
			e.preventDefault();
			lpFired = false;
		} else if (selectionMode) {
			e.preventDefault();
			toggleSelect(id);
		}
	}

	async function deleteSelected() {
		if (deleting || !selected.size) return;
		deleting = true;
		const ids = [...selected];
		const results = await Promise.allSettled(ids.map((id) => itemsApi.remove(id)));
		const failed = ids.filter((_, i) => results[i].status === 'rejected');
		const gone = new Set(ids.filter((_, i) => results[i].status === 'fulfilled'));
		if (gone.size) {
			continueItems = continueItems.filter((it) => !gone.has(it.id));
		}
		deleting = false;
		showDelete = false;
		selected = new Set(failed);
		refreshKinds();
		await loadJump(page);
		if (failed.length) {
			error = `${failed.length} item${failed.length === 1 ? '' : 's'} couldn't be deleted.`;
		}
	}

	const SORT_KEY = 'arca:sort';
	const SORT_FIELDS = ['added_at', 'title', 'creator', 'rating', 'page_count'];
	let sortField = $state('added_at');
	let sortOrder = $state('desc');
	if (typeof localStorage !== 'undefined') {
		try {
			const saved = JSON.parse(localStorage.getItem(SORT_KEY) || '{}');
			if (SORT_FIELDS.includes(saved.field)) sortField = saved.field;
			if (saved.order === 'asc' || saved.order === 'desc') sortOrder = saved.order;
		} catch {
			/* ignored */
		}
	}
	$effect(() => {
		if (typeof localStorage !== 'undefined') {
			localStorage.setItem(SORT_KEY, JSON.stringify({ field: sortField, order: sortOrder }));
		}
	});

	let relevanceFallback = $state(false);
	const effectiveSort = $derived(relevanceFallback ? 'relevance' : sortField);

	let sortInited = false;
	$effect(() => {
		sortField;
		sortOrder;
		if (!sortInited) {
			sortInited = true;
			return;
		}
		if (!relevanceFallback) loadFirst();
	});

	const PAGE_TARGET = 36;
	let gridCols = $state(estimateCols());
	const PAGE_SIZE = $derived(rowAlignedPageSize(gridCols, PAGE_TARGET, 2, 6));

	function estimateCols() {
		if (typeof window === 'undefined') return 6;
		const sidebar = window.innerWidth > 900 ? 200 : 0;
		const pad = 90;
		const gap = 16,
			target = 160,
			min = 2,
			max = 10;
		const avail = window.innerWidth - sidebar - pad;
		return Math.min(max, Math.max(min, Math.floor((avail + gap) / (target + gap))));
	}

	function onCols(n) {
		if (n === gridCols) return;
		gridCols = n;
		if (booted) loadJump(page);
	}

	let items = $state([]);
	let continueItems = $state([]);
	let page = $state(1);
	let pageCount = $state(1);
	let total = $state(0);
	let prevCursor = $state(null);
	let nextCursor = $state(null);
	let loading = $state(true);
	let error = $state(null);
	let booted = $state(false);

	let query = $state('');
	let tagFilter = $state('');
	let searchText = $state('');

	let favoritesOnly = $state(loadFav());
	function loadFav() {
		try {
			return localStorage.getItem('arca:favorites') === '1';
		} catch {
			return false;
		}
	}
	function toggleFavorites() {
		favoritesOnly = !favoritesOnly;
		try {
			localStorage.setItem('arca:favorites', favoritesOnly ? '1' : '0');
		} catch {
			/* ignored */
		}
		loadJump(1);
	}

	let showFilters = $state(false);
	let fUntagged = $state(false);
	let fCompleted = $state(false);
	let fUncompleted = $state(false);
	loadFilters();
	function loadFilters() {
		try {
			const f = JSON.parse(localStorage.getItem('arca:filters') || '{}');
			fUntagged = !!f.untagged;
			fCompleted = !!f.completed;
			fUncompleted = !!f.uncompleted;
		} catch {
			/* ignored */
		}
	}
	const untaggedParam = $derived(fUntagged ? true : undefined);
	const completedParam = $derived(
		fCompleted && !fUncompleted ? true : !fCompleted && fUncompleted ? false : undefined,
	);
	const filtersActive = $derived(untaggedParam !== undefined || completedParam !== undefined);
	function toggleFilter(which) {
		if (which === 'untagged') fUntagged = !fUntagged;
		else if (which === 'completed') fCompleted = !fCompleted;
		else fUncompleted = !fUncompleted;
		try {
			localStorage.setItem(
				'arca:filters',
				JSON.stringify({ untagged: fUntagged, completed: fCompleted, uncompleted: fUncompleted }),
			);
		} catch {
			/* ignored */
		}
		loadJump(1);
	}

	const thumb = (id, v) => media.thumbnail(id, v);
	const continueHref = (a) =>
		a.type === 'series'
			? a.resume_leaf_id != null
				? `/item/${a.resume_leaf_id}`
				: `/series/${a.id}`
			: `/item/${a.id}`;

	function progressFraction(c) {
		if (!c) return 0;
		if (c.value != null) return Math.min(Math.max(c.value, 0), 1);
		if (!c.page_count || c.progress == null) return 0;
		return Math.min((c.progress + 1) / c.page_count, 1);
	}
	const progressPct = (c) => Math.round(progressFraction(c) * 100);
	const isReflow = (c) => c?.modality === 'reflowable' || c?.value != null;

	async function fetchList(params) {
		loading = true;
		error = null;
		try {
			const q = query.trim();
			const data = await itemsApi.list({
				limit: PAGE_SIZE,
				kind: kind || undefined,
				q: q || undefined,
				tags: tagFilter || undefined,
				favorited: favoritesOnly ? true : undefined,
				untagged: untaggedParam,
				completed: completedParam,
				sort: effectiveSort,
				order: sortOrder,
				...params,
			});
			items = data.items;
			prevCursor = data.prev_cursor ?? null;
			nextCursor = data.next_cursor ?? null;
			return data;
		} catch (e) {
			if (e instanceof ApiError && e.status === 401) return null;
			error = e.message ?? String(e);
			return null;
		} finally {
			loading = false;
		}
	}

	async function loadFirst() {
		if (await fetchList({})) page = 1;
	}
	async function loadLast() {
		if (await fetchList({ last: true })) page = pageCount;
	}
	async function loadNext() {
		if (!nextCursor) return;
		if (await fetchList({ cursor: nextCursor })) page = Math.min(page + 1, pageCount);
	}
	async function loadPrev() {
		if (!prevCursor) return;
		if (await fetchList({ before: prevCursor })) page = Math.max(page - 1, 1);
	}
	async function loadJump(p) {
		const d = await fetchList({ page: p });
		if (d) {
			page = d.page ?? p;
			pageCount = d.page_count ?? pageCount;
			total = d.total ?? total;
		}
		return d;
	}

	async function searchWithFallback(p) {
		relevanceFallback = false;
		const d = await loadJump(p);
		if (query.trim() && d && (d.total ?? d.items?.length ?? 0) === 0) {
			relevanceFallback = true;
			return loadJump(p);
		}
		return d;
	}

	async function navigate(p) {
		if (loading || p === page) return;
		if (!nextCursor && !prevCursor) await loadJump(p);
		else if (p === page + 1) await loadNext();
		else if (p === page - 1) await loadPrev();
		else if (p === 1) await loadFirst();
		else if (p === pageCount) await loadLast();
		else await loadJump(p);
		window.scrollTo({ top: 0, behavior: 'smooth' });
	}

	async function loadContinue() {
		try {
			const data = await itemsApi.continue(15, kind || undefined);
			continueItems = data.items;
		} catch {
			/* ignored */
		}
	}

	const hasFilter = () => query !== '' || tagFilter !== '';

	function commitSearch(tagsCsv, q) {
		tagFilter = tagsCsv;
		query = q;
		if (q.trim()) {
			searchWithFallback(1);
		} else {
			relevanceFallback = false;
			loadJump(1);
		}
	}

	function onSearchCommit(text) {
		if (!text.trim()) {
			if (hasFilter()) commitSearch('', '');
			return;
		}
		const { tags, q } = parseSearch(text);
		commitSearch(tags.join(','), q);
	}

	onMount(() => {
		const sp = $pageStore.url.searchParams;
		const t = sp.get('tags');
		const qp = sp.get('q');
		const pg = parseInt(sp.get('page') ?? '', 10);
		if (t) tagFilter = t;
		if (qp) query = qp;
		searchText = [qp, t].filter(Boolean).join(', ');
		if (sp.get('favorited') === 'true') favoritesOnly = true;
		const startPage = Number.isFinite(pg) && pg > 1 ? pg : 1;
		const firstLoad = query.trim() ? searchWithFallback(startPage) : loadJump(startPage);
		const loads = $isGuest ? [firstLoad] : [firstLoad, loadContinue()];
		Promise.all(loads).finally(() => (booted = true));
	});

	function syncUrl() {
		const sp = new URLSearchParams();
		const q = query.trim();
		if (q) sp.set('q', q);
		if (tagFilter) sp.set('tags', tagFilter);
		if (favoritesOnly) sp.set('favorited', 'true');
		if (page > 1) sp.set('page', String(page));
		const qs = sp.toString();
		const base = kind ? `/${encodeURIComponent(kind)}` : '/';
		const url = qs ? `${base}?${qs}` : base;
		if (url !== location.pathname + location.search) {
			goto(url, { replaceState: true, keepFocus: true, noScroll: true });
		}
	}
	let urlInited = false;
	$effect(() => {
		query;
		tagFilter;
		favoritesOnly;
		page;
		if (!urlInited) {
			urlInited = true;
			return;
		}
		syncUrl();
	});
</script>

<div class="page">
	{#if selectionMode}
		<div class="selbar" role="toolbar" aria-label="Selection actions">
			<div class="seltop">
				<span class="selcount">{selected.size} selected</span>
				<button class="selx" type="button" onclick={clearSelection} aria-label="Clear selection">
					<svg
						viewBox="0 0 24 24"
						fill="none"
						stroke="currentColor"
						stroke-width="2"
						stroke-linecap="round"><path d="M6 6l12 12M18 6 6 18" /></svg
					>
				</button>
			</div>
			<button class="seldel" type="button" onclick={() => (showDelete = true)}>
				<svg
					viewBox="0 0 24 24"
					fill="none"
					stroke="currentColor"
					stroke-width="2"
					stroke-linecap="round"
					stroke-linejoin="round"
				>
					<path d="M3 6h18" />
					<path
						d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"
					/>
					<path d="M10 11v6M14 11v6" />
				</svg>
				<span>Delete</span>
			</button>
		</div>
	{/if}

	<PageHeader title={heading} />
	<LibraryTabs {kind} active="library" />

	{#if !booted}
		<Loading />
	{:else}
		{#if error}<p class="err">{error}</p>{/if}

		{#if continueItems.length}
			<Shelf title="Continue Reading" storageKey="arca:shelf:continue">
				{#each continueItems as a (a.type + ':' + a.id)}
					<a class="shelfcard" href={continueHref(a)} draggable="false" title={a.name}>
						<CoverThumbnail src={thumb(coverId(a), a.cover_version)} alt={a.name} eager>
							{#if a.last_read_at}<span class="ago">{relativeTime(a.last_read_at)}</span>{/if}
							<div class="prog">
								<div class="prog-nums">
									{#if isReflow(a)}
										<span>{progressPct(a)}%</span>
									{:else if a.page_count != null}
										<span>{(a.progress ?? 0) + 1}</span>
										<span>{a.page_count}</span>
									{/if}
								</div>
								<div class="prog-track">
									<div class="prog-bar" style={`width:${progressFraction(a) * 100}%`}></div>
								</div>
							</div>
						</CoverThumbnail>
						<p class="cardtitle">{a.name}</p>
					</a>
				{/each}
			</Shelf>
		{/if}

		<h2 class="section">Library</h2>
		<div class="searchrow">
			<LibrarySearch bind:value={searchText} {kind} oncommit={onSearchCommit} />
		</div>
		<div class="libcontrols">
			<div class="leftcontrols">
				{#if relevanceFallback}
					<div class="relsort" title="No exact matches — showing the most relevant results">
						<span class="label">Sort</span>
						<span class="relchip">Relevance</span>
					</div>
				{:else}
					<SortControl bind:field={sortField} bind:order={sortOrder} />
				{/if}
				{#if !$isGuest}
					<button
						class="favbtn"
						class:on={favoritesOnly}
						onclick={toggleFavorites}
						type="button"
						aria-pressed={favoritesOnly}
						title="Show favorites only"
					>
						<svg
							viewBox="0 0 24 24"
							fill={favoritesOnly ? 'currentColor' : 'none'}
							stroke="currentColor"
							stroke-width="2"
						>
							<path
								d="M20.84 4.61a5.5 5.5 0 0 0-7.78 0L12 5.67l-1.06-1.06a5.5 5.5 0 0 0-7.78 7.78L12 21.23l8.84-8.84a5.5 5.5 0 0 0 0-7.78z"
							/>
						</svg>
						<span>Favorites</span>
					</button>
				{/if}
			</div>
			<div class="rightcontrols">
				<span class="count">{total} items</span>
				<div class="filterwrap">
					<button
						class="cog"
						class:active={filtersActive}
						onclick={() => (showFilters = !showFilters)}
						type="button"
						aria-label="Library filters"
						aria-expanded={showFilters}
						title="Library filters"
					>
						<svg
							viewBox="0 0 24 24"
							fill="none"
							stroke="currentColor"
							stroke-width="2"
							stroke-linecap="round"
							stroke-linejoin="round"
						>
							<circle cx="12" cy="12" r="3" />
							<path
								d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"
							/>
						</svg>
						{#if filtersActive}<span class="cogdot"></span>{/if}
					</button>
					{#if showFilters}
						<button
							class="scrim"
							onclick={() => (showFilters = false)}
							aria-label="Close filters"
							type="button"
						></button>
						<div class="filtermenu" role="menu">
							<p class="fhead">Library filters</p>
							<button
								class="fopt"
								onclick={() => toggleFilter('untagged')}
								role="menuitemcheckbox"
								aria-checked={fUntagged}
								type="button"
							>
								<span class="check" class:on={fUntagged}>
									<svg
										viewBox="0 0 24 24"
										fill="none"
										stroke="currentColor"
										stroke-width="3"
										stroke-linecap="round"
										stroke-linejoin="round"><path d="M20 6 9 17l-5-5" /></svg
									>
								</span>
								<span>Show Untagged Items</span>
							</button>
							<button
								class="fopt"
								onclick={() => toggleFilter('completed')}
								role="menuitemcheckbox"
								aria-checked={fCompleted}
								type="button"
							>
								<span class="check" class:on={fCompleted}>
									<svg
										viewBox="0 0 24 24"
										fill="none"
										stroke="currentColor"
										stroke-width="3"
										stroke-linecap="round"
										stroke-linejoin="round"><path d="M20 6 9 17l-5-5" /></svg
									>
								</span>
								<span>Show Completed Items</span>
							</button>
							<button
								class="fopt"
								onclick={() => toggleFilter('uncompleted')}
								role="menuitemcheckbox"
								aria-checked={fUncompleted}
								type="button"
							>
								<span class="check" class:on={fUncompleted}>
									<svg
										viewBox="0 0 24 24"
										fill="none"
										stroke="currentColor"
										stroke-width="3"
										stroke-linecap="round"
										stroke-linejoin="round"><path d="M20 6 9 17l-5-5" /></svg
									>
								</span>
								<span>Show Uncompleted Items</span>
							</button>
						</div>
					{/if}
				</div>
			</div>
		</div>
		{#if relevanceFallback && items.length > 0}
			<p class="fallbacknote">
				No exact matches for “{query.trim()}” — showing the most relevant results instead.
			</p>
		{/if}
		<div class="grid fluid-grid" use:gridColumns={onCols}>
			{#each items as a (a.type + ':' + a.id)}
				{#if a.type === 'series'}
					<a class="gridcard" href={`/series/${a.id}`} title={a.name}>
						<CoverThumbnail src={thumb(a.cover_item_id, a.cover_version)} alt={a.name} />
						<p class="cardtitle">{a.name}</p>
						<p class="pages">{a.leaf_count} {a.leaf_count === 1 ? 'volume' : 'volumes'}</p>
					</a>
				{:else}
					<a
						class="gridcard"
						class:selected={isSelected(a.id)}
						href={`/item/${a.id}`}
						title={a.name}
						draggable="false"
						onpointerdown={(e) => onCardDown(e, a.id)}
						onpointermove={onCardMove}
						onpointerup={onCardUp}
						onpointercancel={onCardUp}
						onclick={(e) => onCardClick(e, a.id)}
						oncontextmenu={(e) => selectionMode && e.preventDefault()}
					>
						<CoverThumbnail src={thumb(a.id, a.cover_version)} alt={a.name}>
							{#if isSelected(a.id)}
								<span class="selring" aria-hidden="true"></span>
								<span class="selmark" aria-hidden="true">
									<svg
										viewBox="0 0 20 20"
										fill="none"
										stroke="currentColor"
										stroke-width="2.5"
										stroke-linecap="round"
										stroke-linejoin="round"><path d="M4 10l4 4 8-8" /></svg
									>
								</span>
							{/if}
							{#if a.progress != null}
								<div class="bar">
									<div
										class="fill"
										class:complete={progressFraction(a) >= 1}
										style={`width:${progressFraction(a) * 100}%`}
									></div>
								</div>
							{/if}
						</CoverThumbnail>
						<p class="cardtitle">{a.name}</p>
						{#if isReflow(a) && a.value != null}
							<p class="pages" class:reading={true} class:complete={progressFraction(a) >= 1}>
								{progressPct(a)}%
							</p>
						{:else if a.page_count != null}
							<p
								class="pages"
								class:reading={a.progress != null}
								class:complete={a.progress != null && progressFraction(a) >= 1}
							>
								{(a.progress ?? -1) + 1} / {a.page_count}
							</p>
						{/if}
					</a>
				{/if}
			{/each}
		</div>

		{#if items.length === 0}
			<p class="status">
				{#if query.trim() || tagFilter}
					No matches for “{[query.trim(), tagFilter].filter(Boolean).join(', ')}”.
				{:else if favoritesOnly}
					No favorites yet — open an item and tap the heart.
				{:else}
					No items indexed yet.
				{/if}
			</p>
		{/if}

		{#if pageCount > 1}
			<div class="pgwrap">
				<Pagination jump {page} {pageCount} onnavigate={navigate} />
			</div>
		{/if}
	{/if}

	{#if showDelete}
		<DeleteConfirm
			count={selected.size}
			busy={deleting}
			onConfirm={deleteSelected}
			onClose={() => (showDelete = false)}
		/>
	{/if}
</div>

<style>
	.page {
		padding: var(--space-6) clamp(var(--space-4), 5vw, var(--space-8));
	}
	@media (max-width: 1200px) {
		.page {
			padding-inline: clamp(var(--space-4), 5vw, var(--space-6));
		}
	}
	.shelfcard {
		flex: 0 0 auto;
		width: 150px;
		-webkit-user-drag: none;
	}
	.shelfcard .cardtitle {
		font-size: 0.72rem;
	}
	.ago {
		position: absolute;
		left: var(--space-2);
		top: var(--space-2);
		padding: 0.1rem 0.45rem;
		border-radius: 9999px;
		background: rgba(0, 0, 0, 0.6);
		border: 1px solid rgba(255, 255, 255, 0.1);
		font-size: 0.55rem;
		text-transform: uppercase;
		letter-spacing: 0.05em;
		color: rgba(255, 255, 255, 0.9);
		backdrop-filter: blur(4px);
	}
	.prog {
		position: absolute;
		inset: auto 0 0 0;
		padding: var(--space-5) var(--space-2) var(--space-2);
		background: linear-gradient(to top, rgba(0, 0, 0, 0.8), rgba(0, 0, 0, 0.3) 60%, transparent);
	}
	.prog-nums {
		display: flex;
		justify-content: space-between;
		margin-bottom: 0.25rem;
		font-variant-numeric: tabular-nums;
		font-size: 0.6rem;
		color: var(--text);
	}
	.prog-track {
		height: 3px;
		border-radius: 9999px;
		overflow: hidden;
		background: rgba(255, 255, 255, 0.22);
	}
	.prog-bar {
		height: 100%;
		background: var(--accent);
	}

	.section {
		margin: var(--space-2) 0 var(--space-3);
		font-size: 1.05rem;
		font-weight: 600;
	}
	.searchrow {
		display: flex;
		justify-content: center;
		margin-bottom: var(--space-4);
	}
	.libcontrols {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--space-4);
		flex-wrap: wrap;
		margin-bottom: var(--space-4);
	}
	.leftcontrols {
		display: flex;
		align-items: center;
		gap: var(--space-3);
		flex-wrap: wrap;
	}
	.relsort {
		display: flex;
		align-items: center;
		gap: var(--space-3);
	}
	.relsort .label {
		font-size: 0.7rem;
		text-transform: uppercase;
		letter-spacing: 0.15em;
		color: var(--muted);
	}
	.relchip {
		font-size: 0.9rem;
		color: var(--text);
	}
	@media (max-width: 640px) {
		.relsort .label {
			display: none;
		}
	}
	.favbtn {
		display: inline-flex;
		align-items: center;
		gap: var(--space-2);
		border-radius: var(--radius-sm);
		font-size: 0.85rem;
		color: var(--muted);
	}
	.favbtn svg {
		width: 0.95rem;
		height: 0.95rem;
	}
	.favbtn:hover {
		color: var(--text);
	}
	.favbtn.on {
		border-color: #e0566f;
		background: rgba(224, 86, 111, 0.12);
		color: #e0566f;
	}
	.favbtn.on:hover {
		border-color: #e0566f;
	}
	@media (max-width: 640px) {
		.favbtn span {
			display: none;
		}
		.favbtn {
			padding-inline: var(--space-2);
		}
		.libcontrols {
			gap: var(--space-2);
		}
		.leftcontrols {
			gap: var(--space-2);
		}
		.rightcontrols {
			gap: var(--space-2);
		}
	}
	.count {
		font-variant-numeric: tabular-nums;
		font-size: 0.78rem;
		color: var(--muted);
		white-space: nowrap;
	}
	.rightcontrols {
		display: flex;
		align-items: center;
		gap: var(--space-3);
	}
	.filterwrap {
		position: relative;
	}
	.cog {
		all: unset;
		position: relative;
		display: inline-flex;
		align-items: center;
		justify-content: center;
		width: 2.1rem;
		height: 2.1rem;
		border: 1px solid var(--border);
		border-radius: var(--radius-sm);
		color: var(--muted);
		cursor: pointer;
		transition:
			border-color var(--ease),
			color var(--ease),
			background var(--ease);
	}
	.cog:hover {
		border-color: var(--accent);
		color: var(--text);
	}
	.cog.active {
		border-color: var(--accent);
		color: var(--accent);
		background: var(--accent-soft);
	}
	.cog svg {
		width: 1.05rem;
		height: 1.05rem;
	}
	.cogdot {
		position: absolute;
		top: -3px;
		right: -3px;
		width: 8px;
		height: 8px;
		border-radius: 50%;
		background: var(--accent);
		border: 2px solid var(--bg);
	}
	.scrim {
		all: unset;
		position: fixed;
		inset: 0;
		z-index: 40;
		cursor: default;
	}
	.filtermenu {
		position: absolute;
		top: calc(100% + 6px);
		right: 0;
		z-index: 41;
		min-width: 15rem;
		padding: var(--space-2);
		background: var(--surface);
		border: 1px solid var(--border);
		border-radius: var(--radius);
		box-shadow: var(--shadow-lg, 0 12px 32px rgba(0, 0, 0, 0.35));
	}
	.fhead {
		margin: var(--space-2) var(--space-2) var(--space-2);
		font-size: 0.66rem;
		text-transform: uppercase;
		letter-spacing: 0.14em;
		color: var(--muted);
	}
	.fopt {
		all: unset;
		display: flex;
		align-items: center;
		gap: var(--space-3);
		width: 100%;
		box-sizing: border-box;
		padding: var(--space-2) var(--space-2);
		border-radius: var(--radius-sm);
		font-size: 0.9rem;
		color: var(--text);
		cursor: pointer;
	}
	.fopt:hover {
		background: var(--surface-2);
	}
	.check {
		flex: 0 0 auto;
		display: inline-flex;
		align-items: center;
		justify-content: center;
		width: 1.1rem;
		height: 1.1rem;
		border: 1.5px solid var(--border);
		border-radius: 5px;
		color: transparent;
		transition:
			background var(--ease),
			border-color var(--ease),
			color var(--ease);
	}
	.check.on {
		background: var(--accent);
		border-color: var(--accent);
		color: #fff;
	}
	.check svg {
		width: 0.8rem;
		height: 0.8rem;
	}
	.grid {
		--min-cols: 2;
		--max-cols: 10;
		--col-target: clamp(150px, 14vw, 195px);
		--grid-gap: var(--space-4);
	}

	.shelfcard,
	.gridcard {
		display: block;
	}
	.gridcard {
		user-select: none;
		-webkit-user-select: none;
		-webkit-touch-callout: none;
	}
	.gridcard :global(.cover),
	.shelfcard :global(.cover) {
		transition:
			transform var(--ease),
			box-shadow var(--ease),
			border-color var(--ease);
	}
	.gridcard:hover :global(.cover),
	.shelfcard:hover :global(.cover) {
		transform: translateY(-3px);
		border-color: var(--accent);
		box-shadow: var(--shadow-lg);
	}
	.gridcard.selected :global(.cover) {
		border-color: var(--accent);
	}

	.selring {
		position: absolute;
		inset: 0;
		border-radius: inherit;
		box-shadow: inset 0 0 0 3px var(--accent);
		background: color-mix(in srgb, var(--accent) 22%, transparent);
		pointer-events: none;
		z-index: 2;
	}
	.selmark {
		position: absolute;
		top: 6px;
		right: 6px;
		z-index: 3;
		display: flex;
		align-items: center;
		justify-content: center;
		width: 1.4rem;
		height: 1.4rem;
		border-radius: 50%;
		background: var(--accent);
		color: #fff;
		box-shadow: 0 1px 3px rgba(0, 0, 0, 0.4);
		pointer-events: none;
	}
	.selmark svg {
		width: 0.9rem;
		height: 0.9rem;
	}

	.selbar {
		position: fixed;
		top: var(--space-4);
		left: 50%;
		transform: translateX(-50%);
		z-index: 40;
		display: flex;
		flex-direction: column;
		gap: var(--space-2);
		width: 20rem;
		max-width: calc(100vw - 2rem);
		padding: var(--space-3);
		border: 1px solid var(--border);
		border-radius: var(--radius-lg);
		background: color-mix(in srgb, var(--surface) 94%, transparent);
		backdrop-filter: blur(10px);
		box-shadow: var(--shadow-lg);
	}
	@media (max-width: 900px) {
		.selbar {
			top: 3.5rem;
		}
	}
	.seltop {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--space-3);
		padding: 0 var(--space-1);
	}
	.selcount {
		font-size: 0.85rem;
		font-weight: 600;
		color: var(--text);
	}
	.seldel {
		display: flex;
		align-items: center;
		justify-content: center;
		gap: var(--space-2);
		width: 100%;
		padding: 0.55rem 0.9rem;
		border: 1px solid #d24a5f;
		border-radius: var(--radius-sm);
		background: #d24a5f;
		color: #fff;
		font-weight: 600;
		font-size: 0.9rem;
	}
	.seldel:hover {
		background: #e0566f;
		border-color: #e0566f;
	}
	.seldel svg {
		width: 1rem;
		height: 1rem;
	}
	.selx {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		width: 1.8rem;
		height: 1.8rem;
		padding: 0;
		border: 1px solid transparent;
		border-radius: var(--radius-sm);
		background: transparent;
		color: var(--muted);
	}
	.selx:hover {
		background: var(--surface-2);
		color: var(--text);
		border-color: var(--border);
	}
	.selx svg {
		width: 1.05rem;
		height: 1.05rem;
	}
	.cardtitle {
		margin: var(--space-2) 0 0;
		font-size: 0.82rem;
		line-height: 1.3;
		color: var(--text);
		display: -webkit-box;
		-webkit-line-clamp: 2;
		-webkit-box-orient: vertical;
		overflow: hidden;
	}
	.pages {
		margin: 0.2rem 0 0;
		font-variant-numeric: tabular-nums;
		font-size: 0.68rem;
		color: var(--muted);
	}
	.pages.reading {
		color: var(--accent);
	}
	.pages.complete {
		color: var(--good);
	}

	.bar {
		position: absolute;
		inset: auto 0 0 0;
		height: 3px;
		background: rgba(0, 0, 0, 0.45);
	}
	.fill {
		height: 100%;
		background: var(--accent);
	}
	.fill.complete {
		background: var(--good);
	}

	.status {
		text-align: center;
		padding: var(--space-5);
	}
	.fallbacknote {
		margin: 0 0 var(--space-4);
		font-size: 0.85rem;
		color: var(--muted);
	}
	.pgwrap {
		display: flex;
		justify-content: center;
		margin-top: var(--space-6);
	}
</style>
