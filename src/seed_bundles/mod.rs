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
    /// `(data model id, version)` this dataset conforms to.
    pub conforms_to: Option<(String, Option<String>)>,
    /// Shape-graph IRIs to register in the Studio library and bind to the dataset.
    pub shape_graphs: Vec<String>,
}

/// A reference model shipped by a bundle (see `[[data_models]]`).
pub struct BundleDataModel {
    pub id: String,
    pub title: String,
    pub namespace: String,
    pub description: Option<String>,
    pub kind: crate::kind_detector::RegistryKind,
    pub version: String,
    /// First graph = the version's base graph, the rest its sub-graphs.
    pub graphs: Vec<BundleGraph>,
    /// The licence of the model's content, when it is a third party's work
    /// (`[data_models.license]`). Recorded on the model's registry entry and
    /// version; see [`record_bundle_licence`].
    pub license: Option<BundleLicense>,
    /// Whether the registry entry is created public.
    pub public: bool,
}

/// The licence and attribution a bundle declares for one of its models.
pub struct BundleLicense {
    /// `(name, uri)` of each licence.
    pub licenses: Vec<(String, String)>,
    pub copyright: Vec<String>,
    pub source: String,
    pub notice: Option<String>,
    pub changes: Option<String>,
    pub remarks: Option<String>,
    pub notice_url: String,
    /// The rights holder allows no altered copies.
    pub no_derivatives: bool,
    /// The payload file of each graph, in graph order.
    pub files: Vec<String>,
}

impl BundleLicense {
    /// The licence record's `file`: which bundle and files the content comes
    /// from. [`Self::is_record_of`] recognises a record written for a bundle.
    fn record_file(&self, bundle_id: &str) -> String {
        format!("seed bundle {bundle_id}: {}", self.files.join(", "))
    }

    /// Whether a licence record's `file` is one written for bundle `bundle_id`.
    fn is_record_of(file: &str, bundle_id: &str) -> bool {
        file.starts_with(&format!("seed bundle {bundle_id}: "))
    }
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
    /// `prefix → namespace IRI` mappings seeded into the server's prefix
    /// registry (served by `GET /api/prefixes`; auto-declared in SPARQL
    /// queries). Existing registry entries always win.
    pub prefixes: std::collections::HashMap<String, String>,
    /// Reference models registered before the datasets, so `conforms_to`
    /// can resolve them.
    pub data_models: Vec<BundleDataModel>,
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
    /// Reference models registered (or already present) from `[[data_models]]`.
    pub models_registered: usize,
    /// Datasets whose `conforms_to` was applied.
    pub conformance_declared: usize,
    /// Shape graphs registered/bound to datasets.
    pub shape_graphs_bound: usize,
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
    /// Prefix mappings newly seeded into the prefix registry.
    pub prefixes_seeded: usize,
}

/// The placeholder a manifest or payload may use for the deployment's base
/// URL, so a bundle can mint IRIs under it — an ontology at
/// `{base_url}/ns/…#` is dereferenceable on the very instance that serves
/// it, which no fixed IRI is. Expanded at apply time in model namespaces,
/// graph IRIs, shape-graph bindings and payload text; a bundle without it is
/// untouched.
pub const BASE_URL_PLACEHOLDER: &str = "{base_url}";

/// `{base_url}` → the deployment's base URL (no trailing slash).
pub fn expand_base<'a>(s: &'a str, base_url: &str) -> Cow<'a, str> {
    if s.contains(BASE_URL_PLACEHOLDER) {
        Cow::Owned(s.replace(BASE_URL_PLACEHOLDER, base_url.trim_end_matches('/')))
    } else {
        Cow::Borrowed(s)
    }
}

/// Execute one bundle against the store + identity DB. Idempotent and
/// best-effort per item: a failing dataset/graph/service is logged and skipped
/// without aborting the rest. Callers decide whether the bundle runs at all
/// ([`Bundle::is_disabled`]).
pub fn apply_bundle(state: &AppState, bundle: &Bundle) -> anyhow::Result<SeedReport> {
    let mut report = SeedReport::default();
    let base_url = state.base_url.as_str();

    // Prefixes first — independent of dataset/graph state, idempotent (existing
    // registry entries win), so they seed even when everything else pre-exists.
    for (label, iri) in &bundle.prefixes {
        if state.prefix_registry.insert_seeded(label, iri) {
            report.prefixes_seeded += 1;
        }
    }

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

    // Reference models first: a dataset's `conforms_to` names one of them.
    // Idempotent — an existing model/version is left alone (its graphs are
    // backfilled only when empty, like dataset graphs).
    for dm in &bundle.data_models {
        let base = state.base_url.as_str();
        let now = chrono::Utc::now().to_rfc3339();
        // The ids of the vocabularies the server seeds are the seeder's, whatever
        // the boot order (bundles run first): a bundle model there would take the
        // id before the seeder, or add content to the seeder's entry (IMBOR
        // allows no altered copies).
        if crate::data_models::seed_vocab::reserves_id(&dm.id) {
            tracing::warn!(bundle = %bundle.id, model = %dm.id, "model id is reserved for a vocabulary the server seeds; bundle model skipped (give it another id)");
            continue;
        }
        // Any other entry whose licence allows no altered copies (a LOV install,
        // another bundle's model) may not get this bundle's graphs or versions.
        if let Some(a) =
            crate::data_models::registry::no_derivatives_attribution(&state.store, base, &dm.id)
                .filter(|a| !BundleLicense::is_record_of(&a.file, &bundle.id))
        {
            tracing::warn!(bundle = %bundle.id, model = %dm.id, file = %a.file, "model entry allows no altered copies; bundle model skipped");
            continue;
        }
        let existing = crate::data_models::registry::get_data_model(&state.store, base, &dm.id);
        // A licensed model's record goes only on an entry this bundle owns; so
        // does a model the manifest keeps private (`public = false`): the
        // bundle cannot keep someone else's entry private.
        if dm.license.is_some() || !dm.public {
            if let Some(e) = &existing {
                if e.owner_type.as_deref() != Some("organisation")
                    || e.owner_id.as_deref() != Some(org_id.as_str())
                {
                    tracing::warn!(bundle = %bundle.id, model = %dm.id, "a registry entry with this id exists and is not this bundle's; licensed or private bundle model skipped");
                    continue;
                }
            }
        }
        if let Some(e) = &existing {
            apply_manifest_visibility(state, bundle, dm, &org_id, e);
        }
        if existing.is_none() {
            if let Err(e) = crate::data_models::registry::insert_data_model(
                &state.store,
                base,
                &dm.id,
                &dm.title,
                &expand_base(&dm.namespace, base_url),
                dm.description.as_deref(),
                dm.public,
                Some("organisation"),
                Some(&org_id),
                None,
                &now,
            ) {
                tracing::warn!(bundle = %bundle.id, model = %dm.id, error = %e, "failed to register data model");
                continue;
            }
            // The entry is this bundle's, created with the manifest's visibility.
            let _ = crate::data_models::registry::set_seeded_by(
                &state.store,
                &crate::data_models::registry::data_model_iri(base, &dm.id),
                &visibility_marker(&bundle.id, dm.public),
            );
            if dm.kind != crate::kind_detector::RegistryKind::DataModel {
                let _ = crate::data_models::registry::set_data_model_kind(
                    &state.store,
                    base,
                    &dm.id,
                    dm.kind,
                );
            }
        }
        for g in &dm.graphs {
            let iri = expand_base(&g.iri, base_url);
            if let Some((data, fmt)) = &g.data {
                let empty = state
                    .store
                    .graph_count_cached(Some(&iri))
                    .map(|n| n == 0)
                    .unwrap_or(true);
                if empty {
                    if let Err(e) = load_graph(state, &iri, &expand_base(data, base_url), *fmt) {
                        tracing::warn!(bundle = %bundle.id, graph = %iri, error = %e, "failed to load model graph");
                    } else {
                        report.graphs_loaded += 1;
                    }
                }
            }
        }
        if crate::data_models::registry::get_version(&state.store, base, &dm.id, &dm.version)
            .is_none()
        {
            let mut iris = dm
                .graphs
                .iter()
                .map(|g| expand_base(&g.iri, base_url).into_owned());
            let graph_iri = iris.next().expect("manifest guarantees at least one graph");
            let version = crate::data_models::models::DataModelVersion {
                data_model_id: dm.id.clone(),
                version: dm.version.clone(),
                status: crate::data_models::models::VersionStatus::Published,
                graph_iri,
                sub_graphs: iris.collect(),
                created_at: now.clone(),
                created_by: None,
                derived_from: None,
                notes: Some(format!("Seeded by bundle '{}'", bundle.id)),
                branch: None,
                sub_graph_status: vec![],
            };
            if let Err(e) =
                crate::data_models::registry::insert_version(&state.store, base, &version)
            {
                tracing::warn!(bundle = %bundle.id, model = %dm.id, error = %e, "failed to register model version");
                continue;
            }
            let _ = crate::data_models::registry::update_latest_published(
                &state.store,
                base,
                &dm.id,
                &dm.version,
            );
        }
        if let Some(license) = &dm.license {
            if let Err(e) = record_bundle_licence(state, bundle, dm, license) {
                tracing::warn!(bundle = %bundle.id, model = %dm.id, error = %e, "licence record not written");
            }
        }
        report.models_registered += 1;
    }

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
            let iri = expand_base(&g.iri, base_url);
            // (Re)load the bundled data only while the target graph is empty:
            // a fresh seed, or a previous seed that registered the graph but
            // never populated it. A graph that already holds triples is left
            // untouched, so an admin's edits are never overwritten.
            let is_empty = state.store.graph_count_cached(Some(&iri)).unwrap_or(0) == 0;
            if is_empty {
                any_graph_empty = true;
                if let Some((data, fmt)) = &g.data {
                    if let Err(e) = load_graph(state, &iri, &expand_base(data, base_url), *fmt) {
                        tracing::warn!(
                            "seed bundle '{}': graph <{}> load failed: {e}",
                            bundle.id,
                            iri
                        );
                        continue;
                    }
                    report.graphs_loaded += 1;
                    if existed {
                        report.graphs_backfilled += 1;
                    }
                }
            }
            let _ = state.auth_db.add_dataset_graph(&ds.slug, &iri);
            if g.role.is_some() {
                let _ = state.auth_db.set_dataset_graph_role(&ds.slug, &iri, g.role);
            }
            report.graphs_registered += 1;
        }

        // Multi-graph payloads (TriG / N-Quads). Loaded only while a declared
        // graph is still empty; re-loading identical triples is a no-op under
        // RDF set semantics, so a partial earlier load self-heals.
        if any_graph_empty || ds.graphs.is_empty() {
            for q in &ds.quads {
                if let Err(e) =
                    state
                        .store
                        .load_str(&expand_base(&q.data, base_url), q.format, None)
                {
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

        // The conformance layer: which model this dataset conforms to, and
        // which shape graphs validate it. Declared, not inferred.
        if let Some((model, version)) = &ds.conforms_to {
            match state.auth_db.update_dataset_conformance(
                &ds.slug,
                Some(model),
                version.as_deref(),
            ) {
                Ok(()) => report.conformance_declared += 1,
                Err(e) => {
                    tracing::warn!(bundle = %bundle.id, dataset = %ds.slug, error = %e, "failed to declare conformance")
                }
            }
        }
        for shapes_iri in &ds.shape_graphs {
            match bind_shape_graph(state, &org_id, ds, &expand_base(shapes_iri, base_url)) {
                Ok(()) => report.shape_graphs_bound += 1,
                Err(e) => {
                    tracing::warn!(bundle = %bundle.id, dataset = %ds.slug, shapes = %shapes_iri, error = %e, "failed to bind shape graph")
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

/// The `ver:seededBy` marker a bundle writes on a registry entry it created:
/// the bundle, and the visibility the manifest asked for the last time the
/// bundle applied it (at creation, or once afterwards; see
/// [`apply_manifest_visibility`]).
fn visibility_marker(bundle_id: &str, public: bool) -> String {
    format!(
        "seed-bundle:{bundle_id}:{}",
        if public { "public" } else { "private" }
    )
}

/// Builds before this one created every bundle model public, whatever the
/// manifest said. When the manifest says `public = false` for an entry this
/// bundle provably created, and the bundle has not applied that yet, the entry
/// is made private, once. Only its visibility changes: no graph, version,
/// note or licence record is touched.
///
/// "Provably created by this bundle": owned by the bundle's organisation, no
/// creator (every entry made through the API has one), and either this
/// bundle's marker ([`visibility_marker`]) or, for an entry an earlier build
/// created without one, a version this bundle registered (no creator, its
/// notes "Seeded by bundle '…'"), which holds the content the manifest keeps
/// private. Any other entry is left as it is.
///
/// The marker then records that the manifest's `public = false` was applied,
/// so an admin who makes the entry public again (a NEN licence of their own,
/// say) is not overruled at the next start. A manifest that later asks for
/// `public = true` makes nothing public: publishing stays an admin's
/// decision; the marker only records it, so that a later `public = false`
/// applies once more.
fn apply_manifest_visibility(
    state: &AppState,
    bundle: &Bundle,
    dm: &BundleDataModel,
    org_id: &str,
    entry: &crate::data_models::models::DataModelRecord,
) {
    use crate::data_models::registry;
    let base = state.base_url.as_str();
    if entry.owner_type.as_deref() != Some("organisation")
        || entry.owner_id.as_deref() != Some(org_id)
        || entry.created_by.is_some()
    {
        return;
    }
    let (provenance, versions) = registry::record_provenance(&state.store, base, &dm.id);
    let Some(provenance) = provenance else {
        return;
    };
    if provenance.created_by.is_some() {
        return;
    }
    let ours = format!("seed-bundle:{}:", bundle.id);
    let applied = match provenance.seeded_by.as_deref() {
        Some(m) if m.starts_with(&ours) => Some(m.to_string()),
        // Another seeder's or another bundle's entry.
        Some(_) => return,
        None => {
            let seeded_note = format!("Seeded by bundle '{}'", bundle.id);
            let registered_here = versions.values().any(|v| {
                v.created_by.is_none() && v.notes.as_deref() == Some(seeded_note.as_str())
            });
            if !registered_here {
                return;
            }
            None
        }
    };
    let wanted = visibility_marker(&bundle.id, dm.public);
    if applied.as_deref() == Some(wanted.as_str()) {
        return;
    }
    let entry_iri = registry::data_model_iri(base, &dm.id);
    if !dm.public && entry.is_public {
        // The change goes in before the marker: a start cut short in between
        // finds the entry private and only writes the marker.
        if let Err(e) = registry::update_data_model(
            &state.store,
            base,
            &dm.id,
            None,
            None,
            None,
            Some(false),
            None,
            None,
        ) {
            tracing::warn!(bundle = %bundle.id, model = %dm.id, error = %e, "the manifest says public = false, but the registry entry could not be made private");
            return;
        }
        state.auth_db.invalidate_accessible_graphs_cache();
        state.mark_vocab_registry_dirty();
        tracing::warn!(bundle = %bundle.id, model = %dm.id, "the registry entry was public (an earlier build made every bundle model public, or the manifest said public = true before); the manifest says public = false, so the entry is now private, once (its content is unchanged). An admin may make it public again.");
    }
    if let Err(e) = registry::set_seeded_by(&state.store, &entry_iri, &wanted) {
        tracing::warn!(bundle = %bundle.id, model = %dm.id, error = %e, "the registry entry's seed marker could not be written");
    }
}

/// Record the licence a bundle declares for model `dm` on its version and,
/// when the bundle owns it, its entry — like the licence record of a bundled
/// vocabulary. Nothing in the model's graphs is changed.
///
/// The record calls the content unchanged only when every graph holds exactly
/// its payload file's triples (compared in the store's canonical literal
/// forms). A start whose files and graphs are the ones checked before (same
/// file SHA-256, same stored digest) reads the graphs once and parses
/// nothing. A graph that differs from its file (an admin's edit, a re-fetched
/// release) is left as it is, and the record says the content may have been
/// modified; with `no_derivatives` the registry then serves the version to no
/// one who may not write the entry.
fn record_bundle_licence(
    state: &AppState,
    bundle: &Bundle,
    dm: &BundleDataModel,
    license: &BundleLicense,
) -> anyhow::Result<()> {
    use crate::data_models::models::{ContentAttribution, LicenseRef};
    use crate::data_models::{content_digest, registry};

    let base_url = state.base_url.as_str();
    let (_, versions) = registry::record_provenance(&state.store, base_url, &dm.id);
    let Some(p) = versions.get(&dm.version) else {
        return Ok(());
    };
    let graphs: Vec<String> = dm
        .graphs
        .iter()
        .map(|g| expand_base(&g.iri, base_url).into_owned())
        .collect();
    // Only the version this bundle registered: no creator, its notes, its graphs.
    let own = p.created_by.is_none()
        && p.notes.as_deref() == Some(format!("Seeded by bundle '{}'", bundle.id).as_str())
        && p.graph_iri.as_deref() == graphs.first().map(String::as_str)
        && p.sub_graphs.iter().all(|g| graphs.contains(g));
    if !own {
        anyhow::bail!(
            "version '{}' of '{}' was not registered by this bundle; no licence record written",
            dm.version,
            dm.id
        );
    }
    let ver_iri = registry::version_record_iri(base_url, &dm.id, &dm.version);

    // The payloads as loaded (the base URL expanded), and their SHA-256.
    let payloads: Vec<(String, Option<(String, Fmt)>)> = dm
        .graphs
        .iter()
        .zip(&graphs)
        .map(|(g, iri)| {
            (
                iri.clone(),
                g.data
                    .as_ref()
                    .map(|(d, f)| (expand_base(d, base_url).into_owned(), *f)),
            )
        })
        .collect();
    let mut concatenated: Vec<u8> = Vec::new();
    for (iri, data) in &payloads {
        concatenated.extend_from_slice(iri.as_bytes());
        concatenated.push(b'\n');
        if let Some((d, _)) = data {
            concatenated.extend_from_slice(d.as_bytes());
        }
        concatenated.push(0);
    }
    let file_sha = content_digest::file_sha256(&concatenated);
    let content_graphs = content_digest::version_graphs(&graphs[0], &graphs[1..]);
    let stored_digest = content_digest::graphs_digest(&state.store, &content_graphs)?;
    let recorded: Option<ContentAttribution> = p
        .attribution_json
        .as_deref()
        .and_then(|j| serde_json::from_str(j).ok());
    let checked_before = p.seed_source_sha256.as_deref() == Some(file_sha.as_str())
        && p.seed_content_digest.as_deref() == Some(stored_digest.as_str())
        && recorded.as_ref().is_some_and(|a| a.unchanged);
    let unchanged = checked_before
        || {
            let mut all = true;
            for (iri, data) in &payloads {
                let Some((d, fmt)) = data else {
                    all = false;
                    break;
                };
                let stored = content_digest::graph_triples(&state.store, iri)?;
                match payload_as_stored(state, iri, d, *fmt) {
                    Ok(expected) if content_digest::same_triples(&stored, &expected) => {}
                    Ok(_) => {
                        tracing::warn!(bundle = %bundle.id, model = %dm.id, graph = %iri, "graph differs from its payload file; kept as it is, and its licence record says it may have been modified");
                        all = false;
                        break;
                    }
                    Err(e) => {
                        tracing::warn!(bundle = %bundle.id, model = %dm.id, graph = %iri, error = %e, "payload could not be compared");
                        all = false;
                        break;
                    }
                }
            }
            all
        };
    registry::set_seed_check(
        &state.store,
        &ver_iri,
        &file_sha,
        unchanged.then_some(stored_digest.as_str()),
    )?;

    let file = license.record_file(&bundle.id);
    let stored_copy = if unchanged {
        format!(
            "The store holds the triples of the payload files of {file}, unchanged: checked \
             against the files (typed literals in the store's canonical form, the same values)."
        )
    } else {
        let withheld = if license.no_derivatives {
            " The licence allows no altered copies, so the registry serves this copy to no one \
             without write access to this entry."
        } else {
            ""
        };
        format!(
            "This copy was loaded from {file}, but its graphs differ from those files or may since \
             have been edited, so it is not that content.{withheld}"
        )
    };
    let record = ContentAttribution {
        file,
        licenses: license
            .licenses
            .iter()
            .map(|(name, uri)| LicenseRef {
                name: name.clone(),
                uri: uri.clone(),
            })
            .collect(),
        copyright: license.copyright.clone(),
        notice: license.notice.clone(),
        status: None,
        source_url: license.source.clone(),
        specification_url: None,
        changes: license.changes.clone(),
        stored_copy,
        unchanged,
        remarks: license.remarks.clone(),
        no_derivatives: license.no_derivatives,
        header: None,
        notice_url: license.notice_url.clone(),
    };
    let json = serde_json::to_string(&record)?;
    let mut wrote = false;
    if p.attribution_json.as_deref() != Some(json.as_str()) {
        if registry::replace_attribution_if(
            &state.store,
            &ver_iri,
            p.attribution_json.as_deref(),
            Some(&record),
        )? {
            wrote = true;
        } else {
            tracing::warn!(bundle = %bundle.id, model = %dm.id, "the version's licence record changed while it was checked; left as the other writer set it");
        }
    }
    // The entry: only when this bundle's organisation owns it (checked by the
    // caller for an existing entry; a new one was created just now).
    let entry_iri = registry::data_model_iri(base_url, &dm.id);
    let entry_json = registry::attribution_json(&state.store, &entry_iri);
    if entry_json.as_deref() != Some(json.as_str()) {
        wrote |= registry::replace_attribution_if(
            &state.store,
            &entry_iri,
            entry_json.as_deref(),
            Some(&record),
        )?;
    }
    if wrote {
        state.mark_vocab_registry_dirty();
    }
    Ok(())
}

/// A payload's triples as the store holds them after [`load_graph`]: parsed
/// like the Graph Store PUT parses it (no base IRI, into `graph_iri`, blank
/// nodes as the store's mode treats them), typed literals in the store's
/// canonical form.
fn payload_as_stored(
    state: &AppState,
    graph_iri: &str,
    data: &str,
    fmt: Fmt,
) -> anyhow::Result<Vec<oxigraph::model::Triple>> {
    use oxigraph::model::{GraphName, NamedNode, Quad};
    let format = fmt
        .rdf_format()
        .ok_or_else(|| anyhow::anyhow!("payload format cannot be compared"))?;
    let graph = GraphName::NamedNode(NamedNode::new(graph_iri)?);
    let quads: Vec<Quad> = oxigraph::io::RdfParser::from_format(format)
        .for_reader(std::io::BufReader::new(data.as_bytes()))
        .map(|r| {
            r.map(|q| Quad::new(q.subject, q.predicate, q.object, graph.clone()))
                .map_err(|e| anyhow::anyhow!("{e}"))
        })
        .collect::<anyhow::Result<_>>()?;
    let quads = match state.store.blank_node_mode() {
        crate::store::engine::BlankNodeMode::Skolem => {
            opengraph::skolem::skolemize(&quads, opengraph::DEFAULT_SKOLEM_BASE).0
        }
        // Blank nodes kept or relabelled: compared up to renaming anyway.
        _ => quads,
    };
    Ok(crate::data_models::content_digest::as_stored(
        &crate::data_models::content_digest::quads_as_triples(&quads),
    )?)
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

/// Register `shapes_iri` in the SHACL Studio library (if it is not yet a
/// shape graph) and bind it to the dataset, so `POST …/validate` and the
/// write gate apply it. Mirrors `register_shape_graph` + `POST /bindings`.
fn bind_shape_graph(
    state: &AppState,
    org_id: &str,
    ds: &BundleDataset,
    shapes_iri: &str,
) -> anyhow::Result<()> {
    use crate::shacl_studio::store::ShaclStudioStore;
    let st = ShaclStudioStore::new(state.auth_db.pool());
    if st.get_shape_graph_by_iri(shapes_iri)?.is_none() {
        let (targets, count) =
            crate::shacl_studio::run::analyze_shapes_graph(&state.store, shapes_iri);
        if count == 0 {
            anyhow::bail!("graph <{shapes_iri}> contains no SHACL shapes");
        }
        let set = st.create_shape_graph(
            &format!("{} shapes", ds.name),
            Some("Shipped by a seed bundle"),
            crate::auth::models::OwnerType::Organisation,
            org_id,
            ds.visibility,
            shapes_iri,
            &[],
            crate::shacl_studio::models::ShapeSource::Imported,
            None,
        )?;
        let turtle = state
            .store
            .graph_store_get(Some(shapes_iri), oxigraph::io::RdfFormat::Turtle)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let turtle = String::from_utf8(turtle)?;
        st.save_shape_graph_revision(
            &set.id,
            &turtle,
            &targets,
            count,
            Some("Seeded by bundle"),
            None,
        )?;
    }
    let target = crate::shacl_studio::bindings::dataset_target_iri(&state.base_url, &ds.slug);
    crate::shacl_studio::bindings::add_binding(&state.store, &target, shapes_iri)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
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
                "seed bundle '{}': org {} ({}), {} dataset(s) created, {} graph(s) registered, {} payload(s) loaded, {} service(s), {} prefix(es)",
                bundle.id,
                r.org_id,
                if r.org_created { "created" } else { "existing" },
                r.datasets_created.len(),
                r.graphs_registered,
                r.graphs_loaded,
                r.services_created,
                r.prefixes_seeded
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
                conforms_to: None,
                shape_graphs: Vec::new(),
            }],
            prefixes: std::collections::HashMap::from([(
                "ex".to_string(),
                format!("https://example.org/{id}/ns#"),
            )]),
            data_models: Vec::new(),
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

    fn model_bundle(id: &str, model_id: &str, license: Option<BundleLicense>) -> Bundle {
        let mut b = tiny_bundle(id);
        b.datasets.clear();
        b.data_models = vec![BundleDataModel {
            id: model_id.into(),
            title: "A model".into(),
            namespace: "https://example.org/otl/".into(),
            description: None,
            kind: crate::kind_detector::RegistryKind::DataModel,
            version: "2025".into(),
            graphs: vec![BundleGraph {
                iri: "https://example.org/otl/def/".into(),
                role: Some(GraphKind::Model),
                data: Some((
                    Cow::Borrowed(
                        "@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .\n\
                         <https://example.org/otl/def/Boom> a <http://www.w3.org/2000/01/rdf-schema#Class> ;\n\
                           <http://www.w3.org/2002/07/owl#minCardinality> \"1\"^^xsd:nonNegativeInteger ;\n\
                           <http://www.w3.org/ns/shacl#property> [ <http://www.w3.org/ns/shacl#path> <https://example.org/otl/def/hoogte> ] .",
                    ),
                    Fmt::Turtle,
                )),
            }],
            license,
            public: true,
        }];
        b
    }

    fn nd_license() -> BundleLicense {
        BundleLicense {
            licenses: vec![(
                "CC BY 4.0".into(),
                "https://creativecommons.org/licenses/by/4.0/".into(),
            )],
            copyright: vec!["© Stichting CROW".into()],
            source: "https://github.com/Stichting-CROW/imbor/releases/tag/2025".into(),
            notice: None,
            changes: None,
            remarks: None,
            notice_url: "https://github.com/Stichting-CROW/imbor/releases/tag/2025".into(),
            no_derivatives: true,
            files: vec!["otl.ttl".into()],
        }
    }

    /// Bundles run before the vocabulary seeder on a first boot. A bundle
    /// model under one of the seeder's ids is skipped whatever the order, so
    /// the seeder's IMBOR record always names the graph it checked.
    #[test]
    fn a_bundle_model_under_a_seeded_id_is_skipped_whatever_the_boot_order() {
        let state = test_state();
        let b = model_bundle("early", "imbor", None);
        apply_bundle(&state, &b).unwrap();
        assert!(crate::data_models::registry::get_data_model(
            &state.store,
            &state.base_url,
            "imbor"
        )
        .is_none());
        crate::data_models::seed_vocab::seed_standard_vocabularies(&state);
        let v = crate::data_models::registry::get_version(
            &state.store,
            &state.base_url,
            "imbor",
            "2025",
        )
        .unwrap();
        assert_eq!(
            v.graph_iri,
            crate::data_models::registry::version_record_iri(&state.base_url, "imbor", "2025")
        );
        let a = crate::data_models::registry::get_attribution(
            &state.store,
            &crate::data_models::registry::version_record_iri(&state.base_url, "imbor", "2025"),
        )
        .unwrap();
        assert!(a.no_derivatives && a.unchanged);
        // Bundle graphs never got in.
        assert_eq!(
            state
                .store
                .graph_count_cached(Some("https://example.org/otl/def/"))
                .unwrap_or(0),
            0
        );
    }

    /// A bundle model that declares a no-derivatives licence gets a licence
    /// record on its entry and version, so every guard applies: a direct
    /// write into its graph is refused, and another bundle cannot add to the
    /// entry. A graph that no longer holds its file's triples is never
    /// modified: the record stops calling it unchanged.
    #[test]
    fn a_licensed_bundle_model_is_recorded_checked_and_never_modified() {
        use crate::data_models::registry;
        let state = test_state();
        let b = model_bundle("crow", "crow-otl", Some(nd_license()));
        apply_bundle(&state, &b).unwrap();
        let ver_iri = registry::version_record_iri(&state.base_url, "crow-otl", "2025");
        let a = registry::get_attribution(&state.store, &ver_iri).unwrap();
        assert!(a.no_derivatives && a.unchanged, "{a:?}");
        assert!(a.file.starts_with("seed bundle crow: "));
        assert_eq!(a.copyright, vec!["© Stichting CROW".to_string()]);
        let entry = registry::get_attribution(
            &state.store,
            &registry::data_model_iri(&state.base_url, "crow-otl"),
        )
        .unwrap();
        assert!(entry.no_derivatives);
        assert!(
            registry::no_derivatives_attribution(&state.store, &state.base_url, "crow-otl")
                .is_some()
        );
        // The direct-write guard refuses its graph.
        assert!(crate::data_models::write_guard::check(
            &state.store,
            &state.base_url,
            "https://example.org/otl/def/"
        )
        .is_err());
        // Re-applying the same bundle is not refused, and changes nothing.
        let json = registry::attribution_json(&state.store, &ver_iri);
        apply_bundle(&state, &b).unwrap();
        assert_eq!(registry::attribution_json(&state.store, &ver_iri), json);
        // Another bundle may not add to the entry.
        let other = model_bundle("other", "crow-otl", None);
        let mut other = other;
        other.data_models[0].version = "2026".into();
        apply_bundle(&state, &other).unwrap();
        assert!(!registry::version_exists(
            &state.store,
            &state.base_url,
            "crow-otl",
            "2026"
        ));

        // An edit outside the registry: kept, and labelled.
        state
            .store
            .update(
                "INSERT DATA { GRAPH <https://example.org/otl/def/> { \
                 <https://example.org/otl/def/x> <http://www.w3.org/2000/01/rdf-schema#label> \"x\" } }",
            )
            .unwrap();
        apply_bundle(&state, &b).unwrap();
        let a = registry::get_attribution(&state.store, &ver_iri).unwrap();
        assert!(!a.unchanged);
        assert!(state
            .store
            .query(
                "ASK { GRAPH <https://example.org/otl/def/> { \
                 <https://example.org/otl/def/x> ?p ?o } }"
            )
            .map(|r| matches!(r, oxigraph::sparql::QueryResults::Boolean(true)))
            .unwrap());
    }

    /// A licensed model whose id an entry of someone else already holds is
    /// skipped: no licence record lands on another owner's entry.
    #[test]
    fn a_licensed_bundle_model_never_records_on_someone_elses_entry() {
        use crate::data_models::registry;
        let state = test_state();
        registry::insert_data_model(
            &state.store,
            &state.base_url,
            "crow-otl",
            "Mine",
            "https://example.org/mine#",
            None,
            false,
            Some("user"),
            Some("u1"),
            Some(&format!("{}/users/u1", state.base_url)),
            "2026-01-01T00:00:00Z",
        )
        .unwrap();
        apply_bundle(
            &state,
            &model_bundle("crow", "crow-otl", Some(nd_license())),
        )
        .unwrap();
        assert!(
            registry::no_derivatives_attribution(&state.store, &state.base_url, "crow-otl")
                .is_none()
        );
        assert!(!registry::version_exists(
            &state.store,
            &state.base_url,
            "crow-otl",
            "2025"
        ));
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

    fn entry_public(state: &AppState, id: &str) -> bool {
        crate::data_models::registry::get_data_model(&state.store, &state.base_url, id)
            .expect("the entry exists")
            .is_public
    }

    fn seed_marker(state: &AppState, id: &str) -> Option<String> {
        crate::data_models::registry::record_provenance(&state.store, &state.base_url, id)
            .0
            .and_then(|p| p.seeded_by)
    }

    /// What a build before this one left: the entry public whatever the
    /// manifest said, and no marker.
    fn as_an_earlier_build_left_it(state: &AppState, id: &str) {
        use crate::data_models::registry;
        registry::update_data_model(
            &state.store,
            &state.base_url,
            id,
            None,
            None,
            None,
            Some(true),
            None,
            None,
        )
        .unwrap();
        let entry = registry::data_model_iri(&state.base_url, id);
        state
            .store
            .update(&format!(
                "DELETE WHERE {{ GRAPH <{}> {{ <{entry}> <urn:system:vocab/seededBy> ?m }} }}",
                registry::REGISTRY_GRAPH
            ))
            .unwrap();
        assert!(seed_marker(state, id).is_none());
    }

    fn model_graph_digest(state: &AppState) -> String {
        crate::data_models::content_digest::triples_digest(
            &crate::data_models::content_digest::graph_triples(
                &state.store,
                "https://example.org/otl/def/",
            )
            .unwrap(),
        )
    }

    /// A new entry gets the manifest's visibility and this bundle's marker.
    #[test]
    fn a_new_bundle_model_gets_the_manifests_visibility() {
        let state = test_state();
        let mut b = model_bundle("nen", "nen-model", None);
        b.data_models[0].public = false;
        apply_bundle(&state, &b).unwrap();
        assert!(!entry_public(&state, "nen-model"));
        assert_eq!(
            seed_marker(&state, "nen-model").as_deref(),
            Some("seed-bundle:nen:private")
        );
        apply_bundle(&state, &b).unwrap();
        assert!(!entry_public(&state, "nen-model"));

        let pub_bundle = model_bundle("open", "open-model", None);
        apply_bundle(&state, &pub_bundle).unwrap();
        assert!(entry_public(&state, "open-model"));
        assert_eq!(
            seed_marker(&state, "open-model").as_deref(),
            Some("seed-bundle:open:public")
        );
    }

    /// An entry an earlier build created public, which the manifest now keeps
    /// private (NEN 2660-2), is made private once: only its visibility
    /// changes. An admin who makes it public again is not overruled at the
    /// next start.
    #[test]
    fn public_false_makes_an_entry_an_earlier_build_published_private_once() {
        use crate::data_models::registry;
        let state = test_state();
        let mut b = model_bundle("nen", "nen-model", None);
        apply_bundle(&state, &b).unwrap();
        as_an_earlier_build_left_it(&state, "nen-model");
        let graph = model_graph_digest(&state);
        let version =
            registry::get_version(&state.store, &state.base_url, "nen-model", "2025").unwrap();

        b.data_models[0].public = false;
        apply_bundle(&state, &b).unwrap();
        assert!(!entry_public(&state, "nen-model"));
        assert_eq!(
            seed_marker(&state, "nen-model").as_deref(),
            Some("seed-bundle:nen:private")
        );
        // Nothing else changed.
        assert_eq!(model_graph_digest(&state), graph);
        let after =
            registry::get_version(&state.store, &state.base_url, "nen-model", "2025").unwrap();
        assert_eq!(after.notes, version.notes);
        assert_eq!(after.status, version.status);
        assert_eq!(after.graph_iri, version.graph_iri);

        // An admin publishes it again (a licence of their own, say).
        registry::update_data_model(
            &state.store,
            &state.base_url,
            "nen-model",
            None,
            None,
            None,
            Some(true),
            None,
            None,
        )
        .unwrap();
        apply_bundle(&state, &b).unwrap();
        assert!(
            entry_public(&state, "nen-model"),
            "the admin's choice stands"
        );
    }

    /// A manifest that later turns `public = false` applies it once to an
    /// entry this build created public; one that turns `public = true` again
    /// publishes nothing.
    #[test]
    fn each_turn_to_public_false_applies_once_and_nothing_is_published() {
        let state = test_state();
        let mut b = model_bundle("nen", "nen-model", None);
        apply_bundle(&state, &b).unwrap();
        assert!(entry_public(&state, "nen-model"));

        b.data_models[0].public = false;
        apply_bundle(&state, &b).unwrap();
        assert!(!entry_public(&state, "nen-model"));

        b.data_models[0].public = true;
        apply_bundle(&state, &b).unwrap();
        assert!(
            !entry_public(&state, "nen-model"),
            "publishing is an admin's decision"
        );
        assert_eq!(
            seed_marker(&state, "nen-model").as_deref(),
            Some("seed-bundle:nen:public")
        );
    }

    /// An entry this bundle did not provably create keeps its visibility: one
    /// with a creator, one another bundle marked, one holding no version this
    /// bundle registered. And a model the manifest keeps private never goes
    /// into an entry the bundle's organisation does not own.
    #[test]
    fn public_false_never_touches_an_entry_the_bundle_did_not_create() {
        use crate::data_models::registry;
        let state = test_state();
        let org_id = apply_bundle(&state, &model_bundle("nen", "nen-model", None))
            .unwrap()
            .org_id;
        let base = state.base_url.to_string();
        let insert = |id: &str, owner_type: &str, owner_id: &str, creator: Option<&str>| {
            registry::insert_data_model(
                &state.store,
                &base,
                id,
                "Someone's",
                "https://example.org/theirs#",
                None,
                true,
                Some(owner_type),
                Some(owner_id),
                creator,
                "2026-01-01T00:00:00Z",
            )
            .unwrap();
        };
        insert(
            "with-creator",
            "organisation",
            &org_id,
            Some(&format!("{base}/users/u1")),
        );
        insert("other-bundle", "organisation", &org_id, None);
        registry::set_seeded_by(
            &state.store,
            &registry::data_model_iri(&base, "other-bundle"),
            "seed-bundle:other:public",
        )
        .unwrap();
        insert("no-version-here", "organisation", &org_id, None);
        insert(
            "users-entry",
            "user",
            "u1",
            Some(&format!("{base}/users/u1")),
        );

        for id in [
            "with-creator",
            "other-bundle",
            "no-version-here",
            "users-entry",
        ] {
            let mut b = model_bundle("nen", id, None);
            b.data_models[0].public = false;
            apply_bundle(&state, &b).unwrap();
            assert!(entry_public(&state, id), "{id}");
        }
        assert!(!registry::version_exists(
            &state.store,
            &base,
            "users-entry",
            "2025"
        ));
    }
}
