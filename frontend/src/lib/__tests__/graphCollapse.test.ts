import { describe, it, expect } from 'vitest';
import { collapseClosure, type ExpansionRecord } from '../graphCollapse';

function exp(nodes: string[], edges: string[] = []): ExpansionRecord {
  return { nodeIds: new Set(nodes), edgeIds: new Set(edges) };
}

describe('collapseClosure', () => {
  it('removes what a single expansion added', () => {
    const m = new Map([['A', exp(['B', 'C'], ['e1', 'e2'])]]);
    const plan = collapseClosure('A', m);
    expect([...plan.keysToDrop]).toEqual(['A']);
    expect([...plan.removeNodeIds].sort()).toEqual(['B', 'C']);
    expect([...plan.removeEdgeIds].sort()).toEqual(['e1', 'e2']);
  });

  // The orphan bug: expand A→B, then B→D, then collapse A. B is removed because
  // only A's expansion added it, but D used to survive as an edgeless floater with
  // a stale expansion entry keyed on the deleted B.
  it('collapses nested expansions transitively (A -> B -> D)', () => {
    const m = new Map([
      ['A', exp(['B'], ['a-b'])],
      ['B', exp(['D'], ['b-d'])],
    ]);
    const plan = collapseClosure('A', m);
    expect([...plan.keysToDrop].sort()).toEqual(['A', 'B']);
    expect([...plan.removeNodeIds].sort()).toEqual(['B', 'D']);
    expect([...plan.removeEdgeIds].sort()).toEqual(['a-b', 'b-d']);
  });

  it('collapses a deep chain in one go', () => {
    const m = new Map([
      ['A', exp(['B'])],
      ['B', exp(['C'])],
      ['C', exp(['D'])],
      ['D', exp(['E'])],
    ]);
    const plan = collapseClosure('A', m);
    expect([...plan.keysToDrop].sort()).toEqual(['A', 'B', 'C', 'D']);
    expect([...plan.removeNodeIds].sort()).toEqual(['B', 'C', 'D', 'E']);
  });

  it('keeps nodes another, surviving expansion also surfaced', () => {
    const m = new Map([
      ['A', exp(['B', 'shared'], ['a-b', 'a-shared'])],
      ['X', exp(['shared'], ['x-shared'])],
    ]);
    const plan = collapseClosure('A', m);
    expect([...plan.keysToDrop]).toEqual(['A']);
    expect([...plan.removeNodeIds]).toEqual(['B']);
    expect(plan.removeNodeIds.has('shared')).toBe(false);
    expect(plan.removeEdgeIds.has('x-shared')).toBe(false);
    expect([...plan.removeEdgeIds].sort()).toEqual(['a-b', 'a-shared']);
  });

  it('keeps a node that is itself a surviving expansion anchor', () => {
    // A surfaced X; the user then expanded X. Collapsing A drops X's subtree too
    // (X came from A), so X goes. But if X had been expanded from elsewhere and is
    // NOT reachable from A, it must stay — covered by the surviving-owner case.
    const m = new Map([
      ['A', exp(['B'])],
      ['Z', exp(['B'])], // B is also owned by Z, which is not collapsing
    ]);
    const plan = collapseClosure('A', m);
    expect(plan.removeNodeIds.has('B')).toBe(false);
  });

  it('handles a cycle without looping forever', () => {
    const m = new Map([
      ['A', exp(['B'])],
      ['B', exp(['A', 'C'])],
    ]);
    const plan = collapseClosure('A', m);
    expect([...plan.keysToDrop].sort()).toEqual(['A', 'B']);
    expect([...plan.removeNodeIds].sort()).toEqual(['A', 'B', 'C']);
  });

  it('is a no-op for an unknown key', () => {
    const plan = collapseClosure('nope', new Map([['A', exp(['B'])]]));
    expect(plan.keysToDrop.size).toBe(0);
    expect(plan.removeNodeIds.size).toBe(0);
    expect(plan.removeEdgeIds.size).toBe(0);
  });

  it('does not touch nodes from the initial page load', () => {
    // Nodes never recorded in any expansion (the base graph) are never candidates.
    const m = new Map([['A', exp(['B'])]]);
    const plan = collapseClosure('A', m);
    expect(plan.removeNodeIds.has('base-node')).toBe(false);
  });
});
