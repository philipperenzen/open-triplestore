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

The `"publisher"` grant only sets `can_publish`; it does not change the account role. SSO grants the capability non-destructively — it is set on matching sign-ins and never revoked just because a claim is absent. (Group/claim mapping applies on the SAML and resource-server OIDC paths; the interactive ID-token OIDC flow does not yet extract groups.)

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
docker exec open-triplestore \
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

Valid roles: `user`, `admin`. You cannot create a user with a higher role than your own.

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

| Variable | Default | Description |
|---|---|---|
| `JWT_SECRET` | *(random, saved to `data/jwt_secret`)* | JWT signing secret. Set explicitly in production so tokens survive restarts. The on-disk file is written `0600`. The server **refuses to start** if this is a well-known default/placeholder (e.g. `change-me-in-production`), naming the fix — leave it unset to auto-generate a strong one. |
| `AUTH_DB_PATH` | `<data-dir>/auth.db` | Path to the SQLite identity database |
| `ACCESS_TOKEN_EXPIRY_MINUTES` | `30` | Access token lifetime |
| `REFRESH_TOKEN_EXPIRY_DAYS` | `30` | Refresh token lifetime |
| `SECURE_COOKIES` | `false` | Issue auth cookies with the `Secure` attribute (HTTPS only). **Set to `true` in any TLS deployment**; leave `false` for plain-HTTP local development. |
| `SERVE_FRONTEND` | `true` | Serve the bundled web UI (frontend SPA) at `/`. Set `false` for a headless, API-only server — SPARQL, Graph Store and REST endpoints are unaffected. Also the `--serve-frontend` CLI flag. |
| `CORS_ORIGINS` | *(empty — same-origin only)* | Comma-separated allowed origins, e.g. `https://app.example.com,https://www.example.com` |
| `TRUSTED_PROXY_CIDRS` | *(empty — direct TCP IP)* | Comma-separated CIDRs of reverse proxies whose `X-Forwarded-For` is honoured for rate limiting, e.g. `10.0.0.0/8,172.16.0.0/12`. Leave empty when not behind a proxy. |
| `RATE_LIMIT_DISABLED` | `false` | Set to `true`/`1` to switch off per-IP rate limiting (auth, SPARQL and import quotas). For trusted/internal deployments and the test/CI harness only — **never enable on a public server**. Secure by default. |
| `BASE_URL` | `http://localhost:7878` | Base URL used to mint linked-data IRIs (no trailing slash) |
| `SPARQL_QUERY_TIMEOUT_SECS` | `30` | Per-query/update execution timeout in seconds |
| `S3_ENDPOINT` | *(unset — local filesystem)* | S3/MinIO endpoint URL. If unset, assets are stored in `<data-dir>/assets/` |
| `S3_BUCKET` | `triplestore-assets` | S3 bucket name |
| `S3_ACCESS_KEY` | | S3 access key |
| `S3_SECRET_KEY` | | S3 secret key |
| `S3_REGION` | `us-east-1` | S3 region |
| `BACKUP_DIR` | `data/backups` | Directory for scheduled backups |
| `BACKUP_RETENTION_COUNT` | `7` | Number of backups to retain |
| `BACKUP_SCHEDULE_HOURS` | `24` | Hours between scheduled backups |
| `BACKUP_ENCRYPT` | `false` | Encrypt backups with `age` X25519 (requires the `backup-encrypt` build feature) |
| `BACKUP_ENCRYPT_KEY_PATH` | `data/backup_key.age` | Path to the backup encryption key (auto-generated if absent) |
| `AUDIT_PSEUDONYMISE_AFTER_DAYS` | `365` | GDPR/AVG: pseudonymise audit rows older than this |
| `TEXT_SEARCH_DIR` | `<data-dir>/tantivy` | Tantivy full-text index directory (requires the `text-search` build feature) |
| `ALERT_WEBHOOK_URL` / `ALERT_SMTP_*` | *(unset — alerting off)* | Optional webhook / SMTP alerting (requires the `alerting` build feature) |

### Recommended production `.env`

```dotenv
JWT_SECRET=<64-char random string>
CORS_ORIGINS=https://www.example.com
SECURE_COOKIES=true
BASE_URL=https://www.example.com
MINIO_ROOT_USER=triplestore
MINIO_ROOT_PASSWORD=<strong password>
```

Pass to Docker Compose:

```bash
docker compose --env-file .env up -d
```

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
docker exec -it open-triplestore /bin/bash

# Promote a user without stopping the server
docker exec open-triplestore \
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
`AUDIT_PSEUDONYMISE_AFTER_DAYS` (default `365`). `actor_id` is replaced with a
SHA-256 hash so events stay linkable for forensic analysis without retaining
the original UUID. No row is deleted — this is the data-minimisation control
required by Article 5(1)(e) AVG / GDPR.

### Backups

The backup subsystem produces a snapshot every `BACKUP_SCHEDULE_HOURS` hours
(default `24`) containing:
- `rdf.nq.gz` — gzipped N-Quads dump of every named graph
- `auth.sqlite` — online `rusqlite::backup::Backup` snapshot of the auth DB
- `manifest.json` — SHA-256 checksums + software version

| Env var | Default | Purpose |
|---|---|---|
| `BACKUP_DIR` | `data/backups` | Local backup directory (mode 0o700) |
| `BACKUP_RETENTION_COUNT` | `7` | Number of snapshots to keep |
| `BACKUP_SCHEDULE_HOURS` | `24` | Cron interval |
| `BACKUP_ENCRYPT` | `false` | Enable `age` X25519 encryption (requires `--features backup-encrypt`) |
| `BACKUP_ENCRYPT_KEY_PATH` | — | Path to a file containing one X25519 recipient |
| `BACKUP_S3_ENABLED` | `false` | Mirror each snapshot to the configured ObjectStore |

**Admin endpoints** (`super_admin` only):
- `POST /api/admin/backup` — trigger immediately
- `GET /api/admin/backup` — list manifests
- `POST /api/admin/backup/{id}/verify` — recompute and compare checksums

Restore is **out of scope for the API** — it is a destructive operation that
should be performed manually: stop the server, replace `auth.sqlite` with the
backup file (decrypting with `age` first if applicable), then re-import the
N-Quads dump into a freshly-initialised RocksDB store.

### Alerting

Two backends, both opt-in via env vars, both no-ops when unset:

| Channel | Env vars | Build flag |
|---|---|---|
| HTTP webhook | `ALERT_WEBHOOK_URL` | always available |
| SMTP email | `ALERT_SMTP_HOST`, `ALERT_SMTP_PORT`, `ALERT_SMTP_USER`, `ALERT_SMTP_PASS`, `ALERT_SMTP_FROM`, `ALERT_SMTP_TO` (comma-separated) | `--features alerting` |

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
      LITESTREAM_ACCESS_KEY_ID: ${S3_ACCESS_KEY}
      LITESTREAM_SECRET_ACCESS_KEY: ${S3_SECRET_KEY}
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
