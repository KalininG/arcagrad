<script>
	import { onMount } from 'svelte';
	import PageHeader from '$lib/components/ui/PageHeader.svelte';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';
	import Dropdown from '$lib/components/ui/Dropdown.svelte';
	import {
		items as itemsApi,
		kinds as kindsApi,
		jobs as jobsApi,
		downloads as downloadsApi,
		credentials as credsApi,
		media,
		ApiError,
	} from '$lib/api.js';
	import { currentUser } from '$lib/session.js';
	import { kinds as kindsStore, ensureKinds, refreshKinds, kindLabel } from '$lib/kinds.js';

	const isAdmin = $derived($currentUser?.role === 'admin');

	let mode = $state('computer');
	let kindInput = $state('');
	let kindDefaulted = false;
	$effect(() => {
		if (!kindDefaulted && !kindInput && $kindsStore.length) {
			kindDefaulted = true;
			kindInput = $kindsStore[0].kind;
		}
	});
	let dragover = $state(false);
	let fileInput;

	onMount(ensureKinds);

	let kindPlugins = $state([]);
	let pluginsLoaded = $state(false);
	let autoScrapeId = $state('');

	const scrapePlugins = $derived(
		kindPlugins.filter(
			(p) =>
				p.enabled &&
				p.capabilities?.includes('scrape') &&
				!p.auth?.required_for?.includes('scrape'),
		),
	);
	const autoScrapeOptions = $derived([
		{ value: '', label: 'No auto-scrape' },
		...scrapePlugins.map((p) => ({ value: p.id, label: p.name })),
	]);

	async function loadKindPlugins(kind) {
		try {
			kindPlugins = (await kindsApi.plugins(kind)) ?? [];
		} catch {
			kindPlugins = [];
		} finally {
			pluginsLoaded = true;
		}
		if (autoScrapeId && !scrapePlugins.some((p) => p.id === autoScrapeId)) autoScrapeId = '';
		if (!dlPlugins.some((p) => p.id === dlPluginId)) dlPluginId = dlPlugins[0]?.id ?? '';
	}
	$effect(() => {
		const k = kindInput.trim();
		const t = setTimeout(() => loadKindPlugins(k), 350);
		return () => clearTimeout(t);
	});

	let kindsRefreshTimer;
	function scheduleKindsRefresh() {
		clearTimeout(kindsRefreshTimer);
		kindsRefreshTimer = setTimeout(refreshKinds, 800);
	}

	let queue = $state([]);
	let nextId = 0;
	const MAX_CONCURRENT = 3;
	let active = 0;

	const IN_PROGRESS = ['queued', 'uploading', 'downloading', 'scraping'];
	const isBusy = (s) => IN_PROGRESS.includes(s);
	const jobs = $derived([...queue].sort((a, b) => b.id - a.id));
	const completed = $derived(
		queue.filter((j) => j.status === 'done' || j.status === 'duplicate' || j.status === 'error'),
	);

	let kindOpen = $state(false);
	let kindTyping = $state(false);
	const kindSuggestions = $derived(
		kindTyping
			? $kindsStore.filter((k) => k.kind.toLowerCase().includes(kindInput.trim().toLowerCase()))
			: $kindsStore,
	);
	function openKindMenu() {
		kindTyping = false;
		kindOpen = true;
	}
	function pickKind(k) {
		kindInput = k;
		kindTyping = false;
		kindOpen = false;
	}

	function addFiles(list) {
		for (const file of Array.from(list)) {
			queue.push({
				id: nextId++,
				type: 'upload',
				name: file.name,
				size: file.size,
				kind: kindInput.trim() || 'uncategorized',
				scrapeId: autoScrapeId,
				file,
				status: 'queued',
				progress: 0,
				item: null,
				tagsApplied: null,
				error: null,
			});
		}
		pump();
	}

	function pump() {
		while (active < MAX_CONCURRENT) {
			const job = queue.find((j) => j.status === 'queued');
			if (!job) break;
			if (job.type === 'download') runDownload(job);
			else runUpload(job);
		}
	}

	async function runUpload(job) {
		active++;
		job.status = 'uploading';
		let newItem = null;
		try {
			const { status, item } = await itemsApi.upload(job.file, {
				kind: job.kind,
				onProgress: (p) => (job.progress = p),
			});
			job.item = item;
			job.progress = 1;
			if (status === 200) {
				job.status = 'duplicate';
			} else {
				newItem = item;
				job.status = 'done';
				scheduleKindsRefresh();
			}
		} catch (e) {
			job.status = 'error';
			job.error = e.message ?? String(e);
		} finally {
			job.file = null;
			active--;
			pump();
		}
		if (newItem && job.scrapeId) await runScrape(job, newItem.id);
	}

	const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

	async function pollJob(jobId) {
		const deadline = Date.now() + 10 * 60 * 1000;
		while (Date.now() < deadline) {
			let j;
			try {
				j = await jobsApi.get(jobId, { wait: true });
			} catch (e) {
				if (e instanceof ApiError && e.status === 404) return { state: 'done' };
				await sleep(1500);
				continue;
			}
			if (j.state === 'done' || j.state === 'failed') return j;
		}
		return null;
	}

	async function runScrape(job, itemId) {
		job.status = 'scraping';
		try {
			const res = await itemsApi.scrape(itemId, job.scrapeId, { wait: true });
			let terminal = res?.state ? { state: res.state, result: res.result } : null;
			if (!terminal && res?.job_id) terminal = await pollJob(res.job_id);
			job.tagsApplied = terminal?.result?.applied ?? null;
		} catch {
			/* ignored */
		}
		job.status = 'done';
	}

	function onPick(e) {
		if (e.target.files?.length) addFiles(e.target.files);
		e.target.value = '';
	}
	function onDrop(e) {
		e.preventDefault();
		dragover = false;
		if (e.dataTransfer?.files?.length) addFiles(e.dataTransfer.files);
	}
	function clearCompleted() {
		queue = queue.filter((j) => j.status === 'queued' || j.status === 'uploading');
	}

	const previewHref = (job) => (job.item ? `/item/${job.item.id}` : null);

	function fmtSize(b) {
		if (b == null) return '';
		const u = ['B', 'KB', 'MB', 'GB'];
		let i = 0;
		let n = b;
		while (n >= 1024 && i < u.length - 1) {
			n /= 1024;
			i++;
		}
		return `${n.toFixed(n < 10 && i > 0 ? 1 : 0)} ${u[i]}`;
	}

	const dlPlugins = $derived(
		kindPlugins.filter((p) => p.enabled && p.capabilities?.includes('download')),
	);
	let dlPluginId = $state('');
	let refText = $state('');
	let credSources = $state(new Set());

	const dlPluginOptions = $derived(dlPlugins.map((p) => ({ value: p.id, label: p.name })));
	const dlSelected = $derived(dlPlugins.find((p) => p.id === dlPluginId) ?? null);
	const dlRefInput = $derived(
		dlSelected?.reference_inputs?.download ?? {
			label: 'Source references',
			placeholder: 'One source-specific URL or identifier per line',
			help: 'Enter one or more references, separated by commas or new lines.',
			required: true,
		},
	);
	const credsMissing = $derived(
		!!dlSelected?.auth?.required_for?.includes('download') && !credSources.has(dlSelected.source),
	);
	const refs = $derived(refText.split(/[\s,]+/).filter(Boolean));
	const canDownload = $derived(!!dlPluginId && refs.length > 0 && !credsMissing);

	onMount(async () => {
		try {
			const list = await credsApi.list();
			credSources = new Set((list ?? []).filter((c) => c.fields?.length).map((c) => c.source));
		} catch {
			/* ignored */
		}
	});

	function startDownloads() {
		if (!canDownload) return;
		const plugin = dlPluginId;
		const kind = kindInput.trim() || 'uncategorized';
		for (const ref of refs) {
			queue.push({
				id: nextId++,
				type: 'download',
				name: `${plugin} · ${ref}`,
				plugin,
				ref,
				kind,
				status: 'queued',
				progress: 0,
				item: null,
				tagsApplied: null,
				error: null,
			});
		}
		refText = '';
		pump();
	}

	async function runDownload(job) {
		active++;
		job.status = 'downloading';
		try {
			const res = await downloadsApi.create(job.plugin, {
				ref: job.ref,
				kind: job.kind,
				wait: true,
			});
			let terminal = res?.state ? { state: res.state, result: res.result } : null;
			if (!terminal && res?.job_id) terminal = await pollJob(res.job_id);
			if (!terminal) {
				job.status = 'error';
				job.error = 'Still running — check back shortly.';
			} else if (terminal.state === 'failed') {
				job.status = 'error';
				job.error = terminal.result?.error ?? 'Download failed';
			} else {
				const r = terminal.result ?? {};
				job.item = r.id != null ? { id: r.id } : null;
				job.tagsApplied = r.applied ?? null;
				job.status = r.created === false ? 'duplicate' : 'done';
				if (job.status === 'done') scheduleKindsRefresh();
				if (r.id != null) {
					const d = await itemsApi.detail(r.id).catch(() => null);
					if (d?.name) job.name = d.name;
				}
			}
		} catch (e) {
			job.status = 'error';
			job.error = e.message ?? String(e);
		} finally {
			active--;
			pump();
		}
	}
</script>

<div class="app-page upload">
	<PageHeader title="Upload" />

	{#if !isAdmin}
		<EmptyState
			title="Admins only"
			message="Uploading archives to the server is restricted to administrators."
		/>
	{:else}
		<div class="modes" role="tablist">
			<button
				class="modebtn"
				class:active={mode === 'computer'}
				role="tab"
				aria-selected={mode === 'computer'}
				onclick={() => (mode = 'computer')}
				type="button">Your computer</button
			>
			<button
				class="modebtn"
				class:active={mode === 'plugin'}
				role="tab"
				aria-selected={mode === 'plugin'}
				onclick={() => (mode = 'plugin')}
				type="button">Plugin download</button
			>
		</div>

		<div class="toolbar">
			<div class="field kindfield">
				<span class="flabel">Type</span>
				<div class="kindcombo">
					<input
						class="kindinput"
						type="text"
						bind:value={kindInput}
						onfocus={openKindMenu}
						oninput={() => {
							kindTyping = true;
							kindOpen = true;
						}}
						placeholder="uncategorized"
						aria-label="Kind (destination folder)"
						spellcheck="false"
						autocapitalize="off"
					/>
					{#if $kindsStore.length}
						<button
							class="kindcaret"
							type="button"
							onclick={() => (kindOpen ? (kindOpen = false) : openKindMenu())}
							aria-label="Existing kinds"
						>
							<svg
								viewBox="0 0 20 20"
								fill="none"
								stroke="currentColor"
								stroke-width="2"
								stroke-linecap="round"
								stroke-linejoin="round"><path d="M5 7l5 5 5-5" /></svg
							>
						</button>
					{/if}
					{#if kindOpen && kindSuggestions.length}
						<button
							class="scrim"
							type="button"
							aria-label="Close"
							onclick={() => (kindOpen = false)}
						></button>
						<ul class="kindmenu" role="listbox">
							{#each kindSuggestions as k (k.kind)}
								<li>
									<button
										type="button"
										class="kindopt"
										class:sel={kindInput.trim() === k.kind}
										onclick={() => pickKind(k.kind)}
									>
										<span>{kindLabel(k.kind)}</span>
										<span class="cnt">{k.count}</span>
									</button>
								</li>
							{/each}
						</ul>
					{/if}
				</div>
			</div>

			{#if mode === 'computer'}
				<div class="field">
					<span class="flabel">Auto-scrape</span>
					<div class="fctl"><Dropdown bind:value={autoScrapeId} options={autoScrapeOptions} /></div>
				</div>
			{:else if dlPlugins.length || !pluginsLoaded}
				<div class="field">
					<span class="flabel">Plugin</span>
					<div class="fctl"><Dropdown bind:value={dlPluginId} options={dlPluginOptions} /></div>
				</div>
			{/if}
		</div>

		{#if mode === 'computer'}
			<input
				bind:this={fileInput}
				type="file"
				multiple
				accept=".cbz,.zip,.epub,application/zip,application/epub+zip"
				onchange={onPick}
				hidden
			/>
			<button
				type="button"
				class="dropzone"
				class:dragover
				onclick={() => fileInput.click()}
				ondragover={(e) => {
					e.preventDefault();
					dragover = true;
				}}
				ondragleave={() => (dragover = false)}
				ondrop={onDrop}
			>
				<span class="dzicon">
					<svg
						viewBox="0 0 24 24"
						fill="none"
						stroke="currentColor"
						stroke-width="1.5"
						stroke-linecap="round"
						stroke-linejoin="round"
					>
						<path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" />
						<path d="M17 8l-5-5-5 5" />
						<path d="M12 3v12" />
					</svg>
				</span>
				<span class="dzmain">Drag and drop, or <span class="link">browse</span></span>
				<span class="dzsub"
					>CBZ, ZIP, or EPUB · multiple files{#if autoScrapeId}
						· auto-scraping with {scrapePlugins.find((p) => p.id === autoScrapeId)?.name}{/if}</span
				>
			</button>
		{:else if pluginsLoaded && !dlPlugins.length}
			<EmptyState
				wide
				title="No download plugins for this kind"
				message="No plugin is enabled for this kind's downloads. Pick a different Type, or:"
			>
				{#snippet icon()}
					<svg
						viewBox="0 0 24 24"
						fill="none"
						stroke="currentColor"
						stroke-width="1.6"
						stroke-linecap="round"
						stroke-linejoin="round"
					>
						<path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" />
						<path d="M7 10l5 5 5-5" />
						<path d="M12 15V3" />
					</svg>
				{/snippet}
				{#snippet action()}
					<a class="settingslink" href="/plugins">Enable one on the Plugins page</a>
				{/snippet}
			</EmptyState>
		{:else}
			{#if credsMissing}
				<p class="warn">
					<svg
						class="warnico"
						viewBox="0 0 24 24"
						fill="none"
						stroke="currentColor"
						stroke-width="1.8"
						stroke-linecap="round"
						stroke-linejoin="round"
						><path
							d="M10.3 3.9 1.8 18a2 2 0 0 0 1.7 3h16.9a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0z"
						/><path d="M12 9v4M12 17h.01" /></svg
					>
					{dlSelected?.name} needs credentials to download. Set them on
					<a href={`/plugins/${dlSelected?.id}?setup=1`}>its plugin page</a>, then come back.
				</p>
			{/if}
			<div class="dlform">
				<label class="dlrefs">
					<span class="flabel">{dlRefInput.label}</span>
					<textarea
						class="reftext"
						bind:value={refText}
						placeholder={dlRefInput.placeholder}
						rows="4"
						spellcheck="false"></textarea>
					{#if dlRefInput.help}<span class="dlhelp">{dlRefInput.help}</span>{/if}
				</label>
				<button class="dlbtn" type="button" disabled={!canDownload} onclick={startDownloads}>
					<svg
						viewBox="0 0 24 24"
						fill="none"
						stroke="currentColor"
						stroke-width="1.8"
						stroke-linecap="round"
						stroke-linejoin="round"
						><path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" /><path d="M7 10l5 5 5-5" /><path
							d="M12 15V3"
						/></svg
					>
					Download{refs.length > 1 ? ` (${refs.length})` : ''}
				</button>
			</div>
		{/if}

		{#if queue.length}
			<section class="activity">
				<div class="acthead">
					<h2>Activity</h2>
					{#if completed.length}<button class="clear" type="button" onclick={clearCompleted}
							>Clear finished</button
						>{/if}
				</div>
				<div class="joblist">
					{#each jobs as job (job.id)}
						{@const href = previewHref(job)}
						{@const busy = isBusy(job.status)}
						<svelte:element
							this={href ? 'a' : 'div'}
							{href}
							class="jobrow"
							class:err={job.status === 'error'}
						>
							<span class="jthumb">
								{#if job.item && job.status !== 'error'}
									<img src={media.thumbnail(job.item.id)} alt="" loading="lazy" />
								{:else}
									<svg
										viewBox="0 0 24 24"
										fill="none"
										stroke="currentColor"
										stroke-width="1.5"
										stroke-linecap="round"
										stroke-linejoin="round"
									>
										{#if job.type === 'download'}<path
												d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"
											/><path d="M7 10l5 5 5-5" /><path d="M12 15V3" />{:else}<path
												d="M14 3v4a1 1 0 0 0 1 1h4"
											/><path
												d="M17 21H7a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h7l5 5v11a2 2 0 0 1-2 2z"
											/>{/if}
									</svg>
								{/if}
							</span>
							<div class="jmeta">
								<span class="jname" title={job.name}>{job.name}</span>
								{#if busy}
									<div class="track">
										<div
											class="fill"
											class:pulse={job.status === 'scraping' || job.status === 'downloading'}
											style={`width:${job.status === 'uploading' ? job.progress * 100 : 100}%`}
										></div>
									</div>
								{:else if job.status === 'done'}
									{@const verb = job.type === 'download' ? 'Downloaded' : 'Added'}
									{#if job.tagsApplied != null && job.tagsApplied > 0}
										<span class="jtag ok"
											>{verb} · {job.tagsApplied} tag{job.tagsApplied === 1 ? '' : 's'}</span
										>
									{:else if job.type === 'download' || job.scrapeId}
										<span class="jtag dup">{verb} · no metadata found</span>
									{:else}
										<span class="jtag ok"
											>{verb}{#if job.size}
												· {fmtSize(job.size)}{/if}</span
										>
									{/if}
								{:else if job.status === 'duplicate'}
									<span class="jtag dup">Already in library</span>
								{:else}
									<span class="jtag bad">{job.error || 'Failed'}</span>
								{/if}
							</div>
							{#if busy}
								<span class="jpct">
									{#if job.status === 'uploading'}{Math.round(job.progress * 100)}%
									{:else if job.status === 'downloading'}Downloading
									{:else if job.status === 'scraping'}Scraping
									{:else}Queued{/if}
								</span>
							{:else if href}
								<svg
									class="jgo"
									viewBox="0 0 24 24"
									fill="none"
									stroke="currentColor"
									stroke-width="2"
									stroke-linecap="round"
									stroke-linejoin="round"><path d="M9 6l6 6-6 6" /></svg
								>
							{/if}
						</svelte:element>
					{/each}
				</div>
			</section>
		{/if}
	{/if}
</div>

<style>
	.modes {
		display: flex;
		gap: 2px;
		padding: 3px;
		margin-bottom: var(--space-5);
		border: 1px solid var(--border);
		border-radius: var(--radius);
		background: var(--surface);
	}
	.modebtn {
		all: unset;
		flex: 1;
		text-align: center;
		cursor: pointer;
		padding: var(--space-2) var(--space-4);
		border-radius: var(--radius-sm);
		font-size: 0.88rem;
		color: var(--muted);
		transition:
			background var(--ease),
			color var(--ease);
	}
	.modebtn:hover {
		color: var(--text);
	}
	.modebtn.active {
		background: var(--accent-soft);
		color: var(--accent);
	}

	.toolbar {
		display: flex;
		flex-wrap: wrap;
		align-items: flex-end;
		gap: var(--space-4);
		margin-bottom: var(--space-4);
	}
	.field {
		flex: 1 1 12rem;
		min-width: 0;
		display: flex;
		flex-direction: column;
		gap: var(--space-1);
	}
	.flabel {
		font-size: 0.72rem;
		text-transform: uppercase;
		letter-spacing: 0.1em;
		color: var(--muted);
	}
	.fctl {
		width: 100%;
	}

	.kindcombo {
		position: relative;
		width: 100%;
	}
	.kindinput {
		width: 100%;
		box-sizing: border-box;
		font: inherit;
		color: var(--text);
		background: var(--surface-2);
		border: 1px solid var(--border);
		border-radius: var(--radius-sm);
		padding: 0.45rem 2rem 0.45rem 0.6rem;
	}
	.kindinput:focus {
		outline: none;
		border-color: var(--accent);
	}
	.kindcaret {
		all: unset;
		position: absolute;
		top: 0;
		right: 0;
		height: 100%;
		width: 2rem;
		display: inline-flex;
		align-items: center;
		justify-content: center;
		color: var(--muted);
		cursor: pointer;
	}
	.kindcaret:hover {
		color: var(--text);
	}
	.kindcaret svg {
		width: 0.9rem;
		height: 0.9rem;
	}
	.scrim {
		all: unset;
		position: fixed;
		inset: 0;
		z-index: 30;
		cursor: default;
	}
	.kindmenu {
		position: absolute;
		top: calc(100% + 4px);
		left: 0;
		right: 0;
		z-index: 31;
		margin: 0;
		padding: var(--space-1);
		list-style: none;
		max-height: 15rem;
		overflow-y: auto;
		background: var(--surface);
		border: 1px solid var(--border);
		border-radius: var(--radius-sm);
		box-shadow: var(--shadow-lg);
	}
	.kindmenu li {
		list-style: none;
	}
	.kindopt {
		all: unset;
		box-sizing: border-box;
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--space-3);
		width: 100%;
		padding: var(--space-2) var(--space-3);
		border-radius: var(--radius-sm);
		cursor: pointer;
		font-size: 0.9rem;
		color: var(--text);
	}
	.kindopt:hover {
		background: var(--surface-2);
	}
	.kindopt.sel {
		color: var(--accent);
	}
	.cnt {
		flex: 0 0 auto;
		font-variant-numeric: tabular-nums;
		font-size: 0.72rem;
		color: var(--muted);
	}

	.dropzone {
		all: unset;
		box-sizing: border-box;
		display: flex;
		flex-direction: column;
		align-items: center;
		justify-content: center;
		gap: var(--space-3);
		width: 100%;
		padding: var(--space-8) var(--space-5);
		min-height: 12rem;
		border: 1.5px dashed var(--border);
		border-radius: var(--radius-lg);
		background: var(--surface);
		color: var(--muted);
		cursor: pointer;
		text-align: center;
		transition:
			border-color var(--ease),
			background var(--ease),
			color var(--ease);
	}
	.dropzone:hover,
	.dropzone.dragover {
		border-color: var(--accent);
		color: var(--text);
		background: var(--accent-soft);
	}
	.dzicon {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		width: 3rem;
		height: 3rem;
		border-radius: 50%;
		background: var(--surface-2);
		border: 1px solid var(--border);
	}
	.dropzone:hover .dzicon,
	.dropzone.dragover .dzicon {
		border-color: var(--accent);
	}
	.dzicon svg {
		width: 1.5rem;
		height: 1.5rem;
	}
	.dzmain {
		font-size: 1rem;
		color: var(--text);
	}
	.dzmain .link {
		color: var(--accent);
	}
	.dzsub {
		font-size: 0.8rem;
	}

	.warn {
		display: flex;
		align-items: flex-start;
		gap: var(--space-2);
		margin: 0 0 var(--space-4);
		padding: var(--space-3) var(--space-4);
		border: 1px solid rgba(224, 86, 111, 0.4);
		background: rgba(224, 86, 111, 0.1);
		border-radius: var(--radius-sm);
		font-size: 0.85rem;
		color: var(--text);
	}
	.warnico {
		flex: 0 0 auto;
		width: 1.05rem;
		height: 1.05rem;
		margin-top: 0.05rem;
		color: #e0566f;
	}
	.warn a {
		color: var(--accent);
		text-decoration: underline;
	}
	.settingslink {
		color: var(--accent);
		font-size: 0.85rem;
		text-decoration: underline;
		text-underline-offset: 2px;
	}
	.settingslink:hover {
		color: var(--text);
	}
	.dlform {
		display: flex;
		flex-direction: column;
		gap: var(--space-3);
	}
	.dlrefs {
		display: grid;
		gap: var(--space-2);
	}
	.dlhelp {
		font-size: 0.8rem;
		color: var(--muted);
	}
	.reftext {
		width: 100%;
		font-family: var(--font-mono);
		font-size: 0.85rem;
		color: var(--text);
		background: var(--surface);
		border: 1px solid var(--border);
		border-radius: var(--radius);
		padding: var(--space-3);
		resize: vertical;
	}
	.reftext:focus {
		outline: none;
		border-color: var(--accent);
	}
	.dlbtn {
		display: flex;
		align-items: center;
		justify-content: center;
		gap: var(--space-2);
		padding: 0.6rem 1.2rem;
		border: 1px solid var(--accent);
		border-radius: var(--radius-sm);
		background: var(--accent);
		color: #fff;
		font-weight: 600;
		font-size: 0.9rem;
	}
	.dlbtn svg {
		width: 1rem;
		height: 1rem;
	}
	.dlbtn:hover:not(:disabled) {
		filter: brightness(1.1);
		border-color: var(--accent);
	}
	.dlbtn:disabled {
		opacity: 0.5;
		cursor: not-allowed;
	}

	.activity {
		margin-top: var(--space-6);
	}
	.acthead {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--space-3);
		margin-bottom: var(--space-3);
	}
	.acthead h2 {
		margin: 0;
		font-size: 0.75rem;
		text-transform: uppercase;
		letter-spacing: 0.12em;
		color: var(--muted);
	}
	.clear {
		all: unset;
		cursor: pointer;
		font-size: 0.8rem;
		color: var(--muted);
	}
	.clear:hover {
		color: var(--accent);
	}
	.joblist {
		display: flex;
		flex-direction: column;
		gap: var(--space-2);
	}
	.jobrow {
		display: flex;
		align-items: center;
		gap: var(--space-3);
		padding: var(--space-2) var(--space-3);
		border: 1px solid var(--border);
		border-radius: var(--radius);
		background: var(--surface);
		text-decoration: none;
		color: inherit;
	}
	a.jobrow:hover {
		border-color: var(--accent);
	}
	.jobrow.err {
		border-color: rgba(224, 86, 111, 0.4);
	}
	.jthumb {
		flex: 0 0 auto;
		display: flex;
		align-items: center;
		justify-content: center;
		width: 2.4rem;
		height: 3.2rem;
		border-radius: var(--radius-sm);
		background: var(--surface-2);
		color: var(--muted);
		overflow: hidden;
	}
	.jthumb img {
		width: 100%;
		height: 100%;
		object-fit: cover;
	}
	.jthumb svg {
		width: 1.2rem;
		height: 1.2rem;
	}
	.jmeta {
		flex: 1;
		min-width: 0;
		display: flex;
		flex-direction: column;
		gap: var(--space-1);
	}
	.jname {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		font-size: 0.88rem;
		color: var(--text);
	}
	.track {
		height: 4px;
		border-radius: 9999px;
		overflow: hidden;
		background: var(--surface-2);
	}
	.fill {
		height: 100%;
		background: var(--accent);
		transition: width 0.15s ease;
	}
	.fill.pulse {
		animation: pulse 1.2s ease-in-out infinite;
	}
	@keyframes pulse {
		0%,
		100% {
			opacity: 1;
		}
		50% {
			opacity: 0.4;
		}
	}
	.jtag {
		font-size: 0.74rem;
	}
	.jtag.ok {
		color: var(--good);
	}
	.jtag.dup {
		color: var(--muted);
	}
	.jtag.bad {
		color: #e0566f;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.jpct {
		flex: 0 0 auto;
		font-variant-numeric: tabular-nums;
		font-size: 0.74rem;
		color: var(--muted);
		white-space: nowrap;
	}
	.jgo {
		flex: 0 0 auto;
		width: 1rem;
		height: 1rem;
		color: var(--muted);
	}
</style>
