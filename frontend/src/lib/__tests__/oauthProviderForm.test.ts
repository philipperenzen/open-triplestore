/**
 * Security → Identity providers: the form must round-trip through the provider
 * API (`OauthProvider` / `OauthProviderCreate` in src/auth/models.rs). The API
 * sends and takes `scopes` and `role_claim_map` as strings, calls the switch
 * `is_active`, and never returns the client secret or the SAML certificate, so
 * an edit that leaves those blank must omit them for the server to keep them.
 */
import { describe, it, expect, vi, beforeAll, beforeEach } from 'vitest';
import { render, cleanup, fireEvent, waitFor } from '@testing-library/svelte';
import { init, addMessages } from 'svelte-i18n';
import en from '../i18n/en.json';
import { user, authInitialized } from '../stores';
import {
  emptyProviderForm,
  providerToForm,
  formToBody,
  normaliseRoleClaimMap,
  ProviderFormError,
  type OauthProvider,
} from '../oauthProviderForm';

const api = vi.hoisted(() => ({
  adminListOauthProviders: vi.fn(),
  adminCreateOauthProvider: vi.fn(),
  adminUpdateOauthProvider: vi.fn(),
  adminDeleteOauthProvider: vi.fn(),
  listEndpointAclRules: vi.fn(),
  createEndpointAclRule: vi.fn(),
  updateEndpointAclRule: vi.fn(),
  deleteEndpointAclRule: vi.fn(),
  listGraphAclRules: vi.fn(),
  grantGraphPermission: vi.fn(),
  revokeGraphPermission: vi.fn(),
  listTripleSecurityLabels: vi.fn(),
  createTripleSecurityLabel: vi.fn(),
  deleteTripleSecurityLabel: vi.fn(),
  browseGraphs: vi.fn(),
  adminListUsers: vi.fn(),
  listOrganisations: vi.fn(),
  adminGetGuestRegistration: vi.fn(),
  adminSetGuestRegistration: vi.fn(),
  adminListOauthClients: vi.fn(),
  adminUpsertOauthClient: vi.fn(),
  adminDeleteOauthClient: vi.fn(),
}));
vi.mock('../api.js', () => api);

import AdminSecurity from '../../pages/AdminSecurity.svelte';

/** The row `ensure_env_provider` creates for OIDC_ISSUER: no client ID. */
const ENV_OIDC: OauthProvider = {
  id: 'p-env',
  name: 'Environment OIDC',
  slug: 'env-oidc',
  provider_type: 'oidc',
  client_id: null,
  discovery_url: 'https://idp.example.org/realms/example/.well-known/openid-configuration',
  tenant_id: null,
  entity_id: null,
  sso_url: null,
  scopes: 'openid email profile',
  role_claim_map: null,
  auto_provision: true,
  default_role: 'user',
  is_active: true,
};

const CORP: OauthProvider = {
  ...ENV_OIDC,
  id: 'p-corp',
  name: 'Corporate IdP',
  slug: 'corp',
  client_id: 'ots-web',
  tenant_id: 'tenant-1',
  role_claim_map: '{"staff":"user","ops":"admin"}',
  is_active: false,
};

const SAML: OauthProvider = {
  ...ENV_OIDC,
  id: 'p-saml',
  name: 'Example SAML',
  slug: 'example-saml',
  provider_type: 'saml',
  discovery_url: null,
  entity_id: 'https://idp.example.org/saml',
  sso_url: 'https://idp.example.org/sso',
};

describe('provider ↔ form mapping', () => {
  it('opens every provider shape without throwing, scopes as the server sent them', () => {
    for (const p of [ENV_OIDC, CORP, SAML]) {
      const f = providerToForm(p);
      expect(f.scopes).toBe('openid email profile');
      expect(f.client_secret).toBe('');
      expect(f.idp_certificate).toBe('');
    }
    expect(providerToForm(ENV_OIDC).client_id).toBe('');
    expect(providerToForm(CORP).is_active).toBe(false);
  });

  it('sends the env-oidc row back unchanged, with client_id still null', () => {
    const body = formToBody(providerToForm(ENV_OIDC));
    expect(body).toEqual({
      name: 'Environment OIDC',
      slug: 'env-oidc',
      provider_type: 'oidc',
      client_id: null,
      discovery_url: ENV_OIDC.discovery_url,
      tenant_id: null,
      entity_id: null,
      sso_url: null,
      scopes: 'openid email profile',
      role_claim_map: null,
      auto_provision: true,
      default_role: 'user',
      is_active: true,
    });
  });

  it('omits a blank secret and certificate so the server keeps the stored ones', () => {
    const body = formToBody(providerToForm(SAML));
    expect('client_secret' in body).toBe(false);
    expect('idp_certificate' in body).toBe(false);
    expect(body.entity_id).toBe(SAML.entity_id);
    expect(body.sso_url).toBe(SAML.sso_url);
    const f = { ...providerToForm(SAML), idp_certificate: '  -----BEGIN CERTIFICATE-----\nAAA\n' };
    expect(formToBody(f).idp_certificate).toBe('-----BEGIN CERTIFICATE-----\nAAA');
    const g = { ...providerToForm(CORP), client_secret: 'env:IDP_SECRET' };
    expect(formToBody(g).client_secret).toBe('env:IDP_SECRET');
  });

  it('carries fields the form does not show (tenant_id) through an edit', () => {
    expect(formToBody(providerToForm(CORP)).tenant_id).toBe('tenant-1');
  });

  it('sends scopes as one space-separated string', () => {
    const f = { ...emptyProviderForm(), name: 'X', slug: 'x', scopes: '  openid\n email   groups ' };
    expect(formToBody(f).scopes).toBe('openid email groups');
    expect(formToBody({ ...f, scopes: '  ' }).scopes).toBeNull();
  });

  it('sends role_claim_map as a JSON string and round-trips it', () => {
    const f = providerToForm(CORP);
    expect(JSON.parse(f.role_claim_map)).toEqual({ staff: 'user', ops: 'admin' });
    expect(formToBody(f).role_claim_map).toBe('{"staff":"user","ops":"admin"}');
  });

  it('refuses a role claim map the server would read as no mapping', () => {
    for (const bad of ['{', '[]', '"admin"', '{"a": 1}', '{"a": {"b": "admin"}}']) {
      expect(() => normaliseRoleClaimMap(bad)).toThrow(ProviderFormError);
    }
    expect(normaliseRoleClaimMap('')).toBeNull();
    expect(normaliseRoleClaimMap('{}')).toBeNull();
  });

  it('saves an older azure_ad row as oidc, the type the sign-in flow runs', () => {
    expect(providerToForm({ ...CORP, provider_type: 'azure_ad' }).provider_type).toBe('oidc');
  });

  it('requires a name and a slug', () => {
    expect(() => formToBody(emptyProviderForm())).toThrow(ProviderFormError);
  });
});

beforeAll(() => {
  addMessages('en', en as unknown as Parameters<typeof addMessages>[1]);
  init({ fallbackLocale: 'en', initialLocale: 'en' });
});

beforeEach(() => {
  cleanup();
  vi.clearAllMocks();
  for (const fn of Object.values(api)) fn.mockResolvedValue([]);
  api.adminListOauthProviders.mockResolvedValue([ENV_OIDC, CORP]);
  api.adminUpdateOauthProvider.mockResolvedValue(undefined);
  api.adminCreateOauthProvider.mockResolvedValue({ ...CORP, id: 'p-new' });
  user.set({ id: 'u1', username: 'admin', role: 'super_admin' } as never);
  authInitialized.set(true);
});

async function renderPage() {
  const view = render(AdminSecurity);
  await view.findByText('Environment OIDC');
  return view;
}

function rowOf(view: { getByText: (t: string) => HTMLElement }, name: string): HTMLElement {
  return view.getByText(name).closest('tr') as HTMLElement;
}

function field(view: { container: HTMLElement }, id: string): HTMLInputElement {
  return view.container.querySelector(`#${id}`) as HTMLInputElement;
}

describe('Identity providers page', () => {
  it('shows each provider status from is_active', async () => {
    const view = await renderPage();
    expect(rowOf(view, 'Environment OIDC').textContent).toContain('Enabled');
    expect(rowOf(view, 'Corporate IdP').textContent).toContain('Disabled');
  });

  it('edits the env-oidc row and saves it back unchanged', async () => {
    const view = await renderPage();
    const row = rowOf(view, 'Environment OIDC');
    await fireEvent.click(row.querySelector('button[title="Edit"]') as HTMLElement);
    expect(field(view, 'prov-scopes').value).toBe('openid email profile');
    expect(field(view, 'prov-slug').disabled).toBe(true);
    await fireEvent.click(view.getByText('Save Changes'));
    await waitFor(() => expect(api.adminUpdateOauthProvider).toHaveBeenCalledTimes(1));
    const [id, body] = api.adminUpdateOauthProvider.mock.calls[0];
    expect(id).toBe('p-env');
    expect(body).toEqual(formToBody(providerToForm(ENV_OIDC)));
    expect(body.client_id).toBeNull();
    expect('client_secret' in body).toBe(false);
  });

  it('enables a disabled provider through the form switch', async () => {
    const view = await renderPage();
    await fireEvent.click(rowOf(view, 'Corporate IdP').querySelector('button[title="Edit"]') as HTMLElement);
    const toggle = view.getByRole('switch', { name: 'Enabled' });
    expect(toggle.getAttribute('aria-checked')).toBe('false');
    await fireEvent.click(toggle);
    await fireEvent.click(view.getByText('Save Changes'));
    await waitFor(() => expect(api.adminUpdateOauthProvider).toHaveBeenCalledTimes(1));
    const [, body] = api.adminUpdateOauthProvider.mock.calls[0];
    expect(body.is_active).toBe(true);
    expect('enabled' in body).toBe(false);
    expect(body.role_claim_map).toBe(CORP.role_claim_map);
    expect(body.tenant_id).toBe('tenant-1');
  });

  it('creates a provider with string scopes and role map', async () => {
    const view = await renderPage();
    await fireEvent.click(view.getByText('Add Provider'));
    await fireEvent.input(field(view, 'prov-name'), { target: { value: 'Example IdP' } });
    await fireEvent.input(field(view, 'prov-slug'), { target: { value: 'example' } });
    await fireEvent.input(field(view, 'prov-client-id'), { target: { value: 'ots' } });
    await fireEvent.input(field(view, 'prov-client-secret'), { target: { value: 'env:IDP_SECRET' } });
    await fireEvent.input(field(view, 'prov-role-map'), { target: { value: '{"ops": "admin"}' } });
    await fireEvent.click(view.getByText('Create Provider'));
    await waitFor(() => expect(api.adminCreateOauthProvider).toHaveBeenCalledTimes(1));
    const [body] = api.adminCreateOauthProvider.mock.calls[0];
    expect(body).toMatchObject({
      name: 'Example IdP',
      slug: 'example',
      provider_type: 'oidc',
      client_id: 'ots',
      client_secret: 'env:IDP_SECRET',
      scopes: 'openid email profile',
      role_claim_map: '{"ops":"admin"}',
      is_active: true,
      auto_provision: true,
    });
  });

  it('refuses a malformed role map without calling the server', async () => {
    const view = await renderPage();
    await fireEvent.click(view.getByText('Add Provider'));
    await fireEvent.input(field(view, 'prov-name'), { target: { value: 'Example IdP' } });
    await fireEvent.input(field(view, 'prov-slug'), { target: { value: 'example' } });
    await fireEvent.input(field(view, 'prov-role-map'), { target: { value: '["admin"]' } });
    await fireEvent.click(view.getByText('Create Provider'));
    await view.findByText(/must be a JSON object/);
    expect(api.adminCreateOauthProvider).not.toHaveBeenCalled();
  });
});
