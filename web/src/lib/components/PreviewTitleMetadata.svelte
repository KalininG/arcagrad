<script>
	import { tagHref } from '$lib/tags.js';

	let {
		tags = [],
		kind = '',
		primaryCount = '',
		readingTime = '',
		format = '',
		publisher = '',
		compact = false,
	} = $props();

	const authors = $derived(tags.filter((tag) => tag.namespace === 'creator'));
	const languages = $derived(tags.filter((tag) => tag.namespace === 'language'));
	const hasStats = $derived(primaryCount || readingTime || languages.length || format || publisher);
</script>

<div class="metadata" class:compact>
	{#if authors.length}
		<div class="authors">
			{#each authors as author, i (author.qualifier + ':' + author.value)}
				{#if i}<span class="author-separator">,</span>{/if}<a href={tagHref(kind, author)}
					>{author.value}</a
				>
			{/each}
		</div>
	{/if}
	{#if hasStats}
		<div class="stats">
			{#if primaryCount}
				<span>
					<svg
						viewBox="0 0 24 24"
						fill="none"
						stroke="currentColor"
						stroke-width="1.8"
						stroke-linecap="round"
						><circle cx="12" cy="12" r="9" /><path d="M12 11v6M12 7h.01" /></svg
					>
					{primaryCount}
				</span>
			{/if}
			{#if readingTime}
				{#if primaryCount}<span class="dot">·</span>{/if}
				<span>
					<svg
						viewBox="0 0 24 24"
						fill="none"
						stroke="currentColor"
						stroke-width="1.8"
						stroke-linecap="round"
						stroke-linejoin="round"><circle cx="12" cy="12" r="9" /><path d="M12 7v5l3 2" /></svg
					>
					{readingTime}
				</span>
			{/if}
			{#if languages.length}
				{#if primaryCount || readingTime}<span class="dot">·</span>{/if}
				<span class="languages">
					<svg
						viewBox="0 0 24 24"
						fill="none"
						stroke="currentColor"
						stroke-width="1.8"
						stroke-linecap="round"
						stroke-linejoin="round"
						><circle cx="12" cy="12" r="9" /><path
							d="M3 12h18M12 3a15 15 0 0 1 0 18M12 3a15 15 0 0 0 0 18"
						/></svg
					>
					{#each languages as language, i (language.qualifier + ':' + language.value)}
						{#if i}<span class="language-separator">,</span>{/if}<a href={tagHref(kind, language)}
							>{language.value}</a
						>
					{/each}
				</span>
			{/if}
			{#if format}
				{#if primaryCount || readingTime || languages.length}<span class="dot">·</span>{/if}
				<span>
					<svg
						viewBox="0 0 24 24"
						fill="none"
						stroke="currentColor"
						stroke-width="1.8"
						stroke-linecap="round"
						stroke-linejoin="round"><path d="M6 3h8l4 4v14H6z" /><path d="M14 3v5h5" /></svg
					>
					{format}
				</span>
			{/if}
			{#if publisher}
				<span class="publisher-separator" aria-hidden="true"></span>
				<span class="publisher">{publisher}</span>
			{/if}
		</div>
	{/if}
</div>

<style>
	.metadata {
		margin-top: var(--space-2);
	}
	.authors {
		font-size: 1rem;
		font-weight: 650;
		line-height: 1.35;
		color: var(--accent);
	}
	.authors a {
		color: inherit;
		text-transform: capitalize;
	}
	.author-separator {
		color: var(--text);
		margin-right: 0.3em;
	}
	.authors a:hover,
	.languages a:hover {
		text-decoration: underline;
		text-underline-offset: 0.15em;
	}
	.stats {
		display: flex;
		align-items: center;
		flex-wrap: wrap;
		gap: 0.45rem;
		margin-top: var(--space-3);
		color: var(--muted);
		font-size: 0.8rem;
		font-variant-numeric: tabular-nums;
	}
	.stats > span:not(.dot) {
		display: inline-flex;
		align-items: center;
		gap: 0.35rem;
	}
	.stats svg {
		width: 1rem;
		height: 1rem;
		flex: none;
	}
	.stats > span.languages {
		gap: 0;
		text-transform: capitalize;
	}
	.languages svg {
		margin-right: 0.35rem;
	}
	.language-separator {
		margin-right: 0.3em;
	}
	.languages a {
		color: inherit;
	}
	.stats .publisher-separator {
		width: 1px;
		height: 1rem;
		margin: 0 0.1rem;
		background: var(--border);
	}
	.stats .publisher {
		padding: 0.22rem 0.5rem;
		border: 1px solid var(--border);
		border-radius: 0.4rem;
		background: color-mix(in srgb, var(--surface) 78%, transparent);
		color: var(--muted);
		line-height: 1;
	}
	.compact {
		margin-top: 0;
	}
	.compact .authors {
		font-size: 0.82rem;
	}
	.compact .stats {
		margin-top: var(--space-1);
		gap: 0.3rem;
		font-size: 0.72rem;
	}
	.compact .stats svg {
		width: 0.85rem;
		height: 0.85rem;
	}
</style>
