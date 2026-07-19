<script>
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import { downloads, setDownloadProgress, dismissDownload } from '$lib/downloads.js';

	function onBubbleClick(d) {
		if (d.state === 'done' && d.href) {
			dismissDownload(d.key);
			goto(d.href);
		}
	}

	onMount(() => {
		const listen = globalThis.__TAURI__?.event?.listen;
		if (!listen) return;
		let unlisten;
		listen('arca:download-progress', (e) => {
			const { id, received, total } = e.payload ?? {};
			if (id == null) return;
			setDownloadProgress(`item-${id}`, received ?? 0, total ?? 0);
		}).then((u) => (unlisten = u));
		return () => unlisten?.();
	});

	const mb = (n) => (n / (1024 * 1024)).toFixed(n >= 100 * 1024 * 1024 ? 0 : 1);
	const sizeLine = (d) =>
		d.total ? `Saving… ${mb(d.received)}/${mb(d.total)} MB` : d.sub || 'Saving…';
</script>

{#if $downloads.length}
	<div class="bubbles" role="status" aria-live="polite">
		{#each $downloads as d (d.key)}
			<!-- svelte-ignore a11y_no_static_element_interactions, a11y_click_events_have_key_events -->
			<div
				class="bubble"
				class:done={d.state === 'done'}
				class:error={d.state === 'error'}
				class:tappable={d.state === 'done' && !!d.href}
				onclick={() => onBubbleClick(d)}
			>
				<span class="ico" aria-hidden="true">
					{#if d.state === 'progress'}
						<span class="ring"></span>
					{:else if d.state === 'done'}
						<svg
							viewBox="0 0 20 20"
							fill="none"
							stroke="currentColor"
							stroke-width="2.5"
							stroke-linecap="round"
							stroke-linejoin="round"><path d="M4 10l4 4 8-8" /></svg
						>
					{:else if d.state === 'error'}
						<svg
							viewBox="0 0 20 20"
							fill="none"
							stroke="currentColor"
							stroke-width="2.5"
							stroke-linecap="round"><path d="M5 5l10 10M15 5L5 15" /></svg
						>
					{:else}
						<svg
							viewBox="0 0 24 24"
							fill="none"
							stroke="currentColor"
							stroke-width="2"
							stroke-linecap="round"
							stroke-linejoin="round"
						>
							<path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" />
							<path d="M7 10l5 5 5-5" />
							<path d="M12 15V3" />
						</svg>
					{/if}
				</span>
				<div class="body">
					<span class="name">{d.name}</span>
					{#if d.state === 'progress'}
						<span class="sub">{sizeLine(d)}</span>
						<div class="track">
							<div
								class="fill"
								class:indet={d.pct == null}
								style={d.pct != null ? `width:${d.pct}%` : ''}
							></div>
						</div>
					{:else if d.state === 'handoff'}
						<span class="sub">Sent to your browser's downloads</span>
					{:else if d.state === 'done'}
						<span class="sub"
							>{d.href ? `${d.note || 'Tap to open'} →` : d.note || 'Downloaded'}</span
						>
					{:else}
						<span class="sub">{d.note || 'Download failed'}</span>
					{/if}
				</div>
				<button
					class="close"
					type="button"
					aria-label="Dismiss"
					onclick={(e) => {
						e.stopPropagation();
						dismissDownload(d.key);
					}}
				>
					<svg
						viewBox="0 0 20 20"
						fill="none"
						stroke="currentColor"
						stroke-width="2"
						stroke-linecap="round"><path d="M5 5l10 10M15 5L5 15" /></svg
					>
				</button>
			</div>
		{/each}
	</div>
{/if}

<style>
	.bubbles {
		position: fixed;
		top: calc(var(--space-4) + env(safe-area-inset-top, 0px));
		right: var(--space-4);
		z-index: 200;
		display: flex;
		flex-direction: column;
		gap: var(--space-2);
		width: min(340px, calc(100vw - 2 * var(--space-4)));
	}
	.bubble {
		display: flex;
		align-items: center;
		gap: var(--space-3);
		width: 100%;
		padding: var(--space-3) var(--space-4);
		border: 1px solid var(--border);
		border-radius: var(--radius);
		background: var(--surface);
		box-shadow: var(--shadow, 0 4px 16px rgba(0, 0, 0, 0.35));
		transition:
			background var(--ease),
			border-color var(--ease);
	}
	.bubble.done {
		background: color-mix(in srgb, var(--good, #46a758) 14%, var(--surface));
		border-color: color-mix(in srgb, var(--good, #46a758) 55%, var(--border));
	}
	.bubble.error {
		background: color-mix(in srgb, var(--bad, #e5484d) 14%, var(--surface));
		border-color: color-mix(in srgb, var(--bad, #e5484d) 55%, var(--border));
	}
	.bubble.tappable {
		cursor: pointer;
	}
	.bubble.tappable:hover {
		background: color-mix(in srgb, var(--good, #46a758) 22%, var(--surface));
	}
	.ico {
		flex: 0 0 auto;
		display: inline-flex;
		width: 1.15rem;
		height: 1.15rem;
		color: var(--muted);
	}
	.ico svg {
		width: 100%;
		height: 100%;
	}
	.bubble.done .ico {
		color: var(--good, #46a758);
	}
	.bubble.error .ico {
		color: var(--bad, #e5484d);
	}
	.ring {
		width: 100%;
		height: 100%;
		border: 2px solid color-mix(in srgb, var(--accent) 18%, transparent);
		border-top-color: var(--accent);
		border-radius: 50%;
		animation: spin 0.8s linear infinite;
	}
	.body {
		min-width: 0;
		flex: 1;
		display: flex;
		flex-direction: column;
		gap: 3px;
	}
	.name {
		font-size: 0.82rem;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.sub {
		font-size: 0.72rem;
		color: var(--muted);
		font-variant-numeric: tabular-nums;
	}
	.bubble.done .sub,
	.bubble.error .sub {
		color: var(--text);
	}
	.track {
		height: 4px;
		border-radius: 9999px;
		background: var(--surface-2);
		overflow: hidden;
	}
	.fill {
		height: 100%;
		background: var(--accent);
		transition: width 0.2s ease;
	}
	.fill.indet {
		width: 35%;
		animation: sweep 1.1s ease-in-out infinite;
	}
	@keyframes sweep {
		0% {
			transform: translateX(-100%);
		}
		100% {
			transform: translateX(300%);
		}
	}
	.close {
		all: unset;
		flex: 0 0 auto;
		display: inline-flex;
		width: 0.85rem;
		height: 0.85rem;
		color: var(--muted);
		cursor: pointer;
	}
	.close:hover {
		color: var(--text);
	}
	.close svg {
		width: 100%;
		height: 100%;
	}
</style>
