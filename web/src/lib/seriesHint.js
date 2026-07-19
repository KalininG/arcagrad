import { writable } from 'svelte/store';

export const seriesHint = writable(null);
export const setSeriesHint = (id, title) => seriesHint.set({ id, title });
