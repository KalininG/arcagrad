<script>
	import { page } from '$app/stores';
	import ItemPreview from '$lib/library/ItemPreview.svelte';
</script>

<ItemPreview id={$page.params.id} />
