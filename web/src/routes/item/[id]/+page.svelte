<script>
	import { page } from '$app/state';
	import ItemPreview from '#lib/library/ItemPreview.svelte';
</script>

<ItemPreview id={page.params.id} />
