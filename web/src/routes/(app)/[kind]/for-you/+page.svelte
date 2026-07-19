<script>
	import { page } from '$app/stores';
	import ForYou from '$lib/library/ForYou.svelte';

	const kind = $derived(decodeURIComponent($page.params.kind));
</script>

{#key kind}
	<ForYou {kind} />
{/key}
