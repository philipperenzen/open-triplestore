/**
 * SHACL Studio: IRIs and dataset names as the user sees them.
 *
 * Three defects this locks down, all of which shipped:
 *   1. The Studio pages each carried a private `shortIRI()` that kept only the
 *      last path/hash segment, so `sh:NodeShape` and `ex:NodeShape` both read
 *      as "NodeShape" and the namespace was gone with no way to get it back.
 *   2. Shortening without a tooltip would just trade one unreadable form for
 *      another, so the full IRI has to survive in `title`.
 *   3. Binding targets come back from the API as bare IRIs, and the impact
 *      chips printed the raw dataset id out of the IRI instead of its name.
 *
 * These drive the real pages: the label and the dataset-name join both depend
 * on Svelte reactivity (the dataset list lands *after* the bindings), which a
 * pure unit test of the helpers would not catch.
 */
import { describe, it, expect, beforeAll, beforeEach, vi } from 'vitest';
import { render, waitFor } from '@testing-library/svelte';
import { init, addMessages } from 'svelte-i18n';
import en from '../i18n/en.json';

const SH_NODE_SHAPE = 'http://www.w3.org/ns/shacl#NodeShape';
const EX_NODE_SHAPE = 'https://data.example/def/NodeShape';
const DATASET_IRI = 'https://data.example/dataset/ds-7f3a1c';
const GRAPH_IRI = `${DATASET_IRI}/graphs/instances`;

const getShapeGraph = vi.fn();
const listBindingsForShapeGraph = vi.fn();
const listDatasets = vi.fn();
const listShapeGraphs = vi.fn();
const listOrganisations = vi.fn();
const listPipelines = vi.fn();
const listLatestPipelineRuns = vi.fn();

vi.mock('../api.js', () => ({
  getShapeGraph: (...a: unknown[]) => getShapeGraph(...a),
  listBindingsForShapeGraph: (...a: unknown[]) => listBindingsForShapeGraph(...a),
  listDatasets: (...a: unknown[]) => listDatasets(...a),
  listShapeGraphs: (...a: unknown[]) => listShapeGraphs(...a),
  listOrganisations: (...a: unknown[]) => listOrganisations(...a),
  updateShapeGraph: vi.fn(),
  listShapeGraphRevisions: vi.fn(),
  getShapeGraphRevision: vi.fn(),
  restoreShapeGraphRevision: vi.fn(),
  validateShapeGraph: vi.fn(),
  stageShapeGraph: vi.fn(),
  publishShapeGraph: vi.fn(),
  deprecateShapeGraph: vi.fn(),
  getShapeGraphTurtle: vi.fn(),
  createShapeGraph: vi.fn(),
  deleteShapeGraph: vi.fn(),
  cloneShapeGraph: vi.fn(),
  listPipelines: (...a: unknown[]) => listPipelines(...a),
  listLatestPipelineRuns: (...a: unknown[]) => listLatestPipelineRuns(...a),
  runPipeline: vi.fn(),
}));

// The Turtle editor (CodeMirror), the shapes catalog and the commit log are not
// what these assertions are about, and each pulls in a large dependency tree.
vi.mock('../../components/ShapesEditor.svelte', async () => ({
  default: (await import('./fixtures/NullComponent.svelte')).default,
}));
vi.mock('../../components/ShapesCatalog.svelte', async () => ({
  default: (await import('./fixtures/NullComponent.svelte')).default,
}));
vi.mock('../../components/CommitHistory.svelte', async () => ({
  default: (await import('./fixtures/NullComponent.svelte')).default,
}));

// Imported after the mocks so the pages pick them up.
const ShapeGraphEditor = (await import('../../pages/ShapeGraphEditor.svelte')).default;
const ShapeLibrary = (await import('../../pages/ShapeLibrary.svelte')).default;
const PipelinesList = (await import('../../pages/PipelinesList.svelte')).default;

beforeAll(() => {
  addMessages('en', en as unknown as Parameters<typeof addMessages>[1]);
  init({ fallbackLocale: 'en', initialLocale: 'en' });
});

beforeEach(() => {
  vi.clearAllMocks();
  getShapeGraph.mockResolvedValue({
    id: 'sg-1',
    name: 'Bridge shapes',
    visibility: 'private',
    status: 'draft',
    version: 3,
    shape_count: 2,
    updated_at: new Date().toISOString(),
    target_classes: [SH_NODE_SHAPE, EX_NODE_SHAPE],
  });
  listBindingsForShapeGraph.mockResolvedValue({ targets: [DATASET_IRI, GRAPH_IRI] });
  listDatasets.mockResolvedValue([{ id: 'ds-7f3a1c', name: 'Waalbrug Bridges' }]);
  listShapeGraphs.mockResolvedValue([]);
  listOrganisations.mockResolvedValue([]);
  listPipelines.mockResolvedValue([]);
  listLatestPipelineRuns.mockResolvedValue([]);
});

const PIPELINE = {
  id: 'p-1',
  name: 'Nightly bridge check',
  visibility: 'private',
  shape_graph_ids: ['sg-9'],
  severity_threshold: 'violation',
  targets: [
    { kind: 'dataset', id: 'ds-7f3a1c' },
    { kind: 'shapegraph', id: 'sg-9' },
  ],
};

/** Target-class chips as `[visible text, title]` pairs. */
const targetChips = (container: HTMLElement) =>
  [...container.querySelectorAll('.chip-target')].map((n) => [
    n.textContent?.trim(),
    n.getAttribute('title'),
  ]);

describe('ShapeGraphEditor target classes', () => {
  it('renders a CURIE that keeps the namespace apart, with the full IRI in the tooltip', async () => {
    const { container } = render(ShapeGraphEditor, { id: 'sg-1' });

    await waitFor(() => expect(targetChips(container)).toHaveLength(2));
    // The old truncator produced "NodeShape" for both of these.
    expect(targetChips(container)).toEqual([
      ['sh:NodeShape', SH_NODE_SHAPE],
      ['def:NodeShape', EX_NODE_SHAPE],
    ]);
  });
});

describe('ShapeGraphEditor impact chips', () => {
  it('names the dataset instead of printing its id', async () => {
    const { container } = render(ShapeGraphEditor, { id: 'sg-1' });

    await waitFor(() => {
      const links = [...container.querySelectorAll('.impact-link')];
      expect(links.map((n) => n.textContent?.trim())).toEqual([
        'Waalbrug Bridges',
        'Waalbrug Bridges / instances',
      ]);
    });
  });

  it('links to the dataset page and keeps the target IRI in the tooltip', async () => {
    const { container } = render(ShapeGraphEditor, { id: 'sg-1' });

    await waitFor(() => expect(container.querySelectorAll('.impact-link')).toHaveLength(2));
    const links = [...container.querySelectorAll('.impact-link')];
    expect(links.map((n) => n.getAttribute('href'))).toEqual([
      '/datasets/ds-7f3a1c',
      '/datasets/ds-7f3a1c',
    ]);
    expect(links.map((n) => n.getAttribute('title'))).toEqual([DATASET_IRI, GRAPH_IRI]);
  });

  it('falls back to the raw id when the dataset list has nothing to join on', async () => {
    listDatasets.mockResolvedValue([]);
    const { container } = render(ShapeGraphEditor, { id: 'sg-1' });

    await waitFor(() => {
      const labels = [...container.querySelectorAll('.impact-link')].map((n) => n.textContent?.trim());
      // Never "undefined", never empty.
      expect(labels).toEqual(['ds-7f3a1c', 'ds-7f3a1c / instances']);
    });
  });

  it('re-labels the chips when the dataset list lands after the bindings', async () => {
    let releaseDatasets: (v: unknown) => void = () => {};
    listDatasets.mockReturnValue(new Promise((resolve) => (releaseDatasets = resolve)));

    const { container } = render(ShapeGraphEditor, { id: 'sg-1' });

    // Bindings first: the id is all we have, and it must render sensibly.
    await waitFor(() => {
      expect(container.querySelector('.impact-link')?.textContent?.trim()).toBe('ds-7f3a1c');
    });

    releaseDatasets([{ id: 'ds-7f3a1c', name: 'Waalbrug Bridges' }]);

    await waitFor(() => {
      expect(container.querySelector('.impact-link')?.textContent?.trim()).toBe('Waalbrug Bridges');
    });
  });
});

describe('PipelinesList scope tooltip', () => {
  const tooltip = (container: HTMLElement) =>
    container.querySelector('.scope-bits')?.getAttribute('title') ?? '';

  it('names the dataset and the shape graph instead of printing their ids', async () => {
    listPipelines.mockResolvedValue([PIPELINE]);
    listShapeGraphs.mockResolvedValue([{ id: 'sg-9', name: 'SHACL core meta-shapes' }]);

    const { container } = render(PipelinesList);

    await waitFor(() => {
      expect(tooltip(container)).toContain('Waalbrug Bridges');
      expect(tooltip(container)).toContain('SHACL core meta-shapes');
    });
    expect(tooltip(container)).not.toContain('ds-7f3a1c');
    expect(tooltip(container)).not.toContain('sg-9');
  });

  it('falls back to the ids when the name lookups come back empty', async () => {
    listPipelines.mockResolvedValue([PIPELINE]);
    listDatasets.mockResolvedValue([]);
    listShapeGraphs.mockResolvedValue([]);

    const { container } = render(PipelinesList);

    await waitFor(() => expect(container.querySelector('.scope-bits')).toBeTruthy());
    await waitFor(() => expect(tooltip(container)).toContain('ds-7f3a1c'));
    expect(tooltip(container)).toContain('sg-9');
    expect(tooltip(container)).not.toContain('undefined');
  });
});

describe('ShapeLibrary target classes', () => {
  it('renders CURIEs with the full IRI in the tooltip', async () => {
    listShapeGraphs.mockResolvedValue([
      {
        id: 'sg-1',
        name: 'Bridge shapes',
        visibility: 'private',
        source: 'manual',
        owner_id: 'u1',
        owner_type: 'user',
        status: 'draft',
        version: 1,
        shape_count: 2,
        updated_at: new Date().toISOString(),
        tags: [],
        target_classes: [SH_NODE_SHAPE, EX_NODE_SHAPE],
      },
    ]);

    const { container } = render(ShapeLibrary);

    await waitFor(() => expect(targetChips(container)).toHaveLength(2));
    expect(targetChips(container)).toEqual([
      ['sh:NodeShape', SH_NODE_SHAPE],
      ['def:NodeShape', EX_NODE_SHAPE],
    ]);
  });
});
