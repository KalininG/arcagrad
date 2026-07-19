<script>
	let { page = 1, pageCount = 1, simple = false, jump = false, onnavigate } = $props();

	function go(p) {
		const clamped = Math.max(1, Math.min(p, pageCount));
		if (clamped !== page) onnavigate?.(clamped);
	}

	let inputVal = $state(String(page));
	$effect(() => {
		inputVal = String(page);
	});
	function commitJump() {
		const n = parseInt(inputVal, 10);
		if (Number.isFinite(n)) go(n);
		else inputVal = String(page);
	}
	function onJumpKey(e) {
		if (e.key === 'Enter') {
			e.preventDefault();
			commitJump();
			e.currentTarget.blur();
		} else if (e.key === 'Escape') {
			inputVal = String(page);
			e.currentTarget.blur();
		}
	}

	const list = $derived(simple ? [] : build(page, pageCount));
	function build(cur, total) {
		const wanted = new Set([1, total, cur, cur - 1, cur + 1]);
		const pages = [...wanted].filter((p) => p >= 1 && p <= total).sort((a, b) => a - b);
		const out = [];
		let prev = 0;
		for (const p of pages) {
			if (p - prev > 1) out.push('…');
			out.push(p);
			prev = p;
		}
		return out;
	}
</script>

{#if jump}
	<nav class="pg simple" aria-label="Pagination">
		<button class="step" onclick={() => go(page - 1)} disabled={page <= 1}>← Prev</button>
		<span class="jind">
			<span>Page</span>
			<input
				class="pageinput"
				type="text"
				inputmode="numeric"
				aria-label="Page number"
				bind:value={inputVal}
				onkeydown={onJumpKey}
				onblur={commitJump}
			/>
			<span>/ {pageCount}</span>
		</span>
		<button class="step" onclick={() => go(page + 1)} disabled={page >= pageCount}>Next →</button>
	</nav>
{:else if simple}
	<nav class="pg simple" aria-label="Pagination">
		<button class="step" onclick={() => go(page - 1)} disabled={page <= 1}>← Prev</button>
		<span class="ind">{page} / {pageCount}</span>
		<button class="step" onclick={() => go(page + 1)} disabled={page >= pageCount}>Next →</button>
	</nav>
{:else}
	<nav class="pg" aria-label="Pagination">
		<button
			class="pgbtn"
			onclick={() => go(page - 1)}
			disabled={page <= 1}
			aria-label="Previous page"
		>
			<svg
				viewBox="0 0 20 20"
				fill="none"
				stroke="currentColor"
				stroke-width="2"
				stroke-linecap="round"
				stroke-linejoin="round"><path d="M12 5l-5 5 5 5" /></svg
			>
		</button>

		{#each list as it, i (it === '…' ? `gap-${i}` : it)}
			{#if it === '…'}
				<span class="ellipsis">…</span>
			{:else}
				<button
					class="pgbtn num"
					class:active={it === page}
					aria-current={it === page ? 'page' : undefined}
					onclick={() => go(it)}
				>
					{it}
				</button>
			{/if}
		{/each}

		<button
			class="pgbtn"
			onclick={() => go(page + 1)}
			disabled={page >= pageCount}
			aria-label="Next page"
		>
			<svg
				viewBox="0 0 20 20"
				fill="none"
				stroke="currentColor"
				stroke-width="2"
				stroke-linecap="round"
				stroke-linejoin="round"><path d="M8 5l5 5-5 5" /></svg
			>
		</button>
	</nav>
{/if}

<style>
	.pg {
		display: flex;
		align-items: center;
		gap: var(--space-1);
		flex-wrap: wrap;
	}
	.pgbtn {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		min-width: 2.25rem;
		height: 2.25rem;
		padding: 0 var(--space-2);
		background: var(--surface);
		font-size: 0.85rem;
		font-variant-numeric: tabular-nums;
	}
	.pgbtn svg {
		width: 1rem;
		height: 1rem;
	}
	.pgbtn.num {
		font-variant-numeric: tabular-nums;
	}
	.pgbtn.active {
		background: var(--accent);
		border-color: var(--accent);
		color: #fff;
		font-weight: 600;
	}
	.pgbtn.active:hover {
		border-color: var(--accent);
	}
	.ellipsis {
		padding: 0 var(--space-1);
		color: var(--muted);
		user-select: none;
	}

	.simple {
		gap: var(--space-2);
	}
	.step {
		background: var(--surface);
		font-size: 0.82rem;
		padding: 0.2rem var(--space-2);
		min-height: 1.7rem;
	}
	.jind {
		display: inline-flex;
		align-items: center;
		gap: var(--space-2);
		font-size: 0.82rem;
		color: var(--muted);
		white-space: nowrap;
	}
	.pageinput {
		width: 2.6rem;
		text-align: center;
		padding: 0.2rem 0.3rem;
		border: 1px solid var(--border);
		border-radius: var(--radius-sm);
		background: var(--surface-2);
		color: var(--text);
		font-size: 0.82rem;
		font-variant-numeric: tabular-nums;
	}
	.pageinput:focus {
		outline: none;
		border-color: var(--accent);
	}
	.ind {
		padding: 0 var(--space-1);
		font-size: 0.8rem;
		color: var(--muted);
		font-variant-numeric: tabular-nums;
		white-space: nowrap;
	}
</style>
