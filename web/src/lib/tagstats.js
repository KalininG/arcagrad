import { writable, get } from 'svelte/store';
import { tags as tagsApi } from '$lib/api.js';

export const tagCounts = writable(null);

let inflight = null;

async function fetchCounts() {
	const list = await tagsApi.list();
	const m = new Map();
	for (const t of list) m.set(`${t.namespace}:${t.value}`, t.count);
	tagCounts.set(m);
}

export function ensureTagCounts() {
	if (get(tagCounts)) return Promise.resolve();
	if (!inflight)
		inflight = fetchCounts()
			.catch(() => {})
			.finally(() => (inflight = null));
	return inflight;
}

export function refreshTagCounts() {
	inflight = fetchCounts()
		.catch(() => {})
		.finally(() => (inflight = null));
	return inflight;
}
