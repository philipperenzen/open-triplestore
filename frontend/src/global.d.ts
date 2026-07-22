// Ambient module declarations for non-code side-effect imports.

// CSS imported for its side effects (Vite handles the actual bundling).
declare module '*.css';
declare module '*.css?*';

// Image assets imported for their URL. Vite resolves these at build time to
// either a hashed `assets/…` path or an inlined `data:` URI, which is what makes
// them correct under a sub-path deployment (OTS_BASE_PATH) — see
// lib/viewer/leafletIcons.ts, where guessing the path at runtime is exactly the
// bug that broke the map markers.
declare module '*.png' {
  const src: string;
  export default src;
}
declare module '*.svg' {
  const src: string;
  export default src;
}
declare module '*.webp' {
  const src: string;
  export default src;
}

// Historical: Leaflet used to be CDN-loaded onto the global `window` as `L`. It
// is a bundled npm dependency now (imported through lib/viewer/leafletIcons.ts),
// but the optional global is kept declared because third-party embeds still set it.
interface Window {
  L?: any;
}

// Build-time constant injected by Vite (`define` in vite.config.js): true only when the
// LD_DISCOVERY opt-in is set. Gates the service-registry client so it contacts no registry
// (and opens no SSE) unless discovery is explicitly enabled.
declare const __LD_DISCOVERY__: boolean;
