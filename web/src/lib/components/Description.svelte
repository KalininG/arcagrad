<script>
	let { text = '' } = $props();

	function clean(s) {
		if (!s) return '';
		return s
			.replace(/\s+/g, ' ')
			.replace(/\s*<\s*br\s*\/?\s*>\s*/gi, '\n')
			.replace(/\s*<\/\s*p\s*>\s*/gi, '\n\n')
			.replace(/<[^>]*>/g, '')
			.replace(/&nbsp;/gi, ' ')
			.replace(/&lt;/gi, '<')
			.replace(/&gt;/gi, '>')
			.replace(/&quot;/gi, '"')
			.replace(/&#0?39;|&apos;/gi, "'")
			.replace(/&amp;/gi, '&')
			.replace(/\n{3,}/g, '\n\n')
			.trim();
	}
	const body = $derived(clean(text));
</script>

{#if body}
	<p class="desc">{body}</p>
{/if}

<style>
	.desc {
		margin: var(--space-3) 0 var(--space-4);
		max-width: 68ch;
		white-space: pre-wrap;
		font-size: 0.8rem;
		line-height: 1.5;
		color: var(--muted);
	}
</style>
