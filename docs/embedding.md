# Embedding & Using the Data in Your Own Web Apps

Everything the built-in explorer shows — maps, 3D models, tables, files — is
backed by plain HTTP APIs, so you can reuse it from any web page in two ways:

1. **Iframe embeds** — drop a ready-made interactive viewer into your page with
   one `<iframe>` tag. Zero JavaScript required.
2. **Direct APIs** — fetch SPARQL results, GeoJSON, 3D Tiles, RDF or the raw
   model files and render them with your own libraries (Chart.js, Leaflet,
   MapLibre, CesiumJS, three.js, …).

Anonymous requests see exactly what a logged-out visitor sees: **public
datasets work without any authentication**. For private data, see
[Authentication](#authentication-for-private-data).

---

## 1. Iframe embeds

The app serves chrome-less, embed-optimised viewer pages under `/embed/…`.
They carry a `Content-Security-Policy: frame-ancestors *` header (configurable,
see below), so any site may iframe them — the rest of the app refuses framing.

| URL | Shows |
| --- | --- |
| `/embed/map/<dataset-id>` | Interactive map (MapLibre) with vector features **and** to-scale 3D models (glTF / IFC / CityJSON / STL) standing on it |
| `/embed/3d/<dataset-id>` | Orbitable 3D scene of the dataset's models (no basemap) |
| `/embed/cesium/<dataset-id>` | CesiumJS globe streaming the dataset's 3D Tiles |
| `/embed/model?src=<file-url>` | A single 3D model file by URL |

```html
<iframe
  src="https://your-triplestore.example/embed/map/viewer-3d-demo"
  width="100%" height="480"
  style="border:0;border-radius:12px"
  loading="lazy" allowfullscreen></iframe>
```

The dataset explorer has an **Embed** button (next to Download) that produces
this snippet for the view you are looking at.

### Query parameters

| Param | Applies to | Meaning |
| --- | --- | --- |
| `element=<IRI>` | map, 3d, cesium | Pre-select (and fly to) one element |
| `basemap=streets\|satellite` | map | Initial basemap |
| `theme=light\|dark` | all | Force a theme (defaults to the visitor's OS preference) |
| `src=<url>` | model | The model file to load (glTF/GLB, IFC, CityJSON, CityGML, STL) |
| `format=gltf\|ifc\|cityjson\|citygml\|stl` | model | Required when `src` has no file extension (e.g. an asset `/download` URL) |
| `up=Z` | model | Source file is Z-up (rotates into the viewer's Y-up scene) |

Example — a dark satellite map focused on one building:

```
/embed/map/viewer-3d-demo?basemap=satellite&theme=dark&element=https%3A%2F%2Fexample.org%2Fbuilding%2F42
```

### Reacting to clicks (postMessage)

When the visitor picks an element inside the embed, the iframe posts a message
to your page — so an embedding webapp can drive its own UI from map/model
clicks:

```js
window.addEventListener('message', (e) => {
  const m = e.data;
  if (m?.source !== 'open-triplestore' || m.type !== 'select') return;
  console.log('picked element', m.id, 'IFC GlobalId', m.guid, 'in dataset', m.dataset);
  // e.g. fetch its RDF: /api/datasets/<dataset>/viewer-feed, or SPARQL DESCRIBE <m.id>
});
```

The payload is `{ source: 'open-triplestore', type: 'select', dataset, id, guid }`
where `id` is the element's linked-data IRI and `guid` its IFC GlobalId (when
the pick hit an IFC sub-element such as a wall or beam).

### Restricting who may embed

By default `/embed/*` allows any origin. Operators can restrict or disable this
with the `EMBED_FRAME_ANCESTORS` environment variable on the server:

```bash
EMBED_FRAME_ANCESTORS="https://intranet.example https://*.example.org"  # allowlist
EMBED_FRAME_ANCESTORS="'none'"                                          # disable embedding
```

The value is used verbatim as the CSP `frame-ancestors` source list.

---

## 2. Direct APIs

All endpoints below are anonymous-capable for public datasets. Replace
`{BASE}` with your server origin and `{dataset}` with the dataset id (slug).

### SPARQL → charts and tables

`POST {BASE}/sparql` with `Accept: application/sparql-results+json` returns
standard SPARQL JSON — feed it to any charting/table library:

```html
<canvas id="chart"></canvas>
<script type="module">
  import Chart from 'https://cdn.jsdelivr.net/npm/chart.js@4/auto/+esm';
  const q = `SELECT ?type (COUNT(?s) AS ?n) WHERE { ?s a ?type } GROUP BY ?type ORDER BY DESC(?n) LIMIT 8`;
  const res = await fetch('{BASE}/sparql', {
    method: 'POST',
    headers: { 'Content-Type': 'application/sparql-query', Accept: 'application/sparql-results+json' },
    body: q,
  });
  const json = await res.json();
  const rows = json.results.bindings;
  new Chart(chart, {
    type: 'bar',
    data: {
      labels: rows.map((r) => r.type.value.split(/[/#]/).pop()),
      datasets: [{ label: 'instances', data: rows.map((r) => Number(r.n.value)) }],
    },
  });
</script>
```

Per-dataset scoped endpoint: `POST {BASE}/api/datasets/{dataset}/sparql`.
Saved queries can also be published as parameterised GET APIs — see *API
Services* in these docs.

### GeoJSON (OGC API – Features) → Leaflet / MapLibre

Every dataset with geometry is an OGC API – Features collection:

```
GET {BASE}/api/ogc/collections                       # list collections
GET {BASE}/api/ogc/collections/{dataset}/items       # GeoJSON FeatureCollection
GET {BASE}/api/ogc/collections/{dataset}/items?limit=1000&bbox=4.8,52.3,5.0,52.4
```

```html
<div id="map" style="height:420px"></div>
<script type="module">
  import L from 'https://cdn.jsdelivr.net/npm/leaflet@1.9/+esm';
  const map = L.map('map').setView([52.37, 4.9], 13);
  L.tileLayer('https://tile.openstreetmap.org/{z}/{x}/{y}.png', { attribution: '© OpenStreetMap' }).addTo(map);
  const fc = await (await fetch('{BASE}/api/ogc/collections/{dataset}/items?limit=1000')).json();
  L.geoJSON(fc, { onEachFeature: (f, l) => l.bindPopup(f.properties?.label ?? f.id) }).addTo(map);
</script>
```

### 3D Tiles → CesiumJS

Datasets with volumetric geometry publish an OGC 3D Tiles 1.1 tileset:

```
GET {BASE}/api/datasets/{dataset}/3dtiles/tileset.json
```

```js
const viewer = new Cesium.Viewer('cesiumContainer');
const tileset = await Cesium.Cesium3DTileset.fromUrl(
  '{BASE}/api/datasets/{dataset}/3dtiles/tileset.json'
);
viewer.scene.primitives.add(tileset);
await viewer.zoomTo(tileset);
// Each feature carries an `iri` property linking back to its RDF subject.
```

### Viewer feed → everything the built-in explorer knows

One JSON call returns each element's IRI, label, types, BOT parent, WGS84 WKT
geometry, IFC GlobalId and 3D file references — ideal for custom viewers:

```
GET {BASE}/api/datasets/{dataset}/viewer-feed             # full (structure tree included)
GET {BASE}/api/datasets/{dataset}/viewer-feed?located=true # only coordinate-bearing elements (fast)
GET {BASE}/api/datasets/{dataset}/geo-stats               # capability probe (has_coordinates/has_3d/…)
```

### RDF downloads (Graph Store)

```
GET {BASE}/store?graph=<graph-iri>&format=turtle    # also: jsonld, rdfxml, ntriples
```

Content negotiation via the `Accept` header works too. Individual resources
dereference at `{BASE}/resource?iri=<IRI>` (and asset IRIs content-negotiate
between RDF metadata and the file bytes).

### Files (3D models, point clouds, documents)

Uploaded files are served at:

```
GET {BASE}/api/datasets/{dataset}/assets                       # list (JSON)
GET {BASE}/api/datasets/{dataset}/assets/{asset-id}/download   # bytes, anonymous-capable
```

Asset bytes are immutable per id and served with
`Cache-Control: immutable` + `ETag`, so browsers cache a 50 MB IFC once.
Load them straight into three.js, `<model-viewer>`, IFC.js, etc.:

```html
<script type="module">
  import 'https://cdn.jsdelivr.net/npm/@google/model-viewer@4/+esm';
</script>
<model-viewer
  src="{BASE}/api/datasets/{dataset}/assets/{asset-id}/download"
  camera-controls auto-rotate style="width:100%;height:420px"></model-viewer>
```

(For a zero-code version of this, use `/embed/model?src=…` above.)

---

## CORS

Iframe embeds need **no** CORS setup. Direct `fetch()` calls from another
origin do: set the server's `CORS_ORIGINS` environment variable to the
origins of your webapps (comma-separated), or `*` to reflect any origin:

```bash
CORS_ORIGINS="https://myapp.example,https://dashboard.example"
```

## Authentication for private data

The embeds and examples above run anonymously and therefore only see public
datasets. For private data from your own backend or scripts, send an API
token: `Authorization: Bearer <token>` (create one under Settings → API
tokens). Do **not** put tokens in iframe URLs or client-side code you ship to
third parties.
