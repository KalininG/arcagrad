<script>
	import { page } from '$app/stores';
	import Library from '$lib/library/Library.svelte';

	const kind = $derived(decodeURIComponent($page.params.kind));
</script>

{#key kind}
	<Library {kind} />
{/key}
