import { writable } from 'svelte/store';

export const navStack = writable([]);

export function pushNav(href) {
	navStack.update((s) => [...s, href]);
}
export function popNav() {
	navStack.update((s) => s.slice(0, -1));
}
export function setNav(list) {
	navStack.set(list);
}

let popping = false;
export function markPop() {
	popping = true;
}
export function consumePop() {
	const was = popping;
	popping = false;
	return was;
}
