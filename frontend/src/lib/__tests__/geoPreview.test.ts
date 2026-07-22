// End-to-end cover for the two reported preview-map defects: markers that
// rendered as broken images, and a map that stayed blank when its container was
// sized after mount (the overlay's maximise/restore, a panel still laying out).
// jsdom reports every element as 0×0, which is a faithful model of "not laid out
// yet", so the layout has to be faked deliberately here — and that is exactly
// what lets the deferred-draw path be exercised.
import { describe, it, expect, beforeAll, afterAll, beforeEach } from 'vitest';
import { render } from '@testing-library/svelte';
import { init, addMessages } from 'svelte-i18n';
import en from '../i18n/en.json';
import GeoPreview from '../../components/GeoPreview.svelte';

// Mutable so a test can put the container "off-layout" and then give it a size.
let fakeWidth = 400;
let fakeHeight = 300;

const observers: FakeResizeObserver[] = [];
class FakeResizeObserver {
  cb: () => void;
  constructor(cb: () => void) {
    this.cb = cb;
    observers.push(this);
  }
  observe() {}
  unobserve() {}
  disconnect() {}
  /** Stand-in for the browser delivering a size change. */
  fire() {
    this.cb();
  }
}

const originalWidth = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'clientWidth');
const originalHeight = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'clientHeight');
const originalRO = globalThis.ResizeObserver;

beforeAll(() => {
  addMessages('en', en as Record<string, unknown>);
  init({ fallbackLocale: 'en', initialLocale: 'en' });
  Object.defineProperty(HTMLElement.prototype, 'clientWidth', {
    configurable: true,
    get: () => fakeWidth,
  });
  Object.defineProperty(HTMLElement.prototype, 'clientHeight', {
    configurable: true,
    get: () => fakeHeight,
  });
  globalThis.ResizeObserver = FakeResizeObserver as unknown as typeof ResizeObserver;
});

afterAll(() => {
  if (originalWidth) Object.defineProperty(HTMLElement.prototype, 'clientWidth', originalWidth);
  if (originalHeight) Object.defineProperty(HTMLElement.prototype, 'clientHeight', originalHeight);
  globalThis.ResizeObserver = originalRO;
});

beforeEach(() => {
  fakeWidth = 400;
  fakeHeight = 300;
  observers.length = 0;
});

const POINT = 'POINT(5.86 51.85)';

describe('GeoPreview markers', () => {
  it('gives the default marker a rooted image URL', () => {
    // The bug: Leaflet's icon-path guessing collapsed to '' in a bundled app, so
    // the img asked for a document-relative "marker-icon.png" that the SPA
    // fallback answered with index.html — a broken-image placeholder.
    const { container } = render(GeoPreview, { wkts: [POINT], height: '200px' });
    const icon = container.querySelector('img.leaflet-marker-icon');
    expect(icon).toBeTruthy();
    const src = icon!.getAttribute('src') || '';
    expect(src).not.toBe('marker-icon.png');
    expect(src).toMatch(/^(data:|blob:|https?:|\/)/);
  });

  it('draws a marker per point geometry', () => {
    const { container } = render(GeoPreview, {
      wkts: ['POINT(5.86 51.85)', 'POINT(4.9 52.37)'],
      height: '200px',
    });
    expect(container.querySelectorAll('img.leaflet-marker-icon')).toHaveLength(2);
  });
});

describe('GeoPreview deferred draw', () => {
  it('waits instead of drawing into a container with no layout', () => {
    fakeWidth = 0;
    fakeHeight = 0;
    const { container } = render(GeoPreview, { wkts: [POINT], height: '200px' });
    // The map itself is created (so it can be observed), but nothing is plotted:
    // fitBounds/setView against a 0×0 container would park it at a NaN zoom.
    expect(container.querySelector('.leaflet-container')).toBeTruthy();
    expect(container.querySelector('img.leaflet-marker-icon')).toBeFalsy();
  });

  it('draws as soon as the container is given a size', () => {
    fakeWidth = 0;
    fakeHeight = 0;
    const { container } = render(GeoPreview, { wkts: [POINT], height: '200px' });
    expect(container.querySelector('img.leaflet-marker-icon')).toBeFalsy();

    // What the overlay's maximise button (or a panel finishing layout) does.
    fakeWidth = 640;
    fakeHeight = 480;
    expect(observers.length).toBeGreaterThan(0);
    observers.forEach((o) => o.fire());

    expect(container.querySelector('img.leaflet-marker-icon')).toBeTruthy();
  });

  it('observes the map container so later resizes are noticed at all', () => {
    render(GeoPreview, { wkts: [POINT], height: '200px' });
    // Leaflet 1.x only re-measures on an explicit invalidateSize() or a *window*
    // resize; without an observer the overlay's maximise leaves the map painting
    // into its old rectangle.
    expect(observers.length).toBe(1);
  });
});
