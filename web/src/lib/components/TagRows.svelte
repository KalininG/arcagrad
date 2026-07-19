<script>
	import { MediaQuery } from 'svelte/reactivity';
	import { tagCounts } from '$lib/tagstats.js';
	import { groupTags, tagHref } from '$lib/tags.js';

	let { tags = [], kind = '', before = [], after = [], hiddenNamespaces = [] } = $props();
	const narrow = new MediaQuery('(max-width: 720px)');
	const tagLimit = $derived(narrow.current ? 15 : 25);
	let tagsExpanded = $state(false);

	const tagCount = (t) => t.count ?? $tagCounts?.get(`${t.namespace}:${t.value}`) ?? null;
	const groups = $derived(
		groupTags(tags)
			.filter((group) => !hiddenNamespaces.includes(group.ns))
			.map((group) => ({
				...group,
				tags: [...group.tags].sort((a, b) => {
					const countOrder = (tagCount(b) ?? -1) - (tagCount(a) ?? -1);
					if (countOrder) return countOrder;
					return (
						a.value.localeCompare(b.value) || (a.qualifier ?? '').localeCompare(b.qualifier ?? '')
					);
				}),
			})),
	);
	const hasContent = $derived(before.length > 0 || groups.length > 0 || after.length > 0);
	$effect(() => {
		tags;
		kind;
		tagsExpanded = false;
	});
</script>

{#snippet extraRow(row)}
	<div class="tagrow">
		<span class="taglabel">{row.label}:</span>
		<div class="chips">
			{#each row.chips as c (c.text)}
				{#if c.href}
					<a
						class="chip clickable"
						class:wrap={c.wrap}
						href={c.href}
						target={c.external ? '_blank' : undefined}
						rel={c.external ? 'noopener noreferrer' : undefined}>{c.text}</a
					>
				{:else}
					<span class="chip" class:wrap={c.wrap}>{c.text}</span>
				{/if}
			{/each}
		</div>
	</div>
{/snippet}

{#if hasContent}
	<div class="tags">
		{#each before as row (row.label)}{@render extraRow(row)}{/each}
		{#each groups as g (g.ns)}
			{@const collapsible = g.ns === 'tag' && g.tags.length > tagLimit}
			{@const visibleTags = collapsible && !tagsExpanded ? g.tags.slice(0, tagLimit) : g.tags}
			<div class="tagrow">
				<span class="taglabel">{g.label}:</span>
				<div class="chips">
					{#each visibleTags as tg (tg.qualifier + ':' + tg.value)}
						<a class="chip clickable" href={tagHref(kind, tg)}>
							{tg.value}{#if tagCount(tg) != null}<span class="cnt">{tagCount(tg)}</span>{/if}
						</a>
					{/each}
					{#if collapsible}
						<button
							class="more"
							type="button"
							aria-expanded={tagsExpanded}
							onclick={() => (tagsExpanded = !tagsExpanded)}
						>
							{#if tagsExpanded}Show less{:else}<span aria-hidden="true">+</span>
								{g.tags.length - tagLimit} more{/if}
						</button>
					{/if}
				</div>
			</div>
		{/each}
		{#each after as row (row.label)}{@render extraRow(row)}{/each}
	</div>
{/if}

<style>
	.tags {
		display: flex;
		flex-direction: column;
		gap: var(--space-2);
		margin-top: var(--space-4);
		margin-bottom: var(--space-5);
	}
	@media (max-width: 720px) {
		.tags {
			margin-top: 0;
		}
	}
	.tagrow {
		display: grid;
		grid-template-columns: 6.5rem 1fr;
		gap: var(--space-2);
		align-items: start;
	}
	@media (max-width: 520px) {
		.tagrow {
			grid-template-columns: 5.5rem 1fr;
		}
	}
	.taglabel {
		font-size: 0.66rem;
		text-transform: uppercase;
		letter-spacing: 0.08em;
		color: var(--muted);
		padding-top: 0.18rem;
		white-space: nowrap;
	}
	.chips {
		display: flex;
		flex-wrap: wrap;
		gap: 0.3rem;
	}
	.chip {
		padding: 0.13rem 0.44rem;
		border: 1px solid var(--border);
		border-radius: var(--radius-sm);
		background: var(--surface);
		font-size: 0.69rem;
		color: var(--text);
		white-space: nowrap;
	}
	.cnt {
		margin-left: 0.35rem;
		color: var(--muted);
		font-size: 0.68rem;
		font-variant-numeric: tabular-nums;
	}
	.chip.clickable:hover {
		border-color: var(--accent);
		color: var(--accent);
	}
	.more {
		display: inline-flex;
		align-items: center;
		gap: 0.25rem;
		padding: 0.13rem 0.44rem;
		border: 1px solid var(--border);
		border-radius: var(--radius-sm);
		background: var(--surface);
		color: var(--muted);
		font: inherit;
		font-size: 0.69rem;
		line-height: normal;
		cursor: pointer;
	}
	.more:hover {
		border-color: var(--accent);
		color: var(--accent);
	}
	.chip.wrap {
		white-space: normal;
		overflow-wrap: anywhere;
	}
</style>
