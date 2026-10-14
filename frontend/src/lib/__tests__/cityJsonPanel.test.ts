// CityJSON ingest: preview converts without writing and shows a sample;
// import reports what it loaded and where; progress is shown while the
// upload runs; a refused file says why.
import { describe, it, expect, vi, beforeAll, beforeEach } from 'vitest';
import { render, cleanup, fireEvent, waitFor } from '@testing-library/svelte';
import { init, addMessages } from 'svelte-i18n';
import en from '../i18n/en.json';

const api = vi.hoisted(() => ({ ingestCityJson: vi.fn() }));
vi.mock('../api', () => api);

import CityJsonPanel from '../../components/exchange/CityJsonPanel.svelte';

const STATS = { objects: 3, geometries: 5, triples: 1200, crs: 'https://www.opengis.net/def/crs/EPSG/0/7415' };

beforeAll(() => {
  addMessages('en', en as never);
  init({ fallbackLocale: 'en', initialLocale: 'en' });
});

beforeEach(() => {
  cleanup();
  vi.clearAllMocks();
});

async function pick(container: HTMLElement) {
  const input = container.querySelector('[data-testid="cityjson-file"]') as HTMLInputElement;
  const file = new File(['{"type":"CityJSON"}'], 'block.city.json', { type: 'application/json' });
  await fireEvent.change(input, { target: { files: [file] } });
  return file;
}

describe('CityJsonPanel', () => {
  it('previews without writing and shows the first triples', async () => {
    api.ingestCityJson.mockResolvedValue({ graph: null, asset_id: null, asset_url: null, stats: STATS, sample: '<a> <b> <c> .', preview: true });
    const { container, getByRole, findByTestId } = render(CityJsonPanel, { datasetId: 'd1' });
    const file = await pick(container);
    await fireEvent.click(getByRole('button', { name: /Preview/ }));
    const result = await findByTestId('cityjson-result');
    expect(api.ingestCityJson.mock.calls[0].slice(0, 3)).toEqual(['d1', file, { targetGraph: undefined, publicAsset: false, preview: true }]);
    expect(result.textContent).toContain('nothing was written');
    expect(result.textContent).toContain('1,200');
    expect(result.textContent).toContain('EPSG/0/7415');
    expect(result.querySelector('pre')?.textContent).toBe('<a> <b> <c> .');
  });

  it('imports into the chosen graph, shows progress, then the result', async () => {
    let report: (f: number) => void = () => {};
    let finish: (v: unknown) => void = () => {};
    api.ingestCityJson.mockImplementation((_id, _f, _o, onProgress) => {
      report = onProgress;
      return new Promise((r) => { finish = r; });
    });
    const { container, getByRole, findByTestId } = render(CityJsonPanel, { datasetId: 'd1' });
    await pick(container);
    await fireEvent.input(container.querySelector('input[type="text"]')!, { target: { value: 'https://x.test/dataset/d1/city' } });
    await fireEvent.click(getByRole('button', { name: /^Import$/ }));
    report(0.4);
    await waitFor(() => expect(container.textContent).toContain('Uploading… 40%'));
    report(1);
    await waitFor(() => expect(container.textContent).toContain('Converting and loading'));
    finish({ graph: 'https://x.test/dataset/d1/city', asset_id: 'a1', asset_url: null, stats: STATS, preview: false });
    const result = await findByTestId('cityjson-result');
    expect(api.ingestCityJson.mock.calls[0][2]).toEqual({ targetGraph: 'https://x.test/dataset/d1/city', publicAsset: false, preview: false });
    expect(result.textContent).toContain('https://x.test/dataset/d1/city');
    expect(result.querySelector('pre')).toBeNull();
  });

  it('shows why a file was refused', async () => {
    api.ingestCityJson.mockRejectedValue(new Error('invalid JSON: expected value at line 1'));
    const { container, getByRole, findByRole } = render(CityJsonPanel, { datasetId: 'd1' });
    await pick(container);
    await fireEvent.click(getByRole('button', { name: /^Import$/ }));
    expect((await findByRole('alert')).textContent).toContain('invalid JSON');
  });
});
