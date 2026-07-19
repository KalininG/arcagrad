<script>
	import { goto } from '$app/navigation';
	import { search as searchApi, media } from '$lib/api.js';
	import { kindLabel } from '$lib/kinds.js';
	import { trailingToken } from '$lib/librarySearch.js';

	let { value = $bindable(''), kind = null, oncommit } = $props();

	let suggestions = $state([]);
	let showSuggest = $state(false);
	let activeIdx = $state(-1);
	let suggestTimer;
	let blurTimer;

	function closeSuggest() {
		showSuggest = false;
		activeIdx = -1;
		suggestions = [];
	}

	function onSearchInput() {
		const text = value;
		if (text.trim() === '') {
			closeSuggest();
			clearTimeout(suggestTimer);
			oncommit('');
			return;
		}
		clearTimeout(suggestTimer);
		suggestTimer = setTimeout(() => fetchSuggest(text), 150);
	}

	async function fetchSuggest(text) {
		const { term } = trailingToken(text);
		if (!term) return closeSuggest();
		try {
			const res = await searchApi.suggest(term, 8, kind || undefined);
			if (text !== value) return;
			suggestions = res.results ?? [];
			showSuggest = suggestions.length > 0;
			activeIdx = -1;
		} catch {
			closeSuggest();
		}
	}

	function selectSuggestion(s) {
		if (s.type === 'title') {
			closeSuggest();
			goto(`/item/${s.id}`);
			return;
		}
		if (s.type === 'series') {
			closeSuggest();
			goto(`/series/${s.id}`);
			return;
		}
		const { start, negated } = trailingToken(value);
		let prefix = value.slice(0, start).trim();
		if (prefix && !prefix.endsWith(',')) prefix += ',';
		if (prefix) prefix += ' ';
		value = `${prefix}${negated ? '-' : ''}${s.namespace}:${s.value}`;
		closeSuggest();
		oncommit(value);
	}

	function onSearchKeydown(e) {
		if (showSuggest && suggestions.length) {
			if (e.key === 'ArrowDown') {
				e.preventDefault();
				activeIdx = (activeIdx + 1) % suggestions.length;
				return;
			}
			if (e.key === 'ArrowUp') {
				e.preventDefault();
				activeIdx = (activeIdx - 1 + suggestions.length) % suggestions.length;
				return;
			}
			if (e.key === 'Enter' && activeIdx >= 0) {
				e.preventDefault();
				selectSuggestion(suggestions[activeIdx]);
				return;
			}
			if (e.key === 'Escape') {
				closeSuggest();
				return;
			}
		}
		if (e.key === 'Enter') {
			closeSuggest();
			oncommit(value.trim());
		}
	}

	function onSearchBlur() {
		blurTimer = setTimeout(closeSuggest, 120);
	}
	function onSearchFocus() {
		clearTimeout(blurTimer);
		if (suggestions.length) showSuggest = true;
	}

	function clearSearch() {
		value = '';
		closeSuggest();
		oncommit('');
	}
</script>

<div class="searchbox">
	<svg
		class="search-ico"
		viewBox="0 0 24 24"
		fill="none"
		stroke="currentColor"
		stroke-width="2"
		stroke-linecap="round"
		stroke-linejoin="round"
		aria-hidden="true"
	>
		<circle cx="11" cy="11" r="7" />
		<path d="M21 21l-4.3-4.3" />
	</svg>
	<input
		class="search"
		type="text"
		placeholder="Search title or tags…  (e.g. author:melville, drama)"
		bind:value
		oninput={onSearchInput}
		onkeydown={onSearchKeydown}
		onblur={onSearchBlur}
		onfocus={onSearchFocus}
		role="combobox"
		aria-expanded={showSuggest}
		aria-autocomplete="list"
		aria-controls="tag-suggest"
	/>
	{#if value}
		<button class="clear" onclick={clearSearch} aria-label="Clear search" type="button">×</button>
	{/if}
	{#if showSuggest && suggestions.length}
		<ul class="suggest" id="tag-suggest" role="listbox">
			{#each suggestions as s, i (s.type === 'tag' ? 'g' + s.namespace + ':' + s.value : s.type + s.id)}
				<li role="option" aria-selected={i === activeIdx}>
					<button
						class="sg"
						class:active={i === activeIdx}
						type="button"
						onmousedown={(e) => e.preventDefault()}
						onclick={() => selectSuggestion(s)}
						onmouseenter={() => (activeIdx = i)}
					>
						{#if s.type === 'tag'}
							<span class="sg-tag"><span class="sg-ns">{s.namespace}:</span>{s.value}</span>
							<span class="sg-cnt">({s.count})</span>
						{:else}
							<img
								class="sg-cover"
								src={media.thumbnail(s.type === 'series' ? s.cover_item_id : s.id, s.cover_version)}
								alt=""
								loading="lazy"
							/>
							<span class="sg-title">{s.title}</span>
							<span class="sg-kind">{s.type === 'series' ? 'Series' : kindLabel(s.kind)}</span>
						{/if}
					</button>
				</li>
			{/each}
		</ul>
	{/if}
</div>

<style>
	.searchbox {
		position: relative;
		display: flex;
		align-items: center;
		width: 100%;
		max-width: 520px;
	}
	.search-ico {
		position: absolute;
		left: 0.6rem;
		width: 1rem;
		height: 1rem;
		color: var(--muted);
		pointer-events: none;
	}
	.search {
		width: 100%;
		padding-left: 2rem;
		padding-right: 1.9rem;
		border-radius: var(--radius-sm);
	}
	.clear {
		all: unset;
		position: absolute;
		right: 0.35rem;
		display: inline-flex;
		align-items: center;
		justify-content: center;
		width: 1.4rem;
		height: 1.4rem;
		border-radius: 9999px;
		color: var(--muted);
		font-size: 1.1rem;
		line-height: 1;
		cursor: pointer;
	}
	.clear:hover {
		color: var(--text);
		background: var(--surface-2);
	}

	.suggest {
		position: absolute;
		top: calc(100% + 4px);
		left: 0;
		right: 0;
		z-index: 30;
		margin: 0;
		padding: var(--space-1);
		list-style: none;
		background: var(--surface);
		border: 1px solid var(--border);
		border-radius: var(--radius-sm);
		box-shadow: var(--shadow-lg);
		max-height: 320px;
		overflow-y: auto;
	}
	.suggest li {
		list-style: none;
	}
	.sg {
		all: unset;
		box-sizing: border-box;
		width: 100%;
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--space-3);
		padding: var(--space-2) var(--space-3);
		border-radius: var(--radius-sm);
		cursor: pointer;
		font-size: 0.9rem;
		color: var(--text);
	}
	.sg.active {
		background: var(--surface-2);
	}
	.sg-tag {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.sg-ns {
		color: var(--muted);
	}
	.sg-cnt {
		flex: 0 0 auto;
		color: var(--muted);
		font-size: 0.8rem;
		font-variant-numeric: tabular-nums;
	}
	.sg-cover {
		flex: 0 0 auto;
		width: 26px;
		height: 34px;
		object-fit: cover;
		border-radius: 3px;
		background: var(--surface-2);
	}
	.sg-title {
		flex: 1 1 auto;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.sg-kind {
		flex: 0 0 auto;
		color: var(--muted);
		font-size: 0.78rem;
		text-transform: capitalize;
	}
</style>
