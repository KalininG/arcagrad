<script>
	import { onMount } from 'svelte';
	import { page } from '$app/stores';
	import { assets } from '$app/paths';
	import { orderedKinds, setKindOrder, ensureKinds, kindLabel, kindHref } from '$lib/kinds.js';
	import { auth } from '$lib/api.js';
	import { currentUser } from '$lib/session.js';
	import { loadCatalog, updateCount } from '$lib/plugins/catalog.js';
	import { loadFollows, followNewCount } from '$lib/follows/store.js';

	let { me, open = false, onclose } = $props();
	const who = $derived($currentUser ?? me);

	onMount(ensureKinds);
	onMount(() => {
		if (who?.role !== 'admin') return;
		loadCatalog().catch(() => {});
		loadFollows().catch(() => {});
		const tick = () => loadFollows().catch(() => {});
		const timer = setInterval(tick, 5 * 60 * 1000);
		const onVisible = () => {
			if (document.visibilityState === 'visible') tick();
		};
		document.addEventListener('visibilitychange', onVisible);
		return () => {
			clearInterval(timer);
			document.removeEventListener('visibilitychange', onVisible);
		};
	});

	let dragKind = $state(null);
	let dropKind = $state(null);
	let dropAfter = $state(false);
	function onDragStart(e, kind) {
		dragKind = kind;
		e.dataTransfer.effectAllowed = 'move';
		try {
			e.dataTransfer.setData('text/plain', kind);
		} catch {
			/* ignored */
		}
	}
	function onDragOver(e, kind) {
		if (!dragKind || kind === dragKind) return;
		e.preventDefault();
		e.dataTransfer.dropEffect = 'move';
		const r = e.currentTarget.getBoundingClientRect();
		dropKind = kind;
		dropAfter = e.clientY > r.top + r.height / 2;
	}
	function onDrop(e) {
		e.preventDefault();
		if (dragKind && dropKind && dragKind !== dropKind) {
			const names = $orderedKinds.map((k) => k.kind);
			names.splice(names.indexOf(dragKind), 1);
			let to = names.indexOf(dropKind);
			if (dropAfter) to += 1;
			names.splice(to, 0, dragKind);
			setKindOrder(names);
		}
		resetDrag();
	}
	const resetDrag = () => {
		dragKind = null;
		dropKind = null;
	};

	const ICONS = {
		home: ['M3 9.6 12 3l9 6.6V20a1 1 0 0 1-1 1h-5v-6H9v6H4a1 1 0 0 1-1-1z'],
		book: [
			'M6.5 2H20v20H6.5A2.5 2.5 0 0 1 4 19.5v-15A2.5 2.5 0 0 1 6.5 2z',
			'M4 19.5A2.5 2.5 0 0 1 6.5 17H20',
		],
		logout: ['M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4', 'M16 17l5-5-5-5', 'M21 12H9'],
		upload: ['M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4', 'M17 8l-5-5-5 5', 'M12 3v12'],
		server: [
			'M4 4h16a1 1 0 0 1 1 1v4a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1V5a1 1 0 0 1 1-1z',
			'M4 14h16a1 1 0 0 1 1 1v4a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1v-4a1 1 0 0 1 1-1z',
			'M7 7h.01',
			'M7 17h.01',
		],
		puzzle: [
			'M19.4 14h1.6a1 1 0 0 0 1-1v-3a1 1 0 0 0-1-1h-1.6a2.4 2.4 0 1 0-4.8 0H13a1 1 0 0 0-1 1v1.6a2.4 2.4 0 1 1 0 4.8V18a1 1 0 0 0 1 1h3a1 1 0 0 0 1-1v-1.6a2.4 2.4 0 1 1 2.4-2.4z',
			'M12 11.6V10a1 1 0 0 0-1-1H9.4a2.4 2.4 0 1 0-4.8 0H3a1 1 0 0 0-1 1v3a1 1 0 0 0 1 1h1.6a2.4 2.4 0 1 0 4.8 0H11a1 1 0 0 0 1-1v-1.4',
		],
		calendar: ['M7 3v3', 'M17 3v3', 'M4 9h16', 'M5 5h14a1 1 0 0 1 1 1v14H4V6a1 1 0 0 1 1-1z'],
	};

	const isAdmin = $derived(me?.role === 'admin');

	const NAV = $derived([
		{ label: 'Home', href: '/', icon: 'home' },
		...$orderedKinds.map((k) => ({
			label: kindLabel(k.kind),
			href: kindHref(k.kind),
			icon: 'book',
			count: k.count,
			kind: k.kind,
			children: [
				{ label: 'Library', href: kindHref(k.kind) },
				...(who?.role !== 'guest' ? [{ label: 'Browse', href: `${kindHref(k.kind)}/browse` }] : []),
			],
		})),
	]);

	const path = $derived($page.url.pathname);
	const isActive = (href, p) => (href === '/' ? p === '/' : p === href || p.startsWith(href + '/'));
	const sectionOpen = (item, p) => p === item.href || p.startsWith(item.href + '/');
</script>

<aside class="sidebar" class:open>
	<div class="head">
		<div class="brand">
			<img class="logo" src="{assets}/favicon.svg" alt="" />
			<span class="name">Arcagrad</span>
		</div>
		<button class="close" onclick={onclose} aria-label="Close menu" type="button">
			<svg
				viewBox="0 0 24 24"
				fill="none"
				stroke="currentColor"
				stroke-width="2"
				stroke-linecap="round"
			>
				<path d="M6 6l12 12M18 6 6 18" />
			</svg>
		</button>
	</div>

	<nav>
		{#each NAV as item (item.href)}
			{#if item.children}
				{@const open = sectionOpen(item, path)}
				<div
					class="navgroup"
					class:dragging={dragKind === item.kind}
					class:dropbefore={dropKind === item.kind && !dropAfter}
					class:dropafter={dropKind === item.kind && dropAfter}
					role="listitem"
					draggable="true"
					ondragstart={(e) => onDragStart(e, item.kind)}
					ondragover={(e) => onDragOver(e, item.kind)}
					ondrop={onDrop}
					ondragend={resetDrag}
				>
					<a
						class="navitem parent"
						class:open
						href={item.href}
						title={item.label}
						draggable="false"
					>
						<svg
							class="ico"
							viewBox="0 0 24 24"
							fill="none"
							stroke="currentColor"
							stroke-width="2"
							stroke-linecap="round"
							stroke-linejoin="round"
						>
							{#each ICONS[item.icon] as d (d)}<path {d} />{/each}
						</svg>
						<span class="label">{item.label}</span>
						{#if item.count != null}<span class="navcount">{item.count}</span>{/if}
						<svg
							class="caret"
							class:open
							viewBox="0 0 20 20"
							fill="none"
							stroke="currentColor"
							stroke-width="2"
							stroke-linecap="round"
							stroke-linejoin="round"
						>
							<path d="M6 8l4 4 4-4" />
						</svg>
					</a>
					{#if open}
						<div class="subnav">
							{#each item.children as child (child.href)}
								<a class="subitem" class:active={path === child.href} href={child.href}>
									{child.label}
								</a>
							{/each}
						</div>
					{/if}
				</div>
			{:else}
				<a
					class="navitem"
					class:active={isActive(item.href, path)}
					href={item.href}
					title={item.label}
				>
					<svg
						class="ico"
						viewBox="0 0 24 24"
						fill="none"
						stroke="currentColor"
						stroke-width="2"
						stroke-linecap="round"
						stroke-linejoin="round"
					>
						{#each ICONS[item.icon] as d (d)}<path {d} />{/each}
					</svg>
					<span class="label">{item.label}</span>
					{#if item.count != null}<span class="navcount">{item.count}</span>{/if}
				</a>
			{/if}
		{/each}
	</nav>

	<div class="foot">
		{#if isAdmin}
			<a
				class="footbtn"
				class:active={isActive('/upload', path)}
				href="/upload"
				title="Upload archives"
			>
				<svg
					class="ico"
					viewBox="0 0 24 24"
					fill="none"
					stroke="currentColor"
					stroke-width="2"
					stroke-linecap="round"
					stroke-linejoin="round"
				>
					{#each ICONS.upload as d (d)}<path {d} />{/each}
				</svg>
				<span>Upload</span>
			</a>
		{/if}
		<a class="footbtn" class:active={isActive('/tracking', path)} href="/tracking" title="Tracking">
			<svg
				class="ico"
				viewBox="0 0 24 24"
				fill="none"
				stroke="currentColor"
				stroke-width="2"
				stroke-linecap="round"
				stroke-linejoin="round"
			>
				{#each ICONS.calendar as d (d)}<path {d} />{/each}
			</svg>
			<span>Tracking</span>
			{#if isAdmin && $followNewCount > 0}
				<span
					class="updot"
					title="{$followNewCount} new {$followNewCount === 1
						? 'item'
						: 'items'} from searches you follow">{$followNewCount}</span
				>
			{/if}
		</a>
		{#if isAdmin}
			<a class="footbtn" class:active={isActive('/plugins', path)} href="/plugins" title="Plugins">
				<svg
					class="ico"
					viewBox="0 0 24 24"
					fill="none"
					stroke="currentColor"
					stroke-width="2"
					stroke-linecap="round"
					stroke-linejoin="round"
				>
					{#each ICONS.puzzle as d (d)}<path {d} />{/each}
				</svg>
				<span>Plugins</span>
				{#if $updateCount > 0}
					<span
						class="updot"
						title="{$updateCount} plugin {$updateCount === 1 ? 'update' : 'updates'} available"
						>{$updateCount}</span
					>
				{/if}
			</a>
			<a class="footbtn" class:active={isActive('/server', path)} href="/server" title="Server">
				<svg
					class="ico"
					viewBox="0 0 24 24"
					fill="none"
					stroke="currentColor"
					stroke-width="2"
					stroke-linecap="round"
					stroke-linejoin="round"
				>
					{#each ICONS.server as d (d)}<path {d} />{/each}
				</svg>
				<span>Server</span>
			</a>
		{/if}
		{#if who?.role === 'guest'}
			<a class="footbtn" href="/login" title="Sign in">
				<svg
					class="ico"
					viewBox="0 0 24 24"
					fill="none"
					stroke="currentColor"
					stroke-width="2"
					stroke-linecap="round"
					stroke-linejoin="round"
				>
					{#each ICONS.logout as d (d)}<path {d} />{/each}
				</svg>
				<span>Sign in</span>
			</a>
		{:else}
			<a
				class="footbtn profile"
				class:active={isActive('/profile', path)}
				href="/profile"
				title="Profile"
			>
				{#if who?.avatar_version}
					<img class="avatar" src={auth.avatar.url(who.avatar_version)} alt="" />
				{:else}
					<span class="avatar" aria-hidden="true"
						>{(who?.username ?? '?').slice(0, 1).toUpperCase()}</span
					>
				{/if}
				<span class="profname">{who?.username ?? 'Profile'}</span>
				{#if who?.role === 'admin'}<span class="rolebadge">admin</span>{/if}
			</a>
		{/if}
	</div>
</aside>

<style>
	.sidebar {
		position: fixed;
		top: 0;
		left: 0;
		height: 100vh;
		width: var(--sidebar-w);
		flex: 0 0 auto;
		display: flex;
		flex-direction: column;
		padding: var(--space-5) var(--space-3);
		border-right: 1px solid var(--border);
		background: var(--bg);
		overflow: hidden;
	}

	.head {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--space-2);
		margin-bottom: var(--space-5);
		padding: 0 var(--space-2);
	}
	.brand {
		display: flex;
		align-items: center;
		gap: var(--space-3);
		min-width: 0;
	}
	.logo {
		width: 26px;
		height: 26px;
		flex: 0 0 auto;
		border-radius: 6px;
		display: block;
	}
	.name {
		font-family: var(--font-display);
		font-weight: 700;
		font-size: 1.15rem;
		white-space: nowrap;
	}
	.close {
		display: none;
	}

	nav {
		display: flex;
		flex-direction: column;
		gap: 2px;
		flex: 1;
		min-height: 0;
		overflow-y: auto;
	}
	.navitem {
		display: flex;
		align-items: center;
		gap: var(--space-3);
		padding: var(--space-2) var(--space-3);
		border-radius: var(--radius);
		color: var(--muted);
		font-size: 0.92rem;
		transition:
			background var(--ease),
			color var(--ease);
	}
	.navitem:hover,
	.footbtn:hover {
		background: var(--surface);
		color: var(--text);
	}
	.navitem.active,
	.footbtn.active {
		background: var(--accent-soft);
		color: var(--accent);
	}
	.ico {
		width: 18px;
		height: 18px;
		flex: 0 0 auto;
	}

	.navgroup {
		display: flex;
		flex-direction: column;
		border-radius: var(--radius);
	}
	.navgroup.dragging {
		opacity: 0.4;
	}
	.navgroup.dropbefore {
		box-shadow: inset 0 2px 0 var(--accent);
	}
	.navgroup.dropafter {
		box-shadow: inset 0 -2px 0 var(--accent);
	}
	.parent .label,
	.navitem .label {
		flex: 1 1 auto;
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.navcount {
		flex: 0 0 auto;
		font-variant-numeric: tabular-nums;
		font-size: 0.72rem;
		color: var(--muted);
	}
	.parent.open {
		color: var(--text);
	}
	.caret {
		width: 0.9rem;
		height: 0.9rem;
		flex: 0 0 auto;
		color: var(--muted);
		transition: transform var(--ease);
	}
	.caret.open {
		transform: rotate(180deg);
	}
	.subnav {
		display: flex;
		flex-direction: column;
		gap: 2px;
		margin: 2px 0;
		padding-left: calc(18px + var(--space-3));
	}
	.subitem {
		display: block;
		padding: var(--space-2) var(--space-3);
		border-radius: var(--radius);
		color: var(--muted);
		font-size: 0.88rem;
		transition:
			background var(--ease),
			color var(--ease);
	}
	.subitem:hover {
		background: var(--surface);
		color: var(--text);
	}
	.subitem.active {
		background: var(--accent-soft);
		color: var(--accent);
	}

	.foot {
		margin-top: auto;
		display: flex;
		flex-direction: column;
		gap: var(--space-2);
		padding-top: var(--space-4);
	}
	.footbtn {
		all: unset;
		box-sizing: border-box;
		display: flex;
		align-items: center;
		gap: var(--space-3);
		padding: var(--space-2) var(--space-3);
		border-radius: var(--radius);
		color: var(--muted);
		font-size: 0.92rem;
		cursor: pointer;
		transition:
			background var(--ease),
			color var(--ease);
	}
	.updot {
		margin-left: auto;
		min-width: 1.15rem;
		padding: 0 0.35rem;
		height: 1.15rem;
		display: inline-flex;
		align-items: center;
		justify-content: center;
		border-radius: 9999px;
		background: var(--accent);
		color: #fff;
		font-size: 0.7rem;
		font-weight: 600;
		font-variant-numeric: tabular-nums;
	}
	.avatar {
		width: 24px;
		height: 24px;
		flex: 0 0 auto;
		display: inline-flex;
		align-items: center;
		justify-content: center;
		border-radius: 50%;
		background: var(--accent);
		color: #fff;
		font-size: 0.72rem;
		font-weight: 700;
		object-fit: cover;
	}
	.profname {
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.rolebadge {
		flex: 0 0 auto;
		margin-left: auto;
		padding: 1px 6px;
		border: 1px solid var(--border);
		border-radius: 9999px;
		font-size: 0.62rem;
		letter-spacing: 0.06em;
		text-transform: uppercase;
		color: var(--muted);
	}

	@media (max-width: 900px) {
		.sidebar {
			position: fixed;
			top: 0;
			left: 0;
			z-index: 50;
			width: min(82vw, 300px);
			box-shadow: var(--shadow-lg);
			transform: translateX(-100%);
			transition: transform var(--ease);
			padding-top: calc(var(--space-5) + env(safe-area-inset-top, 0px));
		}
		.sidebar.open {
			transform: translateX(0);
		}
		.close {
			all: unset;
			display: inline-flex;
			align-items: center;
			justify-content: center;
			width: 1.9rem;
			height: 1.9rem;
			border-radius: var(--radius-sm);
			color: var(--muted);
			cursor: pointer;
		}
		.close svg {
			width: 1.2rem;
			height: 1.2rem;
		}
		.close:hover {
			color: var(--text);
			background: var(--surface);
		}
	}
</style>
