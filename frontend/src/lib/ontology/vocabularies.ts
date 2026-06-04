// Curated vocabulary term lists for the most common RDF namespaces, used
// by SPARQL autocomplete when the user types `prefix:…`. These are NOT
// exhaustive — they cover the terms people reach for 95% of the time.
// Add to them as needed. The shape matches what sparqlCompletion.js expects:
//   { iri, label, kind, comment }
// where kind ∈ 'class' | 'object' | 'datatype' | 'annotation' | 'property'

export interface VocabTerm {
  iri: string;
  label: string;
  kind: string;
  comment: string;
}

const mk = (ns: string, local: string, label: string, kind: string, comment = ''): VocabTerm => ({
  iri: ns + local, label, kind, comment,
});

export const NAMESPACES = {
  rdf:    'http://www.w3.org/1999/02/22-rdf-syntax-ns#',
  rdfs:   'http://www.w3.org/2000/01/rdf-schema#',
  owl:    'http://www.w3.org/2002/07/owl#',
  sh:     'http://www.w3.org/ns/shacl#',
  xsd:    'http://www.w3.org/2001/XMLSchema#',
  skos:   'http://www.w3.org/2004/02/skos/core#',
  foaf:   'http://xmlns.com/foaf/0.1/',
  schema: 'http://schema.org/',
  dct:    'http://purl.org/dc/terms/',
  dcat:   'http://www.w3.org/ns/dcat#',
  prov:   'http://www.w3.org/ns/prov#',
  geo:    'http://www.opengis.net/ont/geosparql#',
  time:   'http://www.w3.org/2006/time#',
  void:   'http://rdfs.org/ns/void#',
  vann:   'http://purl.org/vocab/vann/',
  org:    'http://www.w3.org/ns/org#',
  qb:     'http://purl.org/linked-data/cube#',
};

// prefix → [term]
export const VOCAB: Record<string, VocabTerm[]> = {};

const P: Record<string, string> = {};
for (const [p, ns] of Object.entries(NAMESPACES)) P[p] = ns;

// ---- rdf / rdfs / owl ------------------------------------------------------
VOCAB.rdf = [
  mk(P.rdf, 'type', 'type', 'property', 'The subject is an instance of a class.'),
  mk(P.rdf, 'Property', 'Property', 'class'),
  mk(P.rdf, 'Statement', 'Statement', 'class'),
  mk(P.rdf, 'subject', 'subject', 'property'),
  mk(P.rdf, 'predicate', 'predicate', 'property'),
  mk(P.rdf, 'object', 'object', 'property'),
  mk(P.rdf, 'List', 'List', 'class'),
  mk(P.rdf, 'first', 'first', 'property'),
  mk(P.rdf, 'rest', 'rest', 'property'),
  mk(P.rdf, 'nil', 'nil', 'class'),
  mk(P.rdf, 'langString', 'langString', 'datatype'),
  mk(P.rdf, 'HTML', 'HTML', 'datatype'),
  mk(P.rdf, 'JSON', 'JSON', 'datatype'),
  mk(P.rdf, 'value', 'value', 'property'),
];
VOCAB.rdfs = [
  mk(P.rdfs, 'Class', 'Class', 'class'),
  mk(P.rdfs, 'Resource', 'Resource', 'class'),
  mk(P.rdfs, 'Literal', 'Literal', 'class'),
  mk(P.rdfs, 'Datatype', 'Datatype', 'class'),
  mk(P.rdfs, 'label', 'label', 'annotation', 'A human-readable name.'),
  mk(P.rdfs, 'comment', 'comment', 'annotation', 'A human-readable description.'),
  mk(P.rdfs, 'subClassOf', 'subClassOf', 'object'),
  mk(P.rdfs, 'subPropertyOf', 'subPropertyOf', 'object'),
  mk(P.rdfs, 'domain', 'domain', 'object'),
  mk(P.rdfs, 'range', 'range', 'object'),
  mk(P.rdfs, 'seeAlso', 'seeAlso', 'annotation'),
  mk(P.rdfs, 'isDefinedBy', 'isDefinedBy', 'annotation'),
];
VOCAB.owl = [
  mk(P.owl, 'Class', 'Class', 'class'),
  mk(P.owl, 'ObjectProperty', 'ObjectProperty', 'class'),
  mk(P.owl, 'DatatypeProperty', 'DatatypeProperty', 'class'),
  mk(P.owl, 'AnnotationProperty', 'AnnotationProperty', 'class'),
  mk(P.owl, 'Ontology', 'Ontology', 'class'),
  mk(P.owl, 'NamedIndividual', 'NamedIndividual', 'class'),
  mk(P.owl, 'Thing', 'Thing', 'class'),
  mk(P.owl, 'Nothing', 'Nothing', 'class'),
  mk(P.owl, 'Restriction', 'Restriction', 'class'),
  mk(P.owl, 'equivalentClass', 'equivalentClass', 'object'),
  mk(P.owl, 'equivalentProperty', 'equivalentProperty', 'object'),
  mk(P.owl, 'sameAs', 'sameAs', 'object'),
  mk(P.owl, 'differentFrom', 'differentFrom', 'object'),
  mk(P.owl, 'disjointWith', 'disjointWith', 'object'),
  mk(P.owl, 'inverseOf', 'inverseOf', 'object'),
  mk(P.owl, 'imports', 'imports', 'object'),
  mk(P.owl, 'versionInfo', 'versionInfo', 'annotation'),
  mk(P.owl, 'deprecated', 'deprecated', 'annotation'),
  mk(P.owl, 'onProperty', 'onProperty', 'object'),
  mk(P.owl, 'someValuesFrom', 'someValuesFrom', 'object'),
  mk(P.owl, 'allValuesFrom', 'allValuesFrom', 'object'),
  mk(P.owl, 'hasValue', 'hasValue', 'object'),
  mk(P.owl, 'cardinality', 'cardinality', 'datatype'),
  mk(P.owl, 'minCardinality', 'minCardinality', 'datatype'),
  mk(P.owl, 'maxCardinality', 'maxCardinality', 'datatype'),
  mk(P.owl, 'FunctionalProperty', 'FunctionalProperty', 'class'),
  mk(P.owl, 'InverseFunctionalProperty', 'InverseFunctionalProperty', 'class'),
  mk(P.owl, 'TransitiveProperty', 'TransitiveProperty', 'class'),
  mk(P.owl, 'SymmetricProperty', 'SymmetricProperty', 'class'),
];

// ---- SHACL ------------------------------------------------------------------
VOCAB.sh = [
  mk(P.sh, 'NodeShape', 'NodeShape', 'class', 'A node shape describes a focus node.'),
  mk(P.sh, 'PropertyShape', 'PropertyShape', 'class'),
  mk(P.sh, 'Shape', 'Shape', 'class'),
  mk(P.sh, 'ValidationReport', 'ValidationReport', 'class'),
  mk(P.sh, 'ValidationResult', 'ValidationResult', 'class'),
  mk(P.sh, 'targetClass', 'targetClass', 'object'),
  mk(P.sh, 'targetNode', 'targetNode', 'object'),
  mk(P.sh, 'targetSubjectsOf', 'targetSubjectsOf', 'object'),
  mk(P.sh, 'targetObjectsOf', 'targetObjectsOf', 'object'),
  mk(P.sh, 'property', 'property', 'object'),
  mk(P.sh, 'path', 'path', 'object'),
  mk(P.sh, 'class', 'class', 'object'),
  mk(P.sh, 'datatype', 'datatype', 'object'),
  mk(P.sh, 'nodeKind', 'nodeKind', 'object'),
  mk(P.sh, 'minCount', 'minCount', 'datatype'),
  mk(P.sh, 'maxCount', 'maxCount', 'datatype'),
  mk(P.sh, 'minLength', 'minLength', 'datatype'),
  mk(P.sh, 'maxLength', 'maxLength', 'datatype'),
  mk(P.sh, 'minInclusive', 'minInclusive', 'datatype'),
  mk(P.sh, 'maxInclusive', 'maxInclusive', 'datatype'),
  mk(P.sh, 'minExclusive', 'minExclusive', 'datatype'),
  mk(P.sh, 'maxExclusive', 'maxExclusive', 'datatype'),
  mk(P.sh, 'pattern', 'pattern', 'datatype'),
  mk(P.sh, 'flags', 'flags', 'datatype'),
  mk(P.sh, 'languageIn', 'languageIn', 'object'),
  mk(P.sh, 'uniqueLang', 'uniqueLang', 'datatype'),
  mk(P.sh, 'in', 'in', 'object'),
  mk(P.sh, 'hasValue', 'hasValue', 'object'),
  mk(P.sh, 'node', 'node', 'object'),
  mk(P.sh, 'and', 'and', 'object'),
  mk(P.sh, 'or', 'or', 'object'),
  mk(P.sh, 'not', 'not', 'object'),
  mk(P.sh, 'xone', 'xone', 'object'),
  mk(P.sh, 'severity', 'severity', 'object'),
  mk(P.sh, 'message', 'message', 'annotation'),
  mk(P.sh, 'name', 'name', 'annotation'),
  mk(P.sh, 'description', 'description', 'annotation'),
  mk(P.sh, 'deactivated', 'deactivated', 'datatype'),
  mk(P.sh, 'Violation', 'Violation', 'class'),
  mk(P.sh, 'Warning', 'Warning', 'class'),
  mk(P.sh, 'Info', 'Info', 'class'),
  mk(P.sh, 'IRI', 'IRI', 'class'),
  mk(P.sh, 'Literal', 'Literal', 'class'),
  mk(P.sh, 'BlankNode', 'BlankNode', 'class'),
  mk(P.sh, 'IRIOrLiteral', 'IRIOrLiteral', 'class'),
  mk(P.sh, 'select', 'select', 'annotation'),
  mk(P.sh, 'ask', 'ask', 'annotation'),
  mk(P.sh, 'focusNode', 'focusNode', 'object'),
  mk(P.sh, 'resultPath', 'resultPath', 'object'),
  mk(P.sh, 'sourceShape', 'sourceShape', 'object'),
  mk(P.sh, 'sourceConstraintComponent', 'sourceConstraintComponent', 'object'),
  mk(P.sh, 'conforms', 'conforms', 'datatype'),
  mk(P.sh, 'inversePath', 'inversePath', 'object'),
  mk(P.sh, 'alternativePath', 'alternativePath', 'object'),
  mk(P.sh, 'zeroOrMorePath', 'zeroOrMorePath', 'object'),
  mk(P.sh, 'oneOrMorePath', 'oneOrMorePath', 'object'),
];

// ---- XSD datatypes ---------------------------------------------------------
VOCAB.xsd = [
  'string', 'boolean', 'integer', 'decimal', 'float', 'double',
  'date', 'dateTime', 'time', 'gYear', 'gYearMonth', 'duration',
  'anyURI', 'base64Binary', 'hexBinary', 'long', 'int', 'short', 'byte',
  'nonNegativeInteger', 'positiveInteger', 'negativeInteger',
  'unsignedLong', 'unsignedInt', 'token', 'language', 'Name', 'normalizedString',
].map(local => mk(P.xsd, local, local, 'datatype'));

// ---- SKOS ------------------------------------------------------------------
VOCAB.skos = [
  mk(P.skos, 'Concept', 'Concept', 'class'),
  mk(P.skos, 'ConceptScheme', 'ConceptScheme', 'class'),
  mk(P.skos, 'Collection', 'Collection', 'class'),
  mk(P.skos, 'OrderedCollection', 'OrderedCollection', 'class'),
  mk(P.skos, 'prefLabel', 'prefLabel', 'annotation'),
  mk(P.skos, 'altLabel', 'altLabel', 'annotation'),
  mk(P.skos, 'hiddenLabel', 'hiddenLabel', 'annotation'),
  mk(P.skos, 'definition', 'definition', 'annotation'),
  mk(P.skos, 'note', 'note', 'annotation'),
  mk(P.skos, 'scopeNote', 'scopeNote', 'annotation'),
  mk(P.skos, 'example', 'example', 'annotation'),
  mk(P.skos, 'broader', 'broader', 'object'),
  mk(P.skos, 'narrower', 'narrower', 'object'),
  mk(P.skos, 'related', 'related', 'object'),
  mk(P.skos, 'broaderTransitive', 'broaderTransitive', 'object'),
  mk(P.skos, 'narrowerTransitive', 'narrowerTransitive', 'object'),
  mk(P.skos, 'inScheme', 'inScheme', 'object'),
  mk(P.skos, 'hasTopConcept', 'hasTopConcept', 'object'),
  mk(P.skos, 'topConceptOf', 'topConceptOf', 'object'),
  mk(P.skos, 'member', 'member', 'object'),
  mk(P.skos, 'exactMatch', 'exactMatch', 'object'),
  mk(P.skos, 'closeMatch', 'closeMatch', 'object'),
  mk(P.skos, 'broadMatch', 'broadMatch', 'object'),
  mk(P.skos, 'narrowMatch', 'narrowMatch', 'object'),
  mk(P.skos, 'relatedMatch', 'relatedMatch', 'object'),
  mk(P.skos, 'notation', 'notation', 'datatype'),
];

// ---- FOAF ------------------------------------------------------------------
VOCAB.foaf = [
  'Person', 'Agent', 'Group', 'Organization', 'Document', 'Image',
].map(l => mk(P.foaf, l, l, 'class')).concat([
  'name', 'givenName', 'familyName', 'mbox', 'homepage', 'img', 'depiction',
  'knows', 'member', 'account', 'nick', 'title', 'age',
].map(l => mk(P.foaf, l, l, 'property')));

// ---- schema.org (subset) ---------------------------------------------------
VOCAB.schema = [
  'Thing', 'Person', 'Organization', 'Place', 'Event', 'CreativeWork',
  'Article', 'BlogPosting', 'Product', 'Offer', 'Review', 'Dataset',
].map(l => mk(P.schema, l, l, 'class')).concat([
  'name', 'description', 'url', 'image', 'author', 'dateCreated',
  'dateModified', 'datePublished', 'identifier', 'sameAs', 'email',
  'telephone', 'address', 'keywords', 'inLanguage',
].map(l => mk(P.schema, l, l, 'property')));

// ---- Dublin Core Terms -----------------------------------------------------
VOCAB.dct = [
  'title', 'description', 'creator', 'contributor', 'publisher',
  'created', 'modified', 'issued', 'date', 'identifier', 'license',
  'rights', 'source', 'subject', 'type', 'format', 'language',
  'hasPart', 'isPartOf', 'references', 'isReferencedBy', 'conformsTo',
  'spatial', 'temporal',
].map(l => mk(P.dct, l, l, 'property'));

// ---- DCAT ------------------------------------------------------------------
VOCAB.dcat = [
  mk(P.dcat, 'Catalog', 'Catalog', 'class'),
  mk(P.dcat, 'Dataset', 'Dataset', 'class'),
  mk(P.dcat, 'Distribution', 'Distribution', 'class'),
  mk(P.dcat, 'DataService', 'DataService', 'class'),
  mk(P.dcat, 'Resource', 'Resource', 'class'),
  mk(P.dcat, 'dataset', 'dataset', 'object'),
  mk(P.dcat, 'distribution', 'distribution', 'object'),
  mk(P.dcat, 'downloadURL', 'downloadURL', 'object'),
  mk(P.dcat, 'accessURL', 'accessURL', 'object'),
  mk(P.dcat, 'mediaType', 'mediaType', 'object'),
  mk(P.dcat, 'byteSize', 'byteSize', 'datatype'),
  mk(P.dcat, 'keyword', 'keyword', 'datatype'),
  mk(P.dcat, 'theme', 'theme', 'object'),
  mk(P.dcat, 'landingPage', 'landingPage', 'object'),
  mk(P.dcat, 'contactPoint', 'contactPoint', 'object'),
  mk(P.dcat, 'endpointURL', 'endpointURL', 'object'),
];

// ---- PROV ------------------------------------------------------------------
VOCAB.prov = [
  mk(P.prov, 'Entity', 'Entity', 'class'),
  mk(P.prov, 'Activity', 'Activity', 'class'),
  mk(P.prov, 'Agent', 'Agent', 'class'),
  mk(P.prov, 'Person', 'Person', 'class'),
  mk(P.prov, 'Organization', 'Organization', 'class'),
  mk(P.prov, 'wasGeneratedBy', 'wasGeneratedBy', 'object'),
  mk(P.prov, 'wasDerivedFrom', 'wasDerivedFrom', 'object'),
  mk(P.prov, 'wasAttributedTo', 'wasAttributedTo', 'object'),
  mk(P.prov, 'used', 'used', 'object'),
  mk(P.prov, 'startedAtTime', 'startedAtTime', 'datatype'),
  mk(P.prov, 'endedAtTime', 'endedAtTime', 'datatype'),
];

// ---- GeoSPARQL -------------------------------------------------------------
VOCAB.geo = [
  mk(P.geo, 'Feature', 'Feature', 'class'),
  mk(P.geo, 'Geometry', 'Geometry', 'class'),
  mk(P.geo, 'SpatialObject', 'SpatialObject', 'class'),
  mk(P.geo, 'hasGeometry', 'hasGeometry', 'object'),
  mk(P.geo, 'asWKT', 'asWKT', 'datatype'),
  mk(P.geo, 'wktLiteral', 'wktLiteral', 'datatype'),
  mk(P.geo, 'sfContains', 'sfContains', 'object'),
  mk(P.geo, 'sfWithin', 'sfWithin', 'object'),
  mk(P.geo, 'sfIntersects', 'sfIntersects', 'object'),
];

/** Flat array of all built-in terms across all namespaces. */
export function allBuiltinTerms() {
  const out = [];
  for (const list of Object.values(VOCAB)) out.push(...list);
  return out;
}

/** Get built-in terms by namespace IRI. */
export function termsByNamespace(ns) {
  for (const [prefix, nsIri] of Object.entries(NAMESPACES)) {
    if (nsIri === ns) return VOCAB[prefix] || [];
  }
  return [];
}

// ---------------------------------------------------------------------------
// Vocabulary classification — used by the schema viewer to group by namespace
// ---------------------------------------------------------------------------

export type VocabKind =
  | 'rdf' | 'rdfs' | 'owl' | 'sh' | 'xsd'
  | 'skos' | 'dcterms' | 'foaf' | 'schema' | 'void'
  | 'dcat' | 'prov' | 'geo' | 'time' | 'org' | 'qb' | 'vann'
  | 'custom';

const KIND_BY_NS: Array<[string, VocabKind]> = [
  [NAMESPACES.rdf, 'rdf'],
  [NAMESPACES.rdfs, 'rdfs'],
  [NAMESPACES.owl, 'owl'],
  [NAMESPACES.sh, 'sh'],
  [NAMESPACES.xsd, 'xsd'],
  [NAMESPACES.skos, 'skos'],
  [NAMESPACES.dct, 'dcterms'],
  ['http://purl.org/dc/elements/1.1/', 'dcterms'],
  [NAMESPACES.foaf, 'foaf'],
  [NAMESPACES.schema, 'schema'],
  ['https://schema.org/', 'schema'],
  [NAMESPACES.void, 'void'],
  [NAMESPACES.dcat, 'dcat'],
  [NAMESPACES.prov, 'prov'],
  [NAMESPACES.geo, 'geo'],
  [NAMESPACES.time, 'time'],
  [NAMESPACES.org, 'org'],
  [NAMESPACES.qb, 'qb'],
  [NAMESPACES.vann, 'vann'],
];

/** Split an IRI into (namespace, local) at the last `#` or `/`. */
export function splitIri(iri: string): { ns: string; local: string } {
  if (!iri) return { ns: '', local: '' };
  const hash = iri.lastIndexOf('#');
  const slash = iri.lastIndexOf('/');
  const i = Math.max(hash, slash);
  if (i < 0) return { ns: '', local: iri };
  return { ns: iri.slice(0, i + 1), local: iri.slice(i + 1) };
}

/** Classify an IRI to one of the known vocab "kind" tags, or 'custom'. */
export function kindOf(iri: string): VocabKind {
  if (!iri) return 'custom';
  for (const [ns, kind] of KIND_BY_NS) {
    if (ns && iri.startsWith(ns)) return kind;
  }
  return 'custom';
}

/** Best-effort prefix label for a namespace IRI (e.g. "skos"). */
export function prefixForNamespace(ns: string): string | null {
  for (const [prefix, nsIri] of Object.entries(NAMESPACES)) {
    if (nsIri === ns) return prefix;
  }
  return null;
}
