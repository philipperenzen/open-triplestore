/**
 * The identity-provider form on Security → Identity providers, and how it maps
 * to and from the provider API (`OauthProvider` / `OauthProviderCreate` in
 * `src/auth/models.rs`). Pure functions, so the mapping is tested without the
 * page.
 *
 * What the API expects, and what this module keeps straight:
 * - `scopes` is one space-separated string, not an array.
 * - `role_claim_map` is a JSON *string*, and the server reads it as a flat map
 *   of claim value → grant. Anything else (an array, a nested value) is
 *   silently treated as "no mapping", so the form refuses it up front.
 * - `is_active` (not `enabled`) and `auto_provision` are required booleans.
 * - An update replaces every column, so fields the form does not show
 *   (`tenant_id`, and the OIDC/SAML fields of the other type) are carried over
 *   from the provider being edited rather than dropped.
 * - The client secret and the SAML IdP certificate are never sent back by the
 *   server. Left blank, they are omitted from the body and the server keeps
 *   the stored value.
 * - `saml_config` is sent whole for a SAML provider (the server replaces it)
 *   and not at all for OIDC (the server keeps what is stored).
 */

export type ProviderType = 'oidc' | 'saml';

/** `saml_config` as the API reads and writes it (`SamlConfig` in models.rs). */
export interface SamlConfig {
  sp_entity_id?: string | null;
  idp_metadata_url?: string | null;
  idp_slo_url?: string | null;
  idp_slo_response_url?: string | null;
  name_id_format?: string | null;
  subject_attribute?: string | null;
  email_attributes?: string[];
  name_attributes?: string[];
  group_attributes?: string[];
  allow_idp_initiated?: boolean;
  clock_skew_seconds?: number | null;
  sign_authn_requests?: boolean;
  require_encrypted_assertions?: boolean;
  contact_email?: string | null;
}

/** What `POST /api/admin/oauth/saml-metadata` returns. */
export interface IdpMetadata {
  entity_id: string;
  sso_url: string;
  slo_url: string | null;
  slo_response_url: string | null;
  certificates_pem: string;
  certificates: { sha256_fingerprint?: string; subject?: string; not_after?: string }[];
  want_authn_requests_signed: boolean;
}

export const NAMEID_PERSISTENT = 'urn:oasis:names:tc:SAML:2.0:nameid-format:persistent';
export const NAMEID_TRANSIENT = 'urn:oasis:names:tc:SAML:2.0:nameid-format:transient';
export const NAMEID_EMAIL = 'urn:oasis:names:tc:SAML:1.1:nameid-format:emailAddress';
export const NAMEID_UNSPECIFIED = 'urn:oasis:names:tc:SAML:1.1:nameid-format:unspecified';

/** The SAML part of the form. Lists are edited as one name per line. */
export interface SamlForm {
  sp_entity_id: string;
  idp_metadata_url: string;
  idp_slo_url: string;
  idp_slo_response_url: string;
  name_id_format: string;
  subject_attribute: string;
  email_attributes: string;
  name_attributes: string;
  group_attributes: string;
  allow_idp_initiated: boolean;
  clock_skew_seconds: string;
  sign_authn_requests: boolean;
  require_encrypted_assertions: boolean;
  contact_email: string;
}

/** A provider as `GET /api/admin/oauth/providers` returns it. */
export interface OauthProvider {
  id: string;
  name: string;
  slug: string;
  provider_type: string;
  client_id: string | null;
  discovery_url: string | null;
  tenant_id: string | null;
  entity_id: string | null;
  sso_url: string | null;
  scopes: string;
  role_claim_map: string | null;
  auto_provision: boolean;
  default_role: string;
  is_active: boolean;
  saml_config?: SamlConfig | null;
  created_at?: string;
  updated_at?: string;
}

/** The body of `POST` / `PUT /api/admin/oauth/providers`. */
export interface OauthProviderBody {
  name: string;
  slug: string;
  provider_type: ProviderType;
  client_id: string | null;
  client_secret?: string;
  discovery_url: string | null;
  tenant_id: string | null;
  entity_id: string | null;
  sso_url: string | null;
  idp_certificate?: string;
  scopes: string | null;
  role_claim_map: string | null;
  auto_provision: boolean;
  default_role: string;
  is_active: boolean;
  /** Sent for SAML providers only; absent keeps the stored settings. */
  saml_config?: SamlConfig;
}

/** What the form edits. Every text field is a string, never null. */
export interface ProviderForm {
  name: string;
  slug: string;
  provider_type: ProviderType;
  client_id: string;
  client_secret: string;
  discovery_url: string;
  scopes: string;
  entity_id: string;
  sso_url: string;
  idp_certificate: string;
  /** Not shown; carried so an edit does not clear it. */
  tenant_id: string;
  default_role: string;
  role_claim_map: string;
  auto_provision: boolean;
  is_active: boolean;
  saml: SamlForm;
}

/** Slug of the provider row the server creates for `OIDC_ISSUER`. The server
 *  finds that row by slug, so the form does not let it be renamed. */
export const ENV_OIDC_SLUG = 'env-oidc';

export const DEFAULT_SCOPES = 'openid email profile';

/** A form value the server would refuse or misread; `key` is an i18n key. */
export class ProviderFormError extends Error {
  key: string;
  constructor(key: string) {
    super(key);
    this.key = key;
  }
}

export function emptySamlForm(): SamlForm {
  return {
    sp_entity_id: '',
    idp_metadata_url: '',
    idp_slo_url: '',
    idp_slo_response_url: '',
    name_id_format: NAMEID_PERSISTENT,
    subject_attribute: '',
    email_attributes: '',
    name_attributes: '',
    group_attributes: '',
    allow_idp_initiated: false,
    clock_skew_seconds: '',
    sign_authn_requests: false,
    require_encrypted_assertions: false,
    contact_email: '',
  };
}

function samlToForm(c: SamlConfig | null | undefined): SamlForm {
  const f = emptySamlForm();
  if (!c) return f;
  const lines = (v?: string[]) => (v ?? []).join('\n');
  return {
    sp_entity_id: c.sp_entity_id ?? '',
    idp_metadata_url: c.idp_metadata_url ?? '',
    idp_slo_url: c.idp_slo_url ?? '',
    idp_slo_response_url: c.idp_slo_response_url ?? '',
    name_id_format: c.name_id_format || NAMEID_PERSISTENT,
    subject_attribute: c.subject_attribute ?? '',
    email_attributes: lines(c.email_attributes),
    name_attributes: lines(c.name_attributes),
    group_attributes: lines(c.group_attributes),
    allow_idp_initiated: c.allow_idp_initiated === true,
    clock_skew_seconds: c.clock_skew_seconds == null ? '' : String(c.clock_skew_seconds),
    sign_authn_requests: c.sign_authn_requests === true,
    require_encrypted_assertions: c.require_encrypted_assertions === true,
    contact_email: c.contact_email ?? '',
  };
}

/** One attribute name per line (commas also separate). */
function names(text: string): string[] {
  return text
    .split(/[\n,]/)
    .map((s) => s.trim())
    .filter(Boolean);
}

/** `saml_config` for the request body. Throws {@link ProviderFormError}. */
export function samlFormToConfig(f: SamlForm): SamlConfig {
  let skew: number | null = null;
  if (f.clock_skew_seconds.trim() !== '') {
    skew = Number(f.clock_skew_seconds.trim());
    if (!Number.isInteger(skew) || skew < 0 || skew > 600) {
      throw new ProviderFormError('pages.adminSecurity.samlClockSkewInvalid');
    }
  }
  if (f.name_id_format === NAMEID_TRANSIENT && !f.subject_attribute.trim()) {
    throw new ProviderFormError('pages.adminSecurity.samlTransientNeedsSubject');
  }
  return {
    sp_entity_id: opt(f.sp_entity_id),
    idp_metadata_url: opt(f.idp_metadata_url),
    idp_slo_url: opt(f.idp_slo_url),
    idp_slo_response_url: opt(f.idp_slo_response_url),
    name_id_format: f.name_id_format === NAMEID_PERSISTENT ? null : opt(f.name_id_format),
    subject_attribute: opt(f.subject_attribute),
    email_attributes: names(f.email_attributes),
    name_attributes: names(f.name_attributes),
    group_attributes: names(f.group_attributes),
    allow_idp_initiated: f.allow_idp_initiated,
    clock_skew_seconds: skew,
    sign_authn_requests: f.sign_authn_requests,
    require_encrypted_assertions: f.require_encrypted_assertions,
    contact_email: opt(f.contact_email),
  };
}

/** Fill the form from imported IdP metadata. `url` is where it came from. */
export function applyIdpMetadata(form: ProviderForm, md: IdpMetadata, url?: string): ProviderForm {
  return {
    ...form,
    entity_id: md.entity_id,
    sso_url: md.sso_url,
    idp_certificate: md.certificates_pem,
    saml: {
      ...form.saml,
      idp_metadata_url: url ?? form.saml.idp_metadata_url,
      idp_slo_url: md.slo_url ?? '',
      idp_slo_response_url: md.slo_response_url ?? '',
      sign_authn_requests: form.saml.sign_authn_requests || md.want_authn_requests_signed,
    },
  };
}

export function emptyProviderForm(): ProviderForm {
  return {
    name: '',
    slug: '',
    provider_type: 'oidc',
    client_id: '',
    client_secret: '',
    discovery_url: '',
    scopes: DEFAULT_SCOPES,
    entity_id: '',
    sso_url: '',
    idp_certificate: '',
    tenant_id: '',
    default_role: 'user',
    role_claim_map: '',
    auto_provision: true,
    is_active: true,
    saml: emptySamlForm(),
  };
}

/** The form for editing `p`. */
export function providerToForm(p: OauthProvider): ProviderForm {
  return {
    name: p.name ?? '',
    slug: p.slug ?? '',
    // The server runs the OIDC flow only for `oidc`; an Entra ID tenant is an
    // OIDC provider with a tenant discovery URL. A row stored under any other
    // type (an older `azure_ad`) is saved back as `oidc` so it can sign in.
    provider_type: p.provider_type === 'saml' ? 'saml' : 'oidc',
    client_id: p.client_id ?? '',
    client_secret: '',
    discovery_url: p.discovery_url ?? '',
    scopes: p.scopes ?? DEFAULT_SCOPES,
    entity_id: p.entity_id ?? '',
    sso_url: p.sso_url ?? '',
    idp_certificate: '',
    tenant_id: p.tenant_id ?? '',
    // Sign-in through a provider never grants super_admin (both flows cap it
    // at admin), so the form offers user and admin and shows what applies.
    default_role: p.default_role === 'super_admin' ? 'admin' : p.default_role || 'user',
    role_claim_map: prettyRoleClaimMap(p.role_claim_map),
    auto_provision: p.auto_provision !== false,
    is_active: p.is_active !== false,
    saml: samlToForm(p.saml_config),
  };
}

function prettyRoleClaimMap(raw: string | null): string {
  if (!raw) return '';
  try {
    const parsed = JSON.parse(raw);
    if (parsed && typeof parsed === 'object' && Object.keys(parsed).length === 0) return '';
    return JSON.stringify(parsed, null, 2);
  } catch {
    // Show what is stored, so the admin can see and correct it.
    return raw;
  }
}

/** Blank → null, so an unset optional field stays unset. */
function opt(s: string): string | null {
  const v = s.trim();
  return v === '' ? null : v;
}

/** The role claim map as the JSON string the server stores, or null for none.
 *  Throws when the server would read it as an empty map. */
export function normaliseRoleClaimMap(text: string): string | null {
  const v = text.trim();
  if (v === '') return null;
  let parsed: unknown;
  try {
    parsed = JSON.parse(v);
  } catch {
    throw new ProviderFormError('pages.adminSecurity.roleClaimMapInvalid');
  }
  if (
    !parsed ||
    typeof parsed !== 'object' ||
    Array.isArray(parsed) ||
    !Object.values(parsed).every((x) => typeof x === 'string')
  ) {
    throw new ProviderFormError('pages.adminSecurity.roleClaimMapInvalid');
  }
  return Object.keys(parsed).length === 0 ? null : JSON.stringify(parsed);
}

/** The request body for the form. Throws {@link ProviderFormError}. */
export function formToBody(form: ProviderForm): OauthProviderBody {
  const name = form.name.trim();
  const slug = form.slug.trim();
  if (!name || !slug) throw new ProviderFormError('pages.adminSecurity.nameSlugRequired');
  const scopes = form.scopes.split(/\s+/).filter(Boolean).join(' ');
  const body: OauthProviderBody = {
    name,
    slug,
    provider_type: form.provider_type,
    client_id: opt(form.client_id),
    discovery_url: opt(form.discovery_url),
    tenant_id: opt(form.tenant_id),
    entity_id: opt(form.entity_id),
    sso_url: opt(form.sso_url),
    scopes: scopes || null,
    role_claim_map: normaliseRoleClaimMap(form.role_claim_map),
    auto_provision: form.auto_provision,
    default_role: form.default_role || 'user',
    is_active: form.is_active,
  };
  // Blank means "keep what is stored": the server keeps the existing value
  // when the field is absent, and would store an empty one if it were sent.
  if (form.client_secret.trim()) body.client_secret = form.client_secret.trim();
  if (form.idp_certificate.trim()) body.idp_certificate = form.idp_certificate.trim();
  if (form.provider_type === 'saml') body.saml_config = samlFormToConfig(form.saml);
  return body;
}
