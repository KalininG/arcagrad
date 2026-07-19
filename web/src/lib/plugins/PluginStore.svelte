<script>
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import { credentials as credsApi, kinds as kindsApi } from '$lib/api.js';
	import { currentUser } from '$lib/session.js';
	import { CAP_LABEL, monogram, tintOf } from '$lib/plugins/common.js';
	import {
		loadCatalog,
		reloadCatalog,
		installFromFile,
		update as updatePlugin,
		installs,
		realVersion,
		realAuthor,
	} from '$lib/plugins/catalog.js';
	import { plugins as pluginsApi, media } from '$lib/api.js';
	import Loading from '$lib/components/ui/Loading.svelte';
	import PageHeader from '$lib/components/ui/PageHeader.svelte';
	import DeleteConfirm from '$lib/components/ui/DeleteConfirm.svelte';
	import InstallSheet from '$lib/plugins/InstallSheet.svelte';

	const isAdmin = $derived($currentUser?.role === 'admin');

	let catalog = $state([]);
	let enabledIds = $state(new Set());
	let credSources = $state(new Set());
	let loading = $state(true);
	let error = $state('');
	async function refreshGates() {
		const [kinds, creds] = await Promise.all([
			kindsApi.list().catch(() => []),
			credsApi.list().catch(() => []),
		]);
		const rows = await Promise.all((kinds ?? []).map((k) => kindsApi.plugins(k.kind)));
		const on = new Set();
		for (const row of rows) for (const r of row ?? []) if (r.enabled) on.add(r.id);
		enabledIds = on;
		credSources = new Set((creds ?? []).map((c) => c.source));
	}
	onMount(async () => {
		try {
			catalog = await loadCatalog();
			if (!catalog.some((e) => e.installed) && tab === 'installed') tab = 'discover';
			await refreshGates();
		} catch (e) {
			error = e?.message ?? String(e);
		} finally {
			loading = false;
		}
	});
	const needsKinds = (entry) => !enabledIds.has(entry.id);
	const needsCreds = (entry) => !!entry.auth?.fields?.length && !credSources.has(entry.source);

	const TAB_IDS = ['installed', 'discover', 'repos'];
	function initTab() {
		try {
			const t = new URLSearchParams(location.search).get('tab');
			return TAB_IDS.includes(t) ? t : 'installed';
		} catch {
			return 'installed';
		}
	}
	let tab = $state(initTab());
	function setTab(id) {
		if (id === tab) return;
		tab = id;
		const url = new URL(location.href);
		url.searchParams.set('tab', id);
		goto(url, { replaceState: true, keepFocus: true, noScroll: true });
	}
	let showNsfw = $state(false);
	$effect(() => {
		try {
			showNsfw = localStorage.getItem('arca:show-nsfw-plugins') === '1';
		} catch {
			/* ignored */
		}
	});
	function toggleNsfw() {
		showNsfw = !showNsfw;
		try {
			localStorage.setItem('arca:show-nsfw-plugins', showNsfw ? '1' : '0');
		} catch {
			/* ignored */
		}
	}
	const installedList = $derived(catalog.filter((e) => $installs[e.id]));
	const discoverList = $derived(catalog.filter((e) => !$installs[e.id] && (showNsfw || !e.nsfw)));
	const shown = $derived(tab === 'installed' ? installedList : discoverList);
	const installedCount = $derived(Object.keys($installs).length);

	let sheetEntry = $state(null);
	function openSheet(entry, ev) {
		ev.preventDefault();
		ev.stopPropagation();
		sheetEntry = entry;
	}
	function onInstalled() {
		const entry = sheetEntry;
		sheetEntry = null;
		refreshGates().catch(() => {});
		if (entry?.auth?.fields?.length) goto(`/plugins/${encodeURIComponent(entry.id)}?setup=1`);
		else tab = 'installed';
	}

	const detailHref = (e) => `/plugins/${encodeURIComponent(e.id)}`;
	const capsLine = (e) => e.capabilities.map((c) => CAP_LABEL[c] ?? c).join(' · ');

	let fileInput = $state(null);
	let fileBusy = $state(false);
	let fileErr = $state('');
	async function onFilePicked(ev) {
		const file = ev.currentTarget.files?.[0];
		ev.currentTarget.value = '';
		if (!file || fileBusy) return;
		fileBusy = true;
		fileErr = '';
		try {
			const id = await installFromFile(file);
			catalog = await loadCatalog();
			goto(`/plugins/${encodeURIComponent(id)}`);
		} catch (e) {
			fileErr = e?.message ?? String(e);
		} finally {
			fileBusy = false;
		}
	}

	let repos = $state([]);
	let repoUrl = $state('');
	let repoBusy = $state(false);
	let repoErr = $state('');
	onMount(async () => {
		repos = await pluginsApi.repos().catch(() => []);
	});
	async function addRepo() {
		const url = repoUrl.trim();
		if (!url || repoBusy) return;
		repoBusy = true;
		repoErr = '';
		try {
			await pluginsApi.addRepo(url);
			repoUrl = '';
			[repos, catalog] = await Promise.all([pluginsApi.repos(), reloadCatalog()]);
		} catch (e) {
			repoErr = e?.message ?? String(e);
		} finally {
			repoBusy = false;
		}
	}
	let removeTarget = $state(null);
	function askRemoveRepo(r) {
		const count = catalog.filter((e) => e.installed && e.repo_url === r.url).length;
		removeTarget = { url: r.url, name: r.name ?? repoHost(r.url), count };
	}
	const removeMessage = $derived(
		removeTarget
			? removeTarget.count
				? `${removeTarget.count} ${removeTarget.count === 1 ? 'plugin' : 'plugins'} installed from “${removeTarget.name}” will be uninstalled with it. Credentials and kind assignments are kept if you reinstall later.`
				: `“${removeTarget.name}” will be removed. Nothing is currently installed from it.`
			: '',
	);
	async function removeRepo() {
		if (repoBusy || !removeTarget) return;
		repoBusy = true;
		repoErr = '';
		try {
			await pluginsApi.removeRepo(removeTarget.url);
			[repos, catalog] = await Promise.all([pluginsApi.repos(), reloadCatalog()]);
			removeTarget = null;
		} catch (e) {
			repoErr = e?.message ?? String(e);
			removeTarget = null;
		} finally {
			repoBusy = false;
		}
	}
	let checking = $state(false);
	let checkMsg = $state('');
	async function checkForUpdates() {
		if (checking) return;
		checking = true;
		checkMsg = '';
		try {
			await pluginsApi.refreshRepos();
			[repos, catalog] = await Promise.all([pluginsApi.repos(), reloadCatalog()]);
			const n = catalog.filter((e) => e.update_available).length;
			if (n) {
				tab = 'installed';
				checkMsg = `${n} ${n === 1 ? 'update' : 'updates'} available`;
			} else {
				checkMsg = 'Everything is up to date';
			}
			setTimeout(() => (checkMsg = ''), 5000);
		} catch (e) {
			checkMsg = e?.message ?? String(e);
		} finally {
			checking = false;
		}
	}

	let updating = $state(new Set());
	async function doUpdate(entry, ev) {
		ev.preventDefault();
		ev.stopPropagation();
		if (updating.has(entry.id)) return;
		updating = new Set([...updating, entry.id]);
		try {
			await updatePlugin(entry.id);
			catalog = await loadCatalog();
		} catch (e) {
			error = e?.message ?? String(e);
		} finally {
			const next = new Set(updating);
			next.delete(entry.id);
			updating = next;
		}
	}
	const repoHost = (url) => {
		try {
			return new URL(url).host;
		} catch {
			return url;
		}
	};
</script>

<div class="app-page store">
	{#if !isAdmin}
		<p class="denied">This area is available to administrators only.</p>
	{:else}
		<PageHeader title="Plugins">
			{#snippet actions()}
				<p class="meta">{installedCount} of {catalog.length} installed</p>
				<div class="actrow">
					<button
						class="filebtn"
						type="button"
						disabled={fileBusy}
						onclick={() => fileInput?.click()}
					>
						{fileBusy ? 'Installing…' : 'Install from file…'}
					</button>
					<button class="checkbtn" type="button" disabled={checking} onclick={checkForUpdates}>
						{checking ? 'Checking…' : 'Check for updates'}
					</button>
				</div>
				<label class="nsfwtoggle" title="Show adult (NSFW) sources in Discover">
					<input type="checkbox" checked={showNsfw} onchange={toggleNsfw} />
					Show NSFW
				</label>
				{#if checkMsg}<p class="checkmsg">{checkMsg}</p>{/if}
				<input type="file" accept=".wasm" bind:this={fileInput} onchange={onFilePicked} hidden />
				{#if fileErr}<p class="err">{fileErr}</p>{/if}
			{/snippet}
		</PageHeader>

		{#if loading}
			<Loading label="Loading plugins…" />
		{:else if error}
			<p class="err">{error}</p>
		{:else}
			<nav class="tabs" aria-label="Plugin sections">
				<button
					class="tab"
					class:active={tab === 'installed'}
					type="button"
					onclick={() => setTab('installed')}
				>
					Installed <span class="tcount">{installedList.length}</span>
				</button>
				<button
					class="tab"
					class:active={tab === 'discover'}
					type="button"
					onclick={() => setTab('discover')}
				>
					Discover <span class="tcount">{discoverList.length}</span>
				</button>
				<button
					class="tab"
					class:active={tab === 'repos'}
					type="button"
					onclick={() => setTab('repos')}
				>
					Repositories <span class="tcount">{repos.length}</span>
				</button>
			</nav>

			{#if tab !== 'repos'}
				<section class="group">
					{#each shown as entry (entry.id)}
						{@const inst = $installs[entry.id]}
						{@const ver = realVersion(inst?.version ?? entry.version)}
						{@const author = realAuthor(entry.author)}
						<a class="row" href={detailHref(entry)}>
							<span class="tile" style={tintOf(entry.id)} aria-hidden="true">
								{monogram(entry.name)}
								{#if entry.icon}
									<img
										class="ticon"
										src={media.pluginIcon(entry.icon)}
										alt=""
										loading="lazy"
										onerror={(e) => (e.currentTarget.style.display = 'none')}
									/>
								{/if}
							</span>
							<span class="body">
								<span class="titleline">
									<span class="pname">{entry.name}</span>
									{#if entry.origin === 'bundled'}<span class="bundled">Bundled</span>
									{:else if entry.origin}<span class="origin o-muted">{entry.origin}</span>{/if}
									{#if entry.nsfw}<span class="nsfw">NSFW</span>{/if}
									{#if entry.metadata_status === 'legacy'}<span class="state st-muted"
											>legacy manifest</span
										>{/if}
									{#if entry.last_error}<span class="state st-bad">Failed to load</span>{/if}
									{#if inst && !entry.last_error}
										{@const warns = [
											needsKinds(entry) && 'Not enabled for any kind',
											needsCreds(entry) && 'No credentials set',
										].filter(Boolean)}
										{#if warns.length}
											<span class="state st-warn">
												<svg
													viewBox="0 0 24 24"
													fill="none"
													stroke="currentColor"
													stroke-width="2"
													stroke-linecap="round"
													stroke-linejoin="round"
													aria-hidden="true"
													><path
														d="M10.3 3.9 1.8 18a2 2 0 0 0 1.7 3h17a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0z"
													/><path d="M12 9v4M12 17h.01" /></svg
												>
												{warns.join(' · ')}
											</span>
										{/if}
									{/if}
								</span>
								{#if author}<span class="byline">@{author}</span>{/if}
								{#if entry.description}<span class="sub">{entry.description}</span>{/if}
								<span class="caps">{capsLine(entry)}</span>
							</span>
							<span class="side">
								{#if inst && entry.update_available}
									<button
										class="get"
										type="button"
										disabled={updating.has(entry.id)}
										onclick={(e) => doUpdate(entry, e)}
									>
										{updating.has(entry.id) ? 'Updating…' : 'Update'}
									</button>
									<span class="ver">v{ver} → v{entry.update_available}</span>
								{:else if inst}
									<span class="installed">
										<svg
											viewBox="0 0 20 20"
											fill="none"
											stroke="currentColor"
											stroke-width="2.5"
											stroke-linecap="round"
											stroke-linejoin="round"><path d="M4 10l4 4 8-8" /></svg
										>
										Installed
									</span>
									{#if ver}<span class="ver">v{ver}</span>{/if}
								{:else}
									<button class="get" type="button" onclick={(e) => openSheet(entry, e)}>Get</button
									>
									{#if ver}<span class="ver">v{ver}</span>{/if}
								{/if}
							</span>
						</a>
					{:else}
						{#if tab === 'installed'}
							<p class="tempty">
								Nothing installed yet — find plugins in
								<button class="tlink" type="button" onclick={() => setTab('discover')}
									>Discover</button
								>.
							</p>
						{:else}
							<p class="tempty">
								Everything available is installed. Add a
								<button class="tlink" type="button" onclick={() => setTab('repos')}
									>repository</button
								>
								to discover more.
							</p>
						{/if}
					{/each}
				</section>
			{:else}
				<section class="repos">
					<p class="rdesc">
						Add plugin repositories to find and install community-made plugins. Arcagrad checks each
						plugin before installing it.
					</p>
					{#each repos as r (r.url)}
						<div class="reporow">
							<span class="rbody">
								<span class="rname">{r.name ?? repoHost(r.url)}</span>
								<span class="rurl">{r.url}</span>
								{#if r.last_error}
									<span class="rerr">Last fetch failed: {r.last_error}</span>
								{/if}
							</span>
							<button
								class="rremove"
								type="button"
								disabled={repoBusy}
								onclick={() => askRemoveRepo(r)}
							>
								<svg
									viewBox="0 0 24 24"
									fill="none"
									stroke="currentColor"
									stroke-width="2"
									stroke-linecap="round"
									stroke-linejoin="round"
									aria-hidden="true"
									><path
										d="M3 6h18M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2m3 0v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6h14"
									/><path d="M10 11v6M14 11v6" /></svg
								>
								Remove repository
							</button>
						</div>
					{:else}
						<p class="rempty">
							No repositories configured. Add one to see its plugins in the store.
						</p>
					{/each}
					<form
						class="repoadd"
						onsubmit={(e) => {
							e.preventDefault();
							addRepo();
						}}
					>
						<input
							type="url"
							placeholder="https://example.com/plugins/index.json"
							bind:value={repoUrl}
							disabled={repoBusy}
						/>
						<button class="radd" type="submit" disabled={repoBusy || !repoUrl.trim()}>Add</button>
					</form>
					{#if repoErr}<p class="err">{repoErr}</p>{/if}
				</section>
			{/if}
		{/if}
	{/if}
</div>

{#if sheetEntry}
	<InstallSheet entry={sheetEntry} onclose={() => (sheetEntry = null)} oninstalled={onInstalled} />
{/if}

{#if removeTarget}
	<DeleteConfirm
		heading="Remove this repository?"
		message={removeMessage}
		verb="Remove"
		busyLabel="Removing…"
		busy={repoBusy}
		onConfirm={removeRepo}
		onClose={() => !repoBusy && (removeTarget = null)}
	/>
{/if}

<style>
	.meta {
		margin: 0;
		font-size: 0.8rem;
		color: var(--muted);
	}
	.denied {
		color: var(--muted);
		font-size: 0.9rem;
	}

	.actrow {
		display: flex;
		gap: var(--space-2);
		align-items: center;
	}
	.checkbtn {
		padding: var(--space-1) var(--space-3);
		font-size: 0.85rem;
		font-weight: 600;
		color: #fff;
		background: var(--accent);
		border-color: var(--accent);
	}
	.checkbtn:hover:not(:disabled) {
		opacity: 0.88;
	}
	.checkbtn:disabled {
		opacity: 0.6;
		cursor: default;
	}
	.checkmsg {
		margin: 0;
		font-size: 0.76rem;
		color: var(--muted);
	}
	.filebtn {
		padding: var(--space-1) var(--space-3);
		border-style: dashed;
		background: transparent;
		font-size: 0.85rem;
		color: var(--muted);
	}
	.filebtn:disabled {
		cursor: not-allowed;
		opacity: 0.55;
	}

	.group {
		margin-bottom: var(--space-6);
	}
	.row {
		display: flex;
		align-items: center;
		gap: var(--space-4);
		padding: var(--space-4) 0;
		color: inherit;
		border-bottom: 1px solid var(--border);
	}
	.row:hover .pname {
		color: var(--accent);
	}
	.err {
		margin: 0;
		color: var(--bad, #e5484d);
		font-size: 0.85rem;
	}
	.byline {
		font-size: 0.74rem;
		color: var(--muted);
	}
	.bundled {
		padding: 0.08rem 0.5rem;
		border: 1px solid color-mix(in srgb, var(--good, #46a758) 55%, var(--border));
		border-radius: 9999px;
		font-size: 0.66rem;
		font-weight: 600;
		letter-spacing: 0.04em;
		color: var(--good, #46a758);
	}
	.nsfw {
		padding: 0.08rem 0.5rem;
		border: 1px solid color-mix(in srgb, var(--bad, #e5484d) 55%, var(--border));
		border-radius: 9999px;
		font-size: 0.66rem;
		font-weight: 600;
		letter-spacing: 0.04em;
		color: var(--bad, #e5484d);
	}
	.ticon {
		position: absolute;
		inset: 0;
		width: 100%;
		height: 100%;
		object-fit: cover;
	}
	.tile {
		position: relative;
		overflow: hidden;
		flex: 0 0 auto;
		width: 52px;
		height: 52px;
		border-radius: 24%;
		display: inline-flex;
		align-items: center;
		justify-content: center;
		font-family: var(--font-display);
		font-weight: 700;
		font-size: 1.25rem;
	}
	.body {
		flex: 1;
		min-width: 0;
		display: flex;
		flex-direction: column;
		gap: 2px;
	}
	.titleline {
		display: flex;
		align-items: baseline;
		gap: var(--space-2);
		flex-wrap: wrap;
	}
	.pname {
		font-family: var(--font-display);
		font-weight: 700;
		font-size: 1.02rem;
		transition: color var(--ease);
	}
	.origin {
		font-size: 0.72rem;
	}
	.origin.o-bundled {
		color: var(--accent);
	}
	.origin.o-muted {
		color: var(--muted);
	}
	.state {
		font-size: 0.72rem;
	}
	.state.st-muted {
		color: var(--muted);
	}
	.state.st-bad {
		color: var(--bad, #e5484d);
	}
	.state.st-warn {
		display: inline-flex;
		align-items: center;
		gap: 0.3rem;
		color: #d29922;
	}
	.state.st-warn svg {
		width: 0.8rem;
		height: 0.8rem;
		flex: 0 0 auto;
	}
	.sub {
		font-size: 0.8rem;
		color: var(--muted);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.caps {
		font-size: 0.72rem;
		color: var(--muted);
		opacity: 0.8;
	}
	.side {
		flex: 0 0 auto;
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: 3px;
		min-width: 4.6rem;
	}
	.get {
		padding: var(--space-1) var(--space-3);
		font-size: 0.85rem;
		font-weight: 600;
		color: var(--accent);
		background: var(--accent-soft);
		border-color: color-mix(in srgb, var(--accent) 35%, var(--border));
	}
	.get:hover {
		background: var(--accent);
		color: #fff;
	}
	.installed {
		display: inline-flex;
		align-items: center;
		gap: 0.35rem;
		font-size: 0.8rem;
		color: var(--muted);
	}
	.installed svg {
		width: 0.8rem;
		height: 0.8rem;
		color: var(--good, #46a758);
	}
	.ver {
		font-size: 0.7rem;
		color: var(--muted);
		font-variant-numeric: tabular-nums;
	}

	.tabs {
		display: flex;
		gap: var(--space-4);
		border-bottom: 1px solid var(--border);
		margin-bottom: var(--space-4);
	}
	.nsfwtoggle {
		display: inline-flex;
		align-items: center;
		gap: var(--space-2);
		white-space: nowrap;
		font-size: 0.82rem;
		color: var(--muted);
		cursor: pointer;
		user-select: none;
	}
	.nsfwtoggle input {
		cursor: pointer;
		accent-color: var(--bad, #e5484d);
	}
	.tab {
		all: unset;
		cursor: pointer;
		padding: var(--space-2) 0;
		font-size: 0.9rem;
		color: var(--muted);
		border-bottom: 2px solid transparent;
		margin-bottom: -1px;
	}
	.tab:hover {
		color: var(--text);
	}
	.tab.active {
		color: var(--text);
		border-bottom-color: var(--accent);
	}
	.tcount {
		margin-left: 0.15rem;
		font-size: 0.72rem;
		font-variant-numeric: tabular-nums;
		opacity: 0.75;
	}
	.tempty {
		margin: 0;
		padding: var(--space-5) 0;
		font-size: 0.86rem;
		color: var(--muted);
	}
	.tlink {
		all: unset;
		cursor: pointer;
		color: var(--accent);
	}
	.tlink:hover {
		text-decoration: underline;
	}

	.repos {
		margin-top: var(--space-6);
		display: flex;
		flex-direction: column;
		gap: var(--space-3);
	}
	.rdesc {
		margin: 0;
		font-size: 0.84rem;
		color: var(--muted);
		max-width: 70ch;
	}
	.reporow {
		display: flex;
		align-items: center;
		gap: var(--space-4);
		border: 1px solid var(--border);
		border-radius: var(--radius);
		background: var(--surface);
		padding: var(--space-3) var(--space-4);
	}
	.rbody {
		flex: 1;
		min-width: 0;
		display: flex;
		flex-direction: column;
		gap: 2px;
	}
	.rname {
		font-weight: 600;
		font-size: 0.88rem;
	}
	.rurl {
		font-family: var(--font-mono);
		font-size: 0.72rem;
		color: var(--muted);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.rerr {
		font-size: 0.72rem;
		color: var(--bad, #e5484d);
		word-break: break-all;
	}
	.rremove {
		flex: 0 0 auto;
		display: inline-flex;
		align-items: center;
		gap: var(--space-2);
		padding: var(--space-1) var(--space-3);
		font-size: 0.85rem;
		color: var(--muted);
	}
	.rremove svg {
		width: 0.85rem;
		height: 0.85rem;
	}
	.rremove:hover:not(:disabled) {
		border-color: #e0566f;
		background: rgba(224, 86, 111, 0.1);
		color: #e0566f;
	}
	.rempty {
		margin: 0;
		font-size: 0.82rem;
		color: var(--muted);
	}
	.repoadd {
		display: flex;
		gap: var(--space-2);
		align-items: center;
	}
	.repoadd input {
		flex: 1;
		min-width: 0;
		border-radius: var(--radius-sm);
		padding: 0.45rem 0.6rem;
		font-size: 0.84rem;
	}
	.radd {
		padding: var(--space-1) var(--space-3);
		font-size: 0.85rem;
		background: var(--accent);
		border-color: var(--accent);
		color: #fff;
		font-weight: 600;
	}
	.radd:disabled,
	.rremove:disabled {
		opacity: 0.55;
		cursor: not-allowed;
	}

	@media (max-width: 640px) {
		.row {
			flex-wrap: wrap;
		}
		.body {
			flex-basis: calc(100% - 52px - var(--space-4));
		}
		.side {
			margin-left: calc(52px + var(--space-4));
			flex-direction: row;
			gap: var(--space-3);
		}
	}
</style>
