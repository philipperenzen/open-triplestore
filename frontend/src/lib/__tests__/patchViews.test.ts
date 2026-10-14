// RDF Patch on the dataset patch page: apply (POST …/patch) or apply and record
// (POST …/log, the headers added), and the patch log (GET …/log, a patch by
// version, download links).
import { describe, it, expect, vi, beforeAll, beforeEach } from 'vitest';
import { render, cleanup, fireEvent, waitFor } from '@testing-library/svelte';
import { init, addMessages } from 'svelte-i18n';
import en from '../i18n/en.json';

const api = vi.hoisted(() => ({
  applyDatasetPatch: vi.fn(),
  appendDatasetPatchLog: vi.fn(),
  getDatasetPatchLog: vi.fn(),
  getDatasetLogPatch: vi.fn(),
  datasetLogPatchUrl: (id: string, ref: string | number) => `/api/datasets/${id}/log/patch/${ref}`,
  datasetLogInitUrl: (id: string) => `/api/datasets/${id}/log/init`,
  datasetLogCurrentUrl: (id: string) => `/api/datasets/${id}/log/current`,
}));
vi.mock('../api', () => api);

import PatchApply from '../../components/reasoning/PatchApply.svelte';
import PatchLog from '../../components/reasoning/PatchLog.svelte';

beforeAll(() => {
  Element.prototype.scrollIntoView = () => {};
  addMessages('en', en as unknown as Parameters<typeof addMessages>[1]);
  init({ fallbackLocale: 'en', initialLocale: 'en' });
});

beforeEach(() => {
  cleanup();
  vi.clearAllMocks();
});

const PATCH = 'A <http://e/s> <http://e/p> "o" <http://e/g> .';

describe('PatchApply', () => {
  it('records the patch in the log, adding H id and H prev', async () => {
    api.appendDatasetPatchLog.mockResolvedValue({ applied: true, added: 1, removed: 0, version: 3, prev: 'urn:uuid:2' });
    const view = render(PatchApply, { datasetId: 'ds1', canWrite: true, graphs: ['http://e/g'], latestId: 'urn:uuid:2' });
    await fireEvent.input(view.getByLabelText('Patch'), { target: { value: PATCH } });
    await fireEvent.click(view.getByRole('button', { name: 'Apply patch' }));
    await waitFor(() => expect(api.appendDatasetPatchLog).toHaveBeenCalled());
    const [id, text, graph] = api.appendDatasetPatchLog.mock.calls[0];
    expect(id).toBe('ds1');
    expect(text).toMatch(/^H id <urn:uuid:[^>]+> \.\nH prev <urn:uuid:2> \.\nA </);
    expect(graph).toBeNull();
    expect((await view.findByTestId('patch-result')).textContent).toContain('Applied: 1 added, 0 removed');
    expect(view.getByTestId('patch-result').textContent).toContain('log version 3');
  });

  it('applies without journaling, triples to the chosen graph', async () => {
    api.applyDatasetPatch.mockResolvedValue({ applied: true, added: 2, removed: 1 });
    const view = render(PatchApply, { datasetId: 'ds1', canWrite: true, graphs: ['http://e/g'] });
    await fireEvent.input(view.getByLabelText('Patch'), { target: { value: 'A <s> <p> <o> .' } });
    await fireEvent.click(view.getByRole('button', { name: 'Graph for triples' }));
    await fireEvent.click(await view.findByRole('option', { name: 'http://e/g' }));
    await fireEvent.click(view.getByLabelText("Record it in the dataset's patch log"));
    await fireEvent.click(view.getByRole('button', { name: 'Apply patch' }));
    await waitFor(() => expect(api.applyDatasetPatch).toHaveBeenCalledWith('ds1', 'A <s> <p> <o> .', 'http://e/g'));
    expect(api.appendDatasetPatchLog).not.toHaveBeenCalled();
  });

  it('shows a SHACL gate refusal with its report', async () => {
    api.appendDatasetPatchLog.mockRejectedValue(Object.assign(new Error('A SHACL write gate refused the patch'), {
      status: 422, body: { error: 'refused', report: { conforms: false, results_count: 1 } },
    }));
    const view = render(PatchApply, { datasetId: 'ds1', canWrite: true, graphs: [] });
    await fireEvent.input(view.getByLabelText('Patch'), { target: { value: PATCH } });
    await fireEvent.click(view.getByRole('button', { name: 'Apply patch' }));
    const alert = await view.findByRole('alert');
    expect(alert.textContent).toContain('SHACL write gate');
    expect(alert.textContent).toContain('results_count');
  });

  it('needs write access', () => {
    const view = render(PatchApply, { datasetId: 'ds1', canWrite: false });
    expect(view.queryByRole('button', { name: 'Apply patch' })).toBeNull();
  });
});

describe('PatchLog', () => {
  const LOG = {
    name: 'ds1', latest: { version: 2, id: 'urn:uuid:2' },
    entries: [
      { version: 1, id: 'urn:uuid:1', kind: 'version', author: 'admin', dataset_version: '1.0.0', graphs: ['http://e/g'], created_at: '2026-10-06T09:00:00Z' },
      { version: 2, id: 'urn:uuid:2', withheld: true },
    ],
  };

  it('lists the entries, withholds what may not be read and shows a patch', async () => {
    api.getDatasetPatchLog.mockResolvedValue(LOG);
    api.getDatasetLogPatch.mockResolvedValue('H id <urn:uuid:1> .\nA <s> <p> <o> <http://e/g> .');
    const view = render(PatchLog, { datasetId: 'ds1' });
    const rows = await view.findAllByTestId('log-entry');
    expect(rows).toHaveLength(2);
    expect(rows[0].textContent).toContain('Version cut');
    expect(rows[0].textContent).toContain('v1.0.0');
    expect(rows[1].textContent).toContain('Changes a graph you may not read.');
    expect(view.getByRole('link', { name: 'Download patch 1' }).getAttribute('href')).toBe('/api/datasets/ds1/log/patch/1');
    expect(view.queryByRole('link', { name: 'Download patch 2' })).toBeNull();
    await fireEvent.click(view.getByRole('button', { name: 'Show patch 1' }));
    expect((await view.findByTestId('log-patch-text')).textContent).toContain('A <s> <p> <o>');
    expect(api.getDatasetLogPatch).toHaveBeenCalledWith('ds1', 1);
  });

  it('says when the log is empty', async () => {
    api.getDatasetPatchLog.mockResolvedValue({ name: 'ds1', latest: null, entries: [] });
    const view = render(PatchLog, { datasetId: 'ds1' });
    expect(await view.findByText('The log is empty.')).toBeTruthy();
  });
});
