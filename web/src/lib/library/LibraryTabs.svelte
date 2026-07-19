<script>
	import { kindHref } from '$lib/kinds.js';

	let { kind = null, active } = $props();

	const base = $derived(kind ? kindHref(kind) : '');
	const tabs = $derived([
		{ id: 'library', label: 'Library', href: base || '/' },
		{ id: 'foryou', label: 'For You', href: `${base}/for-you` },
		...(kind ? [{ id: 'tags', label: 'Tags', href: `${base}/tags` }] : []),
	]);
</script>

<nav class="tabs" aria-label="Library views">
	{#each tabs as t (t.id)}
		<a class="tab" class:active={active === t.id} href={t.href}>{t.label}</a>
	{/each}
</nav>

<style>
	.tabs {
		display: flex;
		gap: var(--space-5);
		border-bottom: 1px solid var(--border);
		margin: calc(-1 * var(--space-4)) 0 var(--space-5);
	}
	.tab {
		padding: 0.4rem 0 0.6rem;
		margin-bottom: -1px;
		font-size: 0.92rem;
		font-weight: 550;
		color: var(--muted);
		border-bottom: 2px solid transparent;
		transition:
			color var(--ease),
			border-color var(--ease);
	}
	.tab:hover {
		color: var(--text);
	}
	.tab.active {
		color: var(--text);
		border-color: var(--accent);
	}
</style>
