import { writable, derived, get } from 'svelte/store';
import { goto } from '$app/navigation';
import { auth } from '$lib/api.js';

export const GUEST = { id: 0, username: 'Guest', role: 'guest' };

const KEY = 'arca:user';

function load() {
	try {
		const s = localStorage.getItem(KEY);
		return s ? JSON.parse(s) : null;
	} catch {
		return null;
	}
}

export const currentUser = writable(load());

export const isGuest = derived(currentUser, (u) => u?.role === 'guest');

export function setUser(u) {
	currentUser.set(u);
	try {
		if (u) localStorage.setItem(KEY, JSON.stringify(u));
		else localStorage.removeItem(KEY);
	} catch {
		/* ignored */
	}
}

export function resolveGuestSession() {
	if (get(currentUser)) return;
	auth
		.status()
		.then((s) => {
			if (s.authenticated) setUser(s.user);
			else if (s.guest_enabled) setUser(GUEST);
			else goto('/login');
		})
		.catch(() => {});
}

export async function performLogout() {
	try {
		await auth.logout();
	} catch {
		/* ignored */
	}
	setUser(null);
	const invoke = globalThis.__TAURI__?.core?.invoke;
	if (invoke) {
		try {
			await invoke('disconnect');
		} catch {
			/* ignored */
		}
		globalThis.location.reload();
		return;
	}
	goto('/login');
}
