// Running an IDS against a building's `{building}/ids` projection: the panel
// offers only IDS-imported shape graphs and projection graphs, makes a
// pipeline scoped to exactly that pair on first use and reuses it after,
// shows the last kept run, and the results view groups by specification.
import { describe, it, expect, vi, beforeAll, beforeEach } from 'vitest';
import { render, cleanup, fireEvent, waitFor } from '@testing-library/svelte';
import { init, addMessages } from 'svelte-i18n';
import en from '../i18n/en.json';

const api = vi.hoisted(() => ({
  listShapeGraphs: vi.fn(),
  listPipelines: vi.fn(),
  createPipeline: vi.fn(),
  runPipeline: vi.fn(),
  listPipelineRuns: vi.fn(),
  getPipelineRun: vi.fn(),
  listShapesCatalog: vi.fn(),
}));
vi.mock('../api', () => api);

import IdsCheckPanel from '../../components/exchange/IdsCheckPanel.svelte';
import IdsResults from '../../components/exchange/IdsResults.svelte';

const B = 'https://x.test/dataset/d1/building';
const GRAPHS = [{ graph_iri: B }, { graph_iri: `${B}/ifcowl` }, { graph_iri: `${B}/ids` }];
const SGS = [
  { id: 'sg-ids', name: 'Fire safety', graph_iri: 'urn:sg:fire', tags: ['ids'] },
  { id: 'sg-manual', name: 'Hand-written', graph_iri: 'urn:sg:hand', tags: [] },
];
const CATALOG = { graph: 'urn:sg:fire', shapes: [
  { shape: 'urn:ids:walls', kind: 'node', label: 'Walls have a fire rating' },
  { shape: 'urn:ids:walls-exists', kind: 'node', label: 'Walls exist' },
  { shape: 'urn:ids:doors', kind: 'node', label: 'Doors are named' },
  { shape: 'urn:ids:walls/p1', kind: 'property', label: 'x' },
] };
const RUN = {
  id: 'r1', ran_at: '2026-10-06T08:00:00Z', conforms: false,
  report: { results: [
    { source_shape: 'urn:ids:walls', focus_node: `${B}/w1`, message: 'FireRating is missing' },
    { source_shape: 'urn:ids:walls', focus_node: `${B}/w2`, message: 'FireRating is missing' },
  ] },
};

beforeAll(() => {
  addMessages('en', en as never);
  init({ fallbackLocale: 'en', initialLocale: 'en' });
});

beforeEach(() => {
  cleanup();
  vi.clearAllMocks();
  api.listShapeGraphs.mockResolvedValue(SGS);
  api.listShapesCatalog.mockResolvedValue(CATALOG);
  api.listPipelineRuns.mockResolvedValue([]);
});

describe('IdsCheckPanel', () => {
  it('offers IDS shape graphs and projection graphs only', async () => {
    api.listPipelines.mockResolvedValue([]);
    const { findByTestId } = render(IdsCheckPanel, { datasetId: 'd1', graphs: GRAPHS, datasetName: 'Office' });
    const sg = (await findByTestId('ids-shape-graph')) as HTMLSelectElement;
    expect([...sg.options].map((o) => o.textContent)).toEqual(['Fire safety']);
    const g = (await findByTestId('ids-graph')) as HTMLSelectElement;
    expect([...g.options].map((o) => o.value)).toEqual([`${B}/ids`]);
    expect([...g.options].map((o) => o.textContent)).toEqual([B]);
  });

  it('makes a pipeline for the pair on first use, runs it and shows the results', async () => {
    api.listPipelines.mockResolvedValue([]);
    api.createPipeline.mockResolvedValue({ id: 'p1' });
    api.runPipeline.mockResolvedValue(RUN);
    const { getByRole, findByTestId } = render(IdsCheckPanel, { datasetId: 'd1', graphs: GRAPHS, datasetName: 'Office' });
    await findByTestId('ids-shape-graph');
    await fireEvent.click(getByRole('button', { name: /Run check/ }));
    await waitFor(() => expect(api.runPipeline).toHaveBeenCalledWith('p1'));
    expect(api.createPipeline).toHaveBeenCalledWith(expect.objectContaining({
      name: 'IDS check: Fire safety on Office',
      visibility: 'private',
      dataset_ids: ['d1'],
      graph_iris: [`${B}/ids`],
      shape_graph_ids: ['sg-ids'],
    }));
    const results = await findByTestId('ids-results');
    expect(results.textContent).toContain('1 of 2 specification(s) failed');
    expect(results.textContent).toContain('Walls have a fire rating');
    expect(results.textContent).toContain('Doors are named');
  });

  it('reuses the pipeline and opens with its last run', async () => {
    const pipeline = { id: 'p9', dataset_ids: ['d1'], graph_iris: [`${B}/ids`], shape_graph_ids: ['sg-ids'] };
    api.listPipelines.mockResolvedValue([pipeline]);
    api.listPipelineRuns.mockResolvedValue([{ id: 'r1' }]);
    api.getPipelineRun.mockResolvedValue(RUN);
    api.runPipeline.mockResolvedValue({ ...RUN, report: { results: [] } });
    const { getByRole, findByTestId, findByText } = render(IdsCheckPanel, { datasetId: 'd1', graphs: GRAPHS });
    await findByText(/1 of 2 specification/);
    expect(api.getPipelineRun).toHaveBeenCalledWith('p9', 'r1');
    await fireEvent.click(getByRole('button', { name: /Run check/ }));
    await waitFor(() => expect(api.runPipeline).toHaveBeenCalledWith('p9'));
    expect(api.createPipeline).not.toHaveBeenCalled();
    expect((await findByTestId('ids-results')).textContent).toContain('Every specification passed');
  });

  it('points to the library when there is no IDS shape graph', async () => {
    api.listShapeGraphs.mockResolvedValue([SGS[1]]);
    api.listPipelines.mockResolvedValue([]);
    const { findByText } = render(IdsCheckPanel, { datasetId: 'd1', graphs: GRAPHS });
    await findByText(en.components.exchange.ids.noIdsShapes, { exact: false });
  });
});

describe('IdsResults', () => {
  it('lists failing entities and messages under a specification', async () => {
    const specs = [{ shape: 'urn:ids:walls', label: 'Walls have a fire rating' }, { shape: 'urn:ids:slabs', label: 'Slabs exist' }];
    const run = { ran_at: '2026-10-06T08:00:00Z', report: { results: [
      ...RUN.report.results,
      { source_shape: 'urn:ids:slabs-exists', focus_node: 'urn:model', message: 'No slab' },
    ] } };
    const { container, getAllByRole } = render(IdsResults, { specs, run });
    const rows = [...container.querySelectorAll('tbody tr')].map((r) => r.textContent?.replace(/\s+/g, ' ').trim());
    expect(rows[0]).toContain('Walls have a fire rating');
    expect(rows[0]).toContain('Fail');
    expect(rows[0]).toContain('2');
    expect(rows[1]).toContain('No applicable entity');
    await fireEvent.click(getAllByRole('button', { name: 'Show details' })[0]);
    expect(container.textContent).toContain('FireRating is missing');
    expect(container.textContent).toContain('w1');
  });
});
