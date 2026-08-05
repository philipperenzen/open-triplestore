/**
 * Shared client-side "filter as you type" matching.
 *
 * Every in-page filter box (facet rail, term lists, tab pickers…) funnels
 * through here so they all behave the same way:
 *   - an empty / whitespace-only query matches everything;
 *   - matching is case-insensitive and diacritic-insensitive, so "dwarsprofiel"
 *     finds "Dwarsprofiel" and "gebouwdeel" finds "gebouwdeél";
 *   - the query is split on whitespace and EVERY token must appear somewhere in
 *     the candidate's fields, in any order — typing "geo point" finds
 *     "Point geometry" without the user guessing the field order.
 *
 * Keeping this in one place (rather than a closure inside each component) also
 * keeps it unit-testable and avoids the Svelte reactivity trap where a `$:`
 * statement never re-runs because the query is only read inside a helper.
 */

/** Lower-case and strip combining marks so accents don't defeat a match. */
function fold(s: string): string {
  return (s || '')
    .toLowerCase()
    .normalize('NFD')
    .replace(/[\u0300-\u036f]/g, '');
}

/** Split a raw query into folded, non-empty tokens. */
export function queryTokens(query: string): string[] {
  return fold(query).split(/\s+/).filter(Boolean);
}

/**
 * True when every token in `query` appears in at least one of `fields`.
 * `fields` may contain null/undefined/empty entries; they are ignored.
 */
export function matchesQuery(query: string, ...fields: (string | null | undefined)[]): boolean {
  const tokens = queryTokens(query);
  if (!tokens.length) return true;
  const hay = fields.filter(Boolean).map((f) => fold(f as string));
  if (!hay.length) return false;
  return tokens.every((tok) => hay.some((h) => h.includes(tok)));
}

/**
 * Filter a list with `matchesQuery`, using `fieldsOf` to pull the searchable
 * text out of each item. Returns the original array when the query is empty so
 * callers can cheaply keep object identity.
 */
export function filterByQuery<T>(
  items: readonly T[],
  query: string,
  fieldsOf: (item: T) => (string | null | undefined)[],
): T[] {
  const tokens = queryTokens(query);
  if (!tokens.length) return items as T[];
  return items.filter((item) => matchesQuery(query, ...fieldsOf(item)));
}
