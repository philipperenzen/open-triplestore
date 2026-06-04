# Operations

Endpoints and behaviours that matter when running the triplestore as a service.

## Health & service description

- **Health probe** — `GET /health` returns the software version and the status of each subsystem (triplestore, database, object storage, backup) as JSON, with a `503` when a core service is down — suitable for load-balancer and container health checks.
- **Service description** — `GET /` returns a SPARQL 1.1 Service Description (Turtle) advertising capabilities and the named graphs the caller may access.

## Bulk import

The [Data Import](/import) wizard can upload many files in a single request via `POST /api/import/bulk`. Before committing, `POST /api/import/analyze` runs a pre-flight pass that detects each file's format, embedded graphs, and role (see [Import Auto-Detection](/docs/import)) so you can review the plan first. New datasets and organisations are only created once the first file imports successfully.

## Backups

When a backup directory is configured, admins can create, list, and verify backups. A backup is a compressed snapshot of the RDF data and the account database, with a checksum manifest for integrity verification and optional encryption at rest. Endpoints: `GET` / `POST /api/admin/backup` to create and list, and `POST /api/admin/backup/{id}/verify` to verify against the manifest.

## Rate limiting

Requests are rate-limited per client IP, with stricter quotas on authentication endpoints to resist brute-force attacks and separate quotas for query and import traffic. When running behind a reverse proxy, configure the trusted proxy ranges so limits apply per real client IP rather than the proxy's own address.

For trusted/internal deployments or automated test harnesses that drive many requests from a single IP, the limiter can be switched off with `RATE_LIMIT_DISABLED=true` (see [administration.md](administration.md)). It is secure by default — leave it unset on any public server.
