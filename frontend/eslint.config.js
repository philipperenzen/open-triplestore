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
    // Standalone TypeScript modules (src/lib/**/*.ts, viewer/*.ts, …) were
    // previously unlinted: the lint script only matched .js/.svelte, so a whole
    // tier of code (much of it added with the 3D/CityJSON/IFC work) escaped the
    // linter entirely. Parse them with the TS parser and lint with the base
    // rules. `no-undef` is turned OFF here on the @typescript-eslint project's
    // own recommendation — TypeScript's compiler checks for undefined
    // identifiers (`npm run typecheck`, a CI gate; before that script existed
    // no compiler ever ran and this comment was a promise nothing kept), and
    // the base rule misfires on type references and ambient declarations.
    // `no-unused-vars` stays a (non-failing) warning.
    files: ['**/*.ts'],
    languageOptions: {
      parser: tsParser,
      globals: { ...globals.browser },
    },
    rules: {
      'no-undef': 'off',
      // The base (JS) no-unused-vars misfires on TypeScript type-signature
      // parameter names (e.g. the `xy` in `convert: (xy: …) => …`), which are
      // documentation, not bindings. Skip args here — TypeScript's own
      // noUnusedParameters is the right tool for those — but keep catching
      // genuinely dead locals and imports.
      'no-unused-vars': [
        'warn',
        { args: 'none', varsIgnorePattern: '^_', caughtErrorsIgnorePattern: '^_' },
      ],
    },
  },
  {
    // Ambient declaration files: every name is a declaration consumed by the
    // compiler or other modules, so no-unused-vars is meaningless here.
    files: ['**/*.d.ts'],
    rules: {
      'no-unused-vars': 'off',
    },
  },
  {
    // Leaflet must be reached through the wrapper that pins its default marker
    // icons to bundler-resolved URLs. A bare `import L from 'leaflet'` leaves
    // Leaflet guessing where its images live; under Vite that guess collapses to
    // an empty path and every marker renders as a broken image — and only in a
    // BUILT app, so the dev server never shows it. The wrapper itself is the one
    // place allowed to do the real import.
    files: ['**/*.js', '**/*.ts', '**/*.svelte'],
    ignores: ['src/lib/viewer/leafletIcons.ts'],
    rules: {
      'no-restricted-imports': [
        'error',
        {
          paths: [
            {
              name: 'leaflet',
              message:
                "Import the wrapper instead (lib/viewer/leafletIcons): a bare 'leaflet' import breaks the default marker icons in production builds.",
            },
          ],
        },
      ],
    },
  },
  {
    // Test files run under Node / Vitest — allow Node globals (e.g. `process`).
    files: [
      '**/*.test.js',
      '**/__tests__/**/*.js',
      '**/*.test.ts',
      '**/__tests__/**/*.ts',
    ],
    languageOptions: { globals: { ...globals.node } },
  },
  {
    // eslint-plugin-svelte 3 turned on a batch of rules its v2 `flat/recommended`
    // did not include. They fired ~430 times on code that predates them, and the
    // fixes are behaviour changes (keying an each block changes DOM reuse;
    // SvelteMap/SvelteSet change reactivity), not lint tidy-ups — so they are
    // staged here rather than bundled into a dependency bump.
    //
    // The two bulk stylistic rules are off; adopting either is its own PR.
    // The rest are demoted to warnings so they stay visible without gating CI —
    // several of them (infinite-reactive-loop, no-immutable-reactive-statements)
    // are the exact class of bug that has bitten this codebase before, so they
    // are worth triaging file by file.
    files: ['**/*.svelte'],
    rules: {
      // Off in .svelte only; stays at its recommended `error` for .js/.ts.
      //
      // The rule assumes a statement sequence executes once, so a value written
      // and not read again below is dead. A `$:` block re-runs, and the idiom
      // this codebase uses everywhere — `$: if (x !== lastX) { lastX = x; … }` —
      // reads that write on the NEXT run, which the rule cannot see. All 28
      // remaining hits were this shape (memo guards, run-once auth latches);
      // acting on any of them would reintroduce the bugs three of them carry
      // comments about (DatasetMetadataDialog, OrganisationMetadataDialog and
      // OntologyModelViewer each document why the write sits where it does).
      // Genuine dead stores in plain functions were fixed rather than silenced;
      // this only gives up catching that shape inside a component.
      'no-useless-assignment': 'off',
      'svelte/require-each-key': 'off', // 265 hits
      'svelte/prefer-svelte-reactivity': 'off', // 137 hits
      'svelte/infinite-reactive-loop': 'warn',
      // Off, not warn: the rule's suggestion fixer still calls
      // SourceCode#isSpaceBetweenTokens, which eslint 10 removed, so any report
      // crashes the whole lint run (eslint-plugin-svelte ≤ 3.23.0). Restore to
      // 'warn' once the plugin ships an eslint-10-compatible fixer.
      'svelte/no-reactive-functions': 'off',
      'svelte/no-immutable-reactive-statements': 'warn',
      'svelte/no-dom-manipulating': 'warn',
      'svelte/no-reactive-reassign': 'warn',
    },
  },
  {
    ignores: ['dist/**', 'node_modules/**'],
  },
];
