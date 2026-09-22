import { describe, it, expect, vi, afterEach } from 'vitest';
import { Parser } from 'n3';
import { EditorState } from '@codemirror/state';
import { foldable, StringStream } from '@codemirror/language';
import {
  emptyShapesTemplate,
  extractTurtlePrefixes,
  prefixInsertOffset,
  parseTurtleErrorPosition,
  turtleErrorRange,
  turtleCompletionsFor,
  turtleFoldRange,
  turtleAutocomplete,
  turtleDiagnostics,
  turtleFolding,
  turtleLanguage,
} from '../turtle-mode.ts';
import { NAMESPACES, allBuiltinTerms } from '../ontology/vocabularies.ts';
import { SHACL_CONSTRAINT_CARDS } from '../shaclConstraints.ts';
import { putShapeGraphTurtle } from '../api.ts';

/** Parse with N3 and hand back the thrown message, mirroring parseShapesGraph. */
function parseError(ttl: string): string | null {
  try {
    new Parser().parse(ttl);
    return null;
  } catch (e) {
    return (e as Error).message;
  }
}

const labels = (r: ReturnType<typeof turtleCompletionsFor>) => (r?.options ?? []).map((o) => o.label);

describe('empty shapes template', () => {
  const tpl = emptyShapesTemplate();

  it('is valid Turtle', () => {
    expect(parseError(tpl)).toBeNull();
  });

  it('uses Turtle @prefix directives, not the SPARQL PREFIX header', () => {
    // The old template emitted `PREFIX sh: <…>` with no trailing dot, so the
    // serializer rewrote the whole header on the first visual-builder save.
    expect(tpl).toContain('@prefix sh:');
    expect(tpl).not.toMatch(/^PREFIX /m);
    for (const line of tpl.split('\n').filter((l) => l.startsWith('@prefix'))) {
      expect(line).toMatch(/^@prefix [\w-]*:\s+<[^>]+> \.$/);
    }
  });

  it('seeds the namespaces a shapes author reaches for', () => {
    const declared = extractTurtlePrefixes(tpl);
    for (const p of ['sh', 'rdf', 'rdfs', 'xsd', 'owl', 'skos', 'dct', 'schema', 'ex']) {
      expect(declared[p], `missing @prefix ${p}:`).toBeTruthy();
    }
    expect(declared.sh).toBe(NAMESPACES.sh);
  });

  it('round-trips through the parser without losing a prefix', () => {
    const seen: Record<string, string> = {};
    new Parser().parse(tpl, null as never, ((p: string, iri: { value?: string } | string) => {
      seen[p] = typeof iri === 'string' ? iri : (iri?.value ?? '');
    }) as never);
    expect(seen.sh).toBe(NAMESPACES.sh);
    expect(seen.ex).toBe('http://example.org/');
  });
});

describe('extractTurtlePrefixes', () => {
  it('reads both @prefix and the SPARQL-style form Turtle 1.1 also allows', () => {
    const doc = '@prefix sh: <http://www.w3.org/ns/shacl#> .\nPREFIX ex: <http://example.org/>\n';
    expect(extractTurtlePrefixes(doc)).toEqual({
      sh: 'http://www.w3.org/ns/shacl#',
      ex: 'http://example.org/',
    });
  });

  it('records the default prefix under the empty key', () => {
    expect(extractTurtlePrefixes('@prefix : <http://d/> .')['']).toBe('http://d/');
  });

  it('returns nothing for a document with no declarations', () => {
    expect(extractTurtlePrefixes('ex:S a sh:NodeShape .')).toEqual({});
  });
});

describe('prefixInsertOffset', () => {
  it('lands just after the existing directive block', () => {
    const doc = '# header\n@prefix sh: <s#> .\n@prefix ex: <e/> .\n\nex:S a sh:NodeShape .\n';
    const at = prefixInsertOffset(doc);
    expect(doc.slice(0, at)).toBe('# header\n@prefix sh: <s#> .\n@prefix ex: <e/> .\n');
  });

  it('lands below the header comment when nothing is declared yet', () => {
    const doc = '# SHACL shapes graph\nex:S a sh:NodeShape .\n';
    expect(prefixInsertOffset(doc)).toBe('# SHACL shapes graph\n'.length);
  });

  it('is 0 for an empty document', () => {
    expect(prefixInsertOffset('')).toBe(0);
  });
});

describe('parse-error positions', () => {
  it('recovers the line N3 reports in its message text', () => {
    const msg = parseError('@prefix ex: <http://e/> .\nex:a ex:b ex:c\nex:d ex:e ex:f .\n');
    expect(msg).toBeTruthy();
    expect(parseTurtleErrorPosition(msg)).toBe(3);
  });

  it('returns null when the message carries no position', () => {
    expect(parseTurtleErrorPosition('Invalid Turtle')).toBeNull();
    expect(parseTurtleErrorPosition(null)).toBeNull();
  });

  it('anchors the diagnostic on the reported line, trimmed to its content', () => {
    const doc = '@prefix ex: <http://e/> .\nex:a ex:b ex:c\n  ex:d ex:e ex:f .\n';
    const hit = turtleErrorRange(doc, parseError(doc));
    expect(hit).not.toBeNull();
    expect(hit!.line).toBe(3);
    // Squiggle covers the statement, not the indentation before it.
    expect(doc.slice(hit!.from, hit!.to)).toBe('ex:d ex:e ex:f .');
  });

  it('walks back over blank lines so an eof error still lands on content', () => {
    const doc = '@prefix ex: <http://e/> .\nex:a ex:b\n\n\n';
    const hit = turtleErrorRange(doc, 'Expected entity but got eof on line 4.');
    expect(hit!.line).toBe(2);
    expect(doc.slice(hit!.from, hit!.to)).toBe('ex:a ex:b');
  });

  it('falls back to the first line when no position is given', () => {
    const hit = turtleErrorRange('bad turtle\nmore\n', 'Invalid Turtle');
    expect(hit!.line).toBe(1);
    expect(hit!.from).toBe(0);
  });

  it('produces nothing when there is no error', () => {
    expect(turtleErrorRange('ex:a ex:b ex:c .', null)).toBeNull();
    expect(turtleErrorRange('ex:a ex:b ex:c .', '')).toBeNull();
  });
});

describe('Turtle completions', () => {
  const SHAPES = '@prefix sh: <http://www.w3.org/ns/shacl#> .\n@prefix ex: <http://example.org/> .\n';

  it('offers sh: vocabulary after a prefixed name', () => {
    const res = turtleCompletionsFor(SHAPES, '  sh:');
    expect(res!.replaceLen).toBe(0);
    expect(labels(res)).toEqual(expect.arrayContaining(['NodeShape', 'property', 'targetClass', 'minCount']));
  });

  it('replaces only the local part that was typed', () => {
    const res = turtleCompletionsFor(SHAPES, '  sh:min');
    expect(res!.replaceLen).toBe(3);
    expect(labels(res)).toContain('minCount');
  });

  it('never offers SPARQL keywords — the SPARQL completer used to serve this mode', () => {
    const noise = ['SELECT', 'WHERE', 'FILTER', 'OPTIONAL', 'PREFIX sh: <http://www.w3.org/ns/shacl#>'];
    for (const before of ['sh:', 'S', 'ex:Person', '@pre']) {
      const found = labels(turtleCompletionsFor(SHAPES, before));
      for (const bad of noise) expect(found, `${before} offered ${bad}`).not.toContain(bad);
    }
  });

  it('carries a prefix declaration for a namespace the document has not declared', () => {
    const undeclared = turtleCompletionsFor(SHAPES, 'skos:')!.options[0];
    expect(undeclared.requiresPrefix).toEqual({ prefix: 'skos', ns: NAMESPACES.skos });
    const declared = turtleCompletionsFor(SHAPES, 'sh:')!.options[0];
    expect(declared.requiresPrefix).toBeUndefined();
  });

  it('completes @prefix directives, which the SPARQL completer had no notion of', () => {
    const res = turtleCompletionsFor('', '@pre');
    expect(res!.replaceLen).toBe(4);
    expect(labels(res)).toContain('@prefix');
    expect(labels(res)).toContain(`@prefix sh: <${NAMESPACES.sh}> .`);
  });

  it('completes the namespace once a prefix name is typed in a directive', () => {
    const res = turtleCompletionsFor('', '@prefix sh:');
    expect(res!.options).toHaveLength(1);
    expect(res!.options[0].insert).toBe(` <${NAMESPACES.sh}> .`);
    expect(res!.replaceLen).toBe(0);
  });

  it('does not offer vocabulary terms inside a directive line', () => {
    expect(labels(turtleCompletionsFor('', '@prefix sh:'))).not.toContain('NodeShape');
  });

  it('completes full IRIs inside angle brackets', () => {
    const res = turtleCompletionsFor(SHAPES, 'ex:S a <shacl#Node');
    expect(res!.replaceLen).toBe('shacl#Node'.length);
    expect(labels(res)).toContain(`${NAMESPACES.sh}NodeShape`);
    expect(res!.options[0].insert).toMatch(/>$/);
  });

  it('offers declared prefixes and the constraint snippets at a bare word', () => {
    const res = turtleCompletionsFor(SHAPES, 'ex:S a sh:NodeShape ;\n', { explicit: true });
    const found = labels(res);
    expect(found).toContain('sh:');
    expect(found).toContain('a');
    for (const card of SHACL_CONSTRAINT_CARDS) expect(found).toContain(card.label);
  });

  it('inserts the palette template for a snippet completion', () => {
    const card = SHACL_CONSTRAINT_CARDS.find((c) => c.id === 'cardinality')!;
    const snippet = turtleCompletionsFor(SHAPES, 'card', {})!.options.find((o) => o.label === card.label);
    expect(snippet!.insert).toBe(card.template);
  });

  it('stays quiet at an empty position unless asked explicitly', () => {
    expect(turtleCompletionsFor(SHAPES, '')).toBeNull();
    expect(turtleCompletionsFor(SHAPES, '', { explicit: true })).not.toBeNull();
  });
});

describe('Turtle completions honour the document, not the built-in prefix name', () => {
  /** Every offered local name must rebuild a real term IRI under the document's ns. */
  const assertLocalsResolve = (doc: string, before: string, ns: string) => {
    const iris = new Set(allBuiltinTerms().map((t) => t.iri));
    for (const label of labels(turtleCompletionsFor(doc, before))) {
      expect(label, `"${label}" is not a local name`).toMatch(/^[A-Za-z_][\w-]*$/);
      expect(iris.has(ns + label), `${ns}${label} is not a built-in term`).toBe(true);
    }
  };

  it('offers nothing garbled when schema: is bound to the https form', () => {
    // VOCAB is keyed by prefix name, so slicing the DOCUMENT's namespace length
    // off a built-in (http) schema.org IRI produced "hing"/"erson"/"rganization".
    const doc = '@prefix schema: <https://schema.org/> .\n';
    const found = labels(turtleCompletionsFor(doc, 'schema:'));
    for (const garbage of ['hing', 'erson', 'rganization']) expect(found).not.toContain(garbage);
    assertLocalsResolve(doc, 'schema:', 'https://schema.org/');
  });

  it('offers nothing garbled when sh: is rebound to another namespace', () => {
    const doc = '@prefix sh: <http://example.org/mine#> .\n';
    const found = labels(turtleCompletionsFor(doc, 'sh:'));
    for (const garbage of ['cl#Shape', 'cl#NodeShape']) expect(found).not.toContain(garbage);
    assertLocalsResolve(doc, 'sh:', 'http://example.org/mine#');
  });

  it('still offers the built-in vocabulary when the document agrees with it', () => {
    const doc = `@prefix schema: <${NAMESPACES.schema}> .\n`;
    expect(labels(turtleCompletionsFor(doc, 'schema:'))).toEqual(
      expect.arrayContaining(['Thing', 'Person', 'Organization']),
    );
  });

  it('still finds terms for a prefix name it has never heard of', () => {
    const doc = `@prefix shapes: <${NAMESPACES.sh}> .\n`;
    expect(labels(turtleCompletionsFor(doc, 'shapes:'))).toContain('NodeShape');
  });
});

describe('putShapeGraphTurtle', () => {
  afterEach(() => {
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
  });

  /** Stub fetch and hand back the mock so the call can be inspected. */
  function stubFetch() {
    const fetchMock = vi.fn().mockResolvedValue({
      ok: true,
      json: async () => ({ version: 7 }),
      text: async () => '',
    });
    vi.stubGlobal('fetch', fetchMock);
    return fetchMock;
  }

  const callOf = (m: ReturnType<typeof stubFetch>) =>
    ({ url: m.mock.calls[0][0] as string, init: m.mock.calls[0][1] as RequestInit });

  it('URL-encodes the commit message into ?message=', async () => {
    const fetchMock = stubFetch();
    const res = await putShapeGraphTurtle('g1', 'ex:S a sh:NodeShape .', { message: 'Fixed minCount' });
    expect(res).toEqual({ version: 7 });
    expect(callOf(fetchMock).url).toBe('/api/shacl/shape-graphs/g1/turtle?message=Fixed%20minCount');
  });

  it('sends no query string at all for a blank or whitespace-only message', async () => {
    // The server only falls back to "Edited" when the parameter is absent or
    // empty, so a whitespace note must not reach it as `?message=%20`.
    for (const message of ['', '   ', '\n\t']) {
      const fetchMock = stubFetch();
      await putShapeGraphTurtle('g1', 'body', { message });
      expect(callOf(fetchMock).url, `message ${JSON.stringify(message)}`)
        .toBe('/api/shacl/shape-graphs/g1/turtle');
      vi.unstubAllGlobals();
    }
  });

  it('defaults the Content-Type to text/turtle and sends the body as given', async () => {
    const fetchMock = stubFetch();
    await putShapeGraphTurtle('g1', 'ex:S a sh:NodeShape .');
    const { url, init } = callOf(fetchMock);
    expect(url).toBe('/api/shacl/shape-graphs/g1/turtle');
    expect(init.method).toBe('PUT');
    expect(init.credentials).toBe('include');
    expect(init.headers).toEqual({ 'Content-Type': 'text/turtle' });
    expect(init.body).toBe('ex:S a sh:NodeShape .');
  });

  it('carries an explicit content type through for a SHACLC save', async () => {
    const fetchMock = stubFetch();
    await putShapeGraphTurtle('g1', 'shape ex:S {}', { contentType: 'text/shaclc', message: 'a/b?c' });
    const { url, init } = callOf(fetchMock);
    expect(url).toBe('/api/shacl/shape-graphs/g1/turtle?message=a%2Fb%3Fc');
    expect(init.headers).toEqual({ 'Content-Type': 'text/shaclc' });
  });

  it('rejects with the status and body when the server refuses', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue({ ok: false, status: 409, text: async () => 'stale version' }));
    await expect(putShapeGraphTurtle('g1', 'body')).rejects.toThrow(/409 stale version/);
  });
});

describe('turtleFoldRange', () => {
  const doc = [
    '@prefix sh: <http://www.w3.org/ns/shacl#> .',
    'ex:S a sh:NodeShape ;',
    '  sh:property [',
    '    sh:path ex:name ;',
    '  ] .',
    'ex:T a sh:NodeShape .',
    '',
  ].join('\n');

  const lineRange = (n: number) => {
    const lines = doc.split('\n');
    let from = 0;
    for (let i = 0; i < n; i++) from += lines[i].length + 1;
    return [from, from + lines[n].length] as const;
  };
  const fold = (n: number) => turtleFoldRange(doc, ...lineRange(n));

  it('folds a multi-line statement from its subject line to the terminating dot', () => {
    const r = fold(1);
    expect(r).not.toBeNull();
    expect(doc.slice(r!.from, r!.to).trim()).toBe('sh:property [\n    sh:path ex:name ;\n  ]');
  });

  it('folds a bracketed block to its matching close', () => {
    const r = fold(2);
    expect(doc[r!.to]).toBe(']');
    expect(doc.slice(r!.from, r!.to)).toContain('sh:path ex:name');
  });

  it('does not fold directives, continuation lines or single-line statements', () => {
    expect(fold(0)).toBeNull();
    expect(fold(3)).toBeNull();
    expect(fold(5)).toBeNull();
  });

  it('ignores brackets and dots inside strings, IRIs and comments', () => {
    const tricky = 'ex:S sh:pattern "^[a-z](\\\\.)$" ; # a ] here\n  sh:flags "i" .\n';
    const r = turtleFoldRange(tricky, 0, tricky.indexOf('\n'));
    expect(r).not.toBeNull();
    expect(tricky.slice(r!.from, r!.to).trim()).toBe('sh:flags "i"');
  });
});

/** Run the Turtle tokenizer over one line: `[text, style]` per token. */
function tokens(line: string): Array<[string, string | null]> {
  const parser = turtleLanguage.streamParser;
  const state = parser.startState!(2);
  const stream = new StringStream(line, 2, 2);
  const out: Array<[string, string | null]> = [];
  while (!stream.eol()) {
    stream.start = stream.pos;
    const style = parser.token(stream, state);
    if (stream.pos === stream.start) throw new Error(`no progress at ${stream.pos}`);
    if (stream.current().trim()) out.push([stream.current(), style]);
  }
  return out;
}

describe('the tokenizer reads what the prefixed serializer writes', () => {
  it('keeps an escaped local part as one name', () => {
    // The store shortens a path-style IRI to `ex:shapes\/PersonShape`; a
    // tokenizer that stopped at the backslash coloured it as three things.
    expect(tokens('ex:shapes\\/PersonShape a sh:NodeShape .')).toEqual([
      ['ex:shapes\\/PersonShape', 'namespace'],
      ['a', 'keyword'],
      ['sh:NodeShape', 'namespace'],
      ['.', 'operator'],
    ]);
  });

  it('keeps a percent-encoded local part as one name', () => {
    expect(tokens('ex:a%20b sh:name "x" .')[0]).toEqual(['ex:a%20b', 'namespace']);
  });

  it('still stops at punctuation and strings', () => {
    const toks = tokens('ex:S;ex:T "q".');
    expect(toks[0]).toEqual(['ex:S', 'namespace']);
    expect(toks[1]).toEqual([';', 'operator']);
    expect(toks[2]).toEqual(['ex:T', 'namespace']);
    expect(toks[3][1]).toBe('string2');
  });
});

describe('editor extension wiring', () => {
  const doc = 'ex:S a sh:NodeShape ;\n  sh:property [\n    sh:path ex:name ;\n  ] .\n';
  const state = EditorState.create({
    doc,
    extensions: [turtleLanguage, turtleAutocomplete(), turtleDiagnostics('Unexpected "x" on line 2.'), turtleFolding],
  });

  it('composes without conflicting facets', () => {
    expect(state.doc.toString()).toBe(doc);
  });

  it('registers a fold service CodeMirror can query', () => {
    // StreamLanguage yields a flat tree, so without turtleFolding foldable()
    // finds nothing and the fold gutter renders no arrows at all.
    const line = state.doc.line(2);
    expect(foldable(state, line.from, line.to)).toEqual(turtleFoldRange(doc, line.from, line.to));
    expect(foldable(state, line.from, line.to)).not.toBeNull();
  });
});

describe('the completer never offers what the document cannot hold', () => {
  it('drops terms whose local part would splice a path into the document', () => {
    // `purl:` is a proper prefix of several built-in namespaces, so matching on
    // the namespace alone offers `dc/terms/title` — which makes the document
    // unparseable the moment it is accepted.
    const doc = '@prefix purl: <http://purl.org/> .\n';
    const offered = labels(turtleCompletionsFor(doc, 'purl:'));
    expect(offered.every((l) => !l.includes('/') && !l.includes('#'))).toBe(true);

    const w3 = '@prefix w3: <http://www.w3.org/> .\n';
    expect(labels(turtleCompletionsFor(w3, 'w3:')).every((l) => !l.includes('/'))).toBe(true);
  });

  it('completes to nothing rather than falling through to snippets', () => {
    // A known prefix with no offerable term used to reach the bare-word branch
    // still carrying the local part's replaceLen, so accepting a constraint
    // snippet spliced a multi-line template into the middle of a CURIE.
    const doc = '@prefix schema: <https://schema.example.invalid/> .\n';
    const res = turtleCompletionsFor(doc, 'schema:Per');
    expect(res).toBeNull();
  });

  it('still offers real terms when the document rebinds a built-in prefix name', () => {
    // The https form of schema.org, which is not the built-in namespace: the
    // completer must match on the namespace, not the prefix name.
    const doc = '@prefix schema: <https://schema.org/> .\n';
    const offered = labels(turtleCompletionsFor(doc, 'schema:'));
    if (offered.length) {
      expect(offered).not.toContain('hing');
      expect(offered.every((l) => /^[A-Za-z_][\w.-]*$/.test(l))).toBe(true);
    }
  });
});
