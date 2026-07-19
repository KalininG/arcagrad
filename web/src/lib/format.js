export function compactNumber(n) {
	if (n == null) return '';
	return n >= 1000 ? `${(n / 1000).toFixed(n >= 10000 ? 0 : 1)}k` : String(n);
}

export const WORDS_PER_MINUTE = 250;

export function readingTimeLabel(mins) {
	if (mins <= 0) return '';
	if (mins < 60) return `~${Math.max(1, Math.round(mins))} min`;
	const h = Math.round(mins / 60);
	return `~${h} ${h === 1 ? 'hour' : 'hours'}`;
}

export function compactWords(n) {
	if (n >= 1_000_000) return (n / 1_000_000).toFixed(1).replace(/\.0$/, '') + 'M';
	if (n >= 1_000) return Math.round(n / 1_000) + 'K';
	return String(n);
}

export function relativeTime(value) {
	if (!value) return '';
	const ms = value instanceof Date ? value.getTime() : value * 1000;
	const diff = Math.max(0, Date.now() - ms);
	const m = 60_000,
		h = 3_600_000,
		d = 86_400_000;
	if (diff < m) return 'just now';
	if (diff < h) return `${Math.floor(diff / m)}m ago`;
	if (diff < d) return `${Math.floor(diff / h)}h ago`;
	if (diff < 7 * d) return `${Math.floor(diff / d)}d ago`;
	if (diff < 30 * d) return `${Math.floor(diff / (7 * d))}w ago`;
	return new Date(ms).toLocaleDateString(undefined, {
		month: 'short',
		day: 'numeric',
		year: 'numeric',
	});
}

export function formatAdded(v) {
	if (v == null) return '';
	const ms = typeof v === 'number' ? (v < 1e12 ? v * 1000 : v) : Date.parse(v);
	if (!ms || Number.isNaN(ms)) return '';
	return new Date(ms).toLocaleDateString(undefined, {
		year: 'numeric',
		month: 'short',
		day: 'numeric',
	});
}

export function fileSizeLabel(bytes) {
	if (!Number.isFinite(bytes) || bytes < 0) return '';
	const units = ['B', 'KB', 'MB', 'GB', 'TB'];
	let value = bytes;
	let unit = 0;
	while (value >= 1024 && unit < units.length - 1) {
		value /= 1024;
		unit++;
	}
	const digits = unit === 0 || value >= 10 ? 0 : 1;
	return `${value.toFixed(digits)} ${units[unit]}`;
}
