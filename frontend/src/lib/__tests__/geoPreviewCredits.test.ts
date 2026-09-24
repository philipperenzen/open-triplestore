// GeoPreview carries the data credits its geometry is owed. 3DBAG-derived
// footprints (CC BY 4.0) need 3DBAG's fixed credit, linked to its copyright
// page, in the map's bottom-right attribution — the same credit the other
// viewers take from lib/viewer/attribution.
import { describe, it, expect, beforeAll, afterAll } from 'vitest';
import { render } from '@testing-library/svelte';
import { init, addMessages } from 'svelte-i18n';
import en from '../i18n/en.json';
import GeoPreview from '../../components/GeoPreview.svelte';
import { THREEDBAG_CREDIT } from '../viewer/attribution';

const originalWidth = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'clientWidth');
const originalHeight = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'clientHeight');

beforeAll(() => {
  addMessages('en', en as unknown as Parameters<typeof addMessages>[1]);
  init({ fallbackLocale: 'en', initialLocale: 'en' });
  // jsdom lays nothing out; give the map a size so it draws.
  Object.defineProperty(HTMLElement.prototype, 'clientWidth', { configurable: true, get: () => 400 });
  Object.defineProperty(HTMLElement.prototype, 'clientHeight', { configurable: true, get: () => 300 });
});

afterAll(() => {
  if (originalWidth) Object.defineProperty(HTMLElement.prototype, 'clientWidth', originalWidth);
  if (originalHeight) Object.defineProperty(HTMLElement.prototype, 'clientHeight', originalHeight);
});

const FOOTPRINT = 'POLYGON((5.1 52.1, 5.101 52.1, 5.101 52.101, 5.1 52.101, 5.1 52.1))';
const attribution = (root: Element) => root.querySelector('.leaflet-control-attribution');

describe('GeoPreview credits', () => {
  it('credits 3DBAG when the geometry comes from a 3DBAG source', () => {
    const { container } = render(GeoPreview, {
      wkts: [FOOTPRINT],
      sources: ['https://docs.3dbag.nl/en/copyright/'],
    });
    const box = attribution(container);
    expect(box?.textContent).toContain(THREEDBAG_CREDIT.text);
    const link = box?.querySelector('a[href="https://docs.3dbag.nl/en/copyright/"]');
    expect(link?.textContent).toBe(THREEDBAG_CREDIT.text);
    expect(box?.querySelector('a[href="https://creativecommons.org/licenses/by/4.0/"]')).toBeTruthy();
  });

  it('adds no 3DBAG credit for geometry from elsewhere', () => {
    const { container } = render(GeoPreview, {
      wkts: [FOOTPRINT],
      sources: ['https://example.org/building/1'],
    });
    expect(attribution(container)?.textContent || '').not.toContain(THREEDBAG_CREDIT.text);
  });

  it('shows explicit credits and drops them when the geometry changes hands', async () => {
    const { container, rerender } = render(GeoPreview, { wkts: [FOOTPRINT], credits: [THREEDBAG_CREDIT] });
    expect(attribution(container)?.textContent).toContain(THREEDBAG_CREDIT.text);
    await rerender({ wkts: [FOOTPRINT], credits: [] });
    expect(attribution(container)?.textContent || '').not.toContain(THREEDBAG_CREDIT.text);
  });

  it('lists a credit once, however many sources point at it', () => {
    const { container } = render(GeoPreview, {
      wkts: [FOOTPRINT],
      credits: [THREEDBAG_CREDIT],
      sources: ['https://data.3dbag.nl/def/', '/samples/schependomlaan-3dbag.city.json'],
    });
    const text = attribution(container)?.textContent || '';
    expect(text.split(THREEDBAG_CREDIT.text)).toHaveLength(2);
  });
});
