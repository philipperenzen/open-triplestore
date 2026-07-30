// Sidebar grouping for the dataset viewer's element list.
//
// The default view is the BOT/IFC *structure* tree (Site → Building → Storey →
// Space → Element), which is the right shape for one building but useless for
// answering "what have I got, and where is it?" across a dataset that spans
// several sites and countries. These groupings are the alternative lenses:
//
//   location — the administrative hierarchy (country → region → city), taken
//              from the place path the server infers and mints as entities
//              (`el.place`); see src/geo/places.rs.
//   format   — IFC / glTF / CityJSON / CityGML / STL, from the element's own
//              model references.
//   type     — the element's primary rdf:type.
//
// Each grouping returns the SAME shape as the structure tree's rows so the
// sidebar renders one way: a flat list of rows carrying a depth. Group headers
// are rows with no element.

import { modelRefOf, modelRefsOf, FORMAT_LABELS, type ModelFormat } from './detect';
import { shortenIRI } from '../rdf-utils.js';

/** A viewer-feed element, as far as grouping cares. */
export interface GroupableElement {
  id: string;
  label?: string | null;
  types?: string[];
  wkt4326?: string | null;
  gltf_url?: string | null;
  ifc_url?: string | null;
  files?: [string, string][];
  /** Server-inferred place path, broad → narrow (country → region → city). */
  place?: { id: string; label: string; level: string }[] | null;
}

export type GroupingKey = 'structure' | 'location' | 'format' | 'type';

/** The groupings offered in the sidebar's "Group by" control, in order. */
export const GROUPINGS: GroupingKey[] = ['structure', 'location', 'format', 'type'];

/** One rendered row: a group header (no `el`) or an element. */
export interface GroupRow {
  /** Stable key for Svelte's keyed each. */
  key: string;
  depth: number;
  /** Present on element rows. */
  el?: GroupableElement;
  /** Present on header rows. */
  header?: string;
  /** Elements at or below this header — drives the count pill. */
  count?: number;
  /** Header rows only: the path used as the expand key. */
  path?: string;
  /** Header rows only: whether this group is currently open. */
  open?: boolean;
}

// One collator, reused for every comparison. `String.localeCompare` constructs a
// fresh collator per call, which across thousands of elements is the difference
// between an instant regroup and a visible stall.
const COLLATOR = new Intl.Collator(undefined, { numeric: true, sensitivity: 'base' });

/** Every model format an element offers, in preference order (may be empty). */
export function formatsOf(el: GroupableElement): ModelFormat[] {
  return modelRefsOf(el).map((r) => r.format);
}

/** Short display labels for an element's model formats — the sidebar badges. */
export function formatBadges(el: GroupableElement): string[] {
  return formatsOf(el).map((f) => FORMAT_LABELS[f]);
}

/** The element's primary type, shortened for display. */
function primaryType(el: GroupableElement): string {
  const t = (el.types || [])[0];
  return t ? shortenIRI(t) : '';
}

/**
 * The group path an element belongs to under `key`, broad → narrow. An empty
 * path means "ungrouped" and lands under the trailing catch-all bucket.
 */
function pathOf(el: GroupableElement, key: GroupingKey, unknown: string): string[] {
  if (key === 'location') {
    const place = el.place || [];
    return place.length ? place.map((p) => p.label) : [unknown];
  }
  if (key === 'format') {
    const ref = modelRefOf(el);
    return [ref ? FORMAT_LABELS[ref.format] : unknown];
  }
  if (key === 'type') {
    return [primaryType(el) || unknown];
  }
  return [];
}

/**
 * Group `elements` into renderable rows under `key`.
 *
 * `expanded` holds the group paths (segments joined with ` `) the user has
 * opened. Groups start CLOSED: a few-thousand-element BIM dataset would
 * otherwise emit a row per element the instant the grouping changes, and
 * building that many DOM nodes is what made switching grouping take seconds.
 * Closed by default, a change renders only a handful of headers, and one
 * group's rows appear when it is opened.
 *
 * Groups sort alphabetically within their level, with the `unknown` bucket
 * pinned last so "no location" never displaces real places at the top.
 */
export function groupElements(
  elements: GroupableElement[],
  key: GroupingKey,
  opts: { expanded?: Set<string>; unknown?: string; label?: (el: GroupableElement) => string } = {},
): GroupRow[] {
  // 'structure' is the BOT/IFC containment tree, which the sidebar builds from
  // parent links rather than from paths — there is nothing to bucket here.
  if (key === 'structure') return [];
  const expanded = opts.expanded ?? new Set<string>();
  const unknown = opts.unknown ?? 'Ungrouped';
  const label = opts.label ?? ((el: GroupableElement) => el.label || shortenIRI(el.id));

  // Build a path-keyed tree of buckets. A Map keeps insertion order stable and
  // sorting explicit (below) rather than accidental.
  interface Node {
    children: Map<string, Node>;
    items: GroupableElement[];
    /** Elements at or below this node — the header's count. */
    total: number;
  }
  const makeNode = (): Node => ({ children: new Map(), items: [], total: 0 });
  const root = makeNode();

  for (const el of elements) {
    const path = pathOf(el, key, unknown);
    let node = root;
    node.total += 1;
    for (const seg of path) {
      let child = node.children.get(seg);
      if (!child) {
        child = makeNode();
        node.children.set(seg, child);
      }
      child.total += 1;
      node = child;
    }
    node.items.push(el);
  }

  const rows: GroupRow[] = [];
  const walk = (node: Node, depth: number, prefix: string[]) => {
    const names = [...node.children.keys()].sort((a, b) => {
      // The catch-all bucket always sinks to the bottom of its level.
      if (a === unknown) return 1;
      if (b === unknown) return -1;
      return COLLATOR.compare(a, b);
    });
    for (const name of names) {
      const child = node.children.get(name)!;
      const path = [...prefix, name];
      const pathKey = path.join(' ');
      const open = expanded.has(pathKey);
      rows.push({ key: `g:${pathKey}`, depth, header: name, count: child.total, path: pathKey, open });
      // A closed group contributes exactly one row however many elements it
      // holds — that bound is what keeps regrouping instant.
      if (!open) continue;
      walk(child, depth + 1, path);
    }
    const items = [...node.items].sort((a, b) => COLLATOR.compare(label(a), label(b)));
    for (const el of items) rows.push({ key: `e:${el.id}`, depth, el });
  };
  walk(root, 0, []);
  return rows;
}
