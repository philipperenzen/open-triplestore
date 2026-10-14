// Stored SWRL rules (GET/POST/PUT/DELETE /api/datasets/:id/rules) and one-off
// runs with a regime (POST /api/swrl/execute) on the dataset reasoning page.
import { describe, it, expect, vi, beforeAll, beforeEach } from 'vitest';
import { render, cleanup, fireEvent, waitFor } from '@testing-library/svelte';
import { init, addMessages } from 'svelte-i18n';
import en from '../i18n/en.json';

const api = vi.hoisted(() => ({
  listDatasetRules: vi.fn(),
  createDatasetRules: vi.fn(),
  updateDatasetRules: vi.fn(),
  deleteDatasetRules: vi.fn(),
  runDatasetRules: vi.fn(),
  executeSwrl: vi.fn(),
}));
vi.mock('../api', () => api);
const confirm = vi.hoisted(() => ({ askConfirm: vi.fn() }));
vi.mock('../confirm', () => confirm);

import DatasetRules from '../../components/reasoning/DatasetRules.svelte';
import SwrlRunner from '../../components/reasoning/SwrlRunner.svelte';

const REPORT = { rule_graphs: ['urn:ots:rules:ds1'], rules: 1, target_graph: 'urn:entailment:swrl:ds1', triples_inferred: 4, rounds: 1, converged: true, error: null, ran_at: '2026-10-06T10:00:00Z' };
const SET = {
  id: '6f1c', iri: 'urn:ots:ruleset:6f1c', label: 'Inspection', format: 'swrlapi',
  source: 'ex:Asset(?x) -> ex:Inspected(?x)', options: { prefixes: { ex: 'https://example.org/' } },
  updated_at: '2026-10-06T10:00:00Z', updated_by: 'admin',
  rules: [{ iri: 'urn:ots:ruleset:6f1c:1', functional: 'DLSafeRule(Body(ClassAtom(<https://example.org/Asset> Variable(<urn:ots:ruleset:6f1c:1#x>))) Head(…))' }],
};
function rules(over: Record<string, unknown> = {}) {
  return {
    dataset_id: 'ds1', graph: 'urn:ots:rules:ds1', can_write: true, sets: [SET],
    other: [{ graph: 'https://example.org/model', rules: [{ iri: 'https://example.org/r', functional: 'DLSafeRule(…)' }], error: null }],
    inference_graph: 'urn:entailment:swrl:ds1', last_run: REPORT, formats: ['text', 'swrlapi'],
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

describe('DatasetRules', () => {
  it('lists the stored sets, the read-only rules and the last run', async () => {
    api.listDatasetRules.mockResolvedValue(rules());
    const view = render(DatasetRules, { datasetId: 'ds1', canWrite: true });
    const set = await view.findByTestId('rule-set');
    expect(set.textContent).toContain('Inspection');
    expect(set.textContent).toContain('ex:Asset(?x) -> ex:Inspected(?x)');
    expect(view.getByText('https://example.org/model')).toBeTruthy();
    expect(view.getByTestId('rules-report').textContent).toContain('4');
  });

  it('stores a new set with its prefixes and shows the run', async () => {
    api.listDatasetRules.mockResolvedValue(rules({ sets: [], other: [], last_run: null }));
    api.createDatasetRules.mockResolvedValue({ id: 'n1', rules: 1, run: { regime: null, queued: false, report: REPORT, error: null } });
    const view = render(DatasetRules, { datasetId: 'ds1', canWrite: true });
    await view.findByText('No rules are stored with this dataset.');
    await fireEvent.click(view.getByRole('button', { name: 'Add rules' }));
    await fireEvent.input(view.getByLabelText('Name'), { target: { value: 'Adults' } });
    await fireEvent.input(view.getByLabelText('Prefixes'), { target: { value: 'ex: https://example.org/' } });
    await fireEvent.input(view.getByLabelText('Rules'), { target: { value: 'ex:Person(?p) -> ex:Agent(?p)' } });
    await fireEvent.click(view.getByRole('button', { name: 'Store and run' }));
    await waitFor(() => expect(api.createDatasetRules).toHaveBeenCalledWith('ds1', {
      label: 'Adults', rules: 'ex:Person(?p) -> ex:Agent(?p)', format: 'swrlapi',
      prefixes: { ex: 'https://example.org/' }, rdf_format: undefined, base_iri: undefined,
    }));
    expect(await view.findByTestId('rules-report')).toBeTruthy();
  });

  it('shows why the server refused a rule, naming the built-in', async () => {
    api.listDatasetRules.mockResolvedValue(rules({ sets: [], other: [] }));
    api.createDatasetRules.mockRejectedValue(new Error("Unsupported SWRL builtin 'swrlb:frobnicate'"));
    const view = render(DatasetRules, { datasetId: 'ds1', canWrite: true });
    await fireEvent.click(await view.findByRole('button', { name: 'Add rules' }));
    await fireEvent.input(view.getByLabelText('Rules'), { target: { value: 'x' } });
    await fireEvent.click(view.getByRole('button', { name: 'Store and run' }));
    expect((await view.findByRole('alert')).textContent).toContain('swrlb:frobnicate');
  });

  it('edits a set with PUT, prefilled from its source', async () => {
    api.listDatasetRules.mockResolvedValue(rules());
    api.updateDatasetRules.mockResolvedValue({ id: SET.id, rules: 1, run: { regime: 'owl2-rl', queued: false, report: REPORT, error: null } });
    const view = render(DatasetRules, { datasetId: 'ds1', canWrite: true });
    await fireEvent.click(await view.findByRole('button', { name: 'Edit Inspection' }));
    expect((view.getByLabelText('Rules') as HTMLTextAreaElement).value).toBe(SET.source);
    expect((view.getByLabelText('Prefixes') as HTMLTextAreaElement).value).toBe('ex: https://example.org/');
    await fireEvent.click(view.getByRole('button', { name: 'Save changes' }));
    await waitFor(() => expect(api.updateDatasetRules).toHaveBeenCalledWith('ds1', SET.id, expect.objectContaining({ rules: SET.source, format: 'swrlapi' })));
  });

  it('removes a set only after the app dialog confirms', async () => {
    api.listDatasetRules.mockResolvedValue(rules());
    api.deleteDatasetRules.mockResolvedValue({ deleted: SET.id, run: { regime: null, queued: false, report: null, error: null } });
    confirm.askConfirm.mockResolvedValueOnce(false).mockResolvedValueOnce(true);
    const view = render(DatasetRules, { datasetId: 'ds1', canWrite: true });
    const del = await view.findByRole('button', { name: 'Remove Inspection' });
    await fireEvent.click(del);
    await waitFor(() => expect(confirm.askConfirm).toHaveBeenCalledTimes(1));
    expect(api.deleteDatasetRules).not.toHaveBeenCalled();
    await fireEvent.click(del);
    await waitFor(() => expect(api.deleteDatasetRules).toHaveBeenCalledWith('ds1', SET.id));
  });

  it('is read-only without write access', async () => {
    api.listDatasetRules.mockResolvedValue(rules({ can_write: false }));
    const view = render(DatasetRules, { datasetId: 'ds1', canWrite: false });
    await view.findByTestId('rule-set');
    expect(view.queryByRole('button', { name: 'Add rules' })).toBeNull();
    expect(view.queryByRole('button', { name: 'Remove Inspection' })).toBeNull();
  });
});

describe('SwrlRunner', () => {
  it('runs the rules with a regime and reports rounds and regime triples', async () => {
    api.executeSwrl.mockResolvedValue({ rules_count: 1, iterations: 2, triples_inferred: 3, converged: true, stop_reason: 'fixpoint', regime: 'owl2-rl', rounds: 2, regime_triples: 7, target_graph: 'urn:entailment:owl2-rl:ds1', sources: ['g'], rule_results: [] });
    const view = render(SwrlRunner, { datasetId: 'ds1', canWrite: true, inferenceGraph: 'urn:entailment:owl2-rl:ds1' });
    await fireEvent.input(view.getByLabelText('Rules'), { target: { value: 'ex:A(?x) -> ex:B(?x)' } });
    await choose(view, 'Run jointly with', 'OWL 2 RL');
    await fireEvent.click(view.getByRole('button', { name: 'Run rules' }));
    await waitFor(() => expect(api.executeSwrl).toHaveBeenCalledWith(expect.objectContaining({ dataset: 'ds1', regime: 'owl2-rl', rules: 'ex:A(?x) -> ex:B(?x)', format: 'swrlapi', max_iterations: 100 })));
    expect((await view.findByTestId('swrl-rounds')).textContent).toBe('2');
    expect(view.getByTestId('swrl-regime-triples').textContent).toBe('7');
  });

  it('shows the refusal message, which names the built-in', async () => {
    api.executeSwrl.mockRejectedValue(new Error('swrlb:add(?z, ?x, ?y): infinitely many solutions with ?x, ?y unbound'));
    const view = render(SwrlRunner, { datasetId: 'ds1', canWrite: true });
    await fireEvent.input(view.getByLabelText('Rules'), { target: { value: 'r' } });
    await fireEvent.click(view.getByRole('button', { name: 'Run rules' }));
    expect((await view.findByTestId('swrl-error')).textContent).toContain('swrlb:add');
  });
});
