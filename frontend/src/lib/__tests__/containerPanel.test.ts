// ICDD containers on the dataset page: the OTS_ICDD_ONTOLOGY_DIR warning,
// import with the strict toggle and its report, a strict refusal's report,
// validate-only, and the export's own validation read from its headers.
import { describe, it, expect, vi, beforeAll, beforeEach } from 'vitest';
import { render, cleanup, fireEvent, waitFor } from '@testing-library/svelte';
import { init, addMessages } from 'svelte-i18n';
import en from '../i18n/en.json';

const api = vi.hoisted(() => ({
  listContainerProfiles: vi.fn(),
  importContainer: vi.fn(),
  validateContainer: vi.fn(),
  exportContainer: vi.fn(),
}));
vi.mock('../api', () => api);
const saved = vi.hoisted(() => ({ downloadBlob: vi.fn() }));
vi.mock('../exchange', async (orig) => ({ ...(await orig<typeof import('../exchange')>()), downloadBlob: saved.downloadBlob }));

import ContainerPanel from '../../components/exchange/ContainerPanel.svelte';

const UNSET = { profiles: [{ id: 'icdd', label: 'ICDD', ontology_resources: { env: 'OTS_ICDD_ONTOLOGY_DIR', configured: false, complete: false, missing: ['Container.rdf', 'Linkset.rdf'] } }] };
const COMPLETE = { profiles: [{ id: 'icdd', label: 'ICDD', ontology_resources: { env: 'OTS_ICDD_ONTOLOGY_DIR', configured: true, complete: true, missing: [] } }] };
const REPORT = {
  profile: 'icdd', conforms: false, violations: 1, warnings: 1,
  findings: [
    { severity: 'warning', code: 'ontology-resource-missing', message: 'Container.rdf is missing' },
    { severity: 'violation', code: 'document-missing', message: 'a.pdf is listed but absent', file: 'Payload documents/a.pdf' },
  ],
};

beforeAll(() => {
  addMessages('en', en as never);
  init({ fallbackLocale: 'en', initialLocale: 'en' });
});

beforeEach(() => {
  cleanup();
  vi.clearAllMocks();
});

async function pick(container: HTMLElement) {
  const input = container.querySelector('[data-testid="container-file"]') as HTMLInputElement;
  const file = new File(['PK'], 'site.icdd', { type: 'application/zip' });
  await fireEvent.change(input, { target: { files: [file] } });
  return file;
}

describe('ContainerPanel', () => {
  it('warns that exports are not Part 1-conformant without the ontology directory', async () => {
    api.listContainerProfiles.mockResolvedValue(UNSET);
    const { findByTestId } = render(ContainerPanel, { datasetId: 'd1', canWrite: true, authenticated: true });
    const warn = await findByTestId('ontology-warning');
    expect(warn.textContent).toContain('OTS_ICDD_ONTOLOGY_DIR is not set');
  });

  it('says nothing when the operator configured both files', async () => {
    api.listContainerProfiles.mockResolvedValue(COMPLETE);
    const { queryByTestId } = render(ContainerPanel, { datasetId: 'd1', canWrite: true, authenticated: true });
    await waitFor(() => expect(api.listContainerProfiles).toHaveBeenCalled());
    expect(queryByTestId('ontology-warning')).toBeNull();
  });

  it('imports strictly and shows the report of a refused container', async () => {
    api.listContainerProfiles.mockResolvedValue(COMPLETE);
    const refusal = Object.assign(new Error('ICDD: the container has 1 violation(s); nothing was imported (strict=true)'), {
      status: 422, body: { error: 'x', validation: REPORT },
    });
    api.importContainer.mockRejectedValue(refusal);
    const { container, getByRole, findByTestId, findByText } = render(ContainerPanel, { datasetId: 'd1', canWrite: true, authenticated: true });
    const file = await pick(container);
    await fireEvent.click(getByRole('checkbox'));
    await fireEvent.click(getByRole('button', { name: /^Import$/ }));
    await waitFor(() => expect(api.importContainer).toHaveBeenCalled());
    expect(api.importContainer.mock.calls[0].slice(0, 3)).toEqual(['d1', file, { strict: true }]);
    const report = await findByTestId('container-report');
    await findByText(en.components.exchange.container.reportRefused);
    // Violations first.
    const codes = [...report.querySelectorAll('tbody td:nth-child(2) code')].map((c) => c.textContent);
    expect(codes).toEqual(['document-missing', 'ontology-resource-missing']);
    expect(report.textContent).toContain('Payload documents/a.pdf');
  });

  it('imports and reports documents, graphs and the validation', async () => {
    api.listContainerProfiles.mockResolvedValue(COMPLETE);
    api.importContainer.mockResolvedValue({
      container: 'urn:c', title: 'Site', documents: [{}, {}], graphs: [{}], warnings: ['kept opaque'],
      validation: { ...REPORT, conforms: true, violations: 0, findings: [REPORT.findings[0]] },
    });
    const { container, getByRole, findByText } = render(ContainerPanel, { datasetId: 'd1', canWrite: true, authenticated: true });
    await pick(container);
    await fireEvent.click(getByRole('button', { name: /^Import$/ }));
    await findByText(/Imported “Site”: 2 document\(s\) and 1 graph\(s\)/);
    expect(container.textContent).toContain('kept opaque');
    expect(container.textContent).toContain('Conforms');
  });

  it('validates without importing', async () => {
    api.listContainerProfiles.mockResolvedValue(COMPLETE);
    api.validateContainer.mockResolvedValue(REPORT);
    const { container, getByRole, findByText } = render(ContainerPanel, { datasetId: 'd1', canWrite: false, authenticated: true });
    await pick(container);
    expect(container.textContent).not.toContain('Strict');
    await fireEvent.click(getByRole('button', { name: /Validate only/ }));
    await findByText(en.components.exchange.container.reportValidate);
    expect(api.importContainer).not.toHaveBeenCalled();
    expect(container.textContent).toContain('Does not conform');
  });

  it('exports, saves the file and surfaces the conformance headers', async () => {
    api.listContainerProfiles.mockResolvedValue(UNSET);
    const blob = new Blob(['zip']);
    api.exportContainer.mockResolvedValue({
      blob, filename: 'd1.icdd',
      headers: { conforms: 'false', validation: 'violations=0; warnings=1', findings: 'ontology-resource-missing' },
    });
    const { getByTestId, findByTestId } = render(ContainerPanel, { datasetId: 'd1', canWrite: false, authenticated: false });
    expect(api.listContainerProfiles).not.toHaveBeenCalled();
    await fireEvent.click(getByTestId('container-export'));
    const result = await findByTestId('container-export-result');
    expect(saved.downloadBlob).toHaveBeenCalledWith(blob, 'd1.icdd');
    expect(result.textContent).toContain('d1.icdd');
    expect(result.textContent).toContain('does not fully conform');
    expect(result.textContent).toContain('0 violation(s), 1 warning(s)');
    expect(result.textContent).toContain('ontology-resource-missing');
    expect(result.textContent).toContain('OTS_ICDD_ONTOLOGY_DIR');
  });
});
