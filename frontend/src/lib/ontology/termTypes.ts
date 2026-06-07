// Rich, multi-language metadata for a single RDF vocabulary term, parsed from a
// bundled vocabulary file by termDictionary.ts. Unlike the ontology schema model
// (schema-model.ts), which deliberately collapses to ONE en-preferred label and
// comment, this keeps EVERY language value so the term card can show the full
// linked-data definition (the dcat:mediaType example: 9 labels, 9 comments,
// 9 definitions, scope/change/editorial notes, domain, range, …).

/** A literal value with its (possibly empty) BCP-47 language tag. */
export interface LangValue {
  lang: string; // '' when the literal carries no language tag
  value: string;
}

export type TermType =
  | 'owl:ObjectProperty'
  | 'owl:DatatypeProperty'
  | 'owl:AnnotationProperty'
  | 'rdf:Property'
  | 'owl:Class'
  | 'rdfs:Class'
  | 'skos:Concept'
  | 'rdfs:Datatype'
  | 'owl:NamedIndividual'
  | 'unknown';

export interface TermMeta {
  iri: string;
  source: string; // bundled-vocab key, e.g. 'dcat' | 'ots' — shown as provenance
  termType: TermType; // most-specific rdf:type (see TERM_TYPE_PRECEDENCE)
  allTypes: string[]; // every rdf:type IRI asserted on the term
  labels: LangValue[]; // rdfs:label (+ skos:prefLabel folded in)
  comments: LangValue[]; // rdfs:comment
  definitions: LangValue[]; // skos:definition
  scopeNotes: LangValue[]; // skos:scopeNote
  changeNotes: LangValue[]; // skos:changeNote
  editorialNotes: LangValue[]; // skos:editorialNote
  examples: LangValue[]; // skos:example
  domain: string[]; // rdfs:domain
  range: string[]; // rdfs:range
  subPropertyOf: string[]; // rdfs:subPropertyOf
  subClassOf: string[]; // rdfs:subClassOf
  inverseOf: string[]; // owl:inverseOf (collected in both directions)
  isDefinedBy: string[]; // rdfs:isDefinedBy
  seeAlso: string[]; // rdfs:seeAlso
  versionInfo: string[]; // owl:versionInfo literals
  deprecated: boolean; // owl:deprecated true
}

// Most-specific first. dcat:mediaType asserts BOTH rdf:Property and
// owl:ObjectProperty; we surface owl:ObjectProperty but keep rdf:Property in
// `allTypes` so the card can note "also: rdf:Property".
export const TERM_TYPE_PRECEDENCE: TermType[] = [
  'owl:ObjectProperty',
  'owl:DatatypeProperty',
  'owl:AnnotationProperty',
  'skos:Concept',
  'owl:Class',
  'rdfs:Class',
  'rdfs:Datatype',
  'owl:NamedIndividual',
  'rdf:Property',
];

export function emptyTermMeta(iri: string, source: string): TermMeta {
  return {
    iri,
    source,
    termType: 'unknown',
    allTypes: [],
    labels: [],
    comments: [],
    definitions: [],
    scopeNotes: [],
    changeNotes: [],
    editorialNotes: [],
    examples: [],
    domain: [],
    range: [],
    subPropertyOf: [],
    subClassOf: [],
    inverseOf: [],
    isDefinedBy: [],
    seeAlso: [],
    versionInfo: [],
    deprecated: false,
  };
}
