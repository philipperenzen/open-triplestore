// What a node expansion in the browse graph actually did, worked out away from the
// canvas so it can be unit-tested — and so every caller (double-click, the
// right-click menu, the inspector's automatic blank-node load) reaches the same
// verdict and tells the user the same thing.
//
// Expansion has four honest outcomes, and only one of them used to be visible: a
// double-click that added nodes. Fetching a neighbourhood that is already drawn,
// re-expanding a node whose expansion was restored from the working-state
// snapshot, and a node with no neighbours left in scope all looked identical to a
// broken control — nothing moved and nothing was said.

export interface ExpandElement {
  data: { id: string };
}

export type ExpandOutcome =
  /** Neighbours arrived that were not on the canvas yet. */
  | 'added'
  /** Neighbours arrived, but every one of them was already drawn. */
  | 'nothingNew'
  /** This node had been expanded before, and re-expanding added nothing. */
  | 'already'
  /** The scope holds no neighbours for this node at all. */
  | 'empty';

export interface ExpandDecision {
  outcome: ExpandOutcome;
  /** Distinct fetched nodes that were not on the canvas yet. */
  added: number;
}

/** Translation key per outcome. `added` carries the plural count as `n`. */
export const EXPAND_MESSAGE_KEY: Record<ExpandOutcome, string> = {
  added: 'pages.tripleBrowser.expandAdded',
  nothingNew: 'pages.tripleBrowser.expandNothingNew',
  already: 'pages.tripleBrowser.expandAlready',
  empty: 'pages.tripleBrowser.expandEmpty',
};

/**
 * Decide what an expansion did.
 *
 * `fetched` are the nodes the neighbourhood query produced (outgoing and incoming
 * rows merged, so the same neighbour can appear twice), `presentIds` the node ids
 * already on the canvas when the expansion started, and `wasExpanded` whether this
 * node had an expansion recorded against it — including one restored from the
 * saved working state, which is exactly the case that used to go silent.
 *
 * `wasExpanded` only decides between the two do-nothing outcomes: when genuinely
 * new nodes arrive the user is told how many, expanded before or not.
 */
export function decideExpansion(
  fetched: readonly ExpandElement[] | null | undefined,
  presentIds: ReadonlySet<string>,
  wasExpanded: boolean,
): ExpandDecision {
  const rows = fetched || [];
  const newIds = new Set<string>();
  for (const n of rows) {
    const id = n?.data?.id;
    if (id && !presentIds.has(id)) newIds.add(id);
  }
  if (newIds.size > 0) return { outcome: 'added', added: newIds.size };
  if (wasExpanded) return { outcome: 'already', added: 0 };
  return { outcome: rows.length > 0 ? 'nothingNew' : 'empty', added: 0 };
}
