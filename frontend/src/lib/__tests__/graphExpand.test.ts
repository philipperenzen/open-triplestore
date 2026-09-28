import { describe, it, expect } from 'vitest';
import { decideExpansion, EXPAND_MESSAGE_KEY, type ExpandElement } from '../graphExpand';

function nodes(...ids: string[]): ExpandElement[] {
  return ids.map((id) => ({ data: { id } }));
}

describe('decideExpansion', () => {
  it('reports how many fetched nodes were new', () => {
    const d = decideExpansion(nodes('A', 'B', 'C'), new Set(['A']), false);
    expect(d).toEqual({ outcome: 'added', added: 2 });
  });

  // The bug that started this: double-clicking a node whose neighbours were all
  // already drawn changed nothing on the canvas and said nothing either, which is
  // indistinguishable from a dead control.
  it('distinguishes "fetched, but all of it is already on the canvas" from an empty result', () => {
    const d = decideExpansion(nodes('A', 'B'), new Set(['A', 'B', 'C']), false);
    expect(d).toEqual({ outcome: 'nothingNew', added: 0 });
  });

  // A node whose expansion was restored from the working-state snapshot: the
  // neighbours came back with the graph, so a re-expand adds nothing — say so
  // rather than doing nothing.
  it('prefers the "already expanded" outcome when the node had been expanded before', () => {
    const d = decideExpansion(nodes('A', 'B'), new Set(['A', 'B']), true);
    expect(d).toEqual({ outcome: 'already', added: 0 });
  });

  it('still reports new nodes on a node that was expanded before', () => {
    const d = decideExpansion(nodes('A', 'B'), new Set(['A']), true);
    expect(d).toEqual({ outcome: 'added', added: 1 });
  });

  it('reports an empty scope when nothing came back at all', () => {
    expect(decideExpansion([], new Set(['A']), false)).toEqual({ outcome: 'empty', added: 0 });
    // A previously expanded node that now returns nothing is still "already
    // expanded" — that is the more informative half of the truth.
    expect(decideExpansion([], new Set(['A']), true)).toEqual({ outcome: 'already', added: 0 });
  });

  it('treats a missing result like an empty one', () => {
    expect(decideExpansion(null, new Set(), false)).toEqual({ outcome: 'empty', added: 0 });
  });

  // The fetch merges outgoing and incoming rows, so the same neighbour can appear
  // twice; the count the user is shown must be the count of nodes actually drawn.
  it('counts each new node once', () => {
    const d = decideExpansion(nodes('B', 'B', 'C'), new Set(['A']), false);
    expect(d).toEqual({ outcome: 'added', added: 2 });
  });
});

describe('EXPAND_MESSAGE_KEY', () => {
  it('maps every outcome to an existing translation key', () => {
    expect(EXPAND_MESSAGE_KEY).toEqual({
      added: 'pages.tripleBrowser.expandAdded',
      nothingNew: 'pages.tripleBrowser.expandNothingNew',
      already: 'pages.tripleBrowser.expandAlready',
      empty: 'pages.tripleBrowser.expandEmpty',
    });
  });
});
