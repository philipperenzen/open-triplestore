import { describe, it, expect, vi, beforeEach } from 'vitest';
import { parseTurtle } from '../loader';
import { indexStore, VOCAB_FILES, lookupTerm, lookupTermSync, _resetTermCaches } from '../termDictionary';
import { NAMESPACES } from '../vocabularies';
import { pickLang, groupByLang, pickByLang, langRank, setUiLang, uiLang } from '../termDisplay';

const DCAT = 'http://www.w3.org/ns/dcat#';
const DCT = 'http://purl.org/dc/terms/';
const RDF = 'http://www.w3.org/1999/02/22-rdf-syntax-ns#';
const OWL = 'http://www.w3.org/2002/07/owl#';

// A faithful slice of the canonical dcat:mediaType definition (www.w3.org/ns/dcat3.ttl),
// trimmed to a few languages plus a synthetic @nl label to prove multi-language capture.
const TTL = `
@prefix dcat: <${DCAT}> .
@prefix dcterms: <${DCT}> .
@prefix owl: <${OWL}> .
@prefix rdf: <${RDF}> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix skos: <http://www.w3.org/2004/02/skos/core#> .

dcat:mediaType a rdf:Property ;
  a owl:ObjectProperty ;
  rdfs:comment "The media type of the distribution as defined by IANA"@en ;
  rdfs:comment "Il tipo di media della distribuzione come definito da IANA"@it ;
  rdfs:domain dcat:Distribution ;
  rdfs:isDefinedBy <http://www.w3.org/TR/vocab-dcat/> ;
  rdfs:label "media type"@en ;
  rdfs:label "tipo di media"@it ;
  rdfs:label "mediatype"@nl ;
  rdfs:range dcterms:MediaType ;
  rdfs:subPropertyOf dcterms:format ;
  skos:definition "The media type of the distribution as defined by IANA."@en ;
  skos:scopeNote "This property SHOULD be used when the media type of the distribution is defined in the IANA media types registry."@en ;
  skos:changeNote "The range of dcat:mediaType has been tightened as part of the revision of DCAT."@en ;
  owl:deprecated true ;
  owl:versionInfo "3.0" .
`;

describe('indexStore', () => {
  it('extracts rich, multi-language metadata for dcat:mediaType', async () => {
    const { store } = await parseTurtle(TTL);
    const idx = indexStore(store, 'dcat');
    const m = idx.get(DCAT + 'mediaType');
    expect(m).toBeTruthy();
    if (!m) return;

    // Most-specific type wins, but both are retained.
    expect(m.termType).toBe('owl:ObjectProperty');
    expect(m.allTypes).toEqual(expect.arrayContaining([OWL + 'ObjectProperty', RDF + 'Property']));

    // Every language preserved (no en-collapsing like schema-model.ts).
    expect(m.labels.map((l) => l.lang).sort()).toEqual(['en', 'it', 'nl']);
    expect(m.comments.map((l) => l.lang).sort()).toEqual(['en', 'it']);
    expect(m.definitions).toHaveLength(1);
    expect(m.scopeNotes).toHaveLength(1);
    expect(m.changeNotes).toHaveLength(1);

    // Relationships + annotations.
    expect(m.domain).toContain(DCAT + 'Distribution');
    expect(m.range).toContain(DCT + 'MediaType');
    expect(m.subPropertyOf).toContain(DCT + 'format');
    expect(m.isDefinedBy).toContain('http://www.w3.org/TR/vocab-dcat/');
    expect(m.versionInfo).toContain('3.0');
    expect(m.deprecated).toBe(true);
    expect(m.source).toBe('dcat');
  });

  it('drops content-less subjects (no type, label, comment, definition or relation)', async () => {
    const { store } = await parseTurtle(`
      @prefix owl: <${OWL}> .
      <http://example.org/x> owl:versionInfo "1" .
    `);
    const idx = indexStore(store, 'x');
    expect(idx.get('http://example.org/x')).toBeUndefined();
  });
});

describe('VOCAB_FILES registry', () => {
  it('covers every bundled namespace in NAMESPACES', () => {
    for (const ns of Object.values(NAMESPACES)) {
      expect(VOCAB_FILES[ns], `missing VOCAB_FILES entry for ${ns}`).toBeTruthy();
    }
  });
});

describe('lazy loading', () => {
  beforeEach(() => _resetTermCaches());

  it('fetches a vocabulary file at most once across lookups', async () => {
    const fetchMock = vi.fn(async () => ({ ok: true, status: 200, text: async () => TTL }));
    global.fetch = fetchMock as unknown as typeof fetch;

    const a = await lookupTerm(DCAT + 'mediaType');
    const b = await lookupTerm(DCAT + 'Distribution'); // same file, different term
    expect(a?.termType).toBe('owl:ObjectProperty');
    // dcat:Distribution isn't a *defined term* in our slice → null, but still no 2nd fetch.
    expect(b).toBeNull();
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  it('returns null for an un-bundled namespace without fetching', async () => {
    const fetchMock = vi.fn();
    global.fetch = fetchMock as unknown as typeof fetch;
    expect(await lookupTerm('http://example.com/custom#thing')).toBeNull();
    expect(lookupTermSync('http://example.com/custom#thing')).toBeNull();
    expect(fetchMock).not.toHaveBeenCalled();
  });
});

describe('display helpers', () => {
  const vals = [
    { lang: 'en', value: 'media type' },
    { lang: 'nl', value: 'mediatype' },
    { lang: 'it', value: 'tipo di media' },
  ];

  it('pickLang prefers exact, then base, then English, then first', () => {
    expect(pickLang(vals, 'nl')).toBe('mediatype');
    expect(pickLang(vals, 'nl-BE')).toBe('mediatype'); // base subtag
    expect(pickLang(vals, 'fr')).toBe('media type'); // English fallback
    expect(pickLang([], 'en')).toBe('');
  });

  it('groupByLang orders the active language first, then English', () => {
    expect(groupByLang(vals, 'it').map((v) => v.lang)).toEqual(['it', 'en', 'nl']);
    expect(groupByLang(vals, 'de').map((v) => v.lang)).toEqual(['en', 'it', 'nl']);
  });
});

describe('pickByLang / langRank / uiLang', () => {
  // Arbitrary shapes: the display layer reads labels straight off the wire,
  // where the language tag lives under a different key in every payload.
  const rows = [
    { o: { value: 'Door', 'xml:lang': 'en' } },
    { o: { value: 'Deur', 'xml:lang': 'nl' } },
    { o: { value: 'Tür', 'xml:lang': 'de' } },
  ];
  const langOf = (r: (typeof rows)[number]) => r.o['xml:lang'];

  it('picks the reader language, then English', () => {
    expect(pickByLang(rows, langOf, 'nl')?.o.value).toBe('Deur');
    expect(pickByLang(rows, langOf, 'de')?.o.value).toBe('Tür');
    expect(pickByLang(rows, langOf, 'fr')?.o.value).toBe('Door');
  });

  it('matches a regional tag on its primary subtag', () => {
    expect(pickByLang(rows, langOf, 'nl-BE')?.o.value).toBe('Deur');
    expect(pickByLang(rows, langOf, 'NL')?.o.value).toBe('Deur');
  });

  it('falls back to an untagged literal before an unrelated language', () => {
    const mixed = [{ l: 'de', v: 'Tür' }, { l: '', v: 'Naamloos' }];
    expect(pickByLang(mixed, (x) => x.l, 'nl')?.v).toBe('Naamloos');
  });

  it('keeps the first of equally-ranked values', () => {
    const dupes = [{ l: 'nl', v: 'eerste' }, { l: 'nl', v: 'tweede' }];
    expect(pickByLang(dupes, (x) => x.l, 'nl')?.v).toBe('eerste');
  });

  it('returns null for an empty or missing list', () => {
    expect(pickByLang([], (x: any) => x.l, 'nl')).toBeNull();
    expect(pickByLang(null, (x: any) => x.l, 'nl')).toBeNull();
  });

  it('defaults to the module-level UI language when none is passed', () => {
    setUiLang('nl');
    expect(uiLang()).toBe('nl');
    expect(pickByLang(rows, langOf)?.o.value).toBe('Deur');
    setUiLang('en');
    expect(pickByLang(rows, langOf)?.o.value).toBe('Door');
    setUiLang(null); // resets to the 'en' default
    expect(uiLang()).toBe('en');
  });

  it('langRank orders exact < primary subtag < English < untagged < other', () => {
    expect(langRank('nl', 'nl')).toBeLessThan(langRank('nl-BE', 'nl'));
    expect(langRank('nl-BE', 'nl')).toBeLessThan(langRank('en', 'nl'));
    expect(langRank('en', 'nl')).toBeLessThan(langRank('', 'nl'));
    expect(langRank('', 'nl')).toBeLessThan(langRank('de', 'nl'));
  });
});
