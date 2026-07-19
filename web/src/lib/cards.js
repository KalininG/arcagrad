export const cardHref = (a) => (a.type === 'series' ? `/series/${a.id}` : `/item/${a.id}`);

export const coverId = (a) => (a.type === 'series' ? a.cover_item_id : a.id);

export async function loadSimilarCards(apiFn, id, limit = 15) {
	const data = await apiFn(id, limit);
	return (data.items ?? []).filter((s) => s.type !== 'series' || s.cover_item_id != null);
}
