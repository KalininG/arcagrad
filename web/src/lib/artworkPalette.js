const SAMPLE_SIZE = 32;
const CLUSTERS = 5;
const ITERATIONS = 8;

function saturation(r, g, b) {
	const max = Math.max(r, g, b) / 255;
	const min = Math.min(r, g, b) / 255;
	if (max === min) return 0;
	const light = (max + min) / 2;
	return (max - min) / (1 - Math.abs(2 * light - 1));
}

function luminance(r, g, b) {
	return (0.2126 * r + 0.7152 * g + 0.0722 * b) / 255;
}

function distance(a, b) {
	const dr = a.r - b.r;
	const dg = a.g - b.g;
	const db = a.b - b.b;
	return dr * dr * 0.8 + dg * dg * 1.1 + db * db * 0.7;
}

function vividness(p) {
	return 0.3 + p.sat * 0.7;
}

function seeds(samples, count) {
	const out = [samples.reduce((best, p) => (vividness(p) > vividness(best) ? p : best))];
	while (out.length < count) {
		let next = samples[0];
		let best = -1;
		for (const p of samples) {
			const nearest = Math.min(...out.map((c) => distance(p, c)));
			const score = nearest * vividness(p);
			if (score > best) {
				best = score;
				next = p;
			}
		}
		out.push(next);
	}
	return out.map((p) => ({ r: p.r, g: p.g, b: p.b }));
}

export function paletteFromPixels(data, width, height, count = CLUSTERS) {
	let samples = [];
	for (let y = 0; y < height; y++) {
		for (let x = 0; x < width; x++) {
			const i = (y * width + x) * 4;
			const [r, g, b, a] = data.slice(i, i + 4);
			if (a < 180) continue;
			const lum = luminance(r, g, b);
			if (lum < 0.035 || lum > 0.965) continue;
			samples.push({ r, g, b, x, y, sat: saturation(r, g, b) });
		}
	}
	if (samples.length < count) {
		samples = [];
		for (let y = 0; y < height; y++) {
			for (let x = 0; x < width; x++) {
				const i = (y * width + x) * 4;
				const [r, g, b, a] = data.slice(i, i + 4);
				if (a >= 180) samples.push({ r, g, b, x, y, sat: saturation(r, g, b) });
			}
		}
	}
	if (!samples.length) return [];

	let centers = seeds(samples, Math.min(count, samples.length));
	let groups = [];
	for (let iteration = 0; iteration < ITERATIONS; iteration++) {
		groups = centers.map(() => ({ r: 0, g: 0, b: 0, x: 0, y: 0, count: 0, sat: 0 }));
		for (const p of samples) {
			let at = 0;
			let nearest = Infinity;
			for (let i = 0; i < centers.length; i++) {
				const d = distance(p, centers[i]);
				if (d < nearest) {
					nearest = d;
					at = i;
				}
			}
			const g = groups[at];
			g.r += p.r;
			g.g += p.g;
			g.b += p.b;
			g.x += p.x;
			g.y += p.y;
			g.sat += p.sat;
			g.count++;
		}
		centers = groups.map((g, i) =>
			g.count ? { r: g.r / g.count, g: g.g / g.count, b: g.b / g.count } : centers[i],
		);
	}

	return groups
		.filter((g) => g.count > 0)
		.map((g) => {
			const sat = g.sat / g.count;
			return {
				r: Math.round(g.r / g.count),
				g: Math.round(g.g / g.count),
				b: Math.round(g.b / g.count),
				x: Math.max(8, Math.min(92, ((g.x / g.count + 0.5) / width) * 100)),
				y: Math.max(5, Math.min(82, ((g.y / g.count + 0.5) / height) * 100)),
				population: g.count / samples.length,
				score: (g.count / samples.length) * (0.35 + sat * 0.9),
			};
		})
		.filter((c) => c.population >= 0.01)
		.sort((a, b) => b.score - a.score)
		.slice(0, 4);
}

export async function paletteFromImage(src) {
	const image = new Image();
	image.crossOrigin = 'anonymous';
	image.decoding = 'async';
	await new Promise((resolve, reject) => {
		image.onload = resolve;
		image.onerror = () => reject(new Error('artwork could not be sampled'));
		image.src = src;
	});
	const canvas = document.createElement('canvas');
	canvas.width = SAMPLE_SIZE;
	canvas.height = SAMPLE_SIZE;
	const context = canvas.getContext('2d', { willReadFrequently: true });
	if (!context) return [];
	context.drawImage(image, 0, 0, SAMPLE_SIZE, SAMPLE_SIZE);
	const pixels = context.getImageData(0, 0, SAMPLE_SIZE, SAMPLE_SIZE);
	return paletteFromPixels(pixels.data, SAMPLE_SIZE, SAMPLE_SIZE);
}
