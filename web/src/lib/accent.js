import { writable } from 'svelte/store';

const STORAGE_KEY = 'arca:accent';

export const DEFAULT_ACCENT = '#c1432d';

export const ACCENTS = [
	{ id: 'orange', label: 'Orange', value: '#c1432d' },
	{ id: 'amber', label: 'Amber', value: '#d97706' },
	{ id: 'yellow', label: 'Yellow', value: '#ca8a04' },
	{ id: 'lime', label: 'Lime', value: '#4d7c0f' },
	{ id: 'emerald', label: 'Emerald', value: '#0e8a5f' },
	{ id: 'teal', label: 'Teal', value: '#0d9488' },
	{ id: 'cyan', label: 'Cyan', value: '#0e9cc4' },
	{ id: 'blue', label: 'Blue', value: '#2f6df0' },
	{ id: 'indigo', label: 'Indigo', value: '#5b4fd0' },
	{ id: 'violet', label: 'Violet', value: '#8a3ff0' },
	{ id: 'purple', label: 'Purple', value: '#9b46f0' },
	{ id: 'fuchsia', label: 'Fuchsia', value: '#bf2fce' },
	{ id: 'pink', label: 'Pink', value: '#e0457f' },
	{ id: 'rose', label: 'Rose', value: '#cf1d3f' },
];

function initial() {
	try {
		return localStorage.getItem(STORAGE_KEY) || DEFAULT_ACCENT;
	} catch {
		return DEFAULT_ACCENT;
	}
}

export const accent = writable(initial());

export function applyAccent(value) {
	const v = value || DEFAULT_ACCENT;
	if (typeof document !== 'undefined') {
		if (v === DEFAULT_ACCENT) document.documentElement.style.removeProperty('--accent');
		else document.documentElement.style.setProperty('--accent', v);
	}
	try {
		if (v === DEFAULT_ACCENT) localStorage.removeItem(STORAGE_KEY);
		else localStorage.setItem(STORAGE_KEY, v);
	} catch {
		/* ignored */
	}
	accent.set(v);
}

export function resetAccent() {
	applyAccent(DEFAULT_ACCENT);
}
