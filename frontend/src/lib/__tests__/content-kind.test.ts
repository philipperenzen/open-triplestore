import { describe, it, expect, vi, beforeEach } from 'vitest';

// content-kind.ts probes a graph with SPARQL COUNT queries and classifies it as
// an OWL/RDFS model, a SHACL shapes graph, a SKOS vocabulary, a SWRL entailment
// graph, instance data, mixed, or empty. We mock the SPARQL transport and assert
// the verdict logic — the standards-classification heuristic the UI relies on.
vi.mock('../api.js', () => ({ sparqlQuery: vi.fn(), browseFacets: vi.fn() }));
import { sparqlQuery, browseFacets } from '../api.js';
import { probeContentKind, datasetContentKind } from '../content-kind.js';

interface Counts {
  classes?: number;
  props?: number;
  shapes?: number;
  schemes?: number;
  concepts?: number;
  entailments?: number;
  instances?: number;
}

/** Route the three probe queries (combined counts / instance count / sample types). */
function mockCounts(c: Counts) {
  (sparqlQuery as unknown as ReturnType<typeof vi.fn>).mockImplementation((q: string) => {
    if (q.includes('?classes')) {
      return Promise.resolve({
        results: {
          bindings: [
            {
              classes: { value: String(c.classes ?? 0) },
              props: { value: String(c.props ?? 0) },
              shapes: { value: String(c.shapes ?? 0) },
              schemes: { value: String(c.schemes ?? 0) },
              concepts: { value: String(c.concepts ?? 0) },
              entailments: { value: String(c.entailments ?? 0) },
            },
          ],
        },
      });
    }
    if (q.includes('GROUP BY ?t')) {
      return Promise.resolve({ results: { bindings: [] } });
    }
    return Promise.resolve({ results: { bindings: [{ n: { value: String(c.instances ?? 0) } }] } });
  });
}

describe('probeContentKind verdict classification', () => {
  beforeEach(() => vi.clearAllMocks());

  it('returns "empty" with no graphs (and issues no query)', async () => {
    const r = await probeContentKind([]);
    expect(r.verdict).toBe('empty');
    expect(sparqlQuery).not.toHaveBeenCalled();
  });

  it('returns "empty" when every signal is zero', async () => {
    mockCounts({});
    expect((await probeContentKind(['g'])).verdict).toBe('empty');
  });

  it('classifies an OWL/RDFS schema as "model"', async () => {
    mockCounts({ classes: 5, props: 3, instances: 2 });
    expect((await probeContentKind(['g'])).verdict).toBe('model');
  });

  it('classifies a SHACL-only graph as "shapes"', async () => {
    mockCounts({ shapes: 4, instances: 1 });
    expect((await probeContentKind(['g'])).verdict).toBe('shapes');
  });

  it('classifies a SKOS-dominant graph as "vocabulary"', async () => {
    mockCounts({ schemes: 1, concepts: 20 });
    expect((await probeContentKind(['g'])).verdict).toBe('vocabulary');
  });

  it('classifies a property-only graph (R-Box) as "vocabulary"', async () => {
    // Object/datatype/annotation properties with no class anchor are R-Box →
    // Vocabulary, not Model (which is classes / T-Box).
    mockCounts({ props: 6 });
    expect((await probeContentKind(['g'])).verdict).toBe('vocabulary');
  });

  it('classifies a SWRL rule graph as "entailment"', async () => {
    mockCounts({ entailments: 5 });
    expect((await probeContentKind(['g'])).verdict).toBe('entailment');
  });

  it('classifies an instance-heavy graph as "instances"', async () => {
    mockCounts({ classes: 1, instances: 100 });
    expect((await probeContentKind(['g'])).verdict).toBe('instances');
  });

  it('classifies a balanced schema + data graph as "mixed"', async () => {
    mockCounts({ classes: 2, instances: 6 });
    expect((await probeContentKind(['g'])).verdict).toBe('mixed');
  });

  it('surfaces the raw signal counts alongside the verdict', async () => {
    mockCounts({ classes: 5, props: 3, shapes: 1, instances: 2 });
    const r = await probeContentKind(['g']);
    expect(r.classCount).toBe(5);
    expect(r.propertyCount).toBe(3);
    expect(r.shapeCount).toBe(1);
  });

  // Regression: a FAILED probe must yield the neutral empty result, never a partial
  // verdict. Previously the instance-count query swallowed its error and returned 0,
  // so a real classCount + a timed-out instanceCount=0 produced a spurious "model"
  // classification (→ a false "model in dataset" banner). It now rethrows and the
  // outer catch returns the empty shape.
  it('returns "empty" (not a partial verdict) when a probe query fails', async () => {
    let call = 0;
    (sparqlQuery as unknown as ReturnType<typeof vi.fn>).mockImplementation((q: string) => {
      call++;
      if (q.includes('?classes')) {
        return Promise.resolve({
          results: { bindings: [{ classes: { value: '5' }, props: { value: '0' }, shapes: { value: '0' }, schemes: { value: '0' }, concepts: { value: '0' }, entailments: { value: '0' } }] },
        });
      }
      // The instance-count query times out / errors.
      return Promise.reject(new Error('query timeout'));
    });
    const r = await probeContentKind(['g']);
    expect(r.verdict).toBe('empty');
    expect(r.classCount).toBe(0);
    expect(r.instanceCount).toBe(0);
    expect(call).toBeGreaterThan(1); // it did reach the failing query
  });

  it('threads an AbortSignal through to the SPARQL transport', async () => {
    mockCounts({ classes: 1, instances: 1 });
    const ctrl = new AbortController();
    await probeContentKind(['g'], ctrl.signal);
    const calls = (sparqlQuery as unknown as ReturnType<typeof vi.fn>).mock.calls;
    // Every probe query is passed { signal } as its second arg.
    expect(calls.length).toBeGreaterThan(0);
    for (const c of calls) expect(c[1]).toMatchObject({ signal: ctrl.signal });
  });
});

const OWL = 'http://www.w3.org/2002/07/owl#';
const SKOS = 'http://www.w3.org/2004/02/skos/core#';
const SH = 'http://www.w3.org/ns/shacl#';

describe('datasetContentKind (facet-derived summary)', () => {
  beforeEach(() => vi.clearAllMocks());

  function mockFacets(classes: { iri: string; count: number }[], properties: { iri: string; count: number }[] = []) {
    (browseFacets as unknown as ReturnType<typeof vi.fn>).mockResolvedValue({ classes, properties, graphs: [] });
  }

  it('buckets meta-types into signals and everything else into instances', async () => {
    mockFacets(
      [
        { iri: 'http://ex.org/Building', count: 40 },
        { iri: 'http://ex.org/Wall', count: 120 },
        { iri: OWL + 'Class', count: 3 },
        { iri: OWL + 'ObjectProperty', count: 2 },
        { iri: SH + 'NodeShape', count: 1 },
        { iri: SKOS + 'Concept', count: 5 },
      ],
      [{ iri: 'http://ex.org/p1', count: 9 }, { iri: 'http://ex.org/p2', count: 4 }],
    );
    const r = await datasetContentKind('ds1');
    expect(r.classCount).toBe(3);       // owl:Class instances = class definitions
    expect(r.propertyCount).toBe(2);    // owl:ObjectProperty instances
    expect(r.shapeCount).toBe(1);
    expect(r.skosConceptCount).toBe(5);
    expect(r.instanceCount).toBe(160);  // Building + Wall (non-meta)
    expect(r.instanceTypeCount).toBe(2);
    expect(r.predicateCount).toBe(2);
    expect(r.sampleTypes.map(s => s.cls)).toEqual(['http://ex.org/Building', 'http://ex.org/Wall']);
    expect(r.verdict).toBe('instances'); // 160 instances >> tiny schema signal
  });

  it('classifies a schema-only dataset as model, not instances', async () => {
    mockFacets([
      { iri: OWL + 'Class', count: 8 },
      { iri: OWL + 'ObjectProperty', count: 4 },
    ]);
    const r = await datasetContentKind('ds2');
    expect(r.instanceCount).toBe(0);
    expect(r.verdict).toBe('model');
  });

  it('flags the cap when the facet list is saturated', async () => {
    mockFacets(Array.from({ length: 300 }, (_, i) => ({ iri: `http://ex.org/T${i}`, count: 1 })));
    const r = await datasetContentKind('ds3');
    expect(r.capped).toBe(true);
  });

  it('threads an AbortSignal into browseFacets', async () => {
    mockFacets([{ iri: 'http://ex.org/A', count: 1 }]);
    const ctrl = new AbortController();
    await datasetContentKind('ds4', ctrl.signal);
    const call = (browseFacets as unknown as ReturnType<typeof vi.fn>).mock.calls[0];
    expect(call[0]).toEqual({ dataset_id: 'ds4' });
    expect(call[1]).toMatchObject({ signal: ctrl.signal });
  });
});
