// Admin LDES sync jobs: every job with its last outcome and error, a new
// stream added (and run at once), "run now" with the job's own graph, and
// forgetting a job after asking.
import { describe, it, expect, vi, beforeAll, beforeEach } from 'vitest';
import { render, cleanup, fireEvent, waitFor } from '@testing-library/svelte';
import { init, addMessages } from 'svelte-i18n';
import en from '../i18n/en.json';
import { user, authInitialized } from '../stores';

const api = vi.hoisted(() => ({
  adminListLdesSyncs: vi.fn(),
  adminForgetLdesSync: vi.fn(),
  runLdesSync: vi.fn(),
  listDatasets: vi.fn(),
}));
vi.mock('../api', () => api);
const confirm = vi.hoisted(() => ({ askConfirm: vi.fn() }));
vi.mock('../confirm', () => confirm);

import AdminLdes from '../../pages/AdminLdes.svelte';

const OK = {
  dataset_id: 'mirror', dataset_name: 'Road mirror', source_url: 'https://other.test/api/datasets/roads/ldes',
  graph_iri: 'https://x.test/mirror/instances', created_by: 'u1', last_timestamp: '2026-10-06T07:00:00Z',
  members_applied: 42, synced_at: '2026-10-06T07:01:00Z', last_run_at: '2026-10-06T07:01:00Z',
  last_status: 'ok', last_error: null,
  last_report: { nodes_visited: 3, members_seen: 44, entities_updated: 40, entities_deleted: 2, polling_interval: 60, warnings: [] },
};
const FAILED = {
  ...OK, dataset_id: 'bridges', dataset_name: 'Bridges', source_url: 'https://down.test/ldes',
  last_status: 'error', last_error: 'https://down.test/ldes: HTTP 503 after 3 retries', members_applied: 0,
};

beforeAll(() => {
  addMessages('en', en as never);
  init({ fallbackLocale: 'en', initialLocale: 'en' });
});

beforeEach(() => {
  cleanup();
  vi.clearAllMocks();
  user.set({ id: 'u1', username: 'admin', role: 'admin' } as never);
  authInitialized.set(true);
  api.adminListLdesSyncs.mockResolvedValue([OK, FAILED]);
});

describe('AdminLdes', () => {
  it('lists every job with its status, last run and error', async () => {
    const { findByTestId } = render(AdminLdes);
    const card = await findByTestId('ldes-jobs');
    await waitFor(() => expect(card.textContent).toContain('Road mirror'));
    expect(card.textContent).toContain('OK');
    expect(card.textContent).toContain('Failed');
    expect(card.textContent).toContain('HTTP 503 after 3 retries');
    expect(card.textContent).toContain('42');
  });

  it('shows the last report on demand', async () => {
    const { findByTestId, getAllByRole } = render(AdminLdes);
    const card = await findByTestId('ldes-jobs');
    await waitFor(() => expect(card.textContent).toContain('Road mirror'));
    await fireEvent.click(getAllByRole('button', { name: 'Show details' })[0]);
    expect(card.textContent).toContain('40 / 2');
    expect(card.textContent).toContain(OK.last_timestamp);
  });

  it('runs a job now with its own graph', async () => {
    api.runLdesSync.mockResolvedValue({ entities_updated: 1, entities_deleted: 0, nodes_visited: 1, warnings: [] });
    const { findByTestId, getAllByRole } = render(AdminLdes);
    await waitFor(async () => expect((await findByTestId('ldes-jobs')).textContent).toContain('Road mirror'));
    await fireEvent.click(getAllByRole('button', { name: /Run now/ })[0]);
    await waitFor(() => expect(api.runLdesSync).toHaveBeenCalledWith({
      url: OK.source_url, dataset_id: 'mirror', graph_iri: OK.graph_iri,
    }));
    await waitFor(() => expect(api.adminListLdesSyncs).toHaveBeenCalledTimes(2));
  });

  it('adds a stream, checking the form first', async () => {
    api.listDatasets.mockResolvedValue([{ id: 'mirror', name: 'Road mirror' }]);
    api.runLdesSync.mockResolvedValue({ entities_updated: 5, entities_deleted: 0, nodes_visited: 2, warnings: [] });
    const { getByRole, findByTestId } = render(AdminLdes);
    await fireEvent.click(getByRole('button', { name: /Add a stream/ }));
    const form = await findByTestId('ldes-add');
    const url = form.querySelector('input[type="url"]') as HTMLInputElement;
    await fireEvent.input(url, { target: { value: 'ftp://nope' } });
    await fireEvent.click(getByRole('button', { name: /Add and sync/ }));
    expect(form.textContent).toContain(en.pages.adminLdes.errors.url);
    expect(api.runLdesSync).not.toHaveBeenCalled();

    await fireEvent.input(url, { target: { value: 'https://other.test/api/datasets/roads/ldes' } });
    await waitFor(() => expect((form.querySelector('[data-testid="ldes-dataset"]') as HTMLSelectElement).options.length).toBe(2));
    await fireEvent.change(form.querySelector('[data-testid="ldes-dataset"]')!, { target: { value: 'mirror' } });
    await fireEvent.input(form.querySelector('.xc-grid input[type="text"]')!, { target: { value: 'https://x.test/mirror/roads' } });
    await fireEvent.click(getByRole('button', { name: /Add and sync/ }));
    await waitFor(() => expect(api.runLdesSync).toHaveBeenCalledWith({
      url: 'https://other.test/api/datasets/roads/ldes', dataset_id: 'mirror', graph_iri: 'https://x.test/mirror/roads',
    }));
  });

  it('forgets a job only after asking', async () => {
    confirm.askConfirm.mockResolvedValueOnce(false).mockResolvedValueOnce(true);
    api.adminForgetLdesSync.mockResolvedValue('');
    const { findByTestId, getAllByRole } = render(AdminLdes);
    await waitFor(async () => expect((await findByTestId('ldes-jobs')).textContent).toContain('Bridges'));
    await fireEvent.click(getAllByRole('button', { name: /Forget/ })[1]);
    await waitFor(() => expect(confirm.askConfirm).toHaveBeenCalledTimes(1));
    expect(api.adminForgetLdesSync).not.toHaveBeenCalled();
    await fireEvent.click(getAllByRole('button', { name: /Forget/ })[1]);
    await waitFor(() => expect(api.adminForgetLdesSync).toHaveBeenCalledWith('bridges', FAILED.source_url));
  });

  it('sends a non-admin away', async () => {
    user.set({ id: 'u3', username: 'bob', role: 'user' } as never);
    render(AdminLdes);
    await new Promise((r) => setTimeout(r, 0));
    expect(api.adminListLdesSyncs).not.toHaveBeenCalled();
  });
});
