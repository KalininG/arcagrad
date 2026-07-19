<script>
	import { onMount, onDestroy } from 'svelte';
	import {
		items as itemsApi,
		series as seriesApi,
		kinds as kindsApi,
		credentials as credsApi,
		jobs as jobsApi,
		ApiError,
	} from '$lib/api.js';
	import Dropdown from '$lib/components/ui/Dropdown.svelte';
	import IdentifyModal from '$lib/components/IdentifyModal.svelte';
	import Modal from '$lib/components/ui/Modal.svelte';

	let {
		itemId,
		kind,
		pageCount = null,
		sources = [],
		scope = 'item',
		onClose,
		onUpdated,
	} = $props();
	const scrapeTarget = (id, plugin, opts) =>
		scope === 'series' ? seriesApi.scrape(id, plugin, opts) : itemsApi.scrape(id, plugin, opts);
	const refreshTarget = (id) => (scope === 'series' ? seriesApi.get(id) : itemsApi.detail(id));

	let pluginList = $state([]);
	let selectedId = $state('');
	let credMap = $state(new Map());
	let credInputs = $state({});
	let ref = $state('');
	let booting = $state(true);
	let loadError = $state('');
	let submitting = $state(false);
	let status = $state('');
	let submitError = $state('');
	let applied = $state(null);

	let alive = true;
	onDestroy(() => (alive = false));

	const selected = $derived(pluginList.find((p) => p.id === selectedId) ?? null);
	let showIdentify = $state(false);
	const canIdentify = $derived(
		scope === 'item' && pageCount != null && !!selected?.capabilities?.includes('identify'),
	);
	const refInput = $derived(
		selected?.reference_inputs?.scrape ?? {
			label: 'Source reference',
			placeholder: 'Source-specific URL or identifier',
			help: 'Optional. Selects an exact source entry instead of searching by title.',
			required: false,
		},
	);
	const authFields = $derived(selected?.auth?.fields ?? []);
	const credsRequired = $derived(!!selected?.auth?.required_for?.includes('scrape'));
	const configured = $derived(credMap.get(selected?.source) ?? new Set());
	const missingFields = $derived(authFields.filter((f) => !configured.has(f.name)));

	onMount(async () => {
		try {
			const [kp, creds] = await Promise.all([
				kindsApi.plugins(kind),
				credsApi.list().catch(() => []),
			]);
			const pl = (kp ?? []).filter((p) => p.enabled && p.capabilities?.includes('scrape'));
			pluginList = pl;
			credMap = new Map((creds ?? []).map((c) => [c.source, new Set(c.fields)]));
			selectedId = pl[0]?.id ?? '';
			prefillRef(selectedId);
		} catch (e) {
			loadError = e?.message ?? String(e);
		} finally {
			booting = false;
		}
	});

	function onSelect() {
		credInputs = {};
		status = '';
		submitError = '';
		applied = null;
		prefillRef(selectedId);
	}

	function prefillRef(id) {
		const src = pluginList.find((p) => p.id === id)?.source;
		const match = sources.find((s) => s.source === src);
		ref = match ? match.url : '';
	}

	function filledData() {
		const data = {};
		for (const f of authFields) {
			const v = (credInputs[f.name] ?? '').trim();
			if (v) data[f.name] = v;
		}
		return data;
	}

	const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
	const label = (f) => f.label ?? f.name;

	async function pollJob(jobId) {
		for (const d of [800, 1200, 1600, 2000, 2500, 3000]) {
			await sleep(d);
			if (!alive) return null;
			let j;
			try {
				j = await jobsApi.get(jobId);
			} catch (e) {
				if (e instanceof ApiError && e.status === 404) return { state: 'done' };
				continue;
			}
			if (j.state === 'done' || j.state === 'failed') return j;
		}
		return null;
	}

	async function applyResult(terminal) {
		if (terminal.state === 'failed') {
			status = '';
			submitError = terminal.result?.error ?? 'Scrape failed.';
			return;
		}
		applied = terminal.result?.applied ?? null;
		status = 'done';
		const fresh = await refreshTarget(itemId).catch(() => null);
		if (alive) onUpdated?.(fresh);
	}

	async function submit() {
		if (!selected || submitting) return;
		submitError = '';
		applied = null;
		const data = filledData();
		if (refInput.required && !ref.trim()) {
			submitError = `${refInput.label} is required.`;
			return;
		}

		if (credsRequired) {
			const missing = authFields.filter(
				(f) => f.required && !configured.has(f.name) && !data[f.name],
			);
			if (missing.length) {
				submitError = `${selected.name} needs ${missing.map(label).join(', ')} before scraping.`;
				return;
			}
		}

		submitting = true;
		status = 'queueing';
		try {
			if (Object.keys(data).length) {
				await credsApi.set(selected.source, data);
				credMap = new Map(credMap).set(
					selected.source,
					new Set([...(credMap.get(selected.source) ?? []), ...Object.keys(data)]),
				);
			}

			const res = await scrapeTarget(itemId, selected.id, {
				ref: ref.trim() || undefined,
				wait: true,
			});

			let terminal = res?.state ? { state: res.state, result: res.result } : null;
			if (!terminal) {
				status = 'polling';
				terminal = await pollJob(res.job_id);
			}
			if (!alive) return;

			if (!terminal) status = 'timeout';
			else await applyResult(terminal);
		} catch (e) {
			status = '';
			submitError = e?.message ?? String(e);
		} finally {
			if (alive) submitting = false;
		}
	}
</script>

<Modal title="Add metadata" width="min(420px, 100%)" {onClose}>
	<div class="body">
		{#if booting}
			<p class="muted">Loading plugins…</p>
		{:else if loadError}
			<p class="err">{loadError}</p>
		{:else if pluginList.length === 0}
			<p class="muted">
				No scraper plugins are enabled for this kind. <a class="link" href="/plugins"
					>Enable one on the Plugins page</a
				>.
			</p>
		{:else}
			<div class="field">
				<span class="flabel">Plugin</span>
				<Dropdown
					bind:value={selectedId}
					options={pluginList.map((p) => ({ value: p.id, label: p.name }))}
					onchange={onSelect}
					disabled={submitting}
				/>
			</div>

			{#if selected?.source}
				<p class="source muted">Source: {selected.source}</p>
			{/if}

			<label class="field">
				<span class="flabel"
					>{refInput.label}{#if !refInput.required}
						<span class="muted">(optional)</span>{/if}</span
				>
				<input
					type="text"
					autocomplete="off"
					placeholder={refInput.placeholder}
					bind:value={ref}
					disabled={submitting}
				/>
				{#if refInput.help}<span class="help muted">{refInput.help}</span>{/if}
			</label>

			{#if canIdentify}
				<div class="ordiv"><span>or</span></div>
				<button
					class="identify"
					type="button"
					onclick={() => (showIdentify = true)}
					disabled={submitting}
					title={`Match this cover on ${selected.name} and scrape from the top result.`}
				>
					<svg
						viewBox="0 0 24 24"
						fill="none"
						stroke="currentColor"
						stroke-width="2"
						stroke-linecap="round"
						stroke-linejoin="round"><circle cx="11" cy="11" r="7" /><path d="m21 21-4.3-4.3" /></svg
					>
					Identify by cover
				</button>
			{/if}

			{#if missingFields.length}
				{#if credsRequired}
					<div class="creds">
						<p class="flabel">Credentials</p>
						{#each missingFields as f (f.name)}
							<label class="field">
								<span class="subfield">{label(f)}</span>
								<input
									type={f.secret ? 'password' : 'text'}
									autocomplete="off"
									placeholder={f.required ? 'required' : 'optional'}
									bind:value={credInputs[f.name]}
									disabled={submitting}
								/>
								{#if f.help}<span class="help muted">{f.help}</span>{/if}
							</label>
						{/each}
						<p class="hint muted">
							Stored on the server for this source; secrets are never shown back.
						</p>
					</div>
				{:else}
					<p class="crednote">
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
						<span class="muted">No credentials set (optional).</span>
						<a class="link setnow" href="/plugins/{selected.id}">Set now</a>
					</p>
				{/if}
			{/if}

			{#if status === 'queueing'}
				<p class="status muted">Queuing…</p>
			{:else if status === 'polling'}
				<p class="status muted">
					Scraping… you can close this — it'll keep running in the background.
				</p>
			{:else if status === 'done'}
				<p class="status ok">
					{applied != null
						? `Metadata updated — ${applied} tag${applied === 1 ? '' : 's'} applied.`
						: 'Metadata updated.'}
				</p>
			{:else if status === 'timeout'}
				<p class="status muted">
					Still scraping in the background — reopen this item shortly to see the result.
				</p>
			{/if}

			{#if submitError}<p class="err">{submitError}</p>{/if}
		{/if}
	</div>

	<footer class="mfoot">
		<button class="ghost" onclick={() => onClose?.()} type="button">
			{status === 'done' ? 'Close' : submitting ? 'Run in background' : 'Cancel'}
		</button>
		{#if pluginList.length && status !== 'done'}
			<button class="primary" onclick={submit} type="button" disabled={submitting || !selected}>
				{submitting ? 'Working…' : 'Scrape'}
			</button>
		{/if}
	</footer>
</Modal>

{#if showIdentify}
	<IdentifyModal
		{itemId}
		{kind}
		{pageCount}
		pluginId={selected?.id}
		onClose={() => (showIdentify = false)}
		onScraped={(fresh) => {
			onUpdated?.(fresh);
			onClose?.();
		}}
	/>
{/if}

<style>
	.ordiv {
		display: flex;
		align-items: center;
		gap: var(--space-3);
		margin: calc(-1 * var(--space-1)) 0;
		color: var(--muted);
		font-size: 0.72rem;
	}
	.ordiv::before,
	.ordiv::after {
		content: '';
		flex: 1;
		height: 1px;
		background: var(--border);
	}
	.identify {
		display: flex;
		align-items: center;
		justify-content: center;
		gap: 7px;
		width: 100%;
		background: transparent;
		border: 1px solid var(--border);
		border-radius: var(--radius-sm);
		color: var(--text);
		padding: 0.5rem;
		font-size: 0.85rem;
		cursor: pointer;
	}
	.identify:hover:not(:disabled) {
		border-color: var(--accent);
		color: var(--accent);
	}
	.identify svg {
		width: 15px;
		height: 15px;
	}
	.identify:disabled {
		opacity: 0.55;
		cursor: default;
	}
	.body {
		display: flex;
		flex-direction: column;
		gap: var(--space-4);
		padding: var(--space-5);
	}
	.field {
		display: flex;
		flex-direction: column;
		gap: var(--space-2);
	}
	input {
		border-radius: var(--radius-sm);
		padding: 0.45rem 0.6rem;
	}
	.flabel {
		font-size: 0.78rem;
		text-transform: uppercase;
		letter-spacing: 0.06em;
		color: var(--muted);
	}
	.subfield {
		font-size: 0.8rem;
		color: var(--text);
		font-family: var(--font-mono);
	}
	.help {
		font-size: 0.72rem;
	}
	.source {
		margin: calc(-1 * var(--space-2)) 0 0;
		font-size: 0.78rem;
	}
	.creds {
		display: flex;
		flex-direction: column;
		gap: var(--space-3);
		padding: var(--space-4);
		border: 1px solid var(--border);
		border-radius: var(--radius-sm);
		background: transparent;
	}
	.hint {
		margin: 0;
		font-size: 0.72rem;
	}
	.crednote {
		display: flex;
		align-items: center;
		gap: 0.4rem;
		margin: 0;
		font-size: 0.75rem;
	}
	.crednote svg {
		width: 14px;
		height: 14px;
		flex: 0 0 auto;
		color: #d29922;
	}
	.crednote span {
		min-width: 0;
	}
	.setnow {
		margin-left: auto;
	}
	.status {
		margin: 0;
		font-size: 0.85rem;
	}
	.status.ok {
		color: var(--good);
	}
	.muted {
		color: var(--muted);
	}
	.link {
		color: var(--accent);
		text-decoration: underline;
		text-underline-offset: 2px;
	}
	.link:hover {
		color: var(--text);
	}
	.mfoot {
		display: flex;
		align-items: center;
		justify-content: flex-end;
		gap: var(--space-3);
		padding: var(--space-4) var(--space-5);
		border-top: 1px solid var(--border);
	}
	.ghost,
	.primary {
		padding: 0.45rem 1rem;
		border-radius: var(--radius-sm);
		font-size: 0.9rem;
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
	.primary:hover:not(:disabled) {
		filter: brightness(1.1);
	}
	button:disabled {
		opacity: 0.55;
		cursor: not-allowed;
	}
</style>
