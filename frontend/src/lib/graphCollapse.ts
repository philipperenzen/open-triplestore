// Collapse logic for the browse graph view, extracted so it can be unit-tested
// without a cytoscape canvas.
//
// Each expansion is recorded as `key -> { nodeIds, edgeIds }`, where `key` is the
// IRI (or blank-node id) that was expanded and the id sets are what that expansion
// ADDED. Node ids are the term values, so an expansion key is directly comparable
// to a node id — that is what lets us detect nested expansions.
//
// The naive collapse (remove this expansion's ids minus every other expansion's
// ids) leaves orphans: expand A→B, then B→D, then collapse A. B was added only by
// A's expansion so B is removed, but D was added by B's expansion so D survives —
// stranded with no edges, while `expansions` still holds an entry keyed on the
// now-deleted B. Collapsing A must therefore collapse everything reachable through
// A's expansion tree.

export interface ExpansionRecord {
  nodeIds: Set<string>;
  edgeIds: Set<string>;
}

export interface CollapsePlan {
  /** Expansion keys to forget (the root plus every nested expansion under it). */
  keysToDrop: Set<string>;
  /** Node ids to remove from the graph. */
  removeNodeIds: Set<string>;
  /** Edge ids to remove from the graph. */
  removeEdgeIds: Set<string>;
}

const EMPTY: CollapsePlan = {
  keysToDrop: new Set(),
  removeNodeIds: new Set(),
  removeEdgeIds: new Set(),
};

/**
 * Work out what collapsing `rootKey` should remove.
 *
 * Anything still owned by an expansion that is NOT being collapsed is kept, so
 * two expansions that both surfaced the same node keep it alive.
 */
export function collapseClosure(
  rootKey: string,
  expansions: Map<string, ExpansionRecord>,
): CollapsePlan {
  if (!expansions.has(rootKey)) return { ...EMPTY };

  // 1. Transitive closure of expansions rooted at `rootKey`: a node added by a
  //    collapsing expansion that was itself expanded collapses too.
  const keysToDrop = new Set<string>([rootKey]);
  const frontier = [rootKey];
  while (frontier.length) {
    const key = frontier.pop() as string;
    const rec = expansions.get(key);
    if (!rec) continue;
    for (const nodeId of rec.nodeIds) {
      if (expansions.has(nodeId) && !keysToDrop.has(nodeId)) {
        keysToDrop.add(nodeId);
        frontier.push(nodeId);
      }
    }
  }

  // 2. Everything the SURVIVING expansions still own must stay.
  const keptNodeIds = new Set<string>();
  const keptEdgeIds = new Set<string>();
  for (const [key, rec] of expansions) {
    if (keysToDrop.has(key)) continue;
    for (const id of rec.nodeIds) keptNodeIds.add(id);
    for (const id of rec.edgeIds) keptEdgeIds.add(id);
  }

  // 3. Remove the union of the collapsing expansions' ids, minus anything kept.
  //    A node that is itself a surviving expansion's key is kept too — it is an
  //    anchor the user expanded, not a leaf this expansion produced.
  const removeNodeIds = new Set<string>();
  const removeEdgeIds = new Set<string>();
  for (const key of keysToDrop) {
    const rec = expansions.get(key);
    if (!rec) continue;
    for (const id of rec.nodeIds) {
      if (!keptNodeIds.has(id) && !(expansions.has(id) && !keysToDrop.has(id))) {
        removeNodeIds.add(id);
      }
    }
    for (const id of rec.edgeIds) {
      if (!keptEdgeIds.has(id)) removeEdgeIds.add(id);
    }
  }

  return { keysToDrop, removeNodeIds, removeEdgeIds };
}
