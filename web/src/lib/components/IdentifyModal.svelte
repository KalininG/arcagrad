<script>
	import { onMount, onDestroy } from 'svelte';
	import { items as itemsApi, kinds as kindsApi, media } from '$lib/api.js';
	import Dropdown from '$lib/components/ui/Dropdown.svelte';

	let { itemId, kind, pageCount = null, pluginId = null, onClose, onScraped } = $props();

	let booting = $state(true);
	let loadError = $state('');
	let plugins = $state([]);
	let plugin = $state(null);
	let pickedId = $state('');
	let running = $state(false);
	let runError = $state('');
	let candidates = $state(null);
	let scrapingRef = $state('');
	let scrapeError = $state('');

	const showPicker = $derived(!pluginId && plugins.length > 1);

	let alive = true;
	onDestroy(() => {
		alive = false;
		window.removeEventListener('keydown', onKey);
	});

	onMount(async () => {
		window.addEventListener('keydown', onKey);
		try {
			const kp = await kindsApi.plugins(kind);
			plugins = (kp ?? []).filter((p) => p.enabled && p.capabilities?.includes('identify'));
			plugin = (pluginId && plugins.find((p) => p.id === pluginId)) || plugins[0] || null;
			pickedId = plugin?.id ?? '';
		} catch (e) {
			loadError = e?.message ?? String(e);
		} finally {
			booting = false;
		}
		if (plugin) runIdentify();
	});

	function onKey(e) {
		if (e.key === 'Escape') onClose?.();
	}

	function onSourceChange() {
		if (pickedId === plugin?.id) return;
		plugin = plugins.find((p) => p.id === pickedId) ?? plugin;
		candidates = null;
		scrapeError = '';
		runIdentify();
	}

	async function runIdentify() {
		running = true;
		runError = '';
		candidates = null;
		try {
			const res = await itemsApi.identify(itemId, plugin.id);
			if (alive) candidates = res?.candidates ?? [];
		} catch (e) {
			if (alive) runError = e?.message ?? String(e);
		} finally {
			if (alive) running = false;
		}
	}

	async function scrapeMatch(c) {
		if (!c.plugin_id) return;
		scrapingRef = c.reference;
		scrapeError = '';
		try {
			await itemsApi.scrape(itemId, c.plugin_id, { ref: c.reference, wait: true });
			const fresh = await itemsApi.detail(itemId);
			onScraped?.(fresh);
			onClose?.();
		} catch (e) {
			if (alive) scrapeError = e?.message ?? String(e);
		} finally {
			if (alive) scrapingRef = '';
		}
	}

	const pageFit = (c) => {
		if (pageCount == null || c.page_count == null) return 'unknown';
		const d = Math.abs(c.page_count - pageCount);
		if (d === 0) return 'exact';
		if (d <= 3) return 'close';
		return 'mismatch';
	};
	const simColor = (s) => (s >= 90 ? 'var(--good)' : s >= 55 ? 'var(--text)' : 'var(--muted)');
</script>

<!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
<div class="overlay" onclick={() => onClose?.()}>
	<div
		class="modal"
		role="dialog"
		aria-modal="true"
		aria-label="Identify this item"
		onclick={(e) => e.stopPropagation()}
	>
		<header class="mhead">
			<div>
				<h2>Identify this item</h2>
				<p class="ctx muted">
					Your item{#if pageCount != null}
						· {pageCount} pages{/if}{#if plugin}
						· searched by cover hash on {plugin.name}{/if}
				</p>
			</div>
			<button class="x" onclick={() => onClose?.()} aria-label="Close" type="button">×</button>
		</header>

		<div class="body">
			{#if booting}
				<p class="muted">Loading…</p>
			{:else if loadError}
				<p class="err">{loadError}</p>
			{:else if !plugin}
				<p class="muted">
					No identify source is enabled for this kind.
					<a class="link" href="/plugins">Enable one on the Plugins page</a>.
				</p>
			{:else}
				{#if showPicker}
					<div class="picker">
						<span class="plabel">Source</span>
						<div class="pickerctl">
							<Dropdown
								bind:value={pickedId}
								options={plugins.map((p) => ({ value: p.id, label: p.name }))}
								onchange={onSourceChange}
								disabled={running}
							/>
						</div>
					</div>
				{/if}
				{#if running}
					<p class="muted searching"><span class="spin"></span> Searching {plugin.name}…</p>
				{:else if runError}
					<p class="err">{runError}</p>
					<button class="retry" type="button" onclick={runIdentify}>Try again</button>
				{:else if candidates && candidates.length === 0}
					<p class="muted empty">
						No matches on {plugin.name}. The file may be re-encoded — an exact-hash search only
						finds byte-identical images.
					</p>
				{:else if candidates}
					<div class="listhead">
						<span class="muted"
							>{candidates.length}
							{candidates.length === 1 ? 'match' : 'matches'} on {plugin.name}</span
						>
						<span class="muted">ranked by page-count fit</span>
					</div>
					{#if scrapeError}<p class="err">{scrapeError}</p>{/if}
					<div class="rows">
						{#each candidates as c, i (c.reference)}
							<div class="row" class:top={i === 0}>
								{#if c.cover_url && c.plugin_id}
									<a
										class="cover"
										href={c.url}
										target="_blank"
										rel="noopener"
										title="Open on {c.source}"
									>
										<img src={media.pluginImage(c.plugin_id, c.cover_url)} alt="" loading="lazy" />
									</a>
								{/if}
								<div class="info">
									<a class="ctitle" href={c.url} target="_blank" rel="noopener"
										>{c.title || c.reference}<span class="extic">↗</span></a
									>
									<div class="chips">
										<span class="chip sim" style={`color:${simColor(c.similarity)}`}
											>{Math.round(c.similarity)}% match</span
										>
										<span class="chip src">{c.source}</span>
										{#if c.page_count != null}
											<span class="chip pages {pageFit(c)}">
												{#if pageFit(c) === 'exact'}✓
												{/if}{#if pageFit(c) === 'mismatch'}⚠
												{/if}{c.page_count.toLocaleString()} pages{#if pageFit(c) === 'mismatch'}
													· length mismatch{/if}
											</span>
										{/if}
									</div>
								</div>
								{#if c.plugin_id}
									<button
										class="act"
										class:primary={i === 0}
										type="button"
										disabled={!!scrapingRef}
										onclick={() => scrapeMatch(c)}
									>
										{scrapingRef === c.reference ? 'Scraping…' : 'Scrape with this match'}
									</button>
								{/if}
							</div>
						{/each}
					</div>
					<p class="note muted">
						An exact image can appear in several galleries. The page count picks the one that's
						actually your item — a big compilation containing the same cover sinks to the bottom.
					</p>
				{/if}
			{/if}
		</div>
	</div>
</div>

<style>
	.overlay {
		position: fixed;
		inset: 0;
		z-index: 100;
		display: flex;
		align-items: center;
		justify-content: center;
		padding: var(--space-4);
		background: rgba(0, 0, 0, 0.55);
	}
	.modal {
		width: 100%;
		max-width: 560px;
		max-height: 85vh;
		display: flex;
		flex-direction: column;
		background: var(--surface);
		border: 1px solid var(--border);
		border-radius: 12px;
		overflow: hidden;
	}
	.mhead {
		display: flex;
		align-items: flex-start;
		justify-content: space-between;
		gap: var(--space-4);
		padding: var(--space-4) var(--space-5);
		border-bottom: 1px solid var(--border);
	}
	.mhead h2 {
		margin: 0;
		font-size: 1rem;
	}
	.ctx {
		margin: 3px 0 0;
		font-size: 0.75rem;
	}
	.x {
		background: transparent;
		border: none;
		color: var(--muted);
		font-size: 1.4rem;
		line-height: 1;
		cursor: pointer;
	}
	.body {
		padding: var(--space-5);
		overflow-y: auto;
	}
	.muted {
		color: var(--muted);
		font-size: 0.85rem;
	}
	.err {
		color: #e0566f;
		font-size: 0.85rem;
		margin: 0 0 var(--space-3);
	}
	.link,
	.act.ext {
		color: var(--accent);
	}
	.searching {
		display: flex;
		align-items: center;
		gap: var(--space-2);
	}
	.spin {
		width: 14px;
		height: 14px;
		border: 2px solid var(--border);
		border-top-color: var(--accent);
		border-radius: 50%;
		animation: spin 0.7s linear infinite;
	}
	.retry {
		background: transparent;
		border: 1px solid var(--border);
		border-radius: var(--radius);
		color: var(--text);
		padding: var(--space-2) var(--space-3);
		font-size: 0.8rem;
		cursor: pointer;
	}
	.picker {
		display: flex;
		align-items: center;
		gap: var(--space-3);
		margin-bottom: var(--space-4);
	}
	.plabel {
		flex: 0 0 auto;
		font-size: 0.72rem;
		text-transform: uppercase;
		letter-spacing: 0.06em;
		color: var(--muted);
	}
	.pickerctl {
		flex: 1;
		min-width: 0;
	}
	.listhead {
		display: flex;
		justify-content: space-between;
		font-size: 0.72rem;
		margin-bottom: var(--space-3);
	}
	.rows {
		display: flex;
		flex-direction: column;
		gap: var(--space-2);
	}
	.row {
		display: flex;
		align-items: center;
		gap: var(--space-3);
		padding: var(--space-3);
		border: 1px solid var(--border);
		border-radius: var(--radius);
	}
	.row.top {
		border-color: var(--accent);
	}
	.cover {
		flex: 0 0 auto;
		width: 46px;
		height: 65px;
		border-radius: 6px;
		overflow: hidden;
		border: 1px solid var(--border);
		background: var(--surface-2);
		display: block;
	}
	.cover img {
		width: 100%;
		height: 100%;
		object-fit: cover;
		display: block;
	}
	.info {
		flex: 1;
		min-width: 0;
	}
	.ctitle {
		display: block;
		margin: 0;
		font-size: 0.85rem;
		font-weight: 600;
		color: var(--text);
		text-decoration: none;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}
	.ctitle:hover {
		text-decoration: underline;
	}
	.extic {
		color: var(--muted);
		font-size: 0.75rem;
		margin-left: 4px;
	}
	.chip.sim {
		background: var(--surface-2);
		border: 1px solid var(--border);
		font-weight: 600;
	}
	.chips {
		display: flex;
		gap: 6px;
		margin-top: 5px;
		flex-wrap: wrap;
	}
	.chip {
		font-size: 0.68rem;
		padding: 2px 7px;
		border-radius: 999px;
		white-space: nowrap;
	}
	.chip.src {
		background: color-mix(in srgb, var(--accent) 18%, transparent);
		color: var(--accent);
	}
	.chip.pages {
		background: var(--surface-2);
		color: var(--muted);
		border: 1px solid var(--border);
	}
	.chip.pages.exact {
		background: color-mix(in srgb, var(--good) 18%, transparent);
		color: var(--good);
		border-color: transparent;
	}
	.chip.pages.mismatch {
		background: rgba(217, 164, 65, 0.14);
		color: #d9a441;
		border-color: transparent;
	}
	.act {
		flex: 0 0 auto;
		white-space: nowrap;
		border: 1px solid var(--border-strong, var(--border));
		background: transparent;
		color: var(--text);
		border-radius: var(--radius);
		padding: 7px 12px;
		font-size: 0.8rem;
		cursor: pointer;
	}
	.act.primary {
		background: var(--accent);
		border-color: var(--accent);
		color: #fff;
	}
	.act.ext {
		text-decoration: none;
		border-color: var(--border);
	}
	.act:disabled {
		opacity: 0.55;
		cursor: default;
	}
	.note,
	.empty {
		margin: var(--space-4) 0 0;
		font-size: 0.72rem;
		line-height: 1.5;
	}
	.empty {
		margin: 0;
	}
</style>
