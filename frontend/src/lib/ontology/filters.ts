// Filter state shared by every schema-viewer panel. Pure helpers, no Svelte
// dependency, so they can be unit-tested in isolation.

import type { SchemaModel, ClassEntry, PropertyEntry, ConceptEntry } from './schema-model';
import { kindOf, type VocabKind } from './vocabularies';

export type EntityKind =
  | 'class' | 'objectProperty' | 'datatypeProperty' | 'annotationProperty'
  | 'rdfProperty' | 'concept' | 'conceptScheme' | 'shape';

export type UsageFlag =
  | 'hasInstances' | 'hasSubclasses' | 'isShapeTarget' | 'hasAxioms' | 'leaf';

export type FilterState = {
  vocabs: Set<VocabKind>;     // empty = no vocab filter (show all)
  kinds: Set<EntityKind>;     // empty = all kinds
  usage: Set<UsageFlag>;      // empty = no usage filter
  text: string;
  iriRegex: string;
};

export const ALL_VOCABS: VocabKind[] = [
  'owl', 'rdfs', 'rdf', 'sh', 'skos', 'dcterms',
  'foaf', 'schema', 'void', 'dcat', 'prov', 'geo', 'custom',
];

export function emptyFilter(): FilterState {
  return {
    vocabs: new Set(),
    kinds: new Set(),
    usage: new Set(),
    text: '',
    iriRegex: '',
  };
}

function compileRegex(s: string): RegExp | null {
  if (!s) return null;
  try { return new RegExp(s); } catch { return null; }
}

function textMatches(text: string, label: string, iri: string, comment: string): boolean {
  if (!text) return true;
  const t = text.toLowerCase();
  return (
    (label || '').toLowerCase().includes(t) ||
    (iri || '').toLowerCase().includes(t) ||
    (comment || '').toLowerCase().includes(t)
  );
}

function vocabMatches(state: FilterState, iri: string): boolean {
  if (!state.vocabs.size) return true;
  return state.vocabs.has(kindOf(iri));
}

function regexMatches(re: RegExp | null, iri: string): boolean {
  if (!re) return true;
  return re.test(iri);
}

export type FilteredView = {
  classes: ClassEntry[];
  properties: PropertyEntry[];
  concepts: ConceptEntry[];
  schemes: SchemaModel['schemes'];
  namespaces: SchemaModel['namespaces'];
};

function classHasAxioms(c: ClassEntry): boolean {
  return (c.equivalents?.length ?? 0) > 0
      || (c.disjoints?.length ?? 0) > 0
      || (c.unionOf?.length ?? 0) > 0
      || (c.intersectionOf?.length ?? 0) > 0
      || !!c.complementOf
      || (c.oneOf?.length ?? 0) > 0
      || (c.hasKey?.length ?? 0) > 0
      || (c.restrictions?.length ?? 0) > 0;
}

function propHasAxioms(p: PropertyEntry): boolean {
  return (p.superProperties?.length ?? 0) > 0
      || (p.equivalentProperty?.length ?? 0) > 0
      || (p.inverseOf?.length ?? 0) > 0
      || (p.chains?.length ?? 0) > 0
      || (p.characteristics?.size ?? 0) > 0;
}

export function applyFilter(model: SchemaModel, state: FilterState): FilteredView {
  const re = compileRegex(state.iriRegex);
  const wantsKind = (k: EntityKind) => !state.kinds.size || state.kinds.has(k);

  const classes = [...model.classes.values()].filter(c => {
    if (!wantsKind('class')) return false;
    if (!vocabMatches(state, c.iri)) return false;
    if (!regexMatches(re, c.iri)) return false;
    if (!textMatches(state.text, c.label, c.iri, c.comment)) return false;
    if (state.usage.size) {
      if (state.usage.has('hasInstances') && !((c.instanceCount ?? 0) > 0)) return false;
      if (state.usage.has('hasSubclasses') && !((c.children?.length ?? 0) > 0)) return false;
      if (state.usage.has('isShapeTarget') && !c.hasShape) return false;
      if (state.usage.has('hasAxioms') && !classHasAxioms(c)) return false;
      if (state.usage.has('leaf') && (c.children?.length ?? 0) > 0) return false;
    }
    return true;
  });

  const properties = [...model.properties.values()].filter(p => {
    const k = p.kind === 'object' ? 'objectProperty'
            : p.kind === 'datatype' ? 'datatypeProperty'
            : p.kind === 'annotation' ? 'annotationProperty'
            : 'rdfProperty';
    if (!wantsKind(k as EntityKind)) return false;
    if (!vocabMatches(state, p.iri)) return false;
    if (!regexMatches(re, p.iri)) return false;
    if (!textMatches(state.text, p.label, p.iri, p.comment)) return false;
    if (state.usage.has('hasAxioms') && !propHasAxioms(p)) return false;
    return true;
  });

  const concepts = [...model.concepts.values()].filter(c => {
    if (!wantsKind('concept')) return false;
    if (!vocabMatches(state, c.iri)) return false;
    if (!regexMatches(re, c.iri)) return false;
    if (!textMatches(state.text, c.prefLabel, c.iri, '')) return false;
    return true;
  });

  return { classes, properties, concepts, schemes: model.schemes, namespaces: model.namespaces };
}
