// The identity-policy card on the dataset and organisation pages: the UI for
// GET/PUT/DELETE /api/{datasets|organisations}/:id/identity, which shipped as
// API-only (docs/reasoning.md "Identity policy").
import { describe, it, expect, vi, beforeAll, beforeEach } from 'vitest';
import { render, cleanup, fireEvent, waitFor } from '@testing-library/svelte';
import { init, addMessages } from 'svelte-i18n';
import en from '../i18n/en.json';

const api = vi.hoisted(() => ({
  getIdentityPolicy: vi.fn(),
  setIdentityPolicy: vi.fn(),
  clearIdentityPolicy: vi.fn(),
}));
vi.mock('../api', () => api);

import IdentityPolicyCard from '../../components/IdentityPolicyCard.svelte';

const OPTIONS = ['sameas-off', 'sameas-narrow', 'sameas-full'].map((policy) => ({ policy, description: '' }));

function state(over: Record<string, unknown> = {}) {
  return {
    scope: 'dataset', id: 'ds1', policy: 'sameas-narrow', source: 'organisation', setting: null,
    description: '', never_identity: ['prov:specializationOf', 'skos:exactMatch'], options: OPTIONS,
    ...over,
  };
}

beforeAll(() => {
  // jsdom has no layout; the listbox scrolls its active option into view.
  Element.prototype.scrollIntoView = () => {};
  addMessages('en', en as unknown as Parameters<typeof addMessages>[1]);
  init({ fallbackLocale: 'en', initialLocale: 'en' });
});

beforeEach(() => {
  cleanup();
  vi.clearAllMocks();
});

/** Pick an option in the custom <Select> listbox. */
async function choose(view: any, label: string) {
  await fireEvent.click(view.getByRole('button', { name: 'Setting' }));
  await fireEvent.click(await view.findByRole('option', { name: label }));
}

describe('IdentityPolicyCard', () => {
  it('shows the policy in force and where it comes from', async () => {
    api.getIdentityPolicy.mockResolvedValue(state());
    const view = render(IdentityPolicyCard, { scope: 'dataset', id: 'ds1', canManage: false });
    expect(await view.findByText('Narrow')).toBeTruthy();
    expect(view.getByText('inherited from the organisation')).toBeTruthy();
    expect(view.getByText('prov:specializationOf')).toBeTruthy();
    expect(view.getByText('Changing it needs write access.')).toBeTruthy();
    expect(view.queryByRole('button', { name: 'Save' })).toBeNull();
    expect(api.getIdentityPolicy).toHaveBeenCalledWith('dataset', 'ds1');
  });

  it('sets a dataset policy with PUT', async () => {
    api.getIdentityPolicy.mockResolvedValue(state());
    api.setIdentityPolicy.mockResolvedValue(state({ policy: 'sameas-full', source: 'dataset', setting: 'sameas-full' }));
    const view = render(IdentityPolicyCard, { scope: 'dataset', id: 'ds1', canManage: true });
    await view.findByText('inherited from the organisation');
    const save = view.getByRole('button', { name: 'Save' });
    expect((save as HTMLButtonElement).disabled).toBe(true);
    await choose(view, 'Full');
    expect((save as HTMLButtonElement).disabled).toBe(false);
    await fireEvent.click(save);
    await waitFor(() => expect(api.setIdentityPolicy).toHaveBeenCalledWith('dataset', 'ds1', 'sameas-full'));
    expect(await view.findByText('set on this dataset')).toBeTruthy();
  });

  it('drops the own setting with DELETE when set back to inherit', async () => {
    api.getIdentityPolicy.mockResolvedValue(state({ scope: 'organisation', id: 'org1', policy: 'sameas-off', source: 'organisation', setting: 'sameas-off' }));
    api.clearIdentityPolicy.mockResolvedValue(state({ scope: 'organisation', id: 'org1', source: 'default', setting: null }));
    const view = render(IdentityPolicyCard, { scope: 'organisation', id: 'org1', canManage: true });
    expect(await view.findByText('set on this organisation')).toBeTruthy();
    await choose(view, 'Built-in default');
    await fireEvent.click(view.getByRole('button', { name: 'Save' }));
    await waitFor(() => expect(api.clearIdentityPolicy).toHaveBeenCalledWith('organisation', 'org1'));
    expect(api.setIdentityPolicy).not.toHaveBeenCalled();
  });

  it('hides itself when the server will not show the setting', async () => {
    api.getIdentityPolicy.mockRejectedValue(Object.assign(new Error('Organisation not found'), { status: 404 }));
    const view = render(IdentityPolicyCard, { scope: 'organisation', id: 'org1', canManage: false });
    await waitFor(() => expect(api.getIdentityPolicy).toHaveBeenCalled());
    await waitFor(() => expect(view.queryByText('Identity policy (owl:sameAs)')).toBeNull());
  });
});
