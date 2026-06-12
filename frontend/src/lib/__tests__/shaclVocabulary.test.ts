import { describe, it, expect } from 'vitest';
import { VOCAB, NAMESPACES } from '../ontology/vocabularies.ts';

const SH = NAMESPACES.sh;

describe('SHACL vocabulary completeness', () => {
  const iris = new Set(VOCAB.sh.map((t) => t.iri));

  it.each([
    // qualified value shapes
    'qualifiedValueShape', 'qualifiedMinCount', 'qualifiedMaxCount', 'qualifiedValueShapesDisjoint',
    // SPARQL constraint vocabulary
    'sparql', 'SPARQLConstraint', 'select', 'ask', 'construct', 'update', 'prefixes', 'declare', 'prefix', 'namespace',
    // rules
    'rule', 'Rule', 'SPARQLRule', 'TripleRule', 'subject', 'predicate', 'object', 'condition',
    // path operators
    'inversePath', 'alternativePath', 'zeroOrMorePath', 'oneOrMorePath', 'zeroOrOnePath',
    // target declarations
    'targetClass', 'targetNode', 'targetSubjectsOf', 'targetObjectsOf', 'target', 'Target', 'TargetType', 'SPARQLTarget',
    // result vocabulary
    'ValidationReport', 'ValidationResult', 'conforms', 'result', 'focusNode', 'value', 'resultPath',
    'resultMessage', 'resultSeverity', 'sourceShape', 'sourceConstraint', 'sourceConstraintComponent', 'detail',
    // severity classes
    'Severity', 'Violation', 'Warning', 'Info',
    // logical + core
    'and', 'or', 'xone', 'not', 'closed', 'ignoredProperties', 'deactivated', 'group', 'order',
    'equals', 'disjoint', 'lessThan', 'lessThanOrEquals', 'expression',
  ])('includes sh:%s', (local) => {
    expect(iris.has(SH + local)).toBe(true);
  });

  it('has no duplicate terms', () => {
    expect(iris.size).toBe(VOCAB.sh.length);
  });

  it('uses the standard term shape', () => {
    for (const t of VOCAB.sh) {
      expect(t.iri.startsWith(SH)).toBe(true);
      expect(t.label).toBeTruthy();
      expect(['class', 'object', 'datatype', 'annotation', 'property']).toContain(t.kind);
    }
  });
});
