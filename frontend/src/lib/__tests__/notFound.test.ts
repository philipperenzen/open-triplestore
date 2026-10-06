// The router's catch-all. App.svelte had no catch-all route, so a mistyped or
// stale link (the dataset page once linked the API path /shacl/shape-graphs/…)
// rendered an empty page shell. <Fallback> renders when no mounted <Route>
// matches the path; App.svelte puts the NotFound page in it.
import { describe, it, expect, beforeAll, afterEach } from 'vitest';
import { render, cleanup } from '@testing-library/svelte';
import { tick } from 'svelte';
import { init, addMessages } from 'svelte-i18n';
import en from '../i18n/en.json';
import { syncLocation } from '../locationStore';
import { matchPath } from '../router/match';
import FallbackHarness from './fixtures/FallbackHarness.svelte';
import NotFound from '../../pages/NotFound.svelte';

function at(path: string): void {
  window.history.replaceState({}, '', path);
  syncLocation();
}

beforeAll(() => {
  addMessages('en', en as unknown as Parameters<typeof addMessages>[1]);
  init({ fallbackLocale: 'en', initialLocale: 'en' });
});

afterEach(() => {
  cleanup();
  at('/');
});

describe('matchPath', () => {
  it('binds named segments and rejects extra ones', () => {
    expect(matchPath('/datasets/:id', '/datasets/abc')).toEqual({ id: 'abc' });
    expect(matchPath('/datasets/:id', '/datasets/abc/viewer')).toBeNull();
    expect(matchPath('/', '/')).toEqual({});
    expect(matchPath('/', '/x')).toBeNull();
  });
});

describe('the router fallback', () => {
  it('stays hidden while a route matches', async () => {
    at('/datasets/abc');
    const { container } = render(FallbackHarness);
    await tick();
    expect(container.textContent).toContain('dataset abc');
    expect(container.textContent).not.toContain('nothing here');
  });

  it('renders when no route matches', async () => {
    at('/shacl/shape-graphs/123');
    const { container } = render(FallbackHarness);
    await tick();
    expect(container.textContent).toContain('nothing here');
    expect(container.textContent).not.toContain('home page');
  });

  it('follows navigation in both directions', async () => {
    at('/');
    const { container } = render(FallbackHarness);
    await tick();
    expect(container.textContent).toContain('home page');
    at('/no/such/page');
    await tick();
    expect(container.textContent).toContain('nothing here');
    at('/datasets/x');
    await tick();
    expect(container.textContent).not.toContain('nothing here');
  });
});

describe('the not-found page', () => {
  it('names the missing path and offers a way back', async () => {
    at('/does-not-exist');
    const view = render(NotFound);
    await tick();
    expect(view.getByRole('heading', { name: 'Page not found' })).toBeTruthy();
    expect(view.container.textContent).toContain('/does-not-exist');
    expect(view.getByRole('link', { name: 'Go to the overview' }).getAttribute('href')).toBe('/');
    expect(view.getByRole('link', { name: 'Open the documentation' }).getAttribute('href')).toBe('/docs');
  });
});
