// Basemap styles for the MapLibre dataset viewer.
//
// Streets, light theme — OpenFreeMap's hosted "Liberty" style (colourful,
// detailed, no API key). Streets, dark theme — a compact custom style over the
// same OpenFreeMap vector tiles: a midnight-blue base that keeps *colour*
// (teal water, green parks, amber motorways, warm road hierarchy) instead of
// the usual grayscale dark map. Satellite — Esri World Imagery (+ reference
// labels at low zoom), available in both themes. The vector styles get an OSM
// 3D-building fill-extrusion layer so models stand in a real cityscape.

import type { StyleSpecification, LayerSpecification, Map as MlMap } from 'maplibre-gl';

const OFM_TILES = 'https://tiles.openfreemap.org/planet';
const OFM_GLYPHS = 'https://tiles.openfreemap.org/fonts/{fontstack}/{range}.pbf';
const OSM_ATTRIBUTION = '© <a href="https://www.openstreetmap.org/copyright">OpenStreetMap</a> · <a href="https://openfreemap.org">OpenFreeMap</a>';
const ESRI_ATTRIBUTION = 'Tiles © Esri — Source: Esri, Maxar, Earthstar Geographics, and the GIS User Community';

export type BasemapKind = 'streets' | 'satellite';

/** Hosted colourful light style (OpenFreeMap Liberty). */
export const LIGHT_STYLE_URL = 'https://tiles.openfreemap.org/styles/liberty';

const FONT = ['Noto Sans Regular'];
const FONT_BOLD = ['Noto Sans Bold'];

/**
 * Custom dark streets style: midnight-blue ground with coloured water, parks,
 * road hierarchy and labels over the OpenMapTiles schema.
 */
export function darkStyle(): StyleSpecification {
  const layers: LayerSpecification[] = [
    { id: 'background', type: 'background', paint: { 'background-color': '#0b1118' } },
    {
      id: 'landcover',
      type: 'fill',
      source: 'openmaptiles',
      'source-layer': 'landcover',
      paint: {
        'fill-color': [
          'match', ['get', 'class'],
          'wood', '#13231b',
          'grass', '#13251d',
          'wetland', '#112932',
          'sand', '#2b2a20',
          'ice', '#16242e',
          '#101a22',
        ],
        'fill-opacity': 0.85,
      },
    },
    {
      id: 'landuse',
      type: 'fill',
      source: 'openmaptiles',
      'source-layer': 'landuse',
      filter: ['match', ['get', 'class'], ['residential', 'suburbs', 'neighbourhood'], true, false],
      paint: { 'fill-color': '#111923', 'fill-opacity': 0.7 },
    },
    {
      id: 'park',
      type: 'fill',
      source: 'openmaptiles',
      'source-layer': 'park',
      paint: { 'fill-color': '#142a1f', 'fill-opacity': 0.9 },
    },
    {
      id: 'water',
      type: 'fill',
      source: 'openmaptiles',
      'source-layer': 'water',
      paint: { 'fill-color': '#16374f' },
    },
    {
      id: 'waterway',
      type: 'line',
      source: 'openmaptiles',
      'source-layer': 'waterway',
      paint: { 'line-color': '#1d4259', 'line-width': ['interpolate', ['linear'], ['zoom'], 8, 0.6, 16, 3] },
    },
    {
      id: 'aeroway',
      type: 'line',
      source: 'openmaptiles',
      'source-layer': 'aeroway',
      minzoom: 10,
      paint: { 'line-color': '#23303d', 'line-width': ['interpolate', ['linear'], ['zoom'], 10, 1, 16, 14] },
    },
    {
      id: 'tunnel',
      type: 'line',
      source: 'openmaptiles',
      'source-layer': 'transportation',
      filter: ['==', ['get', 'brunnel'], 'tunnel'],
      paint: { 'line-color': '#1a2430', 'line-width': ['interpolate', ['exponential', 1.6], ['zoom'], 6, 0.4, 18, 14], 'line-dasharray': [2, 2] },
    },
    {
      id: 'road-minor',
      type: 'line',
      source: 'openmaptiles',
      'source-layer': 'transportation',
      filter: ['all', ['!=', ['get', 'brunnel'], 'tunnel'],
        ['match', ['get', 'class'], ['minor', 'service', 'track', 'path', 'pedestrian'], true, false]],
      minzoom: 12,
      paint: {
        'line-color': '#27313d',
        'line-width': ['interpolate', ['exponential', 1.6], ['zoom'], 12, 0.5, 18, 9],
      },
    },
    {
      id: 'road-secondary',
      type: 'line',
      source: 'openmaptiles',
      'source-layer': 'transportation',
      filter: ['all', ['!=', ['get', 'brunnel'], 'tunnel'],
        ['match', ['get', 'class'], ['secondary', 'tertiary'], true, false]],
      minzoom: 9,
      paint: {
        'line-color': '#33445a',
        'line-width': ['interpolate', ['exponential', 1.6], ['zoom'], 9, 0.7, 18, 14],
      },
    },
    {
      id: 'road-primary',
      type: 'line',
      source: 'openmaptiles',
      'source-layer': 'transportation',
      filter: ['all', ['!=', ['get', 'brunnel'], 'tunnel'],
        ['match', ['get', 'class'], ['primary', 'trunk'], true, false]],
      paint: {
        'line-color': '#48597a',
        'line-width': ['interpolate', ['exponential', 1.6], ['zoom'], 6, 0.8, 18, 18],
      },
    },
    {
      id: 'road-motorway',
      type: 'line',
      source: 'openmaptiles',
      'source-layer': 'transportation',
      filter: ['all', ['!=', ['get', 'brunnel'], 'tunnel'], ['==', ['get', 'class'], 'motorway']],
      paint: {
        'line-color': '#7a6334',
        'line-width': ['interpolate', ['exponential', 1.6], ['zoom'], 5, 1, 18, 20],
      },
    },
    {
      id: 'rail',
      type: 'line',
      source: 'openmaptiles',
      'source-layer': 'transportation',
      filter: ['==', ['get', 'class'], 'rail'],
      minzoom: 10,
      paint: { 'line-color': '#2c3543', 'line-width': 1.2, 'line-dasharray': [3, 3] },
    },
    {
      id: 'building',
      type: 'fill',
      source: 'openmaptiles',
      'source-layer': 'building',
      minzoom: 13,
      maxzoom: 15.5, // extrusions take over (see add3dBuildings)
      paint: { 'fill-color': '#1c2735', 'fill-outline-color': '#0d141c' },
    },
    {
      id: 'boundary',
      type: 'line',
      source: 'openmaptiles',
      'source-layer': 'boundary',
      filter: ['<=', ['get', 'admin_level'], 4],
      paint: { 'line-color': '#3c4c61', 'line-width': 1, 'line-dasharray': [4, 3] },
    },
    {
      id: 'water-name',
      type: 'symbol',
      source: 'openmaptiles',
      'source-layer': 'water_name',
      layout: { 'text-field': ['get', 'name'], 'text-font': FONT, 'text-size': 12, 'text-letter-spacing': 0.15 },
      paint: { 'text-color': '#5f93b4', 'text-halo-color': '#0b1118', 'text-halo-width': 1.2 },
    },
    {
      id: 'road-name',
      type: 'symbol',
      source: 'openmaptiles',
      'source-layer': 'transportation_name',
      minzoom: 13,
      layout: {
        'text-field': ['get', 'name'],
        'text-font': FONT,
        'text-size': 11,
        'symbol-placement': 'line',
      },
      paint: { 'text-color': '#7f93a8', 'text-halo-color': '#0b1118', 'text-halo-width': 1.1 },
    },
    {
      id: 'place-village',
      type: 'symbol',
      source: 'openmaptiles',
      'source-layer': 'place',
      filter: ['match', ['get', 'class'], ['village', 'suburb', 'hamlet', 'neighbourhood'], true, false],
      layout: { 'text-field': ['get', 'name'], 'text-font': FONT, 'text-size': 11.5 },
      paint: { 'text-color': '#93a7bb', 'text-halo-color': '#0b1118', 'text-halo-width': 1.3 },
    },
    {
      id: 'place-town',
      type: 'symbol',
      source: 'openmaptiles',
      'source-layer': 'place',
      filter: ['match', ['get', 'class'], ['city', 'town'], true, false],
      layout: { 'text-field': ['get', 'name'], 'text-font': FONT_BOLD, 'text-size': ['match', ['get', 'class'], 'city', 14.5, 12.5] },
      paint: { 'text-color': '#d6e2ef', 'text-halo-color': '#0b1118', 'text-halo-width': 1.4 },
    },
    {
      id: 'place-country',
      type: 'symbol',
      source: 'openmaptiles',
      'source-layer': 'place',
      filter: ['==', ['get', 'class'], 'country'],
      maxzoom: 7,
      layout: { 'text-field': ['get', 'name'], 'text-font': FONT_BOLD, 'text-size': 13, 'text-letter-spacing': 0.1, 'text-transform': 'uppercase' },
      paint: { 'text-color': '#8ba1b8', 'text-halo-color': '#0b1118', 'text-halo-width': 1.4 },
    },
  ];
  return {
    version: 8,
    glyphs: OFM_GLYPHS,
    sources: {
      openmaptiles: { type: 'vector', url: OFM_TILES, attribution: OSM_ATTRIBUTION },
    },
    layers,
  };
}

/** Esri World Imagery, with reference labels up to mid zooms. Theme-agnostic. */
export function satelliteStyle(): StyleSpecification {
  return {
    version: 8,
    glyphs: OFM_GLYPHS, // the viewer's own symbol layers need glyphs
    sources: {
      'esri-imagery': {
        type: 'raster',
        tiles: ['https://server.arcgisonline.com/ArcGIS/rest/services/World_Imagery/MapServer/tile/{z}/{y}/{x}'],
        tileSize: 256,
        maxzoom: 19,
        attribution: ESRI_ATTRIBUTION,
      },
      'esri-reference': {
        type: 'raster',
        tiles: ['https://server.arcgisonline.com/ArcGIS/rest/services/Reference/World_Boundaries_and_Places/MapServer/tile/{z}/{y}/{x}'],
        tileSize: 256,
        maxzoom: 12,
      },
    },
    layers: [
      { id: 'imagery', type: 'raster', source: 'esri-imagery' },
      { id: 'reference', type: 'raster', source: 'esri-reference', maxzoom: 14, paint: { 'raster-opacity': 0.9 } },
    ],
  };
}

/** The style for a basemap/theme combination. */
export function styleFor(kind: BasemapKind, dark: boolean): StyleSpecification | string {
  if (kind === 'satellite') return satelliteStyle();
  return dark ? darkStyle() : LIGHT_STYLE_URL;
}

export const BUILDINGS_LAYER_ID = 'ots-3d-buildings';

/** The OpenMapTiles vector layer every basemap draws its buildings from. */
export const OSM_BUILDING_SOURCE_LAYER = 'building';

/**
 * Every layer in the active style that draws OSM buildings — flat fills as well
 * as extrusions, whoever added them.
 *
 * Enumerating by `source-layer` rather than by a hardcoded id matters twice
 * over: the hosted Liberty style draws buildings from layers we do not own
 * (`building`, `building-3d`), and it is free to rename or add them at any time.
 * Anything that hides or filters "the basemap buildings" has to cover all of
 * them, or a grey block keeps standing through a model.
 */
export function buildingLayerIds(map: MlMap): string[] {
  const layers = map.getStyle()?.layers ?? [];
  return layers
    .filter(
      (l) =>
        (l as { 'source-layer'?: string })['source-layer'] === OSM_BUILDING_SOURCE_LAYER &&
        (l.type === 'fill' || l.type === 'fill-extrusion'),
    )
    .map((l) => l.id);
}

/**
 * Make sure the style has an OSM fill-extrusion building layer, and return its
 * id (null on raster styles, which have no vector buildings at all).
 *
 * A style that already ships one is ADOPTED rather than duplicated: Liberty's
 * own `building-3d` and a second `ots-3d-buildings` over the same geometry
 * z-fought with each other, and the layer toggle only ever reached ours. Our
 * layer is added only when the style has none — the custom dark style, which
 * deliberately stops its flat `building` fill where the extrusions take over.
 */
export function add3dBuildings(map: MlMap, dark: boolean): string | null {
  const style = map.getStyle();
  if (!style) return null;
  const layers = style.layers ?? [];
  const existing = layers.find(
    (l) =>
      l.type === 'fill-extrusion' &&
      (l as { 'source-layer'?: string })['source-layer'] === OSM_BUILDING_SOURCE_LAYER,
  );
  if (existing) return existing.id;
  const sourceId = Object.keys(style.sources ?? {}).find((id) => style.sources[id].type === 'vector');
  if (!sourceId) return null;
  // Below the first symbol layer so labels stay readable.
  const firstSymbol = layers.find((l) => l.type === 'symbol')?.id;
  map.addLayer(
    {
      id: BUILDINGS_LAYER_ID,
      type: 'fill-extrusion',
      source: sourceId,
      'source-layer': OSM_BUILDING_SOURCE_LAYER,
      minzoom: 14.5,
      paint: {
        'fill-extrusion-color': dark ? '#243245' : '#d8d3c8',
        'fill-extrusion-height': ['coalesce', ['get', 'render_height'], 8],
        'fill-extrusion-base': ['coalesce', ['get', 'render_min_height'], 0],
        'fill-extrusion-opacity': dark ? 0.82 : 0.75,
      },
    },
    firstSymbol
  );
  return BUILDINGS_LAYER_ID;
}

/** Raster tile sources for the lightweight Leaflet previews (GeoPreview),
 *  themed to match the MapLibre styles above. */
export function leafletTiles(dark: boolean): { url: string; attribution: string } {
  return dark
    ? {
        url: 'https://{s}.basemaps.cartocdn.com/dark_all/{z}/{x}/{y}{r}.png',
        attribution: '&copy; OpenStreetMap &copy; CARTO',
      }
    : {
        // Carto Voyager, not tile.openstreetmap.org — the OSM tile policy 403s
        // app/localhost traffic, which broke the light preview basemap.
        url: 'https://{s}.basemaps.cartocdn.com/rastertiles/voyager/{z}/{x}/{y}{r}.png',
        attribution: '&copy; OpenStreetMap &copy; CARTO',
      };
}
