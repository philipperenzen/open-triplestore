import js from '@eslint/js';
import globals from 'globals';
import sveltePlugin from 'eslint-plugin-svelte';
import svelteParser from 'svelte-eslint-parser';
import tsParser from '@typescript-eslint/parser';

/** @type {import('eslint').Linter.FlatConfig[]} */
export default [
  js.configs.recommended,
  ...sveltePlugin.configs['flat/recommended'],
  {
    // Let the Svelte parser hand <script lang="ts"> blocks to the TS parser
    files: ['**/*.svelte'],
    languageOptions: {
      parser: svelteParser,
      parserOptions: {
        parser: tsParser,
      },
    },
  },
  {
    languageOptions: {
      globals: {
        ...globals.browser,
      },
    },
    rules: {
      // Catch accidental globals (W4-18)
      'no-undef': 'error',
      'no-unused-vars': ['warn', { argsIgnorePattern: '^_', caughtErrorsIgnorePattern: '^_' }],
      'no-console': 'warn',
      // Empty catch blocks are an intentional "best-effort, ignore failure" pattern here
      'no-empty': ['error', { allowEmptyCatch: true }],
    },
  },
  {
    // Test files run under Node / Vitest — allow Node globals (e.g. `process`).
    files: ['**/*.test.js', '**/__tests__/**/*.js'],
    languageOptions: { globals: { ...globals.node } },
  },
  {
    ignores: ['dist/**', 'node_modules/**'],
  },
];
