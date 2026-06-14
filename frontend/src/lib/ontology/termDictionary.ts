// Offline term dictionary: lazily fetches a bundled vocabulary file (public/vocab/
// *.ttl), parses it once, and answers IRI → rich TermMeta lookups. This is what
// powers the "see the full definition of dcat:mediaType / owl:* / skos:* / …"
// experience — definitions are available even when the user's own data does not
// contain the vocabulary triples.
import { Store } from 'n3';
import type { LangValue, TermMeta, TermType } from './termTypes';
import { emptyTermMeta, TERM_TYPE_PRECEDENCE } from './termTypes';
import { splitIri } from './vocabularies';
import { parseTurtle } from './loader';

const RDF = 'http://www.w3.org/1999/02/22-rdf-syntax-ns#';
const RDFS = 'http://www.w3.org/2000/01/rdf-schema#';
const OWL = 'http://www.w3.org/2002/07/owl#';
const SKOS = 'http://www.w3.org/2004/02/skos/core#';
const RDF_TYPE = RDF + 'type';

// rdf:type IRI → TermType label, used for most-specific-type resolution.
const TYPE_MAP: Record<string, TermType> = {
  [OWL + 'ObjectProperty']: 'owl:ObjectProperty',
  [OWL + 'DatatypeProperty']: 'owl:DatatypeProperty',
  [OWL + 'AnnotationProperty']: 'owl:AnnotationProperty',
  [RDF + 'Property']: 'rdf:Property',
  [OWL + 'Class']: 'owl:Class',
  [RDFS + 'Class']: 'rdfs:Class',
  [SKOS + 'Concept']: 'skos:Concept',
  [RDFS + 'Datatype']: 'rdfs:Datatype',
  [OWL + 'NamedIndividual']: 'owl:NamedIndividual',
};

// Annotation predicates whose (literal) objects are collected per language.
const LANG_FIELDS: Record<string, keyof TermMeta> = {
  [RDFS + 'label']: 'labels',
  [SKOS + 'prefLabel']: 'labels',
  [RDFS + 'comment']: 'comments',
  [SKOS + 'definition']: 'definitions',
  [SKOS + 'scopeNote']: 'scopeNotes',
  [SKOS + 'changeNote']: 'changeNotes',
  [SKOS + 'editorialNote']: 'editorialNotes',
  [SKOS + 'example']: 'examples',
};

// Predicates whose (IRI) objects are collected as relationship lists.
const IRI_FIELDS: Record<string, keyof TermMeta> = {
  [RDFS + 'domain']: 'domain',
  [RDFS + 'range']: 'range',
  [RDFS + 'subPropertyOf']: 'subPropertyOf',
  [RDFS + 'subClassOf']: 'subClassOf',
  [OWL + 'inverseOf']: 'inverseOf',
  [RDFS + 'isDefinedBy']: 'isDefinedBy',
  [RDFS + 'seeAlso']: 'seeAlso',
};

function resolveTermType(typeIris: string[]): TermType {
  const labels = new Set(
    typeIris.map((t) => TYPE_MAP[t]).filter(Boolean) as TermType[],
  );
  for (const t of TERM_TYPE_PRECEDENCE) if (labels.has(t)) return t;
  return 'unknown';
}

/**
 * Build a flat IRI → TermMeta index from a parsed vocabulary store. Exported so
 * tests can exercise it directly (no fetch). Blank-node subjects (OWL class
 * expressions, restrictions) are skipped — the dictionary is a flat term lens.
 */
export function indexStore(store: Store, source: string): Map<string, TermMeta> {
  const out = new Map<string, TermMeta>();
  const typesBySubject = new Map<string, string[]>();
  const get = (iri: string): TermMeta => {
    let m = out.get(iri);
    if (!m) {
      m = emptyTermMeta(iri, source);
      out.set(iri, m);
    }
    return m;
  };

  for (const q of store.getQuads(null, null, null, null)) {
    if (q.subject.termType !== 'NamedNode') continue;
    const s = q.subject.value;
    const p = q.predicate.value;
    const o = q.object;

    if (p === RDF_TYPE && o.termType === 'NamedNode') {
      const arr = typesBySubject.get(s) || [];
      arr.push(o.value);
      typesBySubject.set(s, arr);
      const m = get(s);
      if (!m.allTypes.includes(o.value)) m.allTypes.push(o.value);
      continue;
    }
    const langField = LANG_FIELDS[p];
    if (langField && o.termType === 'Literal') {
      (get(s)[langField] as LangValue[]).push({ lang: o.language || '', value: o.value });
      continue;
    }
    const iriField = IRI_FIELDS[p];
    if (iriField && o.termType === 'NamedNode') {
      const list = get(s)[iriField] as string[];
      if (!list.includes(o.value)) list.push(o.value);
      // owl:inverseOf is symmetric — record the reverse direction too.
      if (p === OWL + 'inverseOf') {
        const rev = get(o.value).inverseOf;
        if (!rev.includes(s)) rev.push(s);
      }
      continue;
    }
    if (p === OWL + 'versionInfo' && o.termType === 'Literal') {
      get(s).versionInfo.push(o.value);
      continue;
    }
    if (p === OWL + 'deprecated' && o.termType === 'Literal' && o.value === 'true') {
      get(s).deprecated = true;
    }
  }

  // Resolve the most-specific type and drop subjects that carry no term content
  // (e.g. the owl:Ontology header, or a bare inverse-only back-reference).
  for (const [iri, m] of [...out.entries()]) {
    m.termType = resolveTermType(typesBySubject.get(iri) || []);
    const hasContent =
      m.allTypes.length ||
      m.labels.length ||
      m.comments.length ||
      m.definitions.length ||
      m.domain.length ||
      m.range.length ||
      m.subPropertyOf.length ||
      m.subClassOf.length;
    if (!hasContent) out.delete(iri);
  }
  return out;
}

export interface VocabFileSpec {
  file: string;
  source: string;
}

// Namespace IRI → bundled vocabulary file. Kept in sync with NAMESPACES in
// vocabularies.ts by a unit test. Both http/https and the two Dublin Core
// namespaces map to one file each.
export const VOCAB_FILES: Record<string, VocabFileSpec> = {
  'http://www.w3.org/2002/07/owl#': { file: 'owl.ttl', source: 'owl' },
  'http://www.w3.org/2000/01/rdf-schema#': { file: 'rdfs.ttl', source: 'rdfs' },
  'http://www.w3.org/1999/02/22-rdf-syntax-ns#': { file: 'rdf.ttl', source: 'rdf' },
  'http://www.w3.org/2004/02/skos/core#': { file: 'skos.ttl', source: 'skos' },
  'http://www.w3.org/ns/dcat#': { file: 'dcat.ttl', source: 'dcat' },
  'http://xmlns.com/foaf/0.1/': { file: 'foaf.ttl', source: 'foaf' },
  'http://purl.org/dc/terms/': { file: 'dcterms.ttl', source: 'dcterms' },
  'http://purl.org/dc/elements/1.1/': { file: 'dcterms.ttl', source: 'dcterms' },
  'http://www.w3.org/ns/prov#': { file: 'prov.ttl', source: 'prov' },
  'http://www.w3.org/2001/XMLSchema#': { file: 'xsd.ttl', source: 'xsd' },
  'http://purl.org/vocab/vann/': { file: 'vann.ttl', source: 'vann' },
  'http://www.w3.org/ns/org#': { file: 'org.ttl', source: 'org' },
  'http://purl.org/linked-data/cube#': { file: 'qb.ttl', source: 'qb' },
  'http://rdfs.org/ns/void#': { file: 'void.ttl', source: 'void' },
  'http://www.opengis.net/ont/geosparql#': { file: 'geosparql.ttl', source: 'geo' },
  'https://w3id.org/bot#': { file: 'bot.ttl', source: 'bot' },
  'https://w3id.org/omg#': { file: 'omg.ttl', source: 'omg' },
  'https://w3id.org/fog#': { file: 'fog.ttl', source: 'fog' },
  'http://www.w3.org/ns/sosa/': { file: 'sosa.ttl', source: 'sosa' },
  'http://www.w3.org/ns/ssn/': { file: 'ssn.ttl', source: 'ssn' },
  'https://data.3dbag.nl/def/': { file: 'bag.ttl', source: 'bag' },
  'https://saref.etsi.org/core/': { file: 'saref.ttl', source: 'saref' },
  'https://data.rws.nl/otl/def/': { file: 'otl.ttl', source: 'otl' },
  'https://data.crow.nl/imbor/def/': { file: 'imbor.ttl', source: 'imbor' },
  'http://www.w3.org/2006/time#': { file: 'time.ttl', source: 'time' },
  'http://www.w3.org/ns/shacl#': { file: 'shacl.ttl', source: 'sh' },
  'http://schema.org/': { file: 'schema.ttl', source: 'schema' },
  'https://schema.org/': { file: 'schema.ttl', source: 'schema' },
  'https://opentriplestore.org/ontology/': { file: 'ots.ttl', source: 'ots' },
  'https://opentriplestore.org/ns#': { file: 'ots.ttl', source: 'ots' },
  'https://opentriplestore.org/ns/asset#': { file: 'ots.ttl', source: 'ots' },
};

const fileCache = new Map<string, Map<string, TermMeta>>();
const inflight = new Map<string, Promise<Map<string, TermMeta>>>();
const termCache = new Map<string, TermMeta | null>();

function vocabBase(): string {
  try {
    return (import.meta as { env?: { BASE_URL?: string } }).env?.BASE_URL || '/';
  } catch {
    return '/';
  }
}

async function loadFile(spec: VocabFileSpec): Promise<Map<string, TermMeta>> {
  const cached = fileCache.get(spec.file);
  if (cached) return cached;
  const pending = inflight.get(spec.file);
  if (pending) return pending;
  const p = (async () => {
    const res = await fetch(`${vocabBase()}vocab/${spec.file}`, { credentials: 'same-origin' });
    if (!res.ok) throw new Error(`vocab ${spec.file}: ${res.status}`);
    const { store } = await parseTurtle(await res.text());
    const idx = indexStore(store, spec.source);
    fileCache.set(spec.file, idx);
    return idx;
  })()
    .catch(() => {
      // Cache an empty index so a missing/broken file isn't refetched every lookup.
      const empty = new Map<string, TermMeta>();
      fileCache.set(spec.file, empty);
      return empty;
    })
    .finally(() => {
      inflight.delete(spec.file);
    });
  inflight.set(spec.file, p);
  return p;
}

function specForIri(iri: string): VocabFileSpec | null {
  if (!iri) return null;
  return VOCAB_FILES[splitIri(iri).ns] || null;
}

/** Async lookup: lazily loads the term's bundled vocabulary file and returns its
 *  metadata, or null when the namespace isn't bundled or the term is absent. */
export async function lookupTerm(iri: string): Promise<TermMeta | null> {
  if (termCache.has(iri)) return termCache.get(iri) as TermMeta | null;
  const spec = specForIri(iri);
  if (!spec) {
    termCache.set(iri, null);
    return null;
  }
  const meta = (await loadFile(spec)).get(iri) || null;
  termCache.set(iri, meta);
  return meta;
}

/** Sync lookup against already-loaded files only — for instant first paint.
 *  Returns `undefined` when the vocabulary file has not been fetched yet. */
export function lookupTermSync(iri: string): TermMeta | null | undefined {
  if (termCache.has(iri)) return termCache.get(iri);
  const spec = specForIri(iri);
  if (!spec) return null;
  const idx = fileCache.get(spec.file);
  if (!idx) return undefined;
  const meta = idx.get(iri) || null;
  termCache.set(iri, meta);
  return meta;
}

/** Prefetch the vocabulary file an IRI belongs to (fire-and-forget). */
export function warmVocab(iri: string): void {
  const spec = specForIri(iri);
  if (spec && !fileCache.has(spec.file)) void loadFile(spec);
}

/** Test helper: clear all in-memory caches between cases. */
export function _resetTermCaches(): void {
  fileCache.clear();
  inflight.clear();
  termCache.clear();
}
