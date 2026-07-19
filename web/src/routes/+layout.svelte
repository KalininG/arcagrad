<script>
	import '../app.css';
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import { configureApi, auth as authApi } from '$lib/api.js';
	import DownloadBubble from '$lib/components/DownloadBubble.svelte';
	import DesktopUpdate from '$lib/components/DesktopUpdate.svelte';

	let { children } = $props();

	// The same bundle uses cookies in browsers and secure-storage Bearer tokens in Tauri.
	const invoke = (cmd, args) => globalThis.__TAURI__?.core?.invoke(cmd, args);
	const isTauri = typeof globalThis !== 'undefined' && !!globalThis.__TAURI__?.core?.invoke;

	async function readStoredSession() {
		if (!('__arcaSession' in globalThis)) {
			globalThis.__arcaSession = (await invoke('stored_session')) ?? null;
		}
		return globalThis.__arcaSession;
	}

	let ready = $state(!isTauri);
	let checking = $state(isTauri);
	let connecting = $state(false);
	let serverUrl = $state('');
	let username = $state('');
	let password = $state('');
	let error = $state(null);

	if (!isTauri) {
		configureApi({ onUnauthorized: () => goto('/login') });
	}

	function applySession(s) {
		configureApi({
			baseUrl: s.server_url,
			auth: 'bearer',
			token: s.token,
			onUnauthorized: handleUnauthorized,
		});
	}

	async function handleUnauthorized() {
		globalThis.__arcaSession = null;
		try {
			await invoke('disconnect');
		} catch {
			/* ignored */
		}
		ready = false;
	}

	onMount(async () => {
		if (!isTauri) return;
		try {
			const s = await readStoredSession();
			if (s) {
				applySession(s);
				await authApi.me();
				serverUrl = s.server_url;
				ready = true;
				return;
			}
		} catch {
			globalThis.__arcaSession = null;
			try {
				await invoke('disconnect');
			} catch {
				/* ignored */
			}
		} finally {
			checking = false;
		}
	});

	async function connect(e) {
		e.preventDefault();
		error = null;
		connecting = true;
		try {
			const s = await invoke('connect', { serverUrl, username, password });
			globalThis.__arcaSession = s;
			applySession(s);
			await authApi.me();
			password = '';
			ready = true;
		} catch (err) {
			error = typeof err === 'string' ? err : (err?.message ?? 'Could not connect.');
		} finally {
			connecting = false;
		}
	}
</script>

<DownloadBubble />
<DesktopUpdate />

{#if ready}
	{@render children()}
{:else if checking}
	<!-- Keep the connection form hidden until the stored session is checked. -->
{:else}
	<div class="connect-wrap">
		<form class="connect-card" onsubmit={connect}>
			<h1>arcagrad</h1>
			<p class="sub">Connect to your server</p>
			<input
				placeholder="Server URL (https://…)"
				autocomplete="url"
				autocapitalize="off"
				spellcheck="false"
				bind:value={serverUrl}
			/>
			<input placeholder="Username" autocomplete="username" bind:value={username} />
			<input
				type="password"
				placeholder="Password"
				autocomplete="current-password"
				bind:value={password}
			/>
			{#if error}<p class="err">{error}</p>{/if}
			<button disabled={connecting || !serverUrl || !username || !password}>
				{connecting ? 'Connecting…' : 'Connect'}
			</button>
		</form>
	</div>
{/if}

<style>
	.connect-wrap {
		display: flex;
		align-items: center;
		justify-content: center;
		min-height: 100vh;
		padding: 1.5rem;
	}
	.connect-card {
		display: flex;
		flex-direction: column;
		gap: 0.8rem;
		width: min(22rem, 100%);
		background: var(--surface);
		border: 1px solid var(--border);
		border-radius: 14px;
		padding: 2rem;
	}
	.connect-card h1 {
		font-family: var(--font-display);
		font-size: 1.4rem;
		font-weight: 700;
		letter-spacing: 0.02em;
		color: var(--text);
		margin: 0;
	}
	.connect-card .sub {
		margin: 0 0 0.5rem;
		color: var(--muted);
		font-size: 0.95rem;
	}
	.connect-card button {
		margin-top: 0.25rem;
		background: var(--accent);
		color: #fff;
		border-color: var(--accent);
		font-weight: 600;
	}
	.connect-card .err {
		margin: 0;
		color: var(--accent);
		font-size: 0.9rem;
	}
</style>
