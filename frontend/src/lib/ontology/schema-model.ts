// Builds the rich SchemaModel consumed by the schema viewer panels.
// Reuses loader.ts:loadOntologyGraph for the SPARQL CONSTRUCT, then walks
// the n3 Store to extract OWL axioms, restrictions, SKOS structures, and
// per-namespace summaries (blank-node-safe, which pure SPARQL cannot easily do).

import { Store, DataFactory } from 'n3';
import type { Term } from 'n3';
import { loadOntologyGraph } from './loader';
import { kindOf, splitIri, prefixForNamespace, type VocabKind } from './vocabularies';
import { isBetterLang } from './termDisplay';
import type { ClassExpr, Restriction } from './dl-render';
import type { PathExpr } from '../shaclModel';

const { namedNode } = DataFactory;

const RDF  = 'http://www.w3.org/1999/02/22-rdf-syntax-ns#';
const RDFS = 'http://www.w3.org/2000/01/rdf-schema#';
const OWL  = 'http://www.w3.org/2002/07/owl#';
const SH   = 'http://www.w3.org/ns/shacl#';
const SKOS = 'http://www.w3.org/2004/02/skos/core#';

const TYPE = RDF + 'type';

export type Characteristic =
  | 'Functional' | 'InverseFunctional' | 'Transitive'
  | 'Symmetric'  | 'Asymmetric' | 'Reflexive' | 'Irreflexive';

const CHAR_BY_TYPE: Record<string, Characteristic> = {
  [OWL + 'FunctionalProperty']:        'Functional',
  [OWL + 'InverseFunctionalProperty']: 'InverseFunctional',
  [OWL + 'TransitiveProperty']:        'Transitive',
  [OWL + 'SymmetricProperty']:         'Symmetric',
  [OWL + 'AsymmetricProperty']:        'Asymmetric',
  [OWL + 'ReflexiveProperty']:         'Reflexive',
  [OWL + 'IrreflexiveProperty']:       'Irreflexive',
};

export type ClassEntry = {
  iri: string;
  label: string;
  comment: string;
  parents: string[];
  children: string[];
  equivalents: ClassExpr[];
  disjoints: string[];
  unionOf?: ClassExpr[];
  intersectionOf?: ClassExpr[];
  complementOf?: ClassExpr;
  oneOf?: string[];
  hasKey?: string[][];
  restrictions: Restriction[];
  instanceCount: number;
  /** Distinct subjects typed with this class inside the loaded graph(s). */
  instances: string[];
  hasShape: boolean;
};

export type PropertyEntry = {
  iri: string;
  kind: 'object' | 'datatype' | 'annotation' | 'property';
  label: string;
  comment: string;
  domain: string[];
  range: string[];
  /** Named members of an `owl:unionOf` domain / range, kept apart from
   *  `domain` / `range` so the property panel still shows what was stated. */
  domainUnion: string[];
  rangeUnion: string[];
  superProperties: string[];
  equivalentProperty: string[];
  inverseOf: string[];
  chains: string[][];
  characteristics: Set<Characteristic>;
};

export type ConceptEntry = {
  iri: string;
  prefLabel: string;
  altLabels: string[];
  hiddenLabels: string[];
  notation: string[];
  scheme: string[];
  topConceptOf: string[];
  broader: string[];
  narrower: string[];
  broaderTransitive: string[];
  related: string[];
};

export type SchemeEntry = { iri: string; label: string; topConcepts: string[] };
export type CollectionEntry = { iri: string; members: string[]; ordered: boolean };

export type NamespaceEntry = {
  ns: string;
  prefix: string | null;
  count: number;
  isImported: boolean;
  kind: VocabKind;
};

export type ShapePropertyEntry = {
  /** The predicate IRI for a plain path; the path node's id otherwise. */
  path: string;
  /** Set only when the path is not a single predicate (inverse, sequence, …). */
  pathExpr?: PathExpr;
  name?: string;
  minCount?: string;
  maxCount?: string;
  datatype?: string;
  cls?: string;
  nodeKind?: string;
  /** Number of members of the `sh:in` list, when there is one. */
  inCount?: number;
  pattern?: string;
};

export type ShapeEntry = {
  iri: string;
  label: string;
  targetClass: string[];
  targetNode: string[];
  /** The shape IRI is itself a class, so it targets that class implicitly. */
  implicitTarget: boolean;
  properties: ShapePropertyEntry[];
};

/**
 * Lookup tables for the class detail pane, built once per model so that
 * selecting a class costs only its own (and its ancestors') entries.
 */
export type SchemaIndex = {
  /** class IRI -> properties whose rdfs:domain (or union domain) names it */
  propsByDomain: Map<string, string[]>;
  /** class IRI -> properties whose rdfs:range (or union range) names it */
  propsByRange: Map<string, string[]>;
  /** class IRI -> shapes that target it (sh:targetClass or implicitly) */
  shapesByClass: Map<string, string[]>;
  /** class IRI -> property shapes whose sh:class names it */
  shapePropsByValueClass: Map<string, Array<{ shape: string; prop: ShapePropertyEntry }>>;
};

export type SchemaModel = {
  classes: Map<string, ClassEntry>;
  properties: Map<string, PropertyEntry>;
  concepts: Map<string, ConceptEntry>;
  schemes: Map<string, SchemeEntry>;
  collections: Map<string, CollectionEntry>;
  namespaces: Map<string, NamespaceEntry>;
  shapes: Map<string, ShapeEntry>;
  imports: string[];
  labels: Map<string, string>;
  comments: Map<string, string>;
  index: SchemaIndex;
};

// ---------------------------------------------------------------------------

function preferredLabel(store: Store, subj: Term, fallback: string): string {
  const candidates = store.getObjects(subj, namedNode(RDFS + 'label'), null)
    .concat(store.getObjects(subj, namedNode(SKOS + 'prefLabel'), null));
  let best = '';
  let bestLang: string | null = null;
  for (const o of candidates) {
    if (o.termType !== 'Literal') continue;
    const lang = (o as any).language || '';
    // UI language first, then English, then untagged — see langRank().
    if (bestLang === null || isBetterLang(lang, bestLang)) {
      best = o.value;
      bestLang = lang;
    }
  }
  return best || fallback;
}

function readList(store: Store, head: Term | null | undefined): Term[] {
  const out: Term[] = [];
  let cur: Term | null = head ?? null;
  const NIL = RDF + 'nil';
  const seen = new Set<string>();
  while (cur && cur.value !== NIL) {
    const key = `${cur.termType}|${cur.value}`;
    if (seen.has(key)) break;
    seen.add(key);
    const first = store.getObjects(cur as any, namedNode(RDF + 'first'), null)[0];
    const rest  = store.getObjects(cur as any, namedNode(RDF + 'rest'), null)[0];
    if (first) out.push(first);
    cur = rest || null;
  }
  return out;
}

function termToClassExpr(store: Store, t: Term, depth = 0): ClassExpr {
  if (depth > 8) return { type: 'unknown', bnode: t.value };
  if (t.termType === 'NamedNode') {
    return { type: 'named', iri: t.value };
  }
  if (t.termType !== 'BlankNode') return { type: 'unknown' };

  const types = store.getObjects(t as any, namedNode(TYPE), null).map(x => x.value);

  if (types.includes(OWL + 'Restriction')) {
    const r = parseRestriction(store, t, depth);
    if (r) return { type: 'restriction', restriction: r };
  }

  const u = store.getObjects(t as any, namedNode(OWL + 'unionOf'), null)[0];
  if (u) {
    return { type: 'union', parts: readList(store, u).map(x => termToClassExpr(store, x, depth + 1)) };
  }
  const i = store.getObjects(t as any, namedNode(OWL + 'intersectionOf'), null)[0];
  if (i) {
    return { type: 'intersection', parts: readList(store, i).map(x => termToClassExpr(store, x, depth + 1)) };
  }
  const c = store.getObjects(t as any, namedNode(OWL + 'complementOf'), null)[0];
  if (c) {
    return { type: 'complement', of: termToClassExpr(store, c, depth + 1) };
  }
  const o = store.getObjects(t as any, namedNode(OWL + 'oneOf'), null)[0];
  if (o) {
    return { type: 'oneOf', members: readList(store, o).map(x => x.value) };
  }
  return { type: 'unknown', bnode: t.value };
}

function parseRestriction(store: Store, t: Term, depth = 0): Restriction | null {
  const onProp = store.getObjects(t as any, namedNode(OWL + 'onProperty'), null)[0];
  if (!onProp) return null;

  const get = (p: string) => store.getObjects(t as any, namedNode(OWL + p), null)[0];
  const num = (q?: Term) => (q && q.termType === 'Literal') ? parseInt(q.value, 10) : undefined;

  const some = get('someValuesFrom');
  if (some) return { onProperty: onProp.value, kind: 'some', filler: termToClassExpr(store, some, depth + 1) };
  const all = get('allValuesFrom');
  if (all) return { onProperty: onProp.value, kind: 'all', filler: termToClassExpr(store, all, depth + 1) };
  const has = get('hasValue');
  if (has) {
    const f = has.termType === 'Literal'
      ? { literal: has.value, datatype: (has as any).datatype?.value, lang: (has as any).language }
      : has.termType === 'NamedNode'
        ? { iri: has.value }
        : termToClassExpr(store, has, depth + 1);
    return { onProperty: onProp.value, kind: 'value', filler: f as any };
  }

  const onClassTerm = get('onClass');
  const onClass = onClassTerm ? termToClassExpr(store, onClassTerm, depth + 1) : undefined;

  const qmin = get('minQualifiedCardinality');
  if (qmin) return { onProperty: onProp.value, kind: 'qmin', n: num(qmin), onClass };
  const qmax = get('maxQualifiedCardinality');
  if (qmax) return { onProperty: onProp.value, kind: 'qmax', n: num(qmax), onClass };
  const qexact = get('qualifiedCardinality');
  if (qexact) return { onProperty: onProp.value, kind: 'qexact', n: num(qexact), onClass };

  const min = get('minCardinality');
  if (min) return { onProperty: onProp.value, kind: 'min', n: num(min) };
  const max = get('maxCardinality');
  if (max) return { onProperty: onProp.value, kind: 'max', n: num(max) };
  const exact = get('cardinality');
  if (exact) return { onProperty: onProp.value, kind: 'exact', n: num(exact) };
  return null;
}

// SHACL property path (SHACL §2.3.1) as a PathExpr, or null when the node is
// not a path this reader understands.
function readPath(store: Store, t: Term, depth = 0): PathExpr | null {
  if (depth > 8) return null;
  if (t.termType === 'NamedNode') return { kind: 'predicate', iri: t.value };
  if (t.termType !== 'BlankNode') return null;
  const one = (p: string) => store.getObjects(t as any, namedNode(SH + p), null)[0];
  const unary = (p: string, kind: 'inverse' | 'zeroOrMore' | 'oneOrMore' | 'zeroOrOne'): PathExpr | null => {
    const o = one(p);
    if (!o) return null;
    const inner = readPath(store, o, depth + 1);
    return inner ? { kind, path: inner } : null;
  };
  const u = unary('inversePath', 'inverse') || unary('zeroOrMorePath', 'zeroOrMore')
    || unary('oneOrMorePath', 'oneOrMore') || unary('zeroOrOnePath', 'zeroOrOne');
  if (u) return u;
  const listOf = (head: Term | undefined): PathExpr[] | null => {
    if (!head) return null;
    const parts = readList(store, head).map(x => readPath(store, x, depth + 1));
    return parts.length && parts.every(Boolean) ? parts as PathExpr[] : null;
  };
  const alt = listOf(one('alternativePath'));
  if (alt) return { kind: 'alternative', paths: alt };
  // A sequence path is the RDF list itself.
  if (store.getObjects(t as any, namedNode(RDF + 'first'), null).length) {
    const seq = listOf(t);
    if (seq) return { kind: 'sequence', paths: seq };
  }
  return null;
}

// ---------------------------------------------------------------------------

export async function buildSchemaModel(graphs: string[]): Promise<SchemaModel> {
  const { store } = await loadOntologyGraph(graphs);
  return extractSchema(store);
}

export function extractSchema(store: Store): SchemaModel {
  const labels = new Map<string, string>();
  const comments = new Map<string, string>();
  // Language tag of whatever is currently in `labels` / `comments`, so a later
  // literal can be compared against it without changing those maps' shape.
  const labelLang = new Map<string, string>();
  const commentLang = new Map<string, string>();
  const classes = new Map<string, ClassEntry>();
  const properties = new Map<string, PropertyEntry>();
  const concepts = new Map<string, ConceptEntry>();
  const schemes = new Map<string, SchemeEntry>();
  const collections = new Map<string, CollectionEntry>();
  const shapes = new Map<string, ShapeEntry>();
  const imports: string[] = [];
  // Subjects typed rdfs:Class / owl:Class. `classes` also holds IRIs that only
  // appear as a superclass, which must not count for SHACL's implicit target.
  const typedClasses = new Set<string>();

  const ensureClass = (iri: string): ClassEntry => {
    let c = classes.get(iri);
    if (!c) {
      c = {
        iri, label: '', comment: '',
        parents: [], children: [],
        equivalents: [], disjoints: [],
        restrictions: [],
        instanceCount: 0, instances: [], hasShape: false,
      };
      classes.set(iri, c);
    }
    return c;
  };
  const ensureProp = (iri: string): PropertyEntry => {
    let p = properties.get(iri);
    if (!p) {
      p = {
        iri, kind: 'property', label: '', comment: '',
        domain: [], range: [], domainUnion: [], rangeUnion: [],
        superProperties: [], equivalentProperty: [], inverseOf: [], chains: [],
        characteristics: new Set(),
      };
      properties.set(iri, p);
    }
    return p;
  };
  const ensureConcept = (iri: string): ConceptEntry => {
    let c = concepts.get(iri);
    if (!c) {
      c = {
        iri, prefLabel: '',
        altLabels: [], hiddenLabels: [], notation: [],
        scheme: [], topConceptOf: [],
        broader: [], narrower: [], broaderTransitive: [], related: [],
      };
      concepts.set(iri, c);
    }
    return c;
  };
  const ensureScheme = (iri: string): SchemeEntry => {
    let s = schemes.get(iri);
    if (!s) { s = { iri, label: '', topConcepts: [] }; schemes.set(iri, s); }
    return s;
  };
  const ensureShape = (iri: string): ShapeEntry => {
    let s = shapes.get(iri);
    if (!s) { s = { iri, label: '', targetClass: [], targetNode: [], implicitTarget: false, properties: [] }; shapes.set(iri, s); }
    return s;
  };
  const addUnique = (arr: string[], v: string) => { if (v && !arr.includes(v)) arr.push(v); };

  // ---- pass 1: labels, types, simple predicates -----------------------------
  for (const q of store.getQuads(null, null, null, null)) {
    const s = q.subject.value;
    const p = q.predicate.value;
    const o = q.object;

    if (q.subject.termType !== 'NamedNode') continue;

    if (p === RDFS + 'label' && o.termType === 'Literal') {
      const lang = (o as any).language || '';
      if (!labels.has(s) || isBetterLang(lang, labelLang.get(s))) {
        labels.set(s, o.value);
        labelLang.set(s, lang);
      }
    } else if (p === RDFS + 'comment' && o.termType === 'Literal') {
      const lang = (o as any).language || '';
      if (!comments.has(s) || isBetterLang(lang, commentLang.get(s))) {
        comments.set(s, o.value);
        commentLang.set(s, lang);
      }
    } else if (p === TYPE) {
      const t = o.value;
      if (t === RDFS + 'Class' || t === OWL + 'Class') { ensureClass(s); typedClasses.add(s); }
      else if (t === RDF + 'Property') { ensureProp(s); }
      else if (t === OWL + 'ObjectProperty') { ensureProp(s).kind = 'object'; }
      else if (t === OWL + 'DatatypeProperty') { ensureProp(s).kind = 'datatype'; }
      else if (t === OWL + 'AnnotationProperty') { const e = ensureProp(s); if (e.kind === 'property') e.kind = 'annotation'; }
      else if (t === SKOS + 'Concept') ensureConcept(s);
      else if (t === SKOS + 'ConceptScheme') ensureScheme(s);
      else if (t === SKOS + 'Collection' || t === SKOS + 'OrderedCollection') {
        if (!collections.has(s)) collections.set(s, { iri: s, members: [], ordered: t === SKOS + 'OrderedCollection' });
      }
      else if (t === SH + 'NodeShape') ensureShape(s);
      else if (CHAR_BY_TYPE[t]) {
        ensureProp(s).characteristics.add(CHAR_BY_TYPE[t]);
      }
    } else if (p === OWL + 'imports' && o.termType === 'NamedNode') {
      addUnique(imports, o.value);
    }
  }

  // ---- pass 2: class/property axioms (incl. blank-node walks) ---------------
  // subClassOf
  for (const q of store.getQuads(null, namedNode(RDFS + 'subClassOf'), null, null)) {
    if (q.subject.termType !== 'NamedNode') continue;
    const c = ensureClass(q.subject.value);
    if (q.object.termType === 'NamedNode') {
      addUnique(c.parents, q.object.value);
      addUnique(ensureClass(q.object.value).children, c.iri);
    } else {
      // bnode parent — could be a Restriction or a class expression
      const types = store.getObjects(q.object as any, namedNode(TYPE), null).map(x => x.value);
      if (types.includes(OWL + 'Restriction')) {
        const r = parseRestriction(store, q.object);
        if (r) c.restrictions.push(r);
      }
      // also attach as a "ghost" axiom via equivalents-style decomposition? No —
      // anonymous-superclass connectives are uncommon; render through restriction.
    }
  }

  // equivalentClass (LHS = NamedNode subject)
  for (const q of store.getQuads(null, namedNode(OWL + 'equivalentClass'), null, null)) {
    if (q.subject.termType !== 'NamedNode') continue;
    const c = ensureClass(q.subject.value);
    if (q.object.termType === 'BlankNode') {
      const types = store.getObjects(q.object as any, namedNode(TYPE), null).map(x => x.value);
      if (types.includes(OWL + 'Restriction')) {
        const r = parseRestriction(store, q.object);
        if (r) c.restrictions.push(r);
      }
      c.equivalents.push(termToClassExpr(store, q.object));
    } else {
      c.equivalents.push({ type: 'named', iri: q.object.value });
    }
  }

  // unionOf / intersectionOf / complementOf / oneOf — when subject is a NamedNode owl:Class
  for (const pred of ['unionOf', 'intersectionOf'] as const) {
    for (const q of store.getQuads(null, namedNode(OWL + pred), null, null)) {
      if (q.subject.termType !== 'NamedNode') continue;
      const parts = readList(store, q.object).map(t => termToClassExpr(store, t));
      const e = ensureClass(q.subject.value);
      if (pred === 'unionOf') e.unionOf = parts;
      else e.intersectionOf = parts;
    }
  }
  for (const q of store.getQuads(null, namedNode(OWL + 'complementOf'), null, null)) {
    if (q.subject.termType !== 'NamedNode') continue;
    ensureClass(q.subject.value).complementOf = termToClassExpr(store, q.object);
  }
  for (const q of store.getQuads(null, namedNode(OWL + 'oneOf'), null, null)) {
    if (q.subject.termType !== 'NamedNode') continue;
    ensureClass(q.subject.value).oneOf = readList(store, q.object).map(t => t.value);
  }

  // disjointWith
  for (const q of store.getQuads(null, namedNode(OWL + 'disjointWith'), null, null)) {
    if (q.subject.termType !== 'NamedNode') continue;
    if (q.object.termType !== 'NamedNode') continue;
    addUnique(ensureClass(q.subject.value).disjoints, q.object.value);
    addUnique(ensureClass(q.object.value).disjoints, q.subject.value);
  }
  // AllDisjointClasses
  for (const q of store.getQuads(null, namedNode(TYPE), namedNode(OWL + 'AllDisjointClasses'), null)) {
    const members = store.getObjects(q.subject as any, namedNode(OWL + 'members'), null)[0];
    if (!members) continue;
    const list = readList(store, members).map(t => t.value).filter(Boolean);
    for (const a of list) for (const b of list) {
      if (a !== b) addUnique(ensureClass(a).disjoints, b);
    }
  }

  // hasKey
  for (const q of store.getQuads(null, namedNode(OWL + 'hasKey'), null, null)) {
    if (q.subject.termType !== 'NamedNode') continue;
    const keys = readList(store, q.object).map(t => t.value).filter(Boolean);
    const e = ensureClass(q.subject.value);
    if (!e.hasKey) e.hasKey = [];
    e.hasKey.push(keys);
  }

  // domain / range. A blank-node domain is usually `[ owl:unionOf (A B) ]`;
  // its named members are kept so A and B both list the property.
  const unionMembers = (t: Term): string[] => {
    const u = store.getObjects(t as any, namedNode(OWL + 'unionOf'), null)[0];
    return u ? readList(store, u).filter(x => x.termType === 'NamedNode').map(x => x.value) : [];
  };
  for (const q of store.getQuads(null, namedNode(RDFS + 'domain'), null, null)) {
    if (q.subject.termType !== 'NamedNode') continue;
    if (q.object.termType === 'NamedNode') addUnique(ensureProp(q.subject.value).domain, q.object.value);
    else if (q.object.termType === 'BlankNode') {
      for (const m of unionMembers(q.object)) addUnique(ensureProp(q.subject.value).domainUnion, m);
    }
  }
  for (const q of store.getQuads(null, namedNode(RDFS + 'range'), null, null)) {
    if (q.subject.termType !== 'NamedNode') continue;
    if (q.object.termType === 'NamedNode') addUnique(ensureProp(q.subject.value).range, q.object.value);
    else if (q.object.termType === 'BlankNode') {
      for (const m of unionMembers(q.object)) addUnique(ensureProp(q.subject.value).rangeUnion, m);
    }
  }
  // subPropertyOf, equivalentProperty, inverseOf
  for (const q of store.getQuads(null, namedNode(RDFS + 'subPropertyOf'), null, null)) {
    if (q.subject.termType !== 'NamedNode' || q.object.termType !== 'NamedNode') continue;
    addUnique(ensureProp(q.subject.value).superProperties, q.object.value);
  }
  for (const q of store.getQuads(null, namedNode(OWL + 'equivalentProperty'), null, null)) {
    if (q.subject.termType !== 'NamedNode' || q.object.termType !== 'NamedNode') continue;
    addUnique(ensureProp(q.subject.value).equivalentProperty, q.object.value);
    addUnique(ensureProp(q.object.value).equivalentProperty, q.subject.value);
  }
  for (const q of store.getQuads(null, namedNode(OWL + 'inverseOf'), null, null)) {
    if (q.subject.termType !== 'NamedNode' || q.object.termType !== 'NamedNode') continue;
    addUnique(ensureProp(q.subject.value).inverseOf, q.object.value);
    addUnique(ensureProp(q.object.value).inverseOf, q.subject.value);
  }
  // propertyChainAxiom
  for (const q of store.getQuads(null, namedNode(OWL + 'propertyChainAxiom'), null, null)) {
    if (q.subject.termType !== 'NamedNode') continue;
    const chain = readList(store, q.object).map(t => t.value).filter(Boolean);
    if (chain.length) ensureProp(q.subject.value).chains.push(chain);
  }

  // ---- SHACL ---------------------------------------------------------------
  for (const q of store.getQuads(null, namedNode(SH + 'targetClass'), null, null)) {
    if (q.subject.termType !== 'NamedNode' || q.object.termType !== 'NamedNode') continue;
    addUnique(ensureShape(q.subject.value).targetClass, q.object.value);
    const c = classes.get(q.object.value); if (c) c.hasShape = true;
  }
  for (const q of store.getQuads(null, namedNode(SH + 'targetNode'), null, null)) {
    if (q.subject.termType !== 'NamedNode') continue;
    addUnique(ensureShape(q.subject.value).targetNode, q.object.value);
  }
  for (const q of store.getQuads(null, namedNode(SH + 'property'), null, null)) {
    if (q.subject.termType !== 'NamedNode') continue;
    const sh = ensureShape(q.subject.value);
    const pShape = q.object;
    const pick = (name: string) => store.getObjects(pShape as any, namedNode(SH + name), null)[0];
    const path = pick('path'); if (!path) continue;
    const entry: ShapePropertyEntry = {
      path: path.value,
      name: pick('name')?.value,
      minCount: pick('minCount')?.value,
      maxCount: pick('maxCount')?.value,
      datatype: pick('datatype')?.value,
      cls: pick('class')?.value,
      nodeKind: pick('nodeKind')?.value,
      pattern: pick('pattern')?.value,
    };
    if (path.termType !== 'NamedNode') {
      const expr = readPath(store, path);
      if (expr) entry.pathExpr = expr;
    }
    const inList = pick('in');
    if (inList) entry.inCount = readList(store, inList).length;
    sh.properties.push(entry);
  }
  // SHACL §2.1.3.3: a shape that is also an rdfs:Class targets that class.
  for (const sh of shapes.values()) {
    if (!typedClasses.has(sh.iri)) continue;
    sh.implicitTarget = true;
    const c = classes.get(sh.iri); if (c) c.hasShape = true;
  }

  // ---- SKOS ----------------------------------------------------------------
  const skosLit = (s: string, p: string): string[] => {
    return store.getObjects(namedNode(s), namedNode(SKOS + p), null)
      .filter(o => o.termType === 'Literal').map(o => o.value);
  };
  const skosObj = (s: string, p: string): string[] => {
    return store.getObjects(namedNode(s), namedNode(SKOS + p), null)
      .filter(o => o.termType === 'NamedNode').map(o => o.value);
  };

  for (const c of concepts.values()) {
    c.prefLabel = skosLit(c.iri, 'prefLabel')[0] || labels.get(c.iri) || '';
    c.altLabels = skosLit(c.iri, 'altLabel');
    c.hiddenLabels = skosLit(c.iri, 'hiddenLabel');
    c.notation = skosLit(c.iri, 'notation');
    c.scheme = skosObj(c.iri, 'inScheme');
    c.topConceptOf = skosObj(c.iri, 'topConceptOf');
    c.broader = skosObj(c.iri, 'broader');
    c.narrower = skosObj(c.iri, 'narrower');
    c.broaderTransitive = skosObj(c.iri, 'broaderTransitive');
    c.related = skosObj(c.iri, 'related');
  }
  // Synthesize narrower from broader inverse if missing
  for (const c of concepts.values()) {
    for (const b of c.broader) {
      const parent = concepts.get(b);
      if (parent && !parent.narrower.includes(c.iri)) parent.narrower.push(c.iri);
    }
  }
  // Schemes: collect topConcepts
  for (const sch of schemes.values()) {
    sch.label = preferredLabel(store, namedNode(sch.iri), '');
    const topVia = store.getObjects(namedNode(sch.iri), namedNode(SKOS + 'hasTopConcept'), null)
      .filter(o => o.termType === 'NamedNode').map(o => o.value);
    sch.topConcepts = topVia;
    for (const c of concepts.values()) {
      if (c.topConceptOf.includes(sch.iri) && !sch.topConcepts.includes(c.iri)) {
        sch.topConcepts.push(c.iri);
      }
    }
  }
  // Collections
  for (const col of collections.values()) {
    const members = store.getObjects(namedNode(col.iri), namedNode(SKOS + 'member'), null)
      .filter(o => o.termType === 'NamedNode').map(o => o.value);
    if (col.ordered) {
      const list = store.getObjects(namedNode(col.iri), namedNode(SKOS + 'memberList'), null)[0];
      if (list) col.members = readList(store, list).map(t => t.value);
      else col.members = members;
    } else {
      col.members = members;
    }
  }

  // ---- finalize labels/comments + instance counts --------------------------
  for (const c of classes.values()) {
    c.label = labels.get(c.iri) || '';
    c.comment = comments.get(c.iri) || '';
  }
  for (const p of properties.values()) {
    p.label = labels.get(p.iri) || '';
    p.comment = comments.get(p.iri) || '';
  }
  for (const sh of shapes.values()) sh.label = labels.get(sh.iri) || '';

  const instanceSets = new Map<ClassEntry, Set<string>>();
  for (const q of store.getQuads(null, namedNode(TYPE), null, null)) {
    const c = classes.get(q.object.value);
    if (!c) continue;
    c.instanceCount++;
    if (q.subject.termType !== 'NamedNode') continue;
    let set = instanceSets.get(c);
    if (!set) { set = new Set(); instanceSets.set(c, set); }
    set.add(q.subject.value);
  }
  for (const [c, set] of instanceSets) c.instances = [...set];

  // ---- per-class lookup tables (one pass over properties and shapes) -------
  const push = <V>(m: Map<string, V[]>, k: string, v: V) => {
    const arr = m.get(k);
    if (arr) arr.push(v); else m.set(k, [v]);
  };
  const index: SchemaIndex = {
    propsByDomain: new Map(), propsByRange: new Map(),
    shapesByClass: new Map(), shapePropsByValueClass: new Map(),
  };
  for (const p of properties.values()) {
    for (const d of p.domain) push(index.propsByDomain, d, p.iri);
    for (const d of p.domainUnion) if (!p.domain.includes(d)) push(index.propsByDomain, d, p.iri);
    for (const r of p.range) push(index.propsByRange, r, p.iri);
    for (const r of p.rangeUnion) if (!p.range.includes(r)) push(index.propsByRange, r, p.iri);
  }
  for (const sh of shapes.values()) {
    for (const tc of sh.targetClass) push(index.shapesByClass, tc, sh.iri);
    if (sh.implicitTarget && !sh.targetClass.includes(sh.iri)) push(index.shapesByClass, sh.iri, sh.iri);
    for (const pp of sh.properties) {
      if (pp.cls && !pp.pathExpr) push(index.shapePropsByValueClass, pp.cls, { shape: sh.iri, prop: pp });
    }
  }

  // ---- namespace summary ---------------------------------------------------
  const nsCount = new Map<string, number>();
  const seenIri = new Set<string>();
  const recordIri = (iri: string) => {
    if (!iri || seenIri.has(iri)) return;
    seenIri.add(iri);
    const { ns } = splitIri(iri);
    if (!ns) return;
    nsCount.set(ns, (nsCount.get(ns) ?? 0) + 1);
  };
  for (const iri of classes.keys()) recordIri(iri);
  for (const iri of properties.keys()) recordIri(iri);
  for (const iri of concepts.keys()) recordIri(iri);
  for (const iri of schemes.keys()) recordIri(iri);
  for (const iri of shapes.keys()) recordIri(iri);

  const importedNs = new Set(imports.map(i => splitIri(i).ns).filter(Boolean));

  const namespaces = new Map<string, NamespaceEntry>();
  for (const [ns, count] of nsCount.entries()) {
    namespaces.set(ns, {
      ns,
      prefix: prefixForNamespace(ns),
      count,
      isImported: importedNs.has(ns),
      kind: kindOf(ns + 'x'),
    });
  }

  return {
    classes, properties, concepts, schemes, collections,
    namespaces, shapes, imports, labels, comments, index,
  };
}
