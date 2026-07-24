import { describe, it, expect } from 'vitest';
import L from '../viewer/leafletIcons';

// Regression guard for the "the map cannot display the image" report: in a built
// app Leaflet's runtime icon-path guessing collapses to an empty imagePath, so
// every marker asked for a *document-relative* `marker-icon.png`, which the SPA
// fallback answered with index.html and the browser drew as a broken image.
// These assertions fail against a plain `import L from 'leaflet'`.
describe('leafletIcons', () => {
  it('drops the Icon.Default override that prefixes the guessed imagePath', () => {
    expect(Object.prototype.hasOwnProperty.call(L.Icon.Default.prototype, '_getIconUrl')).toBe(false);
    // Falling through to Icon's own implementation is what makes the options be
    // used verbatim instead of being concatenated onto Icon.Default.imagePath.
    expect(L.Icon.Default.prototype._getIconUrl).toBe(L.Icon.prototype._getIconUrl);
  });

  it('replaces the bare filename defaults with bundler-resolved URLs', () => {
    const { iconUrl, iconRetinaUrl, shadowUrl } = L.Icon.Default.prototype.options;
    expect(iconUrl).not.toBe('marker-icon.png');
    expect(iconRetinaUrl).not.toBe('marker-icon-2x.png');
    expect(shadowUrl).not.toBe('marker-shadow.png');
    for (const url of [iconUrl, iconRetinaUrl, shadowUrl]) {
      expect(typeof url).toBe('string');
      expect(url.length).toBeGreaterThan(0);
    }
  });

  it('resolves icon and shadow to an absolute or data URL, never a bare filename', () => {
    // A bare "marker-icon.png" resolves against whatever route the SPA happens to
    // be on (e.g. /datasets/<slug>/viewer) — that is the whole bug. Anything
    // rooted (data:, http(s):, or a leading slash) is safe.
    const rooted = /^(data:|blob:|https?:|\/)/;
    const icon = new L.Icon.Default();
    expect(icon._getIconUrl('icon')).toMatch(rooted);
    expect(icon._getIconUrl('shadow')).toMatch(rooted);
  });

  it('does not consult the CSS path-guessing heuristic at all', () => {
    // _detectIconPath() is only reachable through the override we deleted, so
    // Icon.Default.imagePath must stay untouched. If some future refactor puts
    // the override back, this catches it before production does.
    L.Icon.Default.imagePath = undefined;
    expect(new L.Icon.Default()._getIconUrl('icon')).not.toBe('undefinedmarker-icon.png');
    expect(L.Icon.Default.imagePath).toBeUndefined();
  });
});
