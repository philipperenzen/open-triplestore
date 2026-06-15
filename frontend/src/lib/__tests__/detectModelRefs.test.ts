// modelRefsOf / modelRefOf: the inspector's 3D tab offers every linked
// representation of an element (glTF / CityJSON / CityGML / STL) as switchable
// chips, preferring glTF. Lock the dedup + ordering in.
import { describe, it, expect } from 'vitest';
import { modelRefsOf, modelRefOf } from '../viewer/detect';

describe('modelRefsOf', () => {
  it('returns every format once, glTF preferred first', () => {
    const refs = modelRefsOf({
      gltf_url: 'https://files.example/tower.glb',
      files: [
        ['cityjson', 'https://files.example/tower.cityjson'],
        ['stl', 'https://files.example/tower.stl'],
      ],
    });
    expect(refs.map((r) => r.format)).toEqual(['gltf', 'cityjson', 'stl']);
    expect(refs[0].url).toBe('https://files.example/tower.glb');
  });

  it('lets the explicit gltf_url win over a glTF in the file list', () => {
    const refs = modelRefsOf({
      gltf_url: 'https://files.example/preferred.glb',
      files: [['Gltf_v2.0-glb', 'https://files.example/other.glb']],
    });
    expect(refs).toHaveLength(1);
    expect(refs[0]).toEqual({ url: 'https://files.example/preferred.glb', format: 'gltf' });
  });

  it('detects format by FOG key or URL extension and dedupes per format', () => {
    const refs = modelRefsOf({
      files: [
        ['CityGML_v2.0', 'https://files.example/a.gml'],
        ['cityjson', 'https://files.example/b.cityjson'],
        ['cityjson', 'https://files.example/dup.cityjson'],
      ],
    });
    expect(refs.map((r) => r.format)).toEqual(['cityjson', 'citygml']);
    expect(refs.find((r) => r.format === 'cityjson')?.url).toBe('https://files.example/b.cityjson');
  });

  it('is empty for an element with no loadable model', () => {
    expect(modelRefsOf({})).toEqual([]);
    expect(modelRefsOf({ files: [['note', 'https://files.example/readme.txt']] })).toEqual([]);
  });

  it('modelRefOf returns the first / preferred ref', () => {
    expect(modelRefOf({ gltf_url: 'https://files.example/x.glb' })).toEqual({
      url: 'https://files.example/x.glb',
      format: 'gltf',
    });
    expect(modelRefOf({})).toBeNull();
  });
});
