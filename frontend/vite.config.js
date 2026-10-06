import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';
import tailwindcss from '@tailwindcss/vite';
import fs from 'node:fs';
import path from 'node:path';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';
import { otherBundledMaterial, thirdPartyLicenses } from './scripts/third-party-licenses.mjs';
import { normaliseBasePath } from './scripts/base-path.mjs';

// ── Cesium runtime assets, served from this origin ────────────────────────────
//
// Cesium loads its web workers, shaders, widget CSS and 3D-Tiles/terrain helper
// assets at runtime relative to `window.CESIUM_BASE_URL`, outside the module
// graph, so a bundler never sees them. The viewer used to point that at a CDN
// pinned to a literal version — which had drifted 21 minor releases behind the
// `cesium` package npm actually installed, and which made the globe depend on
// internet access. This plugin serves the installed package's own Build/Cesium
// runtime directories under /cesium/ in dev, and copies them into dist/cesium
// at build time, so the assets always match the engine and work air-gapped.
const CESIUM_RUNTIME_DIRS = ['Workers', 'Assets', 'ThirdParty', 'Widgets'];

// ── Sub-path deploys ─────────────────────────────────────────────────────────
//
// OTS_BASE_PATH publishes the UI under a path prefix (`/ots/` →
// https://example.org/ots/…; see docs/operations.md). Vite wants it with a
// leading and a trailing slash, so `ots`, `/ots` and `/ots/` all mean the same.
// The app reads it back as import.meta.env.BASE_URL (src/lib/basePath.ts).
const BASE = normaliseBasePath(process.env.OTS_BASE_PATH);
// `/ots` (no trailing slash), or '' at the root: the prefix dev-server paths get.
const BASE_PREFIX = BASE.replace(/\/$/, '');

// The dev server under a base: every proxied backend path moves below the
// prefix too, and the prefix is cut off again before the request reaches the
// backend, the same as the production reverse proxy does.
function proxyUnderBase(proxy) {
  if (!BASE_PREFIX) return proxy;
  const out = {};
  for (const [key, value] of Object.entries(proxy)) {
    const opts = typeof value === 'string' ? { target: value } : { ...value };
    const inner = opts.rewrite;
    opts.rewrite = (p) => {
      const stripped = p.startsWith(BASE_PREFIX) ? p.slice(BASE_PREFIX.length) || '/' : p;
      return inner ? inner(stripped) : stripped;
    };
    if (opts.bypass) {
      const bypass = opts.bypass;
      // The bypass rules are written against app paths (`/api-docs`): show
      // them the request without the prefix, and put it back on a rewrite.
      opts.bypass = (req, res, o) => {
        const url = req.url || '';
        req.url = url.startsWith(BASE_PREFIX) ? url.slice(BASE_PREFIX.length) || '/' : url;
        try {
          const r = bypass(req, res, o);
          return typeof r === 'string' ? `${BASE_PREFIX}${r}` : r;
        } finally {
          req.url = url;
        }
      };
    }
    out[`${BASE_PREFIX}${key}`] = opts;
  }
  return out;
}
const CESIUM_MIME = {
  '.js': 'text/javascript',
  '.mjs': 'text/javascript',
  '.wasm': 'application/wasm',
  '.json': 'application/json',
  '.css': 'text/css',
  '.png': 'image/png',
  '.jpg': 'image/jpeg',
  '.gif': 'image/gif',
  '.svg': 'image/svg+xml',
  '.xml': 'application/xml',
  '.glsl': 'text/plain',
  '.txt': 'text/plain',
};

function cesiumBuildDir() {
  const require = createRequire(import.meta.url);
  return path.join(path.dirname(require.resolve('cesium/package.json')), 'Build', 'Cesium');
}

function cesiumAssets() {
  const buildDir = cesiumBuildDir();
  let outDir = path.resolve('dist');
  return {
    name: 'ots-cesium-assets',
    configResolved(config) {
      // `vite build --outDir …` (the sub-path smoke build) must get its own copy.
      outDir = path.resolve(config.root, config.build.outDir);
    },
    configureServer(server) {
      // Plugin middlewares run before Vite strips the base, so mount below it.
      server.middlewares.use(`${BASE_PREFIX}/cesium`, (req, res, next) => {
        const rel = decodeURIComponent((req.url || '/').split('?')[0]);
        const file = path.normalize(path.join(buildDir, rel));
        // Containment + only the runtime directories, never the whole package.
        if (!file.startsWith(buildDir + path.sep)) return next();
        if (!CESIUM_RUNTIME_DIRS.some((d) => file.startsWith(path.join(buildDir, d) + path.sep))) {
          return next();
        }
        fs.stat(file, (err, st) => {
          if (err || !st.isFile()) return next();
          res.setHeader('Content-Type', CESIUM_MIME[path.extname(file)] || 'application/octet-stream');
          res.setHeader('Cache-Control', 'public, max-age=3600');
          fs.createReadStream(file).pipe(res);
        });
      });
    },
    closeBundle() {
      for (const d of CESIUM_RUNTIME_DIRS) {
        fs.cpSync(path.join(buildDir, d), path.join(outDir, 'cesium', d), { recursive: true });
      }
    },
  };
}

// One-shot resolve of the service registry at dev-server startup so the backend proxy
// targets can follow discovery. Falls back to {} (→ the localhost default) when it's down.
const REGISTRY_URL = (process.env.LD_REGISTRY_URL || 'http://localhost:8500').replace(/\/+$/, '');

// Cross-app service discovery is OPT-IN. Off by default so a registry that isn't running can't
// spam the dev console with proxy ECONNREFUSED (/resolve, /events). Enable with LD_DISCOVERY=1
// (see README / .env.example) to mount the /registry proxy and let the UI resolve sibling apps.
const DISCOVERY = /^(1|true|yes|on)$/i.test((process.env.LD_DISCOVERY || '').trim());

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

// ── Third-party licence notices ────────────────────────────────────────────────
//
// The minifier strips the @license comments of the npm packages it bundles, so
// the build writes their licence and notice files to dist/THIRD-PARTY-LICENSES.txt
// instead (see scripts/third-party-licenses.mjs). The plugin records what the
// bundler actually put into the main bundle and each web worker; the packages
// below reach dist/ outside the module graph and are added explicitly, and so is
// the third-party material no npm package covers (web-ifc.wasm's statically
// linked libraries, inline icon shapes, EPSG parameters), whose licence texts
// come from the repository's LICENSES/ directory: the build fails without it.
const licenses = thirdPartyLicenses({
  extraPackages: [
    // cesiumAssets() copies Build/Cesium's runtime into dist/cesium; that build
    // contains CesiumJS and the packages it depends on.
    { name: 'cesium', withDependencies: true, reason: 'runtime assets copied to dist/cesium' },
    // @tailwindcss/vite compiles Tailwind's base styles into the stylesheet.
    { name: 'tailwindcss', reason: 'base styles compiled into dist/assets/*.css' },
  ],
  cesiumRuntimeDir: cesiumBuildDir(),
  otherMaterial: otherBundledMaterial(fileURLToPath(new URL('../LICENSES', import.meta.url))),
});

export default defineConfig(async () => {
  // Only probe the registry when discovery is enabled — otherwise skip the startup round-trip.
  const reg = DISCOVERY ? await resolveRegistry() : {};
  // The triplestore backend ("triplestore" in the registry); defaults to the local dev port.
  const TS = reg.triplestore || (process.env.OTS_BACKEND_URL ?? 'http://localhost:7878');
  return {
    // Serve/build under a sub-path (a reverse proxy or a static host that
    // publishes this app at https://host/some-project/). Defaults to root, so
    // every existing deployment is unaffected. Set OTS_BASE_PATH=/my-path/ at
    // build time to change it (see BASE above and docs/operations.md).
    base: BASE,
    // Expose the opt-in flag to the browser bundle so serviceRegistry.ts only contacts the
    // registry when discovery is on (otherwise no /registry/events SSE reconnect loop, no noise).
    define: { __LD_DISCOVERY__: JSON.stringify(DISCOVERY) },
    plugins: [tailwindcss(), svelte(), cesiumAssets(), licenses.plugin()],
    // Web workers are separate bundles: record their third-party modules too.
    worker: { plugins: () => [licenses.workerPlugin()] },
    server: {
      // --no-reload (LD_NO_HMR=1) turns off hot module reload while keeping the dev server + proxy.
      hmr: process.env.LD_NO_HMR === '1' ? false : undefined,
      port: 5173,
      proxy: proxyUnderBase({
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
        // Optional cross-app registry proxy — only mounted when LD_DISCOVERY is set, so a registry
        // that isn't running can't spam the console. The browser serviceRegistry client uses this
        // same-origin path (no CORS); SSE (/registry/events) passes through unbuffered.
        ...(DISCOVERY
          ? {
              '/registry': {
                target: REGISTRY_URL,
                changeOrigin: true,
                rewrite: (p) => p.replace(/^\/registry/, ''),
              },
            }
          : {}),
      }),
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
            if (id.includes('node_modules/three')) {
              return 'three';
            }
            if (id.includes('node_modules/maplibre-gl')) {
              return 'maplibre';
            }
            if (id.includes('node_modules/leaflet')) {
              return 'leaflet';
            }
          },
        },
      },
    },
    // Svelte 5 ships separate client/server builds; vitest must resolve the
    // *client* ("browser") build or component tests fail with
    // "mount(...) is not available on the server".
    resolve: process.env.VITEST ? { conditions: ['browser'] } : undefined,
    test: {
      environment: 'jsdom',
      globals: true,
      setupFiles: ['./src/lib/__tests__/setup.ts'],
      // Vitest owns unit tests under src/; Playwright (npm run e2e) owns e2e/.
      // Without this, vitest's default glob picks up e2e/*.spec.ts and crashes
      // because Playwright's test() can't run under vitest.
      // scripts/*.test.mjs cover the build scripts (e.g. the licence notices).
      include: ['src/**/*.{test,spec}.{js,ts}', 'scripts/**/*.test.mjs'],
    },
  };
});
