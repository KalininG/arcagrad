<script>
	import { onMount } from 'svelte';
	import { series as seriesApi, kinds as kindsApi, media } from '$lib/api.js';
	import Modal from '$lib/components/ui/Modal.svelte';

	let { seriesId, kind, onClose } = $props();
	let providers = $state([]);
	let values = $state({});
	let saved = $state({});
	let busy = $state('');
	let loading = $state(true);
	let error = $state('');
	let notice = $state('');

	const inputFor = (provider) =>
		provider.reference_inputs?.calendar ?? {
			label: 'Series reference',
			placeholder: 'Paste the source series URL or identifier',
			help: 'The calendar plugin uses this reference to check releases.',
			required: true,
		};

	onMount(async () => {
		try {
			const [kindPlugins, trackers] = await Promise.all([
				kindsApi.plugins(kind),
				seriesApi.trackers(seriesId),
			]);
			providers = (kindPlugins ?? []).filter(
				(p) => p.enabled && p.capabilities?.includes('calendar'),
			);
			const current = Object.fromEntries((trackers ?? []).map((t) => [t.plugin_id, t.reference]));
			saved = current;
			values = { ...current };
		} catch (e) {
			error = e?.message ?? String(e);
		} finally {
			loading = false;
		}
	});

	async function save(provider) {
		const reference = (values[provider.id] ?? '').trim();
		if (!reference) {
			error = `${inputFor(provider).label} is required.`;
			return;
		}
		busy = provider.id;
		error = '';
		notice = '';
		try {
			await seriesApi.setTracker(seriesId, provider.id, reference);
			saved = { ...saved, [provider.id]: reference };
			values = { ...values, [provider.id]: reference };
			notice = `${provider.name} tracking saved. Release data will refresh shortly.`;
		} catch (e) {
			error = e?.message ?? String(e);
		} finally {
			busy = '';
		}
	}

	async function remove(provider) {
		busy = provider.id;
		error = '';
		notice = '';
		try {
			await seriesApi.removeTracker(seriesId, provider.id);
			const next = { ...saved };
			delete next[provider.id];
			saved = next;
			values = { ...values, [provider.id]: '' };
			notice = `${provider.name} tracking removed.`;
		} catch (e) {
			error = e?.message ?? String(e);
		} finally {
			busy = '';
		}
	}
</script>

<Modal
	title="Track releases"
	subtitle="Connect this series to a publisher or release calendar."
	width="min(600px, 100%)"
	busy={!!busy}
	{onClose}
>
	<div class="body">
		{#if loading}
			<p class="muted">Loading trackers…</p>
		{:else if providers.length === 0}
			<div class="empty">
				<strong>No release trackers enabled</strong>
				<p>Enable a calendar plugin for this library type in Plugins.</p>
			</div>
		{:else}
			{#each providers as provider (provider.id)}
				{@const input = inputFor(provider)}
				<section class="provider">
					<div class="providerhead">
						<img src={media.pluginIcon(provider.id)} alt="" />
						<div>
							<strong>{provider.name}</strong><span
								>{saved[provider.id] ? 'Tracking' : 'Not connected'}</span
							>
						</div>
					</div>
					<label for={`tracker-${provider.id}`}>{input.label}</label>
					<input
						id={`tracker-${provider.id}`}
						type="url"
						bind:value={values[provider.id]}
						placeholder={input.placeholder}
						disabled={busy === provider.id}
					/>
					<p class="help">{input.help}</p>
					<div class="actions">
						{#if saved[provider.id]}
							<button
								class="remove"
								type="button"
								disabled={!!busy}
								onclick={() => remove(provider)}>Remove</button
							>
						{/if}
						<span></span>
						<button class="save" type="button" disabled={!!busy} onclick={() => save(provider)}
							>{busy === provider.id
								? 'Saving…'
								: saved[provider.id]
									? 'Update'
									: 'Start tracking'}</button
						>
					</div>
				</section>
			{/each}
		{/if}
		{#if error}<p class="message err">{error}</p>{/if}
		{#if notice}<p class="message ok">{notice}</p>{/if}
	</div>
</Modal>

<style>
	.body {
		padding: var(--space-5);
		display: flex;
		flex-direction: column;
		gap: var(--space-4);
	}
	.provider {
		padding: var(--space-4);
		border: 1px solid var(--border);
		border-radius: var(--radius);
		background: var(--surface-2);
	}
	.providerhead {
		display: flex;
		align-items: center;
		gap: var(--space-3);
		margin-bottom: var(--space-4);
	}
	.providerhead img {
		width: 42px;
		height: 42px;
		object-fit: cover;
		border-radius: var(--radius-sm);
		background: var(--surface);
	}
	.providerhead div {
		display: flex;
		flex-direction: column;
	}
	.providerhead span {
		color: var(--muted);
		font-size: 0.72rem;
	}
	label {
		display: block;
		margin-bottom: var(--space-2);
		color: var(--muted);
		font-size: 0.7rem;
		letter-spacing: 0.08em;
		text-transform: uppercase;
	}
	input {
		width: 100%;
		box-sizing: border-box;
		padding: 0.65rem 0.75rem;
		color: var(--text);
		background: var(--surface);
		border: 1px solid var(--border);
		border-radius: var(--radius-sm);
		font: inherit;
		font-size: 0.86rem;
	}
	input:focus {
		outline: none;
		border-color: var(--accent);
	}
	.help {
		margin: var(--space-2) 0 0;
		color: var(--muted);
		font-size: 0.72rem;
		line-height: 1.45;
	}
	.actions {
		display: flex;
		align-items: center;
		gap: var(--space-2);
		margin-top: var(--space-4);
	}
	.actions span {
		flex: 1;
	}
	.actions button {
		padding: 0.55rem 0.9rem;
	}
	.save {
		color: #fff;
		background: var(--accent);
		border-color: var(--accent);
		font-weight: 600;
	}
	.remove {
		color: var(--bad, #e0566f);
		background: transparent;
		border-color: color-mix(in srgb, var(--bad, #e0566f) 55%, var(--border));
	}
	.empty {
		text-align: center;
		padding: var(--space-6);
		border: 1px dashed var(--border);
		border-radius: var(--radius);
	}
	.empty p,
	.muted {
		color: var(--muted);
	}
	.empty p {
		margin: 0.4rem 0 0;
		font-size: 0.8rem;
	}
	.message {
		margin: 0;
		font-size: 0.8rem;
	}
	.err {
		color: var(--bad, #e0566f);
	}
	.ok {
		color: var(--good);
	}
</style>
