import { writable } from 'svelte/store';
import { follows as followsApi } from '$lib/api.js';

export const followNewCount = writable(0);

export async function loadFollows() {
	const rows = (await followsApi.list()) ?? [];
	followNewCount.set(rows.reduce((n, w) => n + (w.new_count ?? 0), 0));
	return rows;
}
