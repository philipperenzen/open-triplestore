// The single sanctioned entry point for Leaflet in this app: it re-exports the
// library with its *default marker icons* wired to bundler-resolved URLs.
//
// Why this exists. Leaflet does not know where its own images live; it guesses.
// `Icon.Default._getIconUrl` prefixes the bare option values ('marker-icon.png',
// 'marker-icon-2x.png', 'marker-shadow.png') with `Icon.Default.imagePath`,
// which `_detectIconPath()` derives by reading the computed background-image of
// a throwaway `.leaflet-default-icon-path` element — a rule leaflet.css declares
// as `url(images/marker-icon.png)` purely so this heuristic has something to
// read. The heuristic cannot survive Vite:
//
//   * all three PNGs are smaller than Vite's 4 KB `assetsInlineLimit`, so the
//     bundled CSS carries a `data:image/png;base64,…` URI, and `_stripUrl()`
//     only accepts a value that literally ends in "marker-icon.png";
//   * its fallback, `document.querySelector('link[href$="leaflet.css"]')`,
//     misses too because the stylesheet is emitted as `assets/leaflet-<hash>.css`
//     (vite.config.js gives Leaflet its own manual chunk).
//
// `imagePath` therefore lands on the empty string and is cached forever, so every
// marker requests a *document-relative* `marker-icon.png`. The SPA fallback route
// answers that with index.html (text/html + `x-content-type-options: nosniff`),
// which the browser refuses to decode as an image — the user sees a broken-image
// placeholder where the map pin should be. It only reproduces in a built app:
// under `vite dev` the CSS url() resolves to a real …/marker-icon.png path, the
// heuristic succeeds, and the bug is invisible.
//
// The fix is to let the bundler resolve the images (which also keeps sub-path
// builds correct — see `base` / OTS_BASE_PATH in vite.config.js) and to drop the
// `Icon.Default` override so Leaflet uses those URLs verbatim instead of gluing
// a guessed prefix in front of them. Hand-written '/marker-icon.png' paths would
// break the moment the app is served from a sub-path.
import L from 'leaflet';
import iconUrl from 'leaflet/dist/images/marker-icon.png';
import iconRetinaUrl from 'leaflet/dist/images/marker-icon-2x.png';
import shadowUrl from 'leaflet/dist/images/marker-shadow.png';

// `Icon.Default` is a global singleton, so this patch must run exactly once and
// strictly before the first `L.marker(...)`. An ES module body gives us both for
// free. Deleting the subclass override falls back to `Icon.prototype._getIconUrl`,
// which returns the option value as-is; setting `imagePath = ''` instead would
// also work today but stays fragile, because that code path still concatenates
// `imagePath` in front of what are now absolute/data URLs.
delete L.Icon.Default.prototype._getIconUrl;
L.Icon.Default.mergeOptions({ iconUrl, iconRetinaUrl, shadowUrl });

export default L;
