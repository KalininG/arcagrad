import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';

const API = process.env.ARCA_DEV_API ?? 'http://127.0.0.1:3000';
const tauriHost = process.env.TAURI_DEV_HOST;

export default defineConfig({
	plugins: [sveltekit()],
	server: {
		host: tauriHost || false,
		strictPort: !!tauriHost,
		proxy: {
			'/api': API,
			'/health': API,
		},
	},
});
