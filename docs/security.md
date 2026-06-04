# Security & Access Control

Beyond dataset visibility and API token scopes, administrators get fine-grained control over identity providers, HTTP endpoints, named graphs, and individual triples. These tools live under [Admin → Security](/admin/security) and require an admin (or super_admin) account.

## Single Sign-On (OIDC / Azure AD / SAML)

Register external identity providers so users can sign in with corporate credentials. Three provider types are supported:

- **Generic OIDC** — Any OpenID Connect provider. Supply a discovery URL (or explicit authorization, token, and userinfo endpoints), client ID, client secret, and requested scopes.
- **Azure AD / Entra ID** — Microsoft Entra tenants, configured like OIDC with tenant-specific endpoints.
- **SAML 2.0** — Enterprise SAML identity providers for browser-based SSO.

A provider can **auto-provision** accounts on first login and map an incoming claim to a system role: `role_claim` names the claim (e.g. a group attribute), `role_claim_map` maps each claim value to `user`, `admin`, or `super_admin`, and `default_role` applies when nothing matches. Providers can be disabled without deleting them; enabled ones appear automatically on the login page. Configured providers are listed at `GET /api/auth/oauth/providers`; admin CRUD is under `/api/admin/oauth/providers`.

## Endpoint ACL

Allow or deny access to HTTP endpoints by principal, independent of token scope. Each rule names a principal (`user`, `role`, `organisation`, or `group`), a path pattern with wildcard support (e.g. `/api/*`), an HTTP method (`GET`, `POST`, `PUT`, `DELETE`, or `*`), an effect (**allow** / **deny**), and a numeric **priority**. Higher priority is evaluated first, and an explicit *deny* overrides an *allow*. Managed at `/api/admin/acl/endpoints`.

## Named-Graph ACL

Grant a principal access to a specific named graph at one of three levels: `read`, `write` (includes read), or `admin` (includes write). The special **public** principal grants unauthenticated read access to a single graph. Grants are additive and apply to both SPARQL queries and the Graph Store Protocol — an explicit write grant is required for `PUT` / `POST` / `DELETE` on `/store` and for any SPARQL Update that targets the graph. Managed at `/api/admin/acl/graphs`.

## Triple Security Labels

For cell-level security, individual triples (matched by subject, predicate, and object) can be assigned to a **label graph**. The triple is then visible only to principals who can read that label graph via the Named-Graph ACL. When a graph carries security labels, both Graph Store downloads and SPARQL results are filtered per caller; admins bypass filtering. Managed at `/api/admin/acl/triples`.

## Audit Log

An append-only audit log records security-relevant events — logins, user and token management, SPARQL updates, backups, and ACL changes — each with the acting principal, a timestamp, and the outcome. Admins review it at `GET /api/admin/audit` and export it as CSV or JSON at `GET /api/admin/audit/export`.

## Asset Malware Scanning

Uploaded assets can be scanned for malware by a [ClamAV](https://www.clamav.net/) daemon before they are stored. Set `CLAMAV_ADDR=host:port` to point at the daemon's INSTREAM endpoint (e.g. `127.0.0.1:3310`); leaving it unset disables scanning. Only an explicit virus hit rejects the upload (HTTP 422); a scanner outage or error fails open so a ClamAV hiccup cannot take uploads down. Requires the `asset-clamav` build feature, which is included in the `full` feature shipped in release/Docker builds.
