// Materialise, DL checks, background jobs and QL rewriting on the dataset
// reasoning page: POST /api/reasoning/materialize|check (sync and ?async=true),
// GET /api/reasoning/jobs, POST /api/reasoning/rewrite.
import { describe, it, expect, vi, beforeAll, beforeEach } from 'vitest';
import { render, cleanup, fireEvent, waitFor } from '@testing-library/svelte';
import { init, addMessages } from 'svelte-i18n';
import en from '../i18n/en.json';

const api = vi.hoisted(() => ({
  materializeEntailment: vi.fn(),
  reasoningCheck: vi.fn(),
  listReasoningJobs: vi.fn(),
  rewriteQuery: vi.fn(),
}));
vi.mock('../api', () => api);
vi.mock('../clipboard', () => ({ copyOrWarn: vi.fn().mockResolvedValue(true) }));

import MaterialisePanel from '../../components/reasoning/MaterialisePanel.svelte';
import ConsistencyCheck from '../../components/reasoning/ConsistencyCheck.svelte';
import ReasoningJobs from '../../components/reasoning/ReasoningJobs.svelte';
import QlRewrite from '../../components/reasoning/QlRewrite.svelte';

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

describe('MaterialisePanel', () => {
  it('materialises into the dataset graph and shows the ignored axioms', async () => {
    api.materializeEntailment.mockResolvedValue({
      regime: 'owl2-el', triples_added: 5, iterations: 2, elapsed_ms: 12, target_graph: 'urn:entailment:owl2-el:ds1',
      sources: ['g1'], consistent: true, ignored: [{ construct: 'ObjectUnionOf', count: 2, example: 'ex:A or ex:B' }],
    });
    const view = render(MaterialisePanel, { datasetId: 'ds1', canWrite: true, dlBackend: null });
    await choose(view, 'Regime to materialise', 'OWL 2 EL');
    expect((view.getByLabelText('Target graph') as HTMLInputElement).placeholder).toBe('urn:entailment:owl2-el:ds1');
    await fireEvent.click(view.getByRole('button', { name: 'Materialise' }));
    await waitFor(() => expect(api.materializeEntailment).toHaveBeenCalledWith({ regime: 'owl2-el', dataset: 'ds1' }, false));
    expect(await view.findByTestId('materialise-report')).toBeTruthy();
    expect(view.getByTestId('ignored-axioms').textContent).toContain('ObjectUnionOf');
    expect(view.getByText(/2 axioms are outside the profile/)).toBeTruthy();
  });

  it('sends eq_ref for OWL 2 RL and queues a background job', async () => {
    api.materializeEntailment.mockResolvedValue({ job_id: 'j1', status: 'queued', location: '/api/reasoning/jobs/j1' });
    const queued = vi.fn();
    // Svelte 5 still wires createEventDispatcher events through mount's `events`.
    const view = render(MaterialisePanel, {
      props: { datasetId: 'ds1', canWrite: true, dlBackend: 'native' },
      events: { queued: (e: CustomEvent) => queued(e.detail) },
    } as unknown as Parameters<typeof render>[1]);
    await fireEvent.click(view.getByLabelText(/eq-ref/));
    await fireEvent.click(view.getByLabelText('Run in the background'));
    await fireEvent.click(view.getByRole('button', { name: 'Materialise' }));
    await waitFor(() => expect(api.materializeEntailment).toHaveBeenCalledWith({ regime: 'owl2-rl', dataset: 'ds1', eq_ref: true }, true));
    await waitFor(() => expect(queued).toHaveBeenCalledWith({ jobId: 'j1' }));
  });

  it('explains a 503 without a DL backend', async () => {
    api.materializeEntailment.mockRejectedValue(Object.assign(new Error('owl2-dl needs OTS_DL_BACKEND'), { status: 503, body: { error: 'owl2-dl needs OTS_DL_BACKEND' } }));
    const view = render(MaterialisePanel, { datasetId: 'ds1', canWrite: true, dlBackend: 'native' });
    await fireEvent.click(view.getByRole('button', { name: 'Materialise' }));
    expect((await view.findByTestId('run-failure')).textContent).toContain('not configured or cannot be reached');
  });

  it('asks for write access', () => {
    const view = render(MaterialisePanel, { datasetId: 'ds1', canWrite: false });
    expect(view.getByText('This needs write access to the dataset.')).toBeTruthy();
    expect(view.queryByRole('button', { name: 'Materialise' })).toBeNull();
  });
});

describe('ConsistencyCheck', () => {
  it('offers only the profile check without a DL backend', async () => {
    api.reasoningCheck.mockResolvedValue({ task: 'profile', result: 'false', regime: 'owl2-dl', backend: 'native', complete: true, sources: ['g'], in_profile: false, violations: [{ rule: 'DL-1', detail: 'a reflexive role in a cardinality' }] });
    const view = render(ConsistencyCheck, { datasetId: 'ds1', dlBackend: null });
    expect(view.getByText(/only the OWL 2 DL profile check/)).toBeTruthy();
    await fireEvent.click(view.getByRole('button', { name: 'Run check' }));
    await waitFor(() => expect(api.reasoningCheck).toHaveBeenCalledWith({ task: 'profile', dataset: 'ds1' }, false));
    const result = await view.findByTestId('check-result');
    expect(result.textContent).toContain('Not in OWL 2 DL');
    expect(result.textContent).toContain('DL-1');
  });

  it('checks a class over a Turtle premise', async () => {
    api.reasoningCheck.mockResolvedValue({ task: 'satisfiability', result: 'false', regime: 'owl2-dl', backend: 'native', complete: false, sources: null });
    const view = render(ConsistencyCheck, { datasetId: 'ds1', dlBackend: 'native' });
    await choose(view, 'Check', 'Class satisfiability');
    await fireEvent.input(view.getByLabelText('Class IRI'), { target: { value: 'http://example.org/Nothing' } });
    await fireEvent.click(view.getByLabelText('A Turtle premise'));
    await fireEvent.input(view.getByLabelText('Premise (Turtle)'), { target: { value: '<a> <b> <c> .' } });
    await fireEvent.click(view.getByRole('button', { name: 'Run check' }));
    await waitFor(() => expect(api.reasoningCheck).toHaveBeenCalledWith({ task: 'satisfiability', premise: '<a> <b> <c> .', class: 'http://example.org/Nothing' }, false));
    expect((await view.findByTestId('check-result')).textContent).toContain('The class is unsatisfiable');
  });
});

describe('ReasoningJobs', () => {
  it('lists the jobs, polls while one runs and shows a finished result', async () => {
    const running = { id: 'j1', kind: 'materialize', dataset: 'ds1', status: 'running', created_at: '2026-10-06T10:00:00Z', started_at: null, finished_at: null, http_status: null, result: null };
    const done = { ...running, status: 'succeeded', finished_at: '2026-10-06T10:00:05Z', http_status: 200, result: { regime: 'rdfs', triples_added: 3, iterations: 1, elapsed_ms: 4, target_graph: 'urn:entailment:rdfs:ds1', sources: [], consistent: null } };
    api.listReasoningJobs.mockResolvedValueOnce({ jobs: [running] }).mockResolvedValue({ jobs: [done] });
    const view = render(ReasoningJobs, { datasetId: 'ds1', pollMs: 10 });
    expect((await view.findByTestId('job-row')).textContent).toContain('Running');
    await waitFor(() => expect(view.getByTestId('job-row').textContent).toContain('Succeeded'));
    expect(api.listReasoningJobs).toHaveBeenCalledWith('ds1');
    await fireEvent.click(view.getByRole('button', { name: 'Result' }));
    expect((await view.findByTestId('materialise-report')).textContent).toContain('3');
  });

  it('shows a failed job with the error body', async () => {
    api.listReasoningJobs.mockResolvedValue({ jobs: [{ id: 'j2', kind: 'check', dataset: null, status: 'failed', created_at: 'x', started_at: 'x', finished_at: 'x', http_status: 504, result: { error: 'timed out', result: 'unknown' } }] });
    const view = render(ReasoningJobs, {});
    await view.findByTestId('job-row');
    await fireEvent.click(view.getByRole('button', { name: 'Result' }));
    expect((await view.findByTestId('run-failure')).textContent).toContain('did not answer in time');
  });
});

describe('QlRewrite', () => {
  it('shows the rewritten query', async () => {
    api.rewriteQuery.mockResolvedValue({ rewritten: 'SELECT ?x WHERE { { ?x a <A> } UNION { ?x a <B> } }' });
    const view = render(QlRewrite);
    await fireEvent.click(view.getByRole('button', { name: 'Rewrite' }));
    expect((await view.findByTestId('rewritten-query')).textContent).toContain('UNION');
    expect(api.rewriteQuery).toHaveBeenCalledWith(expect.stringContaining('SELECT ?x'), 'owl2-ql');
  });
});
