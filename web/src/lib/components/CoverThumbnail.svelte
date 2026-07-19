<script>
	let { src = '', alt = '', eager: _eager = false, children, fallback } = $props();
	let failed = $state(false);
	$effect(() => {
		src;
		failed = false;
	});
</script>

<div class="cover">
	{#if src && !failed}
		<div class="art" role="img" aria-label={alt} style={`background-image:url("${src}")`}></div>
		{#if fallback}
			<img class="probe" {src} alt="" aria-hidden="true" onerror={() => (failed = true)} />
		{/if}
	{:else if fallback}
		{@render fallback()}
	{/if}
	{@render children?.()}
</div>

<style>
	.cover {
		position: relative;
		aspect-ratio: var(--cover-aspect);
		flex-shrink: 0;
		overflow: hidden;
		border: 1px solid var(--border);
		border-radius: var(--radius);
		background: var(--surface-2);
		box-shadow: var(--shadow);
	}
	.art {
		position: absolute;
		inset: 0;
		background-size: cover;
		background-position: center;
		background-repeat: no-repeat;
	}
	.probe {
		display: none;
	}
</style>
