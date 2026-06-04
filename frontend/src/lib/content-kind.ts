// Probe one or more named graphs to estimate whether they contain
// model (OWL/RDFS ontology), shapes (SHACL), vocabulary (SKOS),
// entailment rules (SWRL/SPIN), or instance data.
//
// Returns: {
//   classCount, propertyCount, shapeCount, entailmentCount,  // OWL/RDFS/SHACL/SWRL signals
//   skosSchemeCount, skosConceptCount,                       // SKOS signals
//   instanceCount, sampleTypes,
//   verdict: 'model' | 'shapes' | 'vocabulary' | 'entailment' | 'instances' | 'mixed' | 'empty'
// }

import { sparqlQuery } from './api.js';

export type ContentKindVerdict = 'model' | 'shapes' | 'vocabulary' | 'entailment' | 'instances' | 'mixed' | 'empty';

function fromClauses(graphs: string[]): string {
  if (!graphs?.length) return '';
  return graphs.map(g => `FROM <${g}>`).join('\n');
}

async function askCount(query: string): Promise<number> {
  try {
    const res = await sparqlQuery(query);
    const binding = res?.results?.bindings?.[0];
    return parseInt(binding?.n?.value || '0', 10);
  } catch {
    return 0;
  }
}

export async function probeContentKind(graphs: string[]) {
  const from = fromClauses(graphs);
  if (!from) return { classCount: 0, propertyCount: 0, shapeCount: 0, entailmentCount: 0, instanceCount: 0, verdict: 'empty' as ContentKindVerdict, sampleTypes: [] };

  let classCount = 0, propertyCount = 0, shapeCount = 0, skosSchemeCount = 0, skosConceptCount = 0, entailmentCount = 0, instanceCount = 0;
  try {
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
    }`);
    const b = combined?.results?.bindings?.[0];
    if (b) {
      classCount      = parseInt(b.classes?.value     || '0', 10);
      propertyCount   = parseInt(b.props?.value       || '0', 10);
      shapeCount      = parseInt(b.shapes?.value      || '0', 10);
      skosSchemeCount = parseInt(b.schemes?.value     || '0', 10);
      skosConceptCount= parseInt(b.concepts?.value    || '0', 10);
      entailmentCount = parseInt(b.entailments?.value || '0', 10);
    }
  } catch { /* ignore — probe returns empty on error */ }

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
  }`);

  // Sample the top-5 instance types for display
  let sampleTypes = [];
  try {
    const res = await sparqlQuery(`SELECT ?t (COUNT(DISTINCT ?s) AS ?n) ${from} WHERE {
      ?s a ?t .
      FILTER(ISIRI(?t))
      FILTER NOT EXISTS { ?s a <http://www.w3.org/2000/01/rdf-schema#Class> }
      FILTER NOT EXISTS { ?s a <http://www.w3.org/2002/07/owl#Class> }
      FILTER NOT EXISTS { ?s a <http://www.w3.org/1999/02/22-rdf-syntax-ns#Property> }
      FILTER NOT EXISTS { ?s a <http://www.w3.org/ns/shacl#NodeShape> }
    } GROUP BY ?t ORDER BY DESC(?n) LIMIT 5`);
    sampleTypes = (res?.results?.bindings || []).map(b => ({
      cls: b.t?.value, count: parseInt(b.n?.value || '0', 10)
    }));
  } catch { /* ignore */ }

  const modelSignal = classCount + propertyCount;
  const schemaSignal = modelSignal + shapeCount;
  const skosSignal = skosSchemeCount + skosConceptCount;

  let verdict: ContentKindVerdict = 'empty';
  if (schemaSignal === 0 && skosSignal === 0 && instanceCount === 0 && entailmentCount === 0) {
    verdict = 'empty';
  } else if (entailmentCount > 0 && entailmentCount >= modelSignal && entailmentCount >= instanceCount) {
    verdict = 'entailment';
  } else if (skosSignal > 0 && skosSignal >= schemaSignal * 3 && instanceCount < skosSignal) {
    verdict = 'vocabulary';
  } else if (shapeCount > 0 && modelSignal === 0 && instanceCount <= Math.max(3, shapeCount)) {
    // SHACL shapes with no OWL classes → pure shapes graph
    verdict = 'shapes';
  } else if (schemaSignal > 0 && instanceCount <= Math.max(5, schemaSignal) && skosSignal < schemaSignal * 3) {
    verdict = 'model';
  } else if (instanceCount > Math.max(5, Math.max(schemaSignal, skosSignal) * 3)) {
    verdict = 'instances';
  } else {
    verdict = 'mixed';
  }

  return { classCount, propertyCount, shapeCount, entailmentCount, skosSchemeCount, skosConceptCount, instanceCount, sampleTypes, verdict };
}
