// The "Model and data" card on the resource page: what a resource belongs to,
// each line a link (GET /api/browse/resource/context).
import { describe, it, expect, vi, beforeAll, beforeEach } from 'vitest';
import { render, cleanup, fireEvent } from '@testing-library/svelte';
import { init, addMessages } from 'svelte-i18n';
import en from '../i18n/en.json';
import ResourceContext from '../../components/ResourceContext.svelte';

const EX = 'http://example.org/def#';
const bridges = { model_id: 'bridges', title: 'Bridges', kind: 'data-model', version: '2.0', status: 'published', other_versions: 1 };
const inspections = {
  id: 'inspections',
  name: 'Inspections',
  conforms_to: { model_id: 'bridges', version: '2.0', title: 'Bridges' },
};

beforeAll(() => {
  addMessages('en', en as unknown as Parameters<typeof addMessages>[1]);
  init({ fallbackLocale: 'en', initialLocale: 'en' });
});
beforeEach(() => cleanup());

const hrefs = (view: { container: HTMLElement }) =>
  [...view.container.querySelectorAll('a')].map((a) => a.getAttribute('href'));

describe('ResourceContext', () => {
  it('renders nothing when there is nothing to say', () => {
    for (const context of [null, {}, { defined_in: [], instance_of: [], datasets: [], instances: { total: 0, sources: [] } }]) {
      const view = render(ResourceContext, { props: { context, iri: `${EX}x` } });
      expect(view.container.querySelector('section')).toBeNull();
      cleanup();
    }
  });

  it('an instance: its class with the model that defines it, its dataset and what that conforms to', async () => {
    const onOpenResource = vi.fn();
    const view = render(ResourceContext, {
      props: {
        iri: 'http://example.org/id/b1',
        onOpenResource,
        context: {
          defined_in: [],
          instance_of: [{ iri: `${EX}Bridge`, defined_in: [bridges] }],
          datasets: [inspections],
          conforming_datasets: [],
          instances: { total: 0, sources: [] },
        },
      },
    });
    expect(view.getByText('Instance of')).toBeTruthy();
    expect(view.getByText('In dataset')).toBeTruthy();
    expect(view.queryByText('Defined in')).toBeNull();
    expect(view.queryByText('Instances')).toBeNull();

    // The class's model opens on the class; the dataset's model on the version.
    expect(hrefs(view)).toEqual([
      `/models/bridges/viewer/2.0?term=${encodeURIComponent(`${EX}Bridge`)}`,
      '/datasets/inspections',
      '/models/bridges/viewer/2.0',
    ]);
    expect(view.getByText('also in 1 other version')).toBeTruthy();

    await fireEvent.click(view.getByRole('button', { name: /Bridge/ }));
    expect(onOpenResource).toHaveBeenCalledWith(`${EX}Bridge`);
  });

  it('a class: the model it is defined in, where its instances are, and the datasets conforming to the model', async () => {
    const onShowInstances = vi.fn();
    const view = render(ResourceContext, {
      props: {
        iri: `${EX}Bridge`,
        onShowInstances,
        context: {
          defined_in: [bridges],
          instance_of: [],
          datasets: [],
          conforming_datasets: [{ id: 'inspections', name: 'Inspections', version: '2.0' }],
          instances: { total: 12, elsewhere: 2, sources: [{ dataset: inspections, count: 10 }] },
        },
      },
    });
    expect(view.getByText('Defined in')).toBeTruthy();
    expect(view.getByText('Datasets conforming to this model')).toBeTruthy();
    expect(view.getByText('10')).toBeTruthy();
    expect(view.getByText('in graphs outside any dataset or model')).toBeTruthy();
    expect(hrefs(view)[0]).toBe(`/models/bridges/viewer/2.0?term=${encodeURIComponent(`${EX}Bridge`)}`);
    expect(hrefs(view)).toContain('/datasets/inspections');

    await fireEvent.click(view.getByRole('button', { name: 'Show what is typed with this' }));
    expect(onShowInstances).toHaveBeenCalledOnce();
  });

  it('says so when a class is in no readable model, or a dataset declares none', () => {
    const view = render(ResourceContext, {
      props: {
        iri: 'http://example.org/id/b1',
        context: {
          instance_of: [{ iri: `${EX}Lock`, defined_in: [] }],
          datasets: [{ id: 'loose', name: 'Loose data', conforms_to: null }],
        },
      },
    });
    expect(view.getByText('not defined in a model you can read')).toBeTruthy();
    expect(view.getByText('declares no model it conforms to')).toBeTruthy();
    // No instances button without a handler, and none without instances.
    expect(view.queryByRole('button', { name: 'Show what is typed with this' })).toBeNull();
  });

  it('marks a version that is not published', () => {
    const view = render(ResourceContext, {
      props: { iri: `${EX}Bridge`, context: { defined_in: [{ ...bridges, status: 'draft', other_versions: 0 }] } },
    });
    expect(view.getByText('draft')).toBeTruthy();
    expect(view.queryByText(/other version/)).toBeNull();
  });
});
