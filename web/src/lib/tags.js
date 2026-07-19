import { kindHref } from '$lib/kinds.js';

export const NS_ORDER = [
	'parody',
	'character',
	'creator',
	'group',
	'cosplayer',
	'language',
	'category',
	'tag',
];
export const NS_LABEL = {
	parody: 'Parodies',
	character: 'Characters',
	creator: 'Creators',
	group: 'Groups',
	cosplayer: 'Cosplayers',
	language: 'Languages',
	category: 'Categories',
	tag: 'Tags',
};

export function groupTags(tags = []) {
	const by = new Map();
	for (const t of tags) {
		if (!by.has(t.namespace)) by.set(t.namespace, []);
		by.get(t.namespace).push(t);
	}
	const out = [];
	for (const ns of NS_ORDER) {
		if (by.has(ns)) {
			out.push({ ns, label: NS_LABEL[ns] ?? ns, tags: by.get(ns) });
			by.delete(ns);
		}
	}
	for (const [ns, ts] of by) out.push({ ns, label: NS_LABEL[ns] ?? ns, tags: ts });
	return out;
}

export const tagHref = (kind, t) =>
	`${kind ? kindHref(kind) : '/'}?tags=${encodeURIComponent(`${t.namespace}:${t.value}`)}`;
