// Per-class view for the class hierarchy's detail pane: which properties a
// class carries (stated through rdfs:domain/range or through SHACL shapes,
// its own or inherited), which shapes apply to it, and its instances.
//
// Everything here reads the lookup tables that extractSchema builds once
// (SchemaModel.index), so one call costs the selected class and its ancestors,
// never the whole model. Call it for the selected class only, not per tree node.

import type { SchemaModel, PropertyEntry, ShapePropertyEntry } from './schema-model';

const SH_IRI = 'http://www.w3.org/ns/shacl#IRI';
const SH_BLANK_OR_IRI = 'http://www.w3.org/ns/shacl#BlankNodeOrIRI';

/** One property of the class, merged from every place that states it. */
export type FeatureRow = {
  iri: string;
  label: string;
  kind: 'attribute' | 'relation';
  /** The property's declared kind, or null when only a shape mentions it. */
  propKind: PropertyEntry['kind'] | null;
  /** The class that states it: the selected class itself or an ancestor. */
  origin: string;
  /** Stated through rdfs:domain (directly or as a member of an owl:unionOf). */
  declared: boolean;
  viaUnion: boolean;
  /** Shapes whose property shapes constrain it. */
  shapes: string[];
  /** Declared ranges plus sh:datatype / sh:class, without duplicates. */
  targets: string[];
  name?: string;
  minCount?: string;
  maxCount?: string;
  nodeKind?: string;
  inCount?: number;
  pattern?: string;
  functional: boolean;
};

/** A relation from another class that points at this one. */
export type IncomingRow = {
  iri: string;
  label: string;
  propKind: PropertyEntry['kind'] | null;
  /** The classes it starts from (its domain, or the shapes' target classes). */
  sources: string[];
  /** The class it points at: the selected class itself or an ancestor. */
  origin: string;
  declared: boolean;
  shapes: string[];
};

export type AppliedShape = {
  iri: string;
  label: string;
  origin: string;
  implicit: boolean;
  propertyCount: number;
};

/** A property shape whose path is not a single predicate (inverse, sequence…). */
export type PathConstraint = { shape: string; origin: string; prop: ShapePropertyEntry };

export type ClassDetails = {
  /** Ancestor classes, nearest first. */
  ancestors: string[];
  attributes: FeatureRow[];
  outgoing: FeatureRow[];
  incoming: IncomingRow[];
  shapes: AppliedShape[];
  pathConstraints: PathConstraint[];
};

/** Breadth-first walk over `next`, nearest first, cycle-safe. */
function walk(start: string, next: (iri: string) => string[] | undefined, cap = 5000): string[] {
  const out: string[] = [];
  const seen = new Set<string>([start]);
  let frontier = [start];
  while (frontier.length && out.length < cap) {
    const following: string[] = [];
    for (const iri of frontier) {
      for (const n of next(iri) ?? []) {
        if (seen.has(n)) continue;
        seen.add(n);
        out.push(n);
        following.push(n);
      }
    }
    frontier = following;
  }
  return out;
}

export function ancestorsOf(model: SchemaModel, iri: string): string[] {
  return walk(iri, (c) => model.classes.get(c)?.parents);
}

export function descendantsOf(model: SchemaModel, iri: string): string[] {
  return walk(iri, (c) => model.classes.get(c)?.children);
}

const byLabel = (a: { label: string; iri: string }, b: { label: string; iri: string }) =>
  (a.label || a.iri).localeCompare(b.label || b.iri);

export function classDetails(model: SchemaModel, iri: string): ClassDetails {
  const { index, properties, shapes, labels, classes } = model;
  const ancestors = ancestorsOf(model, iri);
  const origins = [iri, ...ancestors];

  const rows = new Map<string, FeatureRow>();
  const order: FeatureRow[][] = origins.map(() => []);
  const pathConstraints: PathConstraint[] = [];
  const applied: AppliedShape[] = [];
  const seenShape = new Set<string>();

  const rowFor = (prop: string, originIdx: number): FeatureRow => {
    let r = rows.get(prop);
    if (!r) {
      const p = properties.get(prop);
      r = {
        iri: prop,
        label: p?.label || labels.get(prop) || '',
        kind: 'attribute',
        propKind: p?.kind ?? null,
        origin: origins[originIdx],
        declared: false,
        viaUnion: false,
        shapes: [],
        targets: [],
        functional: p?.characteristics.has('Functional') ?? false,
      };
      rows.set(prop, r);
      order[originIdx].push(r);
    }
    return r;
  };
  const addTarget = (r: FeatureRow, v?: string) => { if (v && !r.targets.includes(v)) r.targets.push(v); };

  origins.forEach((origin, i) => {
    for (const prop of index.propsByDomain.get(origin) ?? []) {
      const p = properties.get(prop);
      const r = rowFor(prop, i);
      // The nearest class that states the property owns the row; a farther
      // ancestor stating it again adds nothing new to show.
      if (r.origin !== origin) continue;
      r.declared = true;
      if (p && !p.domain.includes(origin)) r.viaUnion = true;
      for (const t of p?.range ?? []) addTarget(r, t);
      for (const t of p?.rangeUnion ?? []) addTarget(r, t);
    }
    for (const shIri of index.shapesByClass.get(origin) ?? []) {
      if (seenShape.has(shIri)) continue;
      seenShape.add(shIri);
      const sh = shapes.get(shIri);
      if (!sh) continue;
      applied.push({
        iri: sh.iri, label: sh.label, origin,
        implicit: sh.implicitTarget && !sh.targetClass.includes(origin),
        propertyCount: sh.properties.length,
      });
      for (const pp of sh.properties) {
        if (pp.pathExpr || !pp.path) { pathConstraints.push({ shape: sh.iri, origin, prop: pp }); continue; }
        const r = rowFor(pp.path, i);
        if (!r.shapes.includes(sh.iri)) r.shapes.push(sh.iri);
        // Fill gaps only: the first (nearest) statement of a facet wins.
        r.name ??= pp.name;
        r.minCount ??= pp.minCount;
        r.maxCount ??= pp.maxCount;
        r.nodeKind ??= pp.nodeKind;
        r.inCount ??= pp.inCount;
        r.pattern ??= pp.pattern;
        addTarget(r, pp.datatype);
        addTarget(r, pp.cls);
      }
    }
  });

  // Attribute or relation: a declared kind decides; otherwise a shape's
  // sh:class / IRI node kind, or a range that is a class in this model.
  const attributes: FeatureRow[] = [];
  const outgoing: FeatureRow[] = [];
  for (const group of order) {
    group.sort(byLabel);
    for (const r of group) {
      const pk = r.propKind;
      const isRelation = pk === 'object' ? true
        : pk === 'datatype' || pk === 'annotation' ? false
        : r.nodeKind === SH_IRI || r.nodeKind === SH_BLANK_OR_IRI
          || r.targets.some(t => classes.has(t));
      r.kind = isRelation ? 'relation' : 'attribute';
      (isRelation ? outgoing : attributes).push(r);
    }
  }

  // Incoming: properties whose range is this class (or an ancestor), and
  // property shapes elsewhere whose sh:class points here.
  const incomingRows = new Map<string, IncomingRow>();
  const incoming: IncomingRow[] = [];
  const inRow = (prop: string, origin: string): IncomingRow => {
    let r = incomingRows.get(prop);
    if (!r) {
      const p = properties.get(prop);
      r = {
        iri: prop, label: p?.label || labels.get(prop) || '',
        propKind: p?.kind ?? null, sources: [], origin, declared: false, shapes: [],
      };
      incomingRows.set(prop, r);
      incoming.push(r);
    }
    return r;
  };
  const addSource = (r: IncomingRow, v: string) => { if (v && !r.sources.includes(v)) r.sources.push(v); };
  for (const origin of origins) {
    for (const prop of index.propsByRange.get(origin) ?? []) {
      const p = properties.get(prop);
      // A datatype property cannot point at a class; a range naming one is a
      // modelling slip that the diagnostics tab already reports.
      if (p?.kind === 'datatype') continue;
      const r = inRow(prop, origin);
      if (r.origin !== origin) continue;
      r.declared = true;
      for (const d of p?.domain ?? []) addSource(r, d);
      for (const d of p?.domainUnion ?? []) addSource(r, d);
    }
    for (const { shape, prop } of index.shapePropsByValueClass.get(origin) ?? []) {
      const sh = shapes.get(shape);
      if (!sh) continue;
      const r = inRow(prop.path, origin);
      if (!r.shapes.includes(shape)) r.shapes.push(shape);
      for (const tc of sh.targetClass) addSource(r, tc);
      if (sh.implicitTarget) addSource(r, sh.iri);
    }
  }

  return { ancestors, attributes, outgoing, incoming, shapes: applied, pathConstraints };
}

/**
 * Instances of a class inside the loaded graph(s): the direct ones, and those
 * of the class together with every subclass (deduplicated).
 */
export function classInstances(model: SchemaModel, iri: string): { direct: string[]; withSubclasses: string[] } {
  const direct = model.classes.get(iri)?.instances ?? [];
  const descendants = descendantsOf(model, iri);
  if (!descendants.length) return { direct, withSubclasses: direct };
  const all = new Set(direct);
  for (const d of descendants) for (const s of model.classes.get(d)?.instances ?? []) all.add(s);
  return { direct, withSubclasses: [...all] };
}
