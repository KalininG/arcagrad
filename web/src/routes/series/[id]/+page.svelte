<script>
	import { page } from '$app/state';
	import SeriesPreview from '#lib/library/SeriesPreview.svelte';
</script>

<SeriesPreview id={page.params.id} />
