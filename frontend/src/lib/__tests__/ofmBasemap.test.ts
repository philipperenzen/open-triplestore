import { describe, it, expect, vi, afterEach } from 'vitest';
import { get } from 'svelte/store';
import {
  sourceTile,
  roadVisible,
  placeVisible,
  ofmTileTemplate,
  OFM_CREDIT_HTML,
  OFM_MAX_ZOOM,
} from '../viewer/ofmRaster';
import { effectiveBasemap, esriImageryUrl, styleFor, LIGHT_STYLE_URL } from '../viewer/basemaps';

describe('OpenFreeMap raster tiles', () => {
  it('draws a tile beyond the served zoom from its ancestor', () => {
    expect(sourceTile(10, 3, 5)).toEqual({ z: 10, x: 3, y: 5, scale: 1, offsetX: 0, offsetY: 0 });
    const deep = sourceTile(OFM_MAX_ZOOM + 2, 4 * 100 + 3, 4 * 200 + 1);
    expect(deep).toEqual({ z: OFM_MAX_ZOOM, x: 100, y: 200, scale: 4, offsetX: 3, offsetY: 1 });
  });

  it('shows bigger roads and places first', () => {
    expect(roadVisible('motorway', 6)).toBe(true);
    expect(roadVisible('primary', 7)).toBe(false);
    expect(roadVisible('minor', 11)).toBe(false);
    expect(roadVisible('minor', 12)).toBe(true);
    expect(placeVisible('city', 4)).toBe(true);
    expect(placeVisible('village', 10)).toBe(false);
  });

  it('credits OpenFreeMap, OpenMapTiles and OpenStreetMap', () => {
    expect(OFM_CREDIT_HTML).toContain('OpenFreeMap');
    expect(OFM_CREDIT_HTML).toContain('OpenMapTiles');
    expect(OFM_CREDIT_HTML).toContain('openstreetmap.org/copyright');
  });

  it('takes the tile URL from the TileJSON, and retries after a failure', async () => {
    const failing = vi.fn().mockResolvedValue({ ok: false, status: 503 });
    await expect(ofmTileTemplate(failing as unknown as typeof fetch)).rejects.toThrow('503');
    const ok = vi.fn().mockResolvedValue({
      ok: true,
      json: async () => ({ tiles: ['https://tiles.example/planet/v1/{z}/{x}/{y}.pbf'] }),
    });
    await expect(ofmTileTemplate(ok as unknown as typeof fetch)).resolves.toBe(
      'https://tiles.example/planet/v1/{z}/{x}/{y}.pbf',
    );
  });
});

describe('satellite basemap', () => {
  it('is offered only with an Esri key', () => {
    expect(effectiveBasemap('satellite', null)).toBe('streets');
    expect(effectiveBasemap('satellite', 'k')).toBe('satellite');
    expect(styleFor('satellite', false, null)).toBe(LIGHT_STYLE_URL);
    const style = styleFor('satellite', false, 'my key');
    expect(typeof style).toBe('object');
    expect(JSON.stringify(style)).toContain(esriImageryUrl('my key'));
    expect(esriImageryUrl('my key')).toContain('token=my%20key');
  });
});

describe('runtime basemap settings', () => {
  afterEach(() => {
    vi.restoreAllMocks();
    vi.resetModules();
  });

  it('reads the Esri key from /config.json', async () => {
    global.fetch = vi.fn().mockResolvedValue({
      ok: true,
      headers: { get: () => 'application/json' },
      json: async () => ({ basemaps: { esriApiKey: ' AAPTkey ' } }),
    });
    vi.resetModules();
    const { loadRuntimeConfig, runtimeBasemaps } = await import('../runtimeConfig.js');
    expect(get(runtimeBasemaps).esriApiKey).toBeNull();
    loadRuntimeConfig();
    await vi.waitFor(() => expect(get(runtimeBasemaps).esriApiKey).toBe('AAPTkey'));
  });
});
