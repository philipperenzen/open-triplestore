# Authentication & API Token Scopes

## Authentication methods

- **Session Token (JWT)** — Issued automatically on login. Used by this browser interface. Valid for the configured session duration. No setup needed.
- **Bearer API Token** — Created in [Settings → API Tokens](/settings). Use in the `Authorization: Bearer <token>` HTTP header. Suitable for scripts, CI/CD, and integrations. Shown once on creation — store it securely.
- **OAuth 2.0 / OIDC** — Configured providers appear on the login page. After SSO, the user receives a normal session token. API tokens can then be issued for programmatic access.

## API token scopes

Each API token carries one or more scopes. Requests to endpoints that require a scope not present on the token receive `403 Forbidden`. Scopes are additive — a token with both `read` and `write` can do everything both allow.

### `read`

Read-only access to all public and permitted resources.

**Allows**

- SPARQL SELECT, ASK, CONSTRUCT, DESCRIBE queries
- Download named graphs in any RDF format
- Browse datasets, organisations, models, vocabularies, and graphs
- Access full-text search (`ft:search` function)
- GeoSPARQL spatial queries
- Download SHACL validation reports
- Read the DCAT catalog and service description
- View user profiles and organisation members

**Cannot**

- Write or modify any triple data
- Upload files or create resources
- Manage users, tokens, or roles

### `write`

Everything in `read`, plus the ability to modify data and manage resources.

**Allows**

- All read-scope operations
- SPARQL Update (INSERT DATA, DELETE DATA, INSERT/DELETE WHERE, CLEAR, COPY, LOAD)
- Upload RDF files and binary assets
- Create and delete datasets, named graphs, models, and vocabularies
- Create organisations and manage memberships
- Upload model and vocabulary versions and publish them
- Run SHACL validation and update shape graph assignments
- Trigger OWL reasoning on any graph

**Cannot**

- List or manage other users' accounts
- Access admin-only API endpoints
- Change user roles or reset passwords

### `admin`

Full access including user management. **Requires an admin or super_admin account role.**

**Allows**

- All read and write scope operations
- List all users, search and filter accounts
- Create, edit, and deactivate user accounts
- Reset any user's password
- Assign user roles (assigning `admin` requires a super_admin account)
- Revoke any user's API tokens
- Access admin-only system statistics

**Cannot**

- Promote to super_admin (only super_admin accounts can do this)

## Account roles

| Role | Who has it | Additional capabilities |
|---|---|---|
| `user` | Default for new accounts | Create datasets and organisations, upload data with a write token |
| `admin` | Assigned by super_admin | All user capabilities + manage users and tokens, and publish models and vocabularies |
| `super_admin` | System owner (configured at setup) | Full access including assigning admin / super_admin roles |

**Publish permission** is an add-on that can be granted to any user (the role stays `user`) by an admin or super-admin. It allows uploading model and vocabulary versions and publishing them. Admins and super-admins always have it implicitly.

Manage your own account from [Settings](/settings): change your password or email address, enable two-factor authentication, create and revoke API tokens, and deactivate or permanently purge your account. Identity providers for SSO are configured by admins under [Security & Access Control](/docs/security).

## Account lifecycle & recovery

Registration validates the email address, username (3–50 chars, letters/digits/`._-`) and password (8+ chars) server-side, and emails a verification link (valid 24 h). Existing accounts created before email verification existed are grandfathered as verified.

Self-service flows (all enumeration-safe — responses never reveal whether an account exists):

- **Forgot password** — `/forgot-password` emails a single-use reset link (valid 1 h). Completing a reset revokes every existing session and counts as proof of mailbox control.
- **Forgot username** — the same page emails the username tied to an address.
- **Change email** — from Settings, requires the current password; the new address only takes effect after its mailbox confirms the emailed link. Without SMTP configured the change applies immediately but is flagged unverified.
- **Two-factor authentication (TOTP)** — enroll from Settings with any authenticator app (QR or manual key). Login then requires a 6-digit code; ten single-use recovery codes are issued at enrollment (shown exactly once). Disabling 2FA requires the password *and* a live code.

### Email delivery configuration

Account email (verification, resets, reminders) is sent through SMTP when configured; otherwise every message — including its action link — is written to the server log so development setups can complete the flows.

| Variable | Meaning |
|---|---|
| `SMTP_HOST` | SMTP relay host (unset → log-only mode) |
| `SMTP_PORT` | Relay port (default 587) |
| `SMTP_USERNAME` / `SMTP_PASSWORD` | Optional credentials |
| `SMTP_TLS` | `none` \| `starttls` \| `implicit` (default: implicit TLS on 465, STARTTLS otherwise). `none` is plaintext — only for a relay on a trusted private network, like the bundled compose relay |
| `SMTP_STARTTLS` | Legacy switch: force STARTTLS on/off; ignored when `SMTP_TLS` is set |
| `SMTP_FROM` | From mailbox, e.g. `Open Triplestore <no-reply@example.org>` |
| `PUBLIC_BASE_URL` | Base URL minted into emailed links (defaults to the server base URL) |
| `OTS_REQUIRE_VERIFIED_EMAIL` | `1` → password login requires a verified address (a fresh link is auto-resent on blocked logins) |

All of these are wired through `docker-compose.yml`, so setting them in `.env` is enough.

#### Bundled relay (Docker Compose)

The compose stack ships a send-only [Postfix relay](https://github.com/bokysan/docker-postfix) behind the `mail` profile. Enable it and point the store at it in `.env`:

```bash
COMPOSE_PROFILES=mail
SMTP_HOST=mail
SMTP_TLS=none          # the hop to the relay stays on the private compose network
SMTP_FROM=Open Triplestore <no-reply@example.org>
MAIL_SENDER_DOMAINS=example.org
BASE_URL=https://data.example.org   # browser-facing origin — the base for emailed links
```

The relay listens only on the compose network (no host port is published, so it cannot be abused as an open relay) and persists its queue, so deferred mail keeps retrying across restarts. By default it delivers straight to each recipient's MX — that only lands when the host can egress port 25 and `MAIL_HOSTNAME` has matching forward/reverse DNS plus an SPF record. From a laptop or an IP without mail reputation, set `MAIL_RELAYHOST` (+ `MAIL_RELAYHOST_USERNAME` / `MAIL_RELAYHOST_PASSWORD`) to route through a provider smarthost instead. See `.env.example` for the full variable list.

## SSO provider setup (OIDC / SAML)

> **SAML needs the `saml` build feature.** The published Docker image has it
> (the Dockerfile's `CARGO_FEATURES`). `full`, and so a plain `cargo build`,
> does not: the feature links libxml2 and libxmlsec1 and needs pkg-config and
> libclang to build ([build features](build-features.md)).


Providers are configured by admins under **Security & Access Control → Identity providers**. Any standards-compliant OIDC or SAML 2.0 IdP works; the callback/redirect URL to register at the IdP is always:

```
https://<your-host>/api/auth/oauth/<slug>/callback     (OIDC)
https://<your-host>/api/auth/saml/<slug>/acs           (SAML)
```

### Google

1. In [Google Cloud Console](https://console.cloud.google.com/apis/credentials) create an **OAuth client ID** (type *Web application*) and add the callback URL above as an authorized redirect URI.
2. Add a provider with type **OIDC**, discovery URL `https://accounts.google.com/.well-known/openid-configuration`, the client ID/secret from step 1, and scopes `openid email profile`.

Google asserts `email_verified`, so verified Google emails can auto-link to existing local accounts of the same address.

### Microsoft Entra ID (Azure AD)

1. In the [Entra admin center](https://entra.microsoft.com) register an application (*App registrations → New*), add the callback URL as a **Web** redirect URI, and create a client secret.
2. Add a provider with type **OIDC/Azure**, discovery URL `https://login.microsoftonline.com/<tenant-id>/v2.0/.well-known/openid-configuration`, the application (client) ID and secret, and scopes `openid email profile`.
3. To map directory roles/groups, emit them in the token (App roles or the `groups` claim) and configure the provider's *role claim map*, e.g. `{"ots-admins": "admin"}`.

### Apple

Sign in with Apple is **not yet supported** by the generic OIDC integration: Apple requires the client secret to be a short-lived ES256-signed JWT minted from a developer key (`.p8`), rather than a static secret, and returns the authorization response via `form_post`. Until dedicated support lands, front Apple sign-in through a federating IdP (Keycloak, Auth0, Entra External ID) and connect that IdP here as a regular OIDC provider.

### Other IdPs (Keycloak, Auth0, Okta, …)

Any IdP exposing a `.well-known/openid-configuration` works with the generic OIDC type; enterprise IdPs can also connect via SAML 2.0 (see below).

### SAML 2.0

This store acts as the SAML **service provider** (SP) for the web-browser
sign-in profile (SAML 2.0 Profiles §4.1), following the Kantara saml2int
deployment profile. In scope: sign-in started here (AuthnRequest over
HTTP-Redirect, optionally signed; response over HTTP-POST) or at the IdP (off
by default), signed responses with SHA-256 or stronger, encrypted assertions,
IdP metadata import with several signing certificates, SP key rollover,
persistent NameIDs, attribute-to-role mapping and Single Logout started from
either side. Out of scope: the Artifact binding, ECP, attribute queries,
NameID management, and federation metadata aggregates or MDQ (paste the one IdP
you trust instead).

#### Connect an IdP

1. Add a provider with type **SAML**. Under **Import IdP metadata**, give the
   IdP's metadata URL (fetched over https, redirects not followed) or paste its
   XML. That fills in the IdP's entity ID, SSO URL (HTTP-Redirect binding),
   logout URL and every signing certificate. You can also type them:
   - **Entity ID**: the IdP's entity ID. Responses must carry it as `Issuer`.
   - **SSO URL**: `https`, or `http` on loopback only.
   - **IdP signing certificates**: one or more, PEM or bare base64. During an
     IdP key rollover keep the old and the new one; responses signed with
     either are accepted. A provider without one never starts a sign-in.
2. Save, then register the store at the IdP. Either import our SP metadata
   from `https://<your-host>/api/auth/saml/<slug>/metadata` (served once the
   provider is active; admins can download it before from
   `GET /api/admin/oauth/providers/<id>/saml/metadata`), or enter:

   ```
   Entity ID:  https://<your-host>/api/auth/saml/<slug>/metadata
   ACS URL:    https://<your-host>/api/auth/saml/<slug>/acs   (HTTP-POST)
   SLO URL:    https://<your-host>/api/auth/saml/<slug>/slo   (HTTP-Redirect or HTTP-POST)
   ```

   The provider form shows these, our signing/encryption certificate and the
   transport check under **This store as service provider**.

#### What the store checks

The login button sends the browser to `/api/auth/saml/<slug>/login`, which
redirects to the IdP with an AuthnRequest (a 128-bit ID and a NameID policy)
and binds the attempt to the browser with a short-lived `saml_state` cookie.
The ACS accepts a response only if:

- it comes back within 10 minutes, from the same browser, answers that
  AuthnRequest (`InResponseTo`), and only once;
- the response or the assertion is signed by one of the configured IdP
  certificates, with RSA or ECDSA and SHA-256, -384 or -512 (SHA-1 is refused);
- its issuer, audience (our entity ID), destination and recipient (our ACS) are
  right, and it is inside its validity window (`clock_skew_seconds` tolerance,
  default 180 s, at most 600 s);
- the XML has no DOCTYPE and is at most 1 MiB;
- the NameID is not transient, unless a **subject attribute** identifies the
  account instead.

A refused response gets a generic `401 {"error":"SAML sign-in failed"}`; the
reason goes to the audit log (`LoginFailure`, `detail`) and the server log. On
success the browser lands on `/oauth/callback`, signed in.

**IdP-initiated sign-in** (from the IdP's app portal) is off by default: an
unsolicited response is not tied to a browser, so it allows login CSRF. Turn
on *Accept sign-ins started at the IdP* to accept them; each assertion is then
accepted once (replay cache), and one that carries an `InResponseTo` is refused.

#### Encrypted assertions, signed requests and SP keys

Each provider gets its own SP key pair (RSA-3072, self-signed certificate) the
first time it is needed. The private key is stored like an OAuth client secret
(`auth::secret`): encrypted with a key derived from `JWT_SECRET`, or, for a key
you import, as a secret reference (`env:`, `file:`, `vault:`). The metadata
publishes every key with `use="signing"` and with `use="encryption"`.

- **Encrypted assertions** are accepted with AES-128/192/256-GCM or -CBC and
  RSA-OAEP key transport (`rsa-oaep-mgf1p`, or xmlenc 1.1 `rsa-oaep` with
  SHA-1/256/384/512). RSA PKCS#1 v1.5 key transport is refused. Either the
  whole response must be signed or the assertion inside the encryption must
  be. *Require encrypted assertions* refuses plain ones.
- **Signed AuthnRequests**: *Sign authentication requests* signs the
  HTTP-Redirect query with RSA-SHA256 and the metadata says
  `AuthnRequestsSigned="true"`. Logout messages are always signed.
- **Key rollover**: add a key (`POST /api/admin/oauth/providers/<id>/saml/keys`,
  generated, or `{private_key, certificate}`); it is published and decrypts at
  once. When the IdP has read the new metadata, activate it
  (`…/keys/<kid>/activate`) so it signs, then delete the old key
  (`DELETE …/keys/<kid>`; the signing key cannot be deleted).

#### Attributes and roles

Attribute names are matched against `Name` and `FriendlyName`. By default:

| Field | Attributes |
|---|---|
| Email | `urn:oid:0.9.2342.19200300.100.1.3`, `…/claims/emailaddress`, `email`, `mail`, `emailAddress` |
| Display name | `urn:oid:2.16.840.1.113730.3.1.241`, `urn:oid:2.5.4.3`, `…/identity/claims/displayname`, `…/claims/name`, `displayName`, `cn` |
| Groups | `urn:oid:1.3.6.1.4.1.5923.1.5.1.1` (isMemberOf), `urn:oid:1.3.6.1.4.1.5923.1.1.1.7` (eduPersonEntitlement), `…/claims/groups`, `…/claims/role`, `memberOf`, `groups`, `role`, `Role` |

Each list can be replaced per provider. Group values go through the role claim
map. As with OIDC, the mapped role is set when the account is created; later
sign-ins keep the account's role (an admin changes it), the `publisher` grant
is only ever added, and sign-in never grants `super_admin`.

SAML has no standard `email_verified` assertion, so a SAML sign-in never links
to an existing local account by email. A new subject gets a new account (with
auto-provisioning on).

#### Single Logout

Signing out of a SAML session revokes its whole refresh-token family. When the
provider has an IdP logout URL, `POST /api/auth/logout` answers
`{"saml_logout_url": …}` and the web UI sends the browser there with a signed
LogoutRequest naming the NameID and SessionIndex; the IdP's signed
LogoutResponse brings it back. A signed LogoutRequest from the IdP (redirect
or POST binding) revokes the sessions it names (by SessionIndex when it lists
any), is accepted once, and is answered with a signed LogoutResponse.

**Limit:** access tokens are stateless JWTs. One issued before the logout
stays valid until it expires (`ACCESS_TOKEN_EXPIRY_MINUTES`, default 30); only
the refresh token is revoked. Shorten the access-token lifetime if that window
matters.

#### Transport

The IdP returns the browser to the ACS with a cross-site POST, which only a
`SameSite=None; Secure` cookie survives, so SAML needs an `https` `BASE_URL`.
The `saml_state` cookie is `SameSite=None; Secure` whenever `BASE_URL` is
https, whatever `SECURE_COOKIES` says. With a plain-http `BASE_URL` the login
route refuses to start, except on `localhost`, where the cookie is
`SameSite=Lax` and an IdP on the same site works.

#### IdP settings that matter

- Sign the response or the assertion with SHA-256; give the bearer subject
  confirmation a `NotOnOrAfter`.
- Release a persistent NameID (or a stable attribute, configured as the
  subject attribute).
- For Keycloak: set *Name ID format* to `persistent`; *Client signature
  required* only if you turn on signed requests; *Encrypt assertions* works
  with the metadata's encryption key; set the *Logout Service Redirect Binding
  URL* to our SLO URL.
- For Microsoft Entra ID: the default NameID is persistent per application;
  map groups through the `…/claims/groups` claim.

## Guest self-registration (admin toggle)

With normal self-registration closed (`OTS_DISABLE_REGISTRATION=1`), an admin
can still open a low-privilege path from **Security → Registration**: the
public register page then creates accounts with the `guest` system role (no
publish/design/admin rights — client apps decide what guests may fill, e.g.
guest-open forms). The toggle is runtime-changeable (no restart):

- **Off → guests disabled.** Every active guest account is bulk-deactivated
  with the stored reason `guest_disabled`; their sign-in (and any still-valid
  token, on introspection) answers with *"Guest access has been disabled by
  the administrator"* — client apps surface that message verbatim. Guests an
  admin deactivated individually are never touched by the sweep.
- **On → exactly those guests return.** The sweep re-activates only accounts
  carrying the `guest_disabled` reason.

`GET /api/auth/features` exposes the toggle so register UIs adapt their
wording; sweeps are audit-logged with the affected count.

## Introspection for resource servers

Companion services authorizing on this store's accounts get two additive
surfaces:

- `GET /api/auth/me` includes `organisations: [{slug, name, role}]` and
  `groups: [{org_slug, id, name}]` (groups have no slug; match on name/id).
- `GET /api/datasets/:id/permissions/me` reports the caller's effective
  `{role, read, write, manage}` on one dataset through the full ACL stack;
  no access answers 404, indistinguishable from a nonexistent dataset.

For signing users in *to those services* with this store as the identity
provider, see [`oidc-provider.md`](oidc-provider.md).
