<script>
	import { onMount } from 'svelte';
	import { upcoming as upcomingApi, media } from '$lib/api.js';
	import { currentUser } from '$lib/session.js';
	import ScheduleFooter from '$lib/components/ScheduleFooter.svelte';

	let { embedded = false } = $props();

	const DAY = 86_400_000;

	let data = $state({
		window_days: 180,
		generated_at: null,
		next_refresh_at: null,
		sources: [],
		releases: [],
	});
	let view = $state('list');
	let calendarMonth = $state(new Date(new Date().getFullYear(), new Date().getMonth(), 1));
	let selectedRelease = $state(null);
	let filtersOpen = $state(false);
	let selectedFormats = $state(new Set());
	let selectedSources = $state(new Set());
	let refreshedAt = $state(null);
	let loading = $state(true);
	let error = $state(null);
	let refreshing = $state(false);
	const isAdmin = $derived($currentUser?.role === 'admin');

	onMount(load);
	async function load() {
		loading = true;
		try {
			data = await upcomingApi.list();
			selectedSources = new Set(data.sources.map((source) => source.id));
			refreshedAt =
				data.sources.reduce((latest, source) => Math.max(latest, source.last_success_at ?? 0), 0) ||
				data.generated_at;
			error = null;
		} catch (e) {
			error = e?.message ?? String(e);
		} finally {
			loading = false;
		}
	}

	async function refresh() {
		refreshing = true;
		try {
			await upcomingApi.refresh();
		} catch (e) {
			error = e?.message ?? String(e);
		} finally {
			refreshing = false;
		}
	}

	const dateFmt = new Intl.DateTimeFormat(undefined, {
		month: 'short',
		day: 'numeric',
		year: 'numeric',
	});
	const rangeFmt = new Intl.DateTimeFormat(undefined, { month: 'short', day: 'numeric' });
	const weekdayFmt = new Intl.DateTimeFormat(undefined, { weekday: 'short' });
	function dateOnly(value) {
		const [year, month, day] = value.split('-').map(Number);
		return new Date(year, month - 1, day);
	}
	function dateKey(value) {
		const year = value.getFullYear();
		const month = String(value.getMonth() + 1).padStart(2, '0');
		const day = String(value.getDate()).padStart(2, '0');
		return `${year}-${month}-${day}`;
	}

	const shown = $derived(
		data.releases.filter(
			(release) =>
				selectedSources.has(release.source) &&
				(!selectedFormats.size || release.formats.some((format) => selectedFormats.has(format))),
		),
	);

	const groups = $derived.by(() => {
		const now = new Date();
		const today = new Date(now.getFullYear(), now.getMonth(), now.getDate());
		const endThisWeek = new Date(today);
		endThisWeek.setDate(today.getDate() + ((7 - today.getDay()) % 7));
		const endNextWeek = new Date(endThisWeek);
		endNextWeek.setDate(endThisWeek.getDate() + 7);
		const startWindow = new Date(today.getTime() - 7 * DAY);
		const buckets = [
			{
				id: 'past',
				label: 'Recently Released',
				from: startWindow,
				to: new Date(today.getTime() - DAY),
				releases: [],
			},
			{ id: 'today', label: 'Today', from: today, to: today, releases: [] },
			{
				id: 'week',
				label: 'This Week',
				from: new Date(today.getTime() + DAY),
				to: endThisWeek,
				releases: [],
			},
			{
				id: 'next',
				label: 'Next Week',
				from: new Date(endThisWeek.getTime() + DAY),
				to: endNextWeek,
				releases: [],
			},
			{
				id: 'later',
				label: 'Later',
				from: new Date(endNextWeek.getTime() + DAY),
				to: null,
				releases: [],
			},
		];
		for (const release of shown) {
			const date = dateOnly(release.date);
			const bucket = buckets.find(
				(group) =>
					date >= group.from &&
					(!group.to ||
						date <=
							new Date(
								group.to.getFullYear(),
								group.to.getMonth(),
								group.to.getDate(),
								23,
								59,
								59,
							)),
			);
			(bucket ?? buckets.at(-1)).releases.push(release);
		}
		return buckets.filter((group) => group.releases.length);
	});

	const calendarTitle = $derived(
		new Intl.DateTimeFormat(undefined, { month: 'long', year: 'numeric' }).format(calendarMonth),
	);
	const calendarDays = $derived.by(() => {
		const first = new Date(calendarMonth.getFullYear(), calendarMonth.getMonth(), 1);
		const start = new Date(first);
		start.setDate(1 - first.getDay());
		return Array.from({ length: 42 }, (_, index) => {
			const date = new Date(start);
			date.setDate(start.getDate() + index);
			const key = dateKey(date);
			return {
				key,
				date,
				outside: date.getMonth() !== calendarMonth.getMonth(),
				today: key === dateKey(new Date()),
				releases: shown.filter((release) => release.date === key),
			};
		});
	});

	function moveMonth(offset) {
		calendarMonth = new Date(calendarMonth.getFullYear(), calendarMonth.getMonth() + offset, 1);
		selectedRelease = null;
	}

	function goToToday() {
		const now = new Date();
		calendarMonth = new Date(now.getFullYear(), now.getMonth(), 1);
		selectedRelease = null;
	}

	function toggleFormat(format) {
		const next = new Set(selectedFormats);
		next.has(format) ? next.delete(format) : next.add(format);
		selectedFormats = next;
	}

	function toggleSource(id) {
		const next = new Set(selectedSources);
		next.has(id) ? next.delete(id) : next.add(id);
		selectedSources = next;
	}

	function groupRange(group) {
		if (group.id === 'today')
			return `${weekdayFmt.format(group.from)}, ${dateFmt.format(group.from)}`;
		if (!group.to) return `After ${rangeFmt.format(new Date(group.from.getTime() - DAY))}`;
		return `${rangeFmt.format(group.from)} – ${rangeFmt.format(group.to)}`;
	}

	const statusLabel = (status) =>
		status === 'error'
			? 'Error'
			: status === 'pending'
				? 'Pending'
				: status === 'inactive'
					? 'Idle'
					: 'Updated';
	const monogram = (name) =>
		name
			.split(/\s+/)
			.map((word) => word[0])
			.join('')
			.slice(0, 2)
			.toUpperCase();
	const sourceColor = (id) => ({ viz: '#e31b23', anilist: '#4f9ee8' })[id] ?? '#747474';
	const initials = (title) =>
		title
			.split(/\s+/)
			.map((word) => word[0])
			.join('')
			.slice(0, 2)
			.toUpperCase();
</script>

<div class="upcoming-page" class:embedded>
	<header class="hero" class:embedded>
		{#if !embedded}
			<div class="hero-title">
				<svg viewBox="0 0 24 24" aria-hidden="true"
					><path d="M7 3v3m10-3v3M4 9h16M5 5h14a1 1 0 0 1 1 1v14H4V6a1 1 0 0 1 1-1Z" /></svg
				>
				<div>
					<h1>Upcoming Releases</h1>
					<p>
						Releases from your linked sources in the next <strong>{data.window_days} days</strong>
					</p>
				</div>
			</div>
		{/if}
		<div class="toolbar">
			<button
				class:active={filtersOpen || selectedFormats.size}
				onclick={() => (filtersOpen = !filtersOpen)}
				type="button"
			>
				<svg viewBox="0 0 24 24"><path d="M4 5h16l-6 7v6l-4 2v-8Z" /></svg> Filter
				{#if selectedFormats.size}<span class="count">{selectedFormats.size}</span>{/if}
			</button>
			<div class="view-toggle" aria-label="View style">
				<button
					class:active={view === 'list'}
					onclick={() => (view = 'list')}
					aria-label="List view"
					type="button"
					><svg viewBox="0 0 24 24"
						><path d="M8 6h12M8 12h12M8 18h12M4 6h.01M4 12h.01M4 18h.01" /></svg
					></button
				>
				<button
					class:active={view === 'calendar'}
					onclick={() => (view = 'calendar')}
					aria-label="Calendar view"
					type="button"
					><svg viewBox="0 0 24 24"
						><path d="M7 3v3m10-3v3M4 9h16M5 5h14a1 1 0 0 1 1 1v14H4V6a1 1 0 0 1 1-1Z" /></svg
					></button
				>
			</div>
		</div>
	</header>

	{#if filtersOpen}
		<div class="filters">
			<span>Format</span>
			{#each ['Print', 'Digital'] as format (format)}
				<button
					class:active={selectedFormats.has(format)}
					onclick={() => toggleFormat(format)}
					type="button">{format}</button
				>
			{/each}
			{#if selectedFormats.size}<button
					class="clear"
					onclick={() => (selectedFormats = new Set())}
					type="button">Clear</button
				>{/if}
		</div>
	{/if}

	<div class="layout">
		<main class="release-list" class:calendar-mode={view === 'calendar'}>
			{#if loading}
				<div class="empty"><h2>Loading releases…</h2></div>
			{:else if error}
				<div class="empty">
					<h2>Couldn’t load releases</h2>
					<p>{error}</p>
				</div>
			{:else if !shown.length}
				<div class="empty">
					<h2>No matching releases</h2>
					<p>Enable another source or clear the format filter.</p>
				</div>
			{/if}
			{#if !loading && !error && shown.length && view === 'list'}
				{#each groups as group (group.id)}
					<section class="date-group">
						<header>
							<h2>{group.label}</h2>
							<span>{groupRange(group)}</span>
						</header>
						<div class="rows">
							{#each group.releases as release (release.id)}
								<article class="release">
									<div class="cover" aria-hidden="true">
										{#if release.cover_url}<img
												src={media.pluginImage(release.source, release.cover_url)}
												alt=""
											/>{:else if release.cover_item_id}<img
												src={media.thumbnail(release.cover_item_id)}
												alt=""
											/>{:else}<span>{initials(release.title)}</span>{/if}
									</div>
									<div class="identity">
										<h3>{release.title} <span>{release.label}</span></h3>
										<p>{release.creators.join(', ')}</p>
										<small>{release.publisher}</small>
									</div>
									<div class="chips">
										<span>{release.kind}</span>{#each release.formats as format (format)}<span
												>{format}</span
											>{/each}
									</div>
									<div
										class="release-date"
										class:estimated={release.date_status === 'estimated' ||
											release.date_precision !== 'day'}
									>
										<svg viewBox="0 0 24 24"
											><path
												d="M7 3v3m10-3v3M4 9h16M5 5h14a1 1 0 0 1 1 1v14H4V6a1 1 0 0 1 1-1Z"
											/></svg
										>
										<span>{dateFmt.format(dateOnly(release.date))}</span>
										{#if release.date_status === 'estimated' || release.date_precision !== 'day'}<small
												>estimated</small
											>{/if}
									</div>
									<a
										class="external"
										href={release.url ?? `/series/${release.series_id}`}
										target={release.url ? '_blank' : undefined}
										rel="noreferrer"
										aria-label="Open source page"
									>
										<svg viewBox="0 0 24 24"><path d="M14 4h6v6m0-6-9 9M19 13v7H4V5h7" /></svg>
									</a>
								</article>
							{/each}
						</div>
					</section>
				{/each}
			{:else if !loading && !error && shown.length && view === 'calendar'}
				<div class="calendar-scroll">
					<section class="month-calendar">
						<header class="month-toolbar">
							<button onclick={goToToday} type="button">Today</button>
							<div class="month-nav">
								<button onclick={() => moveMonth(-1)} aria-label="Previous month" type="button"
									><svg viewBox="0 0 24 24"><path d="m15 18-6-6 6-6" /></svg></button
								>
								<button onclick={() => moveMonth(1)} aria-label="Next month" type="button"
									><svg viewBox="0 0 24 24"><path d="m9 18 6-6-6-6" /></svg></button
								>
								<h2>{calendarTitle}</h2>
							</div>
						</header>
						<div class="weekdays">
							{#each ['Sun', 'Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat'] as day (day)}<span
									>{day}</span
								>{/each}
						</div>
						<div class="month-grid">
							{#each calendarDays as day (day.key)}
								<div class="day" class:outside={day.outside} class:today={day.today}>
									<time datetime={day.key}>{day.date.getDate()}</time>
									<div class="day-releases">
										{#each day.releases.slice(0, 3) as release (release.id)}
											<button
												class="calendar-event"
												class:digital-only={!release.formats.includes('Print') &&
													release.formats.includes('Digital')}
												onclick={() => (selectedRelease = release)}
												type="button"
											>
												<span></span><strong>{release.title} {release.label}</strong>
											</button>
										{/each}
										{#if day.releases.length > 3}<button
												class="more-events"
												onclick={() => (selectedRelease = day.releases[3])}
												type="button">+{day.releases.length - 3} more</button
											>{/if}
									</div>
								</div>
							{/each}
						</div>
						{#if selectedRelease}
							<button
								class="calendar-dismiss"
								onclick={() => (selectedRelease = null)}
								aria-label="Close release details"
								type="button"
							></button>
							<section class="calendar-detail" aria-label="Release details">
								<div class="detail-head">
									<div class="detail-cover">
										{#if selectedRelease.cover_url}<img
												src={media.pluginImage(selectedRelease.source, selectedRelease.cover_url)}
												alt=""
											/>{:else if selectedRelease.cover_item_id}<img
												src={media.thumbnail(selectedRelease.cover_item_id)}
												alt=""
											/>{:else}<span>{initials(selectedRelease.title)}</span>{/if}
									</div>
									<div>
										<h3>{selectedRelease.title} {selectedRelease.label}</h3>
										<p>{selectedRelease.creators.join(', ')}</p>
										<small>{selectedRelease.publisher}</small>
									</div>
								</div>
								<div class="detail-chips">
									<span>{selectedRelease.kind}</span
									>{#each selectedRelease.formats as format (format)}<span>{format}</span>{/each}
								</div>
								<p class="detail-date">{dateFmt.format(dateOnly(selectedRelease.date))}</p>
								<div class="detail-actions">
									<a href={`/series/${selectedRelease.series_id}`}>View series</a
									>{#if selectedRelease.url}<a
											href={selectedRelease.url}
											target="_blank"
											rel="noreferrer">Open source</a
										>{/if}
								</div>
							</section>
						{/if}
					</section>
				</div>
			{/if}
		</main>

		<aside class="side">
			<section class="panel sources-card">
				<h2>Calendar Sources <span>{data.sources.length}</span></h2>
				<div class="source-list">
					{#each data.sources as source (source.id)}
						<button
							class="source"
							class:disabled={!selectedSources.has(source.id)}
							onclick={() => toggleSource(source.id)}
							type="button"
						>
							<span class="source-icon" style={`--source-color:${sourceColor(source.id)}`}
								>{monogram(source.name)}<img
									src={media.pluginIcon(source.id)}
									alt=""
									onerror={(e) => (e.currentTarget.style.display = 'none')}
								/></span
							>
							<span class="source-name"
								><strong>{source.name}</strong><small>{source.linked_series} linked series</small
								></span
							>
							<span
								class:limited={source.status === 'limited'}
								class:idle={source.status === 'inactive'}
								class="status"
								>{selectedSources.has(source.id) ? statusLabel(source.status) : 'Hidden'}</span
							>
						</button>
					{/each}
				</div>
				<a class="manage" href="/plugins">Manage sources</a>
			</section>

			<section class="panel legend">
				<h2>Calendar Legend</h2>
				<p><span class="key print"></span> Print release</p>
				<p><span class="key digital"></span> Digital release</p>
				<p><span class="calendar exact"></span> Exact date</p>
				<p><span class="calendar estimate"></span> Estimated date</p>
			</section>

			<section class="panel about">
				<h2>About Upcoming Releases</h2>
				<p>
					Release dates come from linked sources. Dates may change, so check the source page for the
					latest information.
				</p>
			</section>
		</aside>
	</div>

	<ScheduleFooter
		updatedLabel="Calendar updated"
		updatedAt={refreshedAt}
		nextAt={data.next_refresh_at}
		showAction={isAdmin}
		busy={refreshing}
		onaction={refresh}
	/>
</div>

<style>
	.upcoming-page {
		min-height: 100vh;
		padding: var(--space-6) var(--space-6) 0;
	}
	.upcoming-page.embedded {
		min-height: 0;
		padding: 0;
	}
	.hero {
		display: flex;
		align-items: flex-start;
		justify-content: space-between;
		gap: var(--space-5);
		margin-bottom: var(--space-5);
	}
	.hero.embedded {
		justify-content: flex-end;
		margin-bottom: var(--space-4);
	}
	.hero-title {
		display: flex;
		align-items: flex-start;
		gap: var(--space-4);
	}
	.hero-title > svg {
		width: 2rem;
		height: 2rem;
		margin-top: 0.1rem;
		color: var(--muted);
		fill: none;
		stroke: currentColor;
		stroke-width: 1.6;
		stroke-linecap: round;
		stroke-linejoin: round;
	}
	h1 {
		margin: 0 0 0.3rem;
		font-family: var(--font-display);
		font-size: 1.75rem;
		line-height: 1;
	}
	.hero p {
		margin: 0;
		color: var(--muted);
		font-size: 0.88rem;
	}
	.hero p strong {
		color: var(--accent);
		font-weight: 500;
	}
	.toolbar {
		display: flex;
		align-items: center;
		gap: var(--space-3);
	}
	.toolbar > button,
	.view-toggle button {
		display: inline-flex;
		align-items: center;
		gap: var(--space-2);
		min-height: 2.6rem;
	}
	.toolbar svg {
		width: 1.05rem;
		height: 1.05rem;
		fill: none;
		stroke: currentColor;
		stroke-width: 1.8;
		stroke-linecap: round;
		stroke-linejoin: round;
	}
	.toolbar button.active,
	.view-toggle button.active,
	.filters button.active {
		color: var(--accent);
		border-color: color-mix(in srgb, var(--accent) 55%, var(--border));
		background: var(--accent-soft);
	}
	.count {
		display: inline-flex;
		min-width: 1.1rem;
		height: 1.1rem;
		align-items: center;
		justify-content: center;
		border-radius: 99px;
		background: var(--accent);
		color: #fff;
		font-size: 0.65rem;
	}
	.view-toggle {
		display: flex;
	}
	.view-toggle button {
		border-radius: 0;
		padding-inline: 0.75rem;
	}
	.view-toggle button:first-child {
		border-radius: var(--radius) 0 0 var(--radius);
	}
	.view-toggle button:last-child {
		margin-left: -1px;
		border-radius: 0 var(--radius) var(--radius) 0;
	}
	.filters {
		display: flex;
		align-items: center;
		gap: var(--space-2);
		margin: -0.4rem 0 var(--space-5);
		padding: var(--space-3);
		border: 1px solid var(--border);
		border-radius: var(--radius);
		background: var(--surface);
	}
	.filters > span {
		margin-right: var(--space-2);
		color: var(--muted);
		font-size: 0.8rem;
		text-transform: uppercase;
		letter-spacing: 0.1em;
	}
	.filters button {
		padding: 0.35rem 0.75rem;
		font-size: 0.8rem;
	}
	.filters .clear {
		margin-left: auto;
		background: transparent;
		border-color: transparent;
		color: var(--muted);
	}
	.layout {
		display: grid;
		grid-template-columns: minmax(0, 1fr) minmax(14rem, 18rem);
		gap: var(--space-4);
		align-items: start;
	}
	.release-list {
		display: flex;
		flex-direction: column;
		gap: var(--space-3);
		min-width: 0;
	}
	.date-group {
		border: 1px solid var(--border);
		border-radius: var(--radius);
		overflow: hidden;
		background: color-mix(in srgb, var(--surface) 35%, transparent);
	}
	.date-group > header {
		display: flex;
		align-items: baseline;
		gap: var(--space-3);
		padding: 0.75rem 1rem;
		border-bottom: 1px solid var(--border);
	}
	.date-group h2 {
		margin: 0;
		color: var(--accent);
		font-size: 1rem;
		font-weight: 600;
	}
	.date-group header span {
		color: var(--muted);
		font-size: 0.75rem;
	}
	.rows {
		display: flex;
		flex-direction: column;
	}
	.release {
		min-width: 0;
		display: grid;
		grid-template-columns: 3.4rem minmax(10rem, 1.6fr) minmax(8rem, 1fr) auto 2.1rem;
		align-items: center;
		gap: var(--space-3);
		padding: 0.55rem 0.85rem;
		border-bottom: 1px solid var(--border);
	}
	.release:last-child {
		border-bottom: 0;
	}
	.cover {
		position: relative;
		width: 3.4rem;
		aspect-ratio: var(--cover-aspect);
		display: grid;
		place-items: center;
		overflow: hidden;
		border-radius: var(--radius-sm);
		background: linear-gradient(145deg, #59616d, #242932);
		box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.14);
	}
	.cover img {
		position: absolute;
		inset: 0;
		width: 100%;
		height: 100%;
		object-fit: cover;
	}
	.cover::after {
		content: '';
		position: absolute;
		inset: 8%;
		border: 1px solid rgba(255, 255, 255, 0.35);
	}
	.cover span {
		z-index: 1;
		font-family: var(--font-display);
		font-weight: 700;
		text-shadow: 0 1px 4px rgba(0, 0, 0, 0.5);
	}
	.identity {
		min-width: 0;
	}
	.identity h3 {
		margin: 0 0 0.2rem;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		font-size: 0.94rem;
		font-weight: 500;
	}
	.identity h3 span {
		color: var(--muted);
	}
	.identity p,
	.identity small {
		display: block;
		margin: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		color: var(--muted);
		font-size: 0.75rem;
	}
	.identity small {
		margin-top: 0.15rem;
		font-size: 0.68rem;
	}
	.chips {
		display: flex;
		flex-wrap: wrap;
		gap: 0.35rem;
	}
	.chips span {
		padding: 0.22rem 0.48rem;
		border: 1px solid var(--border);
		border-radius: var(--radius-sm);
		color: var(--muted);
		font-size: 0.68rem;
		white-space: nowrap;
		background: var(--surface);
	}
	.release-date {
		display: grid;
		grid-template-columns: auto auto;
		align-items: center;
		gap: 0.35rem;
		color: var(--accent);
		white-space: nowrap;
		font-size: 0.78rem;
	}
	.release-date svg,
	.external svg {
		width: 1rem;
		height: 1rem;
		fill: none;
		stroke: currentColor;
		stroke-width: 1.8;
		stroke-linecap: round;
		stroke-linejoin: round;
	}
	.release-date small {
		grid-column: 2;
		margin-top: -0.2rem;
		color: var(--muted);
		font-size: 0.58rem;
		text-transform: uppercase;
		letter-spacing: 0.08em;
	}
	.release-date.estimated svg {
		stroke-dasharray: 2 2;
	}
	.external {
		display: grid;
		place-items: center;
		width: 2rem;
		height: 2rem;
		border-radius: var(--radius-sm);
		color: var(--muted);
	}
	.external:hover {
		color: var(--accent);
		background: var(--accent-soft);
	}
	.side {
		display: flex;
		flex-direction: column;
		gap: var(--space-3);
	}
	.panel {
		border: 1px solid var(--border);
		border-radius: var(--radius);
		padding: var(--space-4);
		background: var(--surface);
	}
	.panel h2 {
		margin: 0 0 var(--space-3);
		font-size: 0.88rem;
		font-weight: 500;
	}
	.panel h2 span {
		margin-left: 0.25rem;
		color: var(--muted);
		font-size: 0.72rem;
		font-weight: 400;
	}
	.source-list {
		display: flex;
		flex-direction: column;
	}
	.source {
		all: unset;
		box-sizing: border-box;
		display: grid;
		grid-template-columns: 2.4rem minmax(0, 1fr) auto;
		align-items: center;
		gap: 0.65rem;
		width: 100%;
		padding: 0.65rem 0;
		border-bottom: 1px solid var(--border);
		cursor: pointer;
	}
	.source:last-child {
		border-bottom: 0;
	}
	.source.disabled {
		opacity: 0.45;
	}
	.source-icon {
		position: relative;
		overflow: hidden;
		width: 2.35rem;
		height: 2.35rem;
		display: grid;
		place-items: center;
		border-radius: var(--radius-sm);
		background: color-mix(in srgb, var(--source-color) 32%, var(--surface-2));
		color: #fff;
		font-size: 0.72rem;
		font-weight: 700;
	}
	.source-icon img {
		position: absolute;
		inset: 0;
		width: 100%;
		height: 100%;
		object-fit: cover;
	}
	.source-name {
		min-width: 0;
		display: flex;
		flex-direction: column;
		gap: 0.1rem;
	}
	.source-name strong {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		font-size: 0.78rem;
		font-weight: 500;
	}
	.source-name small {
		color: var(--muted);
		font-size: 0.64rem;
	}
	.status {
		color: var(--good);
		font-size: 0.62rem;
	}
	.status.limited {
		color: #d49b2a;
	}
	.status.idle {
		color: var(--muted);
	}
	.manage {
		display: block;
		margin-top: var(--space-3);
		padding: 0.55rem;
		border: 1px solid var(--border);
		border-radius: var(--radius-sm);
		text-align: center;
		color: var(--muted);
		font-size: 0.72rem;
	}
	.manage:hover {
		color: var(--text);
		border-color: var(--accent);
	}
	.legend p {
		display: flex;
		align-items: center;
		gap: 0.55rem;
		margin: 0.45rem 0;
		color: var(--muted);
		font-size: 0.7rem;
	}
	.key {
		width: 0.55rem;
		height: 0.75rem;
		border: 1px solid;
		border-radius: 2px;
	}
	.key.print {
		color: var(--accent);
	}
	.key.digital {
		color: #4f9ee8;
	}
	.calendar {
		width: 0.7rem;
		height: 0.7rem;
		border: 1px solid var(--muted);
		border-radius: 2px;
	}
	.calendar.estimate {
		border-style: dashed;
	}
	.about p {
		margin: 0;
		color: var(--muted);
		font-size: 0.7rem;
		line-height: 1.45;
	}
	.empty {
		padding: var(--space-8);
		border: 1px solid var(--border);
		border-radius: var(--radius);
		text-align: center;
	}
	.empty h2 {
		margin: 0 0 var(--space-2);
		font-size: 1rem;
	}
	.empty p {
		margin: 0;
		color: var(--muted);
		font-size: 0.8rem;
	}
	.calendar-mode {
		display: block;
	}
	.calendar-scroll {
		overflow-x: auto;
		padding-bottom: 1px;
		border-radius: var(--radius);
	}
	.month-calendar {
		position: relative;
		min-width: 760px;
		overflow: hidden;
		border: 1px solid var(--border);
		border-radius: var(--radius);
		background: color-mix(in srgb, var(--surface) 35%, transparent);
	}
	.month-toolbar {
		display: flex;
		align-items: center;
		gap: var(--space-4);
		min-height: 3.8rem;
		padding: 0.6rem 0.8rem;
		border-bottom: 1px solid var(--border);
	}
	.month-toolbar > button {
		padding: 0.45rem 0.85rem;
		font-size: 0.75rem;
	}
	.month-nav {
		display: flex;
		align-items: center;
		gap: 0.2rem;
	}
	.month-nav button {
		display: grid;
		width: 2rem;
		height: 2rem;
		padding: 0;
		place-items: center;
		border-color: transparent;
		background: transparent;
	}
	.month-nav button:hover {
		border-color: var(--border);
		background: var(--surface-2);
	}
	.month-nav svg {
		width: 1rem;
		height: 1rem;
		fill: none;
		stroke: currentColor;
		stroke-width: 1.8;
		stroke-linecap: round;
		stroke-linejoin: round;
	}
	.month-nav h2 {
		margin: 0 0 0 0.65rem;
		font-size: 0.96rem;
		font-weight: 500;
	}
	.weekdays {
		display: grid;
		grid-template-columns: repeat(7, minmax(0, 1fr));
		border-bottom: 1px solid var(--border);
		background: color-mix(in srgb, var(--surface-2) 55%, transparent);
	}
	.weekdays span {
		padding: 0.55rem 0.7rem;
		color: var(--muted);
		font-size: 0.64rem;
		text-align: center;
		text-transform: uppercase;
		letter-spacing: 0.08em;
	}
	.month-grid {
		display: grid;
		grid-template-columns: repeat(7, minmax(0, 1fr));
	}
	.day {
		min-width: 0;
		min-height: 7.35rem;
		padding: 0.55rem;
		border-right: 1px solid var(--border);
		border-bottom: 1px solid var(--border);
		background: color-mix(in srgb, var(--surface) 22%, transparent);
	}
	.day:nth-child(7n) {
		border-right: 0;
	}
	.day:nth-last-child(-n + 7) {
		border-bottom: 0;
	}
	.day.outside {
		background: color-mix(in srgb, var(--bg) 45%, transparent);
	}
	.day > time {
		display: grid;
		width: 1.45rem;
		height: 1.45rem;
		place-items: center;
		color: var(--muted);
		font-size: 0.68rem;
	}
	.day.outside > time {
		opacity: 0.38;
	}
	.day.today > time {
		border-radius: 50%;
		background: var(--accent);
		color: #fff;
	}
	.day-releases {
		display: flex;
		flex-direction: column;
		gap: 0.22rem;
		margin-top: 0.25rem;
	}
	.calendar-event,
	.more-events {
		all: unset;
		box-sizing: border-box;
		min-width: 0;
		cursor: pointer;
	}
	.calendar-event {
		display: grid;
		grid-template-columns: 0.52rem minmax(0, 1fr);
		align-items: center;
		gap: 0.35rem;
		padding: 0.17rem 0.2rem;
		border-radius: 3px;
		color: var(--muted);
	}
	.calendar-event:hover {
		color: var(--text);
		background: var(--surface-2);
	}
	.calendar-event > span {
		width: 0.48rem;
		height: 0.62rem;
		border: 1px solid var(--accent);
		border-radius: 2px;
	}
	.calendar-event.digital-only > span {
		border-color: #4f9ee8;
	}
	.calendar-event strong {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		font-size: 0.62rem;
		font-weight: 400;
	}
	.more-events {
		padding: 0.12rem 1.05rem;
		color: var(--muted);
		font-size: 0.6rem;
	}
	.more-events:hover {
		color: var(--accent);
	}
	.calendar-dismiss {
		position: absolute;
		z-index: 4;
		inset: 0;
		width: 100%;
		height: 100%;
		padding: 0;
		border: 0;
		border-radius: 0;
		background: rgba(5, 7, 10, 0.26);
		cursor: default;
	}
	.calendar-detail {
		position: absolute;
		z-index: 5;
		top: 50%;
		left: 50%;
		width: min(23rem, calc(100% - 2rem));
		padding: var(--space-4);
		transform: translate(-50%, -45%);
		border: 1px solid var(--border);
		border-radius: var(--radius);
		background: color-mix(in srgb, var(--surface) 96%, #000);
		box-shadow: 0 1.4rem 4rem rgba(0, 0, 0, 0.45);
	}
	.detail-head {
		display: grid;
		grid-template-columns: 4.1rem minmax(0, 1fr);
		gap: var(--space-3);
		align-items: start;
	}
	.detail-cover {
		position: relative;
		width: 4.1rem;
		aspect-ratio: var(--cover-aspect);
		display: grid;
		overflow: hidden;
		place-items: center;
		border-radius: var(--radius-sm);
		background: var(--surface-2);
	}
	.detail-cover img {
		position: absolute;
		inset: 0;
		width: 100%;
		height: 100%;
		object-fit: cover;
	}
	.detail-cover span {
		font-family: var(--font-display);
	}
	.detail-head h3 {
		margin: 0.1rem 0 0.35rem;
		font-size: 0.94rem;
		font-weight: 550;
	}
	.detail-head p,
	.detail-head small {
		display: block;
		margin: 0;
		color: var(--muted);
		font-size: 0.72rem;
	}
	.detail-head small {
		margin-top: 0.25rem;
		font-size: 0.65rem;
	}
	.detail-chips {
		display: flex;
		flex-wrap: wrap;
		gap: 0.35rem;
		margin: 0.8rem 0;
	}
	.detail-chips span {
		padding: 0.2rem 0.45rem;
		border: 1px solid var(--border);
		border-radius: var(--radius-sm);
		color: var(--muted);
		font-size: 0.64rem;
	}
	.detail-date {
		margin: 0.2rem 0 0.85rem;
		color: var(--accent);
		font-size: 0.75rem;
	}
	.detail-actions {
		display: grid;
		grid-template-columns: repeat(2, minmax(0, 1fr));
		gap: 0.45rem;
	}
	.detail-actions a {
		padding: 0.48rem;
		border: 1px solid var(--border);
		border-radius: var(--radius-sm);
		color: var(--muted);
		text-align: center;
		font-size: 0.68rem;
	}
	.detail-actions a:hover {
		color: var(--text);
		border-color: var(--accent);
	}
	@media (max-width: 1200px) {
		.layout {
			grid-template-columns: minmax(0, 1fr) 15rem;
		}
		.release {
			grid-template-columns: 3.4rem minmax(0, 1fr) auto 2rem;
		}
		.chips {
			grid-column: 2;
		}
	}
	@media (max-width: 900px) {
		.upcoming-page {
			padding: var(--space-5);
		}
		.upcoming-page.embedded {
			padding: 0;
		}
		.hero {
			flex-direction: column;
		}
		.hero.embedded {
			align-items: flex-end;
		}
		.toolbar {
			width: 100%;
			flex-wrap: wrap;
		}
		.layout {
			grid-template-columns: 1fr;
		}
		.side {
			display: grid;
			grid-template-columns: repeat(2, minmax(0, 1fr));
		}
		.sources-card {
			grid-row: span 2;
		}
	}
	@media (max-width: 640px) {
		.hero-title > svg {
			width: 1.6rem;
			height: 1.6rem;
		}
		h1 {
			font-size: 1.45rem;
		}
		.toolbar > button {
			flex: 1;
			justify-content: center;
		}
		.view-toggle {
			margin-left: auto;
		}
		.release {
			grid-template-columns: 3.1rem minmax(0, 1fr) auto;
			align-items: start;
		}
		.cover {
			width: 3.1rem;
			grid-row: span 3;
		}
		.chips {
			grid-column: 2;
		}
		.release-date {
			grid-column: 2;
		}
		.external {
			grid-column: 3;
			grid-row: 1;
		}
		.side {
			display: flex;
		}
		.calendar-scroll {
			margin-inline: calc(-1 * var(--space-2));
		}
	}
</style>
