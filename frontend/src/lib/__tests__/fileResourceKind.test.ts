import { describe, it, expect } from 'vitest';
import { fileResourceKind } from '../viewer/detect';

describe('fileResourceKind', () => {
  it('classifies 3D models (reusing modelFormatFromUrl, incl. .city.json)', () => {
    expect(fileResourceKind('/samples/schependomlaan-3dbag.city.json')).toEqual({
      kind: 'model3d',
      format: 'cityjson',
    });
    expect(fileResourceKind('https://x.test/a.glb')).toEqual({ kind: 'model3d', format: 'gltf' });
    expect(fileResourceKind('/assets/m.stl')).toEqual({ kind: 'model3d', format: 'stl' });
    expect(fileResourceKind('https://x.test/lod2.gml')).toEqual({ kind: 'model3d', format: 'citygml' });
    expect(fileResourceKind('/models/building.ifc')).toEqual({ kind: 'model3d', format: 'ifc' });
  });

  it('classifies images, pdf, json, text and binary by extension', () => {
    expect(fileResourceKind('/img/pic.png')).toEqual({ kind: 'image' });
    expect(fileResourceKind('https://x.test/photo.JPEG')).toEqual({ kind: 'image' }); // case-insensitive
    expect(fileResourceKind('/logo.svg')).toEqual({ kind: 'image' });
    expect(fileResourceKind('/docs/report.pdf')).toEqual({ kind: 'pdf' });
    expect(fileResourceKind('/data/features.geojson')).toEqual({ kind: 'json' });
    expect(fileResourceKind('/data/blob.json')).toEqual({ kind: 'json' });
    expect(fileResourceKind('/vocab/ots.ttl')).toEqual({ kind: 'text' });
    expect(fileResourceKind('/notes.md')).toEqual({ kind: 'text' });
    expect(fileResourceKind('/table.csv')).toEqual({ kind: 'text' });
    expect(fileResourceKind('/archive.zip')).toEqual({ kind: 'binary' });
  });

  it('treats ./ and ../ relative paths as files too', () => {
    expect(fileResourceKind('./local.png')).toEqual({ kind: 'image' });
    expect(fileResourceKind('../up/data.json')).toEqual({ kind: 'json' });
  });

  it('returns null for plain RDF resource IRIs with no file extension', () => {
    expect(fileResourceKind('https://data.3dbag.nl/def/Building')).toBeNull();
    expect(fileResourceKind('http://example.org/Thing')).toBeNull();
    expect(fileResourceKind('http://www.w3.org/2000/01/rdf-schema#label')).toBeNull();
    // A dot in a directory segment is not a file extension.
    expect(fileResourceKind('https://w3id.org/omg#hasGeometry')).toBeNull();
    expect(fileResourceKind('https://example.org/v1.0/Thing')).toBeNull();
  });

  it('ignores query strings and fragments when reading the extension', () => {
    expect(fileResourceKind('https://x.test/data.json?v=2')).toEqual({ kind: 'json' });
    expect(fileResourceKind('https://x.test/doc.pdf#page=3')).toEqual({ kind: 'pdf' });
  });

  it('returns null for non-file schemes and empty input', () => {
    expect(fileResourceKind('urn:isbn:123')).toBeNull();
    expect(fileResourceKind('mailto:a@b.com')).toBeNull();
    expect(fileResourceKind('_:bnode')).toBeNull();
    expect(fileResourceKind('')).toBeNull();
    expect(fileResourceKind(null)).toBeNull();
    expect(fileResourceKind(undefined)).toBeNull();
  });
});
