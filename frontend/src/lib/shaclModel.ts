// Structured SHACL model for the visual builder.
//
// `parseShapesGraph` turns a Turtle shapes graph into an editable model of node
// shapes → property shapes → typed constraints, and `serializeShapesGraph`
// writes that model back to clean, readable Turtle. Together they give the
// builder two-way sync with the Turtle source.
//
// Safety gate: the builder must never silently drop SHACL it doesn't model. The
// parser sets `canRoundTrip = false` whenever it meets anything outside the
// supported subset — path expressions, logical ops (sh:and/or/xone/not), SPARQL
// constraints/rules, qualified shapes, named/shared property shapes, or any
// unrecognised predicate. When `canRoundTrip` is false the builder stays
// read-only and the Turtle source remains the single source of truth, so no
// information is lost.

import { Parser } from 'n3';

export const SH = 'http://www.w3.org/ns/shacl#';
const RDF = 'http://www.w3.org/1999/02/22-rdf-syntax-ns#';
const RDFS = 'http://www.w3.org/2000/01/rdf-schema#';
const XSD = 'http://www.w3.org/2001/XMLSchema#';
const RDF_TYPE = RDF + 'type';
const RDF_FIRST = RDF + 'first';
const RDF_REST = RDF + 'rest';
const RDF_NIL = RDF + 'nil';

export type Severity = 'Violation' | 'Warning' | 'Info';
export type TargetKind = 'class' | 'node' | 'subjectsOf' | 'objectsOf';

/** A concrete RDF value the builder can render and edit. */
export interface TermValue {
  type: 'iri' | 'literal';
  value: string;
  datatype?: string; // for literals, when not xsd:string / langString
  lang?: string;
}

/** Typed constraints we model on a property shape. */
export interface PropConstraints {
  minCount?: number;
  maxCount?: number;
  datatype?: string;
  class?: string;
  nodeKind?: string; // e.g. http://www.w3.org/ns/shacl#IRI
  minInclusive?: string;
  maxInclusive?: string;
  minExclusive?: string;
  maxExclusive?: string;
  minLength?: number;
  maxLength?: number;
  pattern?: string;
  flags?: string;
  in?: TermValue[];
  hasValue?: TermValue;
  languageIn?: string[];
  uniqueLang?: boolean;
  node?: string;
  equals?: string;
  disjoint?: string;
  lessThan?: string;
  lessThanOrEquals?: string;
}

export interface PropertyShape {
  path: string; // single predicate IRI when simple
  pathComplex?: boolean;
  name?: string;
  description?: string;
  message?: string;
  severity?: Severity;
  order?: number;
  group?: string;
  c: PropConstraints;
  /** True when the property carries SHACL the builder doesn't model. */
  complex?: boolean;
}

export interface NodeShape {
  iri: string;
  declared: boolean; // had an explicit `a sh:NodeShape`
  targets: { kind: TargetKind; value: string }[];
  closed?: boolean;
  ignoredProperties?: string[];
  name?: string;
  description?: string;
  message?: string;
  severity?: Severity;
  properties: PropertyShape[];
  complex?: boolean;
}

export interface ShapesGraph {
  prefixes: Record<string, string>;
  shapes: NodeShape[];
  /** When false, the builder must stay read-only (Turtle stays canonical). */
  canRoundTrip: boolean;
  parseError?: string;
}

const TARGET_PRED: Record<TargetKind, string> = {
  class: 'targetClass',
  node: 'targetNode',
  subjectsOf: 'targetSubjectsOf',
  objectsOf: 'targetObjectsOf',
};
const TARGET_BY_PRED: Record<string, TargetKind> = {
  targetClass: 'class',
  targetNode: 'node',
  targetSubjectsOf: 'subjectsOf',
  targetObjectsOf: 'objectsOf',
};

// Predicates the builder fully understands. Anything else flips canRoundTrip.
const NODE_KNOWN = new Set(
  [
    'targetClass',
    'targetNode',
    'targetSubjectsOf',
    'targetObjectsOf',
    'property',
    'closed',
    'ignoredProperties',
    'name',
    'description',
    'message',
    'severity',
  ].map((p) => SH + p),
);
const PROP_KNOWN = new Set(
  [
    'path',
    'name',
    'description',
    'message',
    'severity',
    'order',
    'group',
    'minCount',
    'maxCount',
    'datatype',
    'class',
    'nodeKind',
    'minInclusive',
    'maxInclusive',
    'minExclusive',
    'maxExclusive',
    'minLength',
    'maxLength',
    'pattern',
    'flags',
    'in',
    'hasValue',
    'languageIn',
    'uniqueLang',
    'node',
    'equals',
    'disjoint',
    'lessThan',
    'lessThanOrEquals',
  ].map((p) => SH + p),
);

// ─── Parsing ────────────────────────────────────────────────────────────────

type N3Term = { termType: string; value: string; datatype?: { value: string }; language?: string };

export function parseShapesGraph(ttl: string): ShapesGraph {
  const prefixes: Record<string, string> = {};
  const quads: { subject: N3Term; predicate: N3Term; object: N3Term }[] = [];
  try {
    // Synchronous parse: when no quad callback is given, N3 returns the full
    // quad array immediately (and throws on error). The callback form instead
    // streams asynchronously, so results would not be ready on return.
    const onPrefix = (prefix: string, iri: string | { value: string }) => {
      prefixes[prefix] = typeof iri === 'string' ? iri : iri?.value;
    };
    // Call as a bound method (detaching `parse` would lose its `this`).
    const parsed = new Parser().parse(String(ttl || ''), null as never, onPrefix as never) as unknown as {
      subject: N3Term;
      predicate: N3Term;
      object: N3Term;
    }[];
    for (const q of parsed) quads.push(q);
  } catch (e) {
    return { prefixes, shapes: [], canRoundTrip: false, parseError: (e as Error)?.message || 'Invalid Turtle' };
  }

  // Index subject → predicate → object terms, and note blank-node subjects.
  const idx = new Map<string, Map<string, N3Term[]>>();
  const blank = new Set<string>();
  for (const q of quads) {
    const s = q.subject.value;
    if (q.subject.termType === 'BlankNode') blank.add(s);
    let m = idx.get(s);
    if (!m) {
      m = new Map();
      idx.set(s, m);
    }
    const arr = m.get(q.predicate.value);
    if (arr) arr.push(q.object);
    else m.set(q.predicate.value, [q.object]);
  }

  const consumed = new Set<string>(); // subjects fully accounted for by the model
  const objs = (s: string, local: string) => idx.get(s)?.get(SH + local) || [];
  const firstSh = (s: string, local: string) => objs(s, local)[0];
  const litStr = (s: string, local: string) => {
    const t = firstSh(s, local);
    return t && t.termType === 'Literal' ? t.value : undefined;
  };
  const intSh = (s: string, local: string) => {
    const v = litStr(s, local);
    return v != null && v !== '' && !Number.isNaN(Number(v)) ? Number(v) : undefined;
  };
  const iriSh = (s: string, local: string) => {
    const t = firstSh(s, local);
    return t && t.termType === 'NamedNode' ? t.value : undefined;
  };
  const sevOf = (s: string): Severity | undefined => {
    const v = iriSh(s, 'severity');
    if (!v) return undefined;
    if (v === SH + 'Warning') return 'Warning';
    if (v === SH + 'Info') return 'Info';
    if (v === SH + 'Violation') return 'Violation';
    return undefined; // non-standard severity → caller marks complex
  };

  const termValue = (t: N3Term): TermValue => {
    if (t.termType === 'NamedNode') return { type: 'iri', value: t.value };
    const dt = t.datatype?.value;
    const lang = t.language || '';
    const tv: TermValue = { type: 'literal', value: t.value };
    if (lang) tv.lang = lang;
    else if (dt && dt !== XSD + 'string') tv.datatype = dt;
    return tv;
  };

  const readList = (head: N3Term | undefined): N3Term[] | null => {
    const items: N3Term[] = [];
    const guard = new Set<string>();
    let node = head;
    while (node) {
      if (node.termType === 'NamedNode' && node.value === RDF_NIL) return items;
      if (node.termType !== 'BlankNode') return null;
      if (guard.has(node.value)) return null;
      guard.add(node.value);
      consumed.add(node.value);
      const cell = idx.get(node.value);
      const first = cell?.get(RDF_FIRST)?.[0];
      const rest = cell?.get(RDF_REST)?.[0];
      if (!first || !rest) return null;
      items.push(first);
      node = rest;
    }
    return null;
  };

  const isNodeShapeSubject = (s: string, m: Map<string, N3Term[]>): boolean => {
    const types = (m.get(RDF_TYPE) || []).map((o) => o.value);
    if (types.includes(SH + 'NodeShape')) return true;
    const hasTargetOrProp =
      m.has(SH + 'targetClass') ||
      m.has(SH + 'targetNode') ||
      m.has(SH + 'targetSubjectsOf') ||
      m.has(SH + 'targetObjectsOf') ||
      m.has(SH + 'property');
    return hasTargetOrProp && !m.has(SH + 'path');
  };

  const parseProperty = (pid: string): PropertyShape => {
    consumed.add(pid);
    const m = idx.get(pid) || new Map<string, N3Term[]>();
    let complex = false;

    // path: only a single predicate IRI is modelled; expressions are complex.
    const pathTerm = firstSh(pid, 'path');
    let path = '';
    let pathComplex = false;
    if (pathTerm && pathTerm.termType === 'NamedNode') path = pathTerm.value;
    else if (pathTerm) {
      pathComplex = true;
      complex = true;
    }

    const c: PropConstraints = {};
    if (intSh(pid, 'minCount') != null) c.minCount = intSh(pid, 'minCount');
    if (intSh(pid, 'maxCount') != null) c.maxCount = intSh(pid, 'maxCount');
    if (iriSh(pid, 'datatype')) c.datatype = iriSh(pid, 'datatype');
    if (iriSh(pid, 'class')) c.class = iriSh(pid, 'class');
    if (iriSh(pid, 'nodeKind')) c.nodeKind = iriSh(pid, 'nodeKind');
    for (const k of ['minInclusive', 'maxInclusive', 'minExclusive', 'maxExclusive'] as const) {
      const term = firstSh(pid, k);
      if (!term) continue;
      // Only plain numeric bounds round-trip losslessly (they re-emit bare).
      // Typed bounds (xsd:date, xsd:double exponents, …) drop to read-only so
      // their datatype isn't silently lost.
      if (term.termType === 'Literal' && /^-?\d+(\.\d+)?$/.test(term.value)) c[k] = term.value;
      else {
        complex = true;
        if (term.termType === 'Literal') c[k] = term.value;
      }
    }
    if (intSh(pid, 'minLength') != null) c.minLength = intSh(pid, 'minLength');
    if (intSh(pid, 'maxLength') != null) c.maxLength = intSh(pid, 'maxLength');
    if (litStr(pid, 'pattern') != null) c.pattern = litStr(pid, 'pattern');
    if (litStr(pid, 'flags') != null) c.flags = litStr(pid, 'flags');
    if (litStr(pid, 'uniqueLang') != null) c.uniqueLang = litStr(pid, 'uniqueLang') === 'true';
    if (iriSh(pid, 'node')) c.node = iriSh(pid, 'node');
    for (const k of ['equals', 'disjoint', 'lessThan', 'lessThanOrEquals'] as const) {
      const v = iriSh(pid, k);
      if (v) c[k] = v;
    }
    const hv = firstSh(pid, 'hasValue');
    if (hv) c.hasValue = termValue(hv);

    const inHead = firstSh(pid, 'in');
    if (inHead) {
      const list = readList(inHead);
      if (list) c.in = list.map(termValue);
      else complex = true;
    }
    const langHead = firstSh(pid, 'languageIn');
    if (langHead) {
      const list = readList(langHead);
      if (list && list.every((t) => t.termType === 'Literal')) c.languageIn = list.map((t) => t.value);
      else complex = true;
    }

    if (firstSh(pid, 'severity') && !sevOf(pid)) complex = true; // non-standard severity

    // Any predicate we don't model → complex (preserve via read-only mode).
    for (const pred of m.keys()) {
      if (pred === RDF_TYPE) continue;
      if (!PROP_KNOWN.has(pred)) complex = true;
    }

    const ps: PropertyShape = { path, c };
    if (pathComplex) ps.pathComplex = true;
    const name = litStr(pid, 'name');
    if (name != null) ps.name = name;
    const desc = litStr(pid, 'description');
    if (desc != null) ps.description = desc;
    const msg = litStr(pid, 'message');
    if (msg != null) ps.message = msg;
    const sev = sevOf(pid);
    if (sev) ps.severity = sev;
    if (intSh(pid, 'order') != null) ps.order = intSh(pid, 'order');
    const grp = iriSh(pid, 'group');
    if (grp) ps.group = grp;
    if (complex) ps.complex = true;
    return ps;
  };

  const shapes: NodeShape[] = [];
  for (const [s, m] of idx) {
    if (blank.has(s)) continue;
    if (!isNodeShapeSubject(s, m)) continue;
    consumed.add(s);
    let complex = false;

    const targets: { kind: TargetKind; value: string }[] = [];
    for (const [pred, kind] of Object.entries(TARGET_BY_PRED)) {
      for (const t of objs(s, pred)) {
        if (t.termType === 'NamedNode') targets.push({ kind: kind as TargetKind, value: t.value });
        else complex = true; // e.g. targetClass via a list/expression
      }
    }

    const properties: PropertyShape[] = [];
    for (const po of objs(s, 'property')) {
      if (po.termType === 'BlankNode') properties.push(parseProperty(po.value));
      else {
        // Named / shared property shape — not restructured by the builder.
        complex = true;
        if (idx.has(po.value)) properties.push(parseProperty(po.value));
      }
    }

    const closedV = litStr(s, 'closed');
    const ignoredHead = firstSh(s, 'ignoredProperties');
    let ignoredProperties: string[] | undefined;
    if (ignoredHead) {
      const list = readList(ignoredHead);
      if (list && list.every((t) => t.termType === 'NamedNode')) ignoredProperties = list.map((t) => t.value);
      else complex = true;
    }
    if (firstSh(s, 'severity') && !sevOf(s)) complex = true;

    for (const pred of m.keys()) {
      if (pred === RDF_TYPE) continue;
      if (!NODE_KNOWN.has(pred)) complex = true;
    }
    if (properties.some((p) => p.complex)) complex = true;

    const types = (m.get(RDF_TYPE) || []).map((o) => o.value);
    const ns: NodeShape = {
      iri: s,
      declared: types.includes(SH + 'NodeShape'),
      targets,
      properties,
    };
    if (closedV != null) ns.closed = closedV === 'true';
    if (ignoredProperties) ns.ignoredProperties = ignoredProperties;
    const name = litStr(s, 'name');
    if (name != null) ns.name = name;
    const desc = litStr(s, 'description');
    if (desc != null) ns.description = desc;
    const msg = litStr(s, 'message');
    if (msg != null) ns.message = msg;
    const sev = sevOf(s);
    if (sev) ns.severity = sev;
    if (complex) ns.complex = true;
    shapes.push(ns);
  }

  // Any shape-relevant quad not consumed (standalone property shapes, orphan
  // SHACL, rules, etc.) means we can't safely regenerate the whole document.
  let uncovered = false;
  for (const q of quads) {
    if (consumed.has(q.subject.value)) continue;
    const isShape =
      q.predicate.value.startsWith(SH) ||
      (q.predicate.value === RDF_TYPE && q.object.value.startsWith(SH));
    if (isShape) {
      uncovered = true;
      break;
    }
  }

  shapes.sort((a, b) => a.iri.localeCompare(b.iri));
  const canRoundTrip = !uncovered && shapes.every((s) => !s.complex);
  return { prefixes, shapes, canRoundTrip };
}

// ─── Serialising ──────────────────────────────────────────────────────────────

const DEFAULT_PREFIXES: Record<string, string> = {
  sh: SH,
  rdf: RDF,
  rdfs: RDFS,
  xsd: XSD,
};

export function serializeShapesGraph(g: ShapesGraph): string {
  const prefixes: Record<string, string> = { ...DEFAULT_PREFIXES, ...g.prefixes };
  const curie = makeCurie(prefixes);

  const lit = (tv: TermValue): string => {
    const esc = tv.value
      .replace(/\\/g, '\\\\')
      .replace(/"/g, '\\"')
      .replace(/\n/g, '\\n')
      .replace(/\r/g, '\\r')
      .replace(/\t/g, '\\t');
    if (tv.lang) return `"${esc}"@${tv.lang}`;
    if (tv.datatype && tv.datatype !== XSD + 'string') return `"${esc}"^^${curie(tv.datatype)}`;
    return `"${esc}"`;
  };
  const val = (tv: TermValue): string => (tv.type === 'iri' ? curie(tv.value) : lit(tv));
  const num = (s: string): string => (/^-?\d+(\.\d+)?$/.test(s.trim()) ? s.trim() : `"${s.replace(/"/g, '\\"')}"`);
  const str = (s: string): string =>
    `"${s.replace(/\\/g, '\\\\').replace(/"/g, '\\"').replace(/\n/g, '\\n').replace(/\r/g, '\\r')}"`;

  const propLines = (p: PropertyShape): string[] => {
    const out: string[] = [];
    out.push(`sh:path ${p.path ? curie(p.path) : 'rdf:nil'}`);
    if (p.name != null) out.push(`sh:name ${str(p.name)}`);
    if (p.description != null) out.push(`sh:description ${str(p.description)}`);
    if (p.group) out.push(`sh:group ${curie(p.group)}`);
    if (p.order != null) out.push(`sh:order ${p.order}`);
    const c = p.c || {};
    if (c.minCount != null) out.push(`sh:minCount ${c.minCount}`);
    if (c.maxCount != null) out.push(`sh:maxCount ${c.maxCount}`);
    if (c.datatype) out.push(`sh:datatype ${curie(c.datatype)}`);
    if (c.class) out.push(`sh:class ${curie(c.class)}`);
    if (c.nodeKind) out.push(`sh:nodeKind ${curie(c.nodeKind)}`);
    if (c.minInclusive != null) out.push(`sh:minInclusive ${num(c.minInclusive)}`);
    if (c.maxInclusive != null) out.push(`sh:maxInclusive ${num(c.maxInclusive)}`);
    if (c.minExclusive != null) out.push(`sh:minExclusive ${num(c.minExclusive)}`);
    if (c.maxExclusive != null) out.push(`sh:maxExclusive ${num(c.maxExclusive)}`);
    if (c.minLength != null) out.push(`sh:minLength ${c.minLength}`);
    if (c.maxLength != null) out.push(`sh:maxLength ${c.maxLength}`);
    if (c.pattern != null) out.push(`sh:pattern ${str(c.pattern)}`);
    if (c.flags != null) out.push(`sh:flags ${str(c.flags)}`);
    if (c.languageIn && c.languageIn.length) out.push(`sh:languageIn ( ${c.languageIn.map(str).join(' ')} )`);
    if (c.uniqueLang) out.push(`sh:uniqueLang true`);
    if (c.in && c.in.length) out.push(`sh:in ( ${c.in.map(val).join(' ')} )`);
    if (c.hasValue) out.push(`sh:hasValue ${val(c.hasValue)}`);
    if (c.node) out.push(`sh:node ${curie(c.node)}`);
    for (const k of ['equals', 'disjoint', 'lessThan', 'lessThanOrEquals'] as const) {
      if (c[k]) out.push(`sh:${k} ${curie(c[k] as string)}`);
    }
    if (p.severity) out.push(`sh:severity sh:${p.severity}`);
    if (p.message != null) out.push(`sh:message ${str(p.message)}`);
    return out;
  };

  const shapeBlock = (s: NodeShape): string => {
    const parts: string[] = [];
    for (const t of s.targets) parts.push(`sh:${TARGET_PRED[t.kind]} ${curie(t.value)}`);
    if (s.name != null) parts.push(`sh:name ${str(s.name)}`);
    if (s.description != null) parts.push(`sh:description ${str(s.description)}`);
    if (s.severity) parts.push(`sh:severity sh:${s.severity}`);
    if (s.message != null) parts.push(`sh:message ${str(s.message)}`);
    if (s.closed) parts.push(`sh:closed true`);
    if (s.ignoredProperties && s.ignoredProperties.length)
      parts.push(`sh:ignoredProperties ( ${s.ignoredProperties.map(curie).join(' ')} )`);
    for (const p of s.properties) {
      const lines = propLines(p)
        .map((l) => `    ${l} ;`)
        .join('\n');
      parts.push(`sh:property [\n${lines}\n  ]`);
    }
    const head = `${curie(s.iri)} a sh:NodeShape`;
    if (parts.length === 0) return `${head} .`;
    return `${head} ;\n  ${parts.join(' ;\n  ')} .`;
  };

  const usedPrefixes = Object.entries(prefixes)
    .sort((a, b) => a[0].localeCompare(b[0]))
    .map(([p, ns]) => `@prefix ${p}: <${ns}> .`)
    .join('\n');
  const body = g.shapes.map(shapeBlock).join('\n\n');
  return `${usedPrefixes}\n\n${body}\n`;
}

function makeCurie(prefixes: Record<string, string>): (iri: string) => string {
  const entries = Object.entries(prefixes).sort((a, b) => b[1].length - a[1].length);
  return (iri: string) => {
    for (const [p, ns] of entries) {
      if (ns && iri.startsWith(ns)) {
        const local = iri.slice(ns.length);
        if (/^[A-Za-z_][A-Za-z0-9_.\-]*$/.test(local)) return `${p}:${local}`;
      }
    }
    return `<${iri}>`;
  };
}

// ─── Display helpers (shared by the builder's read-only mode) ────────────────

export function shortLocal(iri: string): string {
  if (!iri) return '';
  const parts = String(iri).split(/[#/]/);
  return parts[parts.length - 1] || iri;
}

/** Compact chips summarising a property's constraints, for read-only display. */
export function propChips(p: PropertyShape, curie: (iri: string) => string): { k: string; v: string }[] {
  const chips: { k: string; v: string }[] = [];
  const c = p.c || {};
  if (c.minCount != null || c.maxCount != null)
    chips.push({ k: 'card', v: `${c.minCount ?? '0'}..${c.maxCount ?? '*'}` });
  if (c.datatype) chips.push({ k: 'type', v: curie(c.datatype) });
  if (c.class) chips.push({ k: 'type', v: 'class ' + curie(c.class) });
  if (c.nodeKind) chips.push({ k: 'type', v: curie(c.nodeKind) });
  if (c.minInclusive != null) chips.push({ k: 'range', v: '≥ ' + c.minInclusive });
  if (c.maxInclusive != null) chips.push({ k: 'range', v: '≤ ' + c.maxInclusive });
  if (c.minExclusive != null) chips.push({ k: 'range', v: '> ' + c.minExclusive });
  if (c.maxExclusive != null) chips.push({ k: 'range', v: '< ' + c.maxExclusive });
  if (c.minLength != null) chips.push({ k: 'range', v: 'len≥' + c.minLength });
  if (c.maxLength != null) chips.push({ k: 'range', v: 'len≤' + c.maxLength });
  if (c.pattern) chips.push({ k: 'str', v: 'pattern' });
  if (c.in && c.in.length) chips.push({ k: 'str', v: 'enum' });
  if (c.hasValue) chips.push({ k: 'str', v: '= ' + (c.hasValue.type === 'iri' ? curie(c.hasValue.value) : c.hasValue.value) });
  if (c.node) chips.push({ k: 'shape', v: 'node ' + curie(c.node) });
  if (c.uniqueLang) chips.push({ k: 'str', v: 'uniqueLang' });
  if (c.languageIn && c.languageIn.length) chips.push({ k: 'str', v: 'lang' });
  if (p.severity) chips.push({ k: 'sev', v: p.severity });
  return chips;
}

export { makeCurie };
