<script>
	import { onMount } from 'svelte';
	import { metrics as metricsApi } from '$lib/api.js';
	import { kindLabel } from '$lib/kinds.js';
	import Loading from '$lib/components/ui/Loading.svelte';

	const KIND_COLORS = [
		'#4a9eff',
		'#3fb950',
		'#d29922',
		'#a78bfa',
		'#f85149',
		'#2dd4bf',
		'#f472b6',
		'#facc15',
		'#38bdf8',
		'#a3e635',
	];
	const kindColor = (i) => KIND_COLORS[i % KIND_COLORS.length];

	const REFRESH_MS = 4000;
	const HIST = 90;

	let data = $state(null);
	let error = $state(null);
	let adminOnly = $state(false);
	let firstLoad = $state(true);
	let hist = $state({ cpu: [], mem: [], rx: [], tx: [], rd: [], wr: [] });

	function push(k, v) {
		const a = hist[k];
		a.push(v ?? null);
		if (a.length > HIST) a.shift();
	}

	async function tick() {
		if (typeof document !== 'undefined' && document.visibilityState === 'hidden') return;
		try {
			const d = await metricsApi.get();
			data = d;
			error = null;
			adminOnly = false;
			const s = d.system;
			push('cpu', s.cpu_pct);
			push('mem', s.mem_rss != null && s.mem_limit ? (s.mem_rss / s.mem_limit) * 100 : null);
			push('rx', s.net.rx_rate);
			push('tx', s.net.tx_rate);
			push('rd', s.disk.read_rate);
			push('wr', s.disk.write_rate);
			hist = { ...hist };
		} catch (e) {
			if (e?.status === 403) adminOnly = true;
			else error = e?.message ?? String(e);
		} finally {
			firstLoad = false;
		}
	}

	onMount(() => {
		tick();
		const t = setInterval(tick, REFRESH_MS);
		return () => clearInterval(t);
	});

	const fmtBytes = (n) => {
		if (n == null) return '—';
		const u = ['B', 'KB', 'MB', 'GB', 'TB', 'PB'];
		let v = n,
			i = 0;
		while (v >= 1024 && i < u.length - 1) {
			v /= 1024;
			i++;
		}
		return `${v >= 100 || i === 0 ? Math.round(v) : v.toFixed(1)} ${u[i]}`;
	};
	const fmtRate = (n) => (n == null ? '—' : `${fmtBytes(n)}/s`);
	const fmtNum = (n) => (n == null ? '—' : n.toLocaleString());
	const fmtDur = (s) => {
		if (s == null) return '—';
		const d = Math.floor(s / 86400),
			h = Math.floor((s % 86400) / 3600),
			m = Math.floor((s % 3600) / 60);
		if (d) return `${d}d ${h}h`;
		if (h) return `${h}h ${m}m`;
		if (m) return `${m}m`;
		return `${Math.floor(s)}s`;
	};
	const ago = (ts) => (ts ? `${fmtDur(Math.floor(Date.now() / 1000) - ts)} ago` : '—');
	const pct = (a, b) => (b > 0 ? Math.round((a / b) * 100) : 0);

	const FAILED_SHOWN = 5;
	let showAllFailed = $state(false);
	const visibleFailed = $derived(
		showAllFailed ? (data?.jobs.failed ?? []) : (data?.jobs.failed ?? []).slice(0, FAILED_SHOWN),
	);
	const jobTotals = $derived.by(() => {
		const t = { pending: 0, running: 0, failed: 0, done: 0 };
		for (const c of data?.jobs?.counts ?? []) t[c.state] = (t[c.state] || 0) + c.count;
		return t;
	});
	let openState = $state(undefined);
	$effect(() => {
		if (openState === undefined && data) openState = jobTotals.failed > 0 ? 'failed' : null;
	});
	const toggleState = (s) => (openState = openState === s ? null : s);
	const pendingByKind = $derived(data?.jobs.pending ?? []);
	const pendingWhen = (runAfter) => {
		const seconds = Math.floor(runAfter - Date.now() / 1000);
		return seconds > 0 ? `runs in ${fmtDur(seconds)}` : 'ready';
	};
	let expandedFails = $state(new Set());
	function toggleFail(key) {
		const s = new Set(expandedFails);
		s.has(key) ? s.delete(key) : s.add(key);
		expandedFails = s;
	}
	const taggedItems = $derived(data ? data.library.items - data.library.untagged : 0);
	const dbPct = $derived(
		data && data.library.bytes > 0 ? (data.storage.db_bytes / data.library.bytes) * 100 : null,
	);
	function dbRating(p) {
		if (p < 0.25) return { label: 'Great', cls: 'r-great' };
		if (p < 0.5) return { label: 'Good', cls: 'r-good' };
		if (p < 1) return { label: 'OK', cls: 'r-ok' };
		if (p < 3) return { label: 'Bad', cls: 'r-bad' };
		return { label: 'Poor', cls: 'r-poor' };
	}
	const netMax = $derived(
		Math.max(1, ...hist.rx.map((x) => x ?? 0), ...hist.tx.map((x) => x ?? 0)),
	);
	const diskMax = $derived(
		Math.max(1, ...hist.rd.map((x) => x ?? 0), ...hist.wr.map((x) => x ?? 0)),
	);
	const spanSec = $derived(Math.max(0, (hist.cpu.length - 1) * (REFRESH_MS / 1000)));
	const xTicks = $derived(
		[1, 0.75, 0.5, 0.25, 0].map((f) => (f === 0 ? 'now' : fmtDur(Math.round(spanSec * f)))),
	);

	const W = 320,
		H = 100;
	function spark(vals, max) {
		const v = vals.map((x) => (x == null ? 0 : x));
		const m = Math.max(max || 0, 1, ...v);
		const n = v.length;
		if (!n) return { line: '', area: '' };
		const dx = n > 1 ? W / (n - 1) : W;
		const pts = v.map(
			(y, i) => `${(i * dx).toFixed(1)},${(H - (Math.min(y, m) / m) * H).toFixed(1)}`,
		);
		const line = pts.join(' ');
		return { line, area: `0,${H} ${line} ${((n - 1) * dx).toFixed(1)},${H}` };
	}
	const C = {
		cpu: '#4a9eff',
		mem: '#a78bfa',
		rx: '#3fb950',
		tx: '#d29922',
		rd: '#4a9eff',
		wr: '#f85149',
	};
</script>

<div class="sh">
	<header class="head">
		<div>
			<h2>Server health</h2>
			<p class="muted">
				arcagrad{data ? ` v${data.version}` : ''}{data ? ` · up ${fmtDur(data.uptime_secs)}` : ''}
			</p>
		</div>
		{#if data}
			<div class="statuses">
				<span class="live"
					><span class="dot" style={`background:${data.jobs.watcher ? '#3fb950' : '#f85149'}`}
					></span>
					Watcher {data.jobs.watcher ? 'on' : 'off'}</span
				>
				<span class="live"><span class="dot"></span> Live</span>
			</div>
		{/if}
	</header>

	{#if adminOnly}
		<div class="card note">Server health is available to admins only.</div>
	{:else if firstLoad && !data}
		{#if error}<div class="card note">Couldn't load metrics: {error}</div>{:else}<Loading
				label="Loading metrics…"
			/>{/if}
	{:else if data}
		{#if error}<div class="card err">Refresh failed: {error} — showing last snapshot.</div>{/if}

		<section class="card">
			<p class="lbl">Health</p>
			<div class="hlist">
				{#each data.health as h (h.level + h.message)}
					<div class="hrow">
						<span class="hico {h.level}"></span>
						<span>{h.message}</span>
					</div>
				{/each}
			</div>
		</section>

		<section class="card">
			<p class="lbl">Library</p>
			<div class="stats4">
				<div class="stat">
					<span class="k">Items</span><span class="v">{fmtNum(data.library.items)}</span>
				</div>
				<div class="stat">
					<span class="k">Series</span><span class="v">{fmtNum(data.library.series)}</span>
				</div>
				<div class="stat">
					<span class="k">Pages</span><span class="v">{fmtNum(data.library.pages)}</span>
				</div>
				<div class="stat">
					<span class="k">Tags</span><span class="v">{fmtNum(data.library.tags)}</span>
				</div>
			</div>
			{#if data.library.by_kind.length}
				{@const total = data.library.items}
				<div class="kindbar">
					{#each data.library.by_kind as k, i (k.kind)}
						<div
							class="kseg"
							style={`width:${(k.count / total) * 100}%;background:${kindColor(i)}`}
							title={`${kindLabel(k.kind)}: ${fmtNum(k.count)}`}
						></div>
					{/each}
				</div>
				<div class="klegend">
					{#each data.library.by_kind as k, i (k.kind)}
						<span class="kleg"
							><span class="kdot" style={`background:${kindColor(i)}`}></span>{kindLabel(k.kind)}
							{fmtNum(k.count)}</span
						>
					{/each}
				</div>
			{/if}
			<div class="bars">
				{#each [['Perceptual hash', data.library.phash_done, data.library.items], ['Neighbours', data.library.neighbours_done, data.library.neighbour_eligible], ['Search index', data.library.search_docs, data.library.items], ['Tagged', taggedItems, data.library.items]] as [label, done, total] (label)}
					<div class="barrow">
						<div class="barhead">
							<span class="muted">{label}</span><span>{fmtNum(done)} / {fmtNum(total)}</span>
						</div>
						<div class="bar">
							<div
								class="fill"
								class:warn={done < total}
								style={`width:${pct(done, total)}%`}
							></div>
						</div>
					</div>
				{/each}
			</div>
		</section>

		<section class="card">
			<p class="lbl">Background jobs</p>
			<div class="chips">
				<button
					class="chip"
					class:on={openState === 'pending'}
					onclick={() => toggleState('pending')}
					type="button">Pending</button
				>
				<button
					class="chip {jobTotals.failed ? 'bad' : ''}"
					class:on={openState === 'failed'}
					onclick={() => toggleState('failed')}
					type="button">Failed {jobTotals.failed}</button
				>
			</div>

			{#if openState === 'pending'}
				<div class="lines">
					{#if pendingByKind.length}
						{#each pendingByKind as c (c.kind)}
							<div class="line">
								<span class="muted">{c.kind}</span><span>{pendingWhen(c.run_after)}</span>
							</div>
						{/each}
					{:else}<p class="muted small pad">No pending jobs.</p>{/if}
				</div>
			{:else if openState === 'failed'}
				{#if visibleFailed.length}
					{#each visibleFailed as f, i (i)}
						{@const key = `${f.kind}:${f.at}:${f.attempts}`}
						<button
							class="failrow"
							class:expanded={expandedFails.has(key)}
							type="button"
							title={f.error ?? ''}
							onclick={() => toggleFail(key)}
						>
							<span class="fk">{f.kind}</span>
							<span class="fe">{f.error ?? `failed after ${f.attempts} attempts`}</span>
							<span class="ft" title={new Date(f.at * 1000).toLocaleString()}>{ago(f.at)}</span>
						</button>
					{/each}
					{#if data.jobs.failed.length > FAILED_SHOWN}
						<button class="linkbtn" type="button" onclick={() => (showAllFailed = !showAllFailed)}>
							{showAllFailed ? 'Show fewer' : `Show all (${data.jobs.failed.length})`}
						</button>
					{/if}
				{:else}<p class="muted small pad">No failed jobs.</p>{/if}
			{/if}

			<div class="lines" style="margin-top:0.7rem;">
				<div class="line">
					<span class="muted">Last scan</span><span>{ago(data.jobs.last_scan_at)}</span>
				</div>
			</div>
		</section>

		<section class="card">
			<p class="lbl">Accounts &amp; access</p>
			<div class="stats4">
				<div class="stat">
					<span class="k">Accounts</span><span class="v">{fmtNum(data.library.users)}</span>
				</div>
				<div class="stat">
					<span class="k">Admins</span><span class="v">{fmtNum(data.library.admins)}</span>
				</div>
				<div class="stat">
					<span class="k">Sessions</span><span class="v">{fmtNum(data.library.sessions)}</span>
				</div>
				<div class="stat">
					<span class="k">API keys</span><span class="v">{fmtNum(data.library.api_keys)}</span>
				</div>
			</div>
			<div class="lines" style="margin-top:0.7rem;">
				<div class="line">
					<span class="muted">Self sign-ups</span>
					<span class:open-posture={data.library.signup_enabled}
						>{data.library.signup_enabled ? 'Open' : 'Off'}</span
					>
				</div>
				<div class="line">
					<span class="muted">Guest browsing</span>
					<span class:open-posture={data.library.guest_enabled}
						>{data.library.guest_enabled ? 'Open' : 'Off'}</span
					>
				</div>
				<div class="line">
					<span class="muted">Visibility rules</span>
					<span
						>{data.library.hidden_kinds
							? `${data.library.hidden_kinds} hidden section${data.library.hidden_kinds === 1 ? '' : 's'}`
							: 'None'}</span
					>
				</div>
			</div>
		</section>

		<section class="card">
			<p class="lbl">Storage</p>
			<p class="line">
				<span class="muted">Library</span><span
					>{fmtBytes(data.library.bytes)} · {fmtNum(data.library.items)} items</span
				>
			</p>
			{#if data.library.by_kind.length && data.library.bytes > 0}
				<div class="kindbar">
					{#each data.library.by_kind as k, i (k.kind)}
						<div
							class="kseg"
							style={`width:${(k.bytes / data.library.bytes) * 100}%;background:${kindColor(i)}`}
							title={`${kindLabel(k.kind)}: ${fmtBytes(k.bytes)}`}
						></div>
					{/each}
				</div>
				<div class="klegend">
					{#each data.library.by_kind as k, i (k.kind)}
						<span class="kleg"
							><span class="kdot" style={`background:${kindColor(i)}`}></span>{kindLabel(k.kind)}
							{fmtBytes(k.bytes)}</span
						>
					{/each}
				</div>
			{/if}
			<p class="line" style="margin-top:0.7rem;">
				<span class="muted">Database</span>
				<span>
					{fmtBytes(data.storage.db_bytes)}
					{#if dbPct != null}
						· <span class="rating {dbRating(dbPct).cls}">{dbRating(dbPct).label}</span>
						<span class="muted"
							>({dbPct < 1 ? dbPct.toFixed(2) : dbPct.toFixed(1)}% of library)</span
						>
					{/if}
				</span>
			</p>
		</section>

		{#snippet sysCard(o)}
			<section class="card">
				<p class="lbl">{o.label}</p>
				<div class="sys">
					<div class="sysL">
						<div class="big">
							{#if o.bigLines}
								{#each o.bigLines as line, i (i)}
									<div>
										<span class="series-key" style={`color:${line.color}`}>{line.key}</span>
										{line.value}
									</div>
								{/each}
							{:else}
								{o.big}
							{/if}
						</div>
						{#if o.bar}
							<div class="vbar">
								<div
									class="vfill"
									style={`width:${Math.min(100, Math.max(0, o.bar.pct))}%;background:${o.bar.color}`}
								></div>
							</div>
						{/if}
						<div class="muted sub">
							{#if o.subParts}
								{#each o.subParts as part, i (i)}<span style:color={part.color ?? undefined}
										>{part.text}</span
									>{/each}
							{:else}
								{o.sub}
							{/if}
						</div>
					</div>
					<div class="chart">
						<svg
							class="plot"
							viewBox={`0 0 ${W} ${H}`}
							preserveAspectRatio="none"
							aria-hidden="true"
						>
							{#each [0, 0.25, 0.5, 0.75, 1] as f (f)}
								<line
									class="grid"
									x1="0"
									y1={H * (1 - f)}
									x2={W}
									y2={H * (1 - f)}
									vector-effect="non-scaling-stroke"
								/>
								<line
									class="grid"
									x1={W * f}
									y1="0"
									x2={W * f}
									y2={H}
									vector-effect="non-scaling-stroke"
								/>
							{/each}
							{#each o.series as sr (sr.color)}
								{@const sp = spark(sr.vals, o.yMax)}
								{#if sp.area}<polygon points={sp.area} fill={sr.color} fill-opacity="0.14" />{/if}
								{#if sp.line}<polyline
										points={sp.line}
										fill="none"
										stroke={sr.color}
										stroke-width="1.4"
										vector-effect="non-scaling-stroke"
									/>{/if}
							{/each}
						</svg>
						<div class="ylabels">
							{#each [1, 0.75, 0.5, 0.25, 0] as f (f)}<span style={`top:${(1 - f) * 100}%`}
									>{o.yFmt(f * o.yMax)}</span
								>{/each}
						</div>
						<div class="xlabels">
							{#each xTicks as t, i (i)}<span>{t}</span>{/each}
						</div>
					</div>
				</div>
			</section>
		{/snippet}

		{@const s = data.system}
		{#if s.cpu_pct != null || s.mem_rss != null}
			{@render sysCard({
				label: 'CPU',
				big: s.cpu_pct != null ? `${s.cpu_pct.toFixed(1)}%` : '—',
				sub:
					s.cores != null
						? `${s.cores % 1 ? s.cores.toFixed(1) : s.cores} cores allocated`
						: 'process CPU',
				series: [{ vals: hist.cpu, color: C.cpu }],
				yMax: 100,
				yFmt: (v) => `${Math.round(v)}`,
				bar: s.cpu_pct != null ? { pct: s.cpu_pct, color: C.cpu } : null,
			})}
			{@render sysCard({
				label: 'Memory',
				big: fmtBytes(s.mem_rss),
				sub: s.mem_limit != null ? `${fmtBytes(s.mem_limit)} limit` : 'no limit set',
				series: [{ vals: hist.mem, color: C.mem }],
				yMax: 100,
				yFmt: (v) =>
					s.mem_limit != null ? fmtBytes((v / 100) * s.mem_limit) : `${Math.round(v)}%`,
				bar:
					s.mem_rss != null && s.mem_limit
						? { pct: (s.mem_rss / s.mem_limit) * 100, color: C.mem }
						: null,
			})}
			{@render sysCard({
				label: 'Network',
				bigLines: [
					{ key: '↓', value: fmtRate(s.net.rx_rate), color: C.rx },
					{ key: '↑', value: fmtRate(s.net.tx_rate), color: C.tx },
				],
				subParts: [
					{ text: 'total ' },
					{ text: '↓', color: C.rx },
					{ text: ` ${fmtBytes(s.net.rx_total)} · ` },
					{ text: '↑', color: C.tx },
					{ text: ` ${fmtBytes(s.net.tx_total)}` },
				],
				series: [
					{ vals: hist.rx, color: C.rx },
					{ vals: hist.tx, color: C.tx },
				],
				yMax: netMax,
				yFmt: fmtBytes,
				bar: null,
			})}
			{@render sysCard({
				label: 'Disk',
				bigLines: [
					{ key: 'R', value: fmtRate(s.disk.read_rate), color: C.rd },
					{ key: 'W', value: fmtRate(s.disk.write_rate), color: C.wr },
				],
				sub: `pool ${s.db_pool.in_use}/${s.db_pool.size} conns`,
				series: [
					{ vals: hist.rd, color: C.rd },
					{ vals: hist.wr, color: C.wr },
				],
				yMax: diskMax,
				yFmt: fmtBytes,
				bar: null,
			})}
		{/if}
	{/if}
</div>

<style>
	.sh {
		display: flex;
		flex-direction: column;
		gap: var(--space-3);
	}
	.head {
		display: flex;
		align-items: flex-end;
		justify-content: space-between;
		gap: var(--space-3);
		flex-wrap: wrap;
		margin-bottom: var(--space-1);
	}
	.head h2 {
		margin: 0;
		font-family: var(--font-display);
		font-weight: 700;
		font-size: 1.35rem;
	}
	.muted {
		color: var(--muted);
	}
	.muted.sub,
	.small {
		font-size: 0.8rem;
	}
	.pad {
		margin: 0.4rem 0 0;
	}
	.head .muted {
		margin: 0.15rem 0 0;
		font-size: 0.82rem;
	}
	.live {
		display: inline-flex;
		align-items: center;
		gap: 0.4rem;
		font-size: 0.78rem;
		color: var(--muted);
	}
	.statuses {
		display: flex;
		align-items: center;
		gap: 0.5rem 1rem;
		flex-wrap: wrap;
	}
	.dot {
		width: 7px;
		height: 7px;
		border-radius: 50%;
		background: #3fb950;
		flex: 0 0 auto;
	}
	.rating {
		font-weight: 600;
	}
	.r-great {
		color: #3fb950;
	}
	.r-good {
		color: #a3e635;
	}
	.r-ok {
		color: #d29922;
	}
	.r-bad {
		color: #f0883e;
	}
	.r-poor {
		color: #f85149;
	}
	.open-posture {
		color: #d29922;
		font-weight: 600;
	}
	.card {
		border: 1px solid var(--border);
		border-radius: var(--radius-lg);
		background: color-mix(in srgb, var(--surface) 55%, transparent);
		padding: var(--space-4) var(--space-5);
	}
	.note {
		color: var(--muted);
	}
	.err {
		border-color: #f8514966;
		color: #f85149;
	}
	.lbl {
		margin: 0 0 var(--space-3);
		font-size: 0.72rem;
		text-transform: uppercase;
		letter-spacing: 0.14em;
		color: var(--muted);
	}
	.hlist {
		display: grid;
		gap: 0.5rem;
	}
	.hrow {
		display: flex;
		align-items: center;
		gap: 0.6rem;
		font-size: 0.9rem;
	}
	.hico {
		width: 8px;
		height: 8px;
		border-radius: 50%;
		flex: 0 0 auto;
	}
	.hico.ok {
		background: #3fb950;
	}
	.hico.warn {
		background: #d29922;
	}
	.hico.error {
		background: #f85149;
	}
	.stats4 {
		display: grid;
		grid-template-columns: repeat(4, 1fr);
		gap: 0.5rem;
	}
	.stat {
		background: var(--surface);
		border-radius: var(--radius);
		padding: 0.6rem 0.7rem;
		display: flex;
		flex-direction: column;
		gap: 0.15rem;
	}
	.stat .k {
		font-size: 0.72rem;
		color: var(--muted);
	}
	.stat .v {
		font-size: 1.3rem;
		font-weight: 600;
		font-variant-numeric: tabular-nums;
	}
	.kindbar {
		display: flex;
		height: 12px;
		margin-top: 0.8rem;
		border-radius: 6px;
		overflow: hidden;
		gap: 1px;
		background: var(--surface);
	}
	.kseg {
		height: 100%;
		min-width: 2px;
	}
	.klegend {
		display: flex;
		flex-wrap: wrap;
		gap: 0.35rem 0.9rem;
		margin-top: 0.55rem;
		font-size: 0.78rem;
		color: var(--muted);
	}
	.kleg {
		display: inline-flex;
		align-items: center;
		gap: 0.35rem;
	}
	.kdot {
		width: 9px;
		height: 9px;
		border-radius: 2px;
		flex: 0 0 auto;
	}
	.bars {
		display: grid;
		gap: 0.6rem;
		margin-top: var(--space-3);
	}
	.barrow {
		display: grid;
		gap: 0.3rem;
	}
	.barhead {
		display: flex;
		justify-content: space-between;
		font-size: 0.8rem;
		font-variant-numeric: tabular-nums;
	}
	.bar {
		height: 6px;
		border-radius: 3px;
		background: var(--surface);
		overflow: hidden;
	}
	.fill {
		height: 100%;
		background: #3fb950;
	}
	.fill.warn {
		background: #d29922;
	}
	.chips {
		display: flex;
		gap: 0.5rem;
		flex-wrap: wrap;
	}
	.chip {
		font-size: 0.78rem;
		font-weight: 600;
		font-family: inherit;
		line-height: 1.2;
		padding: 0.25rem 0.6rem;
		border-radius: var(--radius);
		border: 1px solid transparent;
		background: var(--surface);
		color: var(--muted);
		cursor: pointer;
		transition:
			border-color var(--ease),
			background var(--ease);
	}
	.chip:hover {
		border-color: color-mix(in srgb, currentColor 45%, transparent);
	}
	.chip.on {
		border-color: currentColor;
	}
	.chip.accent {
		background: var(--accent-soft);
		color: var(--accent);
	}
	.chip.bad {
		background: #f851491f;
		color: #f85149;
	}
	.lines {
		display: grid;
		gap: 0.35rem;
		margin-top: 0.7rem;
	}
	.line {
		display: flex;
		justify-content: space-between;
		font-size: 0.85rem;
		margin: 0;
	}
	.failrow {
		all: unset;
		box-sizing: border-box;
		display: flex;
		width: 100%;
		gap: 0.6rem;
		align-items: baseline;
		margin-top: 0.5rem;
		padding: 0.5rem 0.65rem;
		border-radius: var(--radius);
		background: #f851491a;
		font-size: 0.82rem;
		cursor: pointer;
		text-align: left;
	}
	.failrow:hover {
		background: #f8514929;
	}
	.failrow.expanded .fe {
		white-space: normal;
		overflow: visible;
		word-break: break-word;
	}
	.fk {
		flex: 0 0 auto;
		font-weight: 600;
		color: #f85149;
	}
	.fe {
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		color: var(--text);
	}
	.ft {
		margin-left: auto;
		flex: 0 0 auto;
		color: var(--muted);
		white-space: nowrap;
		font-variant-numeric: tabular-nums;
	}
	.linkbtn {
		all: unset;
		display: inline-block;
		margin-top: 0.5rem;
		font-size: 0.78rem;
		color: var(--muted);
		cursor: pointer;
	}
	.linkbtn:hover {
		color: var(--text);
	}
	.sys {
		display: grid;
		grid-template-columns: minmax(150px, 210px) 1fr;
		gap: 1.5rem;
		align-items: center;
	}
	.sysL .big {
		font-size: 1.15rem;
		font-weight: 600;
		font-variant-numeric: tabular-nums;
		white-space: pre-line;
		line-height: 1.25;
	}
	.series-key {
		display: inline-block;
		min-width: 1.1em;
	}
	.vbar {
		height: 6px;
		max-width: 170px;
		margin: 0.5rem 0;
		border-radius: 3px;
		background: var(--surface);
		overflow: hidden;
	}
	.vfill {
		height: 100%;
	}
	.sysL .sub {
		margin-top: 0.2rem;
	}
	.chart {
		position: relative;
		padding-right: 38px;
	}
	.plot {
		display: block;
		width: 100%;
		height: 116px;
	}
	.grid {
		stroke: var(--border);
		stroke-opacity: 0.55;
	}
	.ylabels {
		position: absolute;
		top: 0;
		right: 0;
		width: 38px;
		height: 116px;
	}
	.ylabels span {
		position: absolute;
		right: 3px;
		transform: translateY(-50%);
		font-size: 0.66rem;
		color: var(--muted);
		font-variant-numeric: tabular-nums;
		white-space: nowrap;
	}
	.xlabels {
		display: flex;
		justify-content: space-between;
		margin-top: 4px;
		font-size: 0.66rem;
		color: var(--muted);
		font-variant-numeric: tabular-nums;
	}
	@media (max-width: 560px) {
		.sys {
			grid-template-columns: 1fr;
			gap: 0.6rem;
		}
		.stats4 {
			grid-template-columns: repeat(2, 1fr);
		}
		.line {
			flex-direction: column;
			align-items: flex-start;
			gap: 0.15rem;
		}
	}
</style>
