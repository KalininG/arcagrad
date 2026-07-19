import { defineConfig, passthroughImageService } from 'astro/config';
import starlight from '@astrojs/starlight';
import icon from 'astro-icon';

export default defineConfig({
	site: 'https://arcagrad.com',
	image: { service: passthroughImageService() },
	integrations: [
		icon(),
		starlight({
			title: 'Arcagrad',
			description:
				'Self-hosted comic, manga, and book server — built in Rust for large libraries and multiple users.',
			logo: { src: './src/assets/icon.svg', alt: 'Arcagrad' },
			components: {
				Header: './src/components/Header.astro',
			},
			customCss: ['./src/styles/custom.css'],
			social: [
				{ icon: 'github', label: 'GitHub', href: 'https://github.com/KalininG/arcagrad' },
			],
			sidebar: [
				{
					label: 'Getting started',
					items: [
						{ label: 'Introduction', slug: 'getting-started/introduction' },
						{ label: 'Install with Docker', slug: 'getting-started/install' },
						{ label: 'Configuration', slug: 'getting-started/configuration' },
					],
				},
				{
					label: 'Guides',
					items: [{ autogenerate: { directory: 'guides' } }],
				},
				{ label: 'Administration', slug: 'administration' },
				{
					label: 'Reference',
					items: [{ autogenerate: { directory: 'reference' } }],
				},
			],
		}),
	],
});
