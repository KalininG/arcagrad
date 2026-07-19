export function loadCover(proxiedUrl) {
	return new Promise((resolve) => {
		if (!proxiedUrl) return resolve();
		const img = new Image();
		img.onload = () => resolve();
		img.onerror = () => resolve();
		img.src = proxiedUrl;
	});
}
