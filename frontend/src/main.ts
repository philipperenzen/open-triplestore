import { mount } from 'svelte';
import { waitLocale } from 'svelte-i18n';
import './lib/i18n/index.js';
import App from './App.svelte';
import './theme.css';
import './app.css';
import { initTheme } from './lib/theme.js';
import { initServiceRegistry } from './lib/serviceRegistry.js';
import { loadRuntimeConfig } from './lib/runtimeConfig.js';
import { logBanner } from './lib/banner.js';

// Greet the console with the brand mark + wordmark.
logBanner();

// Apply the persisted/OS-derived dark-mode signal before the app mounts so
// the first paint is already in the correct theme (avoids a light-mode flash).
initTheme();

// Discover sibling-service addresses from the registry and re-broadcast changes as
// 'ldapps-service-change'. Opt-in: a no-op unless LD_DISCOVERY is set (see vite.config.js). This
// app reaches its own backend same-origin (Vite proxy), so the client mainly powers cross-links +
// the change event. Fail-soft: localhost defaults stand.
initServiceRegistry();

// Runtime config (/config.json) — an operator's zero-rebuild way to point this
// deployment at different backends and apply branding. Unconditional (not
// gated by LD_DISCOVERY): fetched once, applied as soon as it resolves, and a
// complete no-op when no config.json is present. See runtimeConfig.ts.
loadRuntimeConfig();

let app: ReturnType<typeof mount>;

// /embed/* mounts a chrome-less single-viewer app for iframing by external
// sites (see EmbedApp.svelte + docs/embedding.md) — no nav, auth or polling.
const isEmbed = /^\/embed(\/|$)/.test(window.location.pathname);

waitLocale().then(async () => {
  const Root = isEmbed ? (await import('./EmbedApp.svelte')).default : App;
  app = mount(Root, {
    target: document.getElementById('app') as HTMLElement,
  });
});

export default app;
