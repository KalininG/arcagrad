import { writable } from 'svelte/store';

export const downloads = writable([]);

const DISMISS_OK_MS = 3000;
const DISMISS_ERR_MS = 6000;
const DISMISS_LINK_MS = 8000;

function scheduleDismiss(key, ms) {
	setTimeout(() => dismissDownload(key), ms);
}

export function beginDownload(key, name, sub = '') {
	downloads.update((list) => [
		...list.filter((d) => d.key !== key),
		{ key, name, state: 'progress', pct: null, received: 0, total: 0, note: '', sub, href: null },
	]);
}

export function setDownloadProgress(key, received, total) {
	downloads.update((list) =>
		list.map((d) =>
			d.key === key
				? {
						...d,
						received,
						total,
						pct: total ? Math.min(100, (received / total) * 100) : null,
					}
				: d,
		),
	);
}

export function finishDownload(key, ok, note = '', opts = {}) {
	downloads.update((list) =>
		list.map((d) =>
			d.key === key ? { ...d, state: ok ? 'done' : 'error', note, href: opts.href ?? null } : d,
		),
	);
	scheduleDismiss(key, !ok ? DISMISS_ERR_MS : opts.href ? DISMISS_LINK_MS : DISMISS_OK_MS);
}

export function dismissDownload(key) {
	downloads.update((list) => list.filter((d) => d.key !== key));
}
