<script>
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import { credentials as credsApi, kinds as kindsApi, media } from '$lib/api.js';
	import { currentUser } from '$lib/session.js';
	import { kindLabel } from '$lib/kinds.js';
	import { CAP_LABEL, monogram, tintOf } from '$lib/plugins/common.js';
	import {
		loadCatalog,
		catalogById,
		installs,
		realVersion,
		realAuthor,
		authorProfile,
		update as updatePlugin,
		uninstall as uninstallPlugin,
	} from '$lib/plugins/catalog.js';
	import InstallSheet from '$lib/plugins/InstallSheet.svelte';

	let { id, setup = false } = $props();
	const repoHost = (url) => {
		try {
			return new URL(url).host;
		} catch {
			return url;
		}
	};
	const isAdmin = $derived($currentUser?.role === 'admin');

	let entry = $state(null);
	const inst = $derived($installs[id] ?? null);
	let setupMode = $state(setup);

	let kindList = $state([]);
	let cells = $state(new Map());
	let credFields = $state(new Set());
	let loaded = $state(false);

	let credOpen = $state(false);
	let credOpenInit = false;
	$effect(() => {
		if (loaded && !credOpenInit) {
			credOpenInit = true;
			credOpen = credFields.size === 0;
		}
	});

	onMount(async () => {
		try {
			await loadCatalog();
			entry = catalogById(id);
			const [kinds, creds] = await Promise.all([
				kindsApi.list().catch(() => []),
				credsApi.list().catch(() => []),
			]);
			kindList = kinds ?? [];
			const rows = await Promise.all(kindList.map((k) => kindsApi.plugins(k.kind)));
			const m = new Map();
			kindList.forEach((k, i) => {
				m.set(
					k.kind,
					new Map((rows[i] ?? []).map((r) => [r.id, { enabled: r.enabled, auto: r.auto }])),
				);
			});
			cells = m;
			const mine = (creds ?? []).find((c) => c.source === entry?.source);
			credFields = new Set(mine?.fields ?? []);
		} finally {
			loaded = true;
		}
	});

	const needsSetup = $derived(
		!!inst &&
			!!entry?.auth?.fields?.length &&
			!credFields.size &&
			(setupMode || (entry.auth.required_for ?? []).length > 0),
	);
	const requiredFor = $derived(
		(entry?.auth?.required_for ?? []).map((c) => (CAP_LABEL[c] ?? c).toLowerCase()).join(', '),
	);

	let sheetOpen = $state(false);
	function onInstalled() {
		sheetOpen = false;
		if (entry?.auth?.fields?.length) setupMode = true;
	}

	let updateBusy = $state(false);
	async function doUpdate() {
		if (updateBusy) return;
		updateBusy = true;
		try {
			await updatePlugin(id);
			entry = catalogById(id);
		} finally {
			updateBusy = false;
		}
	}

	let confirmUninstall = $state(false);
	let uninstallBusy = $state(false);
	async function doUninstall() {
		if (uninstallBusy) return;
		uninstallBusy = true;
		try {
			await uninstallPlugin(id);
			goto('/plugins');
		} finally {
			uninstallBusy = false;
		}
	}

	const canAuto = $derived((entry?.capabilities ?? []).includes('scrape'));
	let kindBusy = $state(false);
	let kindErr = $state('');
	const cellState = (kind) => {
		const c = cells.get(kind)?.get(id);
		return c?.enabled ? (c.auto ? 2 : 1) : 0;
	};
	const cellLabel = (s) => (s === 0 ? 'Off' : s === 1 ? 'On' : 'Auto');
	async function cycleKind(kind) {
		if (kindBusy) return;
		const row = cells.get(kind);
		if (!row) return;
		const prev = new Map([...row].map(([pid, c]) => [pid, { ...c }]));
		const states = canAuto ? 3 : 2;
		const s = (cellState(kind) + 1) % states;
		row.set(id, { enabled: s > 0, auto: s === 2 });
		cells = new Map(cells);
		kindBusy = true;
		kindErr = '';
		try {
			await kindsApi.setPlugins(
				kind,
				[...row].filter(([, c]) => c.enabled).map(([pid]) => pid),
				[...row].filter(([, c]) => c.enabled && c.auto).map(([pid]) => pid),
			);
		} catch (e) {
			cells = new Map(cells).set(kind, prev);
			kindErr = e?.message ?? String(e);
		} finally {
			kindBusy = false;
		}
	}

	let inputs = $state({});
	let credBusy = $state(false);
	let credMsg = $state('');
	async function saveCreds() {
		const data = {};
		for (const f of entry.auth.fields) {
			const v = (inputs[f.name] ?? '').trim();
			if (v) data[f.name] = v;
		}
		if (!Object.keys(data).length) {
			credMsg = 'Enter a value to save.';
			return;
		}
		credBusy = true;
		credMsg = '';
		try {
			await credsApi.set(entry.source, data);
			credFields = new Set([...credFields, ...Object.keys(data)]);
			for (const f of entry.auth.fields) inputs[f.name] = '';
			credMsg = 'Saved.';
			setupMode = false;
			credOpen = false;
		} catch (e) {
			credMsg = e?.message ?? String(e);
		} finally {
			credBusy = false;
		}
	}
	async function removeCreds() {
		credBusy = true;
		credMsg = '';
		try {
			await credsApi.remove(entry.source);
			credFields = new Set();
			credMsg = 'Removed.';
			credOpen = true;
		} catch (e) {
			credMsg = e?.message ?? String(e);
		} finally {
			credBusy = false;
		}
	}
</script>

<div class="pdetail">
	{#if !isAdmin}
		<p class="denied">This area is available to administrators only.</p>
	{:else if !loaded}
		<a class="back" href="/plugins">‹ Plugins</a>
	{:else if !entry}
		<a class="back" href="/plugins">‹ Plugins</a>
		<p class="muted">No plugin with this id.</p>
	{:else}
		{@const ver = realVersion(inst?.version ?? entry.version)}
		{@const author = realAuthor(entry.author)}
		<a class="back" href="/plugins">‹ Plugins</a>

		<header>
			<span class="tile" style={tintOf(entry.id)} aria-hidden="true">
				{monogram(entry.name)}
				{#if entry.icon}
					<img
						class="ticon"
						src={media.pluginIcon(entry.icon)}
						alt=""
						onerror={(e) => (e.currentTarget.style.display = 'none')}
					/>
				{/if}
			</span>
			<div class="idblock">
				<h1>{entry.name}</h1>
				{#if ver || author || entry.repository}
					<p class="hmeta">
						{#if ver}v{ver}{/if}
						{#if author}
							{#if ver}&nbsp;·&nbsp;{/if}
							{#if authorProfile(entry.author)}
								<a class="repo" href={authorProfile(entry.author)} target="_blank" rel="noreferrer"
									>@{author}</a
								>
							{:else}
								@{author}
							{/if}
						{/if}
						{#if entry.repository}
							{#if ver || author}&nbsp;·&nbsp;{/if}<a
								class="repo"
								href={entry.repository}
								target="_blank"
								rel="noreferrer">Source ↗</a
							>
						{/if}
					</p>
				{/if}
				{#if entry.repo_url}
					<p class="hmeta fromrepo" title={entry.repo_url}>
						From repository: {repoHost(entry.repo_url)}
					</p>
				{/if}
				<div class="badges">
					{#if entry.origin}
						<span class="badge" class:st-good={entry.origin === 'bundled'}>
							{entry.origin === 'bundled'
								? 'Bundled'
								: entry.origin === 'local'
									? 'Local'
									: 'Community'}
						</span>
					{/if}
					{#if entry.nsfw}
						<span class="badge st-bad">NSFW</span>
					{/if}
					{#if entry.metadata_status === 'legacy'}
						<span class="badge">Legacy manifest</span>
					{/if}
					{#if inst && entry.last_error}
						<span class="badge st-bad">Failed to load</span>
					{:else if inst}
						<span class="badge st-good">Installed</span>
					{:else}
						<span class="badge st-muted">Not installed</span>
					{/if}
				</div>
			</div>
			{#if inst && entry.last_error}
				<button class="getbig" type="button" onclick={() => (sheetOpen = true)}>Reinstall</button>
			{:else if !inst}
				<button class="getbig" type="button" onclick={() => (sheetOpen = true)}>Get</button>
			{/if}
		</header>
		<p class="desc">{entry.description}</p>

		{#if inst && entry.last_error}
			<div class="loaderr">
				<b>This plugin failed to load</b> — reinstalling refreshes the artifact.
				<code>{entry.last_error}</code>
			</div>
		{/if}

		{#if inst && entry.update_available}
			<div class="updatebanner">
				<span>
					<b>Update available</b> — v{entry.update_available} is published in this plugin's repository{entry.repo_url
						? ` (${repoHost(entry.repo_url)})`
						: ''}.
				</span>
				<button class="updatebtn" type="button" disabled={updateBusy} onclick={doUpdate}>
					{updateBusy ? 'Updating…' : `Update to v${entry.update_available}`}
				</button>
			</div>
		{/if}

		{#if needsSetup && loaded}
			<div class="setupbanner">
				<svg
					viewBox="0 0 24 24"
					fill="none"
					stroke="currentColor"
					stroke-width="2"
					stroke-linecap="round"
					stroke-linejoin="round"
					><path
						d="M21 2l-2 2m-7.6 7.6a5.5 5.5 0 1 1-7.8 7.8 5.5 5.5 0 0 1 7.8-7.8zm0 0L19 4m-3.5 3.5L18 10"
					/></svg
				>
				<span>
					<b>Optional</b> — add your {entry.auth.fields.length === 1
						? (entry.auth.fields[0].label ?? 'credential')
						: 'credentials'} below{requiredFor ? ` to enable ${requiredFor}` : ''}.
				</span>
			</div>
		{/if}

		{#if inst && entry.auth?.fields?.length}
			<section class="card" class:hilite={needsSetup && loaded}>
				<button
					class="credhead"
					type="button"
					aria-expanded={credOpen}
					onclick={() => (credOpen = !credOpen)}
				>
					<svg
						class="chev"
						class:open={credOpen}
						viewBox="0 0 24 24"
						fill="none"
						stroke="currentColor"
						stroke-width="2"
						stroke-linecap="round"
						stroke-linejoin="round"
						aria-hidden="true"><path d="m9 6 6 6-6 6" /></svg
					>
					<span class="credtitle">Credentials</span>
					{#if credFields.size}
						<span class="credset">
							<svg
								viewBox="0 0 24 24"
								fill="none"
								stroke="currentColor"
								stroke-width="2"
								stroke-linecap="round"
								stroke-linejoin="round"
								aria-hidden="true"
								><path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z" /><path
									d="m9 12 2 2 4-4"
								/></svg
							>
							Credentials set
						</span>
					{:else}
						<span class="hnote">— stored server-side, never shown back</span>
					{/if}
				</button>
				{#if credOpen}
					<div class="credbody">
						{#if entry.auth.setup}
							<p class="setupguide">
								<svg
									viewBox="0 0 24 24"
									fill="none"
									stroke="currentColor"
									stroke-width="2"
									stroke-linecap="round"
									stroke-linejoin="round"
									aria-hidden="true"
									><circle cx="12" cy="12" r="9" /><path d="M12 8h.01M11 12h1v4h1" /></svg
								>
								{entry.auth.setup}
							</p>
						{/if}
						<div class="fields">
							{#each entry.auth.fields as f (f.name)}
								<label class="field">
									<span class="flabel">{f.label ?? f.name}</span>
									<input
										type={f.secret ? 'password' : 'text'}
										autocomplete="off"
										placeholder={credFields.has(f.name) ? 'configured — leave blank to keep' : ''}
										bind:value={inputs[f.name]}
										disabled={credBusy}
									/>
									{#if f.help}<span class="fhelp muted">{f.help}</span>{/if}
								</label>
							{/each}
						</div>
						<div class="credactions">
							{#if credMsg}<span class="msg muted">{credMsg}</span>{/if}
							{#if credFields.size}
								<button class="ghost" onclick={removeCreds} disabled={credBusy}>Remove</button>
							{/if}
							<button class="primary" onclick={saveCreds} disabled={credBusy}>
								{credBusy ? 'Saving…' : credFields.size ? 'Update' : 'Save'}
							</button>
						</div>
					</div>
				{/if}
			</section>
		{/if}

		{#if inst && kindList.length}
			<section class="card">
				<h2>
					Enabled for
					<span class="hnote"
						>— per kind; {canAuto
							? 'Auto also runs it on newly added items'
							: 'On offers it on that kind'}</span
					>
				</h2>
				<div class="kindchips">
					{#each kindList as k (k.kind)}
						{@const s = Math.min(cellState(k.kind), canAuto ? 2 : 1)}
						<button
							class="kchip"
							class:on={s > 0}
							type="button"
							disabled={kindBusy}
							onclick={() => cycleKind(k.kind)}
							title={`${kindLabel(k.kind)}: ${cellLabel(s)} — click to change`}
						>
							{kindLabel(k.kind)}
							<span class="kstate">{cellLabel(s)}</span>
						</button>
					{/each}
				</div>
				{#if kindErr}<p class="err">{kindErr}</p>{/if}
			</section>
		{/if}

		<section class="card">
			<h2>Permissions <span class="hnote">— what this plugin can do</span></h2>
			<div class="permcols">
				<div class="permgroup">
					<h3>Operations</h3>
					{#each entry.capabilities as c (c)}
						<p class="perm"><span class="tick">✓</span>{CAP_LABEL[c] ?? c}</p>
					{/each}
				</div>
				<div class="permgroup">
					<h3>Network</h3>
					{#each entry.hosts as h (h)}
						<p class="perm"><span class="tick">✓</span>{h} and its subdomains</p>
					{/each}
				</div>
				<div class="permgroup">
					<h3>Credentials</h3>
					{#if entry.auth?.fields?.length}
						{#each entry.auth.fields as f (f.name)}
							<p class="perm"><span class="tick">✓</span>{f.label ?? f.name}</p>
						{/each}
					{:else}
						<p class="perm muted">None</p>
					{/if}
				</div>
			</div>
		</section>

		{#if inst}
			<section class="card danger">
				<div class="dgrow">
					<div>
						<h2>Uninstall</h2>
						<p class="dghint">
							Removes the plugin from this server. Credentials and kind assignments are kept for a
							reinstall.
						</p>
					</div>
					{#if confirmUninstall}
						<div class="dgactions">
							<button
								class="ghost"
								type="button"
								onclick={() => (confirmUninstall = false)}
								disabled={uninstallBusy}>Keep</button
							>
							<button class="dgbtn" type="button" onclick={doUninstall} disabled={uninstallBusy}>
								{uninstallBusy ? 'Removing…' : 'Uninstall plugin'}
							</button>
						</div>
					{:else}
						<button class="dgbtn outline" type="button" onclick={() => (confirmUninstall = true)}
							>Uninstall</button
						>
					{/if}
				</div>
			</section>
		{/if}
	{/if}
</div>

{#if sheetOpen && entry != null}
	<InstallSheet {entry} onclose={() => (sheetOpen = false)} oninstalled={onInstalled} />
{/if}

<style>
	.pdetail {
		padding: var(--space-6) var(--space-8);
		width: 100%;
		display: flex;
		flex-direction: column;
		gap: var(--space-4);
	}
	.back {
		align-self: flex-start;
		font-size: 0.85rem;
		color: var(--muted);
	}
	.back:hover {
		color: var(--accent);
	}
	.denied,
	.muted {
		color: var(--muted);
	}
	.err {
		margin: 0;
		color: var(--bad, #e5484d);
		font-size: 0.85rem;
	}

	header {
		display: flex;
		align-items: center;
		gap: var(--space-5);
		flex-wrap: wrap;
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
		width: 68px;
		height: 68px;
		border-radius: 24%;
		display: inline-flex;
		align-items: center;
		justify-content: center;
		font-family: var(--font-display);
		font-weight: 700;
		font-size: 1.7rem;
	}
	.idblock {
		flex: 1;
		min-width: 0;
	}
	header h1 {
		margin: 0;
		font-family: var(--font-display);
		font-weight: 700;
		font-size: 1.5rem;
		line-height: 1.15;
	}
	.hmeta {
		margin: 0.15rem 0 0.4rem;
		font-size: 0.8rem;
		color: var(--muted);
	}
	.repo {
		color: var(--accent);
	}
	.fromrepo {
		margin-top: 0;
	}
	.badges {
		display: flex;
		gap: var(--space-2);
		flex-wrap: wrap;
	}
	.badge {
		padding: 0.1rem 0.5rem;
		border: 1px solid var(--border);
		border-radius: 9999px;
		font-size: 0.68rem;
		color: var(--muted);
	}
	.badge.st-good {
		color: var(--good, #46a758);
		border-color: color-mix(in srgb, var(--good, #46a758) 55%, var(--border));
	}
	.badge.st-muted {
		color: var(--muted);
	}
	.badge.st-bad {
		color: var(--bad, #e5484d);
		border-color: color-mix(in srgb, var(--bad, #e5484d) 55%, var(--border));
	}
	.loaderr {
		border: 1px solid color-mix(in srgb, var(--bad, #e5484d) 40%, var(--border));
		background: var(--surface-2);
		border-radius: var(--radius);
		padding: var(--space-3) var(--space-4);
		font-size: 0.84rem;
	}
	.loaderr b {
		font-weight: 600;
	}
	.loaderr code {
		display: block;
		margin-top: var(--space-2);
		font-size: 0.75rem;
		word-break: break-all;
		color: var(--muted);
	}
	.desc {
		margin: 0;
		font-size: 0.88rem;
		color: var(--muted);
		max-width: 60ch;
	}

	.getbig {
		flex: 0 0 auto;
		padding: var(--space-2) var(--space-5);
		font-size: 0.95rem;
		font-weight: 600;
		color: #fff;
		background: var(--accent);
		border-color: var(--accent);
	}
	.getbig:hover {
		opacity: 0.88;
	}

	.updatebanner {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--space-4);
		flex-wrap: wrap;
		border: 1px solid color-mix(in srgb, var(--accent) 50%, var(--border));
		background: var(--accent-soft);
		border-radius: var(--radius);
		padding: var(--space-3) var(--space-4);
		font-size: 0.86rem;
	}
	.updatebanner b {
		font-weight: 600;
	}
	.updatebtn {
		flex: 0 0 auto;
		padding: var(--space-1) var(--space-3);
		font-size: 0.85rem;
		font-weight: 600;
		color: #fff;
		background: var(--accent);
		border-color: var(--accent);
	}
	.updatebtn:disabled {
		opacity: 0.6;
		cursor: default;
	}

	.setupbanner {
		display: flex;
		align-items: center;
		gap: var(--space-3);
		border: 1px solid color-mix(in srgb, var(--accent) 50%, var(--border));
		background: var(--accent-soft);
		border-radius: var(--radius);
		padding: var(--space-3) var(--space-4);
		font-size: 0.86rem;
	}
	.setupbanner svg {
		flex: 0 0 auto;
		width: 1rem;
		height: 1rem;
		color: var(--accent);
	}
	.setupbanner b {
		font-weight: 600;
	}

	.card {
		border: 1px solid var(--border);
		border-radius: var(--radius);
		background: var(--surface);
		padding: var(--space-4) var(--space-5);
	}
	.card.hilite {
		border-color: color-mix(in srgb, var(--accent) 45%, var(--border));
	}
	.card h2 {
		margin: 0 0 var(--space-3);
		font-size: 0.95rem;
	}
	.hnote {
		font-size: 0.75rem;
		font-weight: 400;
		color: var(--muted);
	}

	.credhead {
		all: unset;
		box-sizing: border-box;
		display: flex;
		align-items: center;
		gap: var(--space-2);
		width: 100%;
		cursor: pointer;
	}
	.credhead:focus-visible {
		outline: 2px solid var(--accent);
		outline-offset: 2px;
		border-radius: var(--radius-sm);
	}
	.credtitle {
		font-size: 0.95rem;
		font-weight: 600;
	}
	.chev {
		flex: 0 0 auto;
		width: 0.8rem;
		height: 0.8rem;
		color: var(--muted);
		transition: transform 0.15s ease;
	}
	.chev.open {
		transform: rotate(90deg);
	}
	.credset {
		display: inline-flex;
		align-items: center;
		gap: 0.35rem;
		margin-left: auto;
		font-size: 0.78rem;
		font-weight: 500;
		color: var(--good, #46a758);
	}
	.credset svg {
		width: 0.9rem;
		height: 0.9rem;
	}
	.credbody {
		margin-top: var(--space-4);
	}
	.permcols {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(180px, 1fr));
		gap: var(--space-4);
	}
	.permgroup h3 {
		margin: 0 0 var(--space-1);
		font-size: 0.7rem;
		letter-spacing: 0.1em;
		text-transform: uppercase;
		color: var(--muted);
		font-weight: 600;
	}
	.perm {
		margin: 0;
		font-size: 0.82rem;
		padding: 0.1rem 0;
	}
	.perm .tick {
		color: var(--good, #46a758);
		margin-right: 0.4rem;
	}

	.setupguide {
		display: flex;
		gap: var(--space-2);
		align-items: baseline;
		margin: 0 0 var(--space-3);
		font-size: 0.8rem;
		color: var(--muted);
	}
	.setupguide svg {
		flex: 0 0 auto;
		width: 0.85rem;
		height: 0.85rem;
		position: relative;
		top: 2px;
	}
	.fields {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(220px, 1fr));
		gap: var(--space-3);
	}
	.field {
		display: flex;
		flex-direction: column;
		gap: var(--space-1);
		min-width: 0;
	}
	.flabel {
		font-family: var(--font-mono);
		font-size: 0.78rem;
	}
	.fhelp {
		font-size: 0.72rem;
	}
	.field input {
		border-radius: var(--radius-sm);
		padding: 0.45rem 0.6rem;
		min-width: 0;
	}
	.credactions {
		display: flex;
		align-items: center;
		justify-content: flex-end;
		gap: var(--space-3);
		margin-top: var(--space-3);
	}
	.msg {
		margin-right: auto;
		font-size: 0.82rem;
	}
	.ghost,
	.primary {
		padding: 0.4rem 0.9rem;
		border-radius: var(--radius-sm);
		font-size: 0.88rem;
	}
	.ghost {
		background: transparent;
	}
	.primary {
		background: var(--accent);
		border-color: var(--accent);
		color: #fff;
		font-weight: 600;
	}
	button:disabled {
		opacity: 0.55;
		cursor: not-allowed;
	}

	.kindchips {
		display: flex;
		gap: var(--space-2);
		flex-wrap: wrap;
	}
	.kchip {
		all: unset;
		cursor: pointer;
		display: inline-flex;
		align-items: center;
		gap: 0.45rem;
		padding: 0.3rem 0.8rem;
		border: 1px solid var(--border);
		border-radius: 9999px;
		font-size: 0.82rem;
		color: var(--muted);
		transition:
			background var(--ease),
			color var(--ease),
			border-color var(--ease);
	}
	.kchip:hover:not(:disabled) {
		border-color: var(--accent);
	}
	.kchip.on {
		background: var(--accent-soft);
		border-color: var(--accent);
		color: var(--accent);
	}
	.kstate {
		font-size: 0.68rem;
		text-transform: uppercase;
		letter-spacing: 0.06em;
		opacity: 0.85;
	}
	.kchip:disabled {
		cursor: default;
	}

	.card.danger {
		border-color: color-mix(in srgb, var(--bad, #e5484d) 35%, var(--border));
	}
	.dgrow {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--space-4);
		flex-wrap: wrap;
	}
	.dgrow h2 {
		margin: 0 0 2px;
	}
	.dghint {
		margin: 0;
		font-size: 0.78rem;
		color: var(--muted);
		max-width: 44ch;
	}
	.dgactions {
		display: flex;
		gap: var(--space-2);
	}
	.dgbtn {
		padding: var(--space-1) var(--space-3);
		font-size: 0.85rem;
		font-weight: 600;
		background: var(--bad, #e5484d);
		border-color: var(--bad, #e5484d);
		color: #fff;
	}
	.dgbtn.outline {
		background: var(--surface-2);
		border-color: var(--border);
		color: var(--muted);
		font-weight: 400;
	}
	.dgbtn.outline:hover {
		border-color: #e0566f;
		background: rgba(224, 86, 111, 0.1);
		color: #e0566f;
	}
	.dgbtn:disabled {
		opacity: 0.6;
		cursor: not-allowed;
	}

	@media (max-width: 640px) {
		.pdetail {
			padding: var(--space-5);
		}
	}
</style>
