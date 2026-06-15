// Structured SHACL model for the visual builder.
//
// `parseShapesGraph` turns a Turtle shapes graph into an editable model of node
// shapes → property shapes → typed constraints, and `serializeShapesGraph`
// writes that model back to clean, readable Turtle. Together they give the
// builder two-way sync with the Turtle source.
//
// Lossless guarantee: every quad of the source document is accounted for.
// A quad is either
//   (a) modelled — parsed into a typed field and regenerated on serialise — or
//   (b) retained verbatim in an `extraQuads` bag (per node shape, per property
//       shape, or graph-level) and replayed on serialise with stable
//       blank-node labels, including whole unknown blank-node closures.
// Constructs the builder cannot edit (SPARQL constraints, rules, exotic
// literals, …) therefore survive round-trips untouched. `hasUnsupported`
// flags shapes carrying retained quads so the UI can badge them; the editor
// stays editable for the modelled constructs. `canRoundTrip` is false only
// when the Turtle itself fails to parse.

import { Parser } from 'n3';

export const SH = 'http://www.w3.org/ns/shacl#';
const RDF = 'http://www.w3.org/1999/02/22-rdf-syntax-ns#';
const RDFS = 'http://www.w3.org/2000/01/rdf-schema#';
const XSD = 'http://www.w3.org/2001/XMLSchema#';
const RDF_TYPE = RDF + 'type';
const RDF_FIRST = RDF + 'first';
const RDF_REST = RDF + 'rest';
const RDF_NIL = RDF + 'nil';

/** A severity is any IRI; usually one of the SEVERITY_* constants. */
export type Severity = string;
export const SEVERITY_VIOLATION = SH + 'Violation';
export const SEVERITY_WARNING = SH + 'Warning';
export const SEVERITY_INFO = SH + 'Info';

export type TargetKind = 'class' | 'node' | 'subjectsOf' | 'objectsOf';

/** A concrete RDF value the builder can render and edit. */
export interface TermValue {
  type: 'iri' | 'literal';
  value: string;
  datatype?: string; // for literals, when not xsd:string / langString
  lang?: string;
}

/** A retained RDF term (subject/object of an extra quad). */
export interface ExtraTerm {
  termType: 'NamedNode' | 'BlankNode' | 'Literal';
  value: string;
  datatype?: string;
  language?: string;
}

/** A retained quad, replayed verbatim on serialise. */
export interface ExtraQuad {
  s: ExtraTerm;
  p: string; // predicate IRI
  o: ExtraTerm;
}

/** Structured SHACL property path expression. */
export type PathExpr =
  | { kind: 'predicate'; iri: string }
  | { kind: 'inverse'; path: PathExpr }
  | { kind: 'sequence'; paths: PathExpr[] }
  | { kind: 'alternative'; paths: PathExpr[] }
  | { kind: 'zeroOrMore'; path: PathExpr }
  | { kind: 'oneOrMore'; path: PathExpr }
  | { kind: 'zeroOrOne'; path: PathExpr };

/** A shape operand: a reference to a named shape, or an inline anonymous one. */
export type ShapeRef = { ref: string } | { inline: PropertyShape };

/**
 * Logical operators. `and`/`or`/`xone` each hold ONE operand list (a second
 * sh:and/or/xone on the same shape is retained via extraQuads); `not` holds
 * one entry per sh:not statement.
 */
export interface LogicConstraints {
  and?: ShapeRef[];
  or?: ShapeRef[];
  xone?: ShapeRef[];
  not?: ShapeRef[];
}

export interface QualifiedValue {
  shape: ShapeRef;
  minCount?: number;
  maxCount?: number;
  disjoint?: boolean;
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

/**
 * A property shape. Also used for inline anonymous shapes inside logical
 * operands and qualified value shapes, where `path` may be empty and
 * `properties` may carry nested `sh:property` children.
 */
export interface PropertyShape {
  /** IRI when this is a named (shared) property shape. */
  iri?: string;
  /** Had an explicit `a sh:PropertyShape`. */
  declared?: boolean;
  path: string; // single predicate IRI when simple, '' otherwise
  /** Structured path when it is not a plain predicate IRI. */
  pathExpr?: PathExpr;
  pathComplex?: boolean;
  name?: string;
  description?: string;
  message?: string;
  severity?: Severity;
  order?: number;
  group?: string;
  c: PropConstraints;
  logic?: LogicConstraints;
  qualified?: QualifiedValue;
  /** Nested sh:property children (inline anonymous node shapes). */
  properties?: PropertyShape[];
  /** Quads under this shape the model doesn't understand (kept verbatim). */
  extraQuads?: ExtraQuad[];
  /** True when this shape (or a nested one) carries retained quads. */
  hasUnsupported?: boolean;
  /** @internal original RDF subject; partitions extraQuads on serialise. */
  _own?: ExtraTerm;
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
  logic?: LogicConstraints;
  properties: PropertyShape[];
  extraQuads?: ExtraQuad[];
  hasUnsupported?: boolean;
}

export interface ShapesGraph {
  prefixes: Record<string, string>;
  shapes: NodeShape[];
  /** Quads outside any modelled shape, replayed verbatim on serialise. */
  extraQuads?: ExtraQuad[];
  /** True when any retained (non-modelled) statement is present anywhere. */
  hasUnsupported?: boolean;
  /** False only when the Turtle failed to parse (model edits impossible). */
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

// Lexical forms the serialiser re-emits bare (Turtle INTEGER / DECIMAL).
const BARE_INT = /^[+-]?\d+$/;
const BARE_DECIMAL = /^[+-]?\d*\.\d+$/;
const BARE_NUM = /^[+-]?(\d+|\d*\.\d+)$/;

// ─── Parsing ────────────────────────────────────────────────────────────────

type N3Term = { termType: string; value: string; datatype?: { value: string }; language?: string };
type Q = { subject: N3Term; predicate: N3Term; object: N3Term };

const termKey = (t: N3Term): string =>
  t.termType === 'Literal'
    ? `L${t.value}${t.language || ''}${t.datatype?.value || ''}`
    : (t.termType === 'BlankNode' ? 'B' : 'N') + t.value;

const toExtraTerm = (t: N3Term): ExtraTerm => {
  if (t.termType === 'NamedNode') return { termType: 'NamedNode', value: t.value };
  if (t.termType === 'BlankNode') return { termType: 'BlankNode', value: t.value };
  const e: ExtraTerm = { termType: 'Literal', value: t.value };
  if (t.language) e.language = t.language;
  else if (t.datatype?.value && t.datatype.value !== XSD + 'string') e.datatype = t.datatype.value;
  return e;
};

export function parseShapesGraph(ttl: string): ShapesGraph {
  const prefixes: Record<string, string> = {};
  let parsed: Q[];
  try {
    // Synchronous parse: when no quad callback is given, N3 returns the full
    // quad array immediately (and throws on error). The callback form instead
    // streams asynchronously, so results would not be ready on return.
    const onPrefix = (prefix: string, iri: string | { value: string }) => {
      prefixes[prefix] = typeof iri === 'string' ? iri : iri?.value;
    };
    parsed = new Parser().parse(String(ttl || ''), null as never, onPrefix as never) as unknown as Q[];
  } catch (e) {
    return { prefixes, shapes: [], canRoundTrip: false, parseError: (e as Error)?.message || 'Invalid Turtle' };
  }

  // De-duplicate identical triples (RDF set semantics) so quad-level
  // used-tracking can work by object identity.
  const quads: Q[] = [];
  {
    const seen = new Set<string>();
    for (const q of parsed) {
      const k = `${termKey(q.subject)}>${q.predicate.value}>${termKey(q.object)}`;
      if (!seen.has(k)) {
        seen.add(k);
        quads.push(q);
      }
    }
  }

  // Index subject-key → predicate → quads, and count blank-node references.
  const idx = new Map<string, Map<string, Q[]>>();
  const bRefs = new Map<string, number>();
  for (const q of quads) {
    const sk = termKey(q.subject);
    let m = idx.get(sk);
    if (!m) {
      m = new Map();
      idx.set(sk, m);
    }
    const arr = m.get(q.predicate.value);
    if (arr) arr.push(q);
    else m.set(q.predicate.value, [q]);
    if (q.object.termType === 'BlankNode') bRefs.set(q.object.value, (bRefs.get(q.object.value) || 0) + 1);
  }
  // A blank node referenced from more than one place can't be restructured
  // without breaking co-reference; such structures are retained verbatim.
  const shared = (label: string) => (bRefs.get(label) || 0) > 1;

  const used = new Set<Q>();
  const use = (q: Q) => used.add(q);
  const quadsOf = (sk: string, pred: string): Q[] => idx.get(sk)?.get(pred) || [];
  const sQuads = (sk: string, local: string) => quadsOf(sk, SH + local);

  // ── Pickers: model a value only when serialisation reproduces the quad ──
  const pickStr = (sk: string, local: string): string | undefined => {
    for (const q of sQuads(sk, local)) {
      if (used.has(q)) continue;
      const o = q.object;
      if (o.termType === 'Literal' && !o.language && (!o.datatype || o.datatype.value === XSD + 'string')) {
        use(q);
        return o.value;
      }
    }
    return undefined;
  };
  const pickIri = (sk: string, local: string): string | undefined => {
    for (const q of sQuads(sk, local)) {
      if (used.has(q)) continue;
      if (q.object.termType === 'NamedNode') {
        use(q);
        return q.object.value;
      }
    }
    return undefined;
  };
  const pickInt = (sk: string, local: string): number | undefined => {
    for (const q of sQuads(sk, local)) {
      if (used.has(q)) continue;
      const o = q.object;
      if (
        o.termType === 'Literal' &&
        o.datatype?.value === XSD + 'integer' &&
        BARE_INT.test(o.value) &&
        String(Number(o.value)) === o.value
      ) {
        use(q);
        return Number(o.value);
      }
    }
    return undefined;
  };
  const pickNum = (sk: string, local: string): number | undefined => {
    for (const q of sQuads(sk, local)) {
      if (used.has(q)) continue;
      const o = q.object;
      const dt = o.datatype?.value;
      const ok =
        o.termType === 'Literal' &&
        ((dt === XSD + 'integer' && BARE_INT.test(o.value)) || (dt === XSD + 'decimal' && BARE_DECIMAL.test(o.value))) &&
        String(Number(o.value)) === o.value;
      if (ok) {
        use(q);
        return Number(o.value);
      }
    }
    return undefined;
  };
  const pickBool = (sk: string, local: string): boolean | undefined => {
    for (const q of sQuads(sk, local)) {
      if (used.has(q)) continue;
      const o = q.object;
      if (o.termType === 'Literal' && o.datatype?.value === XSD + 'boolean' && (o.value === 'true' || o.value === 'false')) {
        use(q);
        return o.value === 'true';
      }
    }
    return undefined;
  };
  // Range bounds are stored as strings and re-emitted either bare (numeric)
  // or quoted (plain string). Anything else (dates, doubles, langStrings)
  // is retained verbatim so its datatype isn't silently lost.
  const pickBound = (sk: string, local: string): string | undefined => {
    for (const q of sQuads(sk, local)) {
      if (used.has(q)) continue;
      const o = q.object;
      if (o.termType !== 'Literal' || o.language) continue;
      const dt = o.datatype?.value || XSD + 'string';
      const ok =
        (dt === XSD + 'integer' && BARE_INT.test(o.value)) ||
        (dt === XSD + 'decimal' && BARE_DECIMAL.test(o.value)) ||
        (dt === XSD + 'string' && !BARE_NUM.test(o.value));
      if (ok) {
        use(q);
        return o.value;
      }
    }
    return undefined;
  };
  const termValue = (t: N3Term): TermValue => {
    if (t.termType === 'NamedNode') return { type: 'iri', value: t.value };
    const dt = t.datatype?.value;
    const tv: TermValue = { type: 'literal', value: t.value };
    if (t.language) tv.lang = t.language;
    else if (dt && dt !== XSD + 'string') tv.datatype = dt;
    return tv;
  };
  const pickTerm = (sk: string, local: string): TermValue | undefined => {
    for (const q of sQuads(sk, local)) {
      if (used.has(q)) continue;
      if (q.object.termType === 'NamedNode' || q.object.termType === 'Literal') {
        use(q);
        return termValue(q.object);
      }
    }
    return undefined;
  };

  // Read a well-formed, unshared RDF list without consuming it; the caller
  // commits the returned quads only when it can model every item.
  const readList = (head: N3Term): { items: N3Term[]; quads: Q[] } | null => {
    const items: N3Term[] = [];
    const lq: Q[] = [];
    const guard = new Set<string>();
    let node = head;
    for (;;) {
      if (node.termType === 'NamedNode') return node.value === RDF_NIL ? { items, quads: lq } : null;
      if (node.termType !== 'BlankNode' || guard.has(node.value) || shared(node.value)) return null;
      guard.add(node.value);
      const m = idx.get('B' + node.value);
      const f = m?.get(RDF_FIRST) || [];
      const r = m?.get(RDF_REST) || [];
      // Cells must be pure list cells (extra annotations would dangle after
      // the list is regenerated), with exactly one first/rest each.
      if (!m || m.size !== 2 || f.length !== 1 || r.length !== 1) return null;
      if (used.has(f[0]) || used.has(r[0])) return null;
      items.push(f[0].object);
      lq.push(f[0], r[0]);
      node = r[0].object;
    }
  };

  // Structured path parsing; pure (caller commits quads on success).
  const parsePath = (t: N3Term): { expr: PathExpr; quads: Q[] } | null => {
    if (t.termType === 'NamedNode') {
      if (t.value === RDF_NIL) return null;
      return { expr: { kind: 'predicate', iri: t.value }, quads: [] };
    }
    if (t.termType !== 'BlankNode' || shared(t.value)) return null;
    const m = idx.get('B' + t.value);
    if (!m) return null;
    if (m.has(RDF_FIRST)) {
      const l = readList(t);
      if (!l || l.items.length < 2) return null;
      const subs = l.items.map(parsePath);
      if (subs.some((s) => !s)) return null;
      return {
        expr: { kind: 'sequence', paths: subs.map((s) => s!.expr) },
        quads: l.quads.concat(...subs.map((s) => s!.quads)),
      };
    }
    if (m.size !== 1) return null;
    const [pred, qs] = [...m.entries()][0];
    if (qs.length !== 1 || used.has(qs[0])) return null;
    const q = qs[0];
    const unary = (kind: 'inverse' | 'zeroOrMore' | 'oneOrMore' | 'zeroOrOne') => {
      const s = parsePath(q.object);
      return s ? { expr: { kind, path: s.expr } as PathExpr, quads: [q, ...s.quads] } : null;
    };
    if (pred === SH + 'inversePath') return unary('inverse');
    if (pred === SH + 'zeroOrMorePath') return unary('zeroOrMore');
    if (pred === SH + 'oneOrMorePath') return unary('oneOrMore');
    if (pred === SH + 'zeroOrOnePath') return unary('zeroOrOne');
    if (pred === SH + 'alternativePath') {
      const l = readList(q.object);
      if (!l || l.items.length < 2) return null;
      const subs = l.items.map(parsePath);
      if (subs.some((s) => !s)) return null;
      return {
        expr: { kind: 'alternative', paths: subs.map((s) => s!.expr) },
        quads: [q, ...l.quads, ...subs.flatMap((s) => s!.quads)],
      };
    }
    return null;
  };

  // ── Extras: anything not modelled is retained, with its bnode closure ──
  type ExtraOwner = { extraQuads?: ExtraQuad[]; hasUnsupported?: boolean; declared?: boolean };
  const toQuad = (q: Q): ExtraQuad => ({ s: toExtraTerm(q.subject), p: q.predicate.value, o: toExtraTerm(q.object) });
  const collectClosure = (t: N3Term, owner: ExtraOwner) => {
    if (t.termType !== 'BlankNode') return;
    const stack = [t.value];
    while (stack.length) {
      const lab = stack.pop()!;
      const m = idx.get('B' + lab);
      if (!m) continue;
      for (const qs of m.values()) {
        for (const q of qs) {
          if (used.has(q)) continue;
          use(q);
          owner.extraQuads!.push(toQuad(q));
          if (q.object.termType === 'BlankNode') stack.push(q.object.value);
        }
      }
    }
  };
  const sweep = (sk: string, owner: ExtraOwner, declaredType: string) => {
    const m = idx.get(sk);
    if (!m) return;
    for (const [pred, qs] of m) {
      for (const q of qs) {
        if (used.has(q)) continue;
        if (pred === RDF_TYPE && q.object.termType === 'NamedNode' && q.object.value === declaredType) {
          owner.declared = true;
          use(q);
          continue;
        }
        use(q);
        (owner.extraQuads ||= []).push(toQuad(q));
        owner.hasUnsupported = true;
        collectClosure(q.object, owner);
      }
    }
  };

  const refsUnsupported = (logic?: LogicConstraints, qualified?: QualifiedValue): boolean => {
    const all: ShapeRef[] = [];
    for (const op of ['and', 'or', 'xone', 'not'] as const) if (logic?.[op]) all.push(...logic[op]!);
    if (qualified) all.push(qualified.shape);
    return all.some((r) => 'inline' in r && r.inline.hasUnsupported);
  };

  // Named (IRI-identified) property shapes are parsed once and shared.
  const namedProps = new Map<string, PropertyShape>();

  const shapeRefOf = (t: N3Term): ShapeRef | null => {
    if (t.termType === 'NamedNode') return { ref: t.value };
    if (t.termType === 'BlankNode' && !shared(t.value)) return { inline: parseBody(termKey(t), t) };
    return null;
  };

  const parseLogic = (sk: string): LogicConstraints | undefined => {
    const logic: LogicConstraints = {};
    let any = false;
    for (const op of ['and', 'or', 'xone'] as const) {
      // Only the first list per operator is modelled; further ones (a second
      // sh:or is a separate constraint) are retained verbatim.
      const q = sQuads(sk, op).find((x) => !used.has(x));
      if (!q) continue;
      const l = readList(q.object);
      if (!l) continue;
      if (!l.items.every((t) => t.termType === 'NamedNode' || (t.termType === 'BlankNode' && !shared(t.value)))) continue;
      use(q);
      l.quads.forEach(use);
      logic[op] = l.items.map((t) => shapeRefOf(t)!);
      any = true;
    }
    for (const q of sQuads(sk, 'not')) {
      if (used.has(q)) continue;
      const r = q.object.termType === 'NamedNode' || (q.object.termType === 'BlankNode' && !shared(q.object.value));
      if (!r) continue;
      use(q);
      (logic.not ||= []).push(shapeRefOf(q.object)!);
      any = true;
    }
    return any ? logic : undefined;
  };

  const parseQualified = (sk: string): QualifiedValue | undefined => {
    let shape: ShapeRef | undefined;
    for (const q of sQuads(sk, 'qualifiedValueShape')) {
      if (used.has(q)) continue;
      const r =
        q.object.termType === 'NamedNode' || (q.object.termType === 'BlankNode' && !shared(q.object.value))
          ? shapeRefOf(q.object)
          : null;
      if (r) {
        use(q);
        shape = r;
      }
      break;
    }
    if (!shape) return undefined;
    const qv: QualifiedValue = { shape };
    const mn = pickInt(sk, 'qualifiedMinCount');
    if (mn != null) qv.minCount = mn;
    const mx = pickInt(sk, 'qualifiedMaxCount');
    if (mx != null) qv.maxCount = mx;
    const dj = pickBool(sk, 'qualifiedValueShapesDisjoint');
    if (dj != null) qv.disjoint = dj;
    return qv;
  };

  // Parse a property shape / inline anonymous shape body.
  const parseBody = (sk: string, own: N3Term): PropertyShape => {
    const ps: PropertyShape = { path: '', c: {} };
    ps._own = toExtraTerm(own);

    for (const q of sQuads(sk, 'path')) {
      if (used.has(q)) continue;
      const r = parsePath(q.object);
      if (r) {
        use(q);
        r.quads.forEach(use);
        if (r.expr.kind === 'predicate') ps.path = r.expr.iri;
        else {
          ps.pathExpr = r.expr;
          ps.pathComplex = true;
        }
      }
      break; // only the first sh:path is modelled; a failed parse stays retained
    }

    const c = ps.c;
    const setN = (key: 'minCount' | 'maxCount' | 'minLength' | 'maxLength') => {
      const v = pickInt(sk, key);
      if (v != null) c[key] = v;
    };
    setN('minCount');
    setN('maxCount');
    const dt = pickIri(sk, 'datatype');
    if (dt) c.datatype = dt;
    const cl = pickIri(sk, 'class');
    if (cl) c.class = cl;
    const nk = pickIri(sk, 'nodeKind');
    if (nk) c.nodeKind = nk;
    for (const k of ['minInclusive', 'maxInclusive', 'minExclusive', 'maxExclusive'] as const) {
      const v = pickBound(sk, k);
      if (v != null) c[k] = v;
    }
    setN('minLength');
    setN('maxLength');
    const pat = pickStr(sk, 'pattern');
    if (pat != null) c.pattern = pat;
    const fl = pickStr(sk, 'flags');
    if (fl != null) c.flags = fl;
    const ul = pickBool(sk, 'uniqueLang');
    if (ul != null) c.uniqueLang = ul;
    const nd = pickIri(sk, 'node');
    if (nd) c.node = nd;
    for (const k of ['equals', 'disjoint', 'lessThan', 'lessThanOrEquals'] as const) {
      const v = pickIri(sk, k);
      if (v) c[k] = v;
    }
    const hv = pickTerm(sk, 'hasValue');
    if (hv) c.hasValue = hv;

    for (const q of sQuads(sk, 'in')) {
      if (used.has(q)) continue;
      const l = readList(q.object);
      if (l && l.items.every((t) => t.termType === 'NamedNode' || t.termType === 'Literal')) {
        use(q);
        l.quads.forEach(use);
        c.in = l.items.map(termValue);
      }
      break;
    }
    for (const q of sQuads(sk, 'languageIn')) {
      if (used.has(q)) continue;
      const l = readList(q.object);
      if (
        l &&
        l.items.every((t) => t.termType === 'Literal' && !t.language && (!t.datatype || t.datatype.value === XSD + 'string'))
      ) {
        use(q);
        l.quads.forEach(use);
        c.languageIn = l.items.map((t) => t.value);
      }
      break;
    }

    const name = pickStr(sk, 'name');
    if (name != null) ps.name = name;
    const desc = pickStr(sk, 'description');
    if (desc != null) ps.description = desc;
    const msg = pickStr(sk, 'message');
    if (msg != null) ps.message = msg;
    const sev = pickIri(sk, 'severity');
    if (sev) ps.severity = sev;
    const ord = pickNum(sk, 'order');
    if (ord != null) ps.order = ord;
    const grp = pickIri(sk, 'group');
    if (grp) ps.group = grp;

    const logic = parseLogic(sk);
    if (logic) ps.logic = logic;
    const qualified = parseQualified(sk);
    if (qualified) ps.qualified = qualified;

    const children: PropertyShape[] = [];
    for (const q of sQuads(sk, 'property')) {
      if (used.has(q)) continue;
      const child = propertyRef(q);
      if (child) children.push(child);
    }
    if (children.length) ps.properties = children;

    sweep(sk, ps, SH + 'PropertyShape');
    if (children.some((ch) => ch.hasUnsupported) || refsUnsupported(ps.logic, ps.qualified)) ps.hasUnsupported = true;
    return ps;
  };

  // Resolve a sh:property object: blank (anonymous) or named (shared) shape.
  const propertyRef = (q: Q): PropertyShape | null => {
    const o = q.object;
    if (o.termType === 'BlankNode') {
      if (shared(o.value)) return null; // multi-referenced; retained verbatim
      use(q);
      return parseBody(termKey(o), o);
    }
    if (o.termType === 'NamedNode') {
      use(q);
      let ps = namedProps.get(o.value);
      if (!ps) {
        ps = idx.has(termKey(o)) ? parseBody(termKey(o), o) : { path: '', c: {}, _own: toExtraTerm(o) };
        ps.iri = o.value;
        namedProps.set(o.value, ps);
      }
      return ps;
    }
    return null;
  };

  const isNodeShapeSubject = (m: Map<string, Q[]>): boolean => {
    const types = (m.get(RDF_TYPE) || []).map((q) => q.object.value);
    if (types.includes(SH + 'NodeShape')) return true;
    const hasTargetOrProp =
      m.has(SH + 'targetClass') ||
      m.has(SH + 'targetNode') ||
      m.has(SH + 'targetSubjectsOf') ||
      m.has(SH + 'targetObjectsOf') ||
      m.has(SH + 'property');
    return hasTargetOrProp && !m.has(SH + 'path');
  };

  const shapes: NodeShape[] = [];
  for (const [sk, m] of idx) {
    if (!sk.startsWith('N')) continue; // blank-node shapes stay where referenced
    if (!isNodeShapeSubject(m)) continue;
    const iri = sk.slice(1);
    if (namedProps.has(iri)) continue; // already modelled as a property shape

    const ns: NodeShape = { iri, declared: false, targets: [], properties: [] };
    for (const [pred, kind] of Object.entries(TARGET_BY_PRED)) {
      for (const q of quadsOf(sk, SH + pred)) {
        if (used.has(q)) continue;
        if (q.object.termType === 'NamedNode') {
          use(q);
          ns.targets.push({ kind, value: q.object.value });
        }
      }
    }
    const closed = pickBool(sk, 'closed');
    if (closed != null) ns.closed = closed;
    for (const q of sQuads(sk, 'ignoredProperties')) {
      if (used.has(q)) continue;
      const l = readList(q.object);
      if (l && l.items.every((t) => t.termType === 'NamedNode')) {
        use(q);
        l.quads.forEach(use);
        ns.ignoredProperties = l.items.map((t) => t.value);
      }
      break;
    }
    const name = pickStr(sk, 'name');
    if (name != null) ns.name = name;
    const desc = pickStr(sk, 'description');
    if (desc != null) ns.description = desc;
    const msg = pickStr(sk, 'message');
    if (msg != null) ns.message = msg;
    const sev = pickIri(sk, 'severity');
    if (sev) ns.severity = sev;
    const logic = parseLogic(sk);
    if (logic) ns.logic = logic;

    for (const q of sQuads(sk, 'property')) {
      if (used.has(q)) continue;
      const child = propertyRef(q);
      if (child) ns.properties.push(child);
    }

    sweep(sk, ns, SH + 'NodeShape');
    if (ns.properties.some((p) => p.hasUnsupported) || refsUnsupported(ns.logic)) ns.hasUnsupported = true;
    shapes.push(ns);
  }

  // Everything left over (orphan property shapes, ontology triples mixed into
  // the document, …) is retained at graph level.
  const gExtras: ExtraQuad[] = [];
  for (const q of quads) if (!used.has(q)) gExtras.push(toQuad(q));

  shapes.sort((a, b) => a.iri.localeCompare(b.iri));
  const g: ShapesGraph = { prefixes, shapes, canRoundTrip: true };
  if (gExtras.length) g.extraQuads = gExtras;
  if (gExtras.length || shapes.some((s) => s.hasUnsupported)) g.hasUnsupported = true;
  return g;
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

  // Stable blank-node labels for retained quads (deterministic per serialise).
  const blanks = new Map<string, string>();
  const bl = (label: string): string => {
    let v = blanks.get(label);
    if (!v) {
      v = 'x' + blanks.size;
      blanks.set(label, v);
    }
    return v;
  };
  // Retained quads whose subject is a blank-node closure (or another subject
  // than the owning shape) are emitted as labelled top-level statements.
  const pool: string[] = [];

  const esc = (s: string) => s.replace(/\\/g, '\\\\').replace(/"/g, '\\"').replace(/\n/g, '\\n').replace(/\r/g, '\\r');
  const str = (s: string): string => `"${esc(s)}"`;
  const lit = (tv: TermValue): string => {
    if (tv.lang) return `${str(tv.value)}@${tv.lang}`;
    if (tv.datatype && tv.datatype !== XSD + 'string') return `${str(tv.value)}^^${curie(tv.datatype)}`;
    return str(tv.value);
  };
  const val = (tv: TermValue): string => (tv.type === 'iri' ? curie(tv.value) : lit(tv));
  const num = (s: string): string => (BARE_NUM.test(s.trim()) ? s.trim() : str(s));
  const xTerm = (t: ExtraTerm): string => {
    if (t.termType === 'NamedNode') return curie(t.value);
    if (t.termType === 'BlankNode') return '_:' + bl(t.value);
    if (t.language) return `${str(t.value)}@${t.language}`;
    if (t.datatype) return `${str(t.value)}^^${curie(t.datatype)}`;
    return str(t.value);
  };
  const xPred = (p: string): string => (p === RDF_TYPE ? 'a' : curie(p));

  const pathTtl = (e: PathExpr): string => {
    switch (e.kind) {
      case 'predicate':
        return curie(e.iri);
      case 'inverse':
        return `[ sh:inversePath ${pathTtl(e.path)} ]`;
      case 'sequence':
        return `( ${e.paths.map(pathTtl).join(' ')} )`;
      case 'alternative':
        return `[ sh:alternativePath ( ${e.paths.map(pathTtl).join(' ')} ) ]`;
      case 'zeroOrMore':
        return `[ sh:zeroOrMorePath ${pathTtl(e.path)} ]`;
      case 'oneOrMore':
        return `[ sh:oneOrMorePath ${pathTtl(e.path)} ]`;
      case 'zeroOrOne':
        return `[ sh:zeroOrOnePath ${pathTtl(e.path)} ]`;
    }
  };

  const namedEmitted = new Set<string>();
  const namedBlocks: string[] = [];

  const refTtl = (r: ShapeRef, indent: string): string => ('ref' in r ? curie(r.ref) : inline(r.inline, indent, false));

  const inline = (p: PropertyShape, indent: string, placeholderPath: boolean): string => {
    const inner = indent + '  ';
    const lines: string[] = [];
    if (p.declared && !p.iri) lines.push('a sh:PropertyShape');
    lines.push(...bodyLines(p, inner, placeholderPath));
    if (!lines.length) return '[ ]';
    return `[\n${lines.map((l) => `${inner}${l} ;`).join('\n')}\n${indent}]`;
  };

  const propertyLine = (p: PropertyShape, indent: string): string => {
    if (p.iri) {
      if (!namedEmitted.has(p.iri)) {
        namedEmitted.add(p.iri);
        const lines = bodyLines(p, '  ', false);
        if (p.declared || lines.length) {
          const head = `${curie(p.iri)}${p.declared ? ' a sh:PropertyShape' : ''}`;
          if (!lines.length) namedBlocks.push(`${head} .`);
          else if (p.declared) namedBlocks.push(`${head} ;\n  ${lines.join(' ;\n  ')} .`);
          else namedBlocks.push(`${head} ${lines.join(' ;\n  ')} .`);
        }
      }
      return `sh:property ${curie(p.iri)}`;
    }
    return `sh:property ${inline(p, indent, true)}`;
  };

  const logicLines = (logic: LogicConstraints | undefined, indent: string, out: string[]) => {
    if (!logic) return;
    for (const op of ['and', 'or', 'xone'] as const) {
      const refs = logic[op];
      if (refs) out.push(`sh:${op} ( ${refs.map((r) => refTtl(r, indent)).join(' ')} )`);
    }
    for (const r of logic.not || []) out.push(`sh:not ${refTtl(r, indent)}`);
  };

  // Partition retained quads: subject == the owning node → inline lines;
  // everything else (closures) → labelled top-level statements.
  const splitExtras = (extras: ExtraQuad[] | undefined, own: ExtraTerm | undefined): string[] => {
    const ownLines: string[] = [];
    for (const e of extras || []) {
      if (own && e.s.termType === own.termType && e.s.value === own.value) ownLines.push(`${xPred(e.p)} ${xTerm(e.o)}`);
      else pool.push(`${xTerm(e.s)} ${xPred(e.p)} ${xTerm(e.o)} .`);
    }
    return ownLines;
  };

  const bodyLines = (p: PropertyShape, indent: string, placeholderPath: boolean): string[] => {
    const out: string[] = [];
    const ownExtras = splitExtras(p.extraQuads, p._own);
    if (p.pathExpr) out.push(`sh:path ${pathTtl(p.pathExpr)}`);
    else if (p.path) out.push(`sh:path ${curie(p.path)}`);
    else if (placeholderPath && !ownExtras.some((l) => l.startsWith('sh:path '))) out.push(`sh:path rdf:nil`);
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
    if (c.languageIn) out.push(`sh:languageIn ( ${c.languageIn.map(str).join(' ')} )`);
    if (c.uniqueLang != null) out.push(`sh:uniqueLang ${c.uniqueLang}`);
    if (c.in) out.push(`sh:in ( ${c.in.map(val).join(' ')} )`);
    if (c.hasValue) out.push(`sh:hasValue ${val(c.hasValue)}`);
    if (c.node) out.push(`sh:node ${curie(c.node)}`);
    for (const k of ['equals', 'disjoint', 'lessThan', 'lessThanOrEquals'] as const) {
      if (c[k]) out.push(`sh:${k} ${curie(c[k] as string)}`);
    }
    logicLines(p.logic, indent, out);
    if (p.qualified) {
      out.push(`sh:qualifiedValueShape ${refTtl(p.qualified.shape, indent)}`);
      if (p.qualified.minCount != null) out.push(`sh:qualifiedMinCount ${p.qualified.minCount}`);
      if (p.qualified.maxCount != null) out.push(`sh:qualifiedMaxCount ${p.qualified.maxCount}`);
      if (p.qualified.disjoint != null) out.push(`sh:qualifiedValueShapesDisjoint ${p.qualified.disjoint}`);
    }
    if (p.severity) out.push(`sh:severity ${curie(p.severity)}`);
    if (p.message != null) out.push(`sh:message ${str(p.message)}`);
    for (const child of p.properties || []) out.push(propertyLine(child, indent));
    out.push(...ownExtras);
    return out;
  };

  const shapeBlock = (s: NodeShape): string => {
    const parts: string[] = [];
    const ownExtras = splitExtras(s.extraQuads, { termType: 'NamedNode', value: s.iri });
    for (const t of s.targets) parts.push(`sh:${TARGET_PRED[t.kind]} ${curie(t.value)}`);
    if (s.name != null) parts.push(`sh:name ${str(s.name)}`);
    if (s.description != null) parts.push(`sh:description ${str(s.description)}`);
    if (s.severity) parts.push(`sh:severity ${curie(s.severity)}`);
    if (s.message != null) parts.push(`sh:message ${str(s.message)}`);
    if (s.closed != null) parts.push(`sh:closed ${s.closed}`);
    if (s.ignoredProperties) parts.push(`sh:ignoredProperties ( ${s.ignoredProperties.map(curie).join(' ')} )`);
    logicLines(s.logic, '  ', parts);
    for (const p of s.properties) parts.push(propertyLine(p, '  '));
    parts.push(...ownExtras);
    const head = `${curie(s.iri)} a sh:NodeShape`;
    if (parts.length === 0) return `${head} .`;
    return `${head} ;\n  ${parts.join(' ;\n  ')} .`;
  };

  const blocks = g.shapes.map(shapeBlock);

  // Graph-level retained statements, grouped by subject.
  const tail: string[] = [];
  if (g.extraQuads?.length) {
    const bySubj = new Map<string, { s: ExtraTerm; rows: string[] }>();
    for (const e of g.extraQuads) {
      const k = e.s.termType + '' + e.s.value;
      let ent = bySubj.get(k);
      if (!ent) {
        ent = { s: e.s, rows: [] };
        bySubj.set(k, ent);
      }
      ent.rows.push(`${xPred(e.p)} ${xTerm(e.o)}`);
    }
    for (const { s, rows } of bySubj.values()) tail.push(`${xTerm(s)} ${rows.join(' ;\n  ')} .`);
  }

  const usedPrefixes = Object.entries(prefixes)
    .sort((a, b) => a[0].localeCompare(b[0]))
    .map(([p, ns]) => `@prefix ${p}: <${ns}> .`)
    .join('\n');
  const body = [...blocks, ...namedBlocks, ...tail, ...pool].join('\n\n');
  return `${usedPrefixes}\n\n${body}\n`;
}

function makeCurie(prefixes: Record<string, string>): (iri: string) => string {
  const entries = Object.entries(prefixes).sort((a, b) => b[1].length - a[1].length);
  return (iri: string) => {
    for (const [p, ns] of entries) {
      if (ns && iri.startsWith(ns)) {
        const local = iri.slice(ns.length);
        if (/^[A-Za-z_][A-Za-z0-9_.-]*$/.test(local)) return `${p}:${local}`;
      }
    }
    return `<${iri}>`;
  };
}

// ─── Display helpers (shared by the builder) ─────────────────────────────────

export function shortLocal(iri: string): string {
  if (!iri) return '';
  const parts = String(iri).split(/[#/]/);
  return parts[parts.length - 1] || iri;
}

/**
 * Human-readable, SPARQL-ish rendering of a property path expression,
 * e.g. `^ex:p`, `ex:a/ex:b`, `ex:a|ex:b`, `ex:p*`, `ex:p+`, `ex:p?`.
 */
export function renderPath(expr: PathExpr | string | undefined | null, curie: (iri: string) => string): string {
  if (!expr) return '';
  if (typeof expr === 'string') return curie(expr);
  const wrap = (e: PathExpr): string => (e.kind === 'predicate' ? curie(e.iri) : `(${renderPath(e, curie)})`);
  switch (expr.kind) {
    case 'predicate':
      return curie(expr.iri);
    case 'inverse':
      return '^' + wrap(expr.path);
    case 'sequence':
      return expr.paths.map(wrap).join('/');
    case 'alternative':
      return expr.paths.map(wrap).join('|');
    case 'zeroOrMore':
      return wrap(expr.path) + '*';
    case 'oneOrMore':
      return wrap(expr.path) + '+';
    case 'zeroOrOne':
      return wrap(expr.path) + '?';
  }
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
  if (p.logic) {
    for (const op of ['and', 'or', 'xone', 'not'] as const) {
      const n = p.logic[op]?.length;
      if (n) chips.push({ k: 'shape', v: `${op}(${n})` });
    }
  }
  if (p.qualified) chips.push({ k: 'shape', v: 'qualified' });
  if (p.severity) chips.push({ k: 'sev', v: shortLocal(p.severity) });
  return chips;
}

export { makeCurie };
