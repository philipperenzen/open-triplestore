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

  // Mirror the backend detector (src/kind_detector.rs): Model = classes (T-Box);
  // Vocabulary = properties/relations (R-Box) + SKOS concept schemes; ties between
  // class and vocabulary content go to Model (the class hierarchy anchors the schema).
  const classSignal = classCount;
  const vocabSignal = propertyCount + skosSchemeCount + skosConceptCount;
  const schemaSignal = classSignal + vocabSignal + shapeCount;
  const hasClasses = classCount > 0;

  let verdict: ContentKindVerdict = 'empty';
  if (schemaSignal === 0 && instanceCount === 0 && entailmentCount === 0) {
    verdict = 'empty';
  } else if (entailmentCount > 0 && entailmentCount >= Math.max(classSignal, vocabSignal, shapeCount, instanceCount)) {
    verdict = 'entailment';
  } else if (shapeCount > 0 && !hasClasses && shapeCount >= vocabSignal * 3 && shapeCount >= instanceCount * 3) {
    // SHACL shapes with no class hierarchy → pure shapes graph
    verdict = 'shapes';
  } else if (instanceCount > schemaSignal * 3) {
    verdict = 'instances';
  } else if (vocabSignal > 0 && !hasClasses && instanceCount < vocabSignal) {
    // Properties/relations or SKOS with no class anchor → pure Vocabulary (R-Box)
    verdict = 'vocabulary';
  } else if (classSignal > 0 || vocabSignal > 0) {
    if (classSignal >= vocabSignal) {
      verdict = instanceCount <= Math.max(classSignal, shapeCount) ? 'model' : 'mixed';
    } else {
      verdict = instanceCount <= Math.max(vocabSignal, shapeCount) ? 'vocabulary' : 'mixed';
    }
  } else {
    verdict = 'mixed';
  }

  return { classCount, propertyCount, shapeCount, entailmentCount, skosSchemeCount, skosConceptCount, instanceCount, sampleTypes, verdict };
}
