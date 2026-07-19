<script>
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import { auth, ApiError, apiAuthMode } from '$lib/api.js';
	import { currentUser, setUser, performLogout } from '$lib/session.js';
	import DeleteConfirm from '$lib/components/ui/DeleteConfirm.svelte';
	import PageHeader from '$lib/components/ui/PageHeader.svelte';
	import StatsView from '$lib/stats/StatsView.svelte';
	import DisplaySettings from '$lib/profile/DisplaySettings.svelte';

	const me = $derived($currentUser);
	const TAB_IDS = ['stats', 'display', 'keys'];
	function initTab() {
		try {
			const t = new URLSearchParams(location.search).get('tab');
			return TAB_IDS.includes(t) ? t : 'stats';
		} catch {
			return 'stats';
		}
	}
	let activeTab = $state(initTab());
	function setTab(id) {
		if (id === activeTab) return;
		activeTab = id;
		const url = new URL(location.href);
		url.searchParams.set('tab', id);
		goto(url, { replaceState: true, keepFocus: true, noScroll: true });
	}
	const bearer = apiAuthMode() === 'bearer';

	function fmtMonth(secs) {
		if (!secs) return '';
		return new Date(secs * 1000).toLocaleDateString(undefined, {
			year: 'numeric',
			month: 'long',
		});
	}

	let avatarBusy = $state(false);
	let avatarError = $state(null);
	let fileInput = $state(null);
	async function onAvatarPicked(e) {
		const file = e.target.files?.[0];
		e.target.value = '';
		if (!file || avatarBusy) return;
		avatarBusy = true;
		avatarError = null;
		try {
			await auth.avatar.upload(file);
			setUser(await auth.me());
		} catch (err) {
			avatarError = err.message ?? String(err);
		} finally {
			avatarBusy = false;
		}
	}
	async function removeAvatar() {
		if (avatarBusy) return;
		avatarBusy = true;
		avatarError = null;
		try {
			await auth.avatar.remove();
			setUser(await auth.me());
		} catch (err) {
			avatarError = err.message ?? String(err);
		} finally {
			avatarBusy = false;
		}
	}

	let bannerBusy = $state(false);
	let bannerError = $state(null);
	let bannerInput = $state(null);
	async function onBannerPicked(e) {
		const file = e.target.files?.[0];
		e.target.value = '';
		if (!file || bannerBusy) return;
		bannerBusy = true;
		bannerError = null;
		try {
			await auth.banner.upload(file);
			setUser(await auth.me());
		} catch (err) {
			bannerError = err.message ?? String(err);
		} finally {
			bannerBusy = false;
		}
	}
	async function removeBanner() {
		if (bannerBusy) return;
		bannerBusy = true;
		bannerError = null;
		try {
			await auth.banner.remove();
			setUser(await auth.me());
		} catch (err) {
			bannerError = err.message ?? String(err);
		} finally {
			bannerBusy = false;
		}
	}

	let showPwModal = $state(false);
	let pwCurrent = $state('');
	let pwNew = $state('');
	let pwConfirm = $state('');
	let pwBusy = $state(false);
	let pwError = $state(null);
	let pwDone = $state(false);
	function openPwModal() {
		pwCurrent = pwNew = pwConfirm = '';
		pwError = null;
		pwDone = false;
		showPwModal = true;
	}
	function closePwModal() {
		if (!pwBusy) showPwModal = false;
	}
	function onPwKey(e) {
		if (e.key === 'Escape') closePwModal();
	}
	async function submitPassword(e) {
		e.preventDefault();
		if (pwBusy) return;
		pwError = null;
		if (pwNew.length < 8) {
			pwError = 'New password must be at least 8 characters.';
			return;
		}
		if (pwNew !== pwConfirm) {
			pwError = "New passwords don't match.";
			return;
		}
		pwBusy = true;
		try {
			await auth.changePassword(pwCurrent, pwNew);
			pwCurrent = pwNew = pwConfirm = '';
			pwDone = true;
			showPwModal = false;
		} catch (err) {
			pwError = err.message ?? String(err);
		} finally {
			pwBusy = false;
		}
	}

	let showAllConfirm = $state(false);
	let allBusy = $state(false);
	async function logoutEverywhere() {
		if (allBusy) return;
		allBusy = true;
		try {
			await auth.logoutAll();
		} catch {
			/* ignored */
		}
		await performLogout();
	}

	let keys = $state([]);
	let keysError = $state(null);
	const KEYS_SHOWN = 5;
	let showAllKeys = $state(false);
	const visibleKeys = $derived(showAllKeys ? keys : keys.slice(0, KEYS_SHOWN));
	let newLabel = $state('');
	let creating = $state(false);
	let minted = $state(null);
	let copied = $state(false);

	async function loadKeys() {
		try {
			keys = await auth.keys.list();
			keysError = null;
		} catch (e) {
			if (!(e instanceof ApiError && e.status === 401)) keysError = e.message ?? String(e);
		}
	}

	async function createKey(e) {
		e.preventDefault();
		const label = newLabel.trim();
		if (!label || creating) return;
		creating = true;
		keysError = null;
		try {
			const res = await auth.keys.create(label);
			minted = { label: res.label, key: res.key };
			copied = false;
			newLabel = '';
			await loadKeys();
		} catch (e) {
			keysError = e.message ?? String(e);
		} finally {
			creating = false;
		}
	}

	let revokeTarget = $state(null);
	let revokeBusy = $state(false);
	async function confirmRevoke() {
		if (!revokeTarget || revokeBusy) return;
		revokeBusy = true;
		try {
			await auth.keys.revoke(revokeTarget.id);
			keys = keys.filter((k) => k.id !== revokeTarget.id);
			revokeTarget = null;
		} catch (e) {
			keysError = e.message ?? String(e);
			revokeTarget = null;
		} finally {
			revokeBusy = false;
		}
	}

	async function copyMinted() {
		if (!minted) return;
		try {
			await navigator.clipboard.writeText(minted.key);
			copied = true;
		} catch {
			/* ignored */
		}
	}

	function fmtDate(secs) {
		if (!secs) return '';
		return new Date(secs * 1000).toLocaleDateString(undefined, {
			year: 'numeric',
			month: 'short',
			day: 'numeric',
		});
	}

	onMount(() => {
		if (!bearer) loadKeys();
	});
</script>

<div class="app-page profile">
	<PageHeader title="Profile" />

	{#if me?.banner_version}
		<div class="bannerwrap">
			<button
				class="bannerbtn"
				type="button"
				title="Change banner"
				disabled={bannerBusy}
				onclick={() => bannerInput?.click()}
			>
				<img class="bannerimg" src={auth.banner.url(me.banner_version)} alt="Profile banner" />
				<span class="bannerscrim" aria-hidden="true"></span>
				<span class="banneredit" aria-hidden="true">
					<svg
						viewBox="0 0 24 24"
						fill="none"
						stroke="currentColor"
						stroke-width="2"
						stroke-linecap="round"
						stroke-linejoin="round"
					>
						<path d="M12 20h9" /><path d="M16.5 3.5a2.12 2.12 0 0 1 3 3L7 19l-4 1 1-4Z" />
					</svg>
				</span>
			</button>
			<button
				class="bannerremove"
				type="button"
				title="Remove banner"
				aria-label="Remove banner"
				disabled={bannerBusy}
				onclick={removeBanner}
			>
				<svg
					viewBox="0 0 24 24"
					fill="none"
					stroke="currentColor"
					stroke-width="2.5"
					stroke-linecap="round"
				>
					<path d="M18 6L6 18M6 6l12 12" />
				</svg>
			</button>
		</div>
	{/if}
	<input
		type="file"
		accept="image/*"
		bind:this={bannerInput}
		onchange={onBannerPicked}
		style="display: none"
	/>
	{#if bannerError}<p class="error">{bannerError}</p>{/if}

	<div class="who" class:overbanner={me?.banner_version}>
		<div class="avatarwrap">
			<button
				class="avatarbtn"
				type="button"
				title="Change profile picture"
				disabled={avatarBusy}
				onclick={() => fileInput?.click()}
			>
				{#if me?.avatar_version}
					<img class="avatar" src={auth.avatar.url(me.avatar_version)} alt="Profile picture" />
				{:else}
					<span class="avatar" aria-hidden="true"
						>{(me?.username ?? '?').slice(0, 1).toUpperCase()}</span
					>
				{/if}
				<span class="avataredit" aria-hidden="true">
					<svg
						viewBox="0 0 24 24"
						fill="none"
						stroke="currentColor"
						stroke-width="2"
						stroke-linecap="round"
						stroke-linejoin="round"
					>
						<path d="M12 20h9" /><path d="M16.5 3.5a2.12 2.12 0 0 1 3 3L7 19l-4 1 1-4Z" />
					</svg>
				</span>
			</button>
			{#if me?.avatar_version}
				<button
					class="avatarremove"
					type="button"
					title="Remove picture"
					aria-label="Remove picture"
					disabled={avatarBusy}
					onclick={removeAvatar}
				>
					<svg
						viewBox="0 0 24 24"
						fill="none"
						stroke="currentColor"
						stroke-width="2.5"
						stroke-linecap="round"
					>
						<path d="M18 6L6 18M6 6l12 12" />
					</svg>
				</button>
			{/if}
		</div>
		<input
			type="file"
			accept="image/*"
			bind:this={fileInput}
			onchange={onAvatarPicked}
			style="display: none"
		/>
		<div class="whotext">
			<div class="whorow">
				<span class="uname">{me?.username ?? ''}</span>
				{#if me?.role === 'admin'}<span class="rolebadge">admin</span>{/if}
			</div>
			{#if me?.created_at}<span class="since">Member since {fmtMonth(me.created_at)}</span>{/if}
			<div class="whoactions">
				{#if !bearer}
					<button class="linkbtn" type="button" onclick={openPwModal}>Change password</button>
					{#if pwDone}<span class="okmsg">Password updated</span>{/if}
				{/if}
				{#if !me?.banner_version}
					<button
						class="linkbtn"
						type="button"
						disabled={bannerBusy}
						onclick={() => bannerInput?.click()}
					>
						Add banner
					</button>
				{/if}
			</div>
			{#if avatarError}<span class="error">{avatarError}</span>{/if}
		</div>

		<div class="sessionbox">
			<button class="logout" type="button" onclick={performLogout}>
				<svg
					viewBox="0 0 24 24"
					fill="none"
					stroke="currentColor"
					stroke-width="2"
					stroke-linecap="round"
					stroke-linejoin="round"
				>
					<path d="M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4" />
					<path d="M16 17l5-5-5-5" />
					<path d="M21 12H9" />
				</svg>
				Log out
			</button>
			{#if !bearer}
				<button
					class="linkbtn dangerlink"
					type="button"
					disabled={allBusy}
					onclick={() => (showAllConfirm = true)}
				>
					Log out everywhere…
				</button>
			{/if}
		</div>
	</div>

	<nav class="tabs">
		<button
			class="tab"
			class:active={activeTab === 'stats'}
			type="button"
			onclick={() => setTab('stats')}>Stats</button
		>
		<button
			class="tab"
			class:active={activeTab === 'display'}
			type="button"
			onclick={() => setTab('display')}>Display</button
		>
		<button
			class="tab"
			class:active={activeTab === 'keys'}
			type="button"
			onclick={() => setTab('keys')}>API keys</button
		>
	</nav>

	{#if revokeTarget}
		<DeleteConfirm
			heading={`Revoke “${revokeTarget.label}”?`}
			message="Any client or script using this key stops working immediately. This cannot be undone — create a new key to reconnect."
			verb="Revoke"
			busyLabel="Revoking…"
			busy={revokeBusy}
			onConfirm={confirmRevoke}
			onClose={() => (revokeTarget = null)}
		/>
	{/if}

	{#if showPwModal}
		<!-- svelte-ignore a11y_no_static_element_interactions, a11y_click_events_have_key_events -->
		<div class="overlay" onclick={closePwModal} onkeydown={onPwKey}>
			<div
				class="modal"
				role="dialog"
				aria-modal="true"
				aria-label="Change password"
				onclick={(e) => e.stopPropagation()}
			>
				<h2>Change password</h2>
				<p class="hint">Updating your password signs out every other session and device.</p>
				<form class="pwform" onsubmit={submitPassword}>
					<!-- svelte-ignore a11y_autofocus -->
					<input
						type="password"
						placeholder="Current password"
						bind:value={pwCurrent}
						autocomplete="current-password"
						autofocus
					/>
					<input
						type="password"
						placeholder="New password"
						bind:value={pwNew}
						autocomplete="new-password"
					/>
					<input
						type="password"
						placeholder="Confirm new password"
						bind:value={pwConfirm}
						autocomplete="new-password"
					/>
					{#if pwError}<p class="error">{pwError}</p>{/if}
					<div class="modalrow">
						<button type="button" disabled={pwBusy} onclick={closePwModal}>Cancel</button>
						<button
							class="primary"
							type="submit"
							disabled={pwBusy || !pwCurrent || !pwNew || !pwConfirm}
						>
							{pwBusy ? 'Updating…' : 'Update password'}
						</button>
					</div>
				</form>
			</div>
		</div>
	{/if}

	{#if activeTab === 'stats'}
		<StatsView />
	{:else if activeTab === 'display'}
		<DisplaySettings />
	{:else if activeTab === 'keys'}
		<section class="panel">
			<h2>API keys</h2>
			{#if bearer}
				<p class="hint">Signed in with an API key — manage keys from the web UI instead.</p>
			{:else}
				<p class="hint">
					Keys authenticate external clients and scripts (sent as a Bearer token). The secret is
					shown once at creation and can't be recovered — revoke and re-create if lost.
				</p>

				{#if minted}
					<div class="minted">
						<div class="minted-head">
							<strong>“{minted.label}” created.</strong> Copy the key now — it won't be shown again.
						</div>
						<div class="minted-key">
							<code>{minted.key}</code>
							<button type="button" onclick={copyMinted}>{copied ? 'Copied ✓' : 'Copy'}</button>
						</div>
						<button class="dismiss" type="button" onclick={() => (minted = null)}>Done</button>
					</div>
				{/if}

				{#if keys.length}
					<ul class="keys">
						{#each visibleKeys as k (k.id)}
							<li>
								<span class="klabel">{k.label}</span>
								<span class="kdate">
									Created {fmtDate(k.created_at)} ·
									<span class="kused"
										>{k.last_used ? `Last used ${fmtDate(k.last_used)}` : 'Never used'}</span
									>
								</span>
								<button
									class="revoke"
									type="button"
									onclick={() => (revokeTarget = { id: k.id, label: k.label })}
								>
									Revoke
								</button>
							</li>
						{/each}
					</ul>
					{#if keys.length > KEYS_SHOWN}
						<button class="linkbtn" type="button" onclick={() => (showAllKeys = !showAllKeys)}>
							{showAllKeys ? 'Show fewer' : `Show all (${keys.length})`}
						</button>
					{/if}
				{:else if !minted}
					<p class="empty">No API keys yet.</p>
				{/if}

				<form class="newkey" onsubmit={createKey}>
					<input
						type="text"
						placeholder="Label (e.g. iPad, scripts)"
						bind:value={newLabel}
						maxlength="100"
					/>
					<button type="submit" disabled={creating || !newLabel.trim()}>Create key</button>
				</form>
				{#if keysError}<p class="error">{keysError}</p>{/if}
			{/if}
		</section>
	{/if}
</div>

{#if showAllConfirm}
	<DeleteConfirm
		heading="Log out everywhere?"
		message="Ends every session on every device — including this one; you'll be signed out and back at the login page. API keys stay valid."
		verb="Log out everywhere"
		busyLabel="Logging out…"
		busy={allBusy}
		onConfirm={logoutEverywhere}
		onClose={() => (showAllConfirm = false)}
	/>
{/if}

<style>
	.profile {
		display: flex;
		flex-direction: column;
		gap: var(--space-5);
	}
	.profile > :global(.pagehead) {
		margin-bottom: 0;
	}

	.who {
		display: flex;
		align-items: center;
		gap: var(--space-3);
	}
	.who.overbanner {
		margin-top: -56px;
		position: relative;
		z-index: 1;
	}
	.who.overbanner .avatar {
		box-shadow: 0 0 0 4px var(--bg);
	}

	.bannerwrap {
		position: relative;
	}
	.bannerbtn {
		all: unset;
		display: block;
		cursor: pointer;
		width: 100%;
		position: relative;
		border-radius: var(--radius-lg);
		overflow: hidden;
	}
	.bannerimg {
		display: block;
		width: 100%;
		height: 180px;
		object-fit: cover;
	}
	.bannerscrim {
		position: absolute;
		inset: 0;
		background: linear-gradient(
			180deg,
			rgba(0, 0, 0, 0) 35%,
			color-mix(in srgb, var(--bg) 88%, transparent) 96%
		);
		pointer-events: none;
	}
	.banneredit {
		position: absolute;
		inset: 0;
		display: grid;
		place-items: center;
		background: rgba(0, 0, 0, 0.35);
		color: #fff;
		opacity: 0;
		transition: opacity var(--ease, 0.15s);
	}
	.bannerbtn:hover .banneredit,
	.bannerbtn:focus-visible .banneredit {
		opacity: 1;
	}
	.banneredit svg {
		width: 1.2rem;
		height: 1.2rem;
	}
	.bannerremove {
		all: unset;
		box-sizing: border-box;
		position: absolute;
		right: 10px;
		top: 10px;
		width: 30px;
		height: 30px;
		display: grid;
		place-items: center;
		border-radius: 50%;
		background: var(--bad, #e5484d);
		color: #fff;
		cursor: pointer;
		opacity: 0;
		transition: opacity var(--ease, 0.15s);
		box-shadow: 0 0 0 2px var(--bg);
		z-index: 1;
	}
	.bannerwrap:hover .bannerremove,
	.bannerremove:focus-visible {
		opacity: 1;
	}
	.bannerremove svg {
		width: 0.9rem;
		height: 0.9rem;
	}
	.avatarwrap {
		position: relative;
		flex: 0 0 auto;
	}
	.avatarbtn {
		all: unset;
		position: relative;
		display: block;
		cursor: pointer;
		border-radius: 50%;
	}
	.avatar {
		width: 140px;
		height: 140px;
		display: inline-flex;
		align-items: center;
		justify-content: center;
		border-radius: 50%;
		background: var(--accent);
		color: #fff;
		font-size: 3rem;
		font-weight: 700;
		object-fit: cover;
		vertical-align: top;
	}
	.avatarremove {
		all: unset;
		box-sizing: border-box;
		position: absolute;
		right: 4px;
		bottom: 4px;
		width: 30px;
		height: 30px;
		display: grid;
		place-items: center;
		border-radius: 50%;
		background: var(--bad, #e5484d);
		color: #fff;
		cursor: pointer;
		opacity: 0;
		transition: opacity var(--ease, 0.15s);
		box-shadow: 0 0 0 2px var(--bg);
	}
	.avatarwrap:hover .avatarremove,
	.avatarremove:focus-visible {
		opacity: 1;
	}
	.avatarremove svg {
		width: 0.9rem;
		height: 0.9rem;
	}
	.avataredit {
		position: absolute;
		inset: 0;
		display: grid;
		place-items: center;
		border-radius: 50%;
		background: rgba(0, 0, 0, 0.45);
		color: #fff;
		opacity: 0;
		transition: opacity var(--ease, 0.15s);
	}
	.avatarbtn:hover .avataredit,
	.avatarbtn:focus-visible .avataredit {
		opacity: 1;
	}
	.avataredit svg {
		width: 1.1rem;
		height: 1.1rem;
	}
	.whotext {
		display: flex;
		flex-direction: column;
		gap: 2px;
		min-width: 0;
	}
	.whorow {
		display: flex;
		align-items: center;
		gap: var(--space-2);
		min-width: 0;
	}
	.since {
		font-size: 0.78rem;
		color: var(--muted);
	}
	.linkbtn {
		all: unset;
		align-self: flex-start;
		font-size: 0.78rem;
		color: var(--muted);
		cursor: pointer;
	}
	.linkbtn:hover {
		color: var(--text);
		text-decoration: underline;
	}
	.uname {
		font-family: var(--font-display);
		font-size: 1.25rem;
		font-weight: 700;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.rolebadge {
		padding: 2px 8px;
		border: 1px solid var(--border);
		border-radius: 9999px;
		font-size: 0.65rem;
		letter-spacing: 0.06em;
		text-transform: uppercase;
		color: var(--muted);
	}

	section {
		display: flex;
		flex-direction: column;
		gap: var(--space-3);
		padding-top: var(--space-4);
		border-top: 1px solid var(--border);
	}
	h2 {
		font-size: 0.95rem;
		margin: 0;
	}
	.hint {
		margin: 0;
		font-size: 0.82rem;
		color: var(--muted);
		max-width: 65ch;
	}
	.empty {
		margin: 0;
		font-size: 0.85rem;
		color: var(--muted);
	}
	.error {
		margin: 0;
		font-size: 0.85rem;
		color: var(--bad, #e5484d);
	}

	.keys {
		list-style: none;
		margin: 0;
		padding: 0;
		display: flex;
		flex-direction: column;
	}
	.keys li {
		display: flex;
		align-items: center;
		gap: var(--space-3);
		padding: var(--space-2) 0;
		border-bottom: 1px solid var(--border);
	}
	.keys li:last-child {
		border-bottom: none;
	}
	.klabel {
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.kdate {
		margin-left: auto;
		flex: 0 0 auto;
		font-size: 0.78rem;
		color: var(--muted);
	}
	.kused {
		color: var(--accent);
	}
	.revoke {
		flex: 0 0 auto;
		padding: 2px 10px;
		font-size: 0.78rem;
		color: var(--muted);
	}
	.revoke:hover {
		color: var(--bad, #e5484d);
		border-color: var(--bad, #e5484d);
	}

	.newkey {
		display: flex;
		gap: var(--space-2);
	}
	.newkey input {
		flex: 1;
		min-width: 0;
	}

	.minted {
		display: flex;
		flex-direction: column;
		gap: var(--space-2);
		padding: var(--space-3);
		border: 1px solid var(--accent);
		border-radius: var(--radius);
		background: color-mix(in srgb, var(--accent) 8%, transparent);
		font-size: 0.85rem;
	}
	.minted-key {
		display: flex;
		align-items: center;
		gap: var(--space-2);
	}
	.minted-key code {
		flex: 1;
		min-width: 0;
		overflow-wrap: anywhere;
		user-select: all;
		font-size: 0.8rem;
	}
	.dismiss {
		align-self: flex-start;
		font-size: 0.8rem;
	}

	.pwform {
		display: flex;
		flex-direction: column;
		gap: var(--space-2);
	}
	.whoactions {
		display: flex;
		align-items: center;
		gap: var(--space-3);
		margin-top: var(--space-1);
	}
	.whoactions .linkbtn {
		color: var(--accent);
	}
	.whoactions .linkbtn:hover {
		color: var(--accent);
		text-decoration: underline;
	}

	.overlay {
		position: fixed;
		inset: 0;
		z-index: 100;
		background: rgba(0, 0, 0, 0.55);
		display: grid;
		place-items: center;
		padding: var(--space-4);
	}
	.modal {
		width: min(100%, 380px);
		display: flex;
		flex-direction: column;
		gap: var(--space-3);
		padding: var(--space-4);
		background: var(--surface);
		border: 1px solid var(--border);
		border-radius: var(--radius);
		box-shadow: var(--shadow);
	}
	.modal h2 {
		font-size: 1.05rem;
	}
	.modalrow {
		display: flex;
		gap: var(--space-2);
		margin-top: var(--space-1);
	}
	.modalrow button {
		flex: 1 1 0;
	}
	.modalrow .primary {
		background: var(--accent);
		border-color: var(--accent);
		color: #fff;
		font-weight: 600;
	}
	.modalrow .primary:disabled {
		opacity: 0.6;
	}
	.okmsg {
		font-size: 0.82rem;
		color: var(--good, #46a758);
	}

	.sessionbox {
		margin-left: auto;
		align-self: center;
		display: flex;
		flex-direction: column;
		align-items: flex-end;
		gap: var(--space-2);
		flex: 0 0 auto;
	}
	.logout {
		display: inline-flex;
		align-items: center;
		gap: var(--space-2);
		padding: var(--space-2) var(--space-3);
	}
	.logout svg {
		width: 1rem;
		height: 1rem;
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
		border-top: none;
		padding-top: 0;
	}

	.dangerlink {
		align-self: flex-end;
		font-size: 0.78rem;
		color: color-mix(in srgb, var(--bad, #e5484d) 70%, var(--muted));
	}
	.dangerlink:hover {
		color: var(--bad, #e5484d);
		text-decoration: underline;
	}
	.dangerlink:disabled {
		opacity: 0.5;
		cursor: default;
	}

	@media (max-width: 560px) {
		.who {
			flex-direction: column;
			align-items: center;
		}
		.sessionbox {
			margin-left: 0;
			align-items: center;
		}
		.avatar {
			width: 104px;
			height: 104px;
		}
		.whotext {
			width: 100%;
			align-items: center;
			text-align: center;
		}
		.whorow {
			flex-wrap: wrap;
			justify-content: center;
		}
		.whoactions {
			justify-content: center;
		}
		.uname {
			white-space: normal;
			overflow: visible;
			word-break: break-word;
		}
		.keys li {
			flex-wrap: wrap;
			row-gap: 2px;
		}
		.klabel {
			flex: 1 1 100%;
		}
		.kdate {
			margin-left: 0;
		}
		.revoke {
			margin-left: auto;
		}
	}
</style>
