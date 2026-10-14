// ShEx validation (POST /api/datasets/:id/shex/validate and /api/shex/validate):
// the three ways to say what to validate, and the verdicts view.
import { describe, it, expect, vi, beforeAll, beforeEach } from 'vitest';
import { render, cleanup, fireEvent, waitFor } from '@testing-library/svelte';
import { init, addMessages } from 'svelte-i18n';
import en from '../i18n/en.json';

const api = vi.hoisted(() => ({ validateShex: vi.fn() }));
vi.mock('../api', () => api);

import ShexValidator from '../../components/reasoning/ShexValidator.svelte';

const REPORT = {
  conforms: false,
  results: [
    { focus_node: 'https://example.org/alice', shape: 'https://example.org/PersonShape', status: 'Conformant' },
    { focus_node: 'https://example.org/bob', shape: 'https://example.org/PersonShape', status: { NonConformant: 'missing ex:name' } },
  ],
};

beforeAll(() => {
  Element.prototype.scrollIntoView = () => {};
  addMessages('en', en as unknown as Parameters<typeof addMessages>[1]);
  init({ fallbackLocale: 'en', initialLocale: 'en' });
});

beforeEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe('ShexValidator', () => {
  it('validates every shape by default and shows the verdicts', async () => {
    api.validateShex.mockResolvedValue(REPORT);
    const view = render(ShexValidator, { datasetId: 'ds1' });
    await fireEvent.click(view.getByRole('button', { name: 'Example schema' }));
    await fireEvent.click(view.getByRole('button', { name: 'Validate' }));
    await waitFor(() => expect(api.validateShex).toHaveBeenCalledWith('ds1', { schema: expect.stringContaining('ex:PersonShape') }));
    const report = await view.findByTestId('shex-report');
    expect(report.textContent).toContain('1 of 2 do not conform.');
    expect(report.textContent).toContain('missing ex:name');
    await fireEvent.click(view.getByRole('button', { name: 'Not conforming' }));
    expect(view.getAllByTestId('shex-row')).toHaveLength(1);
    expect(view.getByTestId('shex-row').textContent).toContain('bob');
  });

  it('sends a ShapeMap, or node/shape pairs, and the chosen syntax', async () => {
    api.validateShex.mockResolvedValue({ conforms: true, results: [REPORT.results[0]] });
    const view = render(ShexValidator, { datasetId: null });
    await fireEvent.input(view.getByLabelText('Schema'), { target: { value: '{ "type": "Schema" }' } });
    await fireEvent.click(view.getByRole('button', { name: 'Schema syntax' }));
    await fireEvent.click(await view.findByRole('option', { name: 'ShExJ' }));
    await fireEvent.click(view.getByRole('button', { name: 'ShapeMap' }));
    await fireEvent.input(view.getByLabelText('ShapeMap'), { target: { value: '<a>@<S>' } });
    await fireEvent.click(view.getByRole('button', { name: 'Validate' }));
    await waitFor(() => expect(api.validateShex).toHaveBeenLastCalledWith(null, { schema: '{ "type": "Schema" }', schema_format: 'shexj', shape_map: '<a>@<S>' }));
    expect((await view.findByTestId('shex-report')).textContent).toContain('The node conforms.');

    await fireEvent.click(view.getByRole('button', { name: 'Node and shape' }));
    await fireEvent.input(view.getByLabelText('Node 1'), { target: { value: 'https://example.org/alice' } });
    await fireEvent.input(view.getByLabelText('Shape 1'), { target: { value: 'https://example.org/PersonShape' } });
    await fireEvent.click(view.getByRole('button', { name: 'Validate' }));
    await waitFor(() => expect(api.validateShex).toHaveBeenLastCalledWith(null, {
      schema: '{ "type": "Schema" }', schema_format: 'shexj',
      shape_map: [{ node: 'https://example.org/alice', shape: 'https://example.org/PersonShape' }],
    }));
  });

  it('shows a schema error', async () => {
    api.validateShex.mockRejectedValue(new Error('ShExC parse error at line 1: expected a shape'));
    const view = render(ShexValidator, { datasetId: 'ds1' });
    await fireEvent.input(view.getByLabelText('Schema'), { target: { value: 'nonsense' } });
    await fireEvent.click(view.getByRole('button', { name: 'Validate' }));
    expect((await view.findByRole('alert')).textContent).toContain('parse error');
  });
});
