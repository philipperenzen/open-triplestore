// Content signature for a Model3D `refs` array.
//
// Svelte re-derives the array literal a host passes to `<Model3D refs={[…]} />`
// on every reactive pass, so an identity comparison (`refs !== lastRefs`) fires
// on a mere re-render: the viewer tore down every group, re-cloned the model and
// re-framed the camera — which is why a 3D view snapped back to its default pose
// whenever anything in the page was clicked. Comparing a signature of the fields
// the loader actually consumes makes a re-render a no-op and a genuine
// navigation a reload.
//
// Kept free of a `three` import (and hence separate from models.ts, which pulls
// three in at module scope) so it stays unit-testable and out of the WebGL
// chunk. The guid digest is intentionally order-independent: a recomputed
// descendant list in a different order is the same model set.

/** The subset of a Model3D ref that affects loading or placement. */
export interface ModelRefLike {
  id?: string | null;
  url?: string | null;
  format?: string | null;
  upAxis?: string | null;
  slot?: [number, number] | number[] | null;
  guids?: string[] | null;
}

/** Order-independent 32-bit digest of a guid set (djb2-xor over the sorted ids),
 *  so a 900-element storey doesn't turn into a 20 KB signature. */
export function digestGuids(guids: string[]): string {
  let h = 5381;
  for (const g of [...guids].sort()) {
    for (let i = 0; i < g.length; i++) h = ((h * 33) ^ g.charCodeAt(i)) >>> 0;
    h = (h ^ 0x2d) >>> 0; // separator so [ab,c] != [a,bc]
  }
  return h.toString(36);
}

// Control characters as separators: a URL or an IRI can contain any printable
// character, so a printable separator could make two different ref sets collide.
const FIELD_SEP = String.fromCharCode(0);
const REF_SEP = String.fromCharCode(1);

function refSignature(ref: ModelRefLike | null | undefined): string {
  if (!ref) return '~';
  const slot = Array.isArray(ref.slot) ? ref.slot.join(',') : '';
  const guids = ref.guids?.length ? `${ref.guids.length}~${digestGuids(ref.guids)}` : '';
  return [ref.id ?? '', ref.url ?? '', ref.format ?? '', ref.upAxis ?? '', slot, guids].join(
    FIELD_SEP,
  );
}

/**
 * Signature of a whole refs array. Two arrays with the same contents — in the
 * same order, since the order decides the grid slots — share a signature no
 * matter how many times the literal was re-allocated.
 */
export function refsSignature(refs: ReadonlyArray<ModelRefLike> | null | undefined): string {
  if (!Array.isArray(refs)) return '';
  return refs.map(refSignature).join(REF_SEP);
}

/** Signature of a highlight guid set (order-independent, empty for none). */
export function guidsSignature(guids: ReadonlyArray<string> | null | undefined): string {
  if (!guids || !guids.length) return '';
  return `${guids.length}~${digestGuids([...guids])}`;
}
