// /admin/docs is admin-only like every other /admin/* page. It had no guard:
// anyone who opened the URL got the editor and its document list.
import { describe, it, expect, vi, beforeAll, beforeEach } from 'vitest';
import { render, cleanup, waitFor } from '@testing-library/svelte';
import { tick } from 'svelte';
import { init, addMessages } from 'svelte-i18n';
import en from '../i18n/en.json';
import { user, authInitialized } from '../stores';
import { syncLocation } from '../locationStore';

const api = vi.hoisted(() => ({
  listDocs: vi.fn(),
  getDoc: vi.fn(),
  saveDoc: vi.fn(),
  deleteDoc: vi.fn(),
}));
vi.mock('../api.js', () => api);

import DocEditor from '../../pages/DocEditor.svelte';

beforeAll(() => {
  addMessages('en', en as unknown as Parameters<typeof addMessages>[1]);
  init({ fallbackLocale: 'en', initialLocale: 'en' });
});

beforeEach(() => {
  cleanup();
  vi.clearAllMocks();
  api.listDocs.mockResolvedValue([{ slug: 'intro', title: 'Intro', admin_only: false, source: 'db' }]);
  window.history.replaceState({}, '', '/admin/docs');
  syncLocation();
  authInitialized.set(true);
});

describe('the docs editor guard', () => {
  it('sends a non-admin home without loading or showing anything', async () => {
    user.set({ id: 'u2', username: 'reader', role: 'user' } as never);
    const view = render(DocEditor);
    await tick();
    expect(window.location.pathname).toBe('/');
    expect(api.listDocs).not.toHaveBeenCalled();
    expect(view.queryByRole('heading')).toBeNull();
  });

  it('waits for the session before deciding', async () => {
    authInitialized.set(false);
    user.set(null);
    render(DocEditor);
    await tick();
    expect(window.location.pathname).toBe('/admin/docs');
    expect(api.listDocs).not.toHaveBeenCalled();
  });

  it('loads the editor for an admin', async () => {
    user.set({ id: 'u1', username: 'admin', role: 'admin' } as never);
    const view = render(DocEditor);
    await waitFor(() => expect(api.listDocs).toHaveBeenCalled());
    expect(await view.findByText('Intro')).toBeTruthy();
    expect(window.location.pathname).toBe('/admin/docs');
  });
});
