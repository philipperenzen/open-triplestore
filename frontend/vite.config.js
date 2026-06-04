import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';
import tailwindcss from '@tailwindcss/vite';

// One-shot resolve of the service registry at dev-server startup so the backend proxy
// targets can follow discovery. Falls back to {} (→ the localhost default) when it's down.
const REGISTRY_URL = (process.env.LD_REGISTRY_URL || 'http://localhost:8500').replace(/\/+$/, '');
async function resolveRegistry() {
  try {
    const ctrl = new AbortController();
    const timer = setTimeout(() => ctrl.abort(), 1000);
    const r = await fetch(`${REGISTRY_URL}/resolve`, { signal: ctrl.signal });
    clearTimeout(timer);
    if (!r.ok) return {};
    const data = await r.json();
    const out = {};
    for (const [name, entry] of Object.entries(data.services || {})) {
      if (entry && entry.url) out[name] = entry.url;
    }
    return out;
  } catch {
    return {};
  }
}

export default defineConfig(async () => {
  const reg = await resolveRegistry();
  // The triplestore backend ("triplestore" in the registry); defaults to the local dev port.
  const TS = reg.triplestore || 'http://localhost:7878';
  return {
    plugins: [tailwindcss(), svelte()],
    server: {
      // --no-reload (LD_NO_HMR=1) turns off hot module reload while keeping the dev server + proxy.
      hmr: process.env.LD_NO_HMR === '1' ? false : undefined,
      port: 5173,
      proxy: {
        // Same-origin path to the service registry, so the browser serviceRegistry client needs no
        // cross-origin host / CORS. SSE (/registry/events) passes through unbuffered.
        '/registry': {
          target: REGISTRY_URL,
          changeOrigin: true,
          rewrite: (p) => p.replace(/^\/registry/, ''),
        },
        '/health': TS,
        // `/api-docs` must precede `/api`: Vite matches proxy keys by prefix in
        // insertion order, so `/api` would otherwise swallow `/api-docs` and send
        // the SPA page navigation to the backend (which serves the production
        // dist index.html at 404 — it can't boot under the dev server).
        '/api-docs': {
          target: TS,
          bypass(req) {
            // `/api-docs` is also an SPA page (the OpenAPI viewer). A browser page
            // navigation/reload sends Accept: text/html — serve the SPA so a hard
            // load or bookmark of /api-docs shows the viewer, not a backend 404.
            // Match only the bare page path (ignoring the query string): the
            // sub-path /api-docs/openapi.json must always proxy to the backend,
            // including when opened directly in a browser tab (also text/html).
            const path = (req.url || '').split('?')[0];
            if (path === '/api-docs' && req.headers.accept?.includes('text/html')) {
              return '/index.html';
            }
          },
        },
        '/api': TS,
        '/sparql': {
          target: TS,
          bypass(req) {
            // Browser page navigations send Accept: text/html — serve the SPA
            // so that reloading /sparql shows the frontend, not the backend endpoint.
            if (req.headers.accept?.includes('text/html')) {
              return '/index.html';
            }
          },
        },
        '/store': TS,
        '/resource/': TS,
        '/.well-known': TS,
      },
    },
    build: {
      outDir: 'dist',
      emptyOutDir: true,
      chunkSizeWarningLimit: 2000,
      rollupOptions: {
        output: {
          // W4-20: Split heavy vendor libraries into separate chunks so they are
          // only downloaded when the corresponding page is first visited.
          manualChunks(id) {
            if (id.includes('node_modules/codemirror') || id.includes('node_modules/@codemirror')) {
              return 'codemirror';
            }
            if (id.includes('node_modules/cytoscape')) {
              return 'cytoscape';
            }
          },
        },
      },
    },
    test: {
      environment: 'jsdom',
      globals: true,
      setupFiles: ['./src/lib/__tests__/setup.ts'],
      // Vitest owns unit tests under src/; Playwright (npm run e2e) owns e2e/.
      // Without this, vitest's default glob picks up e2e/*.spec.ts and crashes
      // because Playwright's test() can't run under vitest.
      include: ['src/**/*.{test,spec}.{js,ts}'],
    },
  };
});
