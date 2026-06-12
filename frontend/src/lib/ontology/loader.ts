// Ontology loader: fetches the ontology graph(s) as Turtle + parses with n3.
// Extracts classes, properties, SHACL shapes and prefixes for use by the
// viewer, SPARQL editor completion, validators, etc.
//
// Strategy:
//   1. CONSTRUCT { ?s ?p ?o } WHERE { GRAPH <g> { ?s ?p ?o } } — one shot,
//      ask for text/turtle so we preserve prefixes and can feed SHACL.
//   2. If the server rejects CONSTRUCT or returns non-Turtle, fall back to
//      a paginated SELECT ?s ?p ?o and reconstruct Turtle client-side.

import { Parser, Store, Writer, DataFactory } from 'n3';
import type { Quad_Subject, Quad_Predicate, Quad_Object } from 'n3';
import { fetchRetry429 } from '../api';

const RDF = 'http://www.w3.org/1999/02/22-rdf-syntax-ns#';
const RDFS = 'http://www.w3.org/2000/01/rdf-schema#';
const OWL = 'http://www.w3.org/2002/07/owl#';
const SH = 'http://www.w3.org/ns/shacl#';
const SKOS = 'http://www.w3.org/2004/02/skos/core#';

const { namedNode } = DataFactory;

/**
 * Parse a Turtle string into an N3 store and prefix map.
 */
export async function parseTurtle(turtle: string): Promise<{ store: Store; prefixes: Record<string, string> }> {
  const prefixes: Record<string, string> = {};
  const store = new Store();
  const parser = new Parser();
  await new Promise<void>((resolve, reject) => {
    parser.parse(turtle, (err, quad, pfx) => {
      if (err) return reject(err);
      if (quad) store.addQuad(quad);
      else {
        Object.assign(prefixes, pfx || {});
        resolve();
      }
    });
  });
  return { store, prefixes };
}

/**
 * @param graphs  named graph IRIs to merge
 * @param sparqlUrl  e.g. '/api/sparql'
 */
export async function loadOntologyGraph(graphs: string[], sparqlUrl = '/sparql'): Promise<{ turtle: string; store: Store; prefixes: Record<string, string> }> {
  const scope = (graphs || []).filter(Boolean);
  const graphPattern = scope.length === 0
    ? '?s ?p ?o'
    : scope.length === 1
      ? `GRAPH <${scope[0]}> { ?s ?p ?o }`
      : scope.map(g => `{ GRAPH <${g}> { ?s ?p ?o } }`).join(' UNION ');

  const construct = `CONSTRUCT { ?s ?p ?o } WHERE { ${graphPattern} }`;
  let turtle = '';
  try {
    turtle = await postSparql(sparqlUrl, construct, 'text/turtle');
  } catch {
    turtle = await selectToTurtle(scope, sparqlUrl);
  }

  const prefixes = {};
  const store = new Store();
  const parser = new Parser();
  await new Promise<void>((resolve, reject) => {
    parser.parse(turtle, (err, quad, pfx) => {
      if (err) return reject(err);
      if (quad) store.addQuad(quad);
      else {
        Object.assign(prefixes, pfx || {});
        resolve();
      }
    });
  });

  return { turtle, store, prefixes };
}

async function postSparql(url: string, query: string, accept: string): Promise<string> {
  // /sparql is rate-limited per IP — retry 429s so an ontology load racing other
  // page queries doesn't fail with "Too Many Requests".
  const res = await fetchRetry429(url, {
    method: 'POST',
    credentials: 'include',
    headers: {
      'Content-Type': 'application/sparql-query',
      'Accept': accept,
    },
    body: query,
  });
  if (!res.ok) throw new Error(`SPARQL ${res.status}: ${await res.text().catch(() => '')}`);
  const text = await res.text();
  const ct = (res.headers.get('content-type') || '').toLowerCase();
  if (accept.includes('turtle') && !ct.includes('turtle') && !ct.includes('n-triples')) {
    throw new Error(`Server returned ${ct}, expected Turtle`);
  }
  return text;
}

async function selectToTurtle(graphs: string[], sparqlUrl: string): Promise<string> {
  const graphPattern = graphs.length === 0
    ? '?s ?p ?o'
    : graphs.length === 1
      ? `GRAPH <${graphs[0]}> { ?s ?p ?o }`
      : graphs.map(g => `{ GRAPH <${g}> { ?s ?p ?o } }`).join(' UNION ');
  const pageSize = 50000;
  const writer = new Writer({ format: 'N-Triples' });
  const quads = [];
  let offset = 0;
  while (true) {
    const q = `SELECT ?s ?p ?o WHERE { ${graphPattern} } LIMIT ${pageSize} OFFSET ${offset}`;
    const json = JSON.parse(await postSparql(sparqlUrl, q, 'application/sparql-results+json'));
    const rows = json?.results?.bindings || [];
    if (!rows.length) break;
    for (const r of rows) {
      const s = termFromBinding(r.s);
      const p = termFromBinding(r.p);
      const o = termFromBinding(r.o);
      // SPARQL result rows guarantee position validity (s=subject, p=predicate,
      // o=object); narrow the broad term union back to the quad positions.
      if (s && p && o) {
        quads.push(DataFactory.quad(s as Quad_Subject, p as Quad_Predicate, o as Quad_Object));
      }
    }
    if (rows.length < pageSize) break;
    offset += pageSize;
  }
  writer.addQuads(quads);
  return await new Promise((resolve, reject) => {
    writer.end((err, result) => err ? reject(err) : resolve(result));
  });
}

interface SparqlBinding {
  type: string;
  value: string;
  datatype?: string;
  'xml:lang'?: string;
}

function termFromBinding(b: SparqlBinding | undefined): Quad_Subject | Quad_Predicate | Quad_Object | null {
  if (!b) return null;
  if (b.type === 'uri') return DataFactory.namedNode(b.value);
  if (b.type === 'bnode') return DataFactory.blankNode(b.value);
  if (b.type === 'literal' || b.type === 'typed-literal') {
    const dt = b.datatype ? DataFactory.namedNode(b.datatype) : undefined;
    return DataFactory.literal(b.value, b['xml:lang'] || dt);
  }
  return null;
}

// ---------------------------------------------------------------------------
// Model extraction
// ---------------------------------------------------------------------------

/** @returns {{classes: any[], properties: any[], shapes: any[], labels: Map<string, any>, comments: Map<string, string>}} */
export function extractOntologyModel(store: Store) {
  const labels = new Map();
  const comments = new Map();
  const classes = new Map();
  const properties = new Map();
  const shapes = new Map();

  const ensureClass = (iri) => {
    if (!classes.has(iri)) classes.set(iri, { iri, parents: new Set(), children: new Set(), instanceCount: 0 });
    return classes.get(iri);
  };
  const ensureProp = (iri) => {
    if (!properties.has(iri)) properties.set(iri, { iri, kind: '', domain: new Set(), range: new Set() });
    return properties.get(iri);
  };
  const ensureShape = (iri) => {
    if (!shapes.has(iri)) shapes.set(iri, { iri, targetClass: new Set(), targetNode: new Set(), properties: [] });
    return shapes.get(iri);
  };

  // Named subjects explicitly typed sh:PropertyShape; used to index orphans
  // (property shapes never referenced from a node shape via sh:property).
  const declaredPropertyShapes = new Set<string>();

  for (const q of store.getQuads(null, null, null, null)) {
    const s = q.subject.value, p = q.predicate.value, o = q.object;
    if (p === RDFS + 'label' && o.termType === 'Literal') {
      const cur = labels.get(s);
      if (!cur || (o.language === 'en' && cur.language !== 'en')) {
        labels.set(s, { value: o.value, language: o.language || '' });
      }
    } else if (p === RDFS + 'comment' && o.termType === 'Literal') {
      if (!comments.has(s)) comments.set(s, o.value);
    } else if (p === RDF + 'type') {
      const t = o.value;
      if (t === RDFS + 'Class' || t === OWL + 'Class') ensureClass(s);
      else if (t === RDF + 'Property') { ensureProp(s).kind ||= 'property'; }
      else if (t === OWL + 'ObjectProperty') { ensureProp(s).kind = 'object'; }
      else if (t === OWL + 'DatatypeProperty') { ensureProp(s).kind = 'datatype'; }
      else if (t === OWL + 'AnnotationProperty') { ensureProp(s).kind ||= 'annotation'; }
      else if (t === SH + 'NodeShape') ensureShape(s);
      else if (t === SH + 'PropertyShape') {
        // Attached ones are handled via sh:property below; remember named
        // declarations so orphans can be indexed afterwards.
        if (q.subject.termType === 'NamedNode') declaredPropertyShapes.add(s);
      }
    } else if (p === RDFS + 'subClassOf' && o.termType === 'NamedNode') {
      ensureClass(s).parents.add(o.value);
      ensureClass(o.value).children.add(s);
    } else if (p === RDFS + 'domain' && o.termType === 'NamedNode') {
      ensureProp(s).domain.add(o.value);
    } else if (p === RDFS + 'range' && o.termType === 'NamedNode') {
      ensureProp(s).range.add(o.value);
    } else if (p === SH + 'targetClass' && o.termType === 'NamedNode') {
      ensureShape(s).targetClass.add(o.value);
    } else if (p === SH + 'targetNode') {
      ensureShape(s).targetNode.add(o.value);
    }
  }

  // SHACL property shapes attached via sh:property
  const propertyEntryOf = (pShape) => {
    const pick = (pred) => store.getObjects(pShape, namedNode(SH + pred), null)[0];
    return {
      path: pick('path')?.value || '',
      name: pick('name')?.value || '',
      description: pick('description')?.value || '',
      minCount: pick('minCount')?.value,
      maxCount: pick('maxCount')?.value,
      datatype: pick('datatype')?.value,
      class: pick('class')?.value,
      nodeKind: pick('nodeKind')?.value,
      pattern: pick('pattern')?.value,
      in: store.getObjects(pShape, namedNode(SH + 'in'), null).map(t => t.value),
      severity: pick('severity')?.value,
    };
  };
  const attachedPropertyShapes = new Set<string>();
  for (const q of store.getQuads(null, namedNode(SH + 'property'), null, null)) {
    const shape = ensureShape(q.subject.value);
    attachedPropertyShapes.add(q.object.value);
    const entry = propertyEntryOf(q.object);
    if (entry.path) shape.properties.push(entry);
  }

  // Orphan property shapes (declared but never attached) get their own shape
  // entry so they still surface in the ontology model.
  for (const iri of declaredPropertyShapes) {
    if (attachedPropertyShapes.has(iri)) continue;
    const shape = ensureShape(iri);
    const entry = propertyEntryOf(namedNode(iri));
    if (entry.path) shape.properties.push(entry);
  }

  // Instance counts (heuristic, cheap)
  for (const q of store.getQuads(null, namedNode(RDF + 'type'), null, null)) {
    const t = q.object.value;
    if (classes.has(t)) classes.get(t).instanceCount++;
  }

  const toArray = (m, fn) => Array.from(m.values()).map(fn);
  return {
    labels,
    comments,
    classes: toArray(classes, c => ({
      iri: c.iri,
      label: labels.get(c.iri)?.value || '',
      comment: comments.get(c.iri) || '',
      parents: [...c.parents],
      children: [...c.children],
      instanceCount: c.instanceCount,
    })),
    properties: toArray(properties, p => ({
      iri: p.iri,
      kind: p.kind || 'property',
      label: labels.get(p.iri)?.value || '',
      comment: comments.get(p.iri) || '',
      domain: [...p.domain],
      range: [...p.range],
    })),
    shapes: toArray(shapes, s => ({
      iri: s.iri,
      label: labels.get(s.iri)?.value || '',
      targetClass: [...s.targetClass],
      targetNode: [...s.targetNode],
      properties: s.properties,
    })),
  };
}

/** Build a parent→children index for hierarchy rendering, returning roots. */
export function buildHierarchy(classes: Array<{ iri: string; parents: string[] }>) {
  const byIri = new Map(classes.map(c => [c.iri, c]));
  const roots = classes.filter(c =>
    c.parents.length === 0 || !c.parents.some(p => byIri.has(p))
  );
  return { byIri, roots };
}

export const ONTOLOGY_PREFIXES = { rdf: RDF, rdfs: RDFS, owl: OWL, sh: SH, skos: SKOS };
