// Raster basemap tiles drawn in the browser from OpenFreeMap's vector tiles.
//
// The 3D globe (Cesium) and the Leaflet previews take raster tiles, and every
// keyless raster street map we used is gone or off limits: CARTO watermarks
// keyless tiles since September 2026 ("API KEY REQUIRED"), and the
// OpenStreetMap tile servers refuse app traffic. OpenFreeMap serves the
// OpenMapTiles vector schema with no key and no limit — the 2D viewer already
// draws it through MapLibre — so this module paints those tiles onto a canvas,
// in the same light and dark colours, for the viewers that need pixels.
//
// Attribution (shown on every map that uses it): OpenFreeMap, © OpenMapTiles,
// data from OpenStreetMap — see OFM_CREDIT_HTML.

import { VectorTile, type VectorTileFeature } from '@mapbox/vector-tile';
import { PbfReader } from 'pbf';

/** TileJSON of OpenFreeMap's planet tiles; names the current tile URL. */
export const OFM_TILEJSON = 'https://tiles.openfreemap.org/planet';
/** Deepest zoom OpenFreeMap serves; deeper tiles are drawn from these. */
export const OFM_MAX_ZOOM = 14;
/** The credit every map drawing these tiles shows. */
export const OFM_CREDIT_HTML =
  '<a href="https://openfreemap.org" target="_blank" rel="noopener">OpenFreeMap</a> ' +
  '<a href="https://www.openmaptiles.org/" target="_blank" rel="noopener">© OpenMapTiles</a> ' +
  'Data from <a href="https://www.openstreetmap.org/copyright" target="_blank" rel="noopener">OpenStreetMap</a>';

export interface Palette {
  background: string;
  wood: string;
  grass: string;
  residential: string;
  park: string;
  water: string;
  building: string;
  buildingEdge: string;
  roadMinor: string;
  roadSecondary: string;
  roadPrimary: string;
  roadMotorway: string;
  casing: string | null;
  rail: string;
  boundary: string;
  label: string;
  waterLabel: string;
  halo: string;
}

/** Light: close to OpenFreeMap Liberty, the 2D viewer's light style. */
export const LIGHT: Palette = {
  background: '#f2efe9',
  wood: '#cfe4c1',
  grass: '#dcecce',
  residential: '#ece7df',
  park: '#d3eac4',
  water: '#a9d3e8',
  building: '#dcd4cc',
  buildingEdge: '#cbc1b6',
  roadMinor: '#ffffff',
  roadSecondary: '#fdf5c2',
  roadPrimary: '#fcd9a6',
  roadMotorway: '#f0a6a0',
  casing: '#d2cabe',
  rail: '#a8a29a',
  boundary: '#9e8fb2',
  label: '#3b3b3b',
  waterLabel: '#3f78a0',
  halo: '#ffffff',
};

/** Dark: the colours of the 2D viewer's custom dark style (basemaps.ts). */
export const DARK: Palette = {
  background: '#0b1118',
  wood: '#13231b',
  grass: '#13251d',
  residential: '#111923',
  park: '#142a1f',
  water: '#16374f',
  building: '#1c2735',
  buildingEdge: '#0d141c',
  roadMinor: '#27313d',
  roadSecondary: '#33445a',
  roadPrimary: '#48597a',
  roadMotorway: '#7a6334',
  casing: null,
  rail: '#2c3543',
  boundary: '#3c4c61',
  label: '#93a7bb',
  waterLabel: '#5f93b4',
  halo: '#0b1118',
};

let templatePromise: Promise<string> | null = null;

/** The `{z}/{x}/{y}` tile URL template OpenFreeMap currently serves. */
export function ofmTileTemplate(fetchImpl: typeof fetch = fetch): Promise<string> {
  if (!templatePromise) {
    templatePromise = fetchImpl(OFM_TILEJSON)
      .then((r) => {
        if (!r.ok) throw new Error(`OpenFreeMap TileJSON: HTTP ${r.status}`);
        return r.json();
      })
      .then((doc: { tiles?: string[] }) => {
        const t = doc.tiles?.[0];
        if (!t) throw new Error('OpenFreeMap TileJSON names no tiles');
        return t;
      })
      .catch((e) => {
        templatePromise = null; // retry on the next tile
        throw e;
      });
  }
  return templatePromise;
}

/**
 * The OpenFreeMap tile to draw `z/x/y` from: itself up to OFM_MAX_ZOOM, else
 * its ancestor at that zoom, with the part of it `z/x/y` covers.
 */
export function sourceTile(z: number, x: number, y: number) {
  const dz = Math.max(0, z - OFM_MAX_ZOOM);
  const sz = z - dz;
  const sx = x >> dz;
  const sy = y >> dz;
  return { z: sz, x: sx, y: sy, scale: 2 ** dz, offsetX: x - (sx << dz), offsetY: y - (sy << dz) };
}

const TILE_CACHE_LIMIT = 256;
const tileCache = new Map<string, Promise<VectorTile | null>>();

function vectorTile(z: number, x: number, y: number): Promise<VectorTile | null> {
  const key = `${z}/${x}/${y}`;
  const hit = tileCache.get(key);
  if (hit) {
    tileCache.delete(key); // refresh its place in the LRU order
    tileCache.set(key, hit);
    return hit;
  }
  const p = ofmTileTemplate()
    .then((t) => fetch(t.replace('{z}', String(z)).replace('{x}', String(x)).replace('{y}', String(y))))
    .then(async (r) => {
      if (r.status === 404 || r.status === 204) return null; // nothing there (open sea)
      if (!r.ok) throw new Error(`OpenFreeMap tile ${key}: HTTP ${r.status}`);
      return new VectorTile(new PbfReader(new Uint8Array(await r.arrayBuffer())));
    })
    .catch((e) => {
      tileCache.delete(key);
      throw e;
    });
  tileCache.set(key, p);
  while (tileCache.size > TILE_CACHE_LIMIT) {
    const oldest = tileCache.keys().next().value;
    if (oldest === undefined) break;
    tileCache.delete(oldest);
  }
  return p;
}

/** Line width in pixels of a road class at zoom `z` (for a 256 px tile). */
export function roadWidth(cls: string, z: number): number {
  const base: Record<string, number> = {
    motorway: 2.4,
    trunk: 2.2,
    primary: 2,
    secondary: 1.6,
    tertiary: 1.3,
    minor: 1,
    service: 0.6,
    track: 0.5,
    path: 0.5,
  };
  const b = base[cls] ?? 0.8;
  // Roughly doubles every two zoom levels from z10, as the vector styles do,
  // up to six times its z10 width.
  const f = Math.min(6, Math.max(0.35, 2 ** ((z - 10) / 2)));
  return b * f;
}

/** The colour a road class is drawn in. */
export function roadColor(cls: string, p: Palette): string {
  if (cls === 'motorway' || cls === 'trunk') return p.roadMotorway;
  if (cls === 'primary') return p.roadPrimary;
  if (cls === 'secondary' || cls === 'tertiary') return p.roadSecondary;
  return p.roadMinor;
}

/** Which road classes show at zoom `z`. */
export function roadVisible(cls: string, z: number): boolean {
  if (cls === 'motorway') return z >= 6;
  if (cls === 'trunk') return z >= 7;
  if (cls === 'primary') return z >= 8;
  if (cls === 'secondary') return z >= 9;
  if (cls === 'tertiary') return z >= 10;
  if (cls === 'minor' || cls === 'service') return z >= 12;
  return z >= 14;
}

/** Which place classes are labelled at zoom `z`. */
export function placeVisible(cls: string, z: number): boolean {
  switch (cls) {
    case 'continent':
      return z <= 2;
    case 'country':
      return z >= 2 && z <= 6;
    case 'state':
      return z >= 5 && z <= 8;
    case 'city':
      return z >= 4;
    case 'town':
      return z >= 8;
    case 'village':
      return z >= 11;
    case 'suburb':
    case 'hamlet':
    case 'neighbourhood':
      return z >= 13;
    default:
      return false;
  }
}

function prop(f: VectorTileFeature, k: string): string {
  const v = f.properties[k];
  return v === undefined ? '' : String(v);
}

/**
 * Draw the OpenFreeMap tile covering `z/x/y` onto a new `size`×`size`
 * canvas. Resolves to a canvas filled with water when there is no tile (open
 * sea), and rejects when the tile cannot be fetched.
 */
export async function renderOfmTile(
  z: number,
  x: number,
  y: number,
  size: number,
  palette: Palette,
): Promise<HTMLCanvasElement> {
  const src = sourceTile(z, x, y);
  const tile = await vectorTile(src.z, src.x, src.y);
  const canvas = document.createElement('canvas');
  canvas.width = size;
  canvas.height = size;
  const ctx = canvas.getContext('2d');
  if (!ctx) return canvas;
  // No tile at all is open sea.
  ctx.fillStyle = tile ? palette.background : palette.water;
  ctx.fillRect(0, 0, size, size);
  if (!tile) return canvas;

  const px = (v: number, extent: number, offset: number) => ((v / extent) * src.scale - offset) * size;
  // Widths are designed for a 256 px tile.
  const k = size / 256;

  const path = (f: VectorTileFeature) => {
    const e = f.extent;
    ctx.beginPath();
    for (const ring of f.loadGeometry()) {
      ring.forEach((pt, i) => {
        const X = px(pt.x, e, src.offsetX);
        const Y = px(pt.y, e, src.offsetY);
        if (i === 0) ctx.moveTo(X, Y);
        else ctx.lineTo(X, Y);
      });
      if (f.type === 3) ctx.closePath();
    }
  };
  const each = (layer: string, fn: (f: VectorTileFeature) => void) => {
    const l = tile.layers[layer];
    if (!l) return;
    for (let i = 0; i < l.length; i++) fn(l.feature(i));
  };
  const fillLayer = (layer: string, color: (f: VectorTileFeature) => string | null) =>
    each(layer, (f) => {
      if (f.type !== 3) return;
      const c = color(f);
      if (!c) return;
      path(f);
      ctx.fillStyle = c;
      ctx.fill('evenodd');
    });

  fillLayer('landcover', (f) => {
    const cls = prop(f, 'class');
    if (cls === 'wood' || cls === 'forest') return palette.wood;
    if (cls === 'grass' || cls === 'farmland' || cls === 'wetland') return palette.grass;
    return null;
  });
  fillLayer('landuse', (f) =>
    ['residential', 'suburbs', 'neighbourhood'].includes(prop(f, 'class')) ? palette.residential : null,
  );
  fillLayer('park', () => palette.park);
  fillLayer('water', () => palette.water);
  ctx.lineCap = 'round';
  ctx.lineJoin = 'round';
  each('waterway', (f) => {
    if (f.type !== 2) return;
    path(f);
    ctx.strokeStyle = palette.water;
    ctx.lineWidth = Math.max(0.5, roadWidth('minor', z) * 0.8) * k;
    ctx.stroke();
  });
  if (z >= 13) {
    each('building', (f) => {
      if (f.type !== 3) return;
      path(f);
      ctx.fillStyle = palette.building;
      ctx.fill('evenodd');
      if (z >= 15) {
        ctx.strokeStyle = palette.buildingEdge;
        ctx.lineWidth = 0.5 * k;
        ctx.stroke();
      }
    });
  }
  each('boundary', (f) => {
    const level = Number(f.properties.admin_level ?? 99);
    // Countries everywhere; states and provinces from z5.
    if (f.type !== 2 || level > (z >= 5 ? 4 : 2) || f.properties.maritime === 1) return;
    path(f);
    ctx.setLineDash([4 * k, 3 * k]);
    ctx.strokeStyle = palette.boundary;
    ctx.lineWidth = (level <= 2 ? 1.2 : 0.8) * k;
    ctx.stroke();
    ctx.setLineDash([]);
  });

  // Roads: minor first, so bigger roads draw over them; a casing on light.
  const order = ['path', 'track', 'service', 'minor', 'tertiary', 'secondary', 'primary', 'trunk', 'motorway'];
  const roads: VectorTileFeature[] = [];
  each('transportation', (f) => {
    if (f.type === 2) roads.push(f);
  });
  roads.sort((a, b) => order.indexOf(prop(a, 'class')) - order.indexOf(prop(b, 'class')));
  // All casings first, then all fills: casing each segment right before its
  // fill would draw it over the neighbouring segment's fill (a beaded road).
  for (const pass of ['casing', 'fill'] as const) {
    for (const f of roads) {
      const cls = prop(f, 'class');
      if (cls === 'rail' || cls === 'transit') {
        if (pass === 'casing' || z < 11) continue;
        path(f);
        ctx.setLineDash([3 * k, 3 * k]);
        ctx.strokeStyle = palette.rail;
        ctx.lineWidth = 1 * k;
        ctx.stroke();
        ctx.setLineDash([]);
        continue;
      }
      if (!roadVisible(cls, z)) continue;
      const w = roadWidth(cls, z) * k;
      if (pass === 'casing') {
        if (!palette.casing || w <= 1.5) continue;
        path(f);
        ctx.strokeStyle = palette.casing;
        ctx.lineWidth = w + 1.2 * k;
        ctx.stroke();
      } else {
        path(f);
        ctx.strokeStyle = roadColor(cls, palette);
        ctx.lineWidth = w;
        ctx.stroke();
      }
    }
  }

  // Labels: places (most important first) and named water, with a halo. A
  // label that would overlap one already drawn in this tile is left out.
  const placed: [number, number, number, number][] = [];
  const label = (f: VectorTileFeature, text: string, color: string, fontPx: number, bold: boolean) => {
    const [ring] = f.loadGeometry();
    const pt = ring?.[0];
    if (!pt || !text) return;
    const X = px(pt.x, f.extent, src.offsetX);
    const Y = px(pt.y, f.extent, src.offsetY);
    if (X < 0 || Y < 0 || X > size || Y > size) return;
    ctx.font = `${bold ? '600 ' : ''}${fontPx * k}px "Noto Sans", system-ui, sans-serif`;
    const w = ctx.measureText(text).width + 4 * k;
    const h = fontPx * k * 1.3;
    const box: [number, number, number, number] = [X - w / 2, Y - h / 2, X + w / 2, Y + h / 2];
    if (placed.some((b) => box[0] < b[2] && box[2] > b[0] && box[1] < b[3] && box[3] > b[1])) return;
    placed.push(box);
    ctx.textAlign = 'center';
    ctx.textBaseline = 'middle';
    ctx.lineWidth = 3 * k;
    ctx.strokeStyle = palette.halo;
    ctx.strokeText(text, X, Y);
    ctx.fillStyle = color;
    ctx.fillText(text, X, Y);
  };
  const importance = ['continent', 'country', 'state', 'city', 'town', 'village', 'suburb', 'hamlet', 'neighbourhood'];
  const places: VectorTileFeature[] = [];
  each('place', (f) => {
    if (f.type === 1 && placeVisible(prop(f, 'class'), z)) places.push(f);
  });
  places.sort(
    (a, b) =>
      importance.indexOf(prop(a, 'class')) - importance.indexOf(prop(b, 'class')) ||
      Number(a.properties.rank ?? 99) - Number(b.properties.rank ?? 99),
  );
  for (const f of places) {
    const cls = prop(f, 'class');
    const big = cls === 'city' || cls === 'country' || cls === 'continent' || cls === 'state';
    const text = prop(f, 'name:latin') || prop(f, 'name');
    label(f, cls === 'country' ? text.toUpperCase() : text, palette.label, big ? 13 : 11, big);
  }
  each('water_name', (f) => {
    if (f.type === 1 && z >= 5) label(f, prop(f, 'name:latin') || prop(f, 'name'), palette.waterLabel, 11, false);
  });
  return canvas;
}
