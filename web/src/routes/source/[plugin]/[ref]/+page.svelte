<script>
	import { page } from '$app/stores';
	import SourcePreview from '$lib/browse/SourcePreview.svelte';

	const plugin = $derived(decodeURIComponent($page.params.plugin));
	const reference = $derived(decodeURIComponent($page.params.ref));
	const kind = $derived($page.url.searchParams.get('kind') ?? '');
	const back = $derived($page.url.searchParams.get('back') ?? '');
</script>

{#key plugin + ':' + reference}
	<SourcePreview {plugin} {reference} {kind} {back} />
{/key}
