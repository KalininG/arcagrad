<script>
	import { page } from '$app/stores';
	import SourceBrowse from '$lib/browse/SourceBrowse.svelte';

	const kind = $derived(decodeURIComponent($page.params.kind));
</script>

{#key kind}
	<SourceBrowse {kind} />
{/key}
