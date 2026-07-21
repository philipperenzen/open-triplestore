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
