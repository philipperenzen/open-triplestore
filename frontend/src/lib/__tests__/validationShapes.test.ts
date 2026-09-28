/**
 * The "link shapes" picker on the Validation page. It used to offer only the
 * datasets that already carried a shapes graph, which meant a brand-new shape
 * graph from the Library was unreachable until someone had linked it somewhere
 * else first. These tests pin the Library as the source of the options, and the
 * shape graph's own graph IRI as what gets linked.
 */
import { describe, it, expect, vi, beforeAll, beforeEach } from 'vitest';
import { render, cleanup, fireEvent } from '@testing-library/svelte';
import { init, addMessages } from 'svelte-i18n';
import en from '../i18n/en.json';
import { user, isAuthenticated, authInitialized } from '../stores';

const api = vi.hoisted(() => ({
  listDatasets: vi.fn(),
  validateDataset: vi.fn(),
  updateDatasetShacl: vi.fn(),
  getDataset: vi.fn(),
  getOrganisation: vi.fn(),
  listAccessibleShapeGraphs: vi.fn(),
  listShapeGraphs: vi.fn(),
  listOrganisations: vi.fn(),
  getLatestValidationRun: vi.fn(),
  getValidationHistory: vi.fn(),
  getValidationRun: vi.fn(),
  listLatestValidationRuns: vi.fn(),
  listPublicUsers: vi.fn(),
  getGeoStats: vi.fn(),
  // Avatar, rendered per dataset row, reaches for these.
  getUserAvatarUrl: vi.fn(() => ''),
  getOrgImageUrl: vi.fn(() => ''),
  getDatasetImageUrl: vi.fn(() => ''),
}));
vi.mock('../api.js', () => api);

import Validation from '../../pages/Validation.svelte';

const DATASET = {
  id: 'ds1', name: 'Buildings', owner_type: 'user', owner_id: 'u1',
  shapes_graph_iri: null, shacl_on_write: false,
};

// What GET /api/shacl/shape-graphs returns: the Library, independent of which
// dataset (if any) already uses each graph.
const LIBRARY = [
  { id: 'sg-1', name: 'Building shapes', graph_iri: 'urn:ots:shapes:buildings', shape_count: 7, visibility: 'private', source: 'manual', target_classes: [] },
  { id: 'sg-2', name: 'Address shapes', graph_iri: 'urn:ots:shapes:addresses', shape_count: 1, visibility: 'public', source: 'imported', target_classes: [] },
  { id: 'sg-3', name: 'Geometry shapes', graph_iri: 'urn:ots:shapes:geometry', shape_count: 12, visibility: 'private', source: 'derived', target_classes: [] },
];

beforeAll(() => {
  addMessages('en', en as unknown as Parameters<typeof addMessages>[1]);
  init({ fallbackLocale: 'en', initialLocale: 'en' });
  // jsdom has no layout, so Select's "keep the active option in view" call has
  // nothing to call; without this it rejects asynchronously after each open.
  Element.prototype.scrollIntoView ??= () => {};
});

beforeEach(() => {
  cleanup();
  vi.clearAllMocks();
  user.set({ id: 'u1', username: 'admin', role: 'super_admin' } as never);
  isAuthenticated.set(true);
  authInitialized.set(true);

  api.listDatasets.mockResolvedValue([DATASET]);
  // No dataset anywhere has shapes attached yet — the old option source is empty.
  api.listAccessibleShapeGraphs.mockResolvedValue({ shape_graphs: [] });
  api.listShapeGraphs.mockResolvedValue(LIBRARY);
  api.listOrganisations.mockResolvedValue([]);
  api.listPublicUsers.mockResolvedValue([]);
  api.listLatestValidationRuns.mockResolvedValue([]);
  api.getGeoStats.mockResolvedValue({});
  api.updateDatasetShacl.mockResolvedValue({});
});

/** Open the inline picker on the (single) dataset row and return its options. */
async function openPicker(container: HTMLElement) {
  const trigger = container.querySelector('.inline-shapes-picker button') as HTMLButtonElement;
  expect(trigger).not.toBeNull();
  await fireEvent.click(trigger);
  // Select portals its popup to <body>, so it is outside `container`.
  return [...document.querySelectorAll('.sel-popup [role="option"]')];
}

describe('Validation — link shapes picker', () => {
  it('offers every shape graph in the Library, not only the ones already attached to a dataset', async () => {
    const { findByText, container, unmount } = render(Validation);
    await findByText('Buildings');
    expect(api.listShapeGraphs).toHaveBeenCalledTimes(1);

    const options = await openPicker(container);
    const labels = options.map((o) => o.textContent?.replace(/\s+/g, ' ').trim());
    expect(labels).toHaveLength(3);
    expect(labels[0]).toContain('Building shapes');
    expect(labels[1]).toContain('Address shapes');
    expect(labels[2]).toContain('Geometry shapes');
    // The count is what tells two similarly named graphs apart.
    expect(labels[0]).toContain('7');
    expect(labels[1]).toContain('1');
    unmount();
  });

  it('links the chosen graph by its graph IRI, not its Library id', async () => {
    const { findByText, container, unmount } = render(Validation);
    await findByText('Buildings');

    const options = await openPicker(container);
    await fireEvent.click(options[2]);

    expect(api.updateDatasetShacl).toHaveBeenCalledWith('ds1', {
      shacl_on_write: false,
      shapes_graph_iri: 'urn:ots:shapes:geometry',
    });
    unmount();
  });

  it('renders no picker at all when the Library is empty', async () => {
    api.listShapeGraphs.mockResolvedValue([]);
    const { findByText, container, unmount } = render(Validation);
    await findByText('Buildings');
    expect(container.querySelector('.inline-shapes-picker')).toBeNull();
    unmount();
  });
});
