// Pure helpers for the 3D & map viewer: convert viewer-feed elements (WGS84 WKT
// from /api/datasets/:id/viewer-feed) into Leaflet-ready latlngs and lay 3D
// models out on a simple grid. Kept free of Leaflet/three imports so it is unit
// testable in jsdom.

import { parseWktGeometry, type WktGeometry } from '../ontology/valueType';
import { modelRefOf, type ModelFormat, type ModelRef as DetectedRef } from './detect';

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
  /** Source up-axis of the element's 3D model(s), from `ots:modelUpAxis`. */
  up_axis?: string | null;
}

export type LatLng = [number, number];

export interface MapFeature {
  id: string;
  label: string;
  kind: 'point' | 'line' | 'polygon' | 'geometrycollection';
  /** Leaflet order: [lat, lng]. For polygons: outer ring only. */
  latlngs: LatLng[];
}

/** WKT (lon lat) coordinate pairs → Leaflet [lat, lng]. */
const toLatLng = (c: [number, number]): LatLng => [c[1], c[0]];

/** The representative [lat, lng] list of a geometry (collections flattened). */
function geometryLatLngs(g: WktGeometry): LatLng[] {
  switch (g.kind) {
    case 'point':
      return [toLatLng(g.coord)];
    case 'multipoint':
    case 'linestring':
      return g.coords.map(toLatLng);
    case 'multilinestring':
      return g.lines.flat().map(toLatLng);
    case 'polygon':
      return g.rings[0]?.map(toLatLng) ?? [];
    case 'multipolygon':
      return g.polygons[0]?.[0]?.map(toLatLng) ?? [];
    case 'geometrycollection':
      return g.geometries.flatMap(geometryLatLngs);
    default:
      return [];
  }
}

const FEATURE_KIND: Partial<Record<WktGeometry['kind'], MapFeature['kind']>> = {
  point: 'point',
  multipoint: 'point',
  linestring: 'line',
  multilinestring: 'line',
  polygon: 'polygon',
  multipolygon: 'polygon',
  geometrycollection: 'geometrycollection',
};

/** Flatten a parsed WKT geometry into one drawable map feature per element. */
export function toMapFeature(el: ViewerElement): MapFeature | null {
  if (!el.wkt4326) return null;
  const g: WktGeometry | null = parseWktGeometry(el.wkt4326);
  if (!g) return null;
  const kind = FEATURE_KIND[g.kind];
  if (!kind) return null;
  const label = el.label || el.id.split(/[/#]/).pop() || el.id;
  return { id: el.id, label, kind, latlngs: geometryLatLngs(g) };
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
      // Defensive: a mis-projected geometry (e.g. metres mistaken for degrees)
      // must never produce bounds that make MapLibre's fitBounds throw.
      if (lat < -90 || lat > 90 || lng < -180 || lng > 180) continue;
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
  format: ModelFormat;
  /** Source up-axis from ots:modelUpAxis ('Z' rotates into the Y-up scene). */
  upAxis?: string | null;
  /** Grid slot position in the 3D scene, [x, z]. */
  slot: [number, number];
}

/**
 * Elements with a loadable 3D model (glTF > CityJSON > CityGML > STL, resolved
 * from the FOG file list), laid out on a √n×√n ground grid with `spacing`
 * between slots.
 */
export function modelRefs(elements: ViewerElement[], spacing = 3): ModelRef[] {
  const withModels = elements
    .map((el) => {
      const ref = modelRefOf(el);
      return ref ? { el, ...ref } : null;
    })
    .filter((x): x is { el: ViewerElement } & DetectedRef => x !== null);
  const cols = Math.max(1, Math.ceil(Math.sqrt(withModels.length)));
  return withModels.map(({ el, url, format, upAxis }, i) => ({
    id: el.id,
    label: el.label || el.id.split(/[/#]/).pop() || el.id,
    url,
    format,
    upAxis,
    slot: [(i % cols) * spacing, Math.floor(i / cols) * spacing],
  }));
}

// ── GeoJSON for the MapLibre map ─────────────────────────────────────────────

export interface FeatureProps {
  id: string;
  label: string;
  hasModel: boolean;
}

type Geo =
  | { type: 'Point'; coordinates: [number, number] }
  | { type: 'MultiPoint'; coordinates: [number, number][] }
  | { type: 'LineString'; coordinates: [number, number][] }
  | { type: 'MultiLineString'; coordinates: [number, number][][] }
  | { type: 'Polygon'; coordinates: [number, number][][] }
  | { type: 'MultiPolygon'; coordinates: [number, number][][][] };

export interface GeoFeature {
  type: 'Feature';
  geometry: Geo;
  properties: FeatureProps;
}

export interface ViewerGeoJSON {
  points: GeoFeature[];
  lines: GeoFeature[];
  polygons: GeoFeature[];
}

function wktToGeo(g: WktGeometry): Geo | null {
  switch (g.kind) {
    case 'point':
      return { type: 'Point', coordinates: g.coord };
    case 'multipoint':
      return { type: 'MultiPoint', coordinates: g.coords };
    case 'linestring':
      return { type: 'LineString', coordinates: g.coords };
    case 'multilinestring':
      return { type: 'MultiLineString', coordinates: g.lines };
    case 'polygon':
      return { type: 'Polygon', coordinates: g.rings };
    case 'multipolygon':
      return { type: 'MultiPolygon', coordinates: g.polygons };
    default:
      return null; // collections are flattened by the caller
  }
}

/**
 * Split the located elements into point / line / polygon GeoJSON features
 * (WKT is already lon/lat, the GeoJSON axis order). GeometryCollections
 * contribute one feature per member.
 */
export function elementsToGeoJSON(elements: ViewerElement[]): ViewerGeoJSON {
  const out: ViewerGeoJSON = { points: [], lines: [], polygons: [] };
  for (const el of elements) {
    if (!el.wkt4326) continue;
    const parsed = parseWktGeometry(el.wkt4326);
    if (!parsed) continue;
    const props: FeatureProps = {
      id: el.id,
      label: el.label || el.id.split(/[/#]/).pop() || el.id,
      hasModel: modelRefOf(el) !== null,
    };
    const geoms = parsed.kind === 'geometrycollection' ? parsed.geometries : [parsed];
    for (const g of geoms) {
      const geometry = wktToGeo(g);
      if (!geometry) continue;
      const feature: GeoFeature = { type: 'Feature', geometry, properties: props };
      if (geometry.type === 'Point' || geometry.type === 'MultiPoint') out.points.push(feature);
      else if (geometry.type === 'LineString' || geometry.type === 'MultiLineString') out.lines.push(feature);
      else out.polygons.push(feature);
    }
  }
  return out;
}

/** The [lng, lat] anchor a point element's 3D model should stand at. */
export function modelAnchor(el: ViewerElement): [number, number] | null {
  if (!el.wkt4326) return null;
  const g = parseWktGeometry(el.wkt4326);
  if (!g) return null;
  if (g.kind === 'point') return g.coord;
  if (g.kind === 'multipoint') return g.coords[0] ?? null;
  // Lines/polygons: centroid of their bounding box.
  const f = toMapFeature(el);
  if (!f || !f.latlngs.length) return null;
  const b = featureBounds([f]);
  if (!b) return null;
  return [(b[0][1] + b[1][1]) / 2, (b[0][0] + b[1][0]) / 2];
}
