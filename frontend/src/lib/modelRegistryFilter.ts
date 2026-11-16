/**
 * The Model Registry page's list filter: the kind chips and the search box.
 *
 * The search box uses the shared {@link filterByQuery} matching: every
 * whitespace-separated word of the query must appear, in any order and
 * ignoring case and accents, somewhere in the entry's id, title, namespace or
 * description — "asset status" finds an entry titled "Status values" whose
 * namespace is `https://example.org/asset#`.
 */
import { filterByQuery } from './searchMatch';

export type RegistryKind = 'data-model' | 'vocabulary';
export type RegistryKindFilter = 'all' | RegistryKind;

/** The registry entry fields the filter reads. */
export interface RegistryEntryLike {
  id: string;
  title?: string | null;
  namespace?: string | null;
  description?: string | null;
  kind?: string | null;
}

/**
 * An entry's kind: `vocabulary` (SKOS concept scheme, property set) or
 * `data-model` (OWL/RDFS ontology). The backend auto-detects it on upload and
 * persists it on the record; an entry with no recorded kind is a data model.
 */
export function entryKind(entry: Pick<RegistryEntryLike, 'kind'>): RegistryKind {
  return entry.kind === 'vocabulary' ? 'vocabulary' : 'data-model';
}

/** The text of an entry the search box looks in. */
export function registrySearchFields(entry: RegistryEntryLike): (string | null | undefined)[] {
  return [entry.id, entry.title, entry.namespace, entry.description];
}

/** The entries of `kind` (all of them for `'all'`) that match `query`. */
export function filterRegistryEntries<T extends RegistryEntryLike>(
  entries: readonly T[],
  query: string,
  kind: RegistryKindFilter = 'all',
): T[] {
  const ofKind = kind === 'all' ? entries : entries.filter((e) => entryKind(e) === kind);
  return filterByQuery<T>(ofKind, query, registrySearchFields);
}
