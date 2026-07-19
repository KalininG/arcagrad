// Rebuild into a fresh tree; mutating parsed markup can leave mutation-XSS gaps.
const ALLOWED = new Set([
	'A',
	'B',
	'STRONG',
	'I',
	'EM',
	'U',
	'S',
	'BR',
	'P',
	'SPAN',
	'BLOCKQUOTE',
	'CODE',
	'PRE',
	'UL',
	'OL',
	'LI',
]);
const SAFE_PROTO = /^(https?:|mailto:)/i;

export function sanitizeHtml(html, opts = {}) {
	if (!html || typeof document === 'undefined') return '';
	const doc = new DOMParser().parseFromString(String(html), 'text/html');
	const out = document.createElement('div');
	rebuild(doc.body, out, opts);
	return out.innerHTML;
}

function rebuild(src, dest, opts) {
	for (const node of src.childNodes) {
		if (node.nodeType === Node.TEXT_NODE) {
			if (opts.onText) opts.onText(node.nodeValue, dest);
			else dest.appendChild(document.createTextNode(node.nodeValue));
		} else if (node.nodeType === Node.ELEMENT_NODE) {
			const tag = node.tagName;
			if (tag === 'SCRIPT' || tag === 'STYLE') continue;
			if (tag === 'IMG') {
				const src = node.getAttribute('src') || '';
				if (/^https?:/i.test(src)) {
					const img = document.createElement('img');
					img.setAttribute('src', src);
					img.setAttribute('alt', node.getAttribute('alt') || '');
					img.setAttribute('loading', 'lazy');
					img.className = 'cimg';
					dest.appendChild(img);
				}
				continue;
			}
			if (ALLOWED.has(tag)) {
				const el = document.createElement(tag);
				if (tag === 'A') {
					const href = node.getAttribute('href') || '';
					if (SAFE_PROTO.test(href)) {
						el.setAttribute('href', href);
						el.setAttribute('target', '_blank');
						el.setAttribute('rel', 'noopener noreferrer nofollow');
					}
					rebuild(node, el, { ...opts, onText: null });
				} else {
					rebuild(node, el, opts);
				}
				dest.appendChild(el);
			} else {
				rebuild(node, dest, opts);
			}
		}
	}
}
