<script>
	import { items as itemsApi, series as seriesApi } from '$lib/api.js';
	import Modal from '$lib/components/ui/Modal.svelte';

	let { meta, mode = 'item', onClose, onUpdated } = $props();
	const isSeries = mode === 'series';

	const NAMESPACES = [
		'tag',
		'creator',
		'group',
		'parody',
		'character',
		'category',
		'demographic',
		'language',
	];
	const NS_ORDER = [
		'creator',
		'group',
		'parody',
		'character',
		'tag',
		'category',
		'demographic',
		'language',
	];

	const metaTitle = isSeries ? (meta.title ?? '') : (meta.name ?? '');
	let title = $state(metaTitle);
	let description = $state(meta.description ?? '');
	let override = $state(meta.modality_override ?? '');

	const titleChanged = $derived(title.trim() !== metaTitle && title.trim() !== '');
	const descChanged = $derived(description.trim() !== (meta.description ?? '').trim());
	const overrideChanged = $derived((override || null) !== (meta.modality_override ?? null));

	const keyOf = (t) => `${t.namespace}\n${t.value}\n${t.qualifier ?? 'none'}`;
	let removals = $state(new Set());
	let additions = $state([]);
	let addNs = $state('tag');
	let addValue = $state('');
	let forgets = $state(new Set());

	const grouped = $derived.by(() => {
		const by = new Map();
		for (const t of meta.tags ?? []) {
			if (!by.has(t.namespace)) by.set(t.namespace, []);
			by.get(t.namespace).push(t);
		}
		for (const a of additions) {
			if (!by.has(a.namespace)) by.set(a.namespace, []);
		}
		return NS_ORDER.filter((ns) => by.has(ns)).map((ns) => ({ ns, tags: by.get(ns) ?? [] }));
	});

	function toggleRemoval(t) {
		const k = keyOf(t);
		const next = new Set(removals);
		if (!next.delete(k)) next.add(k);
		removals = next;
	}
	function stageAdd() {
		const value = addValue.trim().toLowerCase();
		if (!value) return;
		const tag = { namespace: addNs, value, qualifier: 'none' };
		const exists = (meta.tags ?? []).some(
			(t) => keyOf(t) === keyOf(tag) && !removals.has(keyOf(t)),
		);
		const staged = additions.some((a) => keyOf(a) === keyOf(tag));
		if (!exists && !staged) additions = [...additions, tag];
		addValue = '';
	}
	function unstageAdd(tag) {
		additions = additions.filter((a) => keyOf(a) !== keyOf(tag));
	}
	function onAddKey(e) {
		if (e.key === 'Enter') {
			e.preventDefault();
			stageAdd();
		}
	}
	function toggleForget(source) {
		const next = new Set(forgets);
		if (!next.delete(source)) next.add(source);
		forgets = next;
	}
	const doomed = (t) =>
		(t.sources?.length ?? 0) > 0 && t.sources.every((s) => forgets.has(s)) && !(isSeries && t.leaf);
	const descDoomed = $derived(
		!!meta.description_source &&
			forgets.has(meta.description_source) &&
			!descChanged &&
			!!description.trim(),
	);

	const pending = $derived(
		(titleChanged ? 1 : 0) +
			(descChanged ? 1 : 0) +
			(overrideChanged ? 1 : 0) +
			removals.size +
			additions.length +
			forgets.size,
	);

	let saving = $state(false);
	let saveError = $state('');
	async function save() {
		if (saving || pending === 0) return;
		saving = true;
		saveError = '';
		try {
			const api = isSeries ? seriesApi : itemsApi;
			for (const s of forgets) {
				await api.forgetSource(meta.id, s);
			}
			for (const t of meta.tags ?? []) {
				if (removals.has(keyOf(t))) {
					try {
						await itemsApi.removeTag(meta.id, {
							namespace: t.namespace,
							value: t.value,
							qualifier: t.qualifier ?? 'none',
						});
					} catch (e) {
						if (e?.status !== 404) throw e;
					}
				}
			}
			for (const a of additions) {
				await itemsApi.addTag(meta.id, a);
			}
			const body = {};
			if (titleChanged) body.title = title.trim();
			if (descChanged) body.description = description.trim() === '' ? null : description.trim();
			if (!isSeries && overrideChanged) body.modality_override = override === '' ? null : override;
			const fresh = await api.editMetadata(meta.id, body);
			onUpdated?.(fresh);
			onClose?.();
		} catch (e) {
			saveError = e?.message ?? String(e);
		} finally {
			saving = false;
		}
	}
</script>

<Modal title="Edit metadata" width="min(620px, 100%)" closeOnEscape={false} {onClose}>
	<div class="body">
		<div class="field">
			<label class="f" for="em-title">Title</label>
			<input id="em-title" type="text" bind:value={title} />
			{#if meta.raw_title}
				<p class="prov" title={meta.raw_title}><b>Original:</b> {meta.raw_title}</p>
			{/if}
		</div>

		<div class="field">
			<div class="fieldrow">
				<label class="f" for="em-desc">Description</label>
				{#if meta.description_manual}
					<span class="hintchip" title="A hand-written description is never overwritten by scrapes"
						>manual — protected from scrapes</span
					>
				{/if}
			</div>
			<textarea
				id="em-desc"
				class:doomed={descDoomed}
				bind:value={description}
				placeholder="A short synopsis. Saving marks it manual — scrapes won't overwrite it."
			></textarea>
			{#if descDoomed}
				<p class="srcwarn">
					This synopsis came from <b>{meta.description_source}</b> — forgetting that source clears it.
					Edit it here to keep (a manual version).
				</p>
			{/if}
			{#if description.trim()}
				<div class="clearrow">
					<button class="clearlink" type="button" onclick={() => (description = '')}>
						Clear (let scrapes fill it again)
					</button>
				</div>
			{/if}
		</div>

		{#if !isSeries}
			<div class="field">
				<span class="f">Reading format</span>
				<div class="seg" role="radiogroup" aria-label="Reading format">
					<button type="button" class:on={override === ''} onclick={() => (override = '')}>
						Auto ({meta.modality_detected})
					</button>
					{#each ['paginated', 'reflowable', 'fixed'] as m (m)}
						<button type="button" class:on={override === m} onclick={() => (override = m)}
							>{m}</button
						>
					{/each}
				</div>
				<p class="seghint">
					Auto = content-detected. Override only if detection got it wrong — it changes which reader
					opens.
				</p>
			</div>
		{/if}

		<div class="field">
			<span class="f">Tags</span>
			<div class="tagrows">
				{#each grouped as g (g.ns)}
					<div class="trow">
						<span class="tns">{g.ns}:</span>
						<span class="chips">
							{#each g.tags as t (keyOf(t))}
								{@const gone = doomed(t)}
								<span
									class="chip"
									class:staged-del={removals.has(keyOf(t)) || gone}
									title={gone
										? `Removed by forgetting ${t.sources.join(', ')}`
										: isSeries && t.leaf && forgets.size && t.sources?.some((s) => forgets.has(s))
											? 'Also on a volume — a series-level forget can’t remove it'
											: undefined}
								>
									{t.value}
									{#if !isSeries}
										<button
											type="button"
											disabled={gone && !removals.has(keyOf(t))}
											title={gone
												? `Removed by forgetting ${t.sources.join(', ')}`
												: removals.has(keyOf(t))
													? 'Keep this tag'
													: 'Remove this tag'}
											onclick={() => toggleRemoval(t)}>×</button
										>
									{/if}
								</span>
							{/each}
							{#each additions.filter((a) => a.namespace === g.ns) as a (keyOf(a))}
								<span class="chip staged-add">
									{a.value}
									<button type="button" title="Undo add" onclick={() => unstageAdd(a)}>×</button>
								</span>
							{/each}
						</span>
					</div>
				{/each}
			</div>
			{#if isSeries}
				<p class="seghint">
					Series tags come from series scrapes and the volumes' own tags — edit a volume's tags on
					its page.
				</p>
			{:else}
				<div class="addrow">
					<select bind:value={addNs} aria-label="Namespace">
						{#each NAMESPACES as ns (ns)}<option value={ns}>{ns}</option>{/each}
					</select>
					<input
						type="text"
						bind:value={addValue}
						onkeydown={onAddKey}
						placeholder="Add a tag…"
						aria-label="Tag value"
					/>
					<button class="add" type="button" onclick={stageAdd}>Add</button>
				</div>
			{/if}
		</div>

		{#if (meta.sources ?? []).length}
			<div class="field">
				<span class="f">Sources</span>
				<div class="chips srcchips">
					{#each meta.sources ?? [] as s (s.source)}
						<span class="chip srcchip" class:staged-del={forgets.has(s.source)} title={s.url}>
							<b>{s.source}</b>
							<span class="srcurl">{s.url}</span>
							<button
								type="button"
								title={forgets.has(s.source)
									? 'Keep this source'
									: isSeries
										? 'Forget this source (removes its series-level tags too)'
										: 'Forget this source (removes its tags and comments too)'}
								onclick={() => toggleForget(s.source)}>×</button
							>
						</span>
					{/each}
				</div>
				{#if forgets.size}
					<p class="srcwarn">
						{#if isSeries}
							Forgetting a source also removes the series-level tags — and any synopsis — it added.
							Tags that a volume carries on its own stay — forget the source on the volume's page to
							remove those too.
						{:else}
							Forgetting a source also removes the tags, comments, and any synopsis it added. A tag
							another source shares may go with it — re-scraping restores it.
						{/if}
					</p>
				{/if}
			</div>
		{/if}

		{#if saveError}<p class="err">{saveError}</p>{/if}
	</div>

	<footer class="mfoot">
		<span class="dirty"
			>{pending === 0
				? 'No changes yet'
				: `${pending} pending change${pending === 1 ? '' : 's'}`}</span
		>
		<span class="spacer"></span>
		<button type="button" onclick={() => onClose?.()}>Cancel</button>
		<button class="save" type="button" disabled={pending === 0 || saving} onclick={save}>
			{saving ? 'Saving…' : 'Save changes'}
		</button>
	</footer>
</Modal>

<style>
	.body {
		padding: var(--space-5);
		display: flex;
		flex-direction: column;
		gap: var(--space-5);
	}
	.f {
		display: block;
		font-size: 0.7rem;
		letter-spacing: 0.1em;
		text-transform: uppercase;
		color: var(--muted);
		margin-bottom: var(--space-2);
	}
	.fieldrow {
		display: flex;
		align-items: baseline;
		justify-content: space-between;
		gap: var(--space-3);
	}
	textarea {
		font: inherit;
		width: 100%;
		min-height: 84px;
		resize: vertical;
		background: var(--surface-2);
		color: var(--text);
		border: 1px solid var(--border);
		border-radius: var(--radius);
		padding: 0.6rem 0.8rem;
	}
	textarea:focus {
		outline: none;
		border-color: var(--accent);
	}
	textarea.doomed {
		border-color: rgba(224, 86, 111, 0.5);
		color: var(--bad, #e0566f);
		text-decoration: line-through;
		opacity: 0.75;
	}
	.prov {
		margin: var(--space-2) 0 0;
		font-size: 0.72rem;
		color: var(--muted);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.prov b {
		font-weight: 600;
	}
	.hintchip {
		font-size: 0.64rem;
		color: var(--accent);
		background: var(--accent-soft);
		border-radius: 999px;
		padding: 2px 9px;
		white-space: nowrap;
	}
	.clearrow {
		display: flex;
		justify-content: flex-end;
		margin-top: var(--space-1);
	}
	.clearlink {
		all: unset;
		cursor: pointer;
		font-size: 0.72rem;
		color: var(--muted);
	}
	.clearlink:hover {
		color: var(--text);
		text-decoration: underline;
	}
	.seg {
		display: inline-flex;
		border: 1px solid var(--border);
		border-radius: var(--radius);
		background: var(--surface-2);
		overflow: hidden;
	}
	.seg button {
		border: none;
		border-radius: 0;
		background: transparent;
		padding: 0.4rem 0.85rem;
		font-size: 0.85rem;
		color: var(--muted);
	}
	.seg button + button {
		border-left: 1px solid var(--border);
	}
	.seg button.on {
		background: var(--accent-soft);
		color: var(--accent);
	}
	.seghint {
		margin: var(--space-2) 0 0;
		font-size: 0.72rem;
		color: var(--muted);
	}
	.tagrows {
		display: flex;
		flex-direction: column;
		gap: var(--space-2);
	}
	.trow {
		display: grid;
		grid-template-columns: 104px 1fr;
		gap: var(--space-3);
		align-items: start;
	}
	.tns {
		font-size: 0.66rem;
		letter-spacing: 0.08em;
		text-transform: uppercase;
		color: var(--muted);
		padding-top: 5px;
		white-space: nowrap;
	}
	.chips {
		display: flex;
		flex-wrap: wrap;
		gap: 6px;
	}
	.chip {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		font-size: 0.8rem;
		background: var(--surface-2);
		border: 1px solid var(--border);
		border-radius: 999px;
		padding: 3px 6px 3px 11px;
	}
	.chip.staged-add {
		border-color: rgba(16, 185, 129, 0.6);
		color: #6ee7b7;
	}
	.chip.staged-del {
		border-color: rgba(224, 86, 111, 0.5);
		color: var(--bad, #e0566f);
		text-decoration: line-through;
		opacity: 0.75;
	}
	.chip button {
		all: unset;
		cursor: pointer;
		color: var(--muted);
		font-size: 0.85rem;
		line-height: 1;
		padding: 0 4px;
	}
	.chip button:hover {
		color: #e0566f;
	}
	.chip.staged-del button:hover {
		color: #6ee7b7;
	}
	.chip button:disabled {
		cursor: default;
		opacity: 0.4;
	}
	.chip button:disabled:hover {
		color: var(--muted);
	}
	.addrow {
		display: flex;
		gap: var(--space-2);
		margin-top: var(--space-3);
	}
	.addrow select {
		font: inherit;
		background-color: var(--surface-2);
		color: var(--text);
		border: 1px solid var(--border);
		border-radius: var(--radius);
		padding: 0.35rem 1.6rem 0.35rem 0.6rem;
		font-size: 0.85rem;
	}
	.addrow input {
		flex: 1;
		font-size: 0.85rem;
		padding: 0.35rem 0.6rem;
	}
	.addrow .add {
		padding: 0.35rem 0.9rem;
		font-size: 0.85rem;
	}
	.srcchip {
		max-width: 100%;
	}
	.srcchip b {
		font-weight: 600;
	}
	.srcurl {
		color: var(--muted);
		font-size: 0.74rem;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		max-width: 26ch;
	}
	.srcwarn {
		margin: var(--space-2) 0 0;
		font-size: 0.72rem;
		color: #e8b04b;
	}
	.err {
		margin: 0;
		color: #e0566f;
		font-size: 0.85rem;
	}
	.mfoot {
		display: flex;
		align-items: center;
		gap: var(--space-3);
		padding: var(--space-4) var(--space-5);
		border-top: 1px solid var(--border);
	}
	.dirty {
		font-size: 0.74rem;
		color: var(--muted);
	}
	.spacer {
		flex: 1;
	}
	.save {
		background: var(--accent);
		border-color: transparent;
		color: #fff;
		font-weight: 600;
	}
	.save:hover:not(:disabled) {
		filter: brightness(1.06);
	}
	.save:disabled {
		opacity: 0.45;
		cursor: default;
	}
</style>
