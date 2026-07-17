// Semantic (modeling) validator — a focused port of
// the companion graph viewer's semantic validator. Operates on an n3 Store.
// Each check returns SemanticIssue[]; validateSemantics() runs all of them.
//
// SemanticIssue = { code, severity, focus, message, predicate?, object? }
//   severity ∈ 'error' | 'warning' | 'info'

import { DataFactory } from 'n3';
import type { Store } from 'n3';

const RDF = 'http://www.w3.org/1999/02/22-rdf-syntax-ns#';
const RDFS = 'http://www.w3.org/2000/01/rdf-schema#';
const OWL = 'http://www.w3.org/2002/07/owl#';
const SH = 'http://www.w3.org/ns/shacl#';
const XSD = 'http://www.w3.org/2001/XMLSchema#';
const { namedNode } = DataFactory;

export interface SemanticIssue {
  code: string;
  severity: 'error' | 'warning' | 'info';
  focus: string;
  message: string;
  predicate?: string;
  object?: string;
}

export function validateSemantics(store: Store): SemanticIssue[] {
  const types = indexTypes(store);
  const issues = [];
  issues.push(...checkSubClassCycles(store));
  issues.push(...checkDomainRangeUnknownClass(store, types));
  issues.push(...checkPropertyKindConflict(store, types));
  issues.push(...checkShaclPathUnknown(store, types));
  issues.push(...checkShaclTargetClassUnknown(store, types));
  issues.push(...checkShaclMinMaxCount(store));
  issues.push(...checkShaclDatatypeVsClassConflict(store));
  issues.push(...checkShaclPathOnNodeShape(store));
  issues.push(...checkShaclTargetOnPropertyShape(store));
  issues.push(...checkShaclQualifiedMinMaxCount(store));
  issues.push(...checkShaclLogicListValues(store));
  issues.push(...checkShaclPathExpressions(store));
  issues.push(...checkShaclPropertyShapeWithoutPath(store));
  issues.push(...checkMissingLabels(store, types));
  issues.push(...checkBareNamespaceIri(store));
  issues.push(...checkOrphanClass(store, types));
  issues.push(...checkLiteralWhereIriExpected(store, types));
  return issues.sort(severityRank);
}

function severityRank(a: SemanticIssue, b: SemanticIssue): number {
  const order = { error: 0, warning: 1, info: 2 };
  return (order[a.severity] ?? 3) - (order[b.severity] ?? 3);
}

function indexTypes(store: Store): Map<string, Set<string>> {
  const byIri = new Map();
  for (const q of store.getQuads(null, namedNode(RDF + 'type'), null, null)) {
    if (!byIri.has(q.subject.value)) byIri.set(q.subject.value, new Set());
    byIri.get(q.subject.value).add(q.object.value);
  }
  return byIri;
}

function isClassIri(iri: string, types: Map<string, Set<string>>): boolean {
  const t = types.get(iri);
  if (!t) return false;
  return t.has(RDFS + 'Class') || t.has(OWL + 'Class');
}

function isPropertyIri(iri: string, types: Map<string, Set<string>>): boolean {
  const t = types.get(iri);
  if (!t) return false;
  return t.has(RDF + 'Property') || t.has(OWL + 'ObjectProperty') ||
         t.has(OWL + 'DatatypeProperty') || t.has(OWL + 'AnnotationProperty');
}

function checkSubClassCycles(store: Store): SemanticIssue[] {
  const parents = new Map();
  for (const q of store.getQuads(null, namedNode(RDFS + 'subClassOf'), null, null)) {
    if (q.object.termType !== 'NamedNode') continue;
    if (!parents.has(q.subject.value)) parents.set(q.subject.value, new Set());
    parents.get(q.subject.value).add(q.object.value);
  }
  const out = [];
  const seen = new Set();
  for (const node of parents.keys()) {
    const path = [];
    if (dfsCycle(node, parents, path, new Set())) {
      const key = [...path].sort().join('→');
      if (seen.has(key)) continue;
      seen.add(key);
      out.push({
        code: 'subclass-cycle', severity: 'error', focus: node,
        message: `rdfs:subClassOf cycle: ${path.join(' → ')}`,
      });
    }
  }
  return out;
}

function dfsCycle(node: string, parents: Map<string, Set<string>>, path: string[], stack: Set<string>): boolean {
  if (stack.has(node)) {
    path.push(node);
    return true;
  }
  stack.add(node); path.push(node);
  for (const p of parents.get(node) || []) {
    if (dfsCycle(p, parents, path, stack)) return true;
  }
  stack.delete(node); path.pop();
  return false;
}

function checkDomainRangeUnknownClass(store: Store, types: Map<string, Set<string>>): SemanticIssue[] {
  const out = [];
  for (const pred of ['domain', 'range']) {
    for (const q of store.getQuads(null, namedNode(RDFS + pred), null, null)) {
      if (q.object.termType !== 'NamedNode') continue;
      const tgt = q.object.value;
      if (tgt.startsWith(XSD) || tgt === RDFS + 'Literal') continue;
      if (!isClassIri(tgt, types)) {
        out.push({
          code: `${pred}-unknown-class`, severity: 'warning', focus: q.subject.value,
          predicate: RDFS + pred, object: tgt,
          message: `rdfs:${pred} "${shorten(tgt)}" is not declared as rdfs:Class or owl:Class.`,
        });
      }
    }
  }
  return out;
}

function checkPropertyKindConflict(store: Store, types: Map<string, Set<string>>): SemanticIssue[] {
  const out = [];
  for (const [iri, ts] of types.entries()) {
    if (ts.has(OWL + 'ObjectProperty') && ts.has(OWL + 'DatatypeProperty')) {
      out.push({
        code: 'property-kind-conflict', severity: 'error', focus: iri,
        message: `Property is typed as both owl:ObjectProperty and owl:DatatypeProperty.`,
      });
    }
  }
  return out;
}

function checkShaclPathUnknown(store: Store, types: Map<string, Set<string>>): SemanticIssue[] {
  const out = [];
  for (const q of store.getQuads(null, namedNode(SH + 'path'), null, null)) {
    if (q.object.termType !== 'NamedNode') continue;
    if (!isPropertyIri(q.object.value, types)) {
      out.push({
        code: 'shacl-path-unknown', severity: 'warning', focus: q.subject.value,
        predicate: SH + 'path', object: q.object.value,
        message: `sh:path "${shorten(q.object.value)}" is not declared as a property.`,
      });
    }
  }
  return out;
}

function checkShaclTargetClassUnknown(store: Store, types: Map<string, Set<string>>): SemanticIssue[] {
  const out = [];
  for (const q of store.getQuads(null, namedNode(SH + 'targetClass'), null, null)) {
    if (q.object.termType !== 'NamedNode') continue;
    if (!isClassIri(q.object.value, types)) {
      out.push({
        code: 'shacl-targetclass-unknown', severity: 'warning', focus: q.subject.value,
        predicate: SH + 'targetClass', object: q.object.value,
        message: `sh:targetClass "${shorten(q.object.value)}" is not declared as a class.`,
      });
    }
  }
  return out;
}

function checkShaclMinMaxCount(store: Store): SemanticIssue[] {
  const out = [];
  // Property shapes are usually blank nodes, so key by the actual subject term —
  // re-wrapping its .value as a NamedNode would never match a BlankNode.
  const subjects = new Map<string, any>();
  for (const q of store.getQuads(null, namedNode(SH + 'minCount'), null, null)) {
    subjects.set(q.subject.value, q.subject);
  }
  for (const [val, term] of subjects) {
    const min = num(store.getObjects(term, namedNode(SH + 'minCount'), null)[0]);
    const max = num(store.getObjects(term, namedNode(SH + 'maxCount'), null)[0]);
    if (min != null && max != null && min > max) {
      out.push({
        code: 'shacl-min-gt-max', severity: 'error', focus: val,
        message: `sh:minCount (${min}) is greater than sh:maxCount (${max}).`,
      });
    }
  }
  return out;
}

function checkShaclDatatypeVsClassConflict(store: Store): SemanticIssue[] {
  const out = [];
  // See checkShaclMinMaxCount: property shapes are typically blank nodes, so we
  // must reuse the subject term rather than re-wrapping its value as a NamedNode.
  const subjects = new Map<string, any>();
  for (const q of store.getQuads(null, namedNode(SH + 'datatype'), null, null)) {
    subjects.set(q.subject.value, q.subject);
  }
  for (const [val, term] of subjects) {
    const hasClass = store.getObjects(term, namedNode(SH + 'class'), null).length > 0;
    if (hasClass) {
      out.push({
        code: 'shacl-datatype-and-class', severity: 'error', focus: val,
        message: `Property shape declares both sh:datatype and sh:class; they are mutually exclusive.`,
      });
    }
  }
  return out;
}

function checkShaclPathOnNodeShape(store: Store): SemanticIssue[] {
  const out = [];
  for (const q of store.getQuads(null, namedNode(RDF + 'type'), namedNode(SH + 'NodeShape'), null)) {
    if (store.getQuads(q.subject, namedNode(SH + 'path'), null, null).length > 0) {
      out.push({
        code: 'shacl-path-on-nodeshape', severity: 'error', focus: q.subject.value,
        predicate: SH + 'path',
        message: `Node shape declares sh:path; paths belong on property shapes (use sh:property).`,
      });
    }
  }
  return out;
}

/** Subjects that count as property shapes: typed so, or attached via sh:property. */
function propertyShapeTerms(store: Store): Map<string, any> {
  const subjects = new Map<string, any>();
  for (const q of store.getQuads(null, namedNode(RDF + 'type'), namedNode(SH + 'PropertyShape'), null)) {
    subjects.set(q.subject.value, q.subject);
  }
  for (const q of store.getQuads(null, namedNode(SH + 'property'), null, null)) {
    if (q.object.termType === 'NamedNode' || q.object.termType === 'BlankNode') {
      subjects.set(q.object.value, q.object);
    }
  }
  // A subject also typed sh:NodeShape is judged by the node-shape rules.
  for (const q of store.getQuads(null, namedNode(RDF + 'type'), namedNode(SH + 'NodeShape'), null)) {
    subjects.delete(q.subject.value);
  }
  return subjects;
}

function checkShaclTargetOnPropertyShape(store: Store): SemanticIssue[] {
  const out = [];
  for (const [val, term] of propertyShapeTerms(store)) {
    if (store.getQuads(term, namedNode(SH + 'targetClass'), null, null).length > 0) {
      out.push({
        code: 'shacl-target-on-propertyshape', severity: 'warning', focus: val,
        predicate: SH + 'targetClass',
        message: `Property shape declares sh:targetClass; targets are usually declared on the node shape.`,
      });
    }
  }
  return out;
}

function checkShaclQualifiedMinMaxCount(store: Store): SemanticIssue[] {
  const out = [];
  // Like checkShaclMinMaxCount: subjects are usually blank nodes, reuse terms.
  const subjects = new Map<string, any>();
  for (const q of store.getQuads(null, namedNode(SH + 'qualifiedMinCount'), null, null)) {
    subjects.set(q.subject.value, q.subject);
  }
  for (const [val, term] of subjects) {
    const min = num(store.getObjects(term, namedNode(SH + 'qualifiedMinCount'), null)[0]);
    const max = num(store.getObjects(term, namedNode(SH + 'qualifiedMaxCount'), null)[0]);
    if (min != null && max != null && min > max) {
      out.push({
        code: 'shacl-qualified-min-gt-max', severity: 'error', focus: val,
        message: `sh:qualifiedMinCount (${min}) is greater than sh:qualifiedMaxCount (${max}).`,
      });
    }
  }
  return out;
}

/** Walks an RDF list; returns null when well-formed, else a problem string. */
function listProblem(store: Store, head: { termType: string; value: string }): string | null {
  let node = head;
  const seen = new Set<string>();
  for (;;) {
    if (node.termType === 'NamedNode') {
      return node.value === RDF + 'nil' ? null : 'does not end in rdf:nil';
    }
    if (node.termType !== 'BlankNode') return 'is not an RDF list';
    if (seen.has(node.value)) return 'is a cyclic list';
    seen.add(node.value);
    const first = store.getObjects(node as any, namedNode(RDF + 'first'), null);
    const rest = store.getObjects(node as any, namedNode(RDF + 'rest'), null);
    if (first.length !== 1 || rest.length !== 1) return 'is a malformed list (rdf:first/rdf:rest)';
    node = rest[0];
  }
}

function checkShaclLogicListValues(store: Store): SemanticIssue[] {
  const out = [];
  for (const local of ['and', 'or', 'xone']) {
    for (const q of store.getQuads(null, namedNode(SH + local), null, null)) {
      const problem = listProblem(store, q.object);
      if (problem) {
        out.push({
          code: 'shacl-logic-not-list', severity: 'error', focus: q.subject.value,
          predicate: SH + local,
          message: `sh:${local} must be an RDF list of shapes; its value ${problem}.`,
        });
      }
    }
  }
  return out;
}

/** Returns null when the path term is a valid SHACL path, else a problem string. */
function pathProblem(store: Store, term: { termType: string; value: string }, seen: Set<string>): string | null {
  if (term.termType === 'Literal') return null; // flagged by literal-where-iri-expected
  if (term.termType === 'NamedNode') {
    return term.value === RDF + 'nil' ? 'is an empty sequence list' : null;
  }
  if (seen.has(term.value)) return 'is cyclic';
  seen.add(term.value);
  const preds = store.getQuads(term as any, null, null, null);
  const first = preds.filter(q => q.predicate.value === RDF + 'first');
  if (first.length > 0) {
    // sequence path: a list with at least two members
    const lp = listProblem(store, term);
    if (lp) return `has a sequence that ${lp}`;
    const items = [];
    let node: any = term;
    while (node.termType === 'BlankNode') {
      items.push(store.getObjects(node, namedNode(RDF + 'first'), null)[0]);
      node = store.getObjects(node, namedNode(RDF + 'rest'), null)[0];
    }
    if (items.length < 2) return 'has a sequence list with fewer than two members';
    for (const it of items) {
      const p = pathProblem(store, it, seen);
      if (p) return p;
    }
    return null;
  }
  const ops = [SH + 'inversePath', SH + 'alternativePath', SH + 'zeroOrMorePath', SH + 'oneOrMorePath', SH + 'zeroOrOnePath'];
  const opQuads = preds.filter(q => ops.includes(q.predicate.value));
  if (opQuads.length !== 1 || preds.length !== 1) return 'is not a recognised path expression';
  const [opQuad] = opQuads;
  if (opQuad.predicate.value === SH + 'alternativePath') {
    const lp = listProblem(store, opQuad.object);
    if (lp) return `has an sh:alternativePath that ${lp}`;
    let node: any = opQuad.object;
    let count = 0;
    while (node.termType === 'BlankNode') {
      const item = store.getObjects(node, namedNode(RDF + 'first'), null)[0];
      count++;
      const p = pathProblem(store, item, seen);
      if (p) return p;
      node = store.getObjects(node, namedNode(RDF + 'rest'), null)[0];
    }
    if (count < 2) return 'has an sh:alternativePath with fewer than two members';
    return null;
  }
  return pathProblem(store, opQuad.object, seen);
}

function checkShaclPathExpressions(store: Store): SemanticIssue[] {
  const out = [];
  for (const q of store.getQuads(null, namedNode(SH + 'path'), null, null)) {
    const problem = pathProblem(store, q.object, new Set());
    if (problem) {
      out.push({
        code: 'shacl-path-invalid', severity: 'error', focus: q.subject.value,
        predicate: SH + 'path',
        message: `sh:path ${problem}.`,
      });
    }
  }
  return out;
}

function checkShaclPropertyShapeWithoutPath(store: Store): SemanticIssue[] {
  const out = [];
  for (const [val, term] of propertyShapeTerms(store)) {
    if (store.getQuads(term, namedNode(SH + 'path'), null, null).length === 0) {
      out.push({
        code: 'shacl-propertyshape-no-path', severity: 'error', focus: val,
        message: `Property shape has no sh:path; its constraints cannot apply to anything.`,
      });
    }
  }
  return out;
}

function checkMissingLabels(store: Store, types: Map<string, Set<string>>): SemanticIssue[] {
  const out = [];
  const labelled = new Set(
    store.getQuads(null, namedNode(RDFS + 'label'), null, null).map(q => q.subject.value)
  );
  for (const [iri, ts] of types.entries()) {
    const isClass = ts.has(RDFS + 'Class') || ts.has(OWL + 'Class');
    const isProp = ts.has(RDF + 'Property') || ts.has(OWL + 'ObjectProperty') ||
                   ts.has(OWL + 'DatatypeProperty') || ts.has(OWL + 'AnnotationProperty');
    if ((isClass || isProp) && !labelled.has(iri) && !iri.startsWith(OWL) &&
        !iri.startsWith(RDFS) && !iri.startsWith(RDF)) {
      out.push({
        code: 'missing-label', severity: 'info', focus: iri,
        message: `${isClass ? 'Class' : 'Property'} "${shorten(iri)}" has no rdfs:label.`,
      });
    }
  }
  return out;
}

function checkBareNamespaceIri(store: Store): SemanticIssue[] {
  const out = [];
  const seen = new Set();
  for (const q of store.getQuads(null, null, null, null)) {
    for (const term of [q.subject, q.object]) {
      if (term.termType !== 'NamedNode') continue;
      const iri = term.value;
      if (seen.has(iri)) continue;
      if (iri.endsWith('/') || iri.endsWith('#')) {
        seen.add(iri);
        out.push({
          code: 'bare-namespace-iri', severity: 'warning', focus: iri,
          message: `IRI ends with "/" or "#" — looks like a namespace used as a term.`,
        });
      }
    }
  }
  return out;
}

function checkOrphanClass(store: Store, types: Map<string, Set<string>>): SemanticIssue[] {
  const out = [];
  const hasChild = new Set(
    store.getQuads(null, namedNode(RDFS + 'subClassOf'), null, null).map(q => q.object.value)
  );
  const hasParent = new Set(
    store.getQuads(null, namedNode(RDFS + 'subClassOf'), null, null).map(q => q.subject.value)
  );
  const instantiated = new Set(
    store.getQuads(null, namedNode(RDF + 'type'), null, null).map(q => q.object.value)
  );
  for (const [iri, ts] of types.entries()) {
    if (!(ts.has(RDFS + 'Class') || ts.has(OWL + 'Class'))) continue;
    if (hasChild.has(iri) || hasParent.has(iri) || instantiated.has(iri)) continue;
    out.push({
      code: 'orphan-class', severity: 'info', focus: iri,
      message: `Class "${shorten(iri)}" has no superclass, subclass, or instance.`,
    });
  }
  return out;
}

function checkLiteralWhereIriExpected(store: Store, types: Map<string, Set<string>>): SemanticIssue[] {
  const out = [];
  const iriPredicates = [RDFS + 'subClassOf', RDFS + 'domain', RDFS + 'range',
                        SH + 'path', SH + 'targetClass', SH + 'class', RDF + 'type'];
  for (const p of iriPredicates) {
    for (const q of store.getQuads(null, namedNode(p), null, null)) {
      if (q.object.termType === 'Literal') {
        out.push({
          code: 'literal-where-iri-expected', severity: 'error', focus: q.subject.value,
          predicate: p, object: q.object.value,
          message: `Predicate <${shorten(p)}> has a literal value; an IRI is expected.`,
        });
      }
    }
  }
  return out;
}

function num(term: { termType?: string; value?: string } | undefined): number | null {
  if (!term || term.termType !== 'Literal') return null;
  const n = Number(term.value);
  return Number.isFinite(n) ? n : null;
}

function shorten(iri: string): string {
  const i = Math.max(iri.lastIndexOf('#'), iri.lastIndexOf('/'));
  return i >= 0 ? iri.slice(i + 1) || iri : iri;
}
