# The store as an OIDC provider

Open Triplestore can act as the **identity provider** for a set of client
apps (unified accounts): users register and manage one account here, and every
client app signs them in with the standard **authorization-code + PKCE** flow.
Nothing needs a separate IdP; corporate SSO (see [auth.md](auth.md)) stays an
optional way to sign in *to this store*, not a replacement for it.

## Endpoints

| Endpoint | What |
|---|---|
| `GET /.well-known/openid-configuration` | Discovery document (issuer = `BASE_URL`). |
| `GET /oauth/authorize` | The SPA route driving login + consent (advertised in discovery; standard clients just redirect here). |
| `POST /oauth/token` | Code → tokens, refresh → tokens. Form-encoded (RFC 6749). |
| `GET /oauth/jwks` | The ES256 public key set for offline verification. |
| `GET /oauth/userinfo` | Standard claims for a provider access token. |
| `POST /api/oauth/authorize` | (Authenticated; used by the authorize SPA route.) Validates client, exact-match redirect URI and PKCE, then mints the single-use 10-minute code. |

Deliberately **not** supported: the implicit/hybrid flows, the `plain` PKCE
method, wildcard redirect URIs, and unauthenticated dynamic client
registration.

## Registering clients

Clients live in the `oauth_clients` table:

- **Admin UI:** Security → *Sign-in apps* (list, register, delete).
- **API:** `GET/POST /api/admin/oauth-clients`, `DELETE /api/admin/oauth-clients/:id`.
- **Declarative (infra-as-code):** set `OAUTH_CLIENTS_JSON` and the store
  upserts at boot:

```json
[
  {"client_id": "web-app", "name": "Web App", "public": true,
   "redirect_uris": ["http://localhost:5173/auth/callback"]},
  {"client_id": "backend-app", "name": "Backend App", "public": false,
   "redirect_uris": ["http://localhost:8081/auth/callback"],
   "secret": "…"}
]
```

Public clients (browser SPAs) have **no secret and must use PKCE (S256)**.
Confidential clients authenticate with `client_secret` (stored AES-GCM
encrypted, like SSO provider secrets). Redirect URIs are an exact-match
allowlist.

## Tokens

- **Access token** — ES256 JWT, 1 hour: `iss` (= `BASE_URL`), `sub` (account
  id), `aud` (client_id), `scope`, `username`, `email`, `role`, and the
  account's `organisations` (`[{slug, role}]`) / `groups`
  (`[{org_slug, id, name}]`) memberships, so resource servers can authorize
  without another round-trip.
- **ID token** — ES256 JWT with `nonce`, `email`, `preferred_username`, `name`.
- **Refresh token** — opaque `otr_…`, 30 days, stored hashed,
  **single-use with rotation**: every refresh returns a new one and the old
  one dies; a replayed token is refused.

The auth middleware accepts provider access tokens directly as bearer tokens,
so `GET /api/auth/me` and every other API endpoint work with them. That
includes the deactivation semantics: a guest disabled by the
[guest-registration toggle](auth.md) gets that specific message. How much a
provider token may *do* is a deployment setting, described next.

## What a provider token may do

A provider access token is a delegation: the user consented to let a client
act for them. `OTS_OIDC_SESSION_POLICY` decides how far that delegation
reaches. It applies only to access tokens this store issues at
`/oauth/token`. Tokens from an external IdP
([resource-server mode](auth.md#oidc-resource-server-mode-idp-access-tokens))
have their own setting, `OIDC_TOKEN_POLICY`, with the same values. Session
tokens and `ots_` API tokens are not affected.

| Policy | Read | Write | Create API tokens |
|---|---|---|---|
| `session` *(default)* | yes | yes | no |
| `scoped` | yes | only if the token's `scope` grants it (see below) | no |
| `full` (`legacy` is accepted too) | yes | yes | yes |

- Under the default, **every registered client's tokens can write** wherever
  the user can, whatever scopes the client asked for. Writes are still subject
  to the user's dataset and graph permissions, but a client the user signed
  into with `openid profile email` can change their data. Register only
  clients you would trust with that, or use `scoped`.
- No policy except `full` lets a provider token mint an API token
  (`POST /api/auth/tokens` answers `403`). This keeps a one-hour delegation
  from becoming permanent account access. Tokens from an external IdP follow
  the same rule under their own default; see
  [auth.md](auth.md#what-an-idp-token-may-do).
- "Write" means every `POST`, `PUT`, `PATCH` and `DELETE` request, including
  SPARQL Update. A refused write gets `403 This API token does not have write
  scope`. A SPARQL *query* sent by `POST` is a read and still works.
- **Admin and super-admin accounts are exempt from the write check** under
  every policy: their provider tokens always write.
- Accounts with the `guest` role stay limited by `OTS_GUEST_CAPABILITIES` on
  top of the policy.
- An unknown value logs a warning and falls back to `session`, never to
  `full`.

### `scoped` makes provider tokens read-only today

Under `scoped`, a token writes only when its `scope` contains `write` or
`admin`, or a value listed in `OTS_OIDC_WRITE_SCOPES` (comma- or
space-separated, compared case-insensitively). But this provider currently
knows only the scopes `openid`, `profile` and `email`: discovery advertises
just those, and any other scope a client requests is dropped before the code
is issued. So no provider token can carry `write` or `admin`, and
`OTS_OIDC_WRITE_SCOPES` can only name one of those three standard scopes,
which would make `scoped` behave like `session` for every client asking for
it. In practice, **`scoped` makes every provider token read-only for non-admin
accounts.** That is the right choice for a deployment whose client apps only
read. A client app that writes needs `session` until the provider can issue a
write scope.

## Verifying tokens in a resource server

Fetch `/.well-known/openid-configuration`, cache `jwks_uri`, verify
`alg=ES256`, `iss`, `exp` and your own `aud` (your `client_id`). Any standard
OIDC/JWT library works; a resource server simply points its
existing `OIDC_ISSUER` configuration at the store's base URL. Where offline verification is
inconvenient, `GET /api/auth/me` with the token as a bearer keeps working as
an introspection endpoint.

## Keys

An ES256 keypair is generated on first boot and persisted in the auth DB with
the private key AES-GCM-encrypted (key derived from `JWT_SECRET` — rotating
`JWT_SECRET` therefore invalidates the stored provider key and a new one is
generated, which signs future tokens). If key material can't be loaded the
provider endpoints answer `503` and everything else keeps running.
