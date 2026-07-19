<script>
	import { onMount } from 'svelte';
	import { checkForUpdate, installUpdate, isDesktop } from '$lib/updates.js';

	let update = $state(null);
	let installing = $state(false);
	let error = $state('');

	onMount(async () => {
		if (!isDesktop()) return;
		update = await checkForUpdate();
	});

	async function install() {
		installing = true;
		error = '';
		try {
			await installUpdate();
		} catch (e) {
			error = typeof e === 'string' ? e : (e?.message ?? 'Update failed.');
			installing = false;
		}
	}
</script>

{#if update}
	<div class="update" role="status">
		<div class="body">
			<strong>Update available</strong>
			<span class="ver">
				v{update.version}
				<span class="muted">· you have v{update.current_version}</span>
			</span>
			{#if error}<span class="err">{error}</span>{/if}
		</div>
		<div class="actions">
			<button class="primary" onclick={install} disabled={installing}>
				{installing ? 'Installing…' : 'Install & Restart'}
			</button>
			{#if !installing}
				<button class="ghost" onclick={() => (update = null)}>Later</button>
			{/if}
		</div>
	</div>
{/if}

<style>
	.update {
		position: fixed;
		bottom: calc(var(--space-4) + env(safe-area-inset-bottom, 0px));
		right: var(--space-4);
		z-index: 200;
		width: min(22rem, calc(100vw - 2 * var(--space-4)));
		display: flex;
		flex-direction: column;
		gap: 0.6rem;
		background: var(--surface);
		border: 1px solid var(--border);
		border-radius: 12px;
		padding: 0.9rem 1rem;
		box-shadow: 0 6px 24px rgba(0, 0, 0, 0.18);
	}
	.body {
		display: flex;
		flex-direction: column;
		gap: 0.15rem;
	}
	.body strong {
		color: var(--text);
		font-weight: 600;
	}
	.ver {
		color: var(--text);
		font-size: 0.9rem;
	}
	.muted {
		color: var(--muted);
	}
	.err {
		color: var(--accent);
		font-size: 0.85rem;
	}
	.actions {
		display: flex;
		gap: 0.5rem;
		justify-content: flex-end;
	}
	.actions button {
		font-weight: 600;
	}
	.actions .primary {
		background: var(--accent);
		color: #fff;
		border-color: var(--accent);
	}
	.actions .ghost {
		background: transparent;
	}
</style>
