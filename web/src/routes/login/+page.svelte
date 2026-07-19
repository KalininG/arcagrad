<script>
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import { auth, ApiError } from '$lib/api.js';

	let setupRequired = $state(false);
	let signupEnabled = $state(false);
	let guestEnabled = $state(false);
	let registering = $state(false);
	let ready = $state(false);
	let username = $state('');
	let password = $state('');
	let confirm = $state('');
	let error = $state(null);
	let busy = $state(false);

	const creating = $derived(setupRequired || registering);

	onMount(async () => {
		try {
			const s = await auth.status();
			if (s.authenticated) {
				goto('/');
				return;
			}
			setupRequired = s.setup_required;
			signupEnabled = s.signup_enabled;
			guestEnabled = s.guest_enabled;
		} catch (e) {
			error = e.message ?? String(e);
		}
		ready = true;
	});

	function toggleRegister() {
		registering = !registering;
		error = null;
		confirm = '';
	}

	async function submit(e) {
		e.preventDefault();
		error = null;
		if (creating) {
			if (password.length < 8) {
				error = 'Password must be at least 8 characters.';
				return;
			}
			if (password !== confirm) {
				error = "Passwords don't match.";
				return;
			}
		}
		busy = true;
		try {
			await (setupRequired
				? auth.setup(username, password)
				: registering
					? auth.register(username, password)
					: auth.login(username, password));
			goto('/');
		} catch (e) {
			if (e instanceof ApiError && e.status === 401) error = 'Invalid username or password.';
			else if (e instanceof ApiError && e.status === 409) error = 'That username is taken.';
			else error = e.message ?? String(e);
		} finally {
			busy = false;
		}
	}
</script>

<div class="wrap">
	{#if ready}
		<div class="card" class:setup={setupRequired}>
			{#if setupRequired}
				<p class="welcome">Welcome to</p>
				<h1>Arcagrad</h1>
				<h2>Create the administrator account</h2>
				<p class="hint">
					This account owns the server: it manages the library, uploads and deletes items,
					configures plugins, and creates accounts for other readers. You can add regular users
					later from Settings → Users.
				</p>
			{:else}
				<h1>arcagrad</h1>
				<h2>{registering ? 'Create an account' : 'Sign in'}</h2>
			{/if}
			<form onsubmit={submit}>
				<input placeholder="Username" autocomplete="username" bind:value={username} />
				<input
					type="password"
					placeholder={creating ? 'Password (min 8 characters)' : 'Password'}
					autocomplete={creating ? 'new-password' : 'current-password'}
					bind:value={password}
				/>
				{#if creating}
					<input
						type="password"
						placeholder="Confirm password"
						autocomplete="new-password"
						bind:value={confirm}
					/>
				{/if}
				{#if error}<p class="err">{error}</p>{/if}
				<button disabled={busy || !username || !password || (creating && !confirm)}>
					{setupRequired ? 'Create admin account' : registering ? 'Create account' : 'Sign in'}
				</button>
			</form>
			{#if !setupRequired && signupEnabled}
				<button class="switch" type="button" onclick={toggleRegister}>
					{registering ? 'Have an account? Sign in' : 'New here? Create an account'}
				</button>
			{/if}
			{#if !setupRequired && guestEnabled}
				<a class="switch guestlink" href="/">Continue as guest</a>
			{/if}
			{#if setupRequired}
				<p class="foot">Your library stays on this server — nothing leaves it.</p>
			{/if}
		</div>
	{/if}
</div>

<style>
	.wrap {
		min-height: 100vh;
		display: grid;
		place-items: center;
		padding: 1.5rem;
	}
	.card {
		width: 100%;
		max-width: 340px;
		background: var(--surface);
		border: 1px solid var(--border);
		border-radius: 14px;
		padding: 2rem;
	}
	h1 {
		font-family: var(--font-display);
		font-size: 1.4rem;
		font-weight: 700;
		margin: 0 0 0.25rem;
		letter-spacing: 0.02em;
	}
	h2 {
		font-size: 1rem;
		font-weight: 500;
		margin: 0 0 1.25rem;
		color: var(--muted);
	}
	.hint {
		font-size: 0.85rem;
		color: var(--muted);
		margin: -0.5rem 0 1rem;
	}
	.card.setup {
		max-width: 420px;
	}
	.card.setup h1 {
		font-size: 1.9rem;
		margin-bottom: 0.75rem;
	}
	.card.setup h2 {
		color: var(--text);
		font-weight: 600;
		margin-bottom: 0.5rem;
	}
	.card.setup .hint {
		margin: 0 0 1.25rem;
		line-height: 1.55;
	}
	.welcome {
		margin: 0;
		font-size: 0.72rem;
		letter-spacing: 0.16em;
		text-transform: uppercase;
		color: var(--muted);
	}
	.foot {
		margin: 1rem 0 0;
		font-size: 0.75rem;
		color: var(--muted);
		text-align: center;
	}
	.switch {
		all: unset;
		display: block;
		margin: 1rem auto 0;
		font-size: 0.82rem;
		color: var(--muted);
		cursor: pointer;
	}
	.switch:hover {
		color: var(--text);
		text-decoration: underline;
	}
	.guestlink {
		width: fit-content;
		text-align: center;
	}
	.switch + .guestlink {
		margin-top: 0.5rem;
	}
	form {
		display: flex;
		flex-direction: column;
		gap: 0.8rem;
	}
	.err {
		margin: 0;
		font-size: 0.85rem;
		line-height: 1.4;
		color: var(--bad, #e5484d);
	}
	button {
		margin-top: 0.25rem;
		background: var(--accent);
		color: #fff;
		border: none;
		font-weight: 600;
	}
</style>
