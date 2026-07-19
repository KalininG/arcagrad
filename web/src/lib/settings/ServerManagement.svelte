<script>
	import { goto } from '$app/navigation';
	import PageHeader from '$lib/components/ui/PageHeader.svelte';
	import ServerHealth from '$lib/settings/ServerHealth.svelte';
	import AccountsTable from '$lib/settings/AccountsTable.svelte';
	import Users from '$lib/settings/Users.svelte';
	import { currentUser } from '$lib/session.js';

	const isAdmin = $derived($currentUser?.role === 'admin');

	const TABS = [
		{ id: 'users', label: 'Users' },
		{ id: 'health', label: 'Server health' },
	];
	function initTab() {
		try {
			const t = new URLSearchParams(location.search).get('tab');
			return TABS.some((x) => x.id === t) ? t : 'users';
		} catch {
			return 'users';
		}
	}
	let active = $state(initTab());
	function setTab(id) {
		if (id === active) return;
		active = id;
		const url = new URL(location.href);
		url.searchParams.set('tab', id);
		goto(url, { replaceState: true, keepFocus: true, noScroll: true });
	}
</script>

<div class="app-page server">
	<PageHeader title="Server" />

	{#if !isAdmin}
		<p class="denied">This area is available to administrators only.</p>
	{:else}
		<nav class="tabs" aria-label="Server management sections">
			{#each TABS as t (t.id)}
				<button
					class="tab"
					class:active={active === t.id}
					onclick={() => setTab(t.id)}
					type="button"
				>
					{t.label}
				</button>
			{/each}
		</nav>

		<div class="panel">
			{#if active === 'users'}
				<AccountsTable />
				<div class="divider"><Users /></div>
			{:else if active === 'health'}
				<ServerHealth />
			{/if}
		</div>
	{/if}
</div>

<style>
	.server {
		display: flex;
		flex-direction: column;
		gap: var(--space-5);
	}
	.server > :global(.pagehead) {
		margin-bottom: 0;
	}
	.denied {
		color: var(--muted);
		font-size: 0.9rem;
	}
	.tabs {
		display: flex;
		gap: var(--space-4);
		border-bottom: 1px solid var(--border);
	}
	.tab {
		all: unset;
		cursor: pointer;
		padding: var(--space-2) 0;
		font-size: 0.9rem;
		color: var(--muted);
		border-bottom: 2px solid transparent;
		margin-bottom: -1px;
	}
	.tab:hover {
		color: var(--text);
	}
	.tab.active {
		color: var(--text);
		border-bottom-color: var(--accent);
	}
	.panel {
		display: flex;
		flex-direction: column;
		gap: var(--space-6);
	}
	.divider {
		border-top: 1px solid var(--border);
		padding-top: var(--space-6);
	}
</style>
