// Element inspector modal: a real render that locks in (1) the i18n labels —
// the bug that prompted this work was the `viewer.*` namespace going missing,
// so the modal showed raw keys like "viewer.properties" — and (2) the restored
// layout: the "Part of" breadcrumb leads the Structure tab, and the Structure /
// 3D tabs only appear when there's something to show. The 3D tab body is never
// opened here (it mounts a WebGL canvas jsdom can't provide).
import { describe, it, expect, beforeAll, vi } from 'vitest';
import { render, fireEvent } from '@testing-library/svelte';
import { init, addMessages } from 'svelte-i18n';
import en from '../../../lib/i18n/en.json';
import ElementModal from '../ElementModal.svelte';

// browseResource backs the Properties tab — stub it so no network is hit.
vi.mock('../../../lib/api.js', async (orig) => ({
  ...(await orig()),
  browseResource: vi.fn(async () => ({
    outgoing: [
      { p: { value: 'http://www.w3.org/2000/01/rdf-schema#label' }, o: { type: 'literal', value: 'Clock tower' } },
    ],
  })),
}));

beforeAll(() => {
  addMessages('en', en as Record<string, unknown>);
  init({ fallbackLocale: 'en', initialLocale: 'en' });
});

const tower = {
  id: 'urn:demo:tower',
  label: 'Clock tower',
  types: ['https://example.org/ns#Tower'],
  parent: 'urn:demo:building',
  gltf_url: 'https://files.example/tower.glb',
  wkt4326: 'POINT(5.86 51.85)',
};
const building = { id: 'urn:demo:building', label: 'Main building' };
const bell = { id: 'urn:demo:bell', label: 'Bell', parent: 'urn:demo:tower' };

describe('ElementModal layout', () => {
  it('renders translated labels (no raw viewer.* keys leak through)', () => {
    const { getByText, container } = render(ElementModal, {
      element: tower,
      elements: [tower, building, bell],
      datasetId: 'ds1',
      hasMap: true,
    });

    getByText('Clock tower'); // header title
    getByText('Properties'); // tab — not "viewer.properties"
    getByText('Structure');
    getByText('3D model');
    getByText('Show on map'); // located → the restored map action
    getByText('ns:Tower'); // type chip (shortened IRI)

    expect(container.textContent).not.toMatch(/viewer\.[a-z]/i);
    expect(container.textContent).not.toContain('datasetViewer.');
  });

  it('leads the Structure tab with the "Part of" breadcrumb, then sub-elements', async () => {
    const { getByText } = render(ElementModal, {
      element: tower,
      elements: [tower, building, bell],
      datasetId: 'ds1',
    });

    await fireEvent.click(getByText('Structure'));

    getByText('Part of'); // breadcrumb label
    getByText('Main building'); // the parent it points to
    getByText('Bell'); // contained sub-element
  });

  it('hides the Structure and 3D tabs for a standalone, location-less element', () => {
    const lonely = { id: 'urn:demo:lonely', label: 'Lonely concept', types: [] };
    const { getByText, queryByText } = render(ElementModal, {
      element: lonely,
      elements: [lonely],
      datasetId: 'ds1',
      hasMap: true,
    });

    getByText('Properties');
    expect(queryByText('Structure')).toBeNull();
    expect(queryByText('3D model')).toBeNull();
    expect(queryByText('Show on map')).toBeNull(); // no geometry in the chain
  });
});
