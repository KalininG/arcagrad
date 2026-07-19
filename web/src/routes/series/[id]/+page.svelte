<script>
	import { page } from '$app/stores';
	import SeriesPreview from '$lib/library/SeriesPreview.svelte';
</script>

<SeriesPreview id={$page.params.id} />
