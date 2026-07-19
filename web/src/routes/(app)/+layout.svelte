<script>
	import { onMount } from 'svelte';
	import { goto, afterNavigate } from '$app/navigation';
	import { auth } from '$lib/api.js';
	import { setUser, GUEST } from '$lib/session.js';
	import { refreshKindsSoon } from '$lib/kinds.js';
	import Sidebar from '$lib/layout/Sidebar.svelte';

	let { children } = $props();
	let me = $state(null);
	let booted = $state(false);
	let navOpen = $state(false);

	onMount(async () => {
		try {
			const s = await auth.status();
			if (s.authenticated) {
				me = s.user;
				setUser(me);
			} else if (s.guest_enabled) {
				me = GUEST;
				setUser(GUEST);
			} else {
				return goto('/login');
			}
		} catch {
			/* ignored */
		}
		booted = true;
	});

	onMount(() => {
		const onVisible = () => document.visibilityState === 'visible' && refreshKindsSoon();
		document.addEventListener('visibilitychange', onVisible);
		window.addEventListener('focus', onVisible);
		return () => {
			document.removeEventListener('visibilitychange', onVisible);
			window.removeEventListener('focus', onVisible);
		};
	});

	afterNavigate(() => {
		navOpen = false;
		refreshKindsSoon();
	});

	onMount(() => {
		function onKey(e) {
			if (e.key !== 'Escape') return;
			if (!window.matchMedia('(max-width: 900px)').matches) return;
			if (navOpen) {
				navOpen = false;
				return;
			}
			const el = document.activeElement;
			if (el && (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA' || el.isContentEditable))
				return;
			navOpen = true;
		}
		window.addEventListener('keydown', onKey);
		return () => window.removeEventListener('keydown', onKey);
	});
</script>

{#if booted}
	<div class="shell">
		<header class="topbar">
			<button
				class="burger"
				onclick={() => (navOpen = true)}
				aria-label="Open menu"
				aria-expanded={navOpen}
				type="button"
			>
				<svg
					viewBox="0 0 24 24"
					fill="none"
					stroke="currentColor"
					stroke-width="2"
					stroke-linecap="round"
				>
					<path d="M4 6h16M4 12h16M4 18h16" />
				</svg>
			</button>
			<span class="brand">Arcagrad</span>
		</header>

		<Sidebar {me} open={navOpen} onclose={() => (navOpen = false)} />

		{#if navOpen}
			<button class="scrim" aria-label="Close menu" onclick={() => (navOpen = false)} type="button"
			></button>
		{/if}

		<main class="content">
			{@render children()}
		</main>
	</div>
{/if}

<style>
	.shell {
		display: flex;
		align-items: flex-start;
		min-height: 100vh;
	}
	.content {
		flex: 1;
		min-width: 0;
		margin-left: var(--sidebar-w);
	}

	.topbar {
		display: none;
	}
	.scrim {
		display: none;
	}

	@media (max-width: 900px) {
		.shell {
			flex-direction: column;
		}
		.topbar {
			position: sticky;
			top: 0;
			z-index: 20;
			display: flex;
			align-items: center;
			gap: var(--space-3);
			width: 100%;
			padding: calc(var(--space-3) + env(safe-area-inset-top, 0px)) var(--space-4) var(--space-3);
			background: color-mix(in srgb, var(--bg) 90%, transparent);
			backdrop-filter: blur(8px);
			border-bottom: 1px solid var(--border);
		}
		.burger {
			all: unset;
			display: inline-flex;
			cursor: pointer;
			color: var(--text);
		}
		.burger svg {
			width: 1.4rem;
			height: 1.4rem;
		}
		.brand {
			font-family: var(--font-display);
			font-weight: 700;
			font-size: 1.05rem;
		}
		.content {
			width: 100%;
			margin-left: 0;
		}
		.scrim {
			all: unset;
			position: fixed;
			inset: 0;
			z-index: 40;
			display: block;
			background: rgba(0, 0, 0, 0.5);
			cursor: pointer;
		}
	}
</style>
