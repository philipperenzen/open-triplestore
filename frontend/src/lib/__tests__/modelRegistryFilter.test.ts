/**
 * The Model Registry page's kind chips and search box.
 */
import { describe, it, expect } from 'vitest';
import {
  entryKind,
  filterRegistryEntries,
  registrySearchFields,
  type RegistryEntryLike,
} from '../modelRegistryFilter';

const ENTRIES: RegistryEntryLike[] = [
  {
    id: 'asset-model',
    title: 'Asset model',
    namespace: 'https://example.org/asset#',
    description: 'Classes and properties for managed assets and their inspections.',
    kind: 'data-model',
  },
  {
    id: 'status-values',
    title: 'Status values',
    namespace: 'https://example.org/asset/status#',
    description: 'Lifecycle states of an asset.',
    kind: 'vocabulary',
  },
  {
    id: 'prov',
    title: 'PROV-O: The PROV Ontology',
    namespace: 'http://www.w3.org/ns/prov#',
    description: null,
    kind: 'data-model',
  },
  {
    id: 'cafe-menu',
    title: 'Café menu',
    namespace: 'https://example.org/café#',
    description: 'Dishes, drinks and their prices.',
    // No recorded kind: a data model.
  },
];

const ids = (entries: RegistryEntryLike[]) => entries.map((e) => e.id);

describe('entryKind', () => {
  it('is vocabulary only when recorded so', () => {
    expect(entryKind({ kind: 'vocabulary' })).toBe('vocabulary');
    expect(entryKind({ kind: 'data-model' })).toBe('data-model');
    expect(entryKind({ kind: null })).toBe('data-model');
    expect(entryKind({})).toBe('data-model');
  });
});

describe('registrySearchFields', () => {
  it('searches the id, title, namespace and description', () => {
    expect(registrySearchFields(ENTRIES[0])).toEqual([
      'asset-model',
      'Asset model',
      'https://example.org/asset#',
      'Classes and properties for managed assets and their inspections.',
    ]);
  });
});

describe('filterRegistryEntries', () => {
  it('keeps every entry for an empty or blank query', () => {
    expect(filterRegistryEntries(ENTRIES, '')).toBe(ENTRIES);
    expect(ids(filterRegistryEntries(ENTRIES, '   '))).toEqual(ids(ENTRIES));
  });

  it('matches every word, in any order, across fields', () => {
    // "status" is in the title, "lifecycle" only in the description.
    expect(ids(filterRegistryEntries(ENTRIES, 'lifecycle status'))).toEqual(['status-values']);
    expect(ids(filterRegistryEntries(ENTRIES, 'status lifecycle'))).toEqual(['status-values']);
    // A word that no field holds rules the entry out.
    expect(ids(filterRegistryEntries(ENTRIES, 'status bridge'))).toEqual([]);
  });

  it('finds an entry by its namespace', () => {
    expect(ids(filterRegistryEntries(ENTRIES, 'w3.org/ns/prov'))).toEqual(['prov']);
    expect(ids(filterRegistryEntries(ENTRIES, 'example.org/asset'))).toEqual([
      'asset-model',
      'status-values',
    ]);
  });

  it('finds an entry by its description', () => {
    expect(ids(filterRegistryEntries(ENTRIES, 'inspections'))).toEqual(['asset-model']);
  });

  it('ignores case and accents on both sides', () => {
    expect(ids(filterRegistryEntries(ENTRIES, 'CAFE'))).toEqual(['cafe-menu']);
    expect(ids(filterRegistryEntries(ENTRIES, 'café drinks'))).toEqual(['cafe-menu']);
    expect(ids(filterRegistryEntries(ENTRIES, 'prov ONTOLOGY'))).toEqual(['prov']);
  });

  it('applies the kind filter before the search', () => {
    expect(ids(filterRegistryEntries(ENTRIES, 'asset', 'vocabulary'))).toEqual(['status-values']);
    expect(ids(filterRegistryEntries(ENTRIES, 'asset', 'data-model'))).toEqual(['asset-model']);
    expect(ids(filterRegistryEntries(ENTRIES, '', 'data-model'))).toEqual([
      'asset-model',
      'prov',
      'cafe-menu',
    ]);
  });
});
