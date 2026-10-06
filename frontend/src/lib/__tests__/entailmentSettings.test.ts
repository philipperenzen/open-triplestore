// The dataset entailment card: GET/PUT /api/datasets/:id/entailment, the last
// run, the DL backend, and polling while a background run is under way.
import { describe, it, expect, vi, beforeAll, beforeEach } from 'vitest';
import { render, cleanup, fireEvent, waitFor } from '@testing-library/svelte';
import { init, addMessages } from 'svelte-i18n';
import en from '../i18n/en.json';

const api = vi.hoisted(() => ({ getEntailment: vi.fn(), setEntailment: vi.fn() }));
vi.mock('../api', () => api);

import EntailmentSettings from '../../components/reasoning/EntailmentSettings.svelte';

function state(over: Record<string, unknown> = {}) {
  return {
    dataset_id: 'ds1', regime: 'owl2-rl', mode: 'materialize', graph: 'urn:entailment:owl2-rl:ds1',
    last_run_at: '2026-10-06T10:00:00Z', last_triples: 12, consistent: true, inconsistency: null,
    status: 'ok', error: null, backend: null, complete: null,
    identity: 'sameas-narrow', identity_source: 'default', identity_options: ['sameas-off', 'sameas-narrow', 'sameas-full'],
    reasoning_sources: ['https://example.org/g'], dl_backend: null, inference_graph: 'urn:entailment:owl2-rl:ds1',
    rules: { graphs: [], count: 2, error: null, last_run: null },
    ...over,
  };
}

beforeAll(() => {
  Element.prototype.scrollIntoView = () => {};
  addMessages('en', en as unknown as Parameters<typeof addMessages>[1]);
  init({ fallbackLocale: 'en', initialLocale: 'en' });
});

beforeEach(() => {
  cleanup();
  vi.clearAllMocks();
});

async function choose(view: any, label: string, option: string) {
  await fireEvent.click(view.getByRole('button', { name: label }));
  await fireEvent.click(await view.findByRole('option', { name: option }));
}

describe('EntailmentSettings', () => {
  it('shows the regime in force, the last run and that no DL backend is configured', async () => {
    api.getEntailment.mockResolvedValue(state());
    const view = render(EntailmentSettings, { datasetId: 'ds1', canWrite: false });
    expect(await view.findByText('OWL 2 RL')).toBeTruthy();
    expect(view.getByTestId('entailment-status').textContent).toContain('OK');
    expect(view.getByText('Consistent')).toBeTruthy();
    expect(view.getByTestId('dl-backend').textContent).toContain('none configured');
    expect(view.getByText('2 rules')).toBeTruthy();
    expect(view.getByText('Changing it needs write access to the dataset.')).toBeTruthy();
    expect(view.queryByRole('button', { name: 'Save and run' })).toBeNull();
  });

  it('saves regime, mode and identity with PUT', async () => {
    api.getEntailment.mockResolvedValue(state({ regime: null, mode: 'off', status: null }));
    api.setEntailment.mockResolvedValue({});
    const view = render(EntailmentSettings, { datasetId: 'ds1', canWrite: true });
    await view.findByText('None configured');
    await choose(view, 'Regime', 'SKOS');
    expect(view.getByText(/SKOS data model/)).toBeTruthy();
    await choose(view, 'Identity policy (owl:sameAs)', 'Full');
    await fireEvent.click(view.getByRole('button', { name: 'Save and run' }));
    await waitFor(() => expect(api.setEntailment).toHaveBeenCalledWith('ds1', { regime: 'skos', mode: 'materialize', identity: 'sameas-full' }));
  });

  it('cannot pick owl2-dl without a backend', async () => {
    api.getEntailment.mockResolvedValue(state());
    const view = render(EntailmentSettings, { datasetId: 'ds1', canWrite: true });
    await view.findByText('OWL 2 RL', { selector: 'strong' });
    await fireEvent.click(view.getByRole('button', { name: 'Regime' }));
    const dl = await view.findByRole('option', { name: 'OWL 2 DL' });
    expect(dl.getAttribute('aria-disabled')).toBe('true');
  });

  it('shows an inconsistency the run reported', async () => {
    api.getEntailment.mockResolvedValue(state());
    api.setEntailment.mockRejectedValue(Object.assign(new Error('the ontology is inconsistent (cax-dw): ex:felix'), {
      status: 422, body: { consistent: false, rule: 'cax-dw', detail: 'ex:felix' },
    }));
    const view = render(EntailmentSettings, { datasetId: 'ds1', canWrite: true });
    await view.findByText('OWL 2 RL', { selector: 'strong' });
    await fireEvent.click(view.getByRole('button', { name: 'Save and run' }));
    expect((await view.findByRole('alert')).textContent).toContain('cax-dw');
  });

  it('polls while a background run is queued or running', async () => {
    api.getEntailment
      .mockResolvedValueOnce(state({ regime: 'owl2-dl', status: 'running', dl_backend: 'native' }))
      .mockResolvedValue(state({ regime: 'owl2-dl', status: 'ok', dl_backend: 'native', backend: 'native', complete: false }));
    const view = render(EntailmentSettings, { datasetId: 'ds1', canWrite: false, pollMs: 10 });
    await waitFor(() => expect(view.getByTestId('entailment-status').textContent).toContain('Running'));
    await waitFor(() => expect(view.getByTestId('entailment-status').textContent).toContain('OK'));
    expect(view.getByText('sound, not complete')).toBeTruthy();
    expect(view.getByTestId('dl-backend').textContent).toContain('native');
  });
});
