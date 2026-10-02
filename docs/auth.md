# Authentication & API Token Scopes

## Authentication methods

- **Session Token (JWT)** — Issued automatically on login. Used by this browser interface. Valid for the configured session duration. No setup needed.
- **Bearer API Token** — Created in [Settings → API Tokens](/settings). Use in the `Authorization: Bearer <token>` HTTP header. Suitable for scripts, CI/CD, and integrations. Shown once on creation — store it securely.
- **OAuth 2.0 / OIDC** — Configured providers appear on the login page. After SSO, the user receives a normal session token. API tokens can then be issued for programmatic access.
- **IdP access token** — Off by default. With [resource-server mode](#oidc-resource-server-mode-idp-access-tokens) configured, a client that already holds an access token from your identity provider sends it as `Authorization: Bearer <token>`, and the store verifies it against the provider's keys.

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

> **SAML is experimental and not in the default build.** The `saml` feature is
> excluded from `full` (and therefore from the published image): the ACS handler
> has never been verified against a real identity provider and carries a known
> request-ID validation defect, so at present no SAML login can succeed. The
> provider type is still listed in the admin UI, marked experimental, for builds
> that enable the feature. Use OIDC, which is complete and tested.


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

Any IdP exposing a `.well-known/openid-configuration` works with the generic OIDC type; enterprise IdPs can also connect via SAML 2.0 (upload the IdP certificate, set the SSO URL, and exchange SP metadata from `/api/auth/saml/<slug>/metadata`).

## OIDC resource-server mode (IdP access tokens)

The providers above sign people in through the browser. Resource-server mode
is a separate path, for a client that already holds an access token from your
identity provider: for example, a single-page app that signs its users in at
Keycloak or Entra ID and then calls this store's API directly. The client
sends the IdP's token as `Authorization: Bearer <token>`. The store checks the
signature against the IdP's published keys, then links the token's subject to
a local account, creating one if needed. The mode is configured only through
environment variables and stays off unless `OIDC_ISSUER` is set.

| Variable | Default | Meaning |
|---|---|---|
| `OIDC_ISSUER` | *(unset: mode off)* | The IdP's issuer URL, e.g. `https://idp.example.org/realms/example`. Must be `https`; plain `http` is accepted only for `localhost` and loopback addresses. If you give a cleartext issuer, startup logs an error and leaves the mode off, because a man in the middle could otherwise serve forged keys. A trailing `/` is removed. |
| `OIDC_AUDIENCE` | *(unset)* | The `aud` value a token must carry for this store. **Required.** If `OIDC_ISSUER` is set without it, startup logs a warning and every IdP token is refused. |
| `OIDC_DEFAULT_ROLE` | `user` | The system role an account created from a token gets when no claim maps to a role: `user` or `guest`. The value is **capped at `user`**, because it applies to every account the IdP knows: `admin` or `super_admin` logs an error at startup and `user` is used instead. Grant admin per account, through `OIDC_ROLE_CLAIM_MAP` or the UI. An unknown value means `user`. It is read only once (see [changing settings later](#changing-settings-later)). |
| `OIDC_ROLE_CLAIMS` | `roles,realm_access.roles,groups` | Comma-separated claim names to check for role mapping. A dotted name reaches into a nested object (`realm_access.roles` is Keycloak's shape). String and string-array values are read. |
| `OIDC_ROLE_CLAIM_MAP` | *(unset: no mapping)* | A JSON object from claim value to role, e.g. `{"idp-admins": "admin", "idp-editors": "publisher", "staff": "user"}`. Invalid JSON is ignored without a warning, which disables mapping. |
| `OIDC_GROUPS_CLAIM` | `groups` | The claim holding group names. Its values are matched against the role map and used for organisation membership. |
| `OIDC_ORG_GROUP_PREFIX` | `org:` | The prefix that marks a group as an organisation membership: `org:example-gis` makes the account a member of the organisation whose slug is `example-gis`. |
| `OIDC_TOKEN_POLICY` | `session` | What an IdP token may do: `session` reads and writes but cannot create API tokens; `scoped` writes only when the token's `scope` or `scp` claim grants it; `full` also creates API tokens. Unknown values mean `session`. See [what an IdP token may do](#what-an-idp-token-may-do). |
| `OIDC_WRITE_SCOPES` | *(unset)* | Under `scoped`: extra scope values, comma- or space-separated, that count as a write grant besides `write` and `admin`, e.g. `ots.write` or `api://open-triplestore/write`. |
| `ACCEPT_LEGACY_TOKENS` | `true` | Keep accepting this store's own session tokens and `ots_` API tokens. `false` or `0` refuses them; see [below](#turning-off-the-stores-own-tokens). |

The Docker Compose file passes all ten through from `.env`.

### What a token must look like

- It is signed with an asymmetric algorithm (RS…, PS…, ES…, EdDSA). HMAC
  (`HS256` and the other HS variants) is refused.
- Its header has a `kid` that names a key in the IdP's key set. The store reads
  `jwks_uri` from `<OIDC_ISSUER>/.well-known/openid-configuration`
  (`jwks_uri` must be `https` too) and caches the keys for an hour. An unknown
  `kid` triggers an immediate re-fetch, so key rotation needs no restart.
- `iss` equals `OIDC_ISSUER`, with or without one trailing slash:
  `https://tenant.example` and `https://tenant.example/` both match, and
  nothing else does.
- `aud` contains `OIDC_AUDIENCE`, `exp` is in the future, and `nbf` (if
  present) is in the past.
- `sub` is present. `email`, `email_verified`, `name` and
  `preferred_username` are read when present.

A bearer token is tried in this order: the store's own session or API token
(while legacy tokens are accepted), an access token from the store's own
[OIDC provider](oidc-provider.md), a [federation](federation.md) assertion
from a trusted peer, and only then the IdP. A token starting with `ots_` never
reaches the IdP check. A token that fails verification gets
`401 Invalid or expired token`.

### Accounts

The store keys each IdP account on its subject (`sub`), stored under a
provider entry named *Environment OIDC* (slug `env-oidc`) that is created at
startup. The first request with a new subject resolves it as follows:

1. If the token's `email` belongs to an existing local account **and** the
   token says `email_verified: true`, the subject is linked to that account.
2. If the email exists but is not asserted as verified, the request is
   refused (`401`). This prevents account takeover through an IdP that lets
   anyone claim any address.
3. Otherwise a new account is created. Its username comes from `name`,
   `preferred_username`, `email` or `sub` (made unique). It gets the token's
   email, or a placeholder `…@oauth.local` address when the token has none,
   and no usable password, so its owner signs in only through the IdP.

Every later request with the same subject reaches the same account. A
deactivated account is refused.

### Roles, publish permission and organisations

On **every** request, the values of the `OIDC_ROLE_CLAIMS` claims and of the
`OIDC_GROUPS_CLAIM` claim are looked up in `OIDC_ROLE_CLAIM_MAP`:

- The highest mapped role wins. `super_admin` is capped at `admin`, so a token
  can never make an account a super admin through the map.
- When a value maps to a role, that role is written to the account, even if
  it is lower than the account's current role. The IdP is authoritative: a
  role an admin set in the UI is overwritten on the next request. When nothing
  maps, an existing account keeps its role, and a new account gets
  `OIDC_DEFAULT_ROLE`.
- The special value `"publisher"` grants the
  [publish permission](administration.md#the-can_publish-capability). It is
  never taken away just because the claim is absent.

Each group value that starts with `OIDC_ORG_GROUP_PREFIX` names an
organisation slug. If that organisation exists, the account is added to it as
a `member`. Organisations are never created this way, and memberships are only
added: one stays after the group disappears from the token.

### What an IdP token may do

An IdP token is a delegation: any client that holds a user's access token for
this store's audience can present it. `OIDC_TOKEN_POLICY` decides how far it
reaches:

| Policy | Read | Write | Create API tokens |
|---|---|---|---|
| `session` *(default)* | yes | yes | no |
| `scoped` | yes | only if the token's scopes grant it (see below) | no |
| `full` (`legacy` is accepted too) | yes | yes | yes |

- Under the default, an IdP token reads and writes wherever the account's
  dataset and graph permissions allow, whatever scopes it carries, like an
  interactive session.
- No policy except `full` lets an IdP token create a long-lived API token:
  `POST /api/auth/tokens` answers `403`. This keeps a short-lived delegation
  from becoming a permanent `ots_` token for the account. To create one, sign
  in to the web UI. Before this setting existed, IdP tokens could always
  create API tokens; `full` restores that.
- Under `scoped`, the scopes are read from the `scope` claim (a space-separated
  string, as Keycloak issues it) and the `scp` claim (a string, as Entra ID
  issues it, or an array, as Okta does). `write` and `admin` grant writing, as
  does any value listed in `OIDC_WRITE_SCOPES`. A token with only
  `openid profile email` reads.
- "Write" means every `POST`, `PUT`, `PATCH` and `DELETE` request, including
  SPARQL Update. A refused write gets `403 This API token does not have write
  scope`. A SPARQL *query* sent by `POST` is a read and still works.
- **Admin and super-admin accounts are exempt from the write check** under
  every policy.
- An unknown value logs a warning and falls back to `session`, never to `full`.

This setting is independent of `OTS_OIDC_SESSION_POLICY`, which governs only
the tokens this store issues as an
[OIDC provider](oidc-provider.md#what-a-provider-token-may-do). The two are
issued to different clients, so tightening one must not change the other.

Accounts with the `guest` role stay limited by `OTS_GUEST_CAPABILITIES`,
whichever way they authenticate.

### Turning off the store's own tokens

`ACCEPT_LEGACY_TOKENS=false` refuses every session token this store issues,
including the one the bundled web UI receives after a password, passkey or SSO
sign-in. Every `ots_` API token is refused too, with
`401 Legacy tokens are disabled`. What still works: IdP tokens, access tokens
from the store's own OIDC provider, and federation assertions. Use it only for
an API-only deployment whose clients all hold IdP tokens. The bundled web UI's
sign-in stops working.

### Changing settings later

The *Environment OIDC* entry is created once, the first time the server starts
with `OIDC_ISSUER` set. It records `OIDC_DEFAULT_ROLE` and has
auto-provisioning on. Changing `OIDC_DEFAULT_ROLE` afterwards does not update
the entry. Edit it under **Security & Access Control → Identity providers**
(or `PUT /api/admin/oauth/providers/:id`) instead. There you can also turn
auto-provisioning off, so that only accounts that already exist (linked by
verified email) can use IdP tokens. The entry's default role is capped at
`user` like `OIDC_DEFAULT_ROLE`: an `admin` value set there, or recorded by an
earlier version, creates `user` accounts. The entry's own role claim map and
its enabled switch do not affect this path; the environment variables above do.

The entry does not appear on the login page: it has no client ID, and the
login page lists only providers a browser sign-in can start from.

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
