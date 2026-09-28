// The Studio's sub-navigation. Every tab must land on a Studio surface: the
// Datasets tab used to leave for the global dataset catalogue at /datasets,
// which is not part of the workspace and has no way back into it. Inside the
// Studio, "datasets" means the datasets you are validating — the same list the
// Overview's "Datasets to validate" card links to.
import { describe, it, expect, beforeAll, afterEach } from 'vitest';
import { render, cleanup } from '@testing-library/svelte';
import { init, addMessages } from 'svelte-i18n';
import en from '../i18n/en.json';
import ShaclStudioNav from '../../components/ShaclStudioNav.svelte';
import { syncLocation } from '../locationStore';

/** The router's store reads `window.location`; `history` alone does not push it. */
function at(path: string): void {
  window.history.replaceState({}, '', path);
  syncLocation();
}

beforeAll(() => {
  addMessages('en', en as unknown as Parameters<typeof addMessages>[1]);
  init({ fallbackLocale: 'en', initialLocale: 'en' });
});

afterEach(cleanup);

function tabs(container: HTMLElement) {
  return Array.from(container.querySelectorAll('a')).map((a) => ({
    label: (a.textContent ?? '').trim(),
    href: a.getAttribute('href') ?? '',
    active: a.getAttribute('data-active') === 'true',
  }));
}

describe('the SHACL Studio navigation', () => {
  it('sends Datasets to the validation overview, not the global catalogue', () => {
    const { container } = render(ShaclStudioNav);
    const datasets = tabs(container).find((t) => t.label === 'Datasets');
    expect(datasets, 'the Datasets tab is missing').toBeTruthy();
    expect(datasets!.href).toBe('/validation');
  });

  it('runs in the order the work runs', () => {
    const { container } = render(ShaclStudioNav);
    expect(tabs(container).map((t) => t.label)).toEqual([
      'Overview',
      'Shapes',
      'Datasets',
      'Pipelines',
      'Results',
    ]);
  });

  it('keeps every tab inside the Studio', () => {
    const { container } = render(ShaclStudioNav);
    const hrefs = tabs(container).map((t) => t.href);
    expect(hrefs.length).toBeGreaterThanOrEqual(5);
    for (const href of hrefs) {
      expect(
        href === '/shacl' || href.startsWith('/shacl/') || href === '/validation',
        `${href} leaves the Studio`,
      ).toBe(true);
    }
  });

  it('gives each path exactly one active tab', () => {
    // Two tabs claiming the same path is how /validation ended up highlighting
    // Results while the Datasets tab that leads there stayed dim.
    for (const path of ['/shacl', '/shacl/shapes', '/shacl/pipelines', '/shacl/results', '/validation']) {
      at(path);
      const { container } = render(ShaclStudioNav);
      const active = tabs(container).filter((t) => t.active);
      expect(active.map((t) => t.label), `${path} highlights ${active.length} tabs`).toHaveLength(1);
      cleanup();
    }
  });

  it('highlights Datasets on the validation overview', () => {
    at('/validation');
    const { container } = render(ShaclStudioNav);
    const active = tabs(container).find((t) => t.active);
    expect(active?.label).toBe('Datasets');
  });
});
