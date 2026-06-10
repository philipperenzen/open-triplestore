import { describe, it, expect } from 'vitest';
import {
  parseChatBlocks,
  parseApiEndpoint,
  describeApiService,
  parseChartSpec,
  parseMapSpec,
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
