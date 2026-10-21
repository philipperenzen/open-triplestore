# Administration Guide

## Roles

| Role | Level | Can do |
|---|---|---|
| `user` | 0 | Access public/member datasets; manage own account |
| `admin` | 1 | Manage users below their level; create datasets for any owner |
| `super_admin` | 2 | Full control; promote users up to `admin`; cannot be modified by `admin` |

Role hierarchy is enforced server-side: you cannot create, promote, demote, or delete a user at or above your own level, and you cannot self-promote.

### The `can_publish` capability

`can_publish` is a per-user **capability**, separate from the role above. It allows a `user` to create, edit, upload, and publish model and vocabulary versions. `admin` and `super_admin` always have it implicitly. (It replaced a former `publisher` role.)

It can be granted by an admin via the user management API, or automatically during SSO sign-in: map an IdP group/claim value to the special grant `"publisher"` in a provider's `role_claim_map`, e.g.

```json
{ "team-admins": "admin", "team-publishers": "publisher", "staff": "user" }
```

The `"publisher"` grant only sets `can_publish`; it does not change the account role. SSO grants the capability non-destructively — it is set on matching sign-ins and never revoked just because a claim is absent. (Group/claim mapping applies on every SSO path: SAML, resource-server OIDC tokens, and the interactive OIDC sign-in, which reads the ID token's `groups` and `roles` claims.)

---

## First Login — Creating the Super Admin

The first account registered on a fresh instance is **automatically granted `super_admin`**. All subsequent registrations default to `user`.

### Via the UI

1. Open `http://localhost:7878/` (or your deployment URL) and click **Register**
2. Fill in username, email, and password (minimum 8 characters)
3. Submit — the account is instantly granted `super_admin`

### Via Docker (exec into running container)

```bash
# Register via the API
curl -s -X POST http://localhost:7878/api/auth/register \
  -H 'Content-Type: application/json' \
  -d '{"username":"alice","email":"alice@example.com","password":"changeme1"}'
```

The response includes `access_token` and `refresh_token`. Store the `access_token` for subsequent admin API calls.

---

## Promote an Existing User to super_admin

Use the `--promote-super-admin` flag. The process starts, promotes the user, prints a confirmation, and exits — the server does not start.

### Binary

```bash
./target/release/open-triplestore \
  --data-dir ./data \
  --promote-super-admin alice
# INFO  open_triplestore: Promoted user 'alice' to super_admin
```

### Docker

```bash
docker compose exec triplestore \
  open-triplestore \
  --data-dir /data \
  --promote-super-admin alice
```

Or run a one-shot container sharing the same named volume:

```bash
docker run --rm \
  -v triplestore_data:/data \
  open_triplestore-triplestore \
  --data-dir /data --promote-super-admin alice
```

---

## Admin API Reference

All endpoints below require a valid `Authorization: Bearer <token>` header from an `admin` or `super_admin` account. Get a token via `POST /api/auth/login`.

Replace `<token>` in examples with your access token, and `<user_id>` with the UUID returned by list/create calls.

### List users

```bash
# All users (paginated)
curl -H "Authorization: Bearer <token>" \
  'http://localhost:7878/api/admin/users?page=1&limit=50'

# Search by username/email
curl -H "Authorization: Bearer <token>" \
  'http://localhost:7878/api/admin/users?search=alice'
```

Response: `{ "users": [...], "total": N, "page": 1, "limit": 50 }`

### Get a single user

```bash
curl -H "Authorization: Bearer <token>" \
  http://localhost:7878/api/admin/users/<user_id>
```

### Create a user

```bash
curl -s -X POST http://localhost:7878/api/admin/users \
  -H "Authorization: Bearer <token>" \
  -H 'Content-Type: application/json' \
  -d '{"username":"bob","email":"bob@example.com","password":"secret123","role":"user"}'
```

Valid roles: `guest`, `user` (the default), `admin` and `super_admin` (only a super_admin may grant it). You cannot create a user with a higher role than your own.

### Change a user's role

```bash
# Promote to admin
curl -X PUT http://localhost:7878/api/admin/users/<user_id> \
  -H "Authorization: Bearer <token>" \
  -H 'Content-Type: application/json' \
  -d '{"role":"admin"}'

# Demote back to user
curl -X PUT http://localhost:7878/api/admin/users/<user_id> \
  -H "Authorization: Bearer <token>" \
  -H 'Content-Type: application/json' \
  -d '{"role":"user"}'
```

### Change a user's email

```bash
curl -X PUT http://localhost:7878/api/admin/users/<user_id> \
  -H "Authorization: Bearer <token>" \
  -H 'Content-Type: application/json' \
  -d '{"email":"newemail@example.com"}'
```

### Deactivate / reactivate a user

Deactivating immediately revokes all refresh tokens and API tokens for that user.

```bash
# Deactivate
curl -X PUT http://localhost:7878/api/admin/users/<user_id> \
  -H "Authorization: Bearer <token>" \
  -H 'Content-Type: application/json' \
  -d '{"is_active":false}'

# Reactivate
curl -X PUT http://localhost:7878/api/admin/users/<user_id> \
  -H "Authorization: Bearer <token>" \
  -H 'Content-Type: application/json' \
  -d '{"is_active":true}'
```

You cannot deactivate yourself, and you cannot deactivate users at or above your own role level.

### Reset a user's password

Forces the user to log in again (all refresh tokens are revoked).

```bash
curl -X POST http://localhost:7878/api/admin/users/<user_id>/reset-password \
  -H "Authorization: Bearer <token>" \
  -H 'Content-Type: application/json' \
  -d '{"new_password":"newpassword99"}'
```

Password must be at least 8 characters.

### Delete (permanently remove) a user

> **Note:** `DELETE` hard-deletes the account record. Use `is_active: false` (above) to suspend without losing history. You cannot delete yourself.

```bash
curl -X DELETE http://localhost:7878/api/admin/users/<user_id> \
  -H "Authorization: Bearer <token>"
```

---

## User feedback

Signed-in users send bug reports, feature requests and questions from the
**Feedback** button in the sidebar footer, or from the buttons at the end of
every documentation page. Reports go to the admins of this instance, not to the
upstream project, and land in **Admin → Feedback** (`/admin/feedback`).

For each report the inbox shows the reporter, the in-app page it was sent from
and, if the reporter allowed it, their browser. An admin can:

- set the status: **Open**, **In progress**, **Resolved** or **Closed**;
- write a **reply to the reporter**, which they read next to their report under
  **My reports** in the dialog;
- keep an **internal note**, which only admins see;
- delete the report.

A user may send at most 20 reports in 24 hours, and submissions are rate-limited
per client address. Read-only API tokens cannot send reports. Deleting a user
deletes their reports too. The dialog tells users not to report security
vulnerabilities through it.

```bash
# The inbox, narrowed to open bug reports
curl "http://localhost:7878/api/admin/feedback?status=open&kind=bug" \
  -H "Authorization: Bearer <token>"

# Reply and mark as resolved
curl -X PATCH http://localhost:7878/api/admin/feedback/<id> \
  -H "Authorization: Bearer <token>" -H "Content-Type: application/json" \
  -d '{"status": "resolved", "admin_response": "Fixed in 0.7.1 — thanks!"}'
```

---

## Self-service (any authenticated user)

### Change your own password

```bash
curl -X POST http://localhost:7878/api/auth/change-password \
  -H "Authorization: Bearer <token>" \
  -H 'Content-Type: application/json' \
  -d '{"current_password":"old","new_password":"newpassword99"}'
```

### Manage API tokens

API tokens are long-lived alternatives to JWT bearer tokens, useful for scripts and CI pipelines.

```bash
# Create a read-only token
curl -X POST http://localhost:7878/api/auth/tokens \
  -H "Authorization: Bearer <token>" \
  -H 'Content-Type: application/json' \
  -d '{"name":"ci-pipeline","scopes":["read"]}'
# Response includes "token" — store it, it is only shown once.

# List your tokens
curl -H "Authorization: Bearer <token>" \
  http://localhost:7878/api/auth/tokens

# Revoke a token
curl -X DELETE http://localhost:7878/api/auth/tokens/<token_id> \
  -H "Authorization: Bearer <token>"
```

Valid scopes: `read`, `write`, `admin`.

---

## Dataset Configuration

### SHACL on Write

To enable automatic SHACL validation for a dataset, set `shacl_on_write` to `true` and ensure the dataset has a shapes graph configured.

```bash
# Enable validation on write
curl -X PUT http://localhost:7878/api/datasets/<dataset_id> \
  -H "Authorization: Bearer <token>" \
  -H 'Content-Type: application/json' \
  -d '{"shacl_on_write": true}'

# Upload a shapes graph (Turtle)
curl -X PUT http://localhost:7878/api/datasets/<dataset_id>/shapes \
  -H "Authorization: Bearer <token>" \
  -H 'Content-Type: application/turtle' \
  --data-binary @shapes.ttl
```

Once enabled, any `PUT` or `POST` to `/store?graph=<iri>` for a graph belonging to this dataset will be validated. Invalid data returns `422 Unprocessable Entity` with the full SHACL report — the store is not modified.

See [shacl.md](shacl.md) for the full SHACL guide.

### RML Mappings

RML mapping documents are stored per dataset in the named graph `urn:dataset:<id>:rml-mappings`. Execution output goes to `urn:dataset:<id>:rml-output` by default.

```bash
# Upload mapping
curl -X PUT http://localhost:7878/api/datasets/<dataset_id>/mappings \
  -H "Authorization: Bearer <token>" \
  -H 'Content-Type: text/turtle' \
  --data-binary @mapping.ttl

# Execute with source files
curl -X POST http://localhost:7878/api/datasets/<dataset_id>/mappings/execute \
  -H "Authorization: Bearer <token>" \
  -F 'data.csv=@data.csv'
```

See [rml.md](rml.md) for the full RML guide.

---

## Environment Variables

### How settings are read

Every setting is an environment variable named `OTS_…`; some also have a
command-line flag (`--help` lists them, each with its variable), and a flag
wins over the variable. A switch is named for what it turns on and ends in
`_ENABLED` (`OTS_REGISTRATION_ENABLED`, `OTS_RATE_LIMIT_ENABLED`). The server
reads them all once at startup, validates them together, and **refuses to
start on any problem, listing every one**:

- **Booleans** take `true`/`false`, `1`/`0`, `yes`/`no` or `on`/`off`, in any
  case. Anything else is an error, never a default.
- **Numbers** must be whole numbers within the setting's range. A value that
  does not parse, or is out of range, is an error; nothing is clamped or
  replaced by the default.
- **Choices** (`OTS_ENV`, `OTS_DCAT_PROFILE`, `OTS_SMTP_TLS`, the token policies, …)
  take only the listed words, in any case. There are no aliases.
- **Units** are in the name: a duration ends in `_MS`, `_SECS` or `_DAYS`, a
  size in `_BYTES` (which also takes `KiB`, `MiB` or `GiB`, e.g. `512MiB`).
- **Empty means unset.** `NAME=` (what compose passes for `${NAME:-}`) gives
  the default.
- **Renamed or removed names stop the start** with the name to use instead
  (for example `SECURE_COOKIES` → `OTS_SECURE_COOKIES_ENABLED`, or
  `RATE_LIMIT_DISABLED` → `OTS_RATE_LIMIT_ENABLED` with the opposite sense),
  so an old `.env` can never be silently ignored. The full list is in the
  changelog. An `OTS_*` variable no setting reads — neither the server's
  own nor one a compiled-in plugin declares ([plugins.md](plugins.md#plugin-settings)) —
  is logged as a warning (a typo, or a secret reference's target).
- **Secrets** (`OTS_JWT_SECRET`, `OTS_LLM_API_KEY`, `OTS_SMTP_PASSWORD`, …) take a
  reference (`env:NAME`, `file:/path`, `vault:…`); a malformed reference is an
  error.

The storage directory, port and bind address are settings too:
`OTS_DATA_DIR` (`./data`, flag `-d`), `OTS_PORT` (`7878`, `-p`) and
`OTS_BIND` (`0.0.0.0`, `-b`).

### Runtime settings

Some settings are policy rather than plumbing, and an administrator can change
them without a restart, under **Admin → Settings** (`/admin/settings`) or with
`PUT /api/admin/settings/{name}`:

| Setting | What it decides |
|---|---|
| `OTS_REGISTRATION_ENABLED` | Whether anyone may register (the guest-registration switch on the Security page opens a guest-only path when it is off) |
| `OTS_GUEST_CAPABILITIES` | What a guest may do beyond reading |
| `OTS_EMBED_FRAME_ANCESTORS` | Who may frame the `/embed/*` pages |
| `OTS_LLM_RATE_LIMIT_PER_MIN`, `OTS_LLM_RATE_LIMIT_ANON_PER_MIN`, `OTS_LLM_ANONYMOUS_ENABLED` | The AI request budgets |
| `OTS_OIDC_PROVIDER_TOKEN_POLICY`, `OTS_OIDC_PROVIDER_WRITE_SCOPES`, `OTS_OIDC_IDP_TOKEN_POLICY`, `OTS_OIDC_IDP_WRITE_SCOPES` | What delegated OIDC tokens may do |

The environment variable stays the default: an override is stored (in the
identity database's `app_settings` table, so followers see it too) only when an
administrator sets one, and **Reset** removes it, so the variable applies
again. An override is validated exactly as the variable would be, takes effect
on the next request, and is audit-logged (`setting_changed`). The
**Effective configuration** table on the same page lists every setting with its
value and source; an overridden one shows source `admin`.

### Production profile

`OTS_ENV=production` refuses raw secret values (every secret setting must be a
`env:`, `file:` or `vault:` reference) and changes these defaults:

| Setting | Development | Production |
|---|---|---|
| `OTS_SECURE_COOKIES_ENABLED` | on when `OTS_BASE_URL` is https | on; an explicit `false` **stops the start** |
| `OTS_REGISTRATION_ENABLED` | `true` | `false` (the first account can still register, to bootstrap) |
| `OTS_LDP_ROOT_ACL` | `open` | `owners` |
| `OTS_EMBED_FRAME_ANCESTORS` | `*` | `'none'` |
| `OTS_SEED_STANDARDS_DEMO_ENABLED` | `true` | `false` (no demo datasets, no demo IFC/STL downloads) |
| `OTS_OIDC_PROVIDER_TOKEN_POLICY` | `session` | `scoped` |
| `OTS_STORE_AUTO_RECOVER_ENABLED` | `true` | `false`: a corrupt store fails the start |

Each default can be overridden by setting the variable. Two settings are
refused outright in production: `OTS_RATE_LIMIT_ENABLED=false`, and `*` in
`OTS_CORS_ORIGINS` (it reflects any origin with credentials). At startup the
server logs the profile and every setting that is not at its built-in
default, with its source; secrets are redacted.

Why a corrupt store fails the start in production: the automatic rebuild
quarantines the store and restores the newest *unencrypted* backup — up to
one backup interval old, with the identity database untouched — and starts
empty when there is none, so the service can come back empty and healthy. A
failed start is visible to the orchestrator and the operator, keeps the data
directory as it was, and serves nothing wrong. (Starting read-only instead
would serve nothing either: a corrupt store cannot be opened.) Restore a
backup with `--restore`, or set `OTS_STORE_AUTO_RECOVER_ENABLED=true` to accept the
rebuild.

`GET /api/admin/config` (admin only) answers every setting, the compiled-in
plugins' included, with its effective value, default, kind, documentation and
source — `cli`, `env`, `file` (a
secret given as a `file:` reference), `profile` (the production profile's
default) or `default`. Secret values are shown as `[redacted]`; a secret given
as a reference shows the reference.


| Variable | Default | Description |
|---|---|---|
| `OTS_JWT_SECRET` | *(random, saved to `data/jwt_secret`)* | JWT signing secret, as a raw value or a secret reference (`env:NAME`, `file:/path`, `vault:…`; see [sources.md](sources.md#credentials-are-references-never-values)). Under `OTS_ENV=production` a raw value is **refused at startup**; in development it is accepted with a one-time warning. Set explicitly in production so tokens survive restarts. The on-disk file is written `0600`. The server **refuses to start** if this is a well-known default/placeholder (e.g. `change-me-in-production`), naming the fix — leave it unset to auto-generate a strong one. |
| `OTS_AUTH_DB_PATH` | `<data-dir>/auth.db` | Path to the SQLite identity database |
| `OTS_ACCESS_TOKEN_EXPIRY_SECS` | `1800` | Access token lifetime, in seconds (at least 60) |
| `OTS_REFRESH_TOKEN_EXPIRY_DAYS` | `30` | Refresh token lifetime |
| `OTS_OIDC_ISSUER`, `OTS_OIDC_AUDIENCE` | *(unset: off)* | Turn on [OIDC resource-server mode](auth.md#oidc-resource-server-mode-idp-access-tokens): access tokens issued by this external IdP are accepted as bearer tokens and linked to (or create) local accounts. The issuer must be `https` (loopback `http` for development); the audience is required, and without it every IdP token is refused. |
| `OTS_OIDC_DEFAULT_ROLE` | `user` | Role for accounts created from an IdP token when no claim maps to one: `user` or `guest`. Any other value (`admin`, `super_admin`) **stops the start**, because the default applies to every IdP account; grant admin through `OTS_OIDC_ROLE_CLAIM_MAP` or the UI. Read only when the `env-oidc` provider entry is first created; edit that entry later instead (the same cap applies there). |
| `OTS_OIDC_IDP_TOKEN_POLICY` | `session` | What an [IdP token](auth.md#what-an-idp-token-may-do) may do: `session` reads and writes but cannot create API tokens; `scoped` writes only when the token's `scope`/`scp` claim grants it; `full` also creates API tokens (the behaviour before this setting). Admin accounts' tokens always write. Any other value stops the start. Independent of `OTS_OIDC_PROVIDER_TOKEN_POLICY`. Not to be confused with `OTS_OIDC_PROVIDER_TOKEN_POLICY` below, which governs the tokens this store issues as an OIDC provider. |
| `OTS_OIDC_IDP_WRITE_SCOPES` | *(unset)* | Under `OTS_OIDC_IDP_TOKEN_POLICY=scoped`: extra IdP scope values, comma- or space-separated, that count as a write grant besides `write` and `admin`. |
| `OTS_OIDC_ROLE_CLAIMS`, `OTS_OIDC_ROLE_CLAIM_MAP`, `OTS_OIDC_GROUPS_CLAIM`, `OTS_OIDC_ORG_GROUP_PREFIX` | `roles,realm_access.roles,groups`, *(none)*, `groups`, `org:` | Which token claims are mapped, through the JSON claim-value → role map, to a role (capped at `admin`) or to publish permission (`"publisher"`), and which group values (`org:<slug>`) add organisation memberships. A mapped role is rewritten on every request, so the IdP is authoritative. See [auth.md](auth.md#roles-publish-permission-and-organisations). |
| `OTS_SESSION_AND_API_TOKENS_ENABLED` | `true` | `false` refuses this store's own session tokens (the bundled web UI's sign-in included) and every `ots_` API token, leaving only IdP tokens, provider tokens and federation assertions. For API-only deployments behind an IdP. |
| `OTS_OIDC_PROVIDER_TOKEN_POLICY` | `session` | What an access token this store issues as an [OIDC provider](oidc-provider.md#what-a-provider-token-may-do) may do: `session` reads and writes but cannot create API tokens; `scoped` writes only with a write scope, which the provider cannot issue today, so its tokens are read-only for non-admins; `full` also creates API tokens. Admin accounts' tokens always write. Any other value stops the start. |
| `OTS_OIDC_PROVIDER_WRITE_SCOPES` | *(unset)* | Under `scoped`: extra scope values, comma- or space-separated, that count as a write grant besides `write` and `admin`. |
| `OTS_SECURE_COOKIES_ENABLED` | on when `OTS_BASE_URL` is https or `OTS_ENV=production`, else off | Issue auth cookies with the `Secure` attribute (HTTPS only). Production refuses `false`; set `false` only for plain-HTTP local development behind an https `OTS_BASE_URL`. |
| `OTS_FRONTEND_ENABLED` | `true` | Serve the bundled web UI (frontend SPA) at `/`. Set `false` for a headless, API-only server — SPARQL, Graph Store and REST endpoints are unaffected. Also the `--serve-frontend` CLI flag. |
| `OTS_SEED_STANDARDS_DEMO_ENABLED` | `true` (`false` in production) | On first start, seed the public *Open Triplestore* demo organisation: one dataset per standards category with saved queries, plus a 3D demo that downloads openly licensed IFC and STL sample files from public URLs (`OTS_OFFLINE=true` skips those downloads; see [operations.md](operations.md)). `false` skips the seed. It runs once: the seed is skipped while the organisation exists, and turning it off later does not remove it. |
| `OTS_CORS_ORIGINS` | *(empty — same-origin only)* | Comma-separated allowed origins, e.g. `https://app.example.com,https://www.example.com` |
| `OTS_EMBED_FRAME_ANCESTORS` | `*` (`'none'` in production) | CSP `frame-ancestors` source list for the `/embed/*` pages, used verbatim: `https://intranet.example https://*.example.org` restricts embedding to those sites, and `'none'` disables it. Other pages are never frameable from another site. See [embedding.md](embedding.md#restricting-who-may-embed). |
| `OTS_TRUSTED_PROXY_CIDRS` | *(empty — direct TCP IP)* | Comma-separated CIDRs of reverse proxies whose `X-Forwarded-For` / `X-Real-IP` is believed, e.g. `10.0.0.0/8,172.16.0.0/12`. The client IP derived this way keys the per-IP rate limits and the guest AI budget and is the IP recorded in the audit log. From any other peer the headers are ignored and the TCP peer address is the client. Leave empty when not behind a proxy; behind one, set it, or every client appears as the proxy. |
| `OTS_DL_BACKEND` | *(unset)* | Backend of the `owl2-dl` regime: `sidecar` (the bundled OWL API + HermiT reasoner, `docker compose --profile reasoner`), `konclude`, or `native` (OWL 2 RL + DL-syntax rules, sound but not complete). Unset: `owl2-dl` answers 503 (see [owl2-dl.md](owl2-dl.md)). |
| `OTS_KONCLUDE_BIN` | `Konclude` | Konclude binary for `OTS_DL_BACKEND=konclude` (not shipped in the image; LGPL-3.0). |
| `OTS_REASONER_URL` | *(unset; `http://reasoner:8090` under Docker Compose)* | Base URL of the reasoner sidecar for `OTS_DL_BACKEND=sidecar`. |
| `OTS_REASONER_TOKEN` | *(unset)* | Bearer token sent to the reasoner sidecar; the bundled sidecar reads the same variable and refuses to start without it. Its own settings are in [owl2-dl.md](owl2-dl.md#running-the-bundled-sidecar). The server reads it as a secret reference (`env:NAME`, `file:/path`, `vault:…`) like `OTS_LLM_API_KEY`: a raw value works with a deprecation warning, and `OTS_ENV=production` refuses it. |
| `OTS_REASONER_TIMEOUT_SECS` | `300` | Time limit of one DL backend call; past it the answer is unknown (504). |
| `OTS_REASONER_MAX_TRIPLES` | `1000000` | Most triples handed to an external DL backend; more is a 413. |
| `OTS_DL_DEBOUNCE_MS` | `2000` | Quiet period after the last write before an `owl2-dl` dataset's background run starts. |
| `OTS_DCAT_PROFILE` | `dcat` | Catalogue application profile: `dcat`, `dcat-ap` or `dcat-ap-nl` (see [dcat.md](dcat.md)). Any other value stops the server at startup. |
| `OTS_CATALOG_TITLE` / `OTS_CATALOG_DESCRIPTION` | instance defaults | The `dcat:Catalog` metadata. |
| `OTS_CATALOG_PUBLISHER_URI` / `OTS_CATALOG_PUBLISHER_NAME` / `OTS_CATALOG_PUBLISHER_IDENTIFIER` | `<base>/publisher` | The catalogue's publishing agent. |
| `OTS_CATALOG_PUBLISHER_TYPE` | unset | The publisher's ADMS publisher type: a code (`LocalAuthority`, `NationalAuthority`, `Company`, …) or an IRI. An unknown code stops the server at startup. |
| `OTS_CATALOG_CONTACT_NAME` / `OTS_CATALOG_CONTACT_EMAIL` | unset | Contact point of the catalogue and its data services, and the fallback for datasets without one under a profile (DCAT-AP-NL requires a contact point on each). |
| `OTS_CATALOG_LANGUAGE` | `ENG` (`NLD` under dcat-ap-nl) | ISO 639-3 code for the catalogue language. |
| `OTS_VOID_PARTITION_LIMIT` | `100` | Class and property partitions listed per dataset in the catalogue's VoID (most frequent first). |
| `OTS_VOID_PARTITION_MAX_TRIPLES` | `5000000` | Datasets larger than this get VoID counts but no partitions (those take a grouping scan). |
| `OTS_CATALOG_LICENSE` | unset | Licence IRI for the catalogue and for distributions of datasets without one. |
| `OTS_REMOTE_AUTH` | unset | `assert` mints a signed identity assertion for allowlisted peers when acting for a user (see [federation.md](federation.md)). |
| `OTS_TRUSTED_ISSUERS` | unset | Comma-separated peer base URLs whose identity assertions are accepted (needs `OTS_BASE_URL`). |
| `OTS_SPARQL_FUNCTION_GRAPHS` | unset | Comma-separated graphs whose `sh:SPARQLFunction`s every query sees, `/sparql` included. Only `urn:system:functions` and graphs under `urn:system:functions:` are accepted (no dataset can hold them, so only an admin can write them); any other entry stops the start. A function at a reserved IRI (`xsd:` casts, GeoSPARQL and the other built-ins) is skipped with a warning. Without it, a function is callable only in runs of the shapes graph that declares it (see [shacl.md](shacl.md#sparql-functions-shsparqlfunction-shacl-af-5)). |
| `OTS_MAX_UPLOAD_BYTES` | 512 MiB (Graph Store, container import/validate), 1 GiB (bulk import) | Request-body limit in bytes (`512MiB`, `2GiB` are accepted) for RDF uploads and container archives; bodies are buffered and parsed before they replace anything, so this also bounds memory per request. Larger datasets: append in chunks or use the bulk import. |
| `OTS_SERVICE_ALLOWLIST` | *(unset)* | URLs a SPARQL `SERVICE` clause may contact, comma-separated. Any `/sparql` caller can name a `SERVICE` endpoint, so keep this to endpoints every query user may reach. Unset: `SERVICE` errors. |
| `OTS_LDES_ALLOWLIST` | *(unset)* | URLs an admin's [LDES client sync](ldes.md) may read. |
| `OTS_SOURCES_ALLOWLIST` | *(unset)* | Endpoints [SQL and SPARQL datasources](sources.md) may reach (also `SERVICE <urn:source:…>`, which uses a registered source's endpoint). |
| `OTS_JSONLD_CONTEXT_ALLOWLIST` | *(unset)* | Remote JSON-LD `@context` URLs an upload may load ([formats.md](formats.md#json-ld-remote-contexts)); the bundled W3C contexts need no entry. |
| `OTS_OFFLINE` | `false` (`true` in production) | No download at startup: the LOV corpus (`OTS_VOCAB_CORPUS_URL`) and the demo IFC and STL models are not fetched. See [outbound requests](#outbound-requests). |
| `OTS_SEED_IFC_URL` | *(the bundled sample)* | Downloads the headline IFC demo model from this URL instead (at first boot, unless `OTS_OFFLINE`). |
| `OTS_ICDD_ONTOLOGY_DIR` | *(unset)* | Directory holding ISO's `Container.rdf` and `Linkset.rdf` (download them from <https://standards.iso.org/iso/21597/-1/ed-1/en/>; this project does not ship them). ICDD exports embed both unchanged in `Ontology resources/`; without them an export carries the `ontology-resource-missing` warning and is not ISO 21597-1 Part 1-conformant. See [containers.md](containers.md). |
| `OTS_JSONLD_CONTEXT_MAX_BYTES` | `1048576` | Largest remote JSON-LD context the document loader reads; a bigger one fails the parse. |
| `OTS_REMOTE_TIMEOUT_SECS` | `10` | Timeout per outbound request. |
| `OTS_REMOTE_MAX_BYTES` | `67108864` (64 MiB) | Response-body limit per outbound request: `SERVICE` results, virtual-datasource queries and snapshots, LDES pages. The body is read as a stream and the request fails as soon as it passes the limit; a `SERVICE` call then fails as described under `OTS_SERVICE_MAX_ROWS`. |
| `OTS_LDES_RETRIES` | `4` | LDES sync: how many times a page fetch is retried after `408`, `425`, `429`, `500`, `502`, `503` or `504`, with exponential back-off and jitter or the wait `Retry-After` asks for. `0` turns retrying off. |
| `OTS_LDES_MAX_RETRY_WAIT_SECS` | `60` | LDES sync: the longest single wait before a retry; a `Retry-After` asking for longer fails the sync instead. |
| `OTS_SERVICE_MAX_ROWS` | `10000` | Row cap per `SERVICE` call. A remote result with more rows is a failed invocation, never a truncated one: the query errors with a message naming this variable, or under `SERVICE SILENT` the clause yields the single empty solution. See [federation.md](federation.md#service-what-a-call-returns). |
| `OTS_VALIDATION_API_URL` | *(unset)* | Base URL of an external SHACL validation service used by `POST /api/datasets/validate-and-commit`, which calls `<url>/validate`. The request carries the data and shapes to validate **and the caller's `Authorization` header**, so the service receives the user's bearer token (session, API or OIDC). Point it only at a service you trust with that. The URL is not checked against any outbound allowlist and need not be `https`. Unset: the endpoint answers `400`. |
| `OTS_SERVICE_MAX_ENDPOINTS` | `16` | Distinct `SERVICE` endpoints one query may contact. The call that would contact one more fails like an over-cap result (an error naming this variable, or the empty solution under `SERVICE SILENT`). Matters mostly for `SERVICE ?var`, whose endpoints come from the data. See [federation.md](federation.md#per-query-limits). |
| `OTS_SERVICE_MAX_CALLS` | `64` | Remote requests one query may make through `SERVICE`. An answer for the same endpoint and pattern is fetched once and reused, and reuse does not count. Over the cap the call fails as above. |
| `OTS_SERVICE_DEADLINE_SECS` | *(the SPARQL query timeout, `OTS_SPARQL_QUERY_TIMEOUT_SECS`: `30`)* | How long after its start a query may still make `SERVICE` requests. A request is cut short at the deadline and the query fails with an error naming this variable, also under `SERVICE SILENT`. |
| `OTS_ENDPOINT_ACL_ENABLED` | `true` | Enforce endpoint ACL rules. Set to `false` to disable enforcement entirely — an escape hatch for a misfiring rule (see [security.md](security.md#endpoint-acl)), not a normal setting. |
| `OTS_RATE_LIMIT_ENABLED` | `true` | Set to `false` to switch off per-IP rate limiting (auth, SPARQL, import and map/3D viewer quotas). For trusted/internal deployments and the test/CI harness only. **Refused under `OTS_ENV=production`.** |
| `OTS_TILES3D_MAX_FEATURES` | `10000` | Most features one 3D Tiles GLB carries. The tileset is a single tile until tiling exists; past the cap it serves the first features in IRI order and flags the cut (`asset.extras.truncated`, `X-Tiles3d-Truncated`). See [geo-3d-platform.md](geo-3d-platform.md#4-3d-tiles--the-binding-contract). |
| `RUST_LOG` | *(unset: the `--log-level` flag, `info`)* | Log filter in `tracing` `EnvFilter` syntax: a level (`debug`), or per-module directives such as `info,open_triplestore::shacl=debug`. Overrides `--log-level` when set; Docker Compose sets `info`. |
| `OTS_BASE_URL` | `http://localhost:7878` | Base URL used to mint linked-data IRIs (no trailing slash). Behind a reverse proxy that serves the instance under a path, include the path (`https://example.org/ots`); see [Serving under a path prefix](operations.md#serving-under-a-path-prefix). |
| `OTS_SPARQL_QUERY_TIMEOUT_SECS` | `30` | Per-query/update execution timeout in seconds |
| `OTS_WRITE_TIMEOUT_SECS` | `120` | Execution timeout for Graph Store `PUT`/`POST`/`DELETE`, data-model `PATCH`/`DELETE` and dataset `DELETE`. An elapsed write answers `503`, but the write itself is not cancelled and may still complete, so re-read before retrying. SPARQL Update runs under `OTS_SPARQL_QUERY_TIMEOUT_SECS`; bulk import (`/api/import/bulk`) has no timeout. Also the `--write-timeout-secs` CLI flag. |
| `OTS_GEOSPARQL_QUERY_REWRITE_ENABLED` | `on` | The GeoSPARQL Query Rewrite Extension: a triple pattern with a topological relation predicate (`geo:sfWithin`, `geo:ehMeet`, `geo:rcc8ntpp`, …) also matches where the geometries imply the relation, in queries and in update `WHERE` clauses. `off`, `0`, `false` or `no` turns it off, so those patterns match asserted triples only (see [geosparql.md](geosparql.md#query-rewrite)). |
| `OTS_CHANGE_CAPTURE_ENABLED` | `off` | `on` records every write in the per-quad change log (`<data-dir>/changes/changes.db`): one row per graph per write with the net delta, exact counts or an honest `unknown`, a sequence number in commit order, and a cursor per consumer. It is what a replication follower tails and what the dataset history and audits read — **turn it on when something reads it**. It is off by default because it is not free and the cost is uneven: a ground `INSERT DATA`/`DELETE DATA` pays 4–13 %, but an `INSERT … WHERE` pays **×2.5** and a `DELETE … WHERE` **×3–4**. A `WHERE` update names its target by pattern, so the only way to know what it changed is to read the target graph before the update, read it again through the transaction, and subtract — a cost proportional to the *graph*, not to the size of the change, so a small `DELETE WHERE` against a large graph is the worst case. A replication leader or cluster member records regardless (its followers tail this log); a follower does not unless set to `on`, since its own log would be partial. Measured table and row format: [versioning.md](versioning.md#what-it-costs). |
| `OTS_CHANGE_CAPTURE_MAX_SCAN` | `250000` | Quads: a `WHERE` update's target graphs are scanned for a before-image only when their summed counts fit; above it the row says `unknown`. Lower it to bound what capture can cost a single write, at the price of less precise rows. |
| `OTS_CHANGE_CAPTURE_MAX_PAYLOAD` | `250000` | Quads a `full` row may carry; above it the row keeps exact counts only. |
| `OTS_CHECKPOINT_EVERY` | `100` | With change capture on, a version is a delta on the newest checkpoint; once this many deltas chain from one checkpoint, the next cut is a checkpoint (a copy) again, to keep chains short. See [versioning.md](versioning.md#delta-versions). |
| `OTS_CHECKPOINT_FRACTION` | `0.25` | …and once the changes since the checkpoint exceed this fraction of its size (and 1 000 quads), likewise. |
| `OTS_MATERIALISATION_MAX_QUADS` | `2000000` | A delta version's graphs are built on first read; a build that would hold more quads than this is refused with `507`. |
| `OTS_MATERIALISATION_TTL_SECS` | `3600` | A built delta unused this long is dropped again (rebuilt on the next read). |
| `OTS_APPLY_DELTA_TX_MAX_QUADS` | `50000` | Quads one transaction applies: a version restore whose inverse fits applies in one transaction across all its graphs (tier 1); a larger one copies the version back (tier 2). Builds split above it. |
| `OTS_LDES_BNODE_DEPTH` | `8` | With change capture on, LDES finds the entity a changed blank-node subtree belongs to by walking up at most this many levels. |
| `OTS_LDES_BNODE_SUBJECTS_MAX` | `10000` | …and republishes a graph whole instead when one write changed more blank-node subjects than this. |
| `OTS_STUDIO_INCREMENTAL_MAX` | `10000` | A write-triggered validation pipeline revalidates only the nodes a write touched, up to this many; above it, in full. |
| `OTS_CHANGE_RETENTION_DAYS` | `90` | Change-log rows older than this are swept — never above the lowest live cursor. |
| `OTS_CURSOR_TTL_DAYS` | `30` | A change-log cursor not updated for this long stops pinning retention and is dropped. |
| `OTS_REPLICATION_ROLE` | `none` | `leader` records every write in the change log and serves followers; `follower` keeps this store read-only and tails a leader. See [operations.md](operations.md#replication). |
| `OTS_REPLICATION_MODE` | `warm` | A follower's temperature — how often it catches up: `cold` (hourly), `warm` (every minute; `medium` is accepted), `hot` (long-polls the leader: the request for rows is held up to 25 s and answered the moment a row lands). |
| `OTS_REPLICATION_LEADER_URL`, `OTS_REPLICATION_TOKEN` | *(unset)* | The leader's base URL and an admin API token minted there — the token raw or as a secret reference (`env:`/`file:`/`vault:`); under `OTS_ENV=production` a raw token is refused and the follower runs without one. A follower without a leader URL reports the omission in its status and applies nothing. |
| `OTS_REPLICATION_GRAPHS` | `all` | The graphs a follower applies: `all`, or a comma-separated list of graph IRIs (`default` for the default graph). |
| `OTS_REPLICATION_DATASETS` | *(unset)* | Instead of graphs: the leader's dataset ids, resolved to graphs through the leader's manifest at every catch-up. |
| `OTS_REPLICATION_NODE_ID` | `$HOSTNAME` | This follower's name — the cursor it keeps on the leader, which pins the leader's retention. |
| `OTS_REPLICATION_POLL_MS` | `500` | Paces a hot follower's retry after a failed catch-up (50–60000); the long-poll hold itself is 25 s. |
| `OTS_REPLICATION_INTERVAL_SECS` | *(temperature)* | A catch-up interval that replaces the temperature's. |
| `OTS_REPLICATION_SYNC_FOLLOWERS` | *(unset: asynchronous)* | On a leader: the follower node ids whose acknowledgement every write waits for. Set it and the leader is synchronous; see [operations.md](operations.md#synchronous-replication). |
| `OTS_REPLICATION_SYNC_REQUIRED` | `1` | How many of the named followers must have applied a write before it returns; `all` for every one. |
| `OTS_REPLICATION_IDENTITY_INTERVAL_SECS` | *(temperature, ≥ 5)* | On a follower: how often it checks the leader's manifest for a changed identity database and, when it changed, fetches and applies the snapshot in place. See [operations.md](operations.md#identity-database). |
| `OTS_REPLICATION_CLUSTER`, `OTS_REPLICATION_CLUSTER_ID`, `OTS_REPLICATION_CLUSTER_SECRET` | *(unset)* | With `OTS_REPLICATION_ROLE=cluster`: every member as `id=url,…`, this node's id, and the secret the members' Raft messages carry. Raft elects the leader; the rest follow hot and acknowledge a majority. See [operations.md](operations.md#consensus). |
| `OTS_REPLICATION_ELECTION_MS`, `OTS_REPLICATION_HEARTBEAT_MS` | `1500`, a fifth of it | The election timeout's lower bound (the upper is twice it) and the leader's heartbeat. |
| `OTS_REPLICATION_SYNC_TIMEOUT_MS` | `2000` | How long a write waits for them (50–60000). After it, the write returns degraded — `X-Replication-Ack: degraded` — and the leader recovers by itself when a follower catches up. |
| `OTS_TELEMETRY_TIMING_STRIDE` | `8` | One query in this many is timed and lands in the latency ring that `GET /api/admin/telemetry` reports percentiles from. The exit counts and the total are exact regardless. Raising it makes the query path cheaper and the percentiles coarser; `1` times every query. The default exists because reading the clock and locking the ring costs about 40 ns, which is 40 % of a cache-hit query. |
| `OTS_TELEMETRY_QUERY_RING`, `OTS_TELEMETRY_VALIDATION_RING` | `8192`, `1024` | How many recent queries and validation runs the telemetry rings keep for `GET /api/admin/telemetry`. Memory only, nothing is persisted; `0` or an invalid value means the default. |
| `OTS_COLUMNAR_QUERY_ENABLED` | `on` | The in-memory mirror's third copy: a term dictionary and sorted permutations of the quads with an evaluator of its own, consulted after the shards and before the full copy for the query shapes it implements exactly, declining the rest. `off` leaves the two engine copies. See [performance.md](performance.md#4-the-columnar-copy-opengraphcolumnar). |
| `OTS_QUERY_CACHE_ENABLED` | `on` | The query-result cache: a repeated SPARQL read is answered from an LRU, and every write invalidates it. `off`/`false`/`0`/`no` disables it, as the benchmarks do. See [performance.md](performance.md). |
| `OTS_QUERY_CACHE_ENTRIES` | `1024` | Maximum cached query results (`0` or invalid: the default). |
| `OTS_QUERY_CACHE_MAX_ROWS` | `10000` | Results with more rows than this are not cached. |
| `OTS_PARALLEL_QUERY_ENABLED` | `on` | The in-memory query mirror (subject-hash shards, a full copy and the columnar copy). `off` answers every query from the persistent store. |
| `OTS_PARALLEL_QUERY_MAX_TRIPLES` | *(derived from RAM)* | Store size above which the mirror stays off. The default is a quarter of the detected memory limit (the cgroup limit in a container) at about 1 KiB per triple — 1 M triples in 4 GiB — never below 100 k or above 24 M; 2 M where no limit can be detected. On Linux a build also waits until the process's resident memory plus the projected copies fit in 80 % of the limit; otherwise the mirror stays off until the next write. |
| `OTS_PARALLEL_QUERY_SHARDS` | *(CPU cores)* | Number of subject-hash shards, 1–16 (another value stops the start). |
| `OTS_PARALLEL_QUERY_REBUILD_QUIET_MS` | `500` | After a write, the mirror is rebuilt only once writes have been quiet this long; queries in the meantime are answered by the persistent store. `0` rebuilds eagerly. |
| `OTS_PARALLEL_QUERY_BACKGROUND_QUIET_MS` | `5000` | The same after a burst of writes (two or more since the mirror was last fresh) to a store over 50 k triples, whose rebuild runs in the background: a bulk load's chunks arrive with sub-second gaps, and a build started in one would run next to the next chunk. A build is also abandoned as soon as a write starts. |
| `OTS_SHACL_RUN_INDEX_MIN_PROBES` | `20000` | A SHACL validation run builds an in-memory index of the predicates its shapes traverse only when it will make at least this many lookups. |
| `OTS_SHACL_RUN_INDEX_MAX_QUADS` | *(derived from RAM)* | The most quads that run index may hold. The default is derived from the memory limit, 1 M where none can be detected, and is never below 250 k or above 8 M. Beyond the cap, lookups go to the store. |
| `OTS_S3_ENDPOINT` | *(unset — local filesystem)* | Endpoint URL of an S3-compatible store (the compose stack points it at its bundled Versity S3 Gateway). If unset, assets are stored in `<data-dir>/assets/` |
| `OTS_S3_BUCKET` | `triplestore-assets` | S3 bucket name |
| `OTS_S3_ACCESS_KEY` | | S3 access key |
| `OTS_S3_SECRET_KEY` | | S3 secret key (raw or a secret reference; raw is refused under `OTS_ENV=production`) |
| `OTS_REGISTRY_TOKEN` | *(unset)* | Bearer token for the cross-app service registry (`OTS_DISCOVERY_ENABLED`), raw or as a secret reference; raw is refused under `OTS_ENV=production`. |
| `OTS_S3_REGION` | `us-east-1` | S3 region |
| `OTS_BACKUP_DIR` | `data/backups` | Directory for scheduled backups |
| `OTS_BACKUP_RETENTION_COUNT` | `7` | Number of backups to retain |
| `OTS_BACKUP_SCHEDULE_SECS` | `86400` | Seconds between scheduled backups (at least 60) |
| `OTS_BACKUP_ENCRYPTION_ENABLED` | `false` | Encrypt backups with `age` X25519 (requires the `backup-encrypt` build feature) |
| `OTS_BACKUP_ENCRYPTION_KEY_PATH` | `<data-dir>/backup_key.age` | File holding the `age` X25519 **recipient** (public `age1…` line). Operator-supplied — the server refuses to start if `OTS_BACKUP_ENCRYPTION_ENABLED=true` and this is absent. |
| `OTS_BACKUP_DECRYPTION_IDENTITY_PATH` | *(unset)* | `age-keygen` identity file, used only to decrypt an encrypted backup on restore (`--restore`, or a restore staged from the admin page and applied at the next start). |
| `OTS_AUDIT_PSEUDONYMISE_AFTER_DAYS` | `365` | GDPR/AVG: pseudonymise audit rows older than this |
| `OTS_TEXT_SEARCH_DIR` | `<data-dir>/tantivy` | Tantivy full-text index directory (requires the `text-search` build feature) |
| `OTS_SMTP_HOST` / `SMTP_*` | *(unset — account email is written to the server log)* | Outbound account email (verification, password reset, reminders) — see [auth.md](auth.md#email-delivery-configuration); the compose stack bundles an optional Postfix relay (`--profile mail`) |
| `OTS_ALERT_WEBHOOK_URL` / `ALERT_SMTP_*` | *(unset — alerting off)* | Optional webhook / SMTP alerting (requires the `alerting` build feature); without `OTS_ALERT_SMTP_HOST` the SMTP channel uses the `SMTP_*` relay — see [Alerting](#alerting) |

### Outbound requests

Four allowlists govern the requests the server makes **on a user's behalf**,
one per purpose: `OTS_SERVICE_ALLOWLIST` (SPARQL `SERVICE`, which any query
user can trigger), `OTS_LDES_ALLOWLIST` (LDES client sync, admin),
`OTS_SOURCES_ALLOWLIST` (datasources, admin) and
`OTS_JSONLD_CONTEXT_ALLOWLIST` (remote JSON-LD contexts in uploads). They are
separate on purpose and none falls back to another: a host an admin
allowlists for LDES sync must not thereby become a `SERVICE` target for every
query user. Every list is empty by default.

Each entry is matched on its parsed origin, not as a string prefix: the
scheme, host (case-insensitive) and port must be equal, the URL may carry no
credentials, and the entry's path must be a prefix of the URL's path on
segment boundaries. So `https://sparql.example.org` (with or without a
trailing slash) admits every path on that origin and nothing else — not
`https://sparql.example.org.evil.net/`, not
`https://sparql.example.org@evil.net/`, not port 8443 — and `https://h/sparql`
admits `/sparql` and `/sparql/…` but not `/sparqlx`. An entry that is not an
absolute http(s) URL without credentials stops the start. Redirects are
followed (at most 10), each hop only if the same list covers it too; a hop to
another host or port drops the `Authorization` header.

Empty allowlists do **not** mean the server makes no outbound requests. It
also contacts what an operator configures directly, which no allowlist
covers: the startup downloads (the LOV corpus and the demo IFC and STL models,
all off with `OTS_OFFLINE`, which production turns on), the LLM gateway
(`OTS_LLM_GATEWAY_URL`), SMTP relays (`OTS_SMTP_HOST`, `OTS_ALERT_SMTP_HOST`) and the alert
webhook (`OTS_ALERT_WEBHOOK_URL`), the OIDC issuer's discovery and keys
(`OTS_OIDC_ISSUER`) and configured OAuth and SAML providers, Vault (`VAULT_ADDR`),
S3 (`OTS_S3_ENDPOINT`), ClamAV (`OTS_CLAMAV_ADDR`), the service registry
(`OTS_REGISTRY_URL` with `OTS_DISCOVERY_ENABLED`), the external validator
(`OTS_VALIDATION_API_URL`), the DL reasoner sidecar (`OTS_REASONER_URL`),
replication peers, and prefix.cc when `OTS_PREFIX_CC_FALLBACK_ENABLED` is on.

### Recommended production `.env`

```dotenv
OTS_ENV=production
# A reference that resolves inside the container (a mounted secret file):
OTS_JWT_SECRET=file:/run/secrets/jwt_secret
OTS_CORS_ORIGINS=https://www.example.com
OTS_BASE_URL=https://www.example.com
S3_GATEWAY_ACCESS_KEY=triplestore
S3_GATEWAY_SECRET_KEY=<strong password>

# Outbound account email via the bundled Postfix relay (full options: .env.example)
COMPOSE_PROFILES=mail
OTS_SMTP_HOST=mail
OTS_SMTP_TLS=none
OTS_SMTP_FROM=Open Triplestore <no-reply@example.com>
MAIL_SENDER_DOMAINS=example.com
MAIL_HOSTNAME=mail.example.com
```

`OTS_JWT_SECRET` must be a reference in production. A file reference needs the
file inside the container; with Compose, mount it as a secret in a
`docker-compose.override.yml`:

```yaml
services:
  triplestore:
    secrets: [jwt_secret]
secrets:
  jwt_secret:
    file: ./secrets/jwt_secret   # openssl rand -hex 32 > secrets/jwt_secret
```

The S3 gateway credential needs nothing: compose hands the server
`OTS_S3_SECRET_KEY=env:S3_GATEWAY_SECRET_KEY`, a reference to the variable it also
forwards. `S3_GATEWAY_ACCESS_KEY` and `S3_GATEWAY_SECRET_KEY` are compose variables:
they set the bundled `s3-gateway` service's root credentials, and the server reads
them only through `OTS_S3_ACCESS_KEY` and that reference.

Put it next to `docker-compose.yml` as `.env` and start the stack:

```bash
docker compose up -d
```

Compose reads `.env` twice: to fill the `${…}` values in `docker-compose.yml`,
and through the service's `env_file`, which hands **every** variable in it to the
server — including settings the compose file does not list by name, such as
`OTS_SECURE_COOKIES_ENABLED`, `OTS_TRUSTED_PROXY_CIDRS` or `OTS_SERVICE_ALLOWLIST`. A few entries are
wired by compose itself and ignore `.env`: `OTS_AUTH_DB_PATH` and `OTS_BACKUP_DIR` (the `/data`
volume) and the `OTS_S3_ENDPOINT` / `OTS_S3_ACCESS_KEY` / `OTS_S3_SECRET_KEY` link to the bundled
S3 gateway (`http://s3-gateway:7070`, signed with `S3_GATEWAY_ACCESS_KEY` /
`S3_GATEWAY_SECRET_KEY`). `env_file` with `required: false` needs Docker Compose 2.24 or later. CI runs
`scripts/check_compose_env.py` to keep the compose file and these docs in step.

---

## Docker Quick Reference

```bash
# Start (first run builds the image)
docker compose up -d

# Rebuild after code changes
docker compose build --no-cache && docker compose up -d

# View live logs
docker compose logs -f triplestore

# Open a shell in the running container
docker compose exec triplestore /bin/bash

# Promote a user without stopping the server
docker compose exec triplestore \
  open-triplestore --data-dir /data --promote-super-admin <username>

# Stop and keep data
docker compose down

# Stop and wipe ALL data (destructive)
docker compose down -v
```

## Audit log, backups, and alerting

### Audit log

Every security-relevant event (login, logout, password change, role change,
SPARQL UPDATE, ACL denial, admin user CRUD) is recorded in the `audit_events`
SQLite table. The table is **append-only**: `BEFORE UPDATE` and `BEFORE DELETE`
triggers reject any modification at the database level.

**Admin endpoints** (`super_admin` only):
- `GET /api/admin/audit?limit=&offset=&event_type=&actor_id=&since=` — paginated list
- `GET /api/admin/audit/export?format=csv|json` — full export (capped at 100k rows)

**GDPR/AVG pseudonymisation.** A nightly task rewrites direct PII
(`actor_username`, `actor_role`, `user_agent`, `ip_address`) on rows older than
`OTS_AUDIT_PSEUDONYMISE_AFTER_DAYS` (default `365`). `actor_id` is replaced with a
SHA-256 hash so events stay linkable for forensic analysis without retaining
the original UUID. No row is deleted — this is the data-minimisation control
required by Article 5(1)(e) AVG / GDPR.

### Backups

The backup subsystem produces a snapshot every `OTS_BACKUP_SCHEDULE_SECS` seconds
(default `86400`, a day) containing:
- `rdf.nq.gz` — gzipped N-Quads dump of every named graph
- `changes.db` — the [change log](versioning.md#change-log), when the store
  keeps one: an online SQLite backup taken just after the dump. An
  unpublished [delta version](versioning.md#delta-versions) is a range of
  it, so without it a backup could not bring one back
- `auth.sqlite` — online `rusqlite::backup::Backup` snapshot of the auth DB
- `manifest.json` — SHA-256 checksums + software version, and `changes_seq`:
  the change-log sequence number the dump holds exactly (its snapshot is
  taken at that point of the log)

With encryption on, every file but the manifest carries an `.age` suffix.

| Env var | Default | Purpose |
|---|---|---|
| `OTS_BACKUP_DIR` | `data/backups` | Local backup directory (mode 0o700) |
| `OTS_BACKUP_RETENTION_COUNT` | `7` | Number of snapshots to keep |
| `OTS_BACKUP_SCHEDULE_SECS` | `86400` | Interval |
| `OTS_BACKUP_ENCRYPTION_ENABLED` | `false` | Enable `age` X25519 encryption (requires `--features backup-encrypt`) |
| `OTS_BACKUP_ENCRYPTION_KEY_PATH` | `<data-dir>/backup_key.age` | File containing one X25519 recipient. Required when `OTS_BACKUP_ENCRYPTION_ENABLED=true` — never auto-generated |
| `OTS_BACKUP_DECRYPTION_IDENTITY_PATH` | — | Identity file for restoring an encrypted backup (`--restore`, or a restore staged from the admin page) |
| `OTS_BACKUP_S3_ENABLED` | `false` | Copy each snapshot (scheduled or taken from the admin page) to the configured ObjectStore |

**Admin page and endpoints** (`super_admin` only). **Admin → Backups & index**
shows the configuration, the S3 copy, every backup, and any staged or last
restore; it drives these endpoints:
- `GET /api/admin/backup/status` — configured or not, directory, retention,
  encryption, schedule, the S3 copy (`enabled`, `object_store_configured`,
  `last_upload`), `pending_restore` and `last_restore`
- `POST /api/admin/backup` — take a backup now; with `OTS_BACKUP_S3_ENABLED=true`
  it is copied to S3 like a scheduled one (`s3: {uploaded, error?}` in the answer)
- `GET /api/admin/backup` — list manifests
- `POST /api/admin/backup/{id}/verify` — recompute and compare checksums
- `POST /api/admin/backup/{id}/restore` — stage a restore for the next start
- `DELETE /api/admin/backup/restore` — withdraw the staged restore

A restore is destructive and replaces the identity DB file the running server
holds open, so it never runs while the server serves. Staging one (in the page,
after typing the backup's id in the confirmation dialog) verifies the
checksums at once — and refuses an encrypted backup when
`OTS_BACKUP_DECRYPTION_IDENTITY_PATH` is not set — then writes
`<OTS_BACKUP_DIR>/restore-pending.json`. The next start applies it before the store
and the identity DB are opened, exactly as the CLI does, and records the
outcome in `restore-last.json`, which the page shows. A staged backup is kept
from retention until it is applied or withdrawn. Restart the server to apply it.

The CLI does the same offline, with the server stopped:

```bash
open-triplestore --restore <backup-id> --data-dir ./data
```

Checksums are verified (and an encrypted backup decrypted) before anything is
touched, so a corrupt backup or a wrong key aborts without destroying the live
store.

The change log goes back where the store keeps it,
`<data-dir>/changes/changes.db`, replacing whatever log was there. Writes
that landed between the dump and the copy of the log are in the copy but not
in the dump, so the restore drops every row past `changes_seq`, and it forgets
the delta versions' builds (each is built again on its next read). The log
then goes on in a new epoch: a delta from before the restore is built forward
from its base, through rows that are all in the backup, never backward from
the live graphs, and replication followers resynchronise. A backup made
before 0.8.1 has no change log and restores as before, except that the
store's own log, if it has one, moves to a new epoch.

#### Encrypted backups

`OTS_BACKUP_ENCRYPTION_ENABLED=true` requires an **operator-supplied** recipient — the server
will not start without one, and deliberately does not generate one:

```bash
age-keygen -o backup-identity.txt        # keep this file OFF the server
grep 'public key' backup-identity.txt    # → age1…
printf 'age1…\n' > /secure/backup_key.age
```

Point `OTS_BACKUP_ENCRYPTION_KEY_PATH` at the recipient file. To restore, give the
CLI the identity you kept:

```bash
OTS_BACKUP_DECRYPTION_IDENTITY_PATH=/secure/backup-identity.txt \
  open-triplestore --restore <backup-id> --data-dir ./data
```

The server stores only the recipient, never the identity — so automatic
crash-recovery (`OTS_STORE_AUTO_RECOVER_ENABLED`) cannot restore an encrypted backup and
says so loudly rather than starting empty.

### Alerting

Two backends, both opt-in via env vars, both no-ops when unset:

| Channel | Env vars | Build flag |
|---|---|---|
| HTTP webhook | `OTS_ALERT_WEBHOOK_URL` | always available |
| SMTP email | `OTS_ALERT_SMTP_HOST`, `OTS_ALERT_SMTP_PORT`, `OTS_ALERT_SMTP_USERNAME`, `OTS_ALERT_SMTP_PASSWORD`, `OTS_ALERT_SMTP_TLS`, `OTS_ALERT_SMTP_FROM`, `OTS_ALERT_SMTP_TO` (comma-separated) | `--features alerting` |

`OTS_ALERT_SMTP_TLS` is `none`, `starttls` or `implicit`, as for `OTS_SMTP_TLS`; unset,
alerting uses implicit TLS (SMTPS), as it always has. When `OTS_ALERT_SMTP_HOST` is
unset, alerting uses the account-email relay instead, as a whole: `OTS_SMTP_HOST`,
`OTS_SMTP_PORT`, `OTS_SMTP_USERNAME`, `OTS_SMTP_PASSWORD` and `OTS_SMTP_TLS` (resolved as for
account email, so port 465 means implicit TLS and anything else STARTTLS). It
never mixes the two, so the account relay's credentials are not sent to a
separate alert host. The sender is `OTS_ALERT_SMTP_FROM`, else `OTS_SMTP_FROM`.
Recipients come only from `OTS_ALERT_SMTP_TO`, so the fallback sends no ops alert
until that is set. Saved-query breakage notices, which go to the affected
dataset's owners rather than `OTS_ALERT_SMTP_TO`, use the same relay, so on a
deployment with only `OTS_SMTP_*` configured they are now delivered.

Every successful dispatch is recorded in the audit log as `alert_sent`.

### Litestream sidecar (optional, recommended for HA)

For continuous replication of the SQLite auth DB to S3 — finer-grained than
the daily snapshots — run a Litestream sidecar alongside the triplestore in
`docker-compose.yml`:

```yaml
  litestream:
    image: litestream/litestream:latest
    restart: unless-stopped
    command: replicate
    volumes:
      - ./data:/data:ro
      - ./litestream.yml:/etc/litestream.yml:ro
    environment:
      LITESTREAM_ACCESS_KEY_ID: ${OTS_S3_ACCESS_KEY}
      LITESTREAM_SECRET_ACCESS_KEY: ${OTS_S3_SECRET_KEY}
```

`litestream.yml`:

```yaml
dbs:
  - path: /data/auth.sqlite
    replicas:
      - type: s3
        bucket: my-triplestore-litestream
        path: auth
        region: eu-west-1
        retention: 720h
```

Litestream streams the WAL frames, so RPO is seconds rather than the daily
snapshot cadence. The two layers complement each other: Litestream for
point-in-time recovery, snapshots for long-term retention and RDF coverage.
