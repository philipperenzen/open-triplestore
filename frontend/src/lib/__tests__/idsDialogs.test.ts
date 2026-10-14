// IDS import and export in the shape library: a refused document shows the
// audit report problem by problem; a passing one shows its specifications
// and creates a shape graph; export says when it is a lossless re-export
// and lists what IDS could not carry.
import { describe, it, expect, vi, beforeAll, beforeEach } from 'vitest';
import { render, cleanup, fireEvent, waitFor } from '@testing-library/svelte';
import { init, addMessages } from 'svelte-i18n';
import en from '../i18n/en.json';

const api = vi.hoisted(() => ({ importSpec: vi.fn(), getShapeGraphTurtle: vi.fn(), exportSpec: vi.fn() }));
vi.mock('../api', () => api);
const saved = vi.hoisted(() => ({ downloadBlob: vi.fn() }));
vi.mock('../exchange', async (orig) => ({ ...(await orig<typeof import('../exchange')>()), downloadBlob: saved.downloadBlob }));

import IdsImportDialog from '../../components/exchange/IdsImportDialog.svelte';
import IdsExportDialog from '../../components/exchange/IdsExportDialog.svelte';

beforeAll(() => {
  addMessages('en', en as never);
  init({ fallbackLocale: 'en', initialLocale: 'en' });
});

beforeEach(() => {
  cleanup();
  vi.clearAllMocks();
});

async function pick(container: HTMLElement) {
  const input = container.querySelector('[data-testid="ids-file"]') as HTMLInputElement;
  const file = new File(['<ids:ids/>'], 'fire.ids', { type: 'application/xml' });
  await fireEvent.change(input, { target: { files: [file] } });
  await waitFor(() => expect((container.querySelector('.xc-dialog-foot button:nth-child(2)') as HTMLButtonElement).disabled).toBe(false));
}

describe('IdsImportDialog', () => {
  it('shows the audit report of a refused IDS', async () => {
    api.importSpec.mockRejectedValue(Object.assign(new Error('ids: the IDS fails…'), {
      status: 422,
      body: { error: 'ids: …', stage: 'audit', problems: ['IFCWAL is not an entity of IFC4', 'Pset_X.Y needs a dataType'] },
    }));
    const { container, getByRole, findByTestId } = render(IdsImportDialog);
    await pick(container);
    await fireEvent.click(getByRole('button', { name: /^Check$/ }));
    const report = await findByTestId('ids-audit-report');
    expect(report.textContent).toContain('Fails the IDS audit rules');
    expect(report.textContent).toContain('2 problem(s)');
    expect([...report.querySelectorAll('li')].map((l) => l.textContent)).toEqual(['IFCWAL is not an entity of IFC4', 'Pset_X.Y needs a dataType']);
    expect((getByRole('button', { name: /Create shape graph/ }) as HTMLButtonElement).disabled).toBe(true);
  });

  it('checks, then creates the shape graph under the chosen name', async () => {
    api.importSpec
      .mockResolvedValueOnce({
        title: 'Fire safety', shape_count: 3, warnings: ['a pattern was approximated'],
        specifications: [{ name: 'Walls', shape: 'urn:ids:w', target_classes: ['a', 'b'], requirements: 2 }],
      })
      .mockResolvedValueOnce({ shape_graph: { id: 'sg1', name: 'Fire' } });
    const { container, getByRole, findByTestId } = render(IdsImportDialog);
    await pick(container);
    await fireEvent.click(getByRole('button', { name: /^Check$/ }));
    await findByTestId('ids-checked');
    expect(container.textContent).toContain('1 specification(s), 3 shape(s)');
    expect(container.textContent).toContain('a pattern was approximated');
    const name = container.querySelector('.xc-grid input[type="text"]') as HTMLInputElement;
    expect(name.value).toBe('Fire safety');
    await fireEvent.input(name, { target: { value: 'Fire' } });
    await fireEvent.click(getByRole('button', { name: /Create shape graph/ }));
    await waitFor(() => expect(api.importSpec).toHaveBeenCalledTimes(2));
    expect(api.importSpec.mock.calls[1]).toEqual(['ids', '<ids:ids/>', { create: true, name: 'Fire', visibility: 'private' }]);
  });
});

describe('IdsExportDialog', () => {
  it('announces a lossless re-export and downloads the document', async () => {
    api.getShapeGraphTurtle.mockResolvedValue('@prefix sh: <http://www.w3.org/ns/shacl#> .');
    api.exportSpec.mockResolvedValue({ document: '<ids:ids/>', specification_count: 2, verbatim_specifications: 2, losses: [] });
    const { findByTestId, getByRole } = render(IdsExportDialog, { shapeGraph: { id: 'sg1', name: 'Fire safety' } });
    await findByTestId('ids-lossless');
    expect(api.exportSpec).toHaveBeenCalledWith('ids', '@prefix sh: <http://www.w3.org/ns/shacl#> .', 'Fire safety');
    await fireEvent.click(getByRole('button', { name: /Download \.ids/ }));
    expect(saved.downloadBlob).toHaveBeenCalledWith(expect.any(Blob), 'Fire_safety.ids');
  });

  it('lists what IDS could not express', async () => {
    api.getShapeGraphTurtle.mockResolvedValue('ttl');
    api.exportSpec.mockResolvedValue({
      document: '<ids:ids/>', specification_count: 2, verbatim_specifications: 1,
      losses: ['specification `Walls` was changed after it was imported'],
    });
    const { findByTestId, container, queryByTestId } = render(IdsExportDialog, { shapeGraph: { id: 'sg1', name: 'Fire' } });
    const losses = await findByTestId('ids-losses');
    expect(losses.textContent).toContain('changed after it was imported');
    expect(queryByTestId('ids-lossless')).toBeNull();
    expect(container.textContent).toContain('1 of 2 specification(s) are written back unchanged');
  });

  it('shows why nothing could be exported', async () => {
    api.getShapeGraphTurtle.mockResolvedValue('ttl');
    api.exportSpec.mockRejectedValue(new Error('ids: no shape could be expressed as an IDS specification'));
    const { findByRole } = render(IdsExportDialog, { shapeGraph: { id: 'sg1', name: 'Fire' } });
    expect((await findByRole('alert')).textContent).toContain('no shape could be expressed');
  });
});
