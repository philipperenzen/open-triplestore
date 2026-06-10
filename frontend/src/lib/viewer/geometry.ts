// Pure helpers for the 3D & map viewer: convert viewer-feed elements (WGS84 WKT
// from /api/datasets/:id/viewer-feed) into Leaflet-ready latlngs and lay 3D
// models out on a simple grid. Kept free of Leaflet/three imports so it is unit
// testable in jsdom.

import { parseWktGeometry, type WktGeometry } from '../ontology/valueType';

export interface ViewerElement {
  id: string;
  label?: string | null;
  types?: string[];
  parent?: string | null;
  ifc_guid?: string | null;
  gltf_url?: string | null;
  ifc_url?: string | null;
  files?: [string, string][];
  source_crs?: string | null;
  wkt4326?: string | null;
  wkt3857?: string | null;
}

export type LatLng = [number, number];

export interface MapFeature {
  id: string;
  label: string;
  kind: 'point' | 'line' | 'polygon';
  /** Leaflet order: [lat, lng]. For polygons: outer ring only. */
  latlngs: LatLng[];
}

/** WKT (lon lat) coordinate pairs → Leaflet [lat, lng]. */
const toLatLng = (c: [number, number]): LatLng => [c[1], c[0]];

/** Flatten a parsed WKT geometry into one drawable map feature per element. */
export function toMapFeature(el: ViewerElement): MapFeature | null {
  if (!el.wkt4326) return null;
  const g: WktGeometry | null = parseWktGeometry(el.wkt4326);
  if (!g) return null;
  const label = el.label || el.id.split(/[/#]/).pop() || el.id;
  switch (g.kind) {
    case 'point':
      return { id: el.id, label, kind: 'point', latlngs: [toLatLng(g.coord)] };
    case 'multipoint':
      return { id: el.id, label, kind: 'point', latlngs: g.coords.map(toLatLng) };
    case 'linestring':
      return { id: el.id, label, kind: 'line', latlngs: g.coords.map(toLatLng) };
    case 'multilinestring':
      return { id: el.id, label, kind: 'line', latlngs: g.lines.flat().map(toLatLng) };
    case 'polygon':
      return { id: el.id, label, kind: 'polygon', latlngs: g.rings[0]?.map(toLatLng) ?? [] };
    case 'multipolygon':
      return {
        id: el.id,
        label,
        kind: 'polygon',
        latlngs: g.polygons[0]?.[0]?.map(toLatLng) ?? [],
      };
    default:
      return null;
  }
}

/** Bounding box over features as [[minLat, minLng], [maxLat, maxLng]], or null. */
export function featureBounds(features: MapFeature[]): [LatLng, LatLng] | null {
  let minLat = Infinity,
    minLng = Infinity,
    maxLat = -Infinity,
    maxLng = -Infinity;
  for (const f of features) {
    for (const [lat, lng] of f.latlngs) {
      if (!Number.isFinite(lat) || !Number.isFinite(lng)) continue;
      minLat = Math.min(minLat, lat);
      minLng = Math.min(minLng, lng);
      maxLat = Math.max(maxLat, lat);
      maxLng = Math.max(maxLng, lng);
    }
  }
  if (!Number.isFinite(minLat)) return null;
  return [
    [minLat, minLng],
    [maxLat, maxLng],
  ];
}

export interface ModelRef {
  id: string;
  label: string;
  url: string;
  format: 'gltf' | 'stl';
  /** Grid slot position in the 3D scene, [x, z]. */
  slot: [number, number];
}

/**
 * Elements with a loadable 3D model (glTF preferred, STL fallback from the FOG
 * file list), laid out on a √n×√n ground grid with `spacing` between slots.
 */
export function modelRefs(elements: ViewerElement[], spacing = 3): ModelRef[] {
  const withModels = elements
    .map((el) => {
      const stl = (el.files || []).find(([f]) => f.startsWith('Stl'))?.[1];
      if (el.gltf_url) return { el, url: el.gltf_url, format: 'gltf' as const };
      if (stl) return { el, url: stl, format: 'stl' as const };
      return null;
    })
    .filter((x): x is { el: ViewerElement; url: string; format: 'gltf' | 'stl' } => x !== null);
  const cols = Math.max(1, Math.ceil(Math.sqrt(withModels.length)));
  return withModels.map(({ el, url, format }, i) => ({
    id: el.id,
    label: el.label || el.id.split(/[/#]/).pop() || el.id,
    url,
    format,
    slot: [(i % cols) * spacing, Math.floor(i / cols) * spacing],
  }));
}
