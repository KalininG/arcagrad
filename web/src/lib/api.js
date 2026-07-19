// Shared transport for the cookie-based web UI and bearer-based native clients.
let config = {
	baseUrl: '',
	auth: 'cookie',
	token: null,
	onUnauthorized: null,
};

export function configureApi(patch) {
	config = { ...config, ...patch };
}

export class ApiError extends Error {
	constructor(status, message) {
		super(message);
		this.name = 'ApiError';
		this.status = status;
	}
}

function authHeaders() {
	return config.auth === 'bearer' && config.token
		? { Authorization: `Bearer ${config.token}` }
		: {};
}

async function request(method, path, body, opts = {}) {
	let res;
	try {
		res = await fetch(config.baseUrl + path, {
			method,
			credentials: config.auth === 'cookie' ? 'same-origin' : 'omit',
			headers: {
				...authHeaders(),
				...(body !== undefined ? { 'content-type': 'application/json' } : {}),
			},
			body: body !== undefined ? JSON.stringify(body) : undefined,
			keepalive: opts.keepalive ?? false,
		});
	} catch (e) {
		throw new ApiError(0, String(e));
	}

	if (res.status === 401) {
		if (!opts.silent401) config.onUnauthorized?.();
		throw new ApiError(401, 'unauthorized');
	}
	if (!res.ok) {
		let msg = `HTTP ${res.status}`;
		try {
			const j = await res.json();
			if (j?.error) msg = j.error;
		} catch {
			/* ignored */
		}
		throw new ApiError(res.status, msg);
	}
	if (res.status === 204) return null;
	const ct = res.headers.get('content-type') || '';
	return ct.includes('application/json') ? res.json() : res;
}

export function uploadFile(path, file, fields = {}, { onProgress, signal } = {}) {
	return new Promise((resolve, reject) => {
		const xhr = new XMLHttpRequest();
		xhr.open('POST', config.baseUrl + path);
		if (config.auth === 'cookie') xhr.withCredentials = true;
		else if (config.token) xhr.setRequestHeader('Authorization', `Bearer ${config.token}`);
		xhr.upload.onprogress = (e) => {
			if (onProgress && e.lengthComputable) onProgress(e.loaded / e.total);
		};
		xhr.onload = () => {
			if (xhr.status === 401) {
				config.onUnauthorized?.();
				reject(new ApiError(401, 'unauthorized'));
				return;
			}
			let body = null;
			try {
				body = JSON.parse(xhr.responseText);
			} catch {
				/* ignored */
			}
			if (xhr.status >= 200 && xhr.status < 300) resolve({ status: xhr.status, body });
			else reject(new ApiError(xhr.status, body?.error || `HTTP ${xhr.status}`));
		};
		xhr.onerror = () => reject(new ApiError(0, 'network error'));
		xhr.onabort = () => reject(new ApiError(0, 'aborted'));
		if (signal) signal.addEventListener('abort', () => xhr.abort());
		const fd = new FormData();
		fd.append('file', file);
		for (const [k, v] of Object.entries(fields)) if (v != null) fd.append(k, String(v));
		xhr.send(fd);
	});
}

export const http = {
	get: (path, opts) => request('GET', path, undefined, opts),
	post: (path, body, opts) => request('POST', path, body, opts),
	put: (path, body, opts) => request('PUT', path, body, opts),
	del: (path, body, opts) => request('DELETE', path, body, opts),
};

export function apiAuthMode() {
	return config.auth;
}

export function mediaUrl(path) {
	// Elements such as <img> cannot attach the Bearer header.
	if (config.auth === 'bearer' && config.token) {
		const sep = path.includes('?') ? '&' : '?';
		return `${config.baseUrl}${path}${sep}token=${encodeURIComponent(config.token)}`;
	}
	return config.baseUrl + path;
}

export const auth = {
	status: () => http.get('/api/auth/status'),
	me: (opts) => http.get('/api/me', opts),
	login: (username, password) => http.post('/api/auth/login', { username, password }),
	setup: (username, password) => http.post('/api/auth/setup', { username, password }),
	register: (username, password) => http.post('/api/auth/register', { username, password }),
	logout: () => http.post('/api/auth/logout'),
	logoutAll: () => http.post('/api/auth/logout-all'),
	changePassword: (current, newPassword) =>
		http.put('/api/auth/password', { current, new: newPassword }),
	avatar: {
		upload: async (file) => {
			let res;
			try {
				res = await fetch(config.baseUrl + '/api/me/avatar', {
					method: 'PUT',
					credentials: config.auth === 'cookie' ? 'same-origin' : 'omit',
					headers: { ...authHeaders(), 'content-type': 'application/octet-stream' },
					body: file,
				});
			} catch (e) {
				throw new ApiError(0, String(e));
			}
			if (!res.ok) {
				let msg = `HTTP ${res.status}`;
				try {
					const j = await res.json();
					if (j?.error) msg = j.error;
				} catch {
					/* ignored */
				}
				throw new ApiError(res.status, msg);
			}
			return res.json();
		},
		remove: () => http.del('/api/me/avatar'),
		url: (version) => mediaUrl(`/api/me/avatar?v=${version}`),
	},
	banner: {
		upload: async (file) => {
			let res;
			try {
				res = await fetch(config.baseUrl + '/api/me/banner', {
					method: 'PUT',
					credentials: config.auth === 'cookie' ? 'same-origin' : 'omit',
					headers: { ...authHeaders(), 'content-type': 'application/octet-stream' },
					body: file,
				});
			} catch (e) {
				throw new ApiError(0, String(e));
			}
			if (!res.ok) {
				let msg = `HTTP ${res.status}`;
				try {
					const j = await res.json();
					if (j?.error) msg = j.error;
				} catch {
					/* ignored */
				}
				throw new ApiError(res.status, msg);
			}
			return res.json();
		},
		remove: () => http.del('/api/me/banner'),
		url: (version) => mediaUrl(`/api/me/banner?v=${version}`),
	},
	keys: {
		list: () => http.get('/api/auth/keys', { silent401: true }),
		create: (label) => http.post('/api/auth/keys', { label }, { silent401: true }),
		revoke: (id) => http.del(`/api/auth/keys/${id}`, undefined, { silent401: true }),
	},
};

export const users = {
	list: () => http.get('/api/users'),
	stats: () => http.get('/api/users/stats'),
	create: (username, password, role = 'user') =>
		http.post('/api/users', { username, password, role }),
	remove: (id) => http.del(`/api/users/${id}`),
	avatarUrl: (id, version) => mediaUrl(`/api/users/${id}/avatar?v=${version}`),
	getAccess: () => http.get('/api/settings/auth'),
	putAccess: (signupEnabled, guestEnabled) =>
		http.put('/api/settings/auth', {
			signup_enabled: signupEnabled,
			guest_enabled: guestEnabled,
		}),
	getKindAccess: () => http.get('/api/settings/kind-access'),
	putKindAccess: (map) => http.put('/api/settings/kind-access', map),
};

export const items = {
	list: ({
		limit = 50,
		page,
		cursor,
		before,
		last,
		q,
		tags,
		match,
		sort,
		order,
		favorited,
		untagged,
		completed,
		kind,
	} = {}) => {
		const qs = new URLSearchParams({ limit: String(limit) });
		if (page) qs.set('page', String(page));
		if (cursor) qs.set('cursor', cursor);
		if (before) qs.set('before', before);
		if (last) qs.set('last', 'true');
		if (kind) qs.set('kind', kind);
		if (q) qs.set('q', q);
		if (tags) qs.set('tags', tags);
		if (match) qs.set('match', match);
		if (favorited != null) qs.set('favorited', favorited ? 'true' : 'false');
		if (untagged != null) qs.set('untagged', untagged ? 'true' : 'false');
		if (completed != null) qs.set('completed', completed ? 'true' : 'false');
		if (sort) qs.set('sort', sort);
		if (order) qs.set('order', order);
		return http.get(`/api/items?${qs}`);
	},
	continue: (limit = 20, kind) => {
		const qs = new URLSearchParams({ limit: String(limit) });
		if (kind) qs.set('kind', kind);
		return http.get(`/api/items/continue?${qs}`);
	},
	finished: (limit = 5, kind) => {
		const qs = new URLSearchParams({ limit: String(limit) });
		if (kind) qs.set('kind', kind);
		return http.get(`/api/items/finished?${qs}`);
	},
	detail: (id) => http.get(`/api/items/${id}`),
	editMetadata: (id, body) => http.put(`/api/items/${id}/metadata`, body),
	addTag: (id, tag) => http.post(`/api/items/${id}/tags`, tag),
	removeTag: (id, tag) => http.del(`/api/items/${id}/tags`, tag),
	forgetSource: (id, source) => http.del(`/api/items/${id}/sources/${encodeURIComponent(source)}`),
	manifest: (id) => http.get(`/api/items/${id}/manifest`),
	similar: (id, limit = 12) => http.get(`/api/items/${id}/similar?limit=${limit}`),
	recommendations: (limit = 20, kind) => {
		const qs = new URLSearchParams({ limit: String(limit) });
		if (kind) qs.set('kind', kind);
		return http.get(`/api/recommendations?${qs}`);
	},
	upload: (file, { kind, onProgress, signal } = {}) =>
		uploadFile('/api/items', file, { kind }, { onProgress, signal }).then((r) => ({
			status: r.status,
			item: r.body,
		})),
	saveProgress: (id, progress, opts) =>
		http.put(
			`/api/items/${id}/progress`,
			typeof progress === 'number' ? { page: progress } : progress,
			opts,
		),
	favorite: (id) => http.post(`/api/items/${id}/favorite`),
	unfavorite: (id) => http.del(`/api/items/${id}/favorite`),
	setRating: (id, value) => http.put(`/api/items/${id}/rating`, { value }),
	clearRating: (id) => http.del(`/api/items/${id}/rating`),
	setReadingMode: (id, mode) => http.put(`/api/items/${id}/reading-mode`, { mode }),
	remove: (id) => http.del(`/api/items/${id}`),
	scrape: (id, plugin, { ref, wait } = {}) => {
		const qs = new URLSearchParams({ plugin });
		if (ref) qs.set('ref', ref);
		if (wait) qs.set('wait', 'true');
		return http.post(`/api/items/${id}/scrape?${qs}`);
	},
	identify: (id, plugin) =>
		http.post(`/api/items/${id}/identify?${new URLSearchParams({ plugin })}`),
};

export const jobs = {
	get: (id, opts = {}) => http.get(`/api/jobs/${id}${opts.wait ? '?wait=true' : ''}`),
};

export const plugins = {
	list: ({ capability, kind } = {}) => {
		const qs = new URLSearchParams();
		if (capability) qs.set('capability', capability);
		if (kind) qs.set('kind', kind);
		const s = qs.toString();
		return http.get(`/api/plugins${s ? `?${s}` : ''}`);
	},
	catalog: () => http.get('/api/plugin-catalog'),
	install: (id) => http.post('/api/plugin-installs', { id }),
	uninstall: (id) => http.del(`/api/plugin-installs/${encodeURIComponent(id)}`),
	installFile: (file) => uploadFile('/api/plugin-installs/file', file, {}, {}).then((r) => r.body),
	repos: () => http.get('/api/plugin-repos'),
	addRepo: (url) => http.post('/api/plugin-repos', { url }),
	removeRepo: (url) => http.del(`/api/plugin-repos?url=${encodeURIComponent(url)}`),
	refreshRepos: () => http.post('/api/plugin-repos/refresh'),
	browse: (id, { feed, range, query, page } = {}) => {
		const qs = new URLSearchParams();
		if (feed) qs.set('feed', feed);
		if (range) qs.set('range', range);
		if (query) qs.set('query', query);
		if (page) qs.set('page', String(page));
		return http.get(`/api/plugins/${encodeURIComponent(id)}/browse?${qs}`);
	},
	item: (id, ref) =>
		http.get(`/api/plugins/${encodeURIComponent(id)}/item?ref=${encodeURIComponent(ref)}`),
	pages: (id, ref) =>
		http.get(`/api/plugins/${encodeURIComponent(id)}/pages?ref=${encodeURIComponent(ref)}`),
};

export const library = {
	match: (queries) => http.post('/api/browse/match', queries),
};

export const metrics = {
	get: () => http.get('/api/metrics'),
};

export const credentials = {
	list: () => http.get('/api/credentials'),
	set: (source, data) => http.put(`/api/credentials/${encodeURIComponent(source)}`, { data }),
	remove: (source) => http.del(`/api/credentials/${encodeURIComponent(source)}`),
};

export const series = {
	get: (id) => http.get(`/api/series/${id}`),
	similar: (id, limit = 12) => http.get(`/api/series/${id}/similar?limit=${limit}`),
	favorite: (id) => http.post(`/api/series/${id}/favorite`),
	unfavorite: (id) => http.del(`/api/series/${id}/favorite`),
	setRating: (id, value) => http.put(`/api/series/${id}/rating`, { value }),
	clearRating: (id) => http.del(`/api/series/${id}/rating`),
	scrape: (id, plugin, { ref, wait } = {}) => {
		const qs = new URLSearchParams({ plugin });
		if (ref) qs.set('ref', ref);
		if (wait) qs.set('wait', 'true');
		return http.post(`/api/series/${id}/scrape?${qs}`);
	},
	editMetadata: (id, body) => http.put(`/api/series/${id}/metadata`, body),
	forgetSource: (id, source) => http.del(`/api/series/${id}/sources/${encodeURIComponent(source)}`),
	allTrackers: () => http.get('/api/trackers'),
	trackers: (id) => http.get(`/api/series/${id}/trackers`),
	setTracker: (id, plugin, reference) =>
		http.put(`/api/series/${id}/trackers/${encodeURIComponent(plugin)}`, { reference }),
	removeTracker: (id, plugin) =>
		http.del(`/api/series/${id}/trackers/${encodeURIComponent(plugin)}`),
};

export const kinds = {
	list: () => http.get('/api/kinds'),
	plugins: (kind) => http.get(`/api/kinds/${encodeURIComponent(kind)}/plugins`),
	setPlugins: (kind, pluginIds, autoIds = []) =>
		http.put(`/api/kinds/${encodeURIComponent(kind)}/plugins`, {
			plugin_ids: pluginIds,
			auto: autoIds,
		}),
};

export const downloads = {
	create: (plugin, { ref, kind, wait } = {}) => {
		const qs = new URLSearchParams();
		if (ref) qs.set('ref', ref);
		if (kind) qs.set('kind', kind);
		if (wait) qs.set('wait', 'true');
		const q = qs.toString();
		return http.post(`/api/plugins/${encodeURIComponent(plugin)}/download${q ? `?${q}` : ''}`);
	},
};

export const follows = {
	list: () => http.get('/api/follows'),
	create: ({ plugin_id, kind, feed, query }) =>
		http.post('/api/follows', { plugin_id, kind, feed, query }),
	remove: (id) => http.del(`/api/follows/${id}`),
	check: (ids = []) => http.post('/api/follows/check', { follows: ids }),
	items: (id) => http.get(`/api/follows/${id}/items`),
	setItemState: (id, reference, state) =>
		http.post(`/api/follows/${id}/items/state`, { reference, state }),
	dismissAll: (id) => http.post(`/api/follows/${id}/items/dismiss-all`),
};

export const tags = {
	list: (kind) => http.get(kind ? `/api/tags?kind=${encodeURIComponent(kind)}` : '/api/tags'),
	favorites: (kind) => http.get(`/api/tags/favorites?kind=${encodeURIComponent(kind)}`),
};

export const stats = {
	get: () => http.get('/api/me/stats'),
};

export const upcoming = {
	list: () => http.get('/api/upcoming'),
	refresh: () => http.post('/api/upcoming/refresh'),
};

export const search = {
	suggest: (q, limit = 10, kind) =>
		http.get(
			`/api/suggest?q=${encodeURIComponent(q)}&limit=${limit}${kind ? `&kind=${encodeURIComponent(kind)}` : ''}`,
		),
};

const ver = (v) => (v ? `?v=${encodeURIComponent(v)}` : '');
export const media = {
	thumbnail: (id, v) => mediaUrl(`/api/items/${id}/thumbnail${ver(v)}`),
	page: (id, n, v) => mediaUrl(`/api/items/${id}/pages/${n}${ver(v)}`),
	download: (id) => mediaUrl(`/api/items/${id}/download`),
	pageThumbnail: (id, n, v) => mediaUrl(`/api/items/${id}/pages/${n}/thumbnail${ver(v)}`),
	epubResource: (href) =>
		// Path auth also covers relative CSS, image, and font requests from the EPUB.
		config.auth === 'bearer' && config.token
			? `${config.baseUrl}${href.replace('/resource/', `/resource/@t/${config.token}/`)}`
			: config.baseUrl + href,
	// Proxy source artwork so the remote host never sees the reader's IP.
	pluginImage: (pluginId, url) =>
		mediaUrl(`/api/plugins/${encodeURIComponent(pluginId)}/image?url=${encodeURIComponent(url)}`),
	pluginIcon: (pathOrId) =>
		mediaUrl(
			pathOrId.startsWith('/') ? pathOrId : `/api/plugins/${encodeURIComponent(pathOrId)}/icon`,
		),
};
