export function gridColumns(node, cb) {
	let cols = 0;
	const measure = () => {
		const tpl = getComputedStyle(node).gridTemplateColumns;
		const n = tpl && tpl !== 'none' ? tpl.split(' ').length : 1;
		if (n !== cols) {
			cols = n;
			cb(n);
		}
	};
	const ro = new ResizeObserver(measure);
	ro.observe(node);
	measure();
	return { destroy: () => ro.disconnect() };
}

export function rowAlignedPageSize(cols, target, minRows = 1, maxRows = 8) {
	const c = Math.max(1, cols | 0);
	const rows = Math.min(maxRows, Math.max(minRows, Math.round(target / c)));
	return rows * c;
}
