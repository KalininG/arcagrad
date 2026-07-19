<script>
	import { goto, replaceState } from '$app/navigation';
	import { page as pageStore } from '$app/stores';
	import { get } from 'svelte/store';
	import {
		plugins as pluginsApi,
		kinds as kindsApi,
		library as libraryApi,
		follows as followsApi,
		media,
		ApiError,
	} from '$lib/api.js';
	import { currentUser } from '$lib/session.js';
	import { loadCover } from '$lib/browse/coverwarm.js';
	import PageHeader from '$lib/components/ui/PageHeader.svelte';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';
	import Pagination from '$lib/components/ui/Pagination.svelte';
	import BrowseCard from '$lib/browse/BrowseCard.svelte';
	import Dropdown from '$lib/components/ui/Dropdown.svelte';
	import Loading from '$lib/components/ui/Loading.svelte';

	let { kind } = $props();

	let plugins = $state([]);
	let pluginId = $state('');
	let feedId = $state('');
	let range = $state('');
	let filter = $state('');
	let filterInput = $state('');
	let page = $state(1);

	let items = $state([]);
	let numPages = $state(1);
	let hasTotal = $state(false);
	let loading = $state(true);
	let feedLoading = $state(false);
	let error = $state('');

	const plugin = $derived(plugins.find((p) => p.id === pluginId) ?? null);
	const feeds = $derived(plugin?.feeds ?? []);
	const feed = $derived(feeds.find((f) => f.id === feedId) ?? null);
	const ranges = $derived(feed?.ranges ?? []);
	const hasQuery = $derived(!!feed?.query);

	const RANGE_LABEL = { today: 'Today', week: 'Week', month: 'Month', all: 'All-time' };
	const rangeLabel = (r) => RANGE_LABEL[r] ?? r;
	const isAdmin = $derived($currentUser?.role === 'admin');

	let followTab = $state(false);
	let myFollows = $state([]);
	let followQuery = $state('');
	let followBusy = $state(false);
	let followMsg = $state('');
	let testItems = $state(null);
	let testBusy = $state(false);
	let testErr = $state('');
	const chronoFeed = $derived(
		plugin?.followable === false
			? null
			: (feeds.find((f) => f.id === 'recent' && f.query) ?? feeds.find((f) => f.query) ?? null),
	);
	const sourceFollows = $derived(
		myFollows.filter((w) => w.plugin_id === pluginId && w.kind === kind),
	);
	const alreadyFollowing = $derived(
		sourceFollows.find((w) => w.query === followQuery.trim()) ?? null,
	);
	async function loadMyFollows() {
		if (!isAdmin) return;
		myFollows = await followsApi.list().catch(() => []);
	}
	function selectFollow() {
		if (followTab) return;
		followTab = true;
		testItems = null;
		testErr = '';
		followMsg = '';
		syncUrl();
	}
	async function testFollow() {
		const q = followQuery.trim();
		if (!q || testBusy || !chronoFeed) return;
		testBusy = true;
		testErr = '';
		followMsg = '';
		try {
			const d = await pluginsApi.browse(pluginId, { feed: chronoFeed.id, query: q, page: 1 });
			testItems = d?.items ?? [];
			runOwnershipMatch(testItems);
		} catch (e) {
			testErr = e?.message ?? String(e);
			testItems = null;
		} finally {
			testBusy = false;
		}
	}
	function onFollowKey(e) {
		if (e.key === 'Enter') {
			e.preventDefault();
			testFollow();
		}
	}
	function onFollowInput() {
		if (!followQuery.trim()) {
			testItems = null;
			testErr = '';
			followMsg = '';
		}
	}
	async function followSearch() {
		const q = followQuery.trim();
		if (!q || followBusy || !chronoFeed) return;
		followBusy = true;
		try {
			await followsApi.create({ plugin_id: pluginId, kind, feed: chronoFeed.id, query: q });
			await loadMyFollows();
			followMsg = `Following ${q}. New uploads get staged for review — nothing from the back-catalog is fetched.`;
		} catch (e) {
			testErr = e?.message ?? String(e);
		} finally {
			followBusy = false;
		}
	}
	async function unfollow(w) {
		await followsApi.remove(w.id).catch(() => {});
		await loadMyFollows();
	}

	$effect(() => {
		kind;
		discover();
	});
	async function discover() {
		loading = true;
		error = '';
		try {
			const [all, kindMap] = await Promise.all([
				pluginsApi.list({ capability: 'browse' }),
				kindsApi.plugins(kind).catch(() => []),
			]);
			const enabledIds = new Set((kindMap ?? []).filter((p) => p.enabled).map((p) => p.id));
			plugins = (all ?? []).filter((p) => enabledIds.has(p.id) && p.feeds?.length);
			loadMyFollows();
			if (plugins.length) {
				const want = readUrlState();
				const p = plugins.find((x) => x.id === want.src) ?? plugins[0];
				pluginId = p.id;
				applyFeedState(p, want);
				await loadFeed();
			} else {
				items = [];
			}
		} catch (e) {
			if (!(e instanceof ApiError && e.status === 401)) error = e?.message ?? String(e);
		} finally {
			loading = false;
		}
	}
	function pickFeedDefaults(p) {
		const f = p.feeds?.[0];
		feedId = f?.id ?? '';
		range = f?.ranges?.[0] ?? '';
		filter = '';
		filterInput = '';
		page = 1;
	}

	function readUrlState() {
		const sp = get(pageStore).url.searchParams;
		return {
			src: sp.get('src') ?? '',
			feed: sp.get('feed') ?? '',
			range: sp.get('range') ?? '',
			q: sp.get('q') ?? '',
			page: Math.max(1, parseInt(sp.get('page') ?? '1', 10) || 1),
		};
	}
	function applyFeedState(p, want) {
		followTab = want.feed === 'follow' && isAdmin && p.followable !== false;
		const f = (p.feeds ?? []).find((x) => x.id === want.feed) ?? p.feeds?.[0];
		feedId = f?.id ?? '';
		range = (f?.ranges ?? []).includes(want.range) ? want.range : (f?.ranges?.[0] ?? '');
		filter = f?.query ? want.q : '';
		filterInput = filter;
		page = want.page;
	}
	function syncUrl() {
		const url = new URL(get(pageStore).url);
		const sp = url.searchParams;
		const put = (k, v) => (v ? sp.set(k, v) : sp.delete(k));
		put('src', plugins.length > 1 ? pluginId : '');
		put('feed', followTab ? 'follow' : feedId);
		put('range', range);
		put('q', filter);
		put('page', page > 1 ? String(page) : '');
		replaceState(url, {});
	}

	async function loadFeed() {
		if (!pluginId || !feedId) return;
		syncUrl();
		if (followTab) return;
		feedLoading = true;
		matchReady = false;
		error = '';
		const token = { p: pluginId, f: feedId, r: range, q: filter, pg: page };
		try {
			const data = await pluginsApi.browse(pluginId, {
				feed: feedId,
				range: range || undefined,
				query: filter || undefined,
				page,
			});
			if (
				token.p !== pluginId ||
				token.f !== feedId ||
				token.r !== range ||
				token.q !== filter ||
				token.pg !== page
			)
				return;
			items = data.items ?? [];
			hasTotal = data.num_pages != null;
			numPages = data.num_pages ?? (items.length ? page + 1 : page);
			runOwnershipMatch(items);
		} catch (e) {
			if (!(e instanceof ApiError && e.status === 401)) error = e?.message ?? String(e);
			items = [];
			matchReady = true;
		} finally {
			feedLoading = false;
		}
	}

	let matches = $state({});
	let matchReady = $state(true);
	let matchToken = 0;
	async function runOwnershipMatch(its) {
		matches = {};
		const pid = pluginId;
		const token = ++matchToken;
		const stale = () => token !== matchToken;
		try {
			if (!its.length) return;
			try {
				const v = await libraryApi.match(its.map((it) => ({ source_url: it.source_url })));
				if (stale()) return;
				applyVerdicts(its, v);
			} catch {
				/* ignored */
			}
			try {
				await Promise.all(its.map((it) => loadCover(media.pluginImage(pid, it.cover_url))));
				if (stale()) return;
				const queries = its.map((it) => ({
					source_url: it.source_url,
					cover_url: it.cover_url,
					page_count: it.page_count,
				}));
				const v = await libraryApi.match(queries);
				if (stale()) return;
				applyVerdicts(its, v);
			} catch {
				/* ignored */
			}
		} finally {
			if (token === matchToken) matchReady = true;
		}
	}
	function applyVerdicts(its, verdicts) {
		if (!Array.isArray(verdicts)) return;
		const next = { ...matches };
		verdicts.forEach((v, i) => {
			const ref = its[i]?.reference;
			if (ref != null && (v?.owned_item_id != null || v?.likely_item_id != null)) next[ref] = v;
		});
		matches = next;
	}

	let hideOwned = $state(
		typeof localStorage !== 'undefined' && localStorage.getItem('arca:browse-hide-owned') === '1',
	);
	function toggleHideOwned() {
		hideOwned = !hideOwned;
		try {
			localStorage.setItem('arca:browse-hide-owned', hideOwned ? '1' : '0');
		} catch {
			/* ignored */
		}
	}
	const visibleItems = $derived(
		hideOwned
			? items.filter((it) => {
					const m = matches[it.reference];
					return !(m?.owned_item_id || m?.likely_item_id);
				})
			: items,
	);

	function selectPlugin(id) {
		if (id === pluginId) return;
		pluginId = id;
		pickFeedDefaults(plugins.find((p) => p.id === id) ?? {});
		loadFeed();
	}
	function selectFeed(id) {
		if (id === feedId && !followTab) return;
		followTab = false;
		feedId = id;
		range = feeds.find((f) => f.id === id)?.ranges?.[0] ?? '';
		page = 1;
		loadFeed();
	}
	function selectRange(r) {
		if (r === range) return;
		range = r;
		page = 1;
		loadFeed();
	}
	function commitFilter() {
		const next = filterInput.trim();
		if (next === filter) return;
		filter = next;
		page = 1;
		loadFeed();
	}
	function onFilterKey(e) {
		if (e.key === 'Enter') {
			e.preventDefault();
			commitFilter();
		}
	}
	function clearFilter() {
		filterInput = '';
		if (filter) {
			filter = '';
			page = 1;
			loadFeed();
		}
	}
	function goPage(p) {
		page = p;
		loadFeed();
		window.scrollTo({ top: 0, behavior: 'smooth' });
	}

	function openPreview(item) {
		const here = window.location.pathname + window.location.search;
		const back = encodeURIComponent(here);
		const q = kind ? `?kind=${encodeURIComponent(kind)}&back=${back}` : `?back=${back}`;
		goto(`/source/${encodeURIComponent(pluginId)}/${encodeURIComponent(item.reference)}${q}`);
	}

	const pluginOptions = $derived(plugins.map((p) => ({ value: p.id, label: p.name })));
</script>

<div class="app-page browse">
	<div class="head">
		<PageHeader title="Browse" />
		{#if plugins.length > 1}
			<div class="pluginpick">
				<Dropdown value={pluginId} options={pluginOptions} onchange={selectPlugin} />
			</div>
		{/if}
	</div>

	{#if loading}
		<Loading />
	{:else if !plugins.length}
		<EmptyState
			title="No browse sources for this kind"
			message="Enable a plugin with the “browse” capability for this kind to explore its catalog and pull items in."
		>
			{#snippet icon()}
				<svg
					viewBox="0 0 24 24"
					fill="none"
					stroke="currentColor"
					stroke-width="1.6"
					stroke-linecap="round"
					stroke-linejoin="round"><circle cx="11" cy="11" r="7" /><path d="m21 21-4.3-4.3" /></svg
				>
			{/snippet}
			{#snippet action()}
				<a class="link" href="/plugins">Manage plugins</a>
			{/snippet}
		</EmptyState>
	{:else}
		{#if feeds.length}
			<div class="feedtabs" role="tablist">
				{#each feeds as f (f.id)}
					<button
						class="feedtab"
						class:on={f.id === feedId && !followTab}
						role="tab"
						aria-selected={f.id === feedId && !followTab}
						type="button"
						onclick={() => selectFeed(f.id)}>{f.label}</button
					>
				{/each}
				{#if isAdmin && chronoFeed}
					<button
						class="feedtab"
						class:on={followTab}
						role="tab"
						aria-selected={followTab}
						type="button"
						onclick={selectFollow}
					>
						Follow
						{#if sourceFollows.length}<span class="fcount">{sourceFollows.length}</span>{/if}
					</button>
				{/if}
			</div>
		{/if}

		{#if followTab}
			<div class="followpane">
				<p class="fintro">
					Follow a search — an artist, a tag, anything this source can filter on — and new uploads
					matching it are <b>staged for review</b> under
					<a class="link" href="/tracking?tab=uploads">Tracking</a>. Only uploads from now on: the
					back-catalog is never fetched, and nothing downloads until you approve it. Checked nightly
					at 3&nbsp;AM.
				</p>
				<div class="fform">
					<input
						type="text"
						placeholder="e.g. artist:&quot;clamp&quot; or romance"
						bind:value={followQuery}
						onkeydown={onFollowKey}
						oninput={onFollowInput}
						aria-label="Search to follow"
					/>
					<button
						class="btn"
						type="button"
						disabled={testBusy || !followQuery.trim()}
						onclick={testFollow}
					>
						{testBusy ? 'Testing…' : 'Test search'}
					</button>
					{#if alreadyFollowing}
						<button
							class="btn following"
							type="button"
							onclick={() => unfollow(alreadyFollowing)}
							title="Stop following this search"
						>
							<svg
								viewBox="0 0 24 24"
								fill="none"
								stroke="currentColor"
								stroke-width="2.4"
								stroke-linecap="round"
								stroke-linejoin="round"><path d="M20 6 9 17l-5-5" /></svg
							>
							Following
						</button>
					{:else}
						<button
							class="btn accent"
							type="button"
							disabled={followBusy || !followQuery.trim()}
							onclick={followSearch}
						>
							{followBusy ? 'Following…' : 'Follow this search'}
						</button>
					{/if}
				</div>
				{#if followMsg}<p class="fmsg good">{followMsg}</p>{/if}
				{#if testErr}<p class="err">{testErr}</p>{/if}

				{#if testItems}
					<p class="ftesthead">
						{#if testItems.length}
							Newest matches right now — uploads after these are what you'd be notified about:
						{:else}
							No matches for that search — check the spelling / the source's tag syntax.
						{/if}
					</p>
					{#if testItems.length}
						<div class="grid fluid-grid">
							{#each testItems.slice(0, 10) as item (item.reference)}
								<BrowseCard
									{pluginId}
									{item}
									match={matches[item.reference]}
									onpreview={openPreview}
								/>
							{/each}
						</div>
					{/if}
				{/if}

				{#if sourceFollows.length}
					<div class="flist">
						<p class="flisthead">Following on this source</p>
						{#each sourceFollows as w (w.id)}
							<div class="frow">
								<span class="fquery">{w.query}</span>
								{#if w.new_count > 0}<a class="fnew" href="/tracking?tab=uploads"
										>{w.new_count} new</a
									>{/if}
								<button class="btn tiny unfollow" type="button" onclick={() => unfollow(w)}
									>Unfollow</button
								>
							</div>
						{/each}
					</div>
				{/if}
			</div>
		{:else}
			<div class="controls">
				{#if ranges.length}
					<div class="chips" role="group" aria-label="Time range">
						{#each ranges as r (r)}
							<button
								class="chip"
								class:on={r === range}
								type="button"
								onclick={() => selectRange(r)}>{rangeLabel(r)}</button
							>
						{/each}
					</div>
				{/if}
				<button
					class="chip hideown"
					class:on={hideOwned}
					type="button"
					onclick={toggleHideOwned}
					title="Hide items already in your library"
				>
					<svg
						viewBox="0 0 24 24"
						fill="none"
						stroke="currentColor"
						stroke-width="1.8"
						stroke-linecap="round"
						stroke-linejoin="round"
						aria-hidden="true"
						><path
							d="M17.94 17.94A10.07 10.07 0 0 1 12 20c-7 0-11-8-11-8a18.45 18.45 0 0 1 5.06-5.94"
						/><path
							d="M9.9 4.24A9.12 9.12 0 0 1 12 4c7 0 11 8 11 8a18.5 18.5 0 0 1-2.16 3.19"
						/><path d="M14.12 14.12a3 3 0 1 1-4.24-4.24" /><line
							x1="1"
							y1="1"
							x2="23"
							y2="23"
						/></svg
					>
					Hide owned
				</button>
				{#if hasQuery}
					<div class="filter">
						<input
							type="text"
							placeholder="Filter…"
							bind:value={filterInput}
							onkeydown={onFilterKey}
							onblur={commitFilter}
							aria-label="Filter"
						/>
						{#if filterInput}<button
								class="clear"
								type="button"
								onclick={clearFilter}
								aria-label="Clear filter">×</button
							>{/if}
					</div>
				{/if}
			</div>

			{#if error && feed?.auth}
				<div class="credneed">
					<svg
						viewBox="0 0 24 24"
						fill="none"
						stroke="currentColor"
						stroke-width="1.6"
						stroke-linecap="round"
						stroke-linejoin="round"
						aria-hidden="true"
						><rect x="4" y="10" width="16" height="10" rx="2" /><path
							d="M8 10V7a4 4 0 0 1 8 0v3"
						/></svg
					>
					<div class="credtext">
						<p class="credtitle">This feed uses your {plugin?.name ?? pluginId} account</p>
						<p class="credmsg">{error}</p>
						{#if isAdmin}
							<a class="link" href={`/plugins/${encodeURIComponent(pluginId)}?setup=1`}
								>Add credentials →</a
							>
						{:else}
							<p class="credmsg muted">Ask your server admin to add credentials under Plugins.</p>
						{/if}
					</div>
				</div>
			{:else if error}
				<p class="err">{error}</p>
			{/if}

			{#if error}
				<!-- The error is rendered above the feed. -->
			{:else if feedLoading || (items.length && !matchReady)}
				<Loading />
			{:else if !items.length}
				<p class="status muted">Nothing here.</p>
			{:else}
				{#if !visibleItems.length}
					<p class="status muted">Everything on this page is already in your library.</p>
				{/if}
				<div class="grid fluid-grid">
					{#each visibleItems as item (item.reference)}
						<BrowseCard {pluginId} {item} match={matches[item.reference]} onpreview={openPreview} />
					{/each}
				</div>

				{#if numPages > 1}
					<div class="pager">
						<Pagination
							jump={hasTotal}
							simple={!hasTotal}
							{page}
							pageCount={numPages}
							onnavigate={goPage}
						/>
					</div>
				{/if}
			{/if}
		{/if}
	{/if}
</div>

<style>
	.head {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--space-4);
		flex-wrap: wrap;
	}
	.pluginpick {
		min-width: 10rem;
	}
	.muted {
		color: var(--muted);
	}
	.status {
		padding: var(--space-6) 0;
	}
	.err {
		color: #e0566f;
		font-size: 0.9rem;
	}
	.fcount {
		margin-left: 0.35rem;
		font-size: 0.7rem;
		color: var(--muted);
	}
	.fintro {
		max-width: 65ch;
		color: var(--muted);
		font-size: 0.92rem;
		line-height: 1.55;
		margin: 0 0 var(--space-4);
	}
	.fintro b {
		color: var(--text);
	}
	.fform {
		display: flex;
		gap: var(--space-2);
		flex-wrap: wrap;
		margin-bottom: var(--space-3);
	}
	.fform input {
		flex: 1;
		min-width: 14rem;
		background: var(--surface);
		border: 1px solid var(--border);
		border-radius: var(--radius-sm);
		color: var(--text);
		font: inherit;
		font-size: 0.9rem;
		padding: 0.5rem 0.8rem;
	}
	.btn {
		font-size: 0.85rem;
		padding: var(--space-1) var(--space-3);
	}
	.btn.accent {
		background: var(--accent);
		border-color: var(--accent);
		color: #fff;
		font-weight: 600;
	}
	.btn.following {
		display: inline-flex;
		align-items: center;
		gap: 0.45rem;
		border-color: var(--good, #5fae7e);
		color: var(--good, #5fae7e);
		background: color-mix(in srgb, var(--good, #5fae7e) 10%, transparent);
	}
	.btn.following svg {
		width: 14px;
		height: 14px;
	}
	.btn.tiny {
		font-size: 0.78rem;
		padding: 0.2rem var(--space-2);
		color: var(--muted);
	}
	.btn.tiny:hover {
		border-color: #e0566f;
		background: rgba(224, 86, 111, 0.1);
		color: #e0566f;
	}
	.fmsg.good {
		color: var(--good, #5fae7e);
		font-size: 0.85rem;
		margin: 0 0 var(--space-3);
	}
	.ftesthead {
		color: var(--muted);
		font-size: 0.85rem;
		margin: var(--space-4) 0 var(--space-3);
	}
	.link {
		color: var(--accent);
	}
	.flist {
		margin-top: var(--space-6);
		border-top: 1px solid var(--border);
		padding-top: var(--space-4);
	}
	.flisthead {
		font-size: 0.75rem;
		letter-spacing: 0.08em;
		text-transform: uppercase;
		color: var(--muted);
		margin: 0 0 var(--space-2);
	}
	.frow {
		display: flex;
		align-items: center;
		gap: var(--space-3);
		padding: var(--space-2) 0;
	}
	.frow .fquery {
		font-family: var(--font-mono);
		font-size: 0.9rem;
	}
	.fnew {
		background: var(--accent);
		color: #fff;
		font-size: 0.7rem;
		font-weight: 600;
		border-radius: 999px;
		padding: 0.1rem 0.5rem;
		text-decoration: none;
	}
	.frow .unfollow {
		margin-left: auto;
	}
	.credneed {
		display: flex;
		gap: var(--space-4);
		align-items: flex-start;
		max-width: 34rem;
		margin: var(--space-8) auto;
		padding: var(--space-4) var(--space-5);
		border: 1px solid var(--border);
		border-radius: 10px;
		background: var(--bg-raised, rgba(255, 255, 255, 0.03));
	}
	.credneed svg {
		width: 22px;
		height: 22px;
		flex: none;
		margin-top: 2px;
		color: var(--muted);
	}
	.credtitle {
		font-weight: 600;
		margin-bottom: var(--space-1);
	}
	.credmsg {
		color: var(--muted);
		font-size: 0.9rem;
		margin-bottom: var(--space-2);
	}

	.feedtabs {
		display: flex;
		gap: var(--space-5);
		border-bottom: 1px solid var(--border);
		margin: 0 0 var(--space-5);
		flex-wrap: wrap;
	}
	.feedtab {
		all: unset;
		cursor: pointer;
		padding: var(--space-3) 0;
		margin-bottom: -1px;
		color: var(--muted);
		border-bottom: 2px solid transparent;
		transition:
			color var(--ease),
			border-color var(--ease);
	}
	.feedtab:hover {
		color: var(--text);
	}
	.feedtab.on {
		color: var(--accent);
		border-bottom-color: var(--accent);
	}

	.controls {
		display: flex;
		align-items: center;
		gap: var(--space-4);
		flex-wrap: wrap;
		margin-bottom: var(--space-5);
	}
	.chips {
		display: inline-flex;
		border: 1px solid var(--border);
		border-radius: var(--radius);
		background: var(--surface-2);
		overflow: hidden;
	}
	.chips .chip {
		border: none;
		border-radius: 0;
		background: transparent;
	}
	.chips .chip + .chip {
		border-left: 1px solid var(--border);
	}
	.chip {
		padding: 0.35rem 0.8rem;
		font-size: 0.85rem;
		color: var(--muted);
	}
	.chip:hover {
		color: var(--text);
	}
	.chip.on {
		background: var(--accent-soft);
		color: var(--accent);
	}
	.hideown {
		display: inline-flex;
		align-items: center;
		gap: 0.4rem;
	}
	.hideown svg {
		width: 0.95rem;
		height: 0.95rem;
	}
	.hideown.on {
		border-color: var(--accent);
	}
	.filter {
		position: relative;
		margin-left: auto;
	}
	.filter input {
		width: 14rem;
		max-width: 100%;
		padding: 0.4rem 1.6rem 0.4rem 0.7rem;
		border: 1px solid var(--border);
		border-radius: var(--radius-sm);
		background: var(--surface);
		color: var(--text);
		font-size: 0.88rem;
	}
	.filter input:focus {
		outline: none;
		border-color: var(--accent);
	}
	.filter .clear {
		position: absolute;
		right: 0.4rem;
		top: 50%;
		transform: translateY(-50%);
		border: none;
		background: transparent;
		color: var(--muted);
		cursor: pointer;
		font-size: 1.1rem;
		line-height: 1;
	}
	@media (max-width: 640px) {
		.feedtabs {
			flex-wrap: nowrap;
			overflow-x: auto;
			scrollbar-width: none;
		}
		.feedtab {
			white-space: nowrap;
		}
		.controls {
			display: grid;
			grid-template-columns: auto 1fr;
		}
		.chips {
			grid-column: 1 / -1;
			display: flex;
		}
		.chips .chip {
			flex: 1;
			padding-inline: 0.2rem;
		}
		.filter {
			margin-left: 0;
			min-width: 0;
		}
		.filter input {
			width: 100%;
			min-width: 0;
		}
	}

	.grid {
		--min-cols: 2;
		--max-cols: 8;
		--col-target: clamp(150px, 15vw, 220px);
		--grid-gap: var(--space-4);
		transition: opacity var(--ease);
	}
	.pager {
		display: flex;
		justify-content: center;
		margin-top: var(--space-6);
	}
</style>
