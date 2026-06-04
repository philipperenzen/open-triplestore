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

Manage your own account from [Settings](/settings): change your password, create and revoke API tokens, and deactivate or permanently purge your account. Identity providers for SSO are configured by admins under [Security & Access Control](/docs/security).
