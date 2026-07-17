//! Generic seed/init bundles — boot-time data plugins, no recompile needed.
//!
//! A *seed bundle* declares an organisation-owned dataset (or several) plus the
//! named graphs and optional saved queries that make it up. Bundles come from
//! two places, both executed by the same [`apply_bundle`] engine:
//!
//! * **Built-in** — the bundled standards demo ([`crate::saved_queries::seed`])
//!   constructs its [`Bundle`] programmatically from embedded data, so this code
//!   path is exercised on every boot and in CI.
//! * **On disk** — `--seed-dir` / `SEED_DIR` points at a directory whose
//!   subdirectories each hold a `manifest.toml` plus RDF payload files
//!   (Turtle / N-Triples / RDF/XML / JSON-LD per graph, TriG / N-Quads for
//!   multi-graph payloads). See [`manifest`] and `docs/plugins.md`; a reference
//!   bundle ships in `examples/seed-bundles/`.
//!
//! Execution is **idempotent** (existing orgs/datasets are kept, a graph is
//! only loaded while empty, saved queries are only created when the dataset has
//! none) and **fail-soft** (a broken bundle is logged and skipped — boot never
//! aborts). Each bundle can be disabled with its opt-out env var.

pub mod manifest;

use std::borrow::Cow;
use std::path::Path;

use oxigraph::io::{JsonLdProfileSet, RdfFormat};
use uuid::Uuid;

use crate::auth::models::{GraphKind, OwnerType, Role, SystemRole, Visibility};
use crate::auth::{dataset_graph, org_graph};
use crate::saved_queries::metadata;
use crate::saved_queries::models::{CreateSavedQueryRequest, QueryScope};
use crate::saved_queries::store::SavedQueryStore;
use crate::server::AppState;

/// Serialization of a bundled graph payload.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fmt {
    Turtle,
    NTriples,
    RdfXml,
    JsonLd,
    /// Turtle-star body wrapped in a SPARQL-star `INSERT DATA` — used for the
    /// RDF-star demo graph so it loads through the (rdf-12) update path rather
    /// than the file parser. Not available to manifest bundles.
    SparqlStarUpdate,
}

impl Fmt {
    pub fn rdf_format(self) -> Option<RdfFormat> {
        match self {
            Fmt::Turtle => Some(RdfFormat::Turtle),
            Fmt::NTriples => Some(RdfFormat::NTriples),
            Fmt::RdfXml => Some(RdfFormat::RdfXml),
            Fmt::JsonLd => Some(RdfFormat::JsonLd {
                profile: JsonLdProfileSet::empty(),
            }),
            Fmt::SparqlStarUpdate => None,
        }
    }
}

/// The organisation a bundle's datasets are owned by. Created if missing;
/// matched by `slug` (and left untouched) if it already exists.
pub struct OrgSpec {
    pub slug: String,
    pub name: String,
    pub description: Option<String>,
}

/// One named graph in a bundle dataset. `data` is `None` for graphs that are
/// only *registered* to the dataset (their triples arrive via a
/// [`QuadsPayload`] or some external process).
pub struct BundleGraph {
    pub iri: String,
    pub role: Option<GraphKind>,
    pub data: Option<(Cow<'static, str>, Fmt)>,
}

/// A multi-graph payload (TriG / N-Quads) loaded as-is — the named graphs are
/// declared inside the file. Loaded only while at least one of the dataset's
/// declared graphs is still empty (RDF set semantics make a re-load a no-op).
pub struct QuadsPayload {
    /// Short label for log lines (the file name for manifest bundles).
    pub label: String,
    pub data: String,
    pub format: RdfFormat,
}

/// One dataset owned by the bundle's organisation.
pub struct BundleDataset {
    /// Dataset id AND URL slug (the minted IRI is `{base}/dataset/{slug}`).
    pub slug: String,
    pub name: String,
    pub description: Option<String>,
    pub visibility: Visibility,
    pub graphs: Vec<BundleGraph>,
    pub quads: Vec<QuadsPayload>,
    pub saved_queries: Vec<CreateSavedQueryRequest>,
}

/// A complete seed bundle: one organisation owning one or more datasets.
pub struct Bundle {
    /// Unique bundle id (used in logs and the default opt-out env var name).
    pub id: String,
    /// Env var that disables this bundle when set to `false`/`0`/`no`/`off`
    /// (same convention as `SEED_STANDARDS_DEMO`). `None` ⇒ derived as
    /// `SEED_BUNDLE_<ID>` (uppercased, `-` → `_`).
    pub opt_out_env: Option<String>,
    pub org: OrgSpec,
    pub datasets: Vec<BundleDataset>,
}

impl Bundle {
    /// The effective opt-out env var name for this bundle.
    pub fn opt_out_env_name(&self) -> String {
        self.opt_out_env.clone().unwrap_or_else(|| {
            format!(
                "SEED_BUNDLE_{}",
                self.id.to_ascii_uppercase().replace('-', "_")
            )
        })
    }

    /// True when the bundle's opt-out env var disables it.
    pub fn is_disabled(&self) -> bool {
        std::env::var(self.opt_out_env_name())
            .map(|v| {
                matches!(
                    v.trim().to_ascii_lowercase().as_str(),
                    "false" | "0" | "no" | "off"
                )
            })
            .unwrap_or(false)
    }
}

/// What [`apply_bundle`] did — lets callers (the demo seed's branding pass,
/// boot logging) react to what was actually created vs. already present.
#[derive(Default)]
pub struct SeedReport {
    pub org_id: String,
    pub org_created: bool,
    /// The admin user owner-attributed content (services, membership) was
    /// attributed to — `None` on a brand-new install with no admin yet, in
    /// which case that content is deferred to the next (re)seed.
    pub owner_id: Option<String>,
    /// Slugs of datasets created by THIS run (not pre-existing ones).
    pub datasets_created: Vec<String>,
    /// Graphs registered to datasets (created or verified).
    pub graphs_registered: usize,
    /// Graphs whose data was loaded into a pre-existing dataset (an earlier
    /// interrupted seed left them registered but empty).
    pub graphs_backfilled: usize,
    /// Graph payloads actually loaded (fresh or back-filled).
    pub graphs_loaded: usize,
    /// Saved-query services created by this run.
    pub services_created: usize,
}

/// Execute one bundle against the store + identity DB. Idempotent and
/// best-effort per item: a failing dataset/graph/service is logged and skipped
/// without aborting the rest. Callers decide whether the bundle runs at all
/// ([`Bundle::is_disabled`]).
pub fn apply_bundle(state: &AppState, bundle: &Bundle) -> anyhow::Result<SeedReport> {
    let mut report = SeedReport::default();

    // Owner of owner-attributed content (saved-query services, org membership):
    // a super_admin (preferred) or any active admin. Everything PUBLIC — the org,
    // datasets, graphs, metadata — is created regardless, so it appears on first
    // boot before anyone registers; owner-attributed content is back-filled
    // idempotently on a later (re)seed once an admin exists.
    let owner = {
        let users = state.auth_db.list_users()?;
        users
            .iter()
            .find(|u| matches!(u.role, SystemRole::SuperAdmin) && u.is_active)
            .or_else(|| users.iter().find(|u| u.role.is_admin() && u.is_active))
            .cloned()
    };
    report.owner_id = owner.as_ref().map(|o| o.id.clone());

    // Resolve or create the organisation. Never bails when it exists: the whole
    // bundle re-runs idempotently every boot and back-fills whatever is missing
    // without clobbering admin edits.
    let org_id = match state.auth_db.get_organisation_by_slug(&bundle.org.slug)? {
        Some(existing) => existing.id,
        None => {
            let org_id = Uuid::new_v4().to_string();
            let org = state.auth_db.create_organisation(
                &org_id,
                &bundle.org.name,
                &bundle.org.slug,
                bundle.org.description.as_deref(),
                None,
            )?;
            org_graph::write_org_metadata_graph(&state.store, &state.base_url, &org, &[]);
            report.org_created = true;
            org_id
        }
    };
    report.org_id = org_id.clone();

    // Make the first admin an Admin member of the org (INSERT OR REPLACE — safe
    // whether creating or back-filling once the first admin exists).
    if let Some(ref owner) = owner {
        let _ = state
            .auth_db
            .add_org_member(&owner.id, &org_id, Role::Admin);
    }

    let sq = SavedQueryStore::new(state.auth_db.pool());

    for ds in &bundle.datasets {
        let existed = matches!(state.auth_db.get_dataset(&ds.slug), Ok(Some(_)));
        if !existed {
            if let Err(e) = state.auth_db.create_dataset(
                &ds.slug,
                &ds.name,
                ds.description.as_deref(),
                OwnerType::Organisation,
                &org_id,
                ds.visibility,
                None,
            ) {
                tracing::warn!(
                    "seed bundle '{}': dataset '{}' create failed: {e}",
                    bundle.id,
                    ds.slug
                );
                continue;
            }
            report.datasets_created.push(ds.slug.clone());
        }

        // Track whether any declared graph is still empty — the trigger for
        // (re)loading this dataset's multi-graph quads payloads.
        let mut any_graph_empty = false;

        for g in &ds.graphs {
            // (Re)load the bundled data only while the target graph is empty:
            // a fresh seed, or a previous seed that registered the graph but
            // never populated it. A graph that already holds triples is left
            // untouched, so an admin's edits are never overwritten.
            let is_empty = state.store.graph_count_cached(Some(&g.iri)).unwrap_or(0) == 0;
            if is_empty {
                any_graph_empty = true;
                if let Some((data, fmt)) = &g.data {
                    if let Err(e) = load_graph(state, &g.iri, data, *fmt) {
                        tracing::warn!(
                            "seed bundle '{}': graph <{}> load failed: {e}",
                            bundle.id,
                            g.iri
                        );
                        continue;
                    }
                    report.graphs_loaded += 1;
                    if existed {
                        report.graphs_backfilled += 1;
                    }
                }
            }
            let _ = state.auth_db.add_dataset_graph(&ds.slug, &g.iri);
            if g.role.is_some() {
                let _ = state
                    .auth_db
                    .set_dataset_graph_role(&ds.slug, &g.iri, g.role);
            }
            report.graphs_registered += 1;
        }

        // Multi-graph payloads (TriG / N-Quads). Loaded only while a declared
        // graph is still empty; re-loading identical triples is a no-op under
        // RDF set semantics, so a partial earlier load self-heals.
        if any_graph_empty || ds.graphs.is_empty() {
            for q in &ds.quads {
                if let Err(e) = state.store.load_str(&q.data, q.format, None) {
                    tracing::warn!(
                        "seed bundle '{}': quads payload '{}' load failed: {e}",
                        bundle.id,
                        q.label
                    );
                    continue;
                }
                report.graphs_loaded += 1;
            }
        }

        // Project the dataset's DCAT/VoID metadata graph so it is discoverable
        // as linked data (mirrors the bulk-import path).
        if let Ok(Some(dsrec)) = state.auth_db.get_dataset(&ds.slug) {
            let entries = state
                .auth_db
                .list_dataset_graph_entries(&ds.slug)
                .unwrap_or_default();
            dataset_graph::write_dataset_metadata_graph(
                &state.store,
                &state.base_url,
                &dsrec,
                &entries,
            );
        }

        // Saved-query services are attributed to the owning admin, so they wait
        // until one exists (back-filled on the reseed after the first admin
        // registers). Created only when the dataset has none yet, so a reseed
        // on every boot doesn't pile up duplicates.
        if let Some(ref owner) = owner {
            let has_services = sq
                .list(QueryScope::Dataset, &ds.slug)
                .map(|v| !v.is_empty())
                .unwrap_or(false);
            if !has_services {
                for req in &ds.saved_queries {
                    match sq.create(QueryScope::Dataset, &ds.slug, req, &owner.id) {
                        Ok(svc) => {
                            metadata::record_service(&state.store, &state.base_url, &svc);
                            metadata::record_revision(
                                &state.store,
                                &state.base_url,
                                &svc.id,
                                svc.current_revision,
                                req.version_name.as_deref(),
                                req.note.as_deref(),
                                svc.sparql.as_deref().unwrap_or(&req.sparql),
                                "manual",
                                &svc.created_by,
                                &svc.created_at,
                            );
                            report.services_created += 1;
                        }
                        Err(e) => {
                            tracing::warn!(
                                "seed bundle '{}': service '{}' failed: {e}",
                                bundle.id,
                                req.name
                            )
                        }
                    }
                }
            }
        }
    }

    // Loaded graph data changes what each principal can see/count — drop the
    // accessible-graph cache so it reflects immediately rather than after TTL.
    if report.graphs_loaded > 0 {
        state.auth_db.invalidate_accessible_graphs_cache();
        #[cfg(feature = "text-search")]
        state.mark_text_dirty();
    }

    Ok(report)
}

/// Load one graph's payload into its named graph. Quoted-triple data goes
/// through a SPARQL-star `INSERT DATA`; everything else through the Graph
/// Store PUT path in its declared serialization.
fn load_graph(state: &AppState, graph_iri: &str, data: &str, fmt: Fmt) -> anyhow::Result<()> {
    match fmt {
        Fmt::SparqlStarUpdate => {
            state
                .store
                .update(&format!(
                    "INSERT DATA {{ GRAPH <{graph_iri}> {{ {data} }} }}"
                ))
                .map_err(|e| anyhow::anyhow!("{e}"))?;
        }
        other => {
            let format = other
                .rdf_format()
                .expect("non-update formats always map to an RdfFormat");
            state
                .store
                .graph_store_put(Some(graph_iri), data, format)
                .map_err(|e| anyhow::anyhow!("{e}"))?;
        }
    }
    Ok(())
}

/// Load every bundle under `dir` (each subdirectory holding a `manifest.toml`),
/// in lexicographic order. Entirely fail-soft: a missing directory, unreadable
/// manifest or failing bundle is logged and skipped — boot always continues.
pub fn load_seed_dir(state: &AppState, dir: &Path) {
    if !dir.is_dir() {
        tracing::info!(
            "seed bundles: directory {:?} does not exist — nothing to load",
            dir
        );
        return;
    }
    let mut bundle_dirs: Vec<_> = match std::fs::read_dir(dir) {
        Ok(entries) => entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect(),
        Err(e) => {
            tracing::warn!("seed bundles: cannot read {:?}: {e}", dir);
            return;
        }
    };
    bundle_dirs.sort();

    for bundle_dir in bundle_dirs {
        let manifest_path = bundle_dir.join("manifest.toml");
        if !manifest_path.is_file() {
            tracing::debug!("seed bundles: skipping {:?} — no manifest.toml", bundle_dir);
            continue;
        }
        let bundle = match manifest::parse_bundle(&bundle_dir) {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!("seed bundles: {:?} skipped (invalid): {e:#}", bundle_dir);
                continue;
            }
        };
        if bundle.is_disabled() {
            tracing::info!(
                "seed bundles: '{}' disabled via {} — skipping",
                bundle.id,
                bundle.opt_out_env_name()
            );
            continue;
        }
        match apply_bundle(state, &bundle) {
            Ok(r) => tracing::info!(
                "seed bundle '{}': org {} ({}), {} dataset(s) created, {} graph(s) registered, {} payload(s) loaded, {} service(s)",
                bundle.id,
                r.org_id,
                if r.org_created { "created" } else { "existing" },
                r.datasets_created.len(),
                r.graphs_registered,
                r.graphs_loaded,
                r.services_created
            ),
            Err(e) => tracing::warn!("seed bundle '{}' failed (fail-soft): {e:#}", bundle.id),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::TripleStore;
    use serde_json::json;

    fn test_state() -> AppState {
        AppState::test_default_with_store(TripleStore::in_memory().unwrap())
    }

    fn tiny_bundle(id: &str) -> Bundle {
        Bundle {
            id: id.to_string(),
            opt_out_env: None,
            org: OrgSpec {
                slug: format!("{id}-org"),
                name: "Test Org".into(),
                description: Some("A bundle test org".into()),
            },
            datasets: vec![BundleDataset {
                slug: format!("{id}-ds"),
                name: "Test Dataset".into(),
                description: None,
                visibility: Visibility::Public,
                graphs: vec![BundleGraph {
                    iri: format!("https://example.org/{id}/model"),
                    role: Some(GraphKind::Model),
                    data: Some((
                        Cow::Borrowed(
                            "@prefix ex: <https://example.org/ns#> .\n\
                             ex:Bridge a <http://www.w3.org/2002/07/owl#Class> .",
                        ),
                        Fmt::Turtle,
                    )),
                }],
                quads: vec![],
                saved_queries: vec![CreateSavedQueryRequest {
                    name: "All statements".into(),
                    slug: Some("all".into()),
                    description: None,
                    sparql: "SELECT ?s ?p ?o WHERE { ?s ?p ?o } LIMIT 10".into(),
                    parameters: Vec::new(),
                    test_parameters: Some(json!({})),
                    visibility: None,
                    version_name: None,
                    note: None,
                }],
            }],
        }
    }

    #[test]
    fn applies_and_is_idempotent() {
        let state = test_state();
        let b = tiny_bundle("idem");

        let r1 = apply_bundle(&state, &b).unwrap();
        assert!(r1.org_created);
        assert_eq!(r1.datasets_created, vec!["idem-ds".to_string()]);
        assert_eq!(r1.graphs_loaded, 1);

        let ds = state.auth_db.get_dataset("idem-ds").unwrap().unwrap();
        assert!(matches!(ds.visibility, Visibility::Public));
        let count = state
            .store
            .graph_count_cached(Some("https://example.org/idem/model"))
            .unwrap_or(0);
        assert!(count > 0, "model graph holds triples");

        // Second run: nothing is re-created or re-loaded.
        let r2 = apply_bundle(&state, &b).unwrap();
        assert!(!r2.org_created);
        assert!(r2.datasets_created.is_empty());
        assert_eq!(r2.graphs_loaded, 0);
        assert_eq!(
            state
                .store
                .graph_count_cached(Some("https://example.org/idem/model"))
                .unwrap_or(0),
            count,
            "graph untouched on reseed"
        );
    }

    #[test]
    fn saved_queries_deferred_until_admin_exists_then_backfilled_once() {
        let state = test_state();
        let b = tiny_bundle("svc");

        apply_bundle(&state, &b).unwrap();
        let sq = SavedQueryStore::new(state.auth_db.pool());
        assert!(
            sq.list(QueryScope::Dataset, "svc-ds")
                .unwrap_or_default()
                .is_empty(),
            "services deferred with no admin"
        );

        state
            .auth_db
            .create_user("u1", "admin", "a@x.test", "hash", SystemRole::SuperAdmin)
            .unwrap();
        let r = apply_bundle(&state, &b).unwrap();
        assert_eq!(r.services_created, 1);
        // Third run must not duplicate.
        let r = apply_bundle(&state, &b).unwrap();
        assert_eq!(r.services_created, 0);
        assert_eq!(
            sq.list(QueryScope::Dataset, "svc-ds").unwrap().len(),
            1,
            "exactly one service after repeated reseeds"
        );
    }

    #[test]
    fn quads_payload_loads_named_graphs_declared_in_file() {
        let state = test_state();
        let mut b = tiny_bundle("quads");
        b.datasets[0].graphs = vec![BundleGraph {
            iri: "https://example.org/quads/instances".into(),
            role: Some(GraphKind::Instances),
            data: None,
        }];
        b.datasets[0].quads = vec![QuadsPayload {
            label: "instances.trig".into(),
            data: "@prefix ex: <https://example.org/ns#> .\n\
                   GRAPH <https://example.org/quads/instances> { ex:b1 a ex:Bridge . }"
                .into(),
            format: RdfFormat::TriG,
        }];

        apply_bundle(&state, &b).unwrap();
        assert!(
            state
                .store
                .graph_count_cached(Some("https://example.org/quads/instances"))
                .unwrap_or(0)
                > 0,
            "TriG-declared graph was loaded and counted"
        );
    }

    #[test]
    fn opt_out_env_var_disables_bundle() {
        let b = tiny_bundle("optout");
        assert_eq!(b.opt_out_env_name(), "SEED_BUNDLE_OPTOUT");
        assert!(!b.is_disabled());
        std::env::set_var("SEED_BUNDLE_OPTOUT", "false");
        assert!(b.is_disabled());
        std::env::set_var("SEED_BUNDLE_OPTOUT", "true");
        assert!(!b.is_disabled());
        std::env::remove_var("SEED_BUNDLE_OPTOUT");
    }

    /// The reference bundle shipped in `examples/seed-bundles/` parses and
    /// applies cleanly — this is the CI exercise of the on-disk path.
    #[test]
    fn reference_bundle_loads_from_examples_dir() {
        let state = test_state();
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/seed-bundles");
        load_seed_dir(&state, &dir);

        let ds = state
            .auth_db
            .get_dataset("bridge-reference")
            .unwrap()
            .expect("reference bundle dataset seeded");
        assert!(matches!(ds.visibility, Visibility::Public));
        assert!(
            state
                .store
                .graph_count_cached(Some("https://example.org/seed-bundles/bridge/model"))
                .unwrap_or(0)
                > 0,
            "model graph loaded from Turtle payload"
        );
        assert!(
            state
                .store
                .graph_count_cached(Some("https://example.org/seed-bundles/bridge/instances"))
                .unwrap_or(0)
                > 0,
            "instances graph loaded from TriG payload"
        );
    }

    /// A directory with a broken manifest must not abort the others (fail-soft).
    #[test]
    fn broken_bundle_is_skipped_without_aborting_others() {
        let state = test_state();
        let tmp = tempfile::tempdir().unwrap();

        // 1) a broken bundle (invalid TOML), sorted first
        let bad = tmp.path().join("a-bad");
        std::fs::create_dir(&bad).unwrap();
        std::fs::write(bad.join("manifest.toml"), "this is [not toml").unwrap();

        // 2) a valid bundle
        let good = tmp.path().join("b-good");
        std::fs::create_dir(&good).unwrap();
        std::fs::write(
            good.join("manifest.toml"),
            r#"
id = "good"

[organisation]
slug = "good-org"
name = "Good Org"

[[datasets]]
slug = "good-ds"
name = "Good Dataset"
visibility = "public"

[[datasets.graphs]]
iri = "https://example.org/good/model"
role = "model"
file = "model.ttl"
"#,
        )
        .unwrap();
        std::fs::write(
            good.join("model.ttl"),
            "<https://example.org/x> a <https://example.org/T> .",
        )
        .unwrap();

        load_seed_dir(&state, tmp.path());
        assert!(
            matches!(state.auth_db.get_dataset("good-ds"), Ok(Some(_))),
            "valid bundle applied despite the broken sibling"
        );
    }
}
