<script>
	import { page } from '$app/state';
	import TagsView from '#lib/tags/TagsView.svelte';

	const kind = $derived(decodeURIComponent(page.params.kind));
</script>

{#key kind}
	<TagsView {kind} />
{/key}
