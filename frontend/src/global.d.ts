// Ambient module declarations for non-code side-effect imports.

// CSS imported for its side effects (Vite handles the actual bundling).
declare module '*.css';
declare module '*.css?*';

// Leaflet attaches itself to the global `window` as `L` when loaded via CDN.
interface Window {
  L?: any;
}
