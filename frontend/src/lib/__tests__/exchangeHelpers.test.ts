// The pure helpers behind the exchange views: LDES retention parsing, the
// container export headers, and grouping an IDS run by specification.
import { describe, it, expect } from 'vitest';
import {
  isXsdDuration, isZonedDateTime, formToRetention, retentionToForm, keepsEverything,
  parseContainerHeaders, sortFindings, idsProjectionGraphs, buildingOf, isIdsShapeGraph,
  summarizeIdsRun, findIdsPipeline, pctOf, fmtBytes,
} from '../exchange';

describe('LDES retention', () => {
  it('accepts the xsd:duration subset the server parses and refuses the rest', () => {
    for (const ok of ['P1D', 'PT1H30M', 'P1Y', 'P2M', 'P1W', 'P1DT0.5S', 'P30D']) expect(isXsdDuration(ok)).toBe(true);
    for (const bad of ['', 'P', 'PT', '1D', '-P1D', 'P1H', 'PD', 'P1DT', '30 days']) expect(isXsdDuration(bad)).toBe(false);
  });

  it('needs a time zone on a date-time', () => {
    expect(isZonedDateTime('2026-01-01T00:00:00Z')).toBe(true);
    expect(isZonedDateTime('2026-01-01T00:00:00+02:00')).toBe(true);
    expect(isZonedDateTime('2026-01-01T00:00:00')).toBe(false);
    expect(isZonedDateTime('yesterday')).toBe(false);
  });

  it('turns a form into a policy, an empty form into {} (keep everything)', () => {
    const empty = retentionToForm(null);
    expect(formToRetention(empty)).toEqual({ policy: {} });
    const f = { ...empty, full_log_duration: ' P30D ', version_amount: '2', version_delete_duration: 'P7D' };
    expect(formToRetention(f)).toEqual({ policy: { full_log_duration: 'P30D', version_amount: 2, version_delete_duration: 'P7D' } });
  });

  it('takes the number a number input hands back', () => {
    const f = { ...retentionToForm(null), version_amount: 3 as unknown as string };
    expect(formToRetention(f)).toEqual({ policy: { version_amount: 3 } });
  });

  it('names the first wrong field', () => {
    const empty = retentionToForm(null);
    expect(formToRetention({ ...empty, full_log_duration: '30 days' })).toEqual({ error: 'fullLogDuration' });
    expect(formToRetention({ ...empty, version_amount: '0' })).toEqual({ error: 'versionAmount' });
    expect(formToRetention({ ...empty, version_duration: 'P1Y' })).toEqual({ error: 'versionDurationNeedsAmount' });
    expect(formToRetention({ ...empty, starting_from: '2026-01-01' })).toEqual({ error: 'startingFrom' });
  });

  it('round-trips a stored policy through the form', () => {
    const p = { full_log_duration: 'P30D', version_amount: 2, starting_from: '2026-01-01T00:00:00Z' };
    expect(formToRetention(retentionToForm(p))).toEqual({ policy: p });
    expect(keepsEverything(null)).toBe(true);
    expect(keepsEverything({})).toBe(true);
    expect(keepsEverything(p)).toBe(false);
  });
});

describe('container export headers', () => {
  it('reads conformance, counts and finding codes', () => {
    expect(parseContainerHeaders({
      conforms: 'false', validation: 'violations=0; warnings=2', findings: 'ontology-resource-missing, document-missing',
    })).toEqual({ conforms: false, violations: 0, warnings: 2, findings: ['ontology-resource-missing', 'document-missing'] });
    expect(parseContainerHeaders({ conforms: null, validation: null, findings: null }))
      .toEqual({ conforms: null, violations: null, warnings: null, findings: [] });
  });

  it('lists violations before warnings before info', () => {
    const sorted = sortFindings([
      { severity: 'info', code: 'a', message: '' },
      { severity: 'violation', code: 'b', message: '' },
      { severity: 'warning', code: 'c', message: '' },
    ]);
    expect(sorted.map((f) => f.code)).toEqual(['b', 'c', 'a']);
  });
});

describe('IDS check', () => {
  it('finds the IDS projection graphs and their buildings', () => {
    const graphs = ['https://x.test/dataset/d/building', { graph_iri: 'https://x.test/dataset/d/building/ids' }, 'https://x.test/ids-notes'];
    expect(idsProjectionGraphs(graphs)).toEqual(['https://x.test/dataset/d/building/ids']);
    expect(buildingOf('https://x.test/dataset/d/building/ids')).toBe('https://x.test/dataset/d/building');
    expect(isIdsShapeGraph({ tags: ['ids'] })).toBe(true);
    expect(isIdsShapeGraph({ tags: [] })).toBe(false);
  });

  it('groups results by specification, folding the existence shape in', () => {
    const specs = [
      { shape: 'urn:ids:walls', label: 'Walls have a fire rating' },
      { shape: 'urn:ids:walls-exists', label: 'exists' },
      { shape: 'urn:ids:doors', label: 'Doors are named' },
      { shape: 'urn:ids:slabs', label: 'Slabs exist' },
    ];
    const results = [
      { source_shape: '<urn:ids:walls>', focus_node: '<urn:w1>', message: 'FireRating missing' },
      { source_shape: 'urn:ids:walls', focus_node: 'urn:w1', message: 'FireRating missing' },
      { source_shape: 'urn:ids:walls', focus_node: 'urn:w2', message: 'FireRating missing' },
      { source_shape: 'urn:ids:slabs-exists', focus_node: 'urn:model', message: 'no applicable entity' },
      { source_shape: 'urn:other', focus_node: 'urn:x', message: 'm' },
    ];
    const rows = summarizeIdsRun(specs, results);
    expect(rows.map((r) => [r.name, r.passed, r.failing.length, r.missing])).toEqual([
      ['Walls have a fire rating', false, 2, false],
      ['Doors are named', true, 0, false],
      ['Slabs exist', false, 0, true],
      ['urn:other', false, 1, false],
    ]);
    expect(rows[0].messages).toEqual(['FireRating missing']);
  });

  it('reuses only a pipeline scoped to exactly that dataset, graph and shape graph', () => {
    const ps = [
      { id: 'wide', dataset_ids: ['d'], graph_iris: [], shape_graph_ids: ['sg'] },
      { id: 'two', dataset_ids: ['d'], graph_iris: ['g'], shape_graph_ids: ['sg', 'other'] },
      { id: 'exact', dataset_ids: ['d'], graph_iris: ['g'], shape_graph_ids: ['sg'] },
    ];
    expect(findIdsPipeline(ps, 'd', 'g', 'sg')?.id).toBe('exact');
    expect(findIdsPipeline(ps, 'd', 'g2', 'sg')).toBeUndefined();
  });
});

describe('formatting', () => {
  it('clamps progress and sizes bytes', () => {
    expect(pctOf(0.456)).toBe(46);
    expect(pctOf(2)).toBe(100);
    expect(pctOf(null)).toBe(0);
    expect(fmtBytes(512)).toBe('512 B');
    expect(fmtBytes(1536)).toBe('1.5 KB');
    expect(fmtBytes(null)).toBe('—');
  });
});
