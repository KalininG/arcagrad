<script>
	import { onMount } from 'svelte';
	import { page } from '$app/stores';
	import { currentUser } from '$lib/session.js';
	import { loadFollows, followNewCount } from '$lib/follows/store.js';
	import PageHeader from '$lib/components/ui/PageHeader.svelte';
	import UpcomingPage from '$lib/upcoming/UpcomingPage.svelte';
	import FollowingPage from '$lib/follows/FollowingPage.svelte';
	import TrackedSeriesPage from '$lib/tracking/TrackedSeriesPage.svelte';

	const isAdmin = $derived($currentUser?.role === 'admin');
	const requestedTab = $derived($page.url.searchParams.get('tab') ?? 'upcoming');
	const activeTab = $derived(
		isAdmin && (requestedTab === 'uploads' || requestedTab === 'series')
			? requestedTab
			: 'upcoming',
	);

	onMount(() => {
		if (isAdmin) loadFollows().catch(() => {});
	});
</script>

<svelte:head><title>Tracking · Arcagrad</title></svelte:head>

<div class="app-page tracking-page">
	<PageHeader title="Tracking" />
	<p class="intro">Keep up with release dates and new uploads from the sources you follow.</p>

	<nav class="tabs" aria-label="Tracking views">
		<a class:active={activeTab === 'upcoming'} href="/tracking">
			<svg viewBox="0 0 24 24" aria-hidden="true"
				><path d="M7 3v3m10-3v3M4 9h16M5 5h14a1 1 0 0 1 1 1v14H4V6a1 1 0 0 1 1-1Z" /></svg
			>
			<span><strong>Upcoming releases</strong><small>Dates from linked series</small></span>
		</a>
		{#if isAdmin}
			<a class:active={activeTab === 'series'} href="/tracking?tab=series">
				<svg viewBox="0 0 24 24" aria-hidden="true"
					><path d="M4 5h6v14H4zM14 5h6v14h-6z" /><path d="M7 8h.01M17 8h.01" /></svg
				>
				<span><strong>Tracked series</strong><small>Manage calendar links</small></span>
			</a>
			<a class:active={activeTab === 'uploads'} href="/tracking?tab=uploads">
				<svg viewBox="0 0 24 24" aria-hidden="true"
					><path d="M12 3a9 9 0 1 1 0 18 9 9 0 0 1 0-18zM12 7v5l3 3" /></svg
				>
				<span><strong>New uploads</strong><small>Review followed searches</small></span>
				{#if $followNewCount > 0}<b class="badge">{$followNewCount}</b>{/if}
			</a>
		{/if}
	</nav>

	<section class="tab-content">
		{#if activeTab === 'uploads'}
			<FollowingPage embedded />
		{:else if activeTab === 'series'}
			<TrackedSeriesPage />
		{:else}
			<UpcomingPage embedded />
		{/if}
	</section>
</div>

<style>
	.tracking-page {
		min-width: 0;
	}
	.intro {
		margin: calc(-1 * var(--space-4)) 0 var(--space-5);
		color: var(--muted);
		font-size: 0.88rem;
	}
	.tabs {
		display: flex;
		gap: var(--space-2);
		margin-bottom: var(--space-5);
		border-bottom: 1px solid var(--border);
	}
	.tabs a {
		position: relative;
		display: flex;
		align-items: center;
		gap: var(--space-3);
		min-width: 13rem;
		padding: var(--space-3) var(--space-4);
		color: var(--muted);
	}
	.tabs a::after {
		content: '';
		position: absolute;
		right: 0;
		bottom: -1px;
		left: 0;
		height: 2px;
		background: transparent;
	}
	.tabs a:hover {
		color: var(--text);
		background: color-mix(in srgb, var(--surface) 55%, transparent);
	}
	.tabs a.active {
		color: var(--text);
	}
	.tabs a.active::after {
		background: var(--accent);
	}
	.tabs svg {
		flex: 0 0 auto;
		width: 1.2rem;
		height: 1.2rem;
		fill: none;
		stroke: currentColor;
		stroke-width: 1.7;
		stroke-linecap: round;
		stroke-linejoin: round;
	}
	.tabs span {
		display: flex;
		flex-direction: column;
		gap: 0.12rem;
	}
	.tabs strong {
		font-size: 0.86rem;
		font-weight: 600;
	}
	.tabs small {
		color: var(--muted);
		font-size: 0.68rem;
	}
	.badge {
		min-width: 1.25rem;
		height: 1.25rem;
		margin-left: auto;
		padding: 0 0.35rem;
		display: grid;
		place-items: center;
		border-radius: 999px;
		background: var(--accent);
		color: #fff;
		font-size: 0.68rem;
	}
	.tab-content {
		min-width: 0;
	}
	@media (max-width: 640px) {
		.tabs {
			overflow-x: auto;
		}
		.tabs a {
			min-width: max-content;
			padding-inline: var(--space-3);
		}
		.tabs small {
			display: none;
		}
	}
</style>
