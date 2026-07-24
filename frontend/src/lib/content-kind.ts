// Probe one or more named graphs to estimate whether they contain
// model (T-Box: classes), vocabulary (R-Box: properties/relations + SKOS),
// shapes (SHACL), entailment rules (SWRL/SPIN), or instance data (A-Box).
//
// Returns: {
//   classCount, propertyCount, shapeCount, entailmentCount,  // OWL/RDFS/SHACL/SWRL signals
//   skosSchemeCount, skosConceptCount,                       // SKOS signals
//   instanceCount, sampleTypes,
//   verdict: 'model' | 'shapes' | 'vocabulary' | 'entailment' | 'instances' | 'mixed' | 'empty'
// }

import { sparqlQuery, browseFacets } from './api.js';

export type ContentKindVerdict = 'model' | 'shapes' | 'vocabulary' | 'entailment' | 'instances' | 'mixed' | 'empty';

export interface ContentKindSignals {
  classCount: number;
  propertyCount: number;
  shapeCount: number;
  skosSchemeCount: number;
  skosConceptCount: number;
  entailmentCount: number;
  instanceCount: number;
}

// Mirror the backend detector (src/kind_detector.rs): Model = classes (T-Box);
// Vocabulary = properties/relations (R-Box) + SKOS concept schemes; ties between
// class and vocabulary content go to Model (the class hierarchy anchors the schema).
// Shared by the (heavy) graph probe and the (cheap) dataset-facet path so both
// classify identically.
export function classifyVerdict(s: ContentKindSignals): ContentKindVerdict {
  const classSignal = s.classCount;
  const vocabSignal = s.propertyCount + s.skosSchemeCount + s.skosConceptCount;
  const schemaSignal = classSignal + vocabSignal + s.shapeCount;
  const hasClasses = s.classCount > 0;

  if (schemaSignal === 0 && s.instanceCount === 0 && s.entailmentCount === 0) return 'empty';
  if (s.entailmentCount > 0 && s.entailmentCount >= Math.max(classSignal, vocabSignal, s.shapeCount, s.instanceCount)) return 'entailment';
  if (s.shapeCount > 0 && !hasClasses && s.shapeCount >= vocabSignal * 3 && s.shapeCount >= s.instanceCount * 3) return 'shapes';
  if (s.instanceCount > schemaSignal * 3) return 'instances';
  if (vocabSignal > 0 && !hasClasses && s.instanceCount < vocabSignal) return 'vocabulary';
  if (classSignal > 0 || vocabSignal > 0) {
    if (classSignal >= vocabSignal) return s.instanceCount <= Math.max(classSignal, s.shapeCount) ? 'model' : 'mixed';
    return s.instanceCount <= Math.max(vocabSignal, s.shapeCount) ? 'vocabulary' : 'mixed';
  }
  return 'mixed';
}

// ── Cheap, always-on content summary from the browse facets ──────────────────
// The facet endpoint returns per-type `?s a ?cls` counts (COUNT(DISTINCT ?s))
// straight from the store — index-backed for graphs, and a single concurrent
// scan for classes/properties — so it works on a multi-million-triple dataset in
// ~1s where the old FILTER-NOT-EXISTS probe took 30-60s (or was skipped entirely).
// A type IRI that is itself a schema meta-type tells us the dataset carries
// definitions (T-Box/R-Box/SHACL/SKOS/SWRL); everything else is instance data.
const RDFS = 'http://www.w3.org/2000/01/rdf-schema#';
const OWL = 'http://www.w3.org/2002/07/owl#';
const RDF = 'http://www.w3.org/1999/02/22-rdf-syntax-ns#';
const SH = 'http://www.w3.org/ns/shacl#';
const SKOS = 'http://www.w3.org/2004/02/skos/core#';
const SWRL = 'http://www.w3.org/2003/11/swrl#';
const CLASS_META = new Set([RDFS + 'Class', OWL + 'Class']);
const PROP_META = new Set([RDF + 'Property', OWL + 'ObjectProperty', OWL + 'DatatypeProperty', OWL + 'AnnotationProperty']);
const SHAPE_META = new Set([SH + 'NodeShape', SH + 'PropertyShape']);
const IGNORE_META = new Set([OWL + 'Ontology', OWL + 'NamedIndividual']); // headers, not instances

export interface DatasetContentSummary extends ContentKindSignals {
  verdict: ContentKindVerdict;
  /** Top instance types (non-meta) by count — the "what's in here" chips. */
  sampleTypes: { cls: string; count: number }[];
  /** Distinct instance types (non-meta classes) present. */
  instanceTypeCount: number;
  /** Distinct predicates used in the dataset. */
  predicateCount: number;
  /** True if the facet lists hit their server cap (counts are lower bounds). */
  capped: boolean;
}

/**
 * A lightweight content summary for a dataset, derived from `/api/browse/facets`.
 * Unlike `probeContentKind` this never scans the store with FILTER-NOT-EXISTS, so
 * it is cheap enough to run on every dataset page regardless of size.
 */
export async function datasetContentKind(datasetId: string, signal?: AbortSignal): Promise<DatasetContentSummary> {
  const facets = await browseFacets({ dataset_id: datasetId }, { signal });
  const classes: { iri: string; count: number }[] = facets?.classes || [];
  const properties: { iri: string; count: number }[] = facets?.properties || [];

  let classCount = 0, propertyCount = 0, shapeCount = 0, skosSchemeCount = 0, skosConceptCount = 0, entailmentCount = 0, instanceCount = 0;
  const instanceTypes: { cls: string; count: number }[] = [];
  for (const c of classes) {
    const n = Number(c.count) || 0;
    if (CLASS_META.has(c.iri)) classCount += n;
    else if (PROP_META.has(c.iri)) propertyCount += n;
    else if (SHAPE_META.has(c.iri)) shapeCount += n;
    else if (c.iri === SKOS + 'ConceptScheme') skosSchemeCount += n;
    else if (c.iri === SKOS + 'Concept') skosConceptCount += n;
    else if (c.iri === SWRL + 'Imp') entailmentCount += n;
    else if (IGNORE_META.has(c.iri)) { /* ontology header / bare individual — not a distinct instance kind */ }
    else { instanceCount += n; instanceTypes.push({ cls: c.iri, count: n }); }
  }
  const signals = { classCount, propertyCount, shapeCount, skosSchemeCount, skosConceptCount, entailmentCount, instanceCount };
  return {
    ...signals,
    verdict: classifyVerdict(signals),
    sampleTypes: instanceTypes.slice(0, 6), // facet is already sorted DESC by count
    instanceTypeCount: instanceTypes.length,
    predicateCount: properties.length,
    // browse_facets caps classes/properties at 300; hitting it means the sums are lower bounds.
    capped: classes.length >= 300 || properties.length >= 300,
  };
}

function fromClauses(graphs: string[]): string {
  if (!graphs?.length) return '';
  return graphs.map(g => `FROM <${g}>`).join('\n');
}

// Returns the count, or RETHROWS on abort / HTTP error. Returning 0 on failure was
// the bug behind the spurious "model-in-dataset" banner: a timed-out instanceCount
// of 0 combined with a real classCount yielded verdict 'model'. The caller catches
// and returns the neutral empty shape instead.
async function askCount(query: string, signal?: AbortSignal): Promise<number> {
  const res = await sparqlQuery(query, { signal });
  const binding = res?.results?.bindings?.[0];
  return parseInt(binding?.n?.value || '0', 10);
}

const EMPTY_PROBE = {
  classCount: 0, propertyCount: 0, shapeCount: 0, entailmentCount: 0,
  skosSchemeCount: 0, skosConceptCount: 0, instanceCount: 0,
  verdict: 'empty' as ContentKindVerdict, sampleTypes: [] as { cls: string; count: number }[],
};

export async function probeContentKind(graphs: string[], signal?: AbortSignal) {
  const from = fromClauses(graphs);
  if (!from) return { ...EMPTY_PROBE };

  try {
    return await probeContentKindInner(graphs, from, signal);
  } catch {
    // Abort or query error → neutral result, so a partial/failed probe never
    // produces a misleading verdict. (Return, don't rethrow: callers treat this as
    // "no signal", which suppresses the banner.)
    return { ...EMPTY_PROBE };
  }
}

async function probeContentKindInner(graphs: string[], from: string, signal?: AbortSignal) {
  let classCount = 0, propertyCount = 0, shapeCount = 0, skosSchemeCount = 0, skosConceptCount = 0, entailmentCount = 0, instanceCount = 0;
  {
    const combined = await sparqlQuery(`SELECT
      (COUNT(DISTINCT ?c)  AS ?classes)
      (COUNT(DISTINCT ?p)  AS ?props)
      (COUNT(DISTINCT ?sh) AS ?shapes)
      (COUNT(DISTINCT ?sc) AS ?schemes)
      (COUNT(DISTINCT ?sk) AS ?concepts)
      (COUNT(DISTINCT ?en) AS ?entailments)
      ${from}
    WHERE {
      OPTIONAL { ?c a ?ct . FILTER(?ct IN (<http://www.w3.org/2000/01/rdf-schema#Class>, <http://www.w3.org/2002/07/owl#Class>)) }
      OPTIONAL { ?p a ?pt . FILTER(?pt IN (<http://www.w3.org/1999/02/22-rdf-syntax-ns#Property>, <http://www.w3.org/2002/07/owl#ObjectProperty>, <http://www.w3.org/2002/07/owl#DatatypeProperty>, <http://www.w3.org/2002/07/owl#AnnotationProperty>)) }
      OPTIONAL { ?sh a <http://www.w3.org/ns/shacl#NodeShape> }
      OPTIONAL { ?sc a <http://www.w3.org/2004/02/skos/core#ConceptScheme> }
      OPTIONAL { ?sk a <http://www.w3.org/2004/02/skos/core#Concept> }
      OPTIONAL { ?en a <http://www.w3.org/2003/11/swrl#Imp> }
    }`, { signal });
    const b = combined?.results?.bindings?.[0];
    if (b) {
      classCount      = parseInt(b.classes?.value     || '0', 10);
      propertyCount   = parseInt(b.props?.value       || '0', 10);
      shapeCount      = parseInt(b.shapes?.value      || '0', 10);
      skosSchemeCount = parseInt(b.schemes?.value     || '0', 10);
      skosConceptCount= parseInt(b.concepts?.value    || '0', 10);
      entailmentCount = parseInt(b.entailments?.value || '0', 10);
    }
  }

  // Instance count as a separate query (complex FILTER NOT EXISTS can't inline easily).
  instanceCount = await askCount(`SELECT (COUNT(DISTINCT ?s) AS ?n) ${from} WHERE {
    ?s a ?t .
    FILTER NOT EXISTS { ?s a <http://www.w3.org/2000/01/rdf-schema#Class> }
    FILTER NOT EXISTS { ?s a <http://www.w3.org/2002/07/owl#Class> }
    FILTER NOT EXISTS { ?s a <http://www.w3.org/1999/02/22-rdf-syntax-ns#Property> }
    FILTER NOT EXISTS { ?s a <http://www.w3.org/2002/07/owl#ObjectProperty> }
    FILTER NOT EXISTS { ?s a <http://www.w3.org/2002/07/owl#DatatypeProperty> }
    FILTER NOT EXISTS { ?s a <http://www.w3.org/2002/07/owl#AnnotationProperty> }
    FILTER NOT EXISTS { ?s a <http://www.w3.org/ns/shacl#NodeShape> }
    FILTER NOT EXISTS { ?s a <http://www.w3.org/ns/shacl#PropertyShape> }
    FILTER NOT EXISTS { ?s a <http://www.w3.org/2002/07/owl#Ontology> }
    FILTER NOT EXISTS { ?s a <http://www.w3.org/2004/02/skos/core#ConceptScheme> }
    FILTER NOT EXISTS { ?s a <http://www.w3.org/2004/02/skos/core#Concept> }
  }`, signal);

  // Sample the top-5 instance types for display
  let sampleTypes = [];
  {
    const res = await sparqlQuery(`SELECT ?t (COUNT(DISTINCT ?s) AS ?n) ${from} WHERE {
      ?s a ?t .
      FILTER(ISIRI(?t))
      FILTER NOT EXISTS { ?s a <http://www.w3.org/2000/01/rdf-schema#Class> }
      FILTER NOT EXISTS { ?s a <http://www.w3.org/2002/07/owl#Class> }
      FILTER NOT EXISTS { ?s a <http://www.w3.org/1999/02/22-rdf-syntax-ns#Property> }
      FILTER NOT EXISTS { ?s a <http://www.w3.org/ns/shacl#NodeShape> }
    } GROUP BY ?t ORDER BY DESC(?n) LIMIT 5`, { signal });
    sampleTypes = (res?.results?.bindings || []).map(b => ({
      cls: b.t?.value, count: parseInt(b.n?.value || '0', 10)
    }));
  }

  const verdict = classifyVerdict({ classCount, propertyCount, shapeCount, skosSchemeCount, skosConceptCount, entailmentCount, instanceCount });

  return { classCount, propertyCount, shapeCount, entailmentCount, skosSchemeCount, skosConceptCount, instanceCount, sampleTypes, verdict };
}
