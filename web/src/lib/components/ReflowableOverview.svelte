<script>
	import { items as itemsApi } from '$lib/api.js';
	import { WORDS_PER_MINUTE, compactWords, readingTimeLabel, relativeTime } from '$lib/format.js';

	let {
		itemId,
		progressValue = null,
		locator = null,
		lastReadAt = null,
		wordCount = 0,
		tags = [],
	} = $props();

	let manifest = $state(null);
	let toc = $state([]);
	let tocList = $state(null);

	let loadedFor = null;
	$effect(() => {
		const id = itemId;
		if (id === loadedFor) return;
		loadedFor = id;
		let live = true;
		manifest = null;
		toc = [];
		itemsApi
			.manifest(id)
			.then((m) => {
				if (!live) return;
				manifest = m;
				toc = m.toc ?? [];
			})
			.catch(() => {});
		return () => (live = false);
	});

	const progression = $derived(progressValue == null ? 0 : Math.min(Math.max(progressValue, 0), 1));
	const percent = $derived(Math.round(progression * 100));
	const started = $derived(progressValue != null || locator != null);
	const spineIndex = $derived(Math.max(0, Number(locator?.chapter) || 0));
	const readingOrder = $derived(manifest?.readingOrder ?? manifest?.reading_order ?? []);
	const currentHref = $derived(readingOrder[spineIndex]?.href ?? '');
	const currentTocIndex = $derived.by(() => {
		const savedToc = locator?.toc;
		if (savedToc?.href && savedToc?.title) {
			const exact = toc.findIndex(
				(entry) => entry.href === savedToc.href && entry.title === savedToc.title,
			);
			if (exact >= 0) return exact;
		}
		const base = currentHref.split('#')[0];
		const matches = toc
			.map((entry, index) => ({ entry, index }))
			.filter(({ entry }) => entry.href.split('#')[0] === base);
		return matches.length === 1 ? matches[0].index : -1;
	});
	function tocFraction(index) {
		if (!started || currentTocIndex < 0) return 0;
		if (index < currentTocIndex) return 1;
		if (index > currentTocIndex) return 0;
		return Math.min(Math.max(Number(locator?.toc?.progress) || 0, 0), 1);
	}
	$effect(() => {
		const list = tocList;
		const index = currentTocIndex;
		if (!list || index < 0) return;
		const frame = requestAnimationFrame(() => {
			const row = list.querySelector(`[data-toc-index="${index}"]`);
			if (!row) return;
			const rowTop =
				row.getBoundingClientRect().top - list.getBoundingClientRect().top + list.scrollTop;
			list.scrollTop = Math.max(0, rowTop - row.offsetHeight * 3);
		});
		return () => cancelAnimationFrame(frame);
	});
	const currentTitle = $derived.by(() => {
		if (!started) return 'Not started';
		const savedToc = locator?.toc;
		if (
			savedToc?.title &&
			savedToc?.href &&
			toc.some((entry) => entry.href === savedToc.href && entry.title === savedToc.title)
		) {
			return savedToc.title;
		}
		const base = currentHref.split('#')[0];
		const entries = toc.filter((t) => t.href.split('#')[0] === base);
		return entries.length === 1 ? entries[0].title : `Section ${spineIndex + 1}`;
	});
	const authors = $derived(
		tags
			.filter((t) => t.namespace === 'creator')
			.map((t) => t.value)
			.join(', '),
	);
	const language = $derived(tags.find((t) => t.namespace === 'language')?.value ?? '');
	const estimate = $derived(wordCount ? readingTimeLabel(wordCount / WORDS_PER_MINUTE) : '');
</script>

<section class="overview" aria-labelledby="reading-overview-title">
	<h2 id="reading-overview-title" class="heading">
		<svg
			viewBox="0 0 24 24"
			fill="none"
			stroke="currentColor"
			stroke-width="1.8"
			stroke-linecap="round"
			stroke-linejoin="round"
			><path d="M4 19.5A2.5 2.5 0 0 1 6.5 17H20" /><path
				d="M6.5 2H20v20H6.5A2.5 2.5 0 0 1 4 19.5v-15A2.5 2.5 0 0 1 6.5 2Z"
			/></svg
		>
		Reading overview
	</h2>

	<div class="cards">
		<article class="card position">
			<div class="label">
				<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"
					><path d="M4 5.5A2.5 2.5 0 0 1 6.5 3H11v16H6.5A2.5 2.5 0 0 0 4 21.5Z" /><path
						d="M20 5.5A2.5 2.5 0 0 0 17.5 3H13v16h4.5a2.5 2.5 0 0 1 2.5 2.5Z"
					/></svg
				>
				Current position
			</div>
			<strong>{currentTitle}</strong>
			<p>
				{started
					? `Section ${spineIndex + 1}${readingOrder.length ? ` of ${readingOrder.length}` : ''} · ${percent}%`
					: 'Open the book to begin reading'}
			</p>
			<div class="track" aria-label={`${percent}% read`}>
				<span style={`width:${percent}%`}></span>
			</div>
			{#if lastReadAt}<p class="last">Last read · {relativeTime(lastReadAt)}</p>{/if}
		</article>

		<article class="card contents">
			<div class="label">
				<svg
					viewBox="0 0 24 24"
					fill="none"
					stroke="currentColor"
					stroke-width="1.8"
					stroke-linecap="round"
					><path d="M9 6h12M9 12h12M9 18h12" /><path d="M4 6h.01M4 12h.01M4 18h.01" /></svg
				>
				Table of contents
			</div>
			{#if toc.length}
				<ol bind:this={tocList}>
					{#each toc as entry, i (i)}
						{@const entryFraction = tocFraction(i)}
						{@const entryPercent = Math.round(entryFraction * 100)}
						<li class:current={i === currentTocIndex} data-toc-index={i}>
							<span class="number">{String(i + 1).padStart(2, '0')}</span>
							<span class="toc-title" style={`padding-left:${(entry.level ?? 0) * 0.7}rem`}
								>{entry.title}</span
							>
							<span class="mini-track"><span style={`width:${entryPercent}%`}></span></span>
							<span class="toc-percent">{entryPercent}%</span>
						</li>
					{/each}
				</ol>
			{:else if manifest}
				<p class="empty">This book doesn’t include a table of contents.</p>
			{:else}
				<p class="empty">Loading contents…</p>
			{/if}
		</article>

		<article class="card details">
			<div class="label">
				<svg
					viewBox="0 0 24 24"
					fill="none"
					stroke="currentColor"
					stroke-width="1.8"
					stroke-linecap="round"
					stroke-linejoin="round"
					><circle cx="12" cy="12" r="9" /><path d="M12 11v6M12 7h.01" /></svg
				>
				Book details
			</div>
			<dl>
				{#if wordCount}<div>
						<dt>Length</dt>
						<dd>{compactWords(wordCount)} words</dd>
					</div>{/if}
				{#if estimate}<div>
						<dt>Reading time</dt>
						<dd>{estimate}</dd>
					</div>{/if}
				{#if authors}<div>
						<dt>Author</dt>
						<dd class="author-name">{authors}</dd>
					</div>{/if}
				{#if language}<div>
						<dt>Language</dt>
						<dd class="capitalize">{language}</dd>
					</div>{/if}
			</dl>
		</article>
	</div>
</section>

<style>
	.overview {
		margin-top: var(--space-6);
		padding-top: var(--space-5);
		border-top: 1px solid var(--border);
		margin-left: calc(-1 * var(--space-6));
		padding-left: var(--space-6);
		margin-right: calc(-1 * var(--space-5));
		padding-right: var(--space-5);
	}
	.heading {
		display: flex;
		align-items: center;
		gap: 0.6rem;
		margin: 0 0 var(--space-3);
		font-size: 1rem;
		font-family: inherit;
		font-weight: 650;
	}
	.heading svg,
	.label svg {
		width: 1.15rem;
		height: 1.15rem;
		flex: none;
	}
	.cards {
		display: grid;
		grid-template-columns: repeat(3, minmax(0, 1fr));
		grid-auto-rows: 250px;
		gap: var(--space-3);
	}
	.card {
		min-width: 0;
		min-height: 0;
		overflow: hidden;
		display: flex;
		flex-direction: column;
		padding: var(--space-4);
		border: 1px solid var(--border);
		border-radius: var(--radius);
		background: color-mix(in srgb, var(--surface) 82%, transparent);
		backdrop-filter: blur(10px);
	}
	.label {
		display: flex;
		align-items: center;
		gap: 0.55rem;
		margin-bottom: var(--space-4);
		color: var(--muted);
		font-size: 0.78rem;
		font-weight: 650;
		text-transform: uppercase;
		letter-spacing: 0.06em;
	}
	.position strong {
		font-size: 1.12rem;
		line-height: 1.3;
	}
	p {
		color: var(--muted);
		font-size: 0.82rem;
		line-height: 1.45;
	}
	.track {
		width: 100%;
		height: 7px;
		min-height: 7px;
		flex: 0 0 7px;
		margin: 0.15rem 0 0.2rem;
		overflow: hidden;
		border-radius: 999px;
		background: var(--surface-2);
	}
	.track span {
		display: block;
		height: 100%;
		border-radius: inherit;
		background: var(--accent);
	}
	.last {
		margin: 0.55rem 0 0;
	}
	.contents {
		overflow: hidden;
	}
	ol {
		flex: 1;
		min-height: 0;
		list-style: none;
		padding: 0 0.3rem 0 0;
		margin: 0;
		display: grid;
		align-content: start;
		gap: 0.72rem;
		overflow-y: auto;
		scrollbar-width: thin;
		scrollbar-color: var(--border) transparent;
	}
	li {
		display: grid;
		grid-template-columns: 1.8rem minmax(0, 1fr) 3.6rem 2.2rem;
		gap: 0.45rem;
		align-items: center;
		color: var(--muted);
		font-size: 0.78rem;
	}
	li.current {
		color: var(--accent);
	}
	.number {
		font-variant-numeric: tabular-nums;
	}
	.toc-title {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		color: var(--text);
	}
	.mini-track {
		height: 6px;
		overflow: hidden;
		border-radius: 999px;
		background: var(--surface-2);
	}
	.mini-track span {
		display: block;
		height: 100%;
		border-radius: inherit;
		background: var(--accent);
	}
	.toc-percent {
		text-align: right;
		font-variant-numeric: tabular-nums;
	}
	.empty {
		margin: auto 0;
		text-align: center;
	}
	dl {
		margin: 0;
		display: grid;
		gap: 0.75rem;
	}
	dl div {
		display: grid;
		gap: 0.15rem;
		padding-bottom: 0.7rem;
		border-bottom: 1px solid color-mix(in srgb, var(--border) 70%, transparent);
	}
	dl div:last-child {
		border: 0;
	}
	dt {
		color: var(--muted);
		font-size: 0.72rem;
		text-transform: uppercase;
		letter-spacing: 0.05em;
	}
	dd {
		margin: 0;
		font-size: 0.9rem;
		line-height: 1.35;
		overflow-wrap: anywhere;
	}
	.author-name {
		text-transform: capitalize;
	}
	.capitalize {
		text-transform: capitalize;
	}
	@media (max-width: 1000px) {
		.cards {
			grid-template-columns: 1fr 1fr;
		}
		.details {
			grid-column: 1/-1;
		}
		.details dl {
			grid-template-columns: repeat(2, minmax(0, 1fr));
		}
	}
	@media (max-width: 720px) {
		.overview {
			margin-left: 0;
			padding-left: 0;
			margin-right: 0;
			padding-right: 0;
		}
	}
	@media (max-width: 680px) {
		.cards {
			grid-template-columns: 1fr;
			grid-auto-rows: auto;
		}
		.card {
			overflow: visible;
		}
		.contents {
			height: 250px;
			overflow: hidden;
		}
		.details {
			grid-column: auto;
		}
		.details dl {
			grid-template-columns: 1fr;
		}
	}
</style>
