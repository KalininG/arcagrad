import { writable } from 'svelte/store';

const STORAGE_KEY = 'arca:theme';
const DEFAULT = 'dark';

function initial() {
	if (typeof document !== 'undefined' && document.documentElement.dataset.theme) {
		return document.documentElement.dataset.theme;
	}
	try {
		const t = localStorage.getItem(STORAGE_KEY);
		if (t === 'light' || t === 'dark') return t;
	} catch {
		/* ignored */
	}
	return DEFAULT;
}

export const theme = writable(initial());

export function applyTheme(name) {
	const t = name === 'light' ? 'light' : 'dark';
	if (typeof document !== 'undefined') document.documentElement.dataset.theme = t;
	try {
		localStorage.setItem(STORAGE_KEY, t);
	} catch {
		/* ignored */
	}
	theme.set(t);
}
