<script>
	import { onMount } from 'svelte';
	import { users as usersApi, ApiError } from '$lib/api.js';
	import { currentUser } from '$lib/session.js';
	import DeleteConfirm from '$lib/components/ui/DeleteConfirm.svelte';
	import Dropdown from '$lib/components/ui/Dropdown.svelte';
	import { relativeTime } from '$lib/format.js';

	const me = $derived($currentUser);
	const PAGE_SIZE = 50;
	const ROLE_OPTIONS = [
		{ value: 'user', label: 'User' },
		{ value: 'admin', label: 'Admin' },
	];
	const SORT_OPTIONS = [
		{ value: 'joined:desc', label: 'Newest' },
		{ value: 'joined:asc', label: 'Oldest' },
		{ value: 'name:asc', label: 'Name A–Z' },
		{ value: 'name:desc', label: 'Name Z–A' },
	];

	let list = $state([]);
	let stats = $state(null);
	let error = $state(null);
	let loading = $state(true);

	let search = $state('');
	let roleFilter = $state('all');
	let sortValue = $state('joined:desc');
	const sortKey = $derived(sortValue.split(':')[0]);
	const sortDir = $derived(sortValue.split(':')[1]);
	let page = $state(1);

	let selected = $state(new Set());
	const selectedCount = $derived(selected.size);

	let showAdd = $state(false);
	let newName = $state('');
	let newPass = $state('');
	let newRole = $state('user');
	let addBusy = $state(false);
	let addError = $state(null);

	let deleteTarget = $state(null);
	let deleteBusy = $state(false);

	async function load() {
		loading = true;
		try {
			const [u, s] = await Promise.all([usersApi.list(), usersApi.stats().catch(() => null)]);
			list = u;
			stats = s;
			error = null;
		} catch (e) {
			if (!(e instanceof ApiError && e.status === 401)) error = e?.message ?? String(e);
		} finally {
			loading = false;
		}
	}
	onMount(load);

	function fmtJoined(secs) {
		if (!secs) return '';
		return new Date(secs * 1000).toLocaleDateString(undefined, { month: 'short', year: 'numeric' });
	}
	const ago = (ts) => (ts ? relativeTime(ts) : 'never');
	function dotClass(ts) {
		if (!ts) return 'grey';
		const age = Date.now() / 1000 - ts;
		if (age < 86400) return 'green';
		if (age < 30 * 86400) return 'amber';
		return 'grey';
	}
	const pct = (n, d) => (d > 0 ? Math.round((n / d) * 100) : 0);

	const total = $derived(list.length);
	const thisMonth = $derived.by(() => {
		const d = new Date();
		const start = Date.UTC(d.getUTCFullYear(), d.getUTCMonth(), 1) / 1000;
		return list.filter((u) => (u.created_at ?? 0) >= start).length;
	});
	const roles = $derived([...new Set(list.map((u) => u.role))]);

	const filtered = $derived.by(() => {
		const q = search.trim().toLowerCase();
		let rows = list.filter(
			(u) =>
				(roleFilter === 'all' || u.role === roleFilter) &&
				(!q || u.username.toLowerCase().includes(q)),
		);
		rows = [...rows].sort((a, b) => {
			const dir = sortDir === 'asc' ? 1 : -1;
			if (sortKey === 'name') return dir * a.username.localeCompare(b.username);
			return dir * ((a.created_at ?? 0) - (b.created_at ?? 0));
		});
		return rows;
	});
	const pageCount = $derived(Math.max(1, Math.ceil(filtered.length / PAGE_SIZE)));
	const pageRows = $derived(filtered.slice((page - 1) * PAGE_SIZE, page * PAGE_SIZE));
	$effect(() => {
		if (page > pageCount) page = pageCount;
	});

	function toggleSort(key) {
		if (sortKey === key) sortValue = `${key}:${sortDir === 'asc' ? 'desc' : 'asc'}`;
		else sortValue = key === 'name' ? 'name:asc' : 'joined:desc';
	}
	const roleOptions = $derived([
		{ value: 'all', label: 'All roles' },
		...roles.map((r) => ({ value: r, label: r })),
	]);

	const selectable = $derived(pageRows.filter((u) => u.id !== me?.id));
	function toggleOne(id) {
		if (id === me?.id) return;
		const s = new Set(selected);
		s.has(id) ? s.delete(id) : s.add(id);
		selected = s;
	}
	const allOnPageSelected = $derived(
		selectable.length > 0 && selectable.every((u) => selected.has(u.id)),
	);
	function toggleAllOnPage() {
		const s = new Set(selected);
		if (allOnPageSelected) selectable.forEach((u) => s.delete(u.id));
		else selectable.forEach((u) => s.add(u.id));
		selected = s;
	}
	function clearSelection() {
		selected = new Set();
	}

	async function submitAdd(e) {
		e.preventDefault();
		if (addBusy || !newName.trim() || !newPass) return;
		addBusy = true;
		addError = null;
		try {
			await usersApi.create(newName.trim(), newPass, newRole);
			newName = newPass = '';
			newRole = 'user';
			showAdd = false;
			await load();
		} catch (err) {
			addError = err?.message ?? String(err);
		} finally {
			addBusy = false;
		}
	}

	function askDelete(ids) {
		const admins = list.filter((u) => u.role === 'admin').length;
		const safe = ids.filter(
			(id) => id !== me?.id && !(list.find((u) => u.id === id)?.role === 'admin' && admins <= 1),
		);
		if (!safe.length) return;
		const label =
			safe.length === 1 ? list.find((u) => u.id === safe[0])?.username : `${safe.length} accounts`;
		deleteTarget = { ids: safe, label };
	}
	async function confirmDelete() {
		if (!deleteTarget || deleteBusy) return;
		deleteBusy = true;
		try {
			for (const id of deleteTarget.ids) await usersApi.remove(id);
			clearSelection();
			deleteTarget = null;
			await load();
		} catch (e) {
			error = e?.message ?? String(e);
			deleteTarget = null;
		} finally {
			deleteBusy = false;
		}
	}
</script>

<div class="accounts">
	<section class="tiles">
		<div class="tile">
			<p class="k">Accounts</p>
			<p class="v">{loading ? '—' : total.toLocaleString()}</p>
			<p class="sub">
				{loading ? ' ' : thisMonth > 0 ? `+${thisMonth} this month` : 'none this month'}
			</p>
		</div>
		<div class="tile">
			<p class="k">Active today</p>
			<p class="v">{stats ? stats.active_today.toLocaleString() : '—'}</p>
			<p class="sub">{stats ? `${stats.active_week.toLocaleString()} this week` : ' '}</p>
		</div>
		<div class="tile">
			<p class="k">Dormant 90 d+</p>
			<p class="v">{stats ? stats.dormant_90.toLocaleString() : '—'}</p>
			<p class="sub">{stats && total ? `${pct(stats.dormant_90, total)}% of accounts` : ' '}</p>
		</div>
		<div class="tile">
			<p class="k">Open sessions</p>
			<p class="v">{stats ? stats.open_sessions.toLocaleString() : '—'}</p>
			<p class="sub">{stats ? `${stats.sign_ins_today.toLocaleString()} sign-ins today` : ' '}</p>
		</div>
	</section>

	<div class="toolbar">
		<div class="searchwrap">
			<svg
				viewBox="0 0 24 24"
				fill="none"
				stroke="currentColor"
				stroke-width="2"
				stroke-linecap="round"><circle cx="11" cy="11" r="7" /><path d="M21 21l-4.3-4.3" /></svg
			>
			<input class="search" type="text" placeholder="Search username" bind:value={search} />
		</div>
		<div class="ddctl">
			<span class="ddlabel">Role</span><Dropdown bind:value={roleFilter} options={roleOptions} />
		</div>
		<div class="ddctl">
			<span class="ddlabel">Sort</span><Dropdown bind:value={sortValue} options={SORT_OPTIONS} />
		</div>
		<button class="pill add" type="button" onclick={() => (showAdd = !showAdd)}
			>+ Add account</button
		>
	</div>

	{#if showAdd}
		<form class="addform" onsubmit={submitAdd}>
			<input type="text" placeholder="Username" bind:value={newName} autocomplete="off" />
			<input
				type="password"
				placeholder="Password"
				bind:value={newPass}
				autocomplete="new-password"
			/>
			<div class="roledd"><Dropdown bind:value={newRole} options={ROLE_OPTIONS} /></div>
			<button class="primary" type="submit" disabled={addBusy || !newName.trim() || !newPass}>
				{addBusy ? 'Creating…' : 'Create'}
			</button>
			<button type="button" onclick={() => (showAdd = false)}>Cancel</button>
			{#if addError}<span class="err">{addError}</span>{/if}
		</form>
	{/if}

	{#if selectedCount > 0}
		<div class="bulkbar">
			<span class="bcount">{selectedCount} selected</span>
			<button class="baction danger" type="button" onclick={() => askDelete([...selected])}
				>Delete</button
			>
			<button class="baction clear" type="button" onclick={clearSelection}>✕ Clear</button>
		</div>
	{/if}

	{#if error}
		<p class="err">{error}</p>
	{:else}
		<div class="tablewrap">
			<table>
				<thead>
					<tr>
						<th class="cbcol"
							><input
								type="checkbox"
								checked={allOnPageSelected}
								disabled={loading}
								onchange={toggleAllOnPage}
								aria-label="Select all on page"
							/></th
						>
						<th class="usercol"
							><button class="sortbtn" onclick={() => toggleSort('name')}
								>User{sortKey === 'name' ? (sortDir === 'asc' ? ' ▲' : ' ▼') : ''}</button
							></th
						>
						<th>Last read activity</th>
						<th class="num">Finished</th>
						<th class="num">Favorites</th>
						<th class="num">Sessions</th>
						<th class="joincol"
							><button class="sortbtn" onclick={() => toggleSort('joined')}
								>Joined{sortKey === 'joined' ? (sortDir === 'asc' ? ' ▲' : ' ▼') : ''}</button
							></th
						>
					</tr>
				</thead>
				<tbody>
					{#if loading}
						{#each Array.from({ length: 8 }) as _, i (i)}
							<tr class="skelrow" aria-hidden="true">
								<td class="cbcol"><span class="sk sk-box"></span></td>
								<td class="usercol"
									><div class="userc">
										<span class="sk sk-av"></span><span class="sk sk-name"></span>
									</div></td
								>
								<td><span class="sk sk-line"></span></td>
								<td class="num"><span class="sk sk-num"></span></td>
								<td class="num"><span class="sk sk-num"></span></td>
								<td class="num"><span class="sk sk-num"></span></td>
								<td class="joincol"><span class="sk sk-line"></span></td>
							</tr>
						{/each}
					{:else}
						{#each pageRows as u (u.id)}
							<tr class:sel={selected.has(u.id)}>
								<td class="cbcol">
									{#if u.id !== me?.id}
										<input
											type="checkbox"
											checked={selected.has(u.id)}
											onchange={() => toggleOne(u.id)}
											aria-label={`Select ${u.username}`}
										/>
									{/if}
								</td>
								<td class="usercol">
									<div class="userc">
										{#if u.avatar_version}
											<img class="uav" src={usersApi.avatarUrl(u.id, u.avatar_version)} alt="" />
										{:else}
											<span class="uav" aria-hidden="true"
												>{u.username.slice(0, 1).toUpperCase()}</span
											>
										{/if}
										<span class="uname">{u.username}</span>
										{#if u.role !== 'user'}<span class="rolebadge">{u.role}</span>{/if}
										{#if u.id === me?.id}<span class="rolebadge you">you</span>{/if}
									</div>
								</td>
								<td><span class="dot {dotClass(u.last_active)}"></span>{ago(u.last_active)}</td>
								<td class="num">{(u.finished ?? 0).toLocaleString()}</td>
								<td class="num">{(u.favorites ?? 0).toLocaleString()}</td>
								<td class="num">{u.sessions ?? 0}</td>
								<td class="joincol">{fmtJoined(u.created_at)}</td>
							</tr>
						{/each}
						{#if !pageRows.length}
							<tr><td colspan="7" class="muted empty">No accounts match.</td></tr>
						{/if}
					{/if}
				</tbody>
			</table>
		</div>

		<div class="pager">
			{#if loading}
				<span class="muted">Loading accounts…</span>
			{:else}
				<span class="muted">
					{filtered.length ? (page - 1) * PAGE_SIZE + 1 : 0}–{Math.min(
						page * PAGE_SIZE,
						filtered.length,
					)} of
					{filtered.length} matching
				</span>
				<div class="pagebtns">
					<button
						type="button"
						disabled={page <= 1}
						onclick={() => (page = Math.max(1, page - 1))}
						aria-label="Previous page">‹</button
					>
					<span>Page {page} of {pageCount}</span>
					<button
						type="button"
						disabled={page >= pageCount}
						onclick={() => (page = Math.min(pageCount, page + 1))}
						aria-label="Next page">›</button
					>
				</div>
			{/if}
		</div>
	{/if}
</div>

{#if deleteTarget}
	<DeleteConfirm
		heading={`Delete ${deleteTarget.label}?`}
		message="Their favorites, progress, ratings, and API keys are removed. This cannot be undone."
		verb="Delete"
		busyLabel="Deleting…"
		busy={deleteBusy}
		onConfirm={confirmDelete}
		onClose={() => (deleteTarget = null)}
	/>
{/if}

<style>
	.accounts {
		display: flex;
		flex-direction: column;
		gap: var(--space-4);
	}
	.muted {
		color: var(--muted);
	}
	.err {
		color: var(--bad, #e5484d);
		font-size: 0.85rem;
	}

	.tiles {
		display: grid;
		grid-template-columns: repeat(4, 1fr);
		gap: var(--space-3);
	}
	.tile {
		background: var(--surface);
		border: 1px solid var(--border);
		border-radius: var(--radius);
		padding: var(--space-3) var(--space-4);
	}
	.tile .k {
		margin: 0;
		font-size: 0.65rem;
		text-transform: uppercase;
		letter-spacing: 0.1em;
		color: var(--muted);
	}
	.tile .v {
		margin: 0.3rem 0 0.15rem;
		font-size: 1.7rem;
		font-weight: 700;
		line-height: 1;
	}
	.tile .sub {
		margin: 0;
		font-size: 0.75rem;
		color: var(--muted);
	}

	.toolbar {
		display: flex;
		align-items: center;
		gap: var(--space-2);
		flex-wrap: wrap;
	}
	.searchwrap {
		position: relative;
		flex: 1 1 240px;
		display: flex;
		align-items: center;
	}
	.searchwrap svg {
		position: absolute;
		left: 0.7rem;
		width: 1rem;
		height: 1rem;
		color: var(--muted);
		pointer-events: none;
	}
	.search {
		width: 100%;
		padding-left: 2.1rem;
	}
	.pill {
		display: inline-flex;
		align-items: center;
		gap: 0.35rem;
		padding: 0.4rem 0.75rem;
		border: 1px solid var(--border);
		border-radius: 9999px;
		background: var(--surface);
		color: var(--text);
		font-size: 0.82rem;
		cursor: pointer;
		white-space: nowrap;
	}
	.pill.add {
		border-color: var(--accent);
		color: var(--accent);
	}
	.ddctl {
		display: flex;
		align-items: center;
		gap: var(--space-2);
	}
	.ddlabel {
		font-size: 0.78rem;
		color: var(--muted);
	}
	.ddctl :global(.dd) {
		min-width: 8rem;
	}

	.addform {
		display: flex;
		gap: var(--space-2);
		flex-wrap: wrap;
		align-items: center;
		padding: var(--space-3);
		border: 1px solid var(--border);
		border-radius: var(--radius);
		background: var(--surface);
	}
	.addform input {
		min-width: 0;
	}
	.roledd :global(.dd) {
		min-width: 7rem;
	}
	.addform .primary {
		background: var(--accent);
		border-color: var(--accent);
		color: #fff;
		font-weight: 600;
	}

	.bulkbar {
		display: flex;
		align-items: center;
		gap: var(--space-4);
		padding: 0.5rem var(--space-3);
		background: color-mix(in srgb, var(--accent) 10%, transparent);
		border: 1px solid color-mix(in srgb, var(--accent) 30%, transparent);
		border-radius: var(--radius);
		flex-wrap: wrap;
	}
	.bcount {
		color: var(--accent);
		font-weight: 600;
		font-size: 0.85rem;
	}
	.baction {
		all: unset;
		cursor: pointer;
		font-size: 0.85rem;
		color: var(--text);
	}
	.baction:hover {
		text-decoration: underline;
	}
	.baction.danger {
		color: var(--bad, #e5484d);
	}
	.baction.clear {
		margin-left: auto;
		color: var(--muted);
	}

	.tablewrap {
		overflow-x: auto;
	}
	table {
		width: 100%;
		border-collapse: collapse;
		font-size: 0.88rem;
	}
	thead th {
		text-align: left;
		padding: 0.5rem 0.6rem;
		font-size: 0.68rem;
		text-transform: uppercase;
		letter-spacing: 0.08em;
		color: var(--muted);
		border-bottom: 1px solid var(--border);
		font-weight: 600;
		white-space: nowrap;
	}
	th.num,
	td.num {
		text-align: right;
	}
	.sortbtn {
		all: unset;
		cursor: pointer;
		text-transform: inherit;
		letter-spacing: inherit;
		color: inherit;
		font: inherit;
	}
	.sortbtn:hover {
		color: var(--text);
	}
	tbody td {
		padding: 0.5rem 0.6rem;
		border-bottom: 1px solid var(--border);
		vertical-align: middle;
	}
	tbody tr.sel {
		background: color-mix(in srgb, var(--accent) 8%, transparent);
	}
	.dot {
		display: inline-block;
		width: 8px;
		height: 8px;
		border-radius: 50%;
		background: var(--border);
		margin-right: 0.4rem;
		vertical-align: middle;
	}

	.sk {
		display: inline-block;
		height: 0.8rem;
		border-radius: 4px;
		background: color-mix(in srgb, var(--text) 9%, transparent);
		animation: skpulse 1.2s ease-in-out infinite;
	}
	.sk-box {
		width: 14px;
		height: 14px;
	}
	.sk-av {
		width: 26px;
		height: 26px;
		border-radius: 50%;
		flex: 0 0 auto;
	}
	.sk-name {
		width: 45%;
	}
	.sk-line {
		width: 60%;
	}
	.sk-num {
		width: 2rem;
	}
	@keyframes skpulse {
		0%,
		100% {
			opacity: 1;
		}
		50% {
			opacity: 0.45;
		}
	}
	.dot.green {
		background: var(--good, #46a758);
	}
	.dot.amber {
		background: #d29922;
	}
	.dot.grey {
		background: var(--muted);
	}
	.userc {
		display: flex;
		align-items: center;
		gap: 0.6rem;
		min-width: 0;
	}
	.uav {
		width: 26px;
		height: 26px;
		flex: 0 0 auto;
		border-radius: 50%;
		object-fit: cover;
		display: inline-flex;
		align-items: center;
		justify-content: center;
		background: var(--accent);
		color: #fff;
		font-size: 0.7rem;
		font-weight: 700;
	}
	.uname {
		font-weight: 600;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.rolebadge {
		flex: 0 0 auto;
		padding: 1px 7px;
		border: 1px solid var(--border);
		border-radius: 9999px;
		font-size: 0.6rem;
		letter-spacing: 0.05em;
		text-transform: uppercase;
		color: var(--muted);
	}
	.rolebadge.you {
		color: var(--accent);
		border-color: color-mix(in srgb, var(--accent) 50%, var(--border));
	}
	.empty {
		text-align: center;
		padding: var(--space-5);
	}

	.pager {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--space-3);
		font-size: 0.82rem;
		flex-wrap: wrap;
	}
	.pagebtns {
		display: flex;
		align-items: center;
		gap: var(--space-3);
	}
	.pagebtns button {
		padding: 0.15rem 0.6rem;
	}

	@media (max-width: 780px) {
		.tiles {
			grid-template-columns: repeat(2, 1fr);
		}
	}
	@media (max-width: 560px) {
		.toolbar {
			display: grid;
			grid-template-columns: 1fr 1fr;
			align-items: center;
		}
		.searchwrap {
			grid-column: 1 / -1;
		}
		.ddctl {
			min-width: 0;
		}
		.ddctl :global(.dd) {
			flex: 1;
			min-width: 0;
		}
		.pill.add {
			grid-column: 1 / -1;
			justify-content: center;
		}
	}
</style>
