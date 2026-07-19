import js from '@eslint/js';
import svelte from 'eslint-plugin-svelte';
import prettier from 'eslint-config-prettier';
import globals from 'globals';

export default [
	{ ignores: ['build/', '.svelte-kit/', 'node_modules/', 'dist/'] },

	js.configs.recommended,
	...svelte.configs['flat/recommended'],
	prettier,
	...svelte.configs['flat/prettier'],

	{
		languageOptions: {
			globals: { ...globals.browser, ...globals.node },
		},
		rules: {
			'svelte/no-navigation-without-resolve': 'off',
			'svelte/prefer-svelte-reactivity': 'off',
			'svelte/no-useless-children-snippet': 'off',
			'svelte/prefer-writable-derived': 'off',
			'no-unused-vars': ['error', { argsIgnorePattern: '^_', varsIgnorePattern: '^_' }],
		},
	},
];
