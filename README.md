<p align="center">
  <img src="docs/assets/logo.svg" alt="Open Triplestore" width="120" height="120">
</p>

<h1 align="center">Open Triplestore</h1>

<p align="center">
  A fast, <strong>source-available</strong> RDF triplestore — SPARQL&nbsp;1.1&nbsp;&amp;&nbsp;1.2 (RDF-star), GeoSPARQL, OWL&nbsp;2, SHACL, LDP, DCAT&nbsp;&amp;&nbsp;full-text search, with a polished web UI.<br>
  Built in Rust on <a href="https://github.com/oxigraph/oxigraph">Oxigraph</a> with an <a href="https://github.com/tokio-rs/axum">Axum</a> HTTP layer.
</p>

<p align="center">
  <a href="https://github.com/philipperenzen/open-triplestore/actions/workflows/ci.yml"><img src="https://github.com/philipperenzen/open-triplestore/actions/workflows/ci.yml/badge.svg?branch=main" alt="CI"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-AGPL--3.0%20%2B%20Commons%20Clause-blue" alt="License"></a>
  <img src="https://img.shields.io/badge/rust-1.88%2B-orange" alt="Rust 1.88+">
  <img src="https://img.shields.io/badge/SPARQL-1.1%20%2F%201.2-8A2BE2" alt="SPARQL 1.1 / 1.2">
  <img src="https://img.shields.io/badge/GeoSPARQL-1.1-2F7A8C" alt="GeoSPARQL 1.1">
  <a href="CONTRIBUTING.md"><img src="https://img.shields.io/badge/PRs-welcome-brightgreen" alt="PRs welcome"></a>
  <a href="https://github.com/philipperenzen/open-triplestore/stargazers"><img src="https://img.shields.io/github/stars/philipperenzen/open-triplestore?style=social" alt="Stars"></a>
</p>

<p align="center">
  <a href="#quick-start"><b>Quick Start</b></a> ·
  <a href="#highlights">Features</a> ·
  <a href="#demo">Demo</a> ·
  <a href="#web-ui">Web UI</a> ·
  <a href="docs/">Docs</a> ·
  <a href="CONTRIBUTING.md">Contributing</a> ·
  <a href="#license">License</a>
</p>

---

> **Status:** current release **`0.2.4`** — source-available: free to use, self-host, and modify; **not for sale or paid hosting** (see [License](#license)).

**Open Triplestore** is a modern, high-performance RDF triple store with full **SPARQL 1.1**, **SPARQL 1.2 (RDF-star)**, **GeoSPARQL 1.1**, **OWL 2** reasoning (RL natively + DL rules, with an external-reasoner bridge for full tableau classification/consistency), and **LDP 1.0** support — built in Rust on top of [Oxigraph](https://github.com/oxigraph/oxigraph) with an [Axum](https://github.com/tokio-rs/axum) HTTP layer, JWT/API-key auth, and a full-featured Svelte web UI.

## Demo

The web UI is **served by the binary itself** at `http://localhost:7878/` — there's no separate frontend to deploy. At a glance:

<!-- 🎬 To embed a screencast: record a short walkthrough of the running app and save it to
     docs/assets/demo.gif, then replace this comment with:
       ![Open Triplestore web UI](docs/assets/demo.gif)
     Run locally with only the bundled (neutral) sample data:
       # macOS / Linux / WSL:
       JWT_SECRET=$(openssl rand -hex 32) \
         cargo run --release -- --port 7878 --data-dir ./demo-data --load examples/standards-demo.ttl
       # Windows PowerShell:
       $env:JWT_SECRET = -join ((1..32) | ForEach-Object { '{0:x2}' -f (Get-Random -Maximum 256) })
       cargo run --release -- --port 7878 --data-dir .\demo-data --load examples\standards-demo.ttl
     then open http://localhost:7878/ and register the first user (it becomes super_admin). -->

| Surface | What it does |
|---|---|
| 🏠 **Overview** | Live triple & named-graph counts, suggested next steps, and quick links into every workflow |
| ⌨️ **SPARQL workspace** | CodeMirror editor (`Ctrl+Enter` to run), Table / JSON / Graph result views, query history, CSV & JSON export, optional NL→SPARQL |
| 🕸️ **Explore & visualize** | Facet triples by subject / predicate / object / graph, then expand resources visually to walk a graph's neighbourhood |
| 📚 **Datasets · Models** | DCAT metadata, per-dataset SHACL, a unified model registry for ontologies & SKOS vocabularies, and version diffs |
| ✅ **Validate** | SHACL (Core + Advanced) and ShEx, with per-dataset reports and severity filters |
| 🔐 **Admin** | Users, roles, and graph-level ACLs |

> The bundled sample catalogue (`examples/standards-demo.ttl`) is generic, neutral data — load it to explore the UI exactly as described.

## Highlights

| Feature | Detail |
|---|---|
| **SPARQL 1.1** | SELECT, CONSTRUCT, ASK, DESCRIBE, UPDATE (INSERT/DELETE) |
| **SPARQL 1.2** | RDF-star embedded triples |
| **GeoSPARQL 1.1** | All 30 OGC requirements — Simple Features, Egenhofer, RCC8, constructive & metric functions |
| **OWL 2 DL** | Native hasSelf, disjointUnionOf, NegativePropertyAssertion, hasKey + all ~80 RL rules; external reasoner bridge for full tableau ([docs](docs/owl2-dl.md)) |
| **LDP 1.0** | Basic, Direct, Indirect Containers; NonRDFSource; PATCH with SPARQL Update; Prefer header ([docs](docs/ldp.md)) |
| **RBAC auth** | `super_admin` › `admin` › `user` role hierarchy; JWT access + refresh tokens; long-lived API keys |
| **Dataset privacy** | Datasets default to `private`; public datasets are queryable without auth |
| **SHACL validation** | Validate data on read or write; SHACL-AF rule inference; shapes stored per dataset |
| **SHACL on write** | Automatic SHACL validation on every Graph Store PUT/POST — returns 422 with full report on violation |
| **SHACL Compact Syntax** | Parse and serialize shapes in [SHACLC](https://w3c.github.io/shacl/shacl-compact-syntax/) via `Accept: text/shaclc` |
| **DCAT 2 catalog** | Full W3C DCAT 2 catalog at `/.well-known/void` — per-dataset distributions, VoID statistics, PROV-O provenance |
| **RML mapping** | [RDF Mapping Language](https://rml.io/specs/rml/) — CSV, JSON (JSONPath), XML (XPath) → RDF with template expansion |
| **OpenAPI docs** | Interactive Swagger UI at `/api-docs/` with JWT Bearer auth; machine-readable spec at `/api-docs/openapi.json` |
| **AI assistant** *(optional)* | Natural-language → SPARQL, a grounded knowledge-graph chat, and a SHACL drafting assistant — run the **bundled local model** (`docker compose --profile llm up`, GPU-accelerated on NVIDIA) or **bring your own** OpenAI-compatible API (OpenAI, vLLM, Azure, …) via `LLM_GATEWAY_URL`; off by default, hidden until reachable ([docs](docs/api-services.md)) |
| **Prefix auto-resolution** | Unknown prefixes resolved on-the-fly via [prefix.cc](https://prefix.cc) with local caching |
| **Multiple RDF formats** | Turtle, N-Triples, N-Quads, TriG, RDF/XML |
| **Storage backends** | In-memory (fast) and persistent RocksDB |
| **HTTP protocols** | SPARQL Protocol + Graph Store HTTP Protocol (RFC 7230) + LDP 1.0 |
| **Docker-ready** | Multi-stage image; non-root runtime; health-check built-in |

---

## Quick Start

> [!NOTE]
> **Runs on Windows, macOS, and Linux.** Docker is the easiest path on every OS and
> the commands are identical. The shell snippets below use bash/`curl` syntax; on
> **Windows PowerShell** call **`curl.exe`** (plain `curl` is an alias for
> `Invoke-WebRequest`, which takes different flags) and either keep each command on
> one line or use a backtick `` ` `` for line continuation instead of `\`. The
> examples run verbatim in **WSL**, **Git Bash**, and `cmd.exe`. Native (non-Docker)
> builds on Windows need extra setup — see the **[Windows guide](docs/windows.md)**;
> WSL2 is the smoothest route.

### Docker (recommended)

Docker Desktop runs the same Linux image on every OS, so this is the simplest way to
get a fully-featured instance on Windows, macOS, or Linux.

**1. Create `.env` with secrets.** Compose ships no insecure defaults and refuses to
start until these are set (Option A only — the standalone container auto-generates a
secret):

```bash
# macOS · Linux · WSL · Git Bash
cp .env.example .env
printf 'JWT_SECRET=%s\n'          "$(openssl rand -hex 32)" >> .env
printf 'MINIO_ROOT_USER=%s\n'     "$(openssl rand -hex 8)"  >> .env
printf 'MINIO_ROOT_PASSWORD=%s\n' "$(openssl rand -hex 24)" >> .env
```

```powershell
# Windows PowerShell — no OpenSSL required
Copy-Item .env.example .env
function New-Secret([int]$n) { -join ((1..$n) | ForEach-Object { '{0:x2}' -f (Get-Random -Maximum 256) }) }
Add-Content .env "JWT_SECRET=$(New-Secret 32)"
Add-Content .env "MINIO_ROOT_USER=$(New-Secret 8)"
Add-Content .env "MINIO_ROOT_PASSWORD=$(New-Secret 24)"
```

**2. Start the stack:**

```bash
# Option A: docker compose — full stack incl. MinIO (S3 asset store); reads .env
docker compose up -d

# Option B: standalone container — no MinIO; JWT secret auto-generates in /data
docker build -t open-triplestore .
docker run -p 7878:7878 -v triplestore_data:/data open-triplestore
```

**Optional — AI features (local LLM).** The AI assistant (natural-language → SPARQL, grounded knowledge-graph chat, SHACL drafting) needs an OpenAI-compatible model endpoint. Run the bundled [Ollama](https://ollama.com) service — it auto-pulls `qwen2.5:7b` on first start:

```bash
docker compose --profile llm up -d                                                   # CPU
docker compose -f docker-compose.yml -f docker-compose.gpu.yml --profile llm up -d   # NVIDIA GPU
```

The first start downloads the model (~5 GB); AI features turn on once it is ready (check `GET /api/llm/health`). Prefer a hosted model? Skip the `llm` profile and point `LLM_GATEWAY_URL` (+ `LLM_API_KEY`) at your endpoint, with `LLM_MODEL` for the model name. The NVIDIA GPU path needs the [NVIDIA Container Toolkit](https://docs.nvidia.com/datacenter/cloud-native/container-toolkit/latest/install-guide.html).

### Native (requires Rust 1.88+)

System libraries are needed on every OS: **GEOS** (GeoSPARQL) always, plus
**libxmlsec1** for the `saml` feature in `--features full`. On Debian/Ubuntu:
`apt-get install libgeos-dev libxmlsec1-dev`; on macOS: `brew install geos libxmlsec1`.

```bash
# macOS · Linux · WSL
cargo build --release
./target/release/open-triplestore --port 7878 --data-dir ./data
```

```powershell
# Windows PowerShell — note the .exe suffix and back-slashes
.\target\release\open-triplestore.exe --port 7878 --data-dir .\data
```

> [!IMPORTANT]
> A **native Windows** build has to provide GEOS (and, for `--features full`,
> libxmlsec1) to the MSVC toolchain, which is fiddly. Prefer **Docker** (above) or
> build inside **WSL2** using the Linux instructions. Full step-by-step setup —
> including an experimental native MSVC build via vcpkg — is in the
> **[Windows guide](docs/windows.md)**.

```
Options:
  -d, --data-dir  <PATH>      Storage directory         [default: ./data]
  -p, --port      <PORT>      HTTP port                 [default: 7878]
  -b, --bind      <ADDR>      Bind address              [default: 0.0.0.0]
      --load      <FILE>      Load RDF file on startup
      --log-level <LEVEL>     Log level                 [default: info]
      --serve-frontend <BOOL> Serve the bundled web UI  [default: true]
      --access-token-expiry-minutes <N>  JWT access token TTL  [default: 30]
      --refresh-token-expiry-days   <N>  Refresh token TTL     [default: 30]
      --promote-super-admin <USERNAME>   Promote user and exit
```

### Verify

```bash
curl http://localhost:7878/health
# {"status":"ok","version":"0.2.4"}
```

> On **Windows PowerShell**, run `curl.exe http://localhost:7878/health` — the bare
> `curl` alias resolves to `Invoke-WebRequest` and won't behave like the examples.
> (`cmd.exe`, Git Bash, and WSL all accept plain `curl`.)

---

## Authentication & First Login

Open Triplestore has no pre-seeded credentials. **The first account registered automatically becomes `super_admin`.**

### Creating the initial super admin

1. Navigate to `http://localhost:7878/` and click **Register**
2. Fill in username, email, and password (min 8 characters)
3. Submit — this account is instantly granted `super_admin` role
4. All subsequent registrations default to `user` role

Alternatively, if you need to promote an existing user (e.g. after a restore):

```bash
# macOS / Linux / WSL
./target/release/open-triplestore --promote-super-admin <username>
# Windows PowerShell: .\target\release\open-triplestore.exe --promote-super-admin <username>
# Prints "Promoted user '<username>' to super_admin" and exits
```

### Role hierarchy

| Role | Capabilities |
|---|---|
| `super_admin` | Full access — can manage all users including other admins, set any role |
| `admin` | Can manage users with `user` role; cannot touch `super_admin` accounts |
| `user` | Can access their own datasets and public resources |

### Tokens

| Token type | Format | TTL | Use |
|---|---|---|---|
| Access token | JWT | 30 min (configurable) | `Authorization: Bearer <jwt>` |
| Refresh token | JWT (DB-tracked) | 30 days (configurable) | `POST /api/auth/refresh` |
| API key | `ots_<random>` | Configurable or permanent | `Authorization: Bearer ots_…` |

API keys are generated in **Settings → API Tokens** in the web UI, or via `POST /api/auth/tokens`. The full key is shown **only once** on creation; only its SHA-256 hash is stored.

---

## Web UI

A full-featured browser interface is bundled with the server at `http://localhost:7878/`.

### Pages

| Route | Description |
|---|---|
| `/` | Dashboard — triple/graph/dataset counts, recent triples, quick-action grid |
| `/sparql` | SPARQL editor — CodeMirror 6, Ctrl+Enter to execute, result tabs (Table / Raw JSON / Graph), query history, CSV/JSON export; optional natural-language → SPARQL when an LLM is configured |
| `/chat` | **Ask AI** *(optional)* — grounded natural-language chat over your knowledge graph that generates and runs read-scoped SPARQL; available when an LLM is configured |
| `/browse` | Triple browser with subject/predicate/object/graph filters and pagination |
| `/graphs` | Named graph dashboard — per-graph triple counts, browse/visualize/export/delete actions |
| `/graph-viz` | Interactive graph visualizer — enter a URI, double-click nodes to expand, multiple layout algorithms |
| `/resource` | Resource detail — outgoing properties, incoming references, type badges, mini neighbourhood graph |
| `/import` | 4-step data import wizard — drag-and-drop RDF files, paste SPARQL Update, optional SHACL pre-validation, RML mapping upload |
| `/shacl/shapes` | SHACL shapes editor — Turtle/SHACLC syntax, per-dataset shape browser, save and SHACL-AF inference |
| `/validation` | Validation dashboard — per-dataset validate buttons, severity filter chips, CSV/JSON export |
| `/datasets` | Dataset management — SHACL-on-write toggle, DCAT metadata, RML mappings (requires auth) |
| `/organisations` | Organisation management (requires auth) |
| `/settings` | Profile, password change, API token management (requires auth) |
| `/admin/users` | User management — create, edit role/status, reset password, deactivate (requires admin+) |

### Development

```bash
cd frontend
npm install
npm run dev       # starts on http://localhost:5173 (proxied to :7878)
npm run build     # production build → frontend/dist/
```

#### Service discovery (optional)

Cross-app discovery is **off by default** — the web UI talks to its own backend directly, so you
need nothing else to run it. Turn it on to resolve sibling apps (and self-register the backend)
through a service registry. It is an explicit opt-in via `LD_DISCOVERY`:

```bash
LD_DISCOVERY=1 npm run dev               # frontend: mount the /registry proxy (dev server)
LD_DISCOVERY=true docker compose up -d   # backend: self-register (or set LD_DISCOVERY in .env)
```

When enabled, point `LD_REGISTRY_URL` at your registry (default `http://localhost:8500`). When off,
the dev server makes no registry calls — so a registry that isn't running can't print
`[vite] http proxy error: /resolve` / `/events`.

---

## SPARQL Endpoint

### Query — `GET /sparql`

```bash
curl 'http://localhost:7878/sparql?query=SELECT+*+WHERE+{+?s+?p+?o+}+LIMIT+10'
curl -H 'Accept: application/sparql-results+json' \
     'http://localhost:7878/sparql?query=SELECT+*+WHERE+{+?s+?p+?o+}'
```

### Query / Update — `POST /sparql`

```bash
# Query
curl -X POST http://localhost:7878/sparql \
     -H 'Content-Type: application/sparql-query' \
     -d 'SELECT ?name WHERE { ?s <http://xmlns.com/foaf/0.1/name> ?name }'

# Update (requires write scope)
curl -X POST http://localhost:7878/sparql \
     -H 'Content-Type: application/sparql-update' \
     -H 'Authorization: Bearer ots_your_api_key' \
     -d 'INSERT DATA { <http://example.org/alice> <http://xmlns.com/foaf/0.1/name> "Alice" . }'
```

### Result formats

| Query type | `Accept` header | Format |
|---|---|---|
| SELECT / ASK | `application/sparql-results+json` *(default)* | SPARQL JSON |
| SELECT / ASK | `application/sparql-results+xml` | SPARQL XML |
| SELECT / ASK | `text/csv` | CSV |
| SELECT / ASK | `text/tab-separated-values` | TSV |
| CONSTRUCT / DESCRIBE | `text/turtle` *(default)* | Turtle |
| CONSTRUCT / DESCRIBE | `application/n-triples` | N-Triples |
| CONSTRUCT / DESCRIBE | `application/rdf+xml` | RDF/XML |

---

## Graph Store HTTP Protocol

```bash
# Load Turtle into a named graph
curl -X PUT 'http://localhost:7878/store?graph=http://example.org/g1' \
     -H 'Content-Type: text/turtle' \
     -H 'Authorization: Bearer ots_your_api_key' \
     -d '@prefix ex: <http://example.org/> . ex:a ex:b ex:c .'

# Read a named graph
curl 'http://localhost:7878/store?graph=http://example.org/g1'

# Merge into the default graph
curl -X POST 'http://localhost:7878/store?default' \
     -H 'Content-Type: text/turtle' \
     -d '<http://example.org/s> <http://example.org/p> "o" .'

# Delete a named graph
curl -X DELETE 'http://localhost:7878/store?graph=http://example.org/g1' \
     -H 'Authorization: Bearer ots_your_api_key'
```

---

## Auth API

### Login / Register

```bash
# Register (first user → super_admin, all others → user)
curl -X POST http://localhost:7878/api/auth/register \
     -H 'Content-Type: application/json' \
     -d '{"username":"alice","email":"alice@example.com","password":"s3cr3tpass"}'
# → {"access_token":"<jwt>","refresh_token":"<jwt>","expires_in":1800,"user":{...}}

# Login
curl -X POST http://localhost:7878/api/auth/login \
     -H 'Content-Type: application/json' \
     -d '{"username":"alice","password":"s3cr3tpass"}'
```

### Token refresh

```bash
curl -X POST http://localhost:7878/api/auth/refresh \
     -H 'Content-Type: application/json' \
     -d '{"refresh_token":"<jwt>"}'
# → new access_token + refresh_token (old refresh token is revoked)
```

### API keys

```bash
# Create (requires auth)
curl -X POST http://localhost:7878/api/auth/tokens \
     -H 'Authorization: Bearer <access_token>' \
     -H 'Content-Type: application/json' \
     -d '{"name":"my-script","scopes":["read","write"]}'
# → {"id":"...","token":"ots_abc123...","prefix":"ots_abc1","..."}
# Full token shown ONCE — store it securely

# List tokens
curl http://localhost:7878/api/auth/tokens \
     -H 'Authorization: Bearer <access_token>'

# Revoke
curl -X DELETE http://localhost:7878/api/auth/tokens/<token_id> \
     -H 'Authorization: Bearer <access_token>'
```

### Admin user management

```bash
# List users (admin+)
curl 'http://localhost:7878/api/admin/users?page=1&limit=20&search=alice' \
     -H 'Authorization: Bearer <admin_access_token>'

# Create user (admin+)
curl -X POST http://localhost:7878/api/admin/users \
     -H 'Authorization: Bearer <admin_access_token>' \
     -H 'Content-Type: application/json' \
     -d '{"username":"bob","email":"bob@example.com","password":"s3cr3tpass","role":"user"}'

# Update user (admin+)
curl -X PUT http://localhost:7878/api/admin/users/<user_id> \
     -H 'Authorization: Bearer <admin_access_token>' \
     -H 'Content-Type: application/json' \
     -d '{"email":"new@example.com","role":"admin","is_active":true}'

# Reset password (admin+)
curl -X POST http://localhost:7878/api/admin/users/<user_id>/reset-password \
     -H 'Authorization: Bearer <admin_access_token>' \
     -H 'Content-Type: application/json' \
     -d '{"new_password":"newpass123"}'

# Deactivate user (admin+) — revokes all tokens
curl -X DELETE http://localhost:7878/api/admin/users/<user_id> \
     -H 'Authorization: Bearer <admin_access_token>'
```

---

## Automatic Prefix Resolution

Write SPARQL without declaring prefixes — they resolve automatically via [prefix.cc](https://prefix.cc):

```sparql
SELECT ?name ?knows WHERE {
  ?person foaf:name ?name ;
          foaf:knows ?knows .
}
```

Resolved mappings are cached in `{data-dir}/prefix_cache.json`.

---

## GeoSPARQL 1.1

All 30 OGC requirements via GEOS bindings.

```sparql
PREFIX geo:  <http://www.opengis.net/ont/geosparql#>
PREFIX geof: <http://www.opengis.net/def/function/geosparql/>

SELECT ?feature WHERE {
  ?feature geo:hasGeometry/geo:asWKT ?wkt .
  FILTER(geof:sfWithin(?wkt, "POLYGON((0 0,10 0,10 10,0 10,0 0))"^^geo:wktLiteral))
}
```

| Family | Functions |
|---|---|
| Simple Features | `sfContains` `sfCrosses` `sfDisjoint` `sfEquals` `sfIntersects` `sfOverlaps` `sfTouches` `sfWithin` |
| Egenhofer | `ehContains` `ehCoveredBy` `ehCovers` `ehDisjoint` `ehEquals` `ehInside` `ehMeet` `ehOverlap` |
| RCC8 | `rcc8dc` `rcc8ec` `rcc8po` `rcc8tppi` `rcc8tpp` `rcc8ntpp` `rcc8ntppi` `rcc8eq` |
| Constructive | `boundary` `buffer` `convexHull` `difference` `envelope` `intersection` `symDifference` `union` |
| Metric | `distance` `area` `getSRID` |

---

## HTTP API Reference

### Auth

| Method | Path | Auth | Description |
|---|---|---|---|
| `POST` | `/api/auth/register` | — | Register (first → super_admin) |
| `POST` | `/api/auth/login` | — | Login → access + refresh tokens |
| `POST` | `/api/auth/refresh` | — | Rotate refresh token |
| `POST` | `/api/auth/logout` | Bearer | Revoke refresh token |
| `GET` | `/api/auth/me` | Bearer | Current user info |
| `GET` | `/api/auth/tokens` | Bearer | List API keys |
| `POST` | `/api/auth/tokens` | Bearer | Create API key |
| `DELETE` | `/api/auth/tokens/:id` | Bearer | Revoke API key |

### Admin (admin+ only)

| Method | Path | Description |
|---|---|---|
| `GET` | `/api/admin/users` | List users (paginated + search) |
| `POST` | `/api/admin/users` | Create user |
| `GET` | `/api/admin/users/:id` | Get user |
| `PUT` | `/api/admin/users/:id` | Update role/email/active |
| `DELETE` | `/api/admin/users/:id` | Deactivate user |
| `POST` | `/api/admin/users/:id/reset-password` | Reset password |

### SPARQL & Graph Store

| Method | Path | Auth | Description |
|---|---|---|---|
| `GET/POST` | `/sparql` | Optional | SPARQL query/update |
| `GET` | `/store?graph=…` | Optional | Read named graph |
| `PUT/POST` | `/store?graph=…` | Bearer (write) | Write named graph |
| `DELETE` | `/store?graph=…` | Bearer (write) | Delete named graph |

### Browse & Data

| Method | Path | Description |
|---|---|---|
| `GET` | `/api/browse/stats` | Triple and graph counts |
| `GET` | `/api/browse/graphs` | List named graphs |
| `GET` | `/api/browse/triples` | Paginated triple browser |
| `GET` | `/api/browse/resource?iri=…` | Resource neighbourhood |
| `GET` | `/api/datasets` | List datasets |
| `GET` | `/health` | Health check |

### SHACL

| Method | Path | Description |
|---|---|---|
| `POST` | `/api/datasets/:id/validate` | Validate dataset against shapes graph |
| `GET` | `/api/datasets/:id/shapes` | Get shapes graph (Turtle or `?format=shaclc`) |
| `PUT` | `/api/datasets/:id/shapes` | Upload shapes graph (Turtle or `Content-Type: text/shaclc`) |
| `POST` | `/api/datasets/:id/infer` | Run SHACL-AF inference, materialize triples |
| `POST` | `/api/shaclc/parse` | Convert SHACLC → Turtle (stateless) |
| `POST` | `/api/shaclc/serialize` | Convert shapes graph → SHACLC (body: IRI or JSON) |

### RML Mapping

| Method | Path | Auth | Description |
|---|---|---|---|
| `PUT` | `/api/datasets/:id/mappings` | Bearer | Store an RML mapping document (Turtle) |
| `GET` | `/api/datasets/:id/mappings` | Bearer | Retrieve stored RML mapping |
| `POST` | `/api/datasets/:id/mappings/execute` | Bearer | Execute mapping with source files (multipart); `?preview=true` for dry-run |
| `POST` | `/api/rml/preview` | — | Dry-run preview (multipart: `mapping` + source parts) |

### Linked Data & Catalog

| Method | Path | Description |
|---|---|---|
| `GET` | `/.well-known/void` | DCAT 2 catalog + VoID statistics (content-negotiated) |
| `GET` | `/resource/*path` | IRI dereference — RDF or 303 redirect to SPA |
| `GET` | `/api-docs/` | Swagger UI (OpenAPI 3.0) |
| `GET` | `/api-docs/openapi.json` | Raw OpenAPI spec |

---

## OpenAPI Documentation

Interactive API documentation is served at **`/api-docs/`** once the server is running. The raw OpenAPI 3.0 spec is available at `/api-docs/openapi.json`.

All endpoints are documented with parameters, response codes, and JWT Bearer security. You can try requests directly from the browser UI.

---

## SHACL Validation

### Validate a dataset on demand

```bash
# Validate dataset against its configured shapes graph
curl -X POST http://localhost:7878/api/datasets/<dataset_id>/validate \
     -H 'Authorization: Bearer <token>'
# → {"conforms": true/false, "results": [...], "results_count": N}
```

### Upload a shapes graph

```bash
# Upload Turtle shapes
curl -X PUT http://localhost:7878/api/datasets/<dataset_id>/shapes \
     -H 'Authorization: Bearer <token>' \
     -H 'Content-Type: text/turtle' \
     --data-binary @shapes.ttl

# Upload SHACL Compact Syntax (parsed to Turtle before storing)
curl -X PUT http://localhost:7878/api/datasets/<dataset_id>/shapes \
     -H 'Authorization: Bearer <token>' \
     -H 'Content-Type: text/shaclc' \
     --data-binary @shapes.shaclc
```

### SHACL validation on write

When `shacl_on_write` is enabled for a dataset, every `PUT` or `POST` to the Graph Store Protocol is automatically validated before the data is committed. Invalid data returns **422 Unprocessable Entity** with a JSON validation report.

Enable via the dataset settings UI or API:

```bash
curl -X PUT http://localhost:7878/api/datasets/<dataset_id> \
     -H 'Authorization: Bearer <token>' \
     -H 'Content-Type: application/json' \
     -d '{"shacl_on_write": true}'
```

### SHACL Compact Syntax (SHACLC)

Retrieve shapes in compact syntax:

```bash
curl http://localhost:7878/api/datasets/<dataset_id>/shapes?format=shaclc \
     -H 'Authorization: Bearer <token>'
# or: Accept: text/shaclc
```

Standalone conversion endpoints:

```bash
# SHACLC → Turtle
curl -X POST http://localhost:7878/api/shaclc/parse \
     -H 'Content-Type: text/shaclc' \
     --data-binary @shapes.shaclc

# Shapes graph IRI → SHACLC
curl -X POST http://localhost:7878/api/shaclc/serialize \
     -H 'Content-Type: application/json' \
     -d '{"shapesGraphIri": "urn:dataset:my-dataset:shapes"}'
```

See [docs/shacl.md](docs/shacl.md) for the full SHACL guide.

---

## DCAT 2 Catalog

The `/.well-known/void` endpoint returns a full **W3C DCAT 2** catalog including:

- `dcat:Catalog` with all registered datasets
- Per-dataset `dcat:Dataset` with distributions (SPARQL endpoint + Graph Store)
- VoID statistics (`void:triples`, `void:distinctSubjects`, `void:properties`) per dataset
- Organization metadata from dataset owners
- `dct:conformsTo` linking to SHACL shapes graphs where configured
- `sd:Service` SPARQL service description

```bash
# Turtle (default)
curl http://localhost:7878/.well-known/void

# JSON-LD
curl -H 'Accept: application/ld+json' http://localhost:7878/.well-known/void

# ?format= override
curl 'http://localhost:7878/.well-known/void?format=jsonld'
```

See [docs/dcat.md](docs/dcat.md) for the full DCAT 2 guide, and [docs/linked-data-modelling-styleguide.md](docs/linked-data-modelling-styleguide.md) for the canonical linked-data modelling standard (the "holy" styleguide covering SKOS/OWL/SHACL/DCAT/VoID/ADMS conventions, graph roles, IRIs and versioning).

---

## RML Mapping

Map tabular and semi-structured data (CSV, JSON, XML) to RDF using the [RDF Mapping Language](https://rml.io/specs/rml/).

### Store and execute a mapping

```bash
# 1. Upload a mapping document
curl -X PUT http://localhost:7878/api/datasets/<dataset_id>/mappings \
     -H 'Authorization: Bearer <token>' \
     -H 'Content-Type: text/turtle' \
     --data-binary @mapping.ttl

# 2. Execute with source files (multipart)
curl -X POST http://localhost:7878/api/datasets/<dataset_id>/mappings/execute \
     -H 'Authorization: Bearer <token>' \
     -F 'people=@people.csv' \
     -F 'orders=@orders.json'

# 3. Preview without committing
curl -X POST 'http://localhost:7878/api/datasets/<dataset_id>/mappings/execute?preview=true' \
     -H 'Authorization: Bearer <token>' \
     -F 'people=@people.csv'
```

### Standalone preview

```bash
curl -X POST http://localhost:7878/api/rml/preview \
     -F 'mapping=@mapping.ttl' \
     -F 'data=@data.csv'
# → {"triples_count": 42, "turtle": "..."}
```

### Example mapping (CSV)

```turtle
@prefix rr:  <http://www.w3.org/ns/r2rml#> .
@prefix rml: <http://semweb.mmlab.be/ns/rml#> .
@prefix ql:  <http://semweb.mmlab.be/ns/ql#> .
@prefix ex:  <http://example.org/> .

<#PersonMap>
  rml:logicalSource [
    rml:source "people.csv" ;
    rml:referenceFormulation ql:CSV
  ] ;
  rr:subjectMap [
    rr:template "http://example.org/person/{id}" ;
    rr:class ex:Person
  ] ;
  rr:predicateObjectMap [
    rr:predicate ex:name ;
    rr:objectMap [ rml:reference "name" ]
  ] ;
  rr:predicateObjectMap [
    rr:predicate ex:age ;
    rr:objectMap [
      rml:reference "age" ;
      rr:datatype <http://www.w3.org/2001/XMLSchema#integer>
    ]
  ] .
```

See [docs/rml.md](docs/rml.md) for the full RML guide including JSON and XML sources.

---

## OWL 2 DL Reasoning

Native OWL 2 DL support runs all ~80 OWL 2 RL forward-chaining rules plus DL-specific SPARQL rules for `owl:hasSelf`, `owl:disjointUnionOf`, `owl:NegativePropertyAssertion`, `owl:hasKey`, and cardinality annotations.  An `ExternalReasonerBridge` allows plugging in a full tableau reasoner (HermiT, Pellet, ELK) for ABox completion.

```bash
# Query with OWL 2 DL entailment
curl "http://localhost:7878/sparql?query=SELECT+*+WHERE+{+?s+a+?t+}&entailment=owl2-dl"
```

See [docs/owl2-dl.md](docs/owl2-dl.md) for the full OWL 2 DL guide.

---

## Linked Data Platform (LDP) 1.0

Full W3C LDP 1.0 implementation under `/ldp/` — Basic, Direct, and Indirect Containers, Non-RDF Sources, PATCH with SPARQL Update, `Prefer` header, ETag concurrency, and Constrained-By Link headers.

```bash
# Create a Basic Container
curl -X PUT http://localhost:7878/ldp/my-container/ \
  -H "Content-Type: text/turtle" \
  -d '<http://localhost/ldp/my-container/> a <http://www.w3.org/ns/ldp#BasicContainer> .'

# PATCH a resource
curl -X PATCH http://localhost:7878/ldp/my-container/item1 \
  -H "Content-Type: application/sparql-update" \
  -d 'INSERT DATA { <http://localhost/ldp/my-container/item1> <http://example.org/updated> true }'
```

See [docs/ldp.md](docs/ldp.md) for the full LDP 1.0 guide.

---

## Architecture

```
open-triplestore
├── src/
│   ├── auth/           RBAC auth — models, db, JWT, middleware, handlers
│   ├── store/          TripleStore (Oxigraph wrapper)
│   ├── server/         Axum HTTP server — routes, OpenAPI spec, error handling
│   │   ├── openapi.rs  Swagger UI + /api-docs/openapi.json
│   │   └── linked_data.rs  /.well-known/void (DCAT 2), /resource/* (dereference)
│   ├── shacl/          SHACL validation engine, SHACL-AF inference, reports
│   ├── shaclc/         SHACL Compact Syntax parser (SHACLC → Turtle) and serializer
│   ├── dcat/           DCAT 2 catalog generator (VoID stats, distributions, PROV-O)
│   ├── rml/            RDF Mapping Language executor
│   │   └── sources/    CSV, JSON (JSONPath), XML (XPath) source adapters
│   ├── geo/            GeoSPARQL function registry (GEOS bindings)
│   ├── prefixes/       prefix.cc resolver
│   └── sparql/         Service description generator
├── frontend/
│   ├── src/
│   │   ├── lib/        api.js, rdf-utils.js, sparql-mode.js, turtle-mode.js, stores.js
│   │   ├── components/ SparqlEditorCM, GraphCanvas, RdfTerm
│   │   └── pages/      One .svelte file per route
│   └── dist/           Production build (served by the Rust binary)
├── docs/               Feature guides (SHACL, DCAT 2, RML, performance, administration)
├── tests/              Conformance & benchmark test suites
├── benches/            Criterion performance benchmarks
└── scripts/            Test runners, W3C conformance tester
```

---

## Performance (Apple M3 Pro, release build)

| Operation | Dataset | Median |
|---|---|---|
| Bulk load | 100 K triples | 98 ms (~1 M t/s) |
| Simple SELECT | 10 K triples | 980 µs |
| Simple SELECT LIMIT 10 | 100 K triples | 22 µs |
| 2-way join | 10 K triples | 1.8 ms |
| COUNT(*) | 100 K triples | 14 ms |

---

## Conformance

| Test suite | Tests | Pass |
|---|---|---|
| W3C SPARQL 1.1 | 112 | **112** |
| W3C RDF 1.1 Formats | 63 | **63** |
| OGC GeoSPARQL 1.1 | 84 | **84** |
| SP2B / BSBM Benchmarks | 28 | **28** |
| sparqloscope | 67 | **67** |
| Unit / Integration | ~39 | **~36** (3 ignored: RocksDB arm64) |

---

## Development

```bash
# Backend
cargo build               # debug
cargo build --release     # optimised
cargo test                # all tests
cargo bench               # benchmarks (HTML report in target/criterion/)

# Frontend
cd frontend
npm install
npm run dev               # dev server → http://localhost:5173
npm run build             # production build

# Docker
docker build -t open-triplestore .
docker run --rm -p 7878:7878 -v ./data:/data open-triplestore
```

---

## Versioning & releases

Open Triplestore follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html)
with a [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) — see
[`CHANGELOG.md`](CHANGELOG.md). Branch model: **`develop`** is active development,
**`main`** is the latest stable release (tagged `vX.Y.Z`), and **`release/X.Y`**
branches carry maintenance fixes. Releases are tag-driven and publish a GHCR image:

```bash
docker pull ghcr.io/philipperenzen/open-triplestore:latest
```

See [`docs/release-process.md`](docs/release-process.md) for the full release flow and
[`SECURITY.md`](SECURITY.md) for supported versions and vulnerability reporting.

---

## License

**Open Triplestore is source-available, not OSI "open source".** It is licensed under the
**GNU Affero General Public License v3.0 with the Commons Clause** — see [`LICENSE`](LICENSE).

In short:

- ✅ **Free to use, self-host, study, and modify** — for anyone, including companies, at no cost.
- ✅ **Contribute back** — if you run a modified version as a network service, the AGPL (§ 13) requires you to make your changes available to its users.
- ❌ **No selling** — the Commons Clause forbids selling the software, offering it as a paid or hosted service, or charging for support whose value derives substantially from it.

If you need terms beyond these, contact the author.

## Legal & Privacy

Open Triplestore is **self-hostable** software: whoever deploys it is the operator
and data controller. The project ships **no telemetry** to the maintainers. The
following templates and guides are starting points only — **not legal advice** —
and should be reviewed with counsel and adapted to your deployment.

- [`LICENSE`](LICENSE) — AGPL-3.0 with the Commons Clause (source-available; no selling/paid hosting).
- [`PRIVACY.md`](PRIVACY.md) — privacy-notice template: data the software stores, the no-telemetry stance, and every optional external call plus how to disable it.
- [`TERMS.md`](TERMS.md) — terms-of-use template tied to the licence (AS-IS, no warranty, acceptable use, operator duties).
- [`docs/gdpr.md`](docs/gdpr.md) — GDPR guide mapping data-subject rights to product features (export, deletion, ACLs, audit log); guidance, not certification.
- [`SECURITY.md`](SECURITY.md) — supported versions and how to report a vulnerability privately.

Copyright © 2026 Open Triplestore.
