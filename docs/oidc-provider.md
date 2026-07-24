# The store as an OIDC provider

Open Triplestore can act as the **identity provider** for a suite of client
apps (Unified Accounts): users register and manage one account here, and every
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
  {"client_id": "otl-viewer", "name": "OTL Viewer", "public": true,
   "redirect_uris": ["http://localhost:5190/auth/callback"]},
  {"client_id": "otl-validation", "name": "OTL Validation", "public": false,
   "redirect_uris": ["http://localhost:5180/auth/callback"],
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

The auth middleware accepts provider access tokens directly, so
`GET /api/auth/me` (and every other API) works with them like any session
token or `ots_` PAT — including the deactivation semantics (a guest disabled
by the [guest-registration toggle](auth.md) gets that specific message).

## Verifying tokens in a resource server

Fetch `/.well-known/openid-configuration`, cache `jwks_uri`, verify
`alg=ES256`, `iss`, `exp` and your own `aud` (your `client_id`). Any standard
OIDC/JWT library works; the suite's validation platform simply points its
existing `OIDC_ISSUER` at the store's base URL. Where offline verification is
inconvenient, `GET /api/auth/me` with the token as a bearer keeps working as
an introspection endpoint.

## Keys

An ES256 keypair is generated on first boot and persisted in the auth DB with
the private key AES-GCM-encrypted (key derived from `JWT_SECRET` — rotating
`JWT_SECRET` therefore invalidates the stored provider key and a new one is
generated, which signs future tokens). If key material can't be loaded the
provider endpoints answer `503` and everything else keeps running.
