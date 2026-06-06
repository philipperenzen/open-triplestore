import { describe, it, expect, vi, beforeEach } from 'vitest';

// content-kind.ts probes a graph with SPARQL COUNT queries and classifies it as
// an OWL/RDFS model, a SHACL shapes graph, a SKOS vocabulary, a SWRL entailment
// graph, instance data, mixed, or empty. We mock the SPARQL transport and assert
// the verdict logic — the standards-classification heuristic the UI relies on.
vi.mock('../api.js', () => ({ sparqlQuery: vi.fn() }));
import { sparqlQuery } from '../api.js';
import { probeContentKind } from '../content-kind.js';

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
});
