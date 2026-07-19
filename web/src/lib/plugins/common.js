export const CAP_LABEL = {
	scrape: 'Add metadata',
	browse: 'Browse catalog',
	pages: 'Read remote content',
	read: 'Read remote content',
	download: 'Download files',
};

export const CAP_HELP = {
	scrape: 'Fetch metadata for items in your library',
	download: 'Download items from the source into the library',
	browse: 'Browse the source catalog',
	pages: 'Read a source item online (page streaming)',
	read: 'Read a source item online (page streaming)',
};

export const STATE_META = {
	active: { label: 'Active', cls: 'st-good' },
	disabled: { label: 'Disabled', cls: 'st-muted' },
	failed: { label: 'Failed to load', cls: 'st-bad' },
	incompatible: { label: 'Permission review', cls: 'st-warn' },
};

export const ORIGIN_LABEL = { bundled: 'Bundled' };

export const stateMeta = (state) => STATE_META[state] ?? { label: state, cls: 'st-muted' };
export const capList = (inst) =>
	(inst?.permissions?.capabilities ?? []).map((c) => CAP_LABEL[c] ?? c);
export const toggleable = (inst) => inst.state === 'active' || inst.state === 'disabled';
export const shortSha = (sha) => (sha ? `${sha.slice(0, 4)}…${sha.slice(-4)}` : '');

export const versionOf = (inst) =>
	inst.active_version === '0.0.0' ? '' : `v${inst.active_version}`;
export const authorOf = (info) => (info?.author && info.author !== 'Unknown' ? info.author : '');

export function categoryOf(inst) {
	const caps = inst?.permissions?.capabilities ?? [];
	return caps.some((c) => c === 'browse' || c === 'download' || c === 'read' || c === 'pages')
		? 'sources'
		: caps.includes('scrape')
			? 'metadata'
			: 'other';
}

const TINTS = [
	['#d4537e', 'rgba(212, 83, 126, 0.16)'],
	['#1d9e75', 'rgba(29, 158, 117, 0.16)'],
	['#7f77dd', 'rgba(127, 119, 221, 0.16)'],
	['#d88a2f', 'rgba(216, 138, 47, 0.16)'],
	['#378add', 'rgba(55, 138, 221, 0.16)'],
	['#d85a30', 'rgba(216, 90, 48, 0.16)'],
];
export function tintOf(id) {
	let h = 0;
	for (const ch of id) h = (h * 31 + ch.charCodeAt(0)) >>> 0;
	const [fg, bg] = TINTS[h % TINTS.length];
	return `color:${fg};background:${bg}`;
}
export const monogram = (name) => (name ?? '?').slice(0, 1).toUpperCase();
