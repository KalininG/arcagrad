<script>
	import { sanitizeHtml } from '$lib/sanitize.js';

	let { comments = [], itemId = '', pageCount = 0 } = $props();

	const PAGE_RE = /\b(?:pages?\.?|pgs?\.?|p\.)\s*(\d{1,4})\b/gi;

	function linkifyText(text, dest) {
		if (!itemId) {
			dest.appendChild(document.createTextNode(text));
			return;
		}
		let last = 0;
		let m;
		PAGE_RE.lastIndex = 0;
		while ((m = PAGE_RE.exec(text)) !== null) {
			const n = parseInt(m[1], 10);
			if (n >= 1 && (!pageCount || n <= pageCount)) {
				if (m.index > last) dest.appendChild(document.createTextNode(text.slice(last, m.index)));
				const a = document.createElement('a');
				a.setAttribute('href', `/reader/${itemId}?page=${n}`);
				a.className = 'pageref';
				a.textContent = m[0];
				dest.appendChild(a);
				last = m.index + m[0].length;
			}
		}
		if (last < text.length) dest.appendChild(document.createTextNode(text.slice(last)));
	}

	const esc = (v) =>
		String(v)
			.replace(/&/g, '&amp;')
			.replace(/"/g, '&quot;')
			.replace(/</g, '&lt;')
			.replace(/>/g, '&gt;');
	function markdownToHtml(s) {
		return String(s)
			.replace(
				/!\[([^\]]*)\]\(([^)\s]+)[^)]*\)/g,
				(_, alt, url) => `<img src="${esc(url)}" alt="${esc(alt)}">`,
			)
			.replace(
				/\[([^\]]+)\]\(([^)\s]+)[^)]*\)/g,
				(_, text, url) => `<a href="${esc(url)}">${esc(text)}</a>`,
			);
	}
	const render = (body) => sanitizeHtml(markdownToHtml(body), { onText: linkifyText });

	let newestFirst = $state(true);
	const sorted = $derived(
		[...comments].sort((a, b) => {
			const d = (b.posted_at ?? 0) - (a.posted_at ?? 0);
			return newestFirst ? d : -d;
		}),
	);

	function fmtDate(ts) {
		if (!ts) return '';
		const d = new Date(ts * 1000);
		return (
			'on ' +
			d.toLocaleDateString(undefined, { day: 'numeric', month: 'long', year: 'numeric' }) +
			', ' +
			d.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit', hour12: false })
		);
	}
	const fmtScore = (s) => (s > 0 ? `+${s}` : `${s}`);
</script>

{#if comments.length}
	<section class="comments">
		<div class="chead">
			<div class="cleft">
				<span class="ctitle">Comments</span>
				<button
					class="sortbtn"
					onclick={() => (newestFirst = !newestFirst)}
					title={newestFirst ? 'Newest first' : 'Oldest first'}
					aria-label="Toggle comment order"
					type="button"
				>
					<svg
						class:flip={!newestFirst}
						viewBox="0 0 24 24"
						fill="none"
						stroke="currentColor"
						stroke-width="2"
						stroke-linecap="round"
						stroke-linejoin="round"
					>
						<path d="M12 5v14" />
						<path d="m19 12-7 7-7-7" />
					</svg>
				</button>
			</div>
			<span class="count">{comments.length}</span>
		</div>

		<div class="list">
			{#each sorted as c (c.source + ':' + c.external_id)}
				<article class="comment">
					<div class="ctop">
						<span class="author">{c.author || 'Anonymous'}</span>
						{#if c.score != null}
							<span
								class="score"
								class:pos={c.score > 0}
								class:neg={c.score < 0}
								class:low={Math.abs(c.score) < 50}
							>
								Score {fmtScore(c.score)}
							</span>
						{/if}
					</div>
					{#if c.posted_at}<p class="date">{fmtDate(c.posted_at)}</p>{/if}
					<!-- eslint-disable-next-line svelte/no-at-html-tags -- sanitized above -->
					<div class="body">{@html render(c.body)}</div>
				</article>
			{/each}
		</div>
	</section>
{/if}

<style>
	.chead {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--space-4);
		margin-bottom: var(--space-4);
	}
	.cleft {
		display: flex;
		align-items: center;
		gap: var(--space-2);
		min-width: 0;
	}
	.ctitle {
		font-size: 0.7rem;
		text-transform: uppercase;
		letter-spacing: 0.15em;
		color: var(--muted);
	}
	.sortbtn {
		all: unset;
		cursor: pointer;
		display: inline-flex;
		align-items: center;
		justify-content: center;
		width: 1.9rem;
		height: 1.9rem;
		border: 1px solid var(--border);
		border-radius: var(--radius-sm);
		color: var(--muted);
		transition:
			border-color var(--ease),
			color var(--ease);
	}
	.sortbtn:hover {
		border-color: var(--accent);
		color: var(--text);
	}
	.sortbtn svg {
		width: 1rem;
		height: 1rem;
		transition: transform var(--ease);
	}
	.sortbtn svg.flip {
		transform: rotate(180deg);
	}
	.count {
		flex: 0 0 auto;
		padding: 0.15rem 0.5rem;
		border: 1px solid var(--border);
		border-radius: 9999px;
		font-variant-numeric: tabular-nums;
		font-size: 0.72rem;
		color: var(--muted);
	}

	.list {
		display: flex;
		flex-direction: column;
		gap: var(--space-3);
	}
	.comment {
		border: 1px solid var(--border);
		border-radius: var(--radius);
		background: var(--surface);
		padding: var(--space-4);
	}
	.ctop {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--space-4);
		margin-bottom: var(--space-2);
	}
	.author {
		font-size: 0.88rem;
		font-weight: 600;
		color: var(--text);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.score {
		flex: 0 0 auto;
		padding: 0.1rem 0.45rem;
		border: 1px solid var(--border);
		border-radius: var(--radius-sm);
		font-variant-numeric: tabular-nums;
		font-size: 0.72rem;
		color: var(--muted);
	}
	.score.pos {
		border-color: color-mix(in srgb, var(--good) 50%, transparent);
		background: color-mix(in srgb, var(--good) 14%, transparent);
		color: var(--good);
	}
	.score.neg {
		border-color: rgba(224, 86, 111, 0.5);
		background: rgba(224, 86, 111, 0.14);
		color: #e0566f;
	}
	.score.low {
		background: transparent;
	}
	.date {
		margin: 0 0 var(--space-2);
		font-size: 0.72rem;
		color: var(--muted);
	}
	.body {
		font-size: 0.8rem;
		line-height: 1.6;
		color: var(--text);
		overflow-wrap: anywhere;
	}
	.body :global(a) {
		color: var(--accent);
		text-decoration: underline;
		text-underline-offset: 2px;
	}
	.body :global(a:hover) {
		color: var(--text);
	}
	.body :global(a.pageref) {
		text-decoration-style: dotted;
		text-underline-offset: 2px;
		cursor: pointer;
	}
	.body :global(img.cimg) {
		display: block;
		max-width: min(100%, 320px);
		max-height: 240px;
		width: auto;
		height: auto;
		margin: var(--space-2) 0 0;
		border-radius: var(--radius-sm);
		object-fit: contain;
	}
	.body :global(blockquote) {
		margin: var(--space-2) 0;
		padding-left: var(--space-3);
		border-left: 2px solid var(--border);
		color: var(--muted);
	}
</style>
