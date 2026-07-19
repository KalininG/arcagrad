<script>
	import { onMount } from 'svelte';
	import { kinds as kindsApi, media } from '$lib/api.js';
	import { kindLabel } from '$lib/kinds.js';
	import { CAP_LABEL, monogram, tintOf } from '$lib/plugins/common.js';
	import { install, realVersion, realAuthor } from '$lib/plugins/catalog.js';

	let { entry, onclose, oninstalled } = $props();
	const metaLine = [
		realVersion(entry.version) && `v${realVersion(entry.version)}`,
		realAuthor(entry.author) && `@${realAuthor(entry.author)}`,
	]
		.filter(Boolean)
		.join(' · ');

	let kindList = $state([]);
	let cells = $state(new Map());
	let picked = $state(new Set());
	let phase = $state('idle');
	let error = $state('');

	onMount(async () => {
		try {
			kindList = (await kindsApi.list()) ?? [];
			const rows = await Promise.all(kindList.map((k) => kindsApi.plugins(k.kind)));
			const m = new Map();
			kindList.forEach((k, i) => {
				m.set(
					k.kind,
					new Map((rows[i] ?? []).map((r) => [r.id, { enabled: r.enabled, auto: r.auto }])),
				);
			});
			cells = m;
			picked = new Set(
				kindList.filter((k) => m.get(k.kind)?.get(entry.id)?.enabled).map((k) => k.kind),
			);
		} catch {
			/* ignored */
		}
	});

	const togglePick = (kind) => {
		const s = new Set(picked);
		s.has(kind) ? s.delete(kind) : s.add(kind);
		picked = s;
	};

	const credLine = $derived.by(() => {
		const a = entry.auth;
		if (!a?.fields?.length) return null;
		const what =
			a.fields.length === 1
				? (a.fields[0].label ?? a.fields[0].name)
				: `${a.fields.length} credential fields`;
		const need = (a.required_for ?? []).map((c) => (CAP_LABEL[c] ?? c).toLowerCase());
		return need.length ? `Use ${what} — needed for ${need.join(', ')}` : `Optionally use ${what}`;
	});

	async function doInstall() {
		if (phase !== 'idle') return;
		phase = 'installing';
		error = '';
		try {
			await install(entry.id);
			for (const k of kindList) {
				const row = cells.get(k.kind) ?? new Map();
				const cur = row.get(entry.id)?.enabled ?? false;
				const want = picked.has(k.kind);
				if (cur === want) continue;
				row.set(entry.id, { enabled: want, auto: false });
				await kindsApi.setPlugins(
					k.kind,
					[...row].filter(([, c]) => c.enabled).map(([id]) => id),
					[...row].filter(([, c]) => c.enabled && c.auto).map(([id]) => id),
				);
			}
			phase = 'done';
			setTimeout(() => oninstalled?.(), 450);
		} catch (e) {
			phase = 'idle';
			error = e?.message ?? String(e);
		}
	}
</script>

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div
	class="backdrop"
	onclick={(e) => e.target === e.currentTarget && phase !== 'installing' && onclose?.()}
>
	<div class="sheet" role="dialog" aria-modal="true" aria-label={`Install ${entry.name}`}>
		<div class="head">
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
			<div class="idb">
				<h2>Install {entry.name}</h2>
				{#if metaLine}<p class="hmeta">{metaLine}</p>{/if}
			</div>
		</div>

		<p class="tl">This plugin can</p>
		<div class="prow">
			<svg
				viewBox="0 0 24 24"
				fill="none"
				stroke="currentColor"
				stroke-width="2"
				stroke-linecap="round"
				stroke-linejoin="round"
				><circle cx="12" cy="12" r="9" /><path
					d="M3 12h18M12 3a15 15 0 0 1 0 18M12 3a15 15 0 0 0 0 18"
				/></svg
			>
			<span>
				Connect to
				{#each entry.hosts as h, i (h)}{#if i}{i === entry.hosts.length - 1 ? ' and ' : ', '}{/if}<b
						>{h}</b
					>{/each}
			</span>
		</div>
		{#if credLine}
			<div class="prow">
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
				<span>{credLine}</span>
			</div>
		{/if}
		<div class="prow">
			<svg
				viewBox="0 0 24 24"
				fill="none"
				stroke="currentColor"
				stroke-width="2"
				stroke-linecap="round"
				stroke-linejoin="round"><path d="M13 2 3 14h7l-1 8 10-12h-7z" /></svg
			>
			<span>{entry.capabilities.map((c) => CAP_LABEL[c] ?? c).join(' · ')}</span>
		</div>

		{#if kindList.length}
			<div class="divider"></div>
			<p class="tl">Enable for</p>
			<div class="chips">
				{#each kindList as k (k.kind)}
					<button
						class="chip"
						class:on={picked.has(k.kind)}
						type="button"
						disabled={phase !== 'idle'}
						onclick={() => togglePick(k.kind)}
					>
						{#if picked.has(k.kind)}<svg
								viewBox="0 0 20 20"
								fill="none"
								stroke="currentColor"
								stroke-width="2.5"
								stroke-linecap="round"
								stroke-linejoin="round"><path d="M4 10l4 4 8-8" /></svg
							>{/if}
						{kindLabel(k.kind)}
					</button>
				{/each}
			</div>
			<p class="hint">
				Where this plugin is offered (scraping, browse). You can change this any time.
			</p>
			{#if picked.size === 0}
				<p class="nokinds">
					<svg
						viewBox="0 0 24 24"
						fill="none"
						stroke="currentColor"
						stroke-width="2"
						stroke-linecap="round"
						stroke-linejoin="round"
						><path
							d="M10.3 3.9 1.8 18a2 2 0 0 0 1.7 3h17a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0z"
						/><path d="M12 9v4M12 17h.01" /></svg
					>
					No kinds selected — the plugin will install but won't be offered anywhere until you enable it
					for a kind here or on its page.
				</p>
			{/if}
		{/if}

		{#if error}<p class="err">{error}</p>{/if}

		<div class="actions">
			<button class="ghost" type="button" onclick={onclose} disabled={phase === 'installing'}
				>Cancel</button
			>
			<button class="primary" type="button" onclick={doInstall} disabled={phase !== 'idle'}>
				{#if phase === 'installing'}
					<span class="ring" aria-hidden="true"></span> Installing…
				{:else if phase === 'done'}
					<svg
						class="check"
						viewBox="0 0 20 20"
						fill="none"
						stroke="currentColor"
						stroke-width="2.5"
						stroke-linecap="round"
						stroke-linejoin="round"><path d="M4 10l4 4 8-8" /></svg
					> Installed
				{:else}
					Install plugin
				{/if}
			</button>
		</div>
	</div>
</div>

<style>
	.backdrop {
		position: fixed;
		inset: 0;
		z-index: 100;
		background: rgba(0, 0, 0, 0.55);
		display: flex;
		align-items: center;
		justify-content: center;
		padding: var(--space-5);
	}
	.sheet {
		width: min(30rem, 100%);
		max-height: min(85vh, 40rem);
		overflow-y: auto;
		border: 1px solid var(--border);
		border-radius: var(--radius-lg, 14px);
		background: var(--surface);
		box-shadow: var(--shadow-lg, 0 12px 40px rgba(0, 0, 0, 0.5));
		padding: var(--space-5) var(--space-6);
	}
	.head {
		display: flex;
		align-items: center;
		gap: var(--space-4);
		margin-bottom: var(--space-4);
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
		font-size: 1.3rem;
	}
	.idb {
		min-width: 0;
	}
	.head h2 {
		margin: 0;
		font-family: var(--font-display);
		font-size: 1.15rem;
		font-weight: 700;
	}
	.hmeta {
		margin: 0.15rem 0 0;
		font-size: 0.76rem;
		color: var(--muted);
	}
	.tl {
		margin: 0 0 var(--space-2);
		font-size: 0.68rem;
		letter-spacing: 0.14em;
		text-transform: uppercase;
		color: var(--muted);
	}
	.prow {
		display: flex;
		gap: var(--space-3);
		align-items: baseline;
		font-size: 0.85rem;
		padding: 0.25rem 0;
	}
	.prow svg {
		flex: 0 0 auto;
		width: 0.95rem;
		height: 0.95rem;
		color: var(--muted);
		position: relative;
		top: 2px;
	}
	.prow b {
		font-weight: 600;
	}
	.divider {
		border-top: 1px solid var(--border);
		margin: var(--space-4) 0;
	}
	.chips {
		display: flex;
		gap: var(--space-2);
		flex-wrap: wrap;
	}
	.chip {
		all: unset;
		cursor: pointer;
		display: inline-flex;
		align-items: center;
		gap: 0.35rem;
		padding: 0.32rem 0.85rem;
		border: 1px solid var(--border);
		border-radius: 9999px;
		font-size: 0.84rem;
		color: var(--muted);
		transition:
			background var(--ease),
			color var(--ease),
			border-color var(--ease);
	}
	.chip:hover:not(:disabled) {
		border-color: var(--accent);
	}
	.chip.on {
		background: var(--accent-soft);
		border-color: var(--accent);
		color: var(--accent);
	}
	.chip svg {
		width: 0.75rem;
		height: 0.75rem;
	}
	.chip:disabled {
		cursor: default;
		opacity: 0.7;
	}
	.hint {
		margin: var(--space-2) 0 0;
		font-size: 0.74rem;
		color: var(--muted);
	}
	.nokinds {
		display: flex;
		gap: var(--space-2);
		align-items: baseline;
		margin: var(--space-3) 0 0;
		padding: var(--space-2) var(--space-3);
		border: 1px solid color-mix(in srgb, #d29922 45%, var(--border));
		border-radius: var(--radius-sm);
		font-size: 0.78rem;
		color: #d29922;
	}
	.nokinds svg {
		flex: 0 0 auto;
		width: 0.85rem;
		height: 0.85rem;
		position: relative;
		top: 2px;
	}
	.err {
		margin: var(--space-3) 0 0;
		color: var(--bad, #e5484d);
		font-size: 0.82rem;
	}
	.actions {
		display: flex;
		justify-content: flex-end;
		gap: var(--space-3);
		margin-top: var(--space-5);
	}
	.ghost,
	.primary {
		padding: 0.45rem 1rem;
		border-radius: var(--radius-sm);
		font-size: 0.88rem;
	}
	.ghost {
		background: transparent;
	}
	.primary {
		display: inline-flex;
		align-items: center;
		gap: 0.45rem;
		background: var(--accent);
		border-color: var(--accent);
		color: #fff;
		font-weight: 600;
	}
	.primary:disabled,
	.ghost:disabled {
		opacity: 0.6;
		cursor: not-allowed;
	}
	.ring {
		width: 0.85rem;
		height: 0.85rem;
		border: 2px solid rgba(255, 255, 255, 0.35);
		border-top-color: #fff;
		border-radius: 50%;
		animation: spin 0.8s linear infinite;
	}
	.check {
		width: 0.9rem;
		height: 0.9rem;
	}
</style>
