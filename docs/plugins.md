# Extending Open Triplestore

Open Triplestore is designed so a downstream operator (e.g. a private fork that
customizes branding, pre-loads its own data, or points the UI at a different
backend) can do all of that **without patching any upstream-owned source
file**. There are three independent extension tiers, from "drop a file next to
the binary" to "compile in a new endpoint." Use the lightest one that does the
job — most customization needs tier 1 or 2, not a code plugin.

| Tier | What it does | Needs a rebuild? | Where |
|---|---|---|---|
| 1. [Seed bundles](#tier-1-seed-bundles-data) | Boot-time org/dataset/data loading | No | `--seed-dir` / `SEED_DIR` |
| 2. [Runtime config](#tier-2-runtime-config-frontend) | Backend URLs + branding for the web UI | No | `/config.json` |
| 3. [Code plugins](#tier-3-code-plugins-compile-time) | New HTTP endpoints, background jobs | Yes (feature flag) | `plugins/<name>` |

All three are **additive**: an instance with none configured behaves exactly
like upstream. None of them require dynamic library loading (`dylib`/ABI
plugins) — that idiom fights Rust's compilation model and this project's
release process, so it is intentionally not used here.

---

## Tier 1: Seed bundles (data)

A seed bundle declares an organisation-owned dataset (or several), the named
graphs that make it up, and optional saved queries — pure data, no Rust code.
It is the generalization of the pattern this project already uses internally:
the bundled "Open Triplestore" standards demo (`src/saved_queries/seed.rs` +
`seed_data.rs`) is itself built as a `Bundle` and run through the exact same
engine (`src/seed_bundles::apply_bundle`), so this code path is exercised by
CI on every change, not just by a downstream fork's own bundles.

### Directory layout

Point `--seed-dir <path>` (or `SEED_DIR=<path>`) at a directory containing one
subdirectory per bundle:

```
my-bundles/
└── acme-catalog/
    ├── manifest.toml
    ├── model.ttl        # a single-graph payload (Turtle/N-Triples/RDF-XML/JSON-LD)
    └── instances.trig   # a multi-graph payload (TriG/N-Quads) — graphs are
                          # declared *inside* the file, not in the manifest
```

Every subdirectory with a `manifest.toml` is loaded; anything else is ignored.
A complete, runnable example (also the one CI exercises) lives in
[`examples/seed-bundles/bridge-reference/`](../examples/seed-bundles/bridge-reference).

### `manifest.toml` reference

```toml
id = "acme-catalog"                 # unique id; used in logs and the default opt-out env var
opt_out_env = "SEED_ACME_CATALOG"   # optional — defaults to SEED_BUNDLE_<ID>

[organisation]
slug = "acme"
name = "Acme Corp"
description = "Owns the Acme catalog datasets."

[[datasets]]
slug = "acme-catalog"               # becomes the dataset id AND its IRI path segment
name = "Acme Product Catalog"
description = "…"
visibility = "public"                # public | members | private

[[datasets.graphs]]
iri = "https://acme.example/catalog/model"
role = "model"                       # instances | model | vocabulary | shapes | entailment | system
file = "model.ttl"                   # omit `file` for a graph populated by a quads payload instead

[[datasets.quads]]
file = "instances.trig"

[[datasets.saved_queries]]
name = "All products"
slug = "all-products"
description = "…"
sparql = "SELECT ?p WHERE { ?p a <https://acme.example/catalog#Product> }"
```

### Semantics

- **Idempotent.** An existing organisation/dataset is never re-created; a
  named graph is only (re)loaded while it holds zero triples — a previous
  interrupted seed self-heals, but an admin's edits to the data are never
  overwritten.
- **Fail-soft.** A broken manifest, an unreadable payload, or a failed graph
  load is logged as a warning and skipped; it never aborts boot or the other
  bundles.
- **Per-bundle opt-out.** Set the bundle's `opt_out_env` (or the derived
  `SEED_BUNDLE_<ID>`) to `false`/`0`/`no`/`off` to disable one bundle without
  removing it from the seed directory.
- **Works with both ephemeral and persistent stores** — it runs at the same
  point in boot as the built-in demo seed, after the store is open.
- Payload paths in a manifest may not be absolute or contain `..` — a bundle
  can only read files inside its own directory.

### Docker: mounting extra bundles

The shipped image doesn't bake in any bundles beyond the reference one used in
tests. Mount a host (or CI-built) directory over `SEED_DIR` and point the
container at it:

```bash
docker run -v ./my-bundles:/seed-bundles \
  -e SEED_DIR=/seed-bundles \
  -p 7878:7878 ghcr.io/philipperenzen/open-triplestore:latest
```

or in `docker-compose.yml`:

```yaml
services:
  triplestore:
    image: ghcr.io/philipperenzen/open-triplestore:latest
    environment:
      SEED_DIR: /seed-bundles
    volumes:
      - ./my-bundles:/seed-bundles:ro
```

No image rebuild, no source patch — the fork's data lives entirely in the
mounted directory.

---

## Tier 2: Runtime config (frontend)

The web UI's backend URLs and branding (title, logo, accent color) can be set
**after the frontend is built**, so a container operator customizes a
deployment without rebuilding the SPA bundle.

### Precedence

For each backend URL the frontend needs (`triplestore`, `form-service`, …see
[`serviceRegistry.ts`](../frontend/src/lib/serviceRegistry.ts)), the effective
value is resolved highest-priority-first:

1. **`VITE_<SERVICE>_URL`** — a build-time environment variable (e.g.
   `VITE_TRIPLESTORE_URL=https://api.example.com`), baked into the bundle at
   `npm run build` time. For a static deploy with no reverse proxy in front
   (GitLab Pages, S3+CloudFront, …) that talks to an external backend.
2. **Runtime config** — `/config.json`'s `"services"` map (below). Changeable
   without a rebuild.
3. **`/registry` discovery** — the existing opt-in (`LD_DISCOVERY`) cross-app
   service-registry SSE resolution, for a multi-app deployment.
4. **localhost defaults** — the dev-mode fallback ports.

### `/config.json`

This app's backend already serves the built frontend from `frontend/dist` via
a static-file layer with an SPA fallback (`src/server/mod.rs`). That means a
`config.json` dropped straight into that directory — e.g. a Docker volume
mounted over the built image, or a file alongside `index.html` on a static
host — is served with **zero backend code changes and zero rebuild**:

```json
{
  "services": {
    "triplestore": "https://api.example.com"
  },
  "branding": {
    "title": "Acme Graph",
    "logoUrl": "/acme-logo.svg",
    "accent": "#7a2fe0"
  }
}
```

Both top-level keys are optional; anything omitted keeps its existing
default/registry value. The frontend fetches this once at boot
([`runtimeConfig.ts`](../frontend/src/lib/runtimeConfig.ts)) and applies it
immediately — `branding.title` becomes the page `<title>` and sidebar
wordmark, `branding.logoUrl` replaces the built-in ring mark and the browser
tab favicon, and `branding.accent` overrides the app's primary brand color
(`--brand-600` in `theme.css`). No file present is a complete no-op (the
fetch either 404s or, on this app's own backend, hits the SPA fallback and
returns HTML instead of JSON — the frontend detects that by content-type and
ignores it either way).

### Docker example

```yaml
services:
  triplestore:
    image: ghcr.io/philipperenzen/open-triplestore:latest
    volumes:
      - ./config.json:/app/frontend/dist/config.json:ro
```

### Base path (static sub-path deploys)

If the built frontend is published under a sub-path (e.g.
`https://example.gitlab.io/otl-suite/`), set `OTS_BASE_PATH` at **build** time:

```bash
OTS_BASE_PATH=/otl-suite/ npm run build
```

Defaults to `/`, so every existing deployment is unaffected.

---

## Tier 3: Code plugins (compile-time)

For anything a data bundle or runtime config can't express — a new HTTP
endpoint, a background task, custom business logic — write a plugin crate.
This is the same idiom the codebase already uses for optional capabilities
(`rdfs-entailment`, `owl2-*`, `text-search`, `ldp`, … in `Cargo.toml`
`[features]`), extended to third-party code: **compile-time, feature-gated,
off by default**. There is deliberately no dynamic (`dylib`) loading.

### The `Plugin` trait

Plugins are written against [`ots-plugin-api`](../plugins/api), a small crate
with **no dependency on `open-triplestore` itself** — see that crate's docs
for why (in short: a plugin crate that depended on `open-triplestore` while
`open-triplestore` depends on the plugin would be a package-level dependency
cycle, which Cargo forbids). Instead, a plugin gets a narrow capability
object:

```rust
pub trait Plugin: Send + Sync + 'static {
    fn name(&self) -> &'static str;               // -> mounted at /ext/<name>
    fn version(&self) -> &'static str;
    fn routes(&self) -> axum::Router<PluginContext> { axum::Router::new() }
    fn on_boot(&self, ctx: &PluginContext) {}
    fn spawn_background(&self, ctx: PluginContext) {}
}
```

`PluginContext` gives a plugin the instance's `base_url` and two capability
objects, both in the same plain-strings idiom so the plugin crate needs no
dependency on this project's internal types:

- `store: Arc<dyn PluginStore>` — `query_json` / `update`, SPARQL against the
  shared store.
- `auth: Arc<dyn PluginAuth>` *(ots-plugin-api 0.2)* — accounts:
  `introspect_bearer` resolves a caller's session JWT / `ots_` API token /
  provider access token to a principal JSON (id, username, email, role,
  org + group memberships), and three **admin-gated** overviews
  (`users_json`, `organisations_json`, `llm_stats_json`) back account
  dashboards. Authorization runs INSIDE the host implementation — a plugin
  can neither skip the checks nor see more than the equivalent admin API
  returns. `ots_plugin_api::NoAuth` is the inert stand-in for unit tests;
  `plugins/hello`'s `GET /ext/hello/whoami` is the minimal example and
  `plugins/accounts-dashboard` (feature `plugin-accounts-dashboard`) a full
  consumer: a suite-wide accounts/entitlements/LLM-usage dashboard at
  `/ext/accounts-dashboard/ui`.

### Writing a new plugin (cookiecutter flow)

1. Copy the template crate: `cp -r plugins/hello plugins/my-plugin`.
2. Rename the package in `plugins/my-plugin/Cargo.toml`:
   `name = "ots-plugin-my-plugin"`.
3. Add it to the root `Cargo.toml`:
   ```toml
   [dependencies]
   ots-plugin-my-plugin = { path = "plugins/my-plugin", optional = true }

   [features]
   plugin-my-plugin = ["dep:ots-plugin-my-plugin"]
   ```
   (`plugins/*` is already a workspace-member glob, so the crate itself needs
   no membership edit.)
4. Register an instance in `src/plugins.rs`'s `registered_plugins()`, gated by
   `#[cfg(feature = "plugin-my-plugin")]`.
5. `cargo build --features plugin-my-plugin` and hit `/ext/my-plugin`.

`plugins/hello` (`ots-plugin-hello`, feature `plugin-hello`) is both a working
example — `GET /ext/hello` and `GET /ext/hello/info` (the latter runs a real
SPARQL query through `PluginContext::store` to count named graphs) — and the
literal template to copy.

### Discovering what's enabled

`GET /api/plugins` lists every plugin compiled into the running binary
(`name`/`version`), so an operator can see what's enabled without inspecting
build flags.

### Running a customized instance

Putting all three tiers together, a downstream fork's *entire* divergence from
upstream can typically be expressed as: a `--seed-dir` of bundles, a
`config.json`, and (only if needed) one or two `plugin-*` features turned on
at build time — with zero patches to any file under `src/` or `frontend/src/`.
The upstreamed port-fallback flag (`--port-fallback` / `PORT_FALLBACK`,
default off) is a related, smaller piece of the same story: when set, a busy
`--port` falls back to any free port instead of refusing to start, and the
advertised base URL used for service-registry self-registration
(`--discovery` / `LD_DISCOVERY`) is rewritten to match — see
`src/netutil.rs`.

---

## Plugin promotion: from fork to core

A plugin doesn't have to stay a fork's private code forever. The path into
core is a normal PR:

1. **Build it as a `plugins/<name>` crate**, following the cookiecutter flow
   above, in your own fork or a feature branch.
2. **Open a PR against `develop`** adding the crate under `plugins/`, the
   `plugin-<name>` feature flag, and the `registered_plugins()` entry —
   exactly like any other change (see [`CONTRIBUTING.md`](../CONTRIBUTING.md)).
3. **Acceptance criteria** for the PR to be merged as an off-by-default
   plugin:
   - Feature-gated and **off by default** — `cargo build` / `cargo test` with
     no extra flags must be completely unaffected.
   - Passes CI at both default features and with the plugin's feature enabled
     (clippy, tests, `cargo fmt`).
   - Has its own unit tests (the plugin's routes and any non-trivial logic).
   - Has a short doc comment/README explaining what it does and its
     `/ext/<name>` surface, and a `CHANGELOG.md` entry.
   - Does not require patching any file outside `plugins/<name>/`, its
     `Cargo.toml`/`[features]` entry, and its `registered_plugins()` line.
   - No new mandatory runtime dependency (e.g. an external service) unless the
     plugin is explicitly about integrating with one — and even then, it must
     fail soft (log + continue) when that dependency is unreachable, matching
     this codebase's existing conventions (see e.g. `svc_registry`, `backup`).
4. **Graduating into core** — once a plugin is broadly useful, well-tested,
   and the maintainer agrees it belongs in the main product rather than as an
   opt-in extra, its code can be moved from `plugins/<name>` into the core
   crate (dropping the `Plugin` trait indirection and feature gate) in a
   follow-up PR. This is a normal refactor, not a special process — the
   plugin's existing tests are the main thing that needs to keep passing.

Seed bundles and runtime config don't go through this process at all — they
are pure data/config, not code, so there is nothing to "promote"; a bundle a
downstream operator finds broadly useful can simply be proposed as a PR adding
it under `examples/seed-bundles/` or, if universally relevant, folded into the
built-in standards demo.
