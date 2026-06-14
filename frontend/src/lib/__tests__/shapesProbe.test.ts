import { describe, it, expect } from 'vitest';
import { collectGraphIris, aggregateShapesProbe } from '../shapesProbe.js';

describe('collectGraphIris', () => {
  it('collects every graph from every successful file (auto-split subgraphs included)', () => {
    const iris = collectGraphIris([
      { status: 'ok', graphIri: 'http://ex.org/g1', graphIris: ['http://ex.org/g1', 'http://ex.org/g1/shapes'] },
      { status: 'ok', graphIri: 'http://ex.org/g2', graphIris: ['http://ex.org/g2'] },
    ]);
    expect(iris).toEqual(['http://ex.org/g1', 'http://ex.org/g1/shapes', 'http://ex.org/g2']);
  });

  it('skips failed files and de-duplicates IRIs', () => {
    const iris = collectGraphIris([
      { status: 'error' },
      { status: 'ok', graphIris: ['http://ex.org/g1', 'http://ex.org/g1'] },
      { status: 'ok', graphIris: ['http://ex.org/g1'] },
    ]);
    expect(iris).toEqual(['http://ex.org/g1']);
  });

  it('falls back to the single graphIri when graphIris is absent', () => {
    expect(collectGraphIris([{ status: 'ok', graphIri: 'http://ex.org/only' }])).toEqual(['http://ex.org/only']);
  });

  it('handles empty and malformed input', () => {
    expect(collectGraphIris([])).toEqual([]);
    expect(collectGraphIris([{ status: 'ok' }])).toEqual([]);
  });
});

describe('aggregateShapesProbe', () => {
  it('lists every shapes-bearing graph and sums counts', () => {
    const agg = aggregateShapesProbe([
      { graphIri: 'http://ex.org/data', result: { shapes_detected: false, shape_count: 0, suggested_datasets: [] } },
      { graphIri: 'http://ex.org/data/shapes', result: { shapes_detected: true, shape_count: 3, suggested_datasets: [{ id: 'ds1', name: 'One' }] } },
      { graphIri: 'http://ex.org/more-shapes', result: { shapes_detected: true, shape_count: 2, suggested_datasets: [{ id: 'ds1', name: 'One' }, { id: 'ds2', name: 'Two', has_shapes: true }] } },
    ]);
    expect(agg.shapesDetected).toBe(true);
    expect(agg.totalShapeCount).toBe(5);
    expect(agg.shapeGraphs).toEqual([
      { graphIri: 'http://ex.org/data/shapes', shapeCount: 3 },
      { graphIri: 'http://ex.org/more-shapes', shapeCount: 2 },
    ]);
    // Suggestions unioned by id; first occurrence wins, extra fields kept.
    expect(agg.suggestedDatasets).toEqual([
      { id: 'ds1', name: 'One' },
      { id: 'ds2', name: 'Two', has_shapes: true },
    ]);
  });

  it('reports no shapes when nothing was detected', () => {
    const agg = aggregateShapesProbe([
      { graphIri: 'http://ex.org/a', result: { shapes_detected: false, shape_count: 0, suggested_datasets: [{ id: 'x', name: 'X' }] } },
    ]);
    expect(agg.shapesDetected).toBe(false);
    expect(agg.totalShapeCount).toBe(0);
    expect(agg.shapeGraphs).toEqual([]);
  });

  it('skips failed probes (null result) without losing the others', () => {
    const agg = aggregateShapesProbe([
      { graphIri: 'http://ex.org/broken', result: null },
      { graphIri: 'http://ex.org/shapes', result: { shapes_detected: true, shape_count: 1 } },
    ]);
    expect(agg.shapesDetected).toBe(true);
    expect(agg.shapeGraphs).toEqual([{ graphIri: 'http://ex.org/shapes', shapeCount: 1 }]);
    expect(agg.suggestedDatasets).toEqual([]);
  });

  it('handles empty input', () => {
    const agg = aggregateShapesProbe([]);
    expect(agg.shapesDetected).toBe(false);
    expect(agg.totalShapeCount).toBe(0);
  });
});
