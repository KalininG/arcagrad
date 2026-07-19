<script>
	let {
		value = $bindable(),
		options = [],
		placeholder = 'Select…',
		disabled = false,
		onchange,
	} = $props();

	let open = $state(false);

	const selected = $derived(options.find((o) => o.value === value));

	function pick(v) {
		open = false;
		if (v !== value) {
			value = v;
			onchange?.(v);
		}
	}
</script>

<div class="dd">
	<button
		type="button"
		class="trigger"
		{disabled}
		aria-haspopup="listbox"
		aria-expanded={open}
		onclick={() => (open = !open)}
	>
		<span class="cur">
			{#if selected}
				{#if selected.short}
					<span class="lbl-full">{selected.label}</span><span class="lbl-short"
						>{selected.short}</span
					>
				{:else}{selected.label}{/if}
			{:else}{placeholder}{/if}
		</span>
		<svg
			class="chev"
			class:open
			viewBox="0 0 20 20"
			fill="none"
			stroke="currentColor"
			stroke-width="2"
		>
			<path d="M5 7l5 5 5-5" stroke-linecap="round" stroke-linejoin="round" />
		</svg>
	</button>

	{#if open}
		<!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
		<div class="catch" onclick={() => (open = false)}></div>
		<ul class="menu" role="listbox">
			{#each options as o (o.value)}
				<li>
					<button
						type="button"
						class="opt"
						class:sel={o.value === value}
						role="option"
						aria-selected={o.value === value}
						onclick={() => pick(o.value)}
					>
						<span class="lbl">{o.label}</span>
						{#if o.value === value}
							<svg
								class="tick"
								viewBox="0 0 20 20"
								fill="none"
								stroke="currentColor"
								stroke-width="2.5"
							>
								<path d="M4 10l4 4 8-8" stroke-linecap="round" stroke-linejoin="round" />
							</svg>
						{/if}
					</button>
				</li>
			{/each}
		</ul>
	{/if}
</div>

<style>
	.dd {
		position: relative;
	}
	.trigger {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--space-2);
		width: 100%;
		padding: 0.45rem 0.6rem;
		border: 1px solid var(--border);
		border-radius: var(--radius-sm);
		background: var(--surface-2);
		color: var(--text);
		font-size: 0.9rem;
	}
	.trigger:hover:not(:disabled),
	.trigger:focus-visible {
		border-color: var(--accent);
		outline: none;
	}
	@media (max-width: 640px) {
		.trigger {
			padding: 0.45rem 0.4rem;
			gap: var(--space-1);
		}
	}
	.cur {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.lbl-short {
		display: none;
	}
	@media (max-width: 400px) {
		.lbl-full {
			display: none;
		}
		.lbl-short {
			display: inline;
		}
	}
	.chev {
		width: 0.85rem;
		height: 0.85rem;
		flex: 0 0 auto;
		color: var(--muted);
		transition: transform var(--ease);
	}
	.chev.open {
		transform: rotate(180deg);
	}

	.catch {
		position: fixed;
		inset: 0;
		z-index: 10;
	}
	.menu {
		position: absolute;
		left: 0;
		right: 0;
		top: calc(100% + 4px);
		z-index: 20;
		margin: 0;
		padding: var(--space-1);
		list-style: none;
		max-height: 16rem;
		overflow-y: auto;
		background: var(--surface);
		border: 1px solid var(--border);
		border-radius: var(--radius-sm);
		box-shadow: var(--shadow-lg);
	}
	.menu li {
		list-style: none;
	}
	.opt {
		all: unset;
		box-sizing: border-box;
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--space-3);
		width: 100%;
		padding: var(--space-2) var(--space-3);
		border-radius: var(--radius-sm);
		cursor: pointer;
		font-size: 0.9rem;
		color: var(--text);
	}
	.opt:hover {
		background: var(--surface-2);
	}
	.opt.sel {
		color: var(--accent);
	}
	.lbl {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.tick {
		width: 0.85rem;
		height: 0.85rem;
		flex: 0 0 auto;
	}
</style>
