/**
 * The resource page's "copy this IRI" control.
 *
 * The IRI is what people come to this page to take away, so three things have
 * to hold: the control is visible and nameable (not a bare 16px glyph at the
 * end of a line), it copies the *whole* IRI, and a copy that fails says so
 * instead of looking like a dead button. The last one is not hypothetical —
 * the async Clipboard API rejects when the document is not focused and does
 * not exist at all over plain HTTP on a LAN, which is a normal deployment.
 *
 * The page fetches on mount, so the API and the clipboard are mocked the way
 * adminOperations.test.ts mocks them, and the router location is set directly.
 */
import { describe, it, expect, vi, beforeAll, beforeEach, afterEach } from 'vitest';
import { render, cleanup, fireEvent } from '@testing-library/svelte';
import { init, addMessages } from 'svelte-i18n';
import en from '../i18n/en.json';
import { location } from '../locationStore';

const api = vi.hoisted(() => ({
  browseResource: vi.fn(),
}));
vi.mock('../api.js', () => api);

const clipboard = vi.hoisted(() => ({
  copyOrWarn: vi.fn(),
  copyToClipboard: vi.fn(),
}));
vi.mock('../clipboard.js', () => clipboard);

import ResourceDetail from '../../pages/ResourceDetail.svelte';

// Long enough that a one-line ellipsis would hide most of it — which is the
// whole point: a user who selects the text by hand must still get all of it.
const IRI =
  'https://example.org/very/long/namespace/path/that/keeps/going/collections/buildings/0f2c8b1a-4d3e-4a7b-9c6d-1e2f3a4b5c6d#geometry';

const RESOURCE = {
  outgoing: [
    {
      p: { type: 'uri', value: 'http://www.w3.org/2000/01/rdf-schema#label' },
      o: { type: 'literal', value: 'A building' },
    },
  ],
  incoming: [],
  bnodes: {},
};

beforeAll(() => {
  addMessages('en', en as unknown as Parameters<typeof addMessages>[1]);
  init({ fallbackLocale: 'en', initialLocale: 'en' });
});

beforeEach(() => {
  vi.clearAllMocks();
  api.browseResource.mockResolvedValue(RESOURCE);
  clipboard.copyOrWarn.mockResolvedValue(true);
  clipboard.copyToClipboard.mockResolvedValue(true);
});

afterEach(() => {
  cleanup();
});

function mount(iri: string) {
  const search = `?iri=${encodeURIComponent(iri)}`;
  location.set({ pathname: '/resource', search, hash: '', href: `/resource${search}` });
  return render(ResourceDetail);
}

describe('resource page IRI copy control', () => {
  it('offers a named, keyboard-reachable button carrying a visible label', async () => {
    const { findByRole } = mount(IRI);
    const button = await findByRole('button', { name: /copy iri/i });

    // A native button is in the tab order and fires on Enter/Space for free.
    expect(button.tagName).toBe('BUTTON');
    // The name must be readable on screen, not only in a tooltip: a tooltip
    // needs a hover to exist, and does not exist at all on a touch device.
    expect(button.textContent).toMatch(/Copy IRI/i);
  });

  it('shows the whole IRI, not a clipped middle', async () => {
    const { container, findByRole } = mount(IRI);
    await findByRole('button', { name: /copy iri/i });

    const shown = [...container.querySelectorAll('*')].find(
      (el) => el.children.length === 0 && el.textContent?.trim() === IRI,
    );
    expect(shown, 'the full IRI should appear as its own element').toBeTruthy();
    // jsdom applies no layout, so the single-line ellipsis clamp is only
    // visible as the class that carries it. Selecting such a line by hand
    // yields the whole string in some browsers and the visible part in
    // others — neither is something to hand a user as "the IRI".
    expect(shown?.className).not.toMatch(/\btruncate\b/);
  });

  it('copies the full IRI and confirms it', async () => {
    const { findByRole, container } = mount(IRI);
    const button = await findByRole('button', { name: /copy iri/i });

    await fireEvent.click(button);
    expect(clipboard.copyOrWarn).toHaveBeenCalledWith(IRI);
    await new Promise((r) => setTimeout(r, 0));
    expect(container.textContent).toContain('Copied!');
  });

  it('tells the user what to do when the copy fails', async () => {
    clipboard.copyOrWarn.mockResolvedValue(false);
    const { findByRole, container } = mount(IRI);
    const button = await findByRole('button', { name: /copy iri/i });

    await fireEvent.click(button);
    // copyOrWarn is the half that raises the toast; routing through it is what
    // turns a silent failure into a sentence. And nothing may claim success.
    expect(clipboard.copyOrWarn).toHaveBeenCalledWith(IRI);
    expect(clipboard.copyToClipboard).not.toHaveBeenCalled();
    await new Promise((r) => setTimeout(r, 0));
    expect(container.textContent).not.toContain('Copied!');
  });

  it('offers the same control on a file resource, which uses its own header', async () => {
    const fileIri = '/samples/schependomlaan.png';
    const { findByRole } = mount(fileIri);
    const button = await findByRole('button', { name: /copy iri/i });

    expect(button.textContent).toMatch(/Copy IRI/i);
    await fireEvent.click(button);
    expect(clipboard.copyOrWarn).toHaveBeenCalledWith(fileIri);
    // A file path is rendered, never resolved as RDF.
    expect(api.browseResource).not.toHaveBeenCalled();
  });
});
