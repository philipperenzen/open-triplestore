// The JS unit tests live in frontend/ and are normally run with `npm test` there.
// This root-level config exists only so that running `vitest` / `npx vitest` from
// the repository root does the right thing instead of falling back to vitest's
// defaults — which use the node environment (no DOM, so DOMPurify/safeUrl/auth
// break), no Svelte/Tailwind plugins (so `.svelte` imports fail to parse), and a
// glob that sweeps in the Playwright e2e specs (which crash under vitest). It
// delegates to the frontend project, which carries the real test config
// (jsdom environment, plugins, setup files, src-only includes). No imports here
// on purpose: the repo root has no node_modules, so resolving `vitest/config`
// would fail — a plain config object is loaded fine.
export default { test: { projects: ['frontend'] } };
