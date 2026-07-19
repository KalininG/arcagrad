<script>
	import { page } from '$app/stores';
	import PluginDetail from '$lib/plugins/PluginDetail.svelte';
</script>

{#key $page.params.id}
	<PluginDetail id={$page.params.id} setup={$page.url.searchParams.get('setup') === '1'} />
{/key}
