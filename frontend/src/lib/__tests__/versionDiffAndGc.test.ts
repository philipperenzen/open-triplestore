// Dataset version diff (GET …/versions/:ver/diff/:other, JSON and RDF Patch) and
// retention (POST …/versions/gc) behind the app's own confirm dialog.
import { describe, it, expect, vi, beforeAll, beforeEach } from 'vitest';
import { render, cleanup, fireEvent, waitFor } from '@testing-library/svelte';
import { init, addMessages } from 'svelte-i18n';
import en from '../i18n/en.json';

const api = vi.hoisted(() => ({
  getDataset: vi.fn(),
  listDatasetVersions: vi.fn(),
  diffDatasetVersions: vi.fn(),
  diffDatasetVersionsPatch: vi.fn(),
  datasetVersionDiffPatchUrl: (id: string, a: string, b: string) => `/api/datasets/${id}/versions/${a}/diff/${b}?format=rdf-patch`,
  getDatasetBranches: vi.fn(),
  createDatasetVersion: vi.fn(),
  stageDatasetVersion: vi.fn(),
  publishDatasetVersion: vi.fn(),
  deprecateDatasetVersion: vi.fn(),
  restoreDatasetVersion: vi.fn(),
  createDatasetBranch: vi.fn(),
  getDatasetVersionDataUrl: () => '/x',
  gcDatasetVersions: vi.fn(),
}));
vi.mock('../api', () => api);
vi.mock('../api.js', () => api);
const confirm = vi.hoisted(() => ({ askConfirm: vi.fn() }));
vi.mock('../confirm', () => confirm);

import DatasetVersionDiff from '../../pages/DatasetVersionDiff.svelte';
import DatasetVersions from '../../components/DatasetVersions.svelte';

const v = (version: string, status: string, created_at: string) => ({ version, status, created_at, snapshot_graphs: ['g'], source_map: [], branch: null, notes: null });
const VERSIONS = [v('1.2.0', 'draft', '2026-03-01T00:00:00Z'), v('1.1.0', 'staged', '2026-02-01T00:00:00Z'), v('1.0.0', 'published', '2026-01-01T00:00:00Z')];

beforeAll(() => {
  Element.prototype.scrollIntoView = () => {};
  addMessages('en', en as unknown as Parameters<typeof addMessages>[1]);
  init({ fallbackLocale: 'en', initialLocale: 'en' });
});

beforeEach(() => {
  cleanup();
  vi.clearAllMocks();
  window.history.replaceState({}, '', '/datasets/ds1/versions/diff?from=1.0.0&to=live');
});

describe('DatasetVersionDiff', () => {
  it('shows per-graph counts and then the changes from the RDF Patch', async () => {
    api.getDataset.mockResolvedValue({ id: 'ds1', name: 'Bridges' });
    api.listDatasetVersions.mockResolvedValue(VERSIONS);
    api.diffDatasetVersions.mockResolvedValue({ dataset_id: 'ds1', from: '1.0.0', to: 'live', added: 1, removed: 1, graphs: [{ source_graph: 'http://e/g', added: 1, removed: 1 }] });
    api.diffDatasetVersionsPatch.mockResolvedValue('H id <urn:uuid:1> .\nA <http://e/s> <http://e/p> "new" <http://e/g> .\nD <http://e/s> <http://e/p> "old" <http://e/g> .');
    const view = render(DatasetVersionDiff, { id: 'ds1' });
    const diff = await view.findByTestId('version-diff');
    expect(api.diffDatasetVersions).toHaveBeenCalledWith('ds1', '1.0.0', 'live');
    expect(diff.textContent).toContain('http://e/g');
    expect(view.getByRole('link', { name: /Download as RDF Patch/ }).getAttribute('href')).toBe('/api/datasets/ds1/versions/1.0.0/diff/live?format=rdf-patch');
    await fireEvent.click(view.getByRole('button', { name: 'Show the changes' }));
    const rows = await view.findAllByTestId('diff-row');
    expect(rows).toHaveLength(2);
    expect(rows[0].textContent).toContain('"new"');
    await fireEvent.click(view.getByRole('button', { name: 'Removed' }));
    expect(view.getAllByTestId('diff-row')).toHaveLength(1);
    expect(view.getByTestId('diff-row').textContent).toContain('"old"');
  });
});

describe('DatasetVersions retention', () => {
  it('previews what goes and deletes only after the app dialog confirms', async () => {
    api.listDatasetVersions.mockResolvedValue(VERSIONS);
    api.getDatasetBranches.mockResolvedValue([]);
    api.gcDatasetVersions.mockResolvedValue({ kept: 1, deleted: ['1.1.0'] });
    confirm.askConfirm.mockResolvedValue(true);
    const view = render(DatasetVersions, { id: 'ds1', canWrite: true, graphs: [] });
    await fireEvent.click(await view.findByRole('button', { name: /Clean up/ }));
    const keep = view.getByLabelText('Unpublished versions to keep');
    await fireEvent.input(keep, { target: { value: '1' } });
    const panel = view.getByTestId('version-gc');
    expect(panel.textContent).toContain('1 version will be deleted');
    expect(panel.textContent).toContain('1.1.0');
    expect(panel.textContent).not.toContain('1.0.0');
    const buttons = view.getAllByRole('button', { name: /Clean up/ });
    await fireEvent.click(buttons[buttons.length - 1]);
    await waitFor(() => expect(confirm.askConfirm).toHaveBeenCalledWith(expect.objectContaining({ variant: 'danger' })));
    await waitFor(() => expect(api.gcDatasetVersions).toHaveBeenCalledWith('ds1', 1));
  });

  it('does not delete when the dialog is cancelled, and offers a diff link per version', async () => {
    api.listDatasetVersions.mockResolvedValue(VERSIONS);
    api.getDatasetBranches.mockResolvedValue([]);
    confirm.askConfirm.mockResolvedValue(false);
    const view = render(DatasetVersions, { id: 'ds1', canWrite: true, graphs: [] });
    const diffs = await view.findAllByRole('link', { name: /Diff/ });
    expect(diffs[0].getAttribute('href')).toBe('/datasets/ds1/versions/diff?from=1.2.0&to=live');
    await fireEvent.click(view.getByRole('button', { name: /Clean up/ }));
    await fireEvent.input(view.getByLabelText('Unpublished versions to keep'), { target: { value: '0' } });
    const buttons = view.getAllByRole('button', { name: /Clean up/ });
    await fireEvent.click(buttons[buttons.length - 1]);
    await waitFor(() => expect(confirm.askConfirm).toHaveBeenCalled());
    expect(api.gcDatasetVersions).not.toHaveBeenCalled();
  });
});
