const invoke = (cmd, args) => globalThis.__TAURI__?.core?.invoke(cmd, args);

export const isDesktop = () => !!globalThis.__TAURI__?.core?.invoke;

export async function checkForUpdate() {
	if (!isDesktop()) return null;
	try {
		return (await invoke('check_for_update')) ?? null;
	} catch (e) {
		console.error('update check failed', e);
		return null;
	}
}

export async function installUpdate() {
	if (!isDesktop()) return;
	await invoke('install_update');
}
