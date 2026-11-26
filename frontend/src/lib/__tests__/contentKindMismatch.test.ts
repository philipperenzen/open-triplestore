/**
 * Which banner a graph set's content earns where it sits. A SKOS concept
 * scheme is data as much as anything: it raises no banner in a dataset.
 */
import { describe, it, expect } from 'vitest';
import { classifyVerdict, contentKindMismatch, type ContentKindSignals } from '../content-kind';

const EMPTY: ContentKindSignals = {
  classCount: 0,
  propertyCount: 0,
  shapeCount: 0,
  skosSchemeCount: 0,
  skosConceptCount: 0,
  entailmentCount: 0,
  instanceCount: 0,
};

/** A probe with `signals` and the verdict the classifier gives them. */
function probe(signals: Partial<ContentKindSignals>) {
  const s = { ...EMPTY, ...signals };
  return { ...s, verdict: classifyVerdict(s) };
}

describe('contentKindMismatch in a dataset', () => {
  it('raises no banner for a concept scheme and nothing else', () => {
    const p = probe({ skosSchemeCount: 1, skosConceptCount: 143 });
    expect(p.verdict).toBe('vocabulary');
    expect(contentKindMismatch(p, false, 'dataset', null)).toBeNull();
  });

  it('raises no banner for a concept scheme beside instances either', () => {
    const p = probe({ skosSchemeCount: 1, skosConceptCount: 143, instanceCount: 400 });
    expect(contentKindMismatch(p, false, 'dataset', null)).toBeNull();
  });

  it('still flags properties kept in a dataset as a vocabulary', () => {
    const p = probe({ propertyCount: 20, skosConceptCount: 5 });
    expect(p.verdict).toBe('vocabulary');
    expect(contentKindMismatch(p, false, 'dataset', null)).toBe('vocabulary-in-dataset');
    expect(contentKindMismatch(p, false, 'dataset', 'vocabulary')).toBeNull();
  });

  it('still flags classes, shapes and rules', () => {
    expect(contentKindMismatch(probe({ classCount: 12 }), false, 'dataset', null)).toBe(
      'model-in-dataset',
    );
    expect(contentKindMismatch(probe({ classCount: 12 }), false, 'dataset', 'model')).toBeNull();
    expect(contentKindMismatch(probe({ shapeCount: 9 }), false, 'dataset', null)).toBe(
      'shapes-in-dataset',
    );
    expect(contentKindMismatch(probe({ entailmentCount: 6 }), false, 'dataset', null)).toBe(
      'entailment-in-dataset',
    );
  });

  it('does not count concepts towards the schema in mixed content', () => {
    // Plenty of concepts and some instances, almost no schema: not worth a banner.
    const light = probe({ skosConceptCount: 300, instanceCount: 500, classCount: 1 });
    expect(light.verdict).toBe('mixed');
    expect(contentKindMismatch(light, false, 'dataset', null)).toBeNull();
    // Real schema among the instances is.
    // (More instances than that would make it instance data, which is a dataset's.)
    const heavy = probe({ classCount: 4, propertyCount: 6, instanceCount: 8 });
    expect(heavy.verdict).toBe('mixed');
    expect(contentKindMismatch(heavy, false, 'dataset', null)).toBe('model-in-dataset');
  });

  it('is quiet for empty, dismissed and unknown probes', () => {
    expect(contentKindMismatch(probe({}), false, 'dataset', null)).toBeNull();
    expect(contentKindMismatch(probe({ classCount: 12 }), true, 'dataset', null)).toBeNull();
    expect(contentKindMismatch(null, false, 'dataset', null)).toBeNull();
  });
});

describe('contentKindMismatch in a registry entry', () => {
  it('flags instance data that outweighs the schema', () => {
    const p = probe({ classCount: 2, instanceCount: 500 });
    expect(contentKindMismatch(p, false, 'model', null)).toBe('instance-in-model');
    expect(contentKindMismatch(p, false, 'vocabulary', null)).toBe('instance-in-model');
  });

  it('never asks a registry entry to look like a dataset', () => {
    expect(contentKindMismatch(probe({ classCount: 12 }), false, 'model', null)).toBeNull();
  });
});
