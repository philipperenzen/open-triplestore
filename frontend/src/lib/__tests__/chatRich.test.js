import { describe, it, expect } from 'vitest';
import {
  parseChatBlocks,
  parseApiEndpoint,
  describeApiService,
  parseChartSpec,
  parseMapSpec,
  parseModel3dSpec,
  parseFileSpec,
  parseInfoCard,
  parseCsv,
  sniffLang,
  normalizeSparqlResult,
  decorateApiLinks,
  lenientJsonParse,
} from '../chatRich.js';

describe('parseChatBlocks', () => {
  it('keeps plain markdown as a single segment', () => {
    const segs = parseChatBlocks('Hello **world**\n\n- a\n- b');
    expect(segs).toEqual([{ kind: 'md', source: 'Hello **world**\n\n- a\n- b' }]);
  });

  it('splits a sparql fence into a runnable segment between markdown', () => {
    const segs = parseChatBlocks(
      'Run this:\n```sparql\nSELECT ?s WHERE { ?s ?p ?o }\n```\nDone.'
    );
    expect(segs.map((s) => s.kind)).toEqual(['md', 'sparql', 'md']);
    expect(segs[1].code).toBe('SELECT ?s WHERE { ?s ?p ?o }');
  });

  it('turns an api fence into a runnable api segment', () => {
    const segs = parseChatBlocks(
      '```api\nGET /api/datasets/spatial/api-services/all-geometries/run\n```'
    );
    expect(segs).toEqual([
      { kind: 'api', method: 'GET', path: '/api/datasets/spatial/api-services/all-geometries/run' },
    ]);
  });

  it('detects an untagged fence that contains only a GET /api line (the common model output)', () => {
    const segs = parseChatBlocks(
      'You can run the "All geometries" API:\n\n```\nGET /api/datasets/spatial/api-services/all-geometries/run\n```'
    );
    expect(segs.map((s) => s.kind)).toEqual(['md', 'api']);
    expect(segs[1].path).toBe('/api/datasets/spatial/api-services/all-geometries/run');
  });

  it('detects untagged SPARQL but leaves untagged Turtle in the markdown flow', () => {
    const sparql = parseChatBlocks('```\nPREFIX ex: <http://x/>\nSELECT ?s WHERE { ?s a ex:T }\n```');
    expect(sparql[0].kind).toBe('sparql');
    const ttl = parseChatBlocks('```\n@prefix ex: <http://x/> .\nex:a ex:b ex:c .\n```');
    expect(ttl).toHaveLength(1);
    expect(ttl[0].kind).toBe('md');
  });

  it('parses chart / map / card / csv fences', () => {
    const segs = parseChatBlocks(
      [
        '```chart',
        '{"type":"bar","title":"T","data":[{"label":"A","value":1},{"label":"B","value":2}]}',
        '```',
        '```map',
        '{"features":[{"label":"Waalbrug","wkt":"POINT(5.86 51.85)"}]}',
        '```',
        '```card',
        '{"title":"Waalbrug","facts":[{"label":"Type","value":"Bridge"}]}',
        '```',
        '```csv',
        'name,count',
        'a,1',
        '```',
      ].join('\n')
    );
    expect(segs.map((s) => s.kind)).toEqual(['chart', 'map', 'card', 'csv']);
    expect(segs[0].spec.series[0].data).toHaveLength(2);
    expect(segs[1].features[0].label).toBe('Waalbrug');
    expect(segs[2].card.title).toBe('Waalbrug');
    expect(segs[3].rows).toEqual([['a', '1']]);
  });

  it('flags malformed widget JSON as broken instead of dropping it', () => {
    const segs = parseChatBlocks('```chart\n{not json\n```');
    expect(segs[0].kind).toBe('broken');
    expect(segs[0].label).toBe('chart');
    expect(segs[0].raw).toBe('{not json');
  });

  it('leaves ordinary code fences (turtle, json) inside the markdown flow', () => {
    const segs = parseChatBlocks('```turtle\n@prefix ex: <http://x/> .\n```');
    expect(segs).toHaveLength(1);
    expect(segs[0].kind).toBe('md');
    expect(segs[0].source).toContain('```turtle');
  });

  it('survives an unterminated fence', () => {
    const segs = parseChatBlocks('text\n```sparql\nSELECT ?s WHERE { ?s ?p ?o }');
    expect(segs.map((s) => s.kind)).toEqual(['md', 'sparql']);
  });
});

describe('parseApiEndpoint', () => {
  it('accepts GET lines, bare /api paths and full URLs', () => {
    expect(parseApiEndpoint('GET /api/x/run?p=1')).toEqual({ method: 'GET', path: '/api/x/run?p=1' });
    expect(parseApiEndpoint('/api/datasets/d/api-services/s/run')).toEqual({
      method: 'GET',
      path: '/api/datasets/d/api-services/s/run',
    });
    expect(parseApiEndpoint('https://host.example/api/x/run')).toEqual({
      method: 'GET',
      path: '/api/x/run',
    });
    expect(parseApiEndpoint('`GET /api/x/run`')).toEqual({ method: 'GET', path: '/api/x/run' });
  });

  it('rejects non-GET methods, non-/api paths and prose', () => {
    expect(parseApiEndpoint('POST /api/x/run')).toBeNull();
    expect(parseApiEndpoint('DELETE /api/x')).toBeNull();
    expect(parseApiEndpoint('GET /sparql')).toBeNull();
    expect(parseApiEndpoint('call the api please')).toBeNull();
    expect(parseApiEndpoint('')).toBeNull();
  });
});

describe('describeApiService', () => {
  it('parses dataset/org/group run paths with query params', () => {
    expect(describeApiService('/api/datasets/abc/api-services/top-n/run?limit=5')).toEqual({
      scope: 'datasets',
      ownerId: 'abc',
      slug: 'top-n',
      params: { limit: '5' },
    });
    expect(describeApiService('/api/organisations/o1/api-services/q/run').scope).toBe('organisations');
    expect(describeApiService('/api/groups/g1/api-services/q/run').scope).toBe('groups');
  });

  it('returns null for other api paths', () => {
    expect(describeApiService('/api/datasets/abc')).toBeNull();
    expect(describeApiService('/api/llm/chat')).toBeNull();
  });
});

describe('parseChartSpec', () => {
  it('normalises single-series data and coerces numeric strings', () => {
    const { spec } = parseChartSpec('{"type":"pie","data":[{"label":"A","value":"3"},{"label":"B","value":1}]}');
    expect(spec.type).toBe('pie');
    expect(spec.series[0].data).toEqual([
      { label: 'A', value: 3 },
      { label: 'B', value: 1 },
    ]);
  });

  it('accepts x/y point objects and multi-series', () => {
    const { spec } = parseChartSpec(
      '{"type":"line","series":[{"name":"s1","data":[{"x":"Jan","y":2}]},{"name":"s2","data":[[ "Jan", 4 ]]}]}'
    );
    expect(spec.series).toHaveLength(2);
    expect(spec.series[0].data[0]).toEqual({ label: 'Jan', value: 2 });
    expect(spec.series[1].data[0]).toEqual({ label: 'Jan', value: 4 });
  });

  it('rejects empty or non-numeric data', () => {
    expect(parseChartSpec('{"type":"bar","data":[]}').error).toBeTruthy();
    expect(parseChartSpec('{"type":"bar","data":[{"label":"A","value":"many"}]}').error).toBeTruthy();
    expect(parseChartSpec('nope').error).toBe('invalid JSON');
  });
});

describe('parseMapSpec', () => {
  it('accepts the features object, a bare array, and plain WKT lines', () => {
    expect(parseMapSpec('{"features":[{"wkt":"POINT(1 2)","label":"P"}]}').features).toEqual([
      { wkt: 'POINT(1 2)', label: 'P', iri: '' },
    ]);
    expect(parseMapSpec('["POINT(1 2)"]').features[0].wkt).toBe('POINT(1 2)');
    expect(parseMapSpec('POINT(1 2)\nPOINT(3 4)').features).toHaveLength(2);
  });

  it('reports malformed specs', () => {
    expect(parseMapSpec('{"features":7}').error).toBeTruthy();
    expect(parseMapSpec('{oops').error).toBe('invalid JSON');
  });

  it('returns empty models for the plain feature forms', () => {
    expect(parseMapSpec('{"features":[{"wkt":"POINT(1 2)"}]}').models).toEqual([]);
    expect(parseMapSpec('POINT(1 2)').models).toEqual([]);
  });

  it('parses validated 3D models alongside features', () => {
    const { features, models } = parseMapSpec(
      '{"features":[{"label":"Site","wkt":"POINT(5.86 51.85)"}],' +
        '"models":[{"label":"Bridge","url":"https://x.example/b.glb","wkt":"POINT(5.86 51.85)"}]}'
    );
    expect(features).toHaveLength(1);
    expect(models).toEqual([
      { label: 'Bridge', url: 'https://x.example/b.glb', format: 'gltf', wkt: 'POINT(5.86 51.85)' },
    ]);
  });

  it('drops invalid models (unsafe URL / undetectable format / non-POINT anchor) and keeps the features', () => {
    const { features, models, error } = parseMapSpec(
      '{"features":[{"wkt":"POINT(1 2)"}],"models":[' +
        '{"url":"javascript:alert(1)","wkt":"POINT(1 2)"},' +
        '{"url":"https://x.example/page.html","wkt":"POINT(1 2)"},' +
        '{"url":"https://x.example/m.glb","wkt":"LINESTRING(1 2, 3 4)"}]}'
    );
    expect(error).toBeUndefined();
    expect(models).toEqual([]);
    expect(features).toHaveLength(1);
  });

  it('accepts a models-only map — the models carry their own POINT anchors', () => {
    const { features, models, error } = parseMapSpec(
      '{"models":[{"url":"/files/t.cityjson","wkt":"POINT(4.3 52.1)"}]}'
    );
    expect(error).toBeUndefined();
    expect(features).toEqual([]);
    expect(models[0].format).toBe('cityjson');
  });

  it('still errors when neither features nor models survive', () => {
    expect(parseMapSpec('{"features":[],"models":[{"url":"javascript:x","wkt":"POINT(1 2)"}]}').error).toBeTruthy();
  });
});

describe('parseModel3dSpec / model3d blocks', () => {
  it('parses a models array into ids + URL-detected formats', () => {
    const segs = parseChatBlocks(
      '```model3d\n{"models":[{"label":"Bridge","url":"https://x.example/bridge.glb"},{"label":"Tower","url":"/files/tower.ifc"}]}\n```'
    );
    expect(segs).toEqual([
      {
        kind: 'model3d',
        models: [
          { id: 'm0', label: 'Bridge', url: 'https://x.example/bridge.glb', format: 'gltf' },
          { id: 'm1', label: 'Tower', url: '/files/tower.ifc', format: 'ifc' },
        ],
      },
    ]);
  });

  it('wraps a single {url} object into models[]', () => {
    const segs = parseChatBlocks('```model3d\n{"url":"https://x.example/m.stl","label":"M"}\n```');
    expect(segs).toEqual([
      { kind: 'model3d', models: [{ id: 'm0', label: 'M', url: 'https://x.example/m.stl', format: 'stl' }] },
    ]);
  });

  it('flags invalid JSON as a broken model3d block', () => {
    const segs = parseChatBlocks('```model3d\n{nope\n```');
    expect(segs[0]).toMatchObject({ kind: 'broken', label: 'model3d', raw: '{nope' });
  });

  it('is broken when no model URL has a detectable format', () => {
    const segs = parseChatBlocks('```model3d\n{"models":[{"url":"https://x.example/readme.txt"}]}\n```');
    expect(segs[0]).toMatchObject({ kind: 'broken', label: 'model3d' });
  });

  it('drops unsafe-scheme models, keeps loadable ones, and re-numbers ids', () => {
    const mixed = parseModel3dSpec(
      '{"models":[{"url":"javascript:alert(1)"},{"label":"ok","url":"https://x.example/ok.gltf"}]}'
    );
    expect(mixed.models).toEqual([{ id: 'm0', label: 'ok', url: 'https://x.example/ok.gltf', format: 'gltf' }]);
    expect(parseModel3dSpec('{"models":[{"url":"javascript:alert(1)//x.glb"}]}').error).toBeTruthy();
  });

  it('only triggers on the explicit fence tag — untagged JSON stays markdown', () => {
    const segs = parseChatBlocks('```\n{"models":[{"url":"https://x.example/m.glb"}]}\n```');
    expect(segs).toHaveLength(1);
    expect(segs[0].kind).toBe('md');
  });
});

describe('parseFileSpec / file blocks', () => {
  it('parses a valid file spec into a file segment', () => {
    const segs = parseChatBlocks(
      '```file\n{"label":"Quarterly report","url":"https://x.example/files/report.pdf","filename":"report.pdf"}\n```'
    );
    expect(segs).toEqual([
      {
        kind: 'file',
        file: {
          label: 'Quarterly report',
          filename: 'report.pdf',
          url: 'https://x.example/files/report.pdf',
          blocked: false,
        },
      },
    ]);
  });

  it('derives the filename from the URL path when missing', () => {
    const { file } = parseFileSpec('{"url":"/api/assets/a1/download/model%20v2.glb"}');
    expect(file.filename).toBe('model v2.glb');
    expect(file.blocked).toBe(false);
    expect(file.url).toBe('/api/assets/a1/download/model%20v2.glb');
  });

  it('blocks unsafe schemes — the segment carries no URL a renderer could link', () => {
    const segs = parseChatBlocks(
      '```file\n{"label":"x","url":"javascript:alert(document.cookie)","filename":"x.pdf"}\n```'
    );
    expect(segs[0].kind).toBe('file');
    expect(segs[0].file.blocked).toBe(true);
    expect(segs[0].file.url).toBe('');
    expect(parseFileSpec('{"url":"data:text/html,<script>1</script>"}').file.blocked).toBe(true);
  });

  it('reports malformed specs as broken file blocks', () => {
    expect(parseChatBlocks('```file\n{oops\n```')[0]).toMatchObject({ kind: 'broken', label: 'file' });
    expect(parseFileSpec('{"label":"no url"}').error).toBeTruthy();
  });

  it('only triggers on the explicit fence tag', () => {
    const segs = parseChatBlocks('```\n{"url":"https://x.example/report.pdf"}\n```');
    expect(segs).toHaveLength(1);
    expect(segs[0].kind).toBe('md');
  });
});

describe('lenientJsonParse', () => {
  it('tolerates // and /* */ comments and trailing commas, preserving URLs', () => {
    const card = lenientJsonParse(
      '{\n  "title": "Waalbrug",\n  "image": "https://example.com/x.jpg", // replace if available\n  /* facts below */\n  "facts": [{"label":"Type","value":"Bridge"},],\n}'
    );
    expect(card).toEqual({
      title: 'Waalbrug',
      image: 'https://example.com/x.jpg',
      facts: [{ label: 'Type', value: 'Bridge' }],
    });
  });

  it('still parses strict JSON and rejects garbage', () => {
    expect(lenientJsonParse('{"a":1}')).toEqual({ a: 1 });
    expect(lenientJsonParse('{nope')).toBeUndefined();
  });

  it('feeds the card parser so commented specs render instead of breaking', () => {
    const { card } = parseInfoCard('{"title":"X", // note\n"facts":[]}');
    expect(card.title).toBe('X');
  });
});

describe('parseInfoCard', () => {
  it('keeps only well-formed facts and requires a title', () => {
    const { card } = parseInfoCard(
      '{"title":"Waalbrug","subtitle":"Arch bridge","iri":"http://x/waalbrug","facts":[{"label":"Length","value":"604 m"},{"label":"","value":"x"}]}'
    );
    expect(card.title).toBe('Waalbrug');
    expect(card.facts).toEqual([{ label: 'Length', value: '604 m', iri: '' }]);
    expect(parseInfoCard('{"subtitle":"no title"}').error).toBeTruthy();
  });
});

describe('parseCsv', () => {
  it('parses quoted fields, escaped quotes and pads short rows', () => {
    const { columns, rows } = parseCsv('name,note\n"Smith, J","said ""hi"""\nshort');
    expect(columns).toEqual(['name', 'note']);
    expect(rows).toEqual([
      ['Smith, J', 'said "hi"'],
      ['short', ''],
    ]);
  });
});

describe('normalizeSparqlResult', () => {
  it('classifies graph, boolean and bindings responses', () => {
    expect(normalizeSparqlResult({ _graphResult: true, ntriples: '<a> <b> <c> .' }).kind).toBe('graph');
    expect(normalizeSparqlResult({ boolean: true })).toEqual({ kind: 'boolean', value: true });
    const b = normalizeSparqlResult({ head: { vars: ['s'] }, results: { bindings: [{ s: { type: 'uri', value: 'x' } }] } });
    expect(b.kind).toBe('bindings');
    expect(b.vars).toEqual(['s']);
    expect(b.bindings).toHaveLength(1);
  });
});

describe('decorateApiLinks', () => {
  it('marks inline GET /api codes as run buttons', () => {
    const html = decorateApiLinks('<p>Run <code>GET /api/datasets/d/api-services/s/run</code> now</p>');
    expect(html).toContain('chat-api-link');
    expect(html).toContain('data-path="/api/datasets/d/api-services/s/run"');
    expect(html).toContain('role="button"');
  });

  it('replaces anchors that point at run URLs, but leaves other links and pre blocks alone', () => {
    const html = decorateApiLinks(
      '<p><a href="/api/datasets/d/api-services/s/run">All geometries</a> and <a href="/docs/x">docs</a></p>' +
        '<pre><code>GET /api/datasets/d/api-services/s/run</code></pre>'
    );
    expect(html).not.toContain('<a href="/api/datasets/d/api-services/s/run"');
    expect(html).toContain('<a href="/docs/x">docs</a>');
    // The pre>code block is untouched (it is rendered by the block parser instead).
    expect(html.match(/chat-api-link/g)).toHaveLength(1);
  });

  it('does not invent markup from text content', () => {
    const html = decorateApiLinks('<p><code>GET /api/x/run?a=&lt;b&gt;</code></p>');
    expect(html).toContain('&lt;b&gt;');
  });
});

describe('sniffLang', () => {
  it('detects api and sparql, defers everything else', () => {
    expect(sniffLang('GET /api/d/run')).toBe('api');
    expect(sniffLang('SELECT ?s WHERE { ?s ?p ?o }')).toBe('sparql');
    expect(sniffLang('@prefix ex: <http://x/> .')).toBe('');
    expect(sniffLang('just some text')).toBe('');
  });
});
