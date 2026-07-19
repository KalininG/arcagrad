import { writable } from 'svelte/store';
import { plugins as pluginsApi } from '$lib/api.js';

let cached = null;
export async function loadCatalog() {
	if (!cached) return reloadCatalog();
	return cached;
}

export async function reloadCatalog() {
	const rows = (await pluginsApi.catalog()) ?? [];
	cached = rows.map((r) => ({
		...r.plugin,
		installed: r.installed,
		last_error: r.last_error,
		repo_url: r.repo_url ?? null,
		update_available: r.update_available ?? null,
	}));
	syncInstalls();
	return cached;
}
export const catalogById = (id) => (cached ?? []).find((p) => p.id === id) ?? null;

export const installs = writable({});
export const updateCount = writable(0);
function syncInstalls() {
	const m = {};
	let updates = 0;
	for (const p of cached ?? []) {
		if (p.installed) {
			m[p.id] = { version: p.version };
			if (p.update_available) updates += 1;
		}
	}
	installs.set(m);
	updateCount.set(updates);
}

export const realVersion = (v) => (v && v !== '0.0.0' ? v : '');
export const realAuthor = (a) => (a && a !== 'Unknown' ? a : '');

const GH_USER = /^[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,37}[a-zA-Z0-9])?$/;
export const authorProfile = (a) =>
	realAuthor(a) && GH_USER.test(a) ? `https://github.com/${a}` : '';

export async function update(id) {
	await pluginsApi.install(id);
	await reloadCatalog();
}

export async function install(id) {
	await pluginsApi.install(id);
	const entry = catalogById(id);
	if (entry) {
		entry.installed = true;
		entry.last_error = null;
	}
	syncInstalls();
}

export async function uninstall(id) {
	await pluginsApi.uninstall(id);
	await reloadCatalog();
}

export async function installFromFile(file) {
	const { id } = await pluginsApi.installFile(file);
	await reloadCatalog();
	return id;
}
