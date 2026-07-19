const TAG_NAMESPACES = new Set([
	'creator',
	'group',
	'parody',
	'character',
	'tag',
	'category',
	'demographic',
	'language',
]);

const NS_ALIASES = { author: 'creator', artist: 'creator' };

function nsBoundary() {
	return new RegExp(
		`(^|[\\s,])(-?)(${[...TAG_NAMESPACES, ...Object.keys(NS_ALIASES)].join('|')})\\s*:\\s*`,
		'gi',
	);
}

export function parseSearch(text) {
	const tags = [];
	const qParts = [];
	for (const piece of text.split(',')) {
		const matches = [...piece.matchAll(nsBoundary())];
		if (!matches.length) {
			if (piece.trim()) qParts.push(piece.trim());
			continue;
		}
		const lead = piece.slice(0, matches[0].index).trim();
		if (lead) qParts.push(lead);
		matches.forEach((m, i) => {
			const start = m.index + m[0].length;
			const end = i + 1 < matches.length ? matches[i + 1].index : piece.length;
			const value = piece.slice(start, end).trim();
			const ns = NS_ALIASES[m[3].toLowerCase()] ?? m[3].toLowerCase();
			if (value) tags.push(`${m[2]}${ns}:${value}`);
		});
	}
	return { tags, q: qParts.join(' ') };
}

export function trailingToken(text) {
	const lastComma = text.lastIndexOf(',');
	let start = lastComma >= 0 ? lastComma + 1 : 0;
	let rest = text.slice(start);
	let term = rest;
	let negated = false;
	const matches = [...rest.matchAll(nsBoundary())];
	if (matches.length) {
		const m = matches[matches.length - 1];
		start += m.index + (m[1] ? m[1].length : 0);
		negated = m[2] === '-';
		term = rest.slice(m.index + m[0].length);
	} else if (rest.trimStart().startsWith('-')) {
		negated = true;
		term = rest.replace(/^\s*-/, '');
	}
	if (!matches.length && term.includes(':')) term = term.slice(term.indexOf(':') + 1);
	return { start, term: term.trim(), negated };
}
