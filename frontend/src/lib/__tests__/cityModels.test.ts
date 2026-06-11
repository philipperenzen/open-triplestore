import { describe, it, expect } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import type { Mesh, MeshStandardMaterial } from 'three';
import { epsgFromReference, toLonLat } from '../viewer/crs';
import { parseCityJSON, parseCityGML } from '../viewer/cityjson';
import { modelFormatFromUrl, modelRefOf, modelRefsOf, isGeometryPredicate, isIfcGuidPredicate } from '../viewer/detect';
import { elementsToGeoJSON, modelAnchor } from '../viewer/geometry';

// Synthetic two-building sample — kept as a compact parser fixture.
const SAMPLE_PATH = resolve(process.cwd(), 'public/samples/nijmegen-buildings.city.json');
const sample = JSON.parse(readFileSync(SAMPLE_PATH, 'utf8'));
// The REAL bundled demo excerpt (3DBAG LoD2.2 around the Schependomlaan site,
// referenced by the seeded viewer-3d-demo context graph) — parsing it here
// keeps the shipped file valid.
const BAG_PATH = resolve(process.cwd(), 'public/samples/schependomlaan-3dbag.city.json');
const bagSample = JSON.parse(readFileSync(BAG_PATH, 'utf8'));

describe('crs', () => {
  it('extracts EPSG codes from the common reference spellings', () => {
    expect(epsgFromReference('EPSG:28992')).toBe(28992);
    expect(epsgFromReference('urn:ogc:def:crs:EPSG::7415')).toBe(7415);
    expect(epsgFromReference('https://www.opengis.net/def/crs/EPSG/0/28992')).toBe(28992);
    expect(epsgFromReference('not a crs')).toBeNull();
  });

  it('reprojects RD New (and its compound alias 7415) to WGS84', () => {
    for (const code of [28992, 7415]) {
      const conv = toLonLat(code)!;
      const [lon, lat] = conv([187550, 428250]);
      expect(lon).toBeCloseTo(5.85953, 3);
      expect(lat).toBeCloseTo(51.84188, 3);
    }
  });

  it('passes 4326 through and rejects unknown codes', () => {
    expect(toLonLat(4326)!([5.5, 52.0])).toEqual([5.5, 52.0]);
    expect(toLonLat(999999)).toBeNull();
  });
});

describe('modelRefsOf', () => {
  it('lists every linked representation ordered by preference', () => {
    const el = {
      gltf_url: 'https://x/model.glb',
      ifc_url: 'https://x/model.ifc#0TestBeamAAAAAAAAAAAA5',
      files: [['Stl', 'https://x/model.stl']] as [string, string][],
    };
    const refs = modelRefsOf(el);
    expect(refs.map((r) => r.format)).toEqual(['gltf', 'stl', 'ifc']);
    expect(modelRefOf(el)!.format).toBe('gltf');
    // The IFC ref keeps its element-isolating fragment.
    expect(refs.find((r) => r.format === 'ifc')!.url).toContain('#0TestBeam');
  });

  it('falls back to the dedicated ifc_url when FOG lists none', () => {
    const refs = modelRefsOf({ ifc_url: 'https://x/m.ifc' });
    expect(refs).toEqual([{ url: 'https://x/m.ifc', format: 'ifc' }]);
  });
});

describe('parseCityJSON (3DBAG excerpt)', () => {
  it('parses the bundled Schependomlaan 3DBAG block with real geometry near the site', () => {
    const model = parseCityJSON(bagSample);
    // Semantics are stripped from the excerpt, so all 157 buildings merge into
    // one mesh bucket — assert on the actual triangulated geometry instead.
    const mesh = model.group.children[0] as Mesh;
    expect(mesh.geometry.getAttribute('position').count).toBeGreaterThan(3000);
    const [lon, lat] = model.anchorLonLat!;
    expect(lon).toBeCloseTo(5.834, 1);
    expect(lat).toBeCloseTo(51.841, 1);
  });
});

describe('parseCityJSON', () => {
  const model = parseCityJSON(sample);

  it('georeferences the model from its EPSG:7415 metadata', () => {
    expect(model.anchorLonLat).not.toBeNull();
    expect(model.anchorLonLat![0]).toBeCloseTo(5.85953, 3);
    expect(model.anchorLonLat![1]).toBeCloseTo(51.84188, 3);
  });

  it('builds metre-scaled geometry for both buildings', () => {
    expect(model.objectCount).toBe(2);
    expect(model.triangleCount).toBeGreaterThanOrEqual(15); // 9 + 6 surfaces, quads → 2 tris
    expect(model.sizeMeters.x).toBeCloseTo(30, 0); // block spans 30 m east-west
    expect(model.sizeMeters.y).toBeCloseTo(9, 0); // ridge height
    expect(model.sizeMeters.z).toBeCloseTo(8, 0); // deepest footprint
  });

  it('colours by semantic surface (ground / wall / roof buckets)', () => {
    const colors = model.group.children.map((m) =>
      ((m as Mesh).material as MeshStandardMaterial).color.getHexString()
    );
    expect(colors).toHaveLength(3);
    expect(colors).toContain('b0563c'); // RoofSurface terracotta
  });

  it('rejects non-CityJSON documents', () => {
    expect(() => parseCityJSON({ type: 'GeoJSON' })).toThrow(/not a CityJSON/i);
  });
});

describe('parseCityGML', () => {
  const GML = `<?xml version="1.0" encoding="UTF-8"?>
<core:CityModel xmlns:core="http://www.opengis.net/citygml/2.0"
    xmlns:bldg="http://www.opengis.net/citygml/building/2.0"
    xmlns:gml="http://www.opengis.net/gml">
  <gml:boundedBy><gml:Envelope srsName="EPSG:28992" srsDimension="3"/></gml:boundedBy>
  <core:cityObjectMember>
    <bldg:Building>
      <bldg:lod2MultiSurface><gml:MultiSurface>
        <gml:surfaceMember>
          <bldg:RoofSurface><bldg:lod2MultiSurface><gml:MultiSurface><gml:surfaceMember>
            <gml:Polygon><gml:exterior><gml:LinearRing>
              <gml:posList srsDimension="3">187360 428400 6 187370 428400 6 187365 428404 9 187360 428400 6</gml:posList>
            </gml:LinearRing></gml:exterior></gml:Polygon>
          </gml:surfaceMember></gml:MultiSurface></bldg:lod2MultiSurface></bldg:RoofSurface>
        </gml:surfaceMember>
        <gml:surfaceMember>
          <gml:Polygon><gml:exterior><gml:LinearRing>
            <gml:posList srsDimension="3">187360 428400 0 187370 428400 0 187370 428400 6 187360 428400 6 187360 428400 0</gml:posList>
          </gml:LinearRing></gml:exterior></gml:Polygon>
        </gml:surfaceMember>
      </gml:MultiSurface></bldg:lod2MultiSurface>
    </bldg:Building>
  </core:cityObjectMember>
</core:CityModel>`;

  it('collects polygons, semantics and the CRS from a CityGML document', () => {
    const model = parseCityGML(GML);
    expect(model.anchorLonLat).not.toBeNull();
    expect(model.anchorLonLat![0]).toBeCloseTo(5.857, 2);
    expect(model.triangleCount).toBeGreaterThan(0);
    const colors = model.group.children.map((m) =>
      ((m as Mesh).material as MeshStandardMaterial).color.getHexString()
    );
    expect(colors).toContain('b0563c'); // the RoofSurface kept its semantic colour
  });

  it('rejects documents without polygons', () => {
    expect(() => parseCityGML('<a/>')).toThrow(/no gml:Polygon/i);
  });
});

describe('model format detection', () => {
  it('detects city-model formats from URLs, including site-relative ones', () => {
    expect(modelFormatFromUrl('https://x.test/a.city.json')).toBe('cityjson');
    expect(modelFormatFromUrl('https://x.test/a.cityjson')).toBe('cityjson');
    expect(modelFormatFromUrl('/samples/nijmegen-buildings.city.json')).toBe('cityjson');
    expect(modelFormatFromUrl('https://x.test/lod2.gml')).toBe('citygml');
    expect(modelFormatFromUrl('relative.gml')).toBeNull();
  });

  it('resolves the best ref by FOG key with the documented priority', () => {
    const el = {
      files: [
        ['Stl', 'https://x.test/m.stl'],
        ['Cityjson_v2.0', 'https://x.test/m'], // extensionless: the key decides
      ] as [string, string][],
    };
    expect(modelRefOf(el)).toEqual({ url: 'https://x.test/m', format: 'cityjson' });
    expect(modelRefOf({ gltf_url: 'https://x.test/m.glb', ...el })?.format).toBe('gltf');
  });
});

describe('geo/BIM predicate matching (mirrors src/geo/viewer_feed.rs)', () => {
  it('matches exactly geo:hasGeometry and omg:hasGeometry', () => {
    expect(isGeometryPredicate('http://www.opengis.net/ont/geosparql#hasGeometry')).toBe(true);
    expect(isGeometryPredicate('https://w3id.org/omg#hasGeometry')).toBe(true);
    // Not just any *hasGeometry — only the two predicates the server feed follows.
    expect(isGeometryPredicate('http://example.org/vocab#hasGeometry')).toBe(false);
    expect(isGeometryPredicate('http://www.opengis.net/ont/geosparql#asWKT')).toBe(false);
    expect(isGeometryPredicate(undefined)).toBe(false);
  });

  it('matches ifcGuid case-sensitively, like the feed STRENDS filter', () => {
    expect(isIfcGuidPredicate('http://example.org/props#ifcGuid')).toBe(true);
    expect(isIfcGuidPredicate('https://w3id.org/props#ifcGuid')).toBe(true);
    expect(isIfcGuidPredicate('http://example.org/props#IfcGUID')).toBe(false);
    expect(isIfcGuidPredicate('http://example.org/props#ifcguid')).toBe(false);
    expect(isIfcGuidPredicate('')).toBe(false);
    expect(isIfcGuidPredicate(null)).toBe(false);
  });
});

describe('elementsToGeoJSON', () => {
  it('splits features by kind and flags modelled elements', () => {
    const gj = elementsToGeoJSON([
      { id: 'a', label: 'A', wkt4326: 'POINT(5.86 51.85)', files: [['Stl', 'https://x.test/a.stl']] },
      { id: 'b', wkt4326: 'LINESTRING(5.85 51.84, 5.87 51.86)' },
      { id: 'c', wkt4326: 'POLYGON((5.8 51.8, 5.9 51.8, 5.9 51.9, 5.8 51.8))' },
    ]);
    expect(gj.points).toHaveLength(1);
    expect(gj.lines).toHaveLength(1);
    expect(gj.polygons).toHaveLength(1);
    expect(gj.points[0].properties).toEqual({ id: 'a', label: 'A', hasModel: true });
    expect(gj.points[0].geometry).toEqual({ type: 'Point', coordinates: [5.86, 51.85] });
    expect(gj.lines[0].properties.hasModel).toBe(false);
  });

  it('anchors models at the point, or the centroid of extended geometry', () => {
    expect(modelAnchor({ id: 'a', wkt4326: 'POINT(5.86 51.85)' })).toEqual([5.86, 51.85]);
    const c = modelAnchor({ id: 'b', wkt4326: 'LINESTRING(5.84 51.84, 5.88 51.86)' })!;
    expect(c[0]).toBeCloseTo(5.86, 6);
    expect(c[1]).toBeCloseTo(51.85, 6);
  });
});
