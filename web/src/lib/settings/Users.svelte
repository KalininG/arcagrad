<script>
	import { onMount } from 'svelte';
	import { users as usersApi, kinds as kindsApi } from '$lib/api.js';
	import { kindLabel } from '$lib/kinds.js';

	let error = $state(null);

	let signupEnabled = $state(false);
	let guestEnabled = $state(false);
	let accessLoaded = $state(false);
	let accessBusy = $state(false);
	async function loadAccess() {
		try {
			const a = await usersApi.getAccess();
			signupEnabled = a.signup_enabled;
			guestEnabled = a.guest_enabled;
			accessLoaded = true;
		} catch (e) {
			error = e.message ?? String(e);
		}
	}
	async function setSignup(on) {
		if (accessBusy || on === signupEnabled) return;
		accessBusy = true;
		const prev = signupEnabled;
		signupEnabled = on;
		try {
			await usersApi.putAccess(on, guestEnabled);
		} catch (e) {
			signupEnabled = prev;
			error = e.message ?? String(e);
		} finally {
			accessBusy = false;
		}
	}
	async function setGuest(on) {
		if (accessBusy || on === guestEnabled) return;
		accessBusy = true;
		const prev = guestEnabled;
		guestEnabled = on;
		try {
			await usersApi.putAccess(signupEnabled, on);
		} catch (e) {
			guestEnabled = prev;
			error = e.message ?? String(e);
		} finally {
			accessBusy = false;
		}
	}

	let kindRows = $state([]);
	let hiddenKinds = $state({ user: [], guest: [] });
	let visLoaded = $state(false);
	let visBusy = $state(false);
	async function loadVisibility() {
		try {
			const [ks, acc] = await Promise.all([kindsApi.list(), usersApi.getKindAccess()]);
			kindRows = ks;
			hiddenKinds = acc;
			visLoaded = true;
		} catch (e) {
			error = e.message ?? String(e);
		}
	}
	const isVisible = (kind, audience) => !hiddenKinds[audience]?.includes(kind);
	async function toggleVisibility(kind, audience) {
		if (visBusy) return;
		visBusy = true;
		const prev = hiddenKinds;
		const set = new Set(hiddenKinds[audience] ?? []);
		set.has(kind) ? set.delete(kind) : set.add(kind);
		hiddenKinds = { ...hiddenKinds, [audience]: [...set] };
		try {
			await usersApi.putKindAccess(hiddenKinds);
		} catch (e) {
			hiddenKinds = prev;
			error = e.message ?? String(e);
		} finally {
			visBusy = false;
		}
	}

	onMount(() => {
		loadAccess();
		loadVisibility();
	});
</script>

<div class="userspanel">
	<section class="block">
		<div class="blockhead">
			<h2>Library visibility</h2>
			<p class="desc">Hide content from users and guests. Admins always see everything.</p>
		</div>
		{#if visLoaded && kindRows.length}
			<div class="card vismatrix" role="grid" aria-label="Kind visibility">
				<div class="visrow vishead" role="row">
					<span></span>
					<span>Users</span>
					<span>Guests</span>
				</div>
				{#each kindRows as k (k.kind)}
					<div class="visrow" role="row">
						<span class="viskind">
							{kindLabel(k.kind)}
							<span class="viscount">{k.count}</span>
						</span>
						{#each ['user', 'guest'] as aud (aud)}
							<button
								class="vischeck"
								class:on={isVisible(k.kind, aud)}
								disabled={visBusy}
								onclick={() => toggleVisibility(k.kind, aud)}
								aria-pressed={isVisible(k.kind, aud)}
								title={isVisible(k.kind, aud)
									? `Visible to ${aud}s — click to hide`
									: `Hidden from ${aud}s — click to show`}
								type="button"
							>
								<svg
									viewBox="0 0 20 20"
									fill="none"
									stroke="currentColor"
									stroke-width="2.5"
									stroke-linecap="round"
									stroke-linejoin="round"
								>
									{#if isVisible(k.kind, aud)}
										<path d="M4 10l4 4 8-8" />
									{:else}
										<path d="M5 5l10 10M15 5L5 15" />
									{/if}
								</svg>
							</button>
						{/each}
					</div>
				{/each}
			</div>
		{:else if visLoaded}
			<p class="desc">No library sections yet — add some items first.</p>
		{/if}
	</section>

	<section class="block">
		<div class="blockhead">
			<h2>Access</h2>
		</div>
		<div class="card">
			<div class="accessrow">
				<div class="accessmeta">
					<span class="accesslabel">Allow sign-ups</span>
					<span class="accessdesc">Anyone can create their own account from the sign-in page.</span>
				</div>
				<div class="seg" role="group" aria-label="Allow sign-ups">
					<button
						class="segbtn"
						class:on={signupEnabled}
						disabled={!accessLoaded || accessBusy}
						onclick={() => setSignup(true)}
						type="button">On</button
					>
					<button
						class="segbtn"
						class:on={!signupEnabled}
						disabled={!accessLoaded || accessBusy}
						onclick={() => setSignup(false)}
						type="button">Off</button
					>
				</div>
			</div>
			<div class="accessrow">
				<div class="accessmeta">
					<span class="accesslabel">Guest browsing</span>
					<span class="accessdesc">Anyone can browse and read without an account.</span>
				</div>
				<div class="seg" role="group" aria-label="Guest browsing">
					<button
						class="segbtn"
						class:on={guestEnabled}
						disabled={!accessLoaded || accessBusy}
						onclick={() => setGuest(true)}
						type="button">On</button
					>
					<button
						class="segbtn"
						class:on={!guestEnabled}
						disabled={!accessLoaded || accessBusy}
						onclick={() => setGuest(false)}
						type="button">Off</button
					>
				</div>
			</div>
		</div>
	</section>

	{#if error}<p class="error">{error}</p>{/if}
</div>

<style>
	.userspanel {
		display: grid;
		grid-template-columns: minmax(0, 1fr) minmax(0, 1fr);
		gap: var(--space-6) var(--space-8);
		align-items: start;
	}
	@media (max-width: 900px) {
		.userspanel {
			grid-template-columns: 1fr;
			max-width: 720px;
		}
	}
	.block {
		display: flex;
		flex-direction: column;
		gap: var(--space-3);
	}
	.card {
		border: 1px solid var(--border);
		border-radius: var(--radius);
		background: var(--surface);
		overflow: hidden;
	}
	.blockhead h2 {
		margin: 0 0 2px;
		font-size: 1rem;
	}
	.desc {
		margin: 0;
		font-size: 0.82rem;
		color: var(--muted);
		max-width: 60ch;
	}
	.error {
		margin: 0;
		font-size: 0.85rem;
		color: var(--bad, #e5484d);
	}

	.accessrow {
		display: flex;
		align-items: center;
		gap: var(--space-4);
		justify-content: space-between;
		padding: var(--space-4);
	}
	.accessrow + .accessrow {
		border-top: 1px solid var(--border);
	}
	.accessmeta {
		display: flex;
		flex-direction: column;
		gap: 2px;
		min-width: 0;
	}
	.accesslabel {
		font-weight: 600;
		font-size: 0.92rem;
	}
	.accessdesc {
		font-size: 0.8rem;
		color: var(--muted);
		max-width: 55ch;
	}

	.vismatrix {
		display: flex;
		flex-direction: column;
	}
	.visrow {
		display: grid;
		grid-template-columns: 1fr 72px 72px;
		align-items: center;
		gap: var(--space-3);
		padding: var(--space-3) var(--space-4);
	}
	.visrow:not(.vishead) {
		border-top: 1px solid var(--border);
	}
	.visrow:not(.vishead):hover {
		background: var(--surface-2);
	}
	.vishead {
		font-size: 0.72rem;
		letter-spacing: 0.08em;
		text-transform: uppercase;
		color: var(--muted);
		padding-top: var(--space-3);
		padding-bottom: var(--space-2);
	}
	.vishead span {
		text-align: center;
	}
	.vishead span:first-child {
		text-align: left;
	}
	.viskind {
		font-size: 0.92rem;
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.viscount {
		margin-left: var(--space-2);
		font-size: 0.75rem;
		color: var(--muted);
		font-variant-numeric: tabular-nums;
	}
	.vischeck {
		all: unset;
		box-sizing: border-box;
		justify-self: center;
		display: inline-flex;
		align-items: center;
		justify-content: center;
		width: 1.7rem;
		height: 1.7rem;
		border: 1px solid var(--border);
		border-radius: var(--radius-sm);
		color: var(--muted);
		cursor: pointer;
		transition:
			background var(--ease),
			color var(--ease),
			border-color var(--ease);
	}
	.vischeck svg {
		width: 0.95rem;
		height: 0.95rem;
	}
	.vischeck.on {
		background: var(--accent-soft);
		border-color: var(--accent);
		color: var(--accent);
	}
	.vischeck:not(.on) {
		opacity: 0.55;
	}
	.vischeck:disabled {
		cursor: default;
		opacity: 0.4;
	}
	.seg {
		display: inline-flex;
		border: 1px solid var(--border);
		border-radius: var(--radius);
		overflow: hidden;
		flex: 0 0 auto;
	}
	.segbtn {
		all: unset;
		box-sizing: border-box;
		padding: 0.35rem 0.9rem;
		font-size: 0.85rem;
		color: var(--muted);
		cursor: pointer;
		border-right: 1px solid var(--border);
	}
	.segbtn:last-child {
		border-right: none;
	}
	.segbtn.on {
		background: var(--accent);
		color: #fff;
		font-weight: 600;
	}
	.segbtn:disabled {
		opacity: 0.6;
		cursor: default;
	}
</style>
