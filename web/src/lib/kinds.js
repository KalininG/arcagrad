import { writable, derived, get } from 'svelte/store';
import { kinds as kindsApi } from '$lib/api.js';
import { currentUser } from '$lib/session.js';

export const kinds = writable([]);

const ORDER_KEY = 'arca:kind-order';
function loadOrder() {
	try {
		const arr = JSON.parse(localStorage.getItem(ORDER_KEY) || '[]');
		return Array.isArray(arr) ? arr.filter((k) => typeof k === 'string') : [];
	} catch {
		return [];
	}
}
export const kindOrder = writable(loadOrder());
export function setKindOrder(names) {
	kindOrder.set(names);
	try {
		localStorage.setItem(ORDER_KEY, JSON.stringify(names));
	} catch {
		/* ignored */
	}
}

export const orderedKinds = derived([kinds, kindOrder], ([$kinds, $order]) => {
	const pos = new Map($order.map((k, i) => [k, i]));
	return [...$kinds].sort((a, b) => {
		const ai = pos.has(a.kind) ? pos.get(a.kind) : Infinity;
		const bi = pos.has(b.kind) ? pos.get(b.kind) : Infinity;
		return ai !== bi ? ai - bi : 0;
	});
});

let started = false;
let lastRefresh = 0;

export async function refreshKinds() {
	if (!get(currentUser)) return;
	started = true;
	lastRefresh = Date.now();
	try {
		kinds.set(await kindsApi.list());
	} catch {
		/* ignored */
	}
}

export function refreshKindsSoon(minGapMs = 1500) {
	if (Date.now() - lastRefresh < minGapMs) return;
	refreshKinds();
}

export function ensureKinds() {
	if (started) return;
	started = true;
	refreshKinds();
}

export const kindLabel = (k) => (k ? k.charAt(0).toUpperCase() + k.slice(1) : 'All');

export const kindHref = (k) => (k ? `/${encodeURIComponent(k)}` : '/');
