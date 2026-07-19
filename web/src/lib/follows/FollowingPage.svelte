<script>
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import {
		follows as followsApi,
		downloads as downloadsApi,
		plugins as pluginsApi,
		library as libraryApi,
		jobs as jobsApi,
		media,
	} from '$lib/api.js';
	import { loadFollows } from '$lib/follows/store.js';
	import { loadCover } from '$lib/browse/coverwarm.js';
	import { beginDownload, finishDownload } from '$lib/downloads.js';
	import { refreshKinds } from '$lib/kinds.js';
	import { monogram, tintOf } from '$lib/plugins/common.js';
	import { kindLabel } from '$lib/kinds.js';
	import BrowseCard from '$lib/browse/BrowseCard.svelte';
	import PageHeader from '$lib/components/ui/PageHeader.svelte';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';
	import DeleteConfirm from '$lib/components/ui/DeleteConfirm.svelte';
	import Loading from '$lib/components/ui/Loading.svelte';
	import ScheduleFooter from '$lib/components/ScheduleFooter.svelte';
	import { relativeTime } from '$lib/format.js';

	let { embedded = false } = $props();

	let rows = $state([]);
	let installed = $state(new Set());
	let pluginNames = $state({});
	let loading = $state(true);
	let error = $state('');
	let expanded = $state(new Set());
	let itemsBy = $state({});
	let itemsLoading = $state(new Set());
	let checking = $state(false);

	const lastCheckedAt = $derived(
		rows.reduce((latest, row) => Math.max(latest, row.last_checked_at ?? 0), 0) || null,
	);
	const nextCheckAt = $derived.by(() => {
		const next = new Date();
		next.setHours(3, 0, 0, 0);
		if (next.getTime() <= Date.now()) next.setDate(next.getDate() + 1);
		return next.toISOString();
	});

	onMount(refresh);
	async function refresh() {
		try {
			const [ws, plugins] = await Promise.all([loadFollows(), pluginsApi.list().catch(() => [])]);
			rows = ws;
			installed = new Set((plugins ?? []).map((p) => p.id));
			pluginNames = Object.fromEntries((plugins ?? []).map((p) => [p.id, p.name ?? p.id]));
			expanded = new Set(ws.map((w) => w.id));
			await Promise.all([...expanded].map(loadItems));
		} catch (e) {
			error = e?.message ?? String(e);
		} finally {
			loading = false;
		}
	}

	async function checkNow() {
		if (checking) return;
		checking = true;
		try {
			await followsApi.check();
		} catch (e) {
			error = e?.message ?? String(e);
		} finally {
			checking = false;
		}
	}

	let itemsError = $state({});
	async function loadItems(id) {
		itemsLoading = new Set(itemsLoading).add(id);
		try {
			const items = (await followsApi.items(id)) ?? [];
			itemsBy = { ...itemsBy, [id]: items };
			itemsError = { ...itemsError, [id]: '' };
			runOwnershipMatch(id, items);
		} catch (e) {
			itemsError = { ...itemsError, [id]: e?.message ?? String(e) };
		} finally {
			const s = new Set(itemsLoading);
			s.delete(id);
			itemsLoading = s;
		}
	}

	function toggle(id) {
		const s = new Set(expanded);
		if (s.has(id)) s.delete(id);
		else {
			s.add(id);
			if (!itemsBy[id]) loadItems(id);
		}
		expanded = s;
	}

	let dlBusy = $state(new Set());
	async function download(w, it) {
		const busyKey = `${w.id}:${it.reference}`;
		if (dlBusy.has(busyKey)) return;
		dlBusy = new Set(dlBusy).add(busyKey);
		const toastKey = `follow-dl-${w.plugin_id}-${it.reference}`;
		beginDownload(toastKey, it.item?.title ?? String(it.reference), 'Adding to library…');
		try {
			await followsApi.setItemState(w.id, it.reference, 'queued').catch(() => {});
			await loadItems(w.id);
			const r = await downloadsApi.create(w.plugin_id, {
				ref: it.reference,
				kind: w.kind,
				wait: true,
			});
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
			if (st === 'done') {
				await followsApi.setItemState(w.id, it.reference, 'downloaded').catch(() => {});
				refreshKinds();
				const id = result?.id;
				finishDownload(
					toastKey,
					true,
					id != null ? 'Tap to open in library' : 'Added to library',
					id != null ? { href: `/item/${id}` } : {},
				);
			} else {
				await followsApi.setItemState(w.id, it.reference, 'new').catch(() => {});
				finishDownload(
					toastKey,
					false,
					st === 'failed'
						? (result?.error ?? 'Download failed')
						: 'Still downloading on the server — check back soon',
				);
			}
			await loadItems(w.id);
			rows = await loadFollows();
		} catch (e) {
			await followsApi.setItemState(w.id, it.reference, 'new').catch(() => {});
			finishDownload(toastKey, false, e?.message ?? String(e));
			await loadItems(w.id);
		} finally {
			const s = new Set(dlBusy);
			s.delete(busyKey);
			dlBusy = s;
		}
	}
	async function dismiss(w, it) {
		await followsApi.setItemState(w.id, it.reference, 'skipped').catch(() => {});
		await loadItems(w.id);
		rows = await loadFollows();
	}
	async function downloadAll(w) {
		const news = (itemsBy[w.id] ?? []).filter((i) => i.state === 'new');
		for (const it of news) {
			await download(w, it);
		}
	}
	async function dismissAll(w) {
		await followsApi.dismissAll(w.id).catch(() => {});
		await loadItems(w.id);
		rows = await loadFollows();
	}

	let removeTarget = $state(null);
	async function removeWatch() {
		if (!removeTarget) return;
		await followsApi.remove(removeTarget.id).catch(() => {});
		removeTarget = null;
		rows = await loadFollows();
	}

	const dormant = (w) => !installed.has(w.plugin_id);
	const newItems = (id) => (itemsBy[id] ?? []).filter((i) => i.state === 'new');
	const shownItems = (id) => (itemsBy[id] ?? []).filter((i) => i.state !== 'skipped' && i.item);

	const ago = (ts) => (ts ? relativeTime(ts) : 'never');
	const iconUrl = (id) => media.pluginIcon(id);
	const openPreview = (w, it) =>
		goto(
			`/source/${encodeURIComponent(w.plugin_id)}/${encodeURIComponent(it.reference)}?kind=${encodeURIComponent(w.kind)}`,
		);

	let matchesBy = $state({});
	async function runOwnershipMatch(wid, items) {
		const w = rows.find((x) => x.id === wid);
		const its = items.filter((i) => i.item).map((i) => ({ ref: i.reference, card: i.item }));
		if (!w || !its.length) return;
		try {
			await Promise.all(
				its.map((i) => loadCover(media.pluginImage(w.plugin_id, i.card.cover_url))),
			);
			const v = await libraryApi.match(
				its.map((i) => ({
					source_url: i.card.source_url,
					cover_url: i.card.cover_url,
					page_count: i.card.page_count,
				})),
			);
			if (!Array.isArray(v)) return;
			const next = { ...(matchesBy[wid] ?? {}) };
			v.forEach((m, i) => {
				if (m?.owned_item_id != null || m?.likely_item_id != null) next[its[i].ref] = m;
			});
			matchesBy = { ...matchesBy, [wid]: next };
		} catch {
			/* ignored */
		}
	}
</script>

<div class="app-page" class:embedded>
	{#if !embedded}<PageHeader title="Following" />{/if}
	{#if error}<p class="err">{error}</p>{/if}

	{#if loading}
		<Loading />
	{:else if !rows.length}
		<EmptyState
			title="Follow an artist or a search"
			message="Open a source's Browse page and use its Follow tab. New uploads matching a followed search land here for review — nothing downloads until you say so."
		>
			{#snippet icon()}
				<svg
					viewBox="0 0 24 24"
					fill="none"
					stroke="currentColor"
					stroke-width="1.6"
					stroke-linecap="round"
					stroke-linejoin="round"><circle cx="12" cy="12" r="9" /><path d="M12 7v5l3 3" /></svg
				>
			{/snippet}
		</EmptyState>
	{:else}
		<div class="list">
			{#each rows as w (w.id)}
				<div class="wrow" class:dormant={dormant(w)}>
					<button
						class="wmain"
						type="button"
						onclick={() => toggle(w.id)}
						aria-expanded={expanded.has(w.id)}
					>
						<span class="tile" style={tintOf(w.plugin_id)} aria-hidden="true">
							{monogram(pluginNames[w.plugin_id] ?? w.plugin_id)}
							<img
								class="ticon"
								src={iconUrl(w.plugin_id)}
								alt=""
								onerror={(e) => (e.currentTarget.style.display = 'none')}
							/>
						</span>
						<span class="wtext">
							<span class="wtitle">
								<span class="wquery">{w.query}</span>
								{#if w.new_count > 0}<span class="newpill">{w.new_count} new</span>{/if}
								{#if dormant(w)}<span class="dormpill">plugin not installed</span>{/if}
								{#if w.last_error && !dormant(w)}<span class="warnline">⚠ last check failed</span
									>{/if}
							</span>
							<span class="wmeta">
								{pluginNames[w.plugin_id] ?? w.plugin_id} · downloads to
								<b>{kindLabel(w.kind)}</b> · checked {ago(w.last_checked_at)}
							</span>
							{#if w.last_error && !dormant(w)}
								<span class="werror">{w.last_error}</span>
							{/if}
						</span>
					</button>
					<div class="wacts">
						<button class="unfollow" type="button" onclick={() => (removeTarget = w)}>
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
							<span>Unfollow</span>
						</button>
					</div>
				</div>

				{#if expanded.has(w.id)}
					<div class="wexpand" class:slim={!itemsLoading.has(w.id) && !shownItems(w.id).length}>
						{#if itemsLoading.has(w.id) && !itemsBy[w.id]}
							<Loading />
						{:else if itemsError[w.id]}
							<p class="status slimline warn">
								Couldn't load this follow's items ({itemsError[w.id]})
								<button class="btn tiny" type="button" onclick={() => loadItems(w.id)}>Retry</button
								>
							</p>
						{:else if !shownItems(w.id).length}
							<p class="status muted slimline">
								{#if w.new_count > 0}
									{w.new_count} new recorded but not loaded
									<button class="btn tiny" type="button" onclick={() => loadItems(w.id)}
										>Retry</button
									>
								{:else if w.last_checked_at}
									No new items
								{:else}
									Baseline set — new uploads show up from the next check
								{/if}
							</p>
						{:else}
							<div class="bar">
								<span class="n">
									{#if newItems(w.id).length}<b>{newItems(w.id).length} new</b> to review{:else}Recent
										discoveries{/if}
								</span>
								{#if newItems(w.id).length}
									<span class="bacts">
										<button class="btn small accent" type="button" onclick={() => downloadAll(w)}
											>Download all</button
										>
										<button class="btn small" type="button" onclick={() => dismissAll(w)}
											>Dismiss all</button
										>
									</span>
								{/if}
							</div>
							<div class="grid fluid-grid">
								{#each shownItems(w.id) as it (it.reference)}
									{@const m = matchesBy[w.id]?.[it.reference]}
									<BrowseCard
										pluginId={w.plugin_id}
										item={it.item}
										match={m}
										onpreview={() => openPreview(w, it)}
									>
										{#snippet actions()}
											{#if !m?.owned_item_id}
												<button
													class="btn tiny accent"
													type="button"
													disabled={dlBusy.has(`${w.id}:${it.reference}`)}
													onclick={() => download(w, it)}
												>
													{dlBusy.has(`${w.id}:${it.reference}`) ? 'Downloading…' : 'Download'}
												</button>
											{/if}
											<button class="btn tiny" type="button" onclick={() => dismiss(w, it)}
												>Dismiss</button
											>
										{/snippet}
									</BrowseCard>
								{/each}
							</div>
						{/if}
					</div>
				{/if}
			{/each}
		</div>
	{/if}

	<ScheduleFooter
		updatedLabel="Searches checked"
		updatedAt={lastCheckedAt}
		nextLabel="Next check"
		nextAt={nextCheckAt}
		actionLabel="Check now"
		busy={checking}
		onaction={checkNow}
	/>
</div>

{#if removeTarget}
	<DeleteConfirm
		heading="Stop following?"
		message={`“${removeTarget.query}” will no longer be checked. Already-downloaded items stay in your library.`}
		verb="Unfollow"
		busyLabel="Stopping…"
		cooldown={0}
		onConfirm={removeWatch}
		onClose={() => (removeTarget = null)}
	/>
{/if}

<style>
	.app-page.embedded {
		padding: 0;
	}
	.muted {
		color: var(--muted);
	}
	.err {
		color: #e0566f;
		font-size: 0.9rem;
		margin-bottom: var(--space-4);
	}
	.btn.accent {
		background: var(--accent);
		border-color: var(--accent);
		color: #fff;
		font-weight: 600;
	}
	.btn.small {
		font-size: 0.85rem;
		padding: var(--space-1) var(--space-3);
	}
	.btn.tiny {
		font-size: 0.78rem;
		padding: 0.2rem var(--space-2);
	}
	.unfollow {
		display: inline-flex;
		align-items: center;
		gap: var(--space-2);
		padding: var(--space-1) var(--space-3);
		font-size: 0.85rem;
		color: var(--muted);
	}
	.unfollow svg {
		width: 1rem;
		height: 1rem;
	}
	.unfollow:hover {
		border-color: #e0566f;
		background: rgba(224, 86, 111, 0.1);
		color: #e0566f;
	}

	.list {
		display: flex;
		flex-direction: column;
	}
	.wrow {
		display: flex;
		align-items: flex-start;
		gap: var(--space-3);
		padding: var(--space-4) var(--space-1);
		border-bottom: 1px solid var(--border);
	}
	.wrow.dormant {
		opacity: 0.55;
	}
	.wmain {
		flex: 1;
		min-width: 0;
		display: flex;
		gap: var(--space-4);
		align-items: flex-start;
		background: none;
		border: none;
		padding: 0;
		text-align: left;
		color: inherit;
		font: inherit;
		cursor: pointer;
	}
	.tile {
		flex: none;
		width: 44px;
		height: 44px;
		border-radius: var(--radius);
		display: flex;
		align-items: center;
		justify-content: center;
		font-family: var(--font-display);
		font-size: 1.15rem;
		color: #fff;
		position: relative;
		overflow: hidden;
	}
	.ticon {
		position: absolute;
		inset: 0;
		width: 100%;
		height: 100%;
		object-fit: cover;
	}
	.wtext {
		display: flex;
		flex-direction: column;
		gap: 0.15rem;
		min-width: 0;
	}
	.wtitle {
		display: flex;
		align-items: center;
		gap: var(--space-2);
		flex-wrap: wrap;
	}
	.wquery {
		font-weight: 600;
		font-family: var(--font-mono);
		font-size: 0.98rem;
	}
	.newpill {
		background: var(--accent);
		color: #fff;
		font-size: 0.72rem;
		font-weight: 600;
		border-radius: 999px;
		padding: 0.12rem 0.55rem;
	}
	.dormpill {
		border: 1px solid var(--border);
		color: var(--muted);
		font-size: 0.72rem;
		border-radius: 999px;
		padding: 0.12rem 0.55rem;
	}
	.warnline {
		color: #d9a13d;
		font-size: 0.8rem;
	}
	.wmeta {
		color: var(--muted);
		font-size: 0.82rem;
	}
	.wmeta b {
		color: var(--text);
		font-weight: 500;
	}
	.werror {
		color: #d9a13d;
		font-size: 0.82rem;
	}
	.wacts {
		flex: none;
		display: flex;
		gap: var(--space-2);
		padding-top: 0.2rem;
	}

	.wexpand {
		margin: var(--space-3) 0 var(--space-4);
		border: 1px solid var(--border);
		border-radius: var(--radius);
		background: var(--surface);
		padding: var(--space-4);
	}
	.wexpand.slim {
		padding: var(--space-2) var(--space-4);
	}
	.slimline {
		font-size: 0.82rem;
		margin: 0;
		display: flex;
		align-items: center;
		gap: var(--space-2);
	}
	.slimline.warn {
		color: #d9a13d;
	}
	.bar {
		display: flex;
		align-items: center;
		justify-content: space-between;
		margin-bottom: var(--space-3);
	}
	.bar .n {
		font-size: 0.86rem;
		color: var(--muted);
	}
	.bar .n b {
		color: var(--text);
	}
	.bacts {
		display: flex;
		gap: var(--space-2);
	}
	.status {
		font-size: 0.88rem;
		margin: 0;
	}

	.grid {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(120px, 1fr));
		gap: var(--space-3);
	}
</style>
