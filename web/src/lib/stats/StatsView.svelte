<script>
	import { onMount } from 'svelte';
	import { stats as statsApi, kinds as kindsApi, media, ApiError } from '$lib/api.js';
	import Loading from '$lib/components/ui/Loading.svelte';
	import CoverThumbnail from '$lib/components/CoverThumbnail.svelte';
	import { relativeTime } from '$lib/format.js';

	const DAY = 86400;
	let data = $state(null);
	let libraryTotal = $state(0);
	let error = $state(null);
	let loading = $state(true);
	let year = $state(new Date().getUTCFullYear());

	onMount(load);
	async function load() {
		loading = true;
		try {
			const [s, ks] = await Promise.all([statsApi.get(), kindsApi.list().catch(() => [])]);
			data = s;
			libraryTotal = (ks ?? []).reduce((a, k) => a + (k.count ?? 0), 0);
			const days = s.activity?.days ?? [];
			if (days.length) year = new Date(days[days.length - 1].day * DAY * 1000).getUTCFullYear();
		} catch (e) {
			if (!(e instanceof ApiError && e.status === 401)) error = e?.message ?? String(e);
		} finally {
			loading = false;
		}
	}

	const fmt = (n) => (n ?? 0).toLocaleString();
	function compact(n) {
		n = n ?? 0;
		if (n >= 1e6) return (n / 1e6).toFixed(n >= 1e7 ? 0 : 1).replace(/\.0$/, '') + 'M';
		if (n >= 1e3) return (n / 1e3).toFixed(n >= 1e4 ? 0 : 1).replace(/\.0$/, '') + 'K';
		return String(n);
	}
	const MONTHS = [
		'Jan',
		'Feb',
		'Mar',
		'Apr',
		'May',
		'Jun',
		'Jul',
		'Aug',
		'Sep',
		'Oct',
		'Nov',
		'Dec',
	];
	const MONTHS_LONG = [
		'January',
		'February',
		'March',
		'April',
		'May',
		'June',
		'July',
		'August',
		'September',
		'October',
		'November',
		'December',
	];
	const monthOf = (dayIdx) => new Date(dayIdx * DAY * 1000).getUTCMonth();

	const activity = $derived(data?.activity ?? { days: [] });
	const days = $derived(activity.days ?? []);
	const dayMap = $derived(new Map(days.map((d) => [d.day, d])));
	const years = $derived(
		[...new Set(days.map((d) => new Date(d.day * DAY * 1000).getUTCFullYear()))].sort(
			(a, b) => b - a,
		),
	);

	const libraryPct = $derived(
		libraryTotal > 0
			? Math.round(((data?.totals.comics_finished ?? 0) / libraryTotal) * 100)
			: null,
	);

	const busiestMonth = $derived.by(() => {
		const inYear = days.filter((d) => new Date(d.day * DAY * 1000).getUTCFullYear() === year);
		if (!inYear.length) return null;
		const sums = Array(12).fill(0);
		for (const d of inYear) sums[monthOf(d.day)] += d.pages || d.updates;
		let best = 0;
		for (let m = 1; m < 12; m++) if (sums[m] > sums[best]) best = m;
		return MONTHS_LONG[best];
	});
	const readingDaysThisYear = $derived(
		days.filter((d) => new Date(d.day * DAY * 1000).getUTCFullYear() === year).length,
	);
	const longestStreakMonth = $derived.by(() => {
		if (!days.length) return null;
		let bestLen = 0,
			bestEnd = null,
			run = 0,
			prev = null;
		for (const d of days) {
			run = prev != null && d.day === prev + 1 ? run + 1 : 1;
			if (run >= bestLen) {
				bestLen = run;
				bestEnd = d.day;
			}
			prev = d.day;
		}
		return bestEnd == null ? null : MONTHS_LONG[monthOf(bestEnd)];
	});

	const heat = $derived.by(() => {
		const start = Date.UTC(year, 0, 1) / 1000 / DAY;
		const end = Date.UTC(year, 11, 31) / 1000 / DAY;
		const firstDow = new Date(start * DAY * 1000).getUTCDay();
		const weeks = [];
		let col = new Array(firstDow).fill(null);
		const maxPages = Math.max(1, ...days.map((d) => d.pages || d.updates));
		for (let idx = start; idx <= end; idx++) {
			const row = dayMap.get(idx);
			const v = row ? row.pages || row.updates : 0;
			const level = v === 0 ? 0 : Math.min(4, 1 + Math.floor((v / maxPages) * 3.999));
			col.push({ idx, level, pages: row?.pages ?? 0, date: new Date(idx * DAY * 1000) });
			if (col.length === 7) {
				weeks.push(col);
				col = [];
			}
		}
		if (col.length) {
			while (col.length < 7) col.push(null);
			weeks.push(col);
		}
		const monthCols = [];
		weeks.forEach((w, wi) => {
			const firstReal = w.find((c) => c);
			if (!firstReal) return;
			const m = firstReal.date.getUTCMonth();
			if (!monthCols.length || monthCols[monthCols.length - 1].m !== m) monthCols.push({ m, wi });
		});
		return { weeks, monthCols };
	});

	const ratings = $derived(
		data?.ratings ?? { count: 0, average: 0, distribution: [0, 0, 0, 0, 0, 0, 0, 0, 0, 0] },
	);
	const ratingMax = $derived(Math.max(1, ...(ratings.distribution ?? [])));

	const taste = $derived((data?.taste ?? []).slice(0, 11));
	const tasteMax = $derived(Math.max(1, ...taste.map((t) => t.count)));
	const kindMix = $derived.by(() => {
		const bk = data?.by_kind ?? [];
		const total = bk.reduce((a, k) => a + k.started, 0);
		if (!total) return [];
		return bk
			.map((k) => ({ kind: k.kind, pct: Math.round((k.started / total) * 100) }))
			.filter((k) => k.pct > 0)
			.slice(0, 3);
	});

	const barMax = (list) => Math.max(1, ...list.map((t) => t.count));

	const recent = $derived(data?.recent ?? []);
	const recentCaption = $derived.by(() => {
		if (!recent.length) return '';
		const first = recent[0];
		const week = recent.filter((r) => Date.now() / 1000 - r.finished_at < 7 * 86400).length;
		const more = week - 1;
		return `${first.name} · ${relativeTime(first.finished_at)}${more > 0 ? ` — and ${more} more this week` : ''}`;
	});

	const seriesItems = $derived(data?.series.items ?? []);
	const longestCompleted = $derived(data?.series.longest_completed ?? null);
</script>

<div class="page">
	{#if loading}
		<Loading />
	{:else if error}
		<div class="err">Couldn't load your stats: {error}</div>
	{:else if data}
		{#if years.length}
			<header class="head">
				<select class="range" bind:value={year} aria-label="Year">
					{#each years as y (y)}<option value={y}>{y}</option>{/each}
				</select>
			</header>
		{/if}

		<section class="tiles">
			<div class="tile">
				<p class="k">Comics Read</p>
				<p class="v">{fmt(data.totals.comics_finished)}</p>
				<p class="sub">{libraryPct != null ? `${libraryPct}% of your library` : ' '}</p>
			</div>
			<div class="tile">
				<p class="k">Books Read</p>
				<p class="v">{fmt(data.totals.books_finished)}</p>
				<p class="sub">
					{data.totals.words_read ? `${compact(data.totals.words_read)} words` : ' '}
				</p>
			</div>
			<div class="tile">
				<p class="k">Pages Reached</p>
				<p class="v">{fmt(data.totals.pages_read)}</p>
				<p class="sub">~{Math.round(activity.pages_per_active_day ?? 0)} per day</p>
			</div>
			<div class="tile">
				<p class="k">Day Streak</p>
				<p class="v">
					<span class="accent">{activity.current_streak ?? 0}</span> <small>days</small>
				</p>
				<p class="sub">
					Best: {activity.longest_streak ?? 0}{longestStreakMonth
						? ` in ${longestStreakMonth}`
						: ''}
				</p>
			</div>
		</section>

		<section class="card activity">
			<div class="cardhead">
				<h2>Activity</h2>
				<span class="muted">
					{readingDaysThisYear} reading days this year{busiestMonth
						? ` · most active in ${busiestMonth}`
						: ''}
				</span>
			</div>
			<div class="heatwrap">
				<div class="months" style={`grid-template-columns:repeat(${heat.weeks.length}, 1fr)`}>
					{#each heat.monthCols as mc (mc.m)}
						<span class="mlabel" style={`grid-column-start:${mc.wi + 1}`}>{MONTHS[mc.m]}</span>
					{/each}
				</div>
				<div class="grid" style={`grid-template-columns:repeat(${heat.weeks.length}, 1fr)`}>
					{#each heat.weeks as w, wi (wi)}
						<div class="wcol">
							{#each w as cell, di (di)}
								<div
									class="cell l{cell?.level ?? 0}"
									class:empty={!cell}
									title={cell
										? `${cell.date.toISOString().slice(0, 10)} · ${cell.pages} pages`
										: ''}
								></div>
							{/each}
						</div>
					{/each}
				</div>
			</div>
			<div class="legend">
				<span class="lgtext">Less</span>
				<div class="cell l0"></div>
				<div class="cell l1"></div>
				<div class="cell l2"></div>
				<div class="cell l3"></div>
				<div class="cell l4"></div>
				<span class="lgtext">More</span>
			</div>
		</section>

		<div class="cols2">
			<section class="card">
				<h2>Top creators</h2>
				<div class="bars">
					{#each (data.top.creators ?? []).slice(0, 5) as t (t.value)}
						{@const mx = barMax(data.top.creators)}
						<div class="brow">
							<span class="blabel" title={t.value}>{t.value}</span>
							<span class="btrack"
								><span class="bfill" style={`width:${(t.count / mx) * 100}%`}></span></span
							>
							<span class="bnum">{t.count}</span>
						</div>
					{/each}
				</div>
				<p class="foot">Across your finished reads with creator tags</p>
			</section>

			<section class="card">
				<h2>Top sources</h2>
				<div class="bars">
					{#each (data.top.parodies ?? []).slice(0, 5) as t (t.value)}
						{@const mx = barMax(data.top.parodies)}
						<div class="brow">
							<span class="blabel" title={t.value}>{t.value}</span>
							<span class="btrack"
								><span class="bfill" style={`width:${(t.count / mx) * 100}%`}></span></span
							>
							<span class="bnum">{t.count}</span>
						</div>
					{/each}
				</div>
				<p class="foot">Parody tags across finished items</p>
			</section>

			<section class="card">
				<h2>Your ratings</h2>
				<div class="ravg">
					<span class="ravgn">{ratings.average ? (ratings.average / 2).toFixed(1) : '—'}</span>
					<span class="rmeta muted">average / 5 · {fmt(ratings.count)} rated</span>
				</div>
				<div class="bars">
					{#each [10, 9, 8, 7, 6, 5, 4, 3, 2, 1] as v (v)}
						{@const n = ratings.distribution[v - 1] ?? 0}
						<div class="brow">
							<span class="rstar">{(v / 2).toFixed(1)}★</span>
							<span class="btrack"
								><span class="bfill" style={`width:${(n / ratingMax) * 100}%`}></span></span
							>
							<span class="bnum">{n}</span>
						</div>
					{/each}
				</div>
			</section>

			<section class="card">
				<h2>Your taste</h2>
				{#if taste.length}
					<div class="cloud">
						{#each taste as t, i (t.value)}
							<span
								class="chip"
								class:top={i < 3}
								style={`font-size:${0.75 + (t.count / tasteMax) * 0.6}rem`}>{t.value}</span
							>
						{/each}
					</div>
				{:else}
					<p class="muted">No tagged reads yet.</p>
				{/if}
				{#if kindMix.length}
					<p class="foot">
						Weighted from your favorites (2×) + finished reads · {kindMix
							.map((k) => `${k.pct}% ${k.kind}`)
							.join(', ')}
					</p>
				{/if}
			</section>

			<section class="card">
				<h2>Series progress</h2>
				{#if seriesItems.length}
					<div class="bars">
						{#each seriesItems.slice(0, 6) as sp (sp.id)}
							<a class="brow slink" href={`/series/${sp.id}`}>
								<span class="blabel" title={sp.title}>{sp.title}</span>
								<span class="btrack"
									><span class="bfill" style={`width:${(sp.read / Math.max(1, sp.total)) * 100}%`}
									></span></span
								>
								<span class="bnum">{sp.read}/{sp.total}</span>
							</a>
						{/each}
					</div>
					<p class="foot">
						{data.series.finished} series finished{longestCompleted
							? ` · longest completed: ${longestCompleted.title} (${longestCompleted.total} volumes)`
							: ''}
					</p>
				{:else}
					<p class="muted">No series started yet.</p>
				{/if}
			</section>

			<section class="card">
				<h2>Recently finished</h2>
				{#if recent.length}
					<div class="covers">
						{#each recent.slice(0, 5) as r (r.id)}
							<a class="cover" href={`/item/${r.id}`} title={r.name}>
								<CoverThumbnail src={media.thumbnail(r.id, r.cover_version)} alt={r.name} />
							</a>
						{/each}
					</div>
					<p class="foot">{recentCaption}</p>
				{:else}
					<p class="muted">Nothing finished yet.</p>
				{/if}
			</section>
		</div>
	{/if}
</div>

<style>
	.page {
		display: flex;
		flex-direction: column;
		gap: var(--space-4);
	}
	.head {
		display: flex;
		align-items: center;
		justify-content: flex-end;
	}
	.range {
		background-color: var(--surface);
		color: var(--text);
		border: 1px solid var(--border);
		border-radius: var(--radius-sm);
		padding: 0.4rem 1.7rem 0.4rem 0.75rem;
		font-size: 0.85rem;
	}
	.err {
		padding: var(--space-4);
		border: 1px solid rgba(224, 86, 111, 0.4);
		background: rgba(224, 86, 111, 0.1);
		color: #e0566f;
		border-radius: var(--radius);
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
		padding: var(--space-4);
	}
	.tile .k {
		margin: 0;
		font-size: 0.7rem;
		text-transform: uppercase;
		letter-spacing: 0.12em;
		color: var(--muted);
	}
	.tile .v {
		margin: 0.35rem 0 0.2rem;
		font-size: 2rem;
		font-weight: 700;
		line-height: 1.1;
	}
	.tile .v small {
		font-size: 0.9rem;
		font-weight: 500;
		color: var(--muted);
	}
	.tile .sub {
		margin: 0;
		font-size: 0.8rem;
		color: var(--muted);
	}
	.accent {
		color: var(--accent);
	}

	.card {
		background: var(--surface);
		border: 1px solid var(--border);
		border-radius: var(--radius);
		padding: var(--space-5);
	}
	.card h2 {
		margin: 0 0 var(--space-4);
		font-size: 1.05rem;
	}
	.cardhead {
		display: flex;
		align-items: baseline;
		justify-content: space-between;
		gap: var(--space-3);
		margin-bottom: var(--space-4);
		flex-wrap: wrap;
	}
	.cardhead h2 {
		margin: 0;
	}
	.muted {
		color: var(--muted);
	}
	.small {
		font-size: 0.78rem;
	}

	.heatwrap {
		overflow-x: auto;
	}
	.months {
		display: grid;
		gap: 3px;
		font-size: 0.68rem;
		color: var(--muted);
		margin-bottom: 4px;
		min-width: 640px;
	}
	.mlabel {
		grid-row: 1;
		white-space: nowrap;
	}
	.grid {
		display: grid;
		gap: 3px;
		min-width: 640px;
	}
	.wcol {
		display: grid;
		grid-template-rows: repeat(7, 1fr);
		gap: 3px;
	}
	.cell {
		aspect-ratio: 1;
		border-radius: 3px;
		background: color-mix(in srgb, var(--accent) 12%, transparent);
	}
	.cell.empty {
		background: transparent;
	}
	.cell.l0 {
		background: color-mix(in srgb, var(--text) 6%, transparent);
	}
	.cell.l1 {
		background: color-mix(in srgb, var(--accent) 30%, transparent);
	}
	.cell.l2 {
		background: color-mix(in srgb, var(--accent) 50%, transparent);
	}
	.cell.l3 {
		background: color-mix(in srgb, var(--accent) 72%, transparent);
	}
	.cell.l4 {
		background: var(--accent);
	}

	.legend {
		display: flex;
		align-items: center;
		justify-content: flex-end;
		gap: 4px;
		margin-top: var(--space-3);
	}
	.legend .cell {
		width: 12px;
		height: 12px;
		aspect-ratio: auto;
	}
	.lgtext {
		font-size: 0.72rem;
		color: var(--muted);
	}
	.lgtext:first-child {
		margin-right: 2px;
	}
	.lgtext:last-child {
		margin-left: 2px;
	}

	.cols2 {
		display: grid;
		grid-template-columns: 1fr 1fr;
		gap: var(--space-4);
	}

	.bars {
		display: flex;
		flex-direction: column;
		gap: 0.6rem;
	}
	.brow {
		display: grid;
		grid-template-columns: 7.5rem 1fr 2.2rem;
		align-items: center;
		gap: 0.6rem;
	}
	.blabel {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		font-size: 0.85rem;
	}
	.rstar {
		font-size: 0.85rem;
		color: var(--muted);
		font-variant-numeric: tabular-nums;
	}
	.btrack {
		height: 8px;
		border-radius: 9999px;
		background: color-mix(in srgb, var(--text) 8%, transparent);
		overflow: hidden;
	}
	.bfill {
		display: block;
		height: 100%;
		border-radius: 9999px;
		background: var(--accent);
	}
	.bnum {
		text-align: right;
		font-variant-numeric: tabular-nums;
		font-size: 0.85rem;
		color: var(--muted);
	}
	.foot {
		margin: var(--space-4) 0 0;
		font-size: 0.78rem;
		color: var(--muted);
	}

	.ravg {
		display: flex;
		align-items: baseline;
		gap: 0.6rem;
		margin-bottom: var(--space-4);
	}
	.ravgn {
		font-size: 2rem;
		font-weight: 700;
	}

	.cloud {
		display: flex;
		flex-wrap: wrap;
		gap: 0.5rem;
		align-items: center;
	}
	.chip {
		padding: 0.25rem 0.7rem;
		border: 1px solid var(--border);
		border-radius: 9999px;
		line-height: 1.2;
		color: var(--text);
	}
	.chip.top {
		color: var(--accent);
		border-color: var(--accent);
	}

	.slink {
		text-decoration: none;
		color: inherit;
		cursor: pointer;
	}
	.slink:hover .blabel {
		color: var(--accent);
	}
	.covers {
		display: grid;
		grid-template-columns: repeat(5, 1fr);
		gap: 0.6rem;
		margin-bottom: var(--space-3);
	}
	.cover {
		display: block;
		border-radius: var(--radius-sm);
		overflow: hidden;
		aspect-ratio: 3 / 4;
	}

	@media (max-width: 780px) {
		.tiles {
			grid-template-columns: repeat(2, 1fr);
		}
		.cols2 {
			grid-template-columns: 1fr;
		}
	}
</style>
