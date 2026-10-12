// The Home page's standards chips come from the server (`capabilities` in
// GET /health, built from the compiled feature set) instead of a hard-coded
// list that named engines a build might not contain.
import { describe, it, expect, vi, beforeAll, beforeEach } from 'vitest';
import { render, cleanup } from '@testing-library/svelte';
import { tick } from 'svelte';
import { init, addMessages } from 'svelte-i18n';
import en from '../i18n/en.json';
import { backendHealth } from '../stores';

vi.mock('../api.js', () => ({
  browseStats: vi.fn().mockResolvedValue(null),
  listDatasets: vi.fn().mockResolvedValue([]),
  getHealth: vi.fn(),
  tryRefreshToken: vi.fn(),
}));
vi.mock('../../components/LinkedDataBackground.svelte', async () => import('./fixtures/NullComponent.svelte'));

import Home from '../../pages/Home.svelte';

beforeAll(() => {
  addMessages('en', en as unknown as Parameters<typeof addMessages>[1]);
  init({ fallbackLocale: 'en', initialLocale: 'en' });
});

beforeEach(cleanup);

describe('Home capability chips', () => {
  it('lists what the server reports', async () => {
    backendHealth.set({ status: 'ok', capabilities: ['SPARQL 1.1', 'SHACL', 'SWRL'], services: null });
    const view = render(Home);
    await tick();
    const list = view.getByRole('list', { name: 'Standards this server supports' });
    expect(Array.from(list.querySelectorAll('li')).map((li) => li.textContent)).toEqual(['SPARQL 1.1', 'SHACL', 'SWRL']);
  });

  it('shows no chips until the server has said', async () => {
    backendHealth.set(null);
    const view = render(Home);
    await tick();
    expect(view.queryByRole('list', { name: 'Standards this server supports' })).toBeNull();
  });
});
