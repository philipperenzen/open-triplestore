// Point MapLibre at a worker bundle Vite actually emits.
//
// maplibre-gl 6 moved its worker OUT of the main bundle: instead of inlining it
// as a blob (v5), it resolves `new URL('./maplibre-gl-worker.mjs',
// import.meta.url)` at runtime. That path only exists if the bundler happens to
// copy the file next to the emitted chunk — Vite does not, because nothing
// statically imports it. The request then lands on the SPA's catch-all route and
// the worker is handed `index.html`, so it dies on the first line.
//
// The failure is SILENT and total: MapLibre reports no error, `isStyleLoaded()`
// simply never becomes true, no tile is ever requested, and the map renders an
// empty canvas. That is the "the background map doesn't load" bug.
//
// `?worker&url` makes Vite bundle the worker together with the shared chunk it
// imports and hand back the hashed URL of the emitted file, which is exactly
// what `config.WORKER_URL` wants.
import maplibreWorkerUrl from 'maplibre-gl/dist/maplibre-gl-worker.mjs?worker&url';
import * as maplibregl from 'maplibre-gl';

let applied = false;

/**
 * Call once before constructing a `maplibregl.Map`. Idempotent, so every
 * MapLibre entry point can call it without coordinating.
 */
export function configureMapLibreWorker(): void {
  if (applied) return;
  applied = true;
  // `config` is MapLibre's global settings object; WORKER_URL overrides the
  // import.meta.url guess above.
  (maplibregl as unknown as { config: { WORKER_URL?: string } }).config.WORKER_URL =
    maplibreWorkerUrl;
}

/** The emitted worker URL — exported for the build-time regression test. */
export const MAPLIBRE_WORKER_URL = maplibreWorkerUrl;
