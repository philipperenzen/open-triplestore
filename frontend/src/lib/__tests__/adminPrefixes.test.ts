/**
 * The prefix-override admin page and the checks that run before it calls the
 * server: what a malformed label or namespace is told, and what happens when
 * the label is already taken — the 409 answer carries the namespace the label
 * resolves to today, and that is the fact the user decides from.
 */
import { describe, it, expect, vi, beforeAll, beforeEach } from 'vitest';
import { render, cleanup, fireEvent } from '@testing-library/svelte';
import { init, addMessages } from 'svelte-i18n';
import en from '../i18n/en.json';
import { user, authInitialized } from '../stores';
import { validatePrefixLabel, validatePrefixNamespace } from '../validate';

const api = vi.hoisted(() => ({
  adminListPrefixOverrides: vi.fn(),
  adminCreatePrefixOverride: vi.fn(),
  adminPutPrefixOverride: vi.fn(),
  adminDeletePrefixOverride: vi.fn(),
  lookupPrefixLabel: vi.fn(),
}));
vi.mock('../api.js', () => api);

import AdminPrefixes from '../../pages/AdminPrefixes.svelte';

const GEO = {
  label: 'geo',
  namespace: 'https://example.org/geo#',
  created_by: 'u1',
  created_at: '2026-09-18T10:00:00Z',
  updated_at: '2026-09-18T10:00:00Z',
};

/** The server's 409 body, thrown the way api.ts throws it. */
function conflictError(label: string, namespace: string) {
  return Object.assign(new Error(`the prefix "${label}" already resolves to ${namespace} here`), {
    status: 409,
    body: { error: 'already set', label, namespace },
  });
}

beforeAll(() => {
  addMessages('en', en as unknown as Parameters<typeof addMessages>[1]);
  init({ fallbackLocale: 'en', initialLocale: 'en' });
});

beforeEach(() => {
  cleanup();
  vi.clearAllMocks();
  api.adminListPrefixOverrides.mockResolvedValue([]);
  api.lookupPrefixLabel.mockRejectedValue(Object.assign(new Error('Unknown prefix'), { status: 404 }));
  user.set({ id: 'u1', username: 'admin', role: 'super_admin' } as never);
  authInitialized.set(true);
});

async function openAddForm() {
  const view = render(AdminPrefixes);
  await view.findByText('Add an override');
  await fireEvent.click(view.getByText('Add an override'));
  return view;
}

function field(view: { container: HTMLElement }, id: string): HTMLInputElement {
  return view.container.querySelector(`#${id}`) as HTMLInputElement;
}

describe('prefix label validation', () => {
  it('accepts a label the server would accept', () => {
    expect(validatePrefixLabel('geo')).toBeNull();
    expect(validatePrefixLabel('ex-2')).toBeNull();
    expect(validatePrefixLabel('my_ns')).toBeNull();
    expect(validatePrefixLabel('a')).toBeNull();
  });

  it('requires a label at all', () => {
    expect(validatePrefixLabel('')).toBe('required');
  });

  it('requires the first character to be a letter', () => {
    expect(validatePrefixLabel('1geo')).toBe('start');
    expect(validatePrefixLabel('_geo')).toBe('start');
    expect(validatePrefixLabel('-geo')).toBe('start');
  });

  it('rejects spaces and punctuation after the first character', () => {
    expect(validatePrefixLabel('my ns')).toBe('charset');
    expect(validatePrefixLabel('geo:')).toBe('charset');
    expect(validatePrefixLabel('geo.ns')).toBe('charset');
    expect(validatePrefixLabel('geo/ns')).toBe('charset');
  });

  it('stops at the server’s 64-character ceiling', () => {
    expect(validatePrefixLabel('a'.repeat(64))).toBeNull();
    expect(validatePrefixLabel('a'.repeat(65))).toBe('tooLong');
  });
});

describe('prefix namespace validation', () => {
  it('accepts absolute http and https IRIs', () => {
    expect(validatePrefixNamespace('http://example.org/ns#')).toBeNull();
    expect(validatePrefixNamespace('https://example.org/ns/')).toBeNull();
  });

  it('requires a namespace at all', () => {
    expect(validatePrefixNamespace('')).toBe('required');
  });

  it('refuses the schemes the server refuses', () => {
    expect(validatePrefixNamespace('javascript:alert(1)')).toBe('scheme');
    expect(validatePrefixNamespace('file:///etc/passwd')).toBe('scheme');
    expect(validatePrefixNamespace('data:text/plain,hi')).toBe('scheme');
    expect(validatePrefixNamespace('urn:example:ns')).toBe('scheme');
  });

  it('refuses anything that is not a URL, relative references included', () => {
    expect(validatePrefixNamespace('example.org/ns')).toBe('format');
    expect(validatePrefixNamespace('/ns#')).toBe('format');
    expect(validatePrefixNamespace('not a namespace')).toBe('format');
  });
});

describe('AdminPrefixes', () => {
  it('lists the overrides this deployment has set', async () => {
    api.adminListPrefixOverrides.mockResolvedValue([GEO]);
    const view = render(AdminPrefixes);
    await view.findByText('geo:');
    expect(view.container.textContent).toContain('https://example.org/geo#');
    expect(api.adminListPrefixOverrides).toHaveBeenCalledTimes(1);
    view.unmount();
  });

  it('explains an empty store rather than showing an empty list', async () => {
    const view = render(AdminPrefixes);
    await view.findByText('No prefix overrides yet');
    view.unmount();
  });

  it('says what a malformed label and namespace are wrong about, and sends nothing', async () => {
    const view = await openAddForm();
    await fireEvent.input(field(view, 'prefix-label'), { target: { value: '1geo' } });
    await fireEvent.input(field(view, 'prefix-namespace'), { target: { value: 'javascript:alert(1)' } });

    await view.findByText('A prefix starts with a letter.');
    await view.findByText('A namespace is an http or https IRI.');

    // The client check is not the gate, but it does keep a doomed request home.
    const submit = view.getByText('Create override');
    await fireEvent.click(submit);
    expect(api.adminCreatePrefixOverride).not.toHaveBeenCalled();
    view.unmount();
  });

  it('shows what a label resolves to today, and from which tier', async () => {
    api.lookupPrefixLabel.mockResolvedValue({
      prefix: 'geo',
      namespace: 'http://www.opengis.net/ont/geosparql#',
      source: 'prefix.cc',
    });
    const view = await openAddForm();
    await fireEvent.input(field(view, 'prefix-label'), { target: { value: 'geo' } });

    await view.findByText('http://www.opengis.net/ont/geosparql#');
    expect(view.container.textContent).toContain('geo: already resolves to');
    expect(view.container.textContent).toContain('from the bundled prefix.cc snapshot');
    expect(api.lookupPrefixLabel).toHaveBeenCalledWith('geo');
    view.unmount();
  });

  it('says a label nothing defines yet is free', async () => {
    const view = await openAddForm();
    await fireEvent.input(field(view, 'prefix-label'), { target: { value: 'acme' } });
    await view.findByText('Nothing defines acme: here yet.');
    view.unmount();
  });

  it('refuses a duplicate label with the namespace it already resolves to', async () => {
    api.adminCreatePrefixOverride.mockRejectedValue(conflictError('geo', 'https://example.org/geo#'));
    const view = await openAddForm();
    await fireEvent.input(field(view, 'prefix-label'), { target: { value: 'geo' } });
    await fireEvent.input(field(view, 'prefix-namespace'), { target: { value: 'https://acme.example/geo#' } });
    await fireEvent.click(view.getByText('Create override'));

    await view.findByText('geo: already has an override here');
    // The 409 body's namespace is the point: the current meaning, verbatim.
    expect(view.container.textContent).toContain('https://example.org/geo#');
    expect(api.adminPutPrefixOverride).not.toHaveBeenCalled();
    view.unmount();
  });

  it('repoints the label from the conflict, with a PUT rather than a second POST', async () => {
    api.adminCreatePrefixOverride.mockRejectedValue(conflictError('geo', 'https://example.org/geo#'));
    api.adminPutPrefixOverride.mockResolvedValue({ ...GEO, namespace: 'https://acme.example/geo#' });
    const view = await openAddForm();
    await fireEvent.input(field(view, 'prefix-label'), { target: { value: 'geo' } });
    await fireEvent.input(field(view, 'prefix-namespace'), { target: { value: 'https://acme.example/geo#' } });
    await fireEvent.click(view.getByText('Create override'));

    const repoint = await view.findByText('Repoint it instead');
    await fireEvent.click(repoint);

    expect(api.adminPutPrefixOverride).toHaveBeenCalledWith('geo', 'https://acme.example/geo#');
    expect(api.adminCreatePrefixOverride).toHaveBeenCalledTimes(1);
    view.unmount();
  });

  it('shows a non-conflict refusal as the server worded it', async () => {
    api.adminCreatePrefixOverride.mockRejectedValue(
      Object.assign(new Error('"geo" is not a prefix label'), { status: 400, body: null }),
    );
    const view = await openAddForm();
    await fireEvent.input(field(view, 'prefix-label'), { target: { value: 'geo' } });
    await fireEvent.input(field(view, 'prefix-namespace'), { target: { value: 'https://acme.example/geo#' } });
    await fireEvent.click(view.getByText('Create override'));

    await view.findByText('"geo" is not a prefix label');
    view.unmount();
  });

  it('repoints an existing override in place', async () => {
    api.adminListPrefixOverrides.mockResolvedValue([GEO]);
    api.adminPutPrefixOverride.mockResolvedValue({ ...GEO, namespace: 'https://acme.example/geo#' });
    const view = render(AdminPrefixes);
    await fireEvent.click(await view.findByText('Repoint'));

    const input = view.container.querySelector('.edit-field input') as HTMLInputElement;
    expect(input.value).toBe('https://example.org/geo#');
    await fireEvent.input(input, { target: { value: 'https://acme.example/geo#' } });
    await fireEvent.click(view.getByText('Save'));

    expect(api.adminPutPrefixOverride).toHaveBeenCalledWith('geo', 'https://acme.example/geo#');
    view.unmount();
  });

  it('asks before removing an override, and says what the label falls back to', async () => {
    api.adminListPrefixOverrides.mockResolvedValue([GEO]);
    api.adminDeletePrefixOverride.mockResolvedValue('');
    const view = render(AdminPrefixes);
    await fireEvent.click(await view.findByText('Remove'));

    await view.findByText('Remove the override for geo:?');
    expect(document.body.textContent).toContain('The prefix itself is not deleted');
    expect(api.adminDeletePrefixOverride).not.toHaveBeenCalled();

    await fireEvent.click(view.getByText('Remove override'));
    expect(api.adminDeletePrefixOverride).toHaveBeenCalledWith('geo');
    view.unmount();
  });
});
