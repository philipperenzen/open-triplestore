// Ambient module declarations for non-code side-effect imports.

// CSS imported for its side effects (Vite handles the actual bundling).
declare module '*.css';
declare module '*.css?*';

// Leaflet attaches itself to the global `window` as `L` when loaded via CDN.
interface Window {
  L?: any;
}

// Build-time constant injected by Vite (`define` in vite.config.js): true only when the
// LD_DISCOVERY opt-in is set. Gates the service-registry client so it contacts no registry
// (and opens no SSE) unless discovery is explicitly enabled.
declare const __LD_DISCOVERY__: boolean;
