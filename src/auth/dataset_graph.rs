use crate::auth::models::{Dataset, DatasetGraphEntry, GraphKind};
use crate::store::engine::TripleStore;
use crate::store::escape_sparql_iri;
use oxigraph::io::RdfFormat;

/// Named graph IRI for a dataset's DCAT metadata.
pub fn dataset_metadata_graph_iri(dataset_id: &str) -> String {
    format!("urn:system:metadata:dataset:{}", dataset_id)
}

/// Canonical dataset IRI per the styleguide (§3.3): `{base}/dataset/{id}`
/// (singular). This is the single source of truth for the dataset's own IRI and
/// the prefix of its owned graph namespace (`{base}/dataset/{id}/...`). It MUST
/// match the catalogue, version registry and API-service registry; the bulk
/// import write boundary also keys off this prefix, so any divergence would let
/// a caller's "namespaced" target graph fall outside the gate.
pub fn dataset_iri(base_url: &str, dataset_id: &str) -> String {
    format!("{}/dataset/{}", base_url.trim_end_matches('/'), dataset_id)
}

/// Named graph IRI for a dataset's "default graph": where DEFAULT-graph (and
/// blank-node-graph) triples from a dataset-scoped quad import are routed so they
/// fall under the per-graph write boundary instead of the shared global default
/// graph. Lives under the dataset's owned namespace, so the bulk-import authorize
/// gate admits it (`g.starts_with("{base}/dataset/{id}/")`). `default` is neither
/// an auto-split role suffix nor a `urn:system:` graph, so it cannot collide.
pub fn dataset_default_graph_iri(base_url: &str, dataset_id: &str) -> String {
    format!("{}/default", dataset_iri(base_url, dataset_id))
}

/// True iff `graph_iri` lies inside `dataset_id`'s own reserved graph namespace —
/// either the canonical HTTP prefix `{base}/dataset/{id}/` or the
/// `urn:dataset:{id}:` URN prefix (shapes, RML mappings/output, the namespaced
/// default graph). Both prefixes carry a trailing delimiter so one dataset id can
/// never prefix-match another (`d1` vs `d12`).
pub fn dataset_owns_graph(base_url: &str, dataset_id: &str, graph_iri: &str) -> bool {
    let http_ns = format!("{}/", dataset_iri(base_url, dataset_id));
    let urn_ns = format!("urn:dataset:{dataset_id}:");
    graph_iri.starts_with(&http_ns) || graph_iri.starts_with(&urn_ns)
}

/// True iff `graph_iri` is in a *reserved* namespace owned by another dataset or
/// the system — `urn:system:*`, `urn:dataset:{other}:*`, or
/// `{base}/dataset/{other}/*`. A non-admin may never register or write such a
/// graph for `dataset_id`.
fn graph_in_foreign_reserved_namespace(base_url: &str, dataset_id: &str, graph_iri: &str) -> bool {
    if graph_iri.starts_with("urn:system:") {
        return true;
    }
    if let Some(rest) = graph_iri.strip_prefix("urn:dataset:") {
        return match rest.split_once(':') {
            Some((other, _)) => other != dataset_id,
            None => true, // malformed `urn:dataset:` with no id segment → reserved
        };
    }
    let datasets_root = format!("{}/dataset/", base_url.trim_end_matches('/'));
    if let Some(rest) = graph_iri.strip_prefix(&datasets_root) {
        let other = rest.split('/').next().unwrap_or("");
        return other != dataset_id;
    }
    false
}

/// Authorize a non-admin caller naming `graph_iri` as a write/registration target
/// for `dataset_id`.
///
/// Closes the cross-tenant graph-claim vector: registration (`POST /datasets/:id/
/// graphs`) and RML output both feed `dataset_graphs`, and `get_accessible_graph_iris`
/// then treats any graph registered to an accessible dataset as readable — so a
/// writer who attached *another tenant's* graph IRI to their own dataset could read
/// (or, via RML, write) it. The rule, mirroring the bulk-import write boundary: the
/// graph must be the dataset's own namespaced graph, or a non-reserved external IRI
/// that no other dataset has claimed. Admins bypass (the caller checks `is_admin`).
/// Returns `Err(message)` on rejection (map to HTTP 403); fails closed on a
/// registry lookup error.
pub fn authorize_dataset_graph_target(
    db: &crate::auth::db::AuthDb,
    base_url: &str,
    dataset_id: &str,
    graph_iri: &str,
) -> Result<(), String> {
    // The dataset's own reserved namespace is always allowed.
    if dataset_owns_graph(base_url, dataset_id, graph_iri) {
        return Ok(());
    }
    // Reserved namespaces of other datasets / the system are never claimable.
    if graph_in_foreign_reserved_namespace(base_url, dataset_id, graph_iri) {
        return Err(graph_boundary_error(dataset_id, graph_iri));
    }
    // A non-reserved external graph is allowed only if no OTHER dataset claims it
    // (a lookup error fails closed — treat as foreign).
    match db.graph_has_other_dataset_refs(graph_iri, dataset_id) {
        Ok(false) => Ok(()),
        _ => Err(graph_boundary_error(dataset_id, graph_iri)),
    }
}

fn graph_boundary_error(dataset_id: &str, graph_iri: &str) -> String {
    format!(
        "Target graph <{graph_iri}> is outside dataset '{dataset_id}'. A dataset may only use its \
         own namespaced graphs or an unclaimed external graph — not a graph owned by another \
         dataset or the system."
    )
}

/// Write (or overwrite) the DCAT/ADMS/VoID/VCARD metadata named graph for a dataset.
/// Silently ignores errors so that metadata graph failures never abort the main operation.
///
/// Pass `graph_entries` (from `db.list_dataset_graph_entries`) to include
/// per-graph role triples in the metadata. Pass an empty slice if not available.
pub fn write_dataset_metadata_graph(
    store: &TripleStore,
    base_url: &str,
    dataset: &Dataset,
    graph_entries: &[DatasetGraphEntry],
) {
    let graph_iri = dataset_metadata_graph_iri(&dataset.id);
    let ttl = build_dataset_metadata_ttl(base_url, dataset, graph_entries);
    let _ = store.graph_store_put(Some(&graph_iri), &ttl, RdfFormat::Turtle);
}

/// Like [`write_dataset_metadata_graph`], but validates the built metadata against
/// the built-in **dataset-structure** SHACL model first. Returns the
/// non-conforming `ValidationReport` (and writes nothing) when the metadata
/// violates the model; otherwise writes and returns `Ok`. Used by the dataset
/// create/update API so non-conforming dataset metadata is rejected (HTTP 422).
pub fn write_dataset_metadata_graph_checked(
    store: &TripleStore,
    base_url: &str,
    dataset: &Dataset,
    graph_entries: &[DatasetGraphEntry],
) -> Result<(), crate::shacl::report::ValidationReport> {
    let ttl = build_dataset_metadata_ttl(base_url, dataset, graph_entries);
    if let Some(report) = crate::auth::dataset_audit::validate_metadata(store, &ttl) {
        if !report.conforms {
            return Err(report);
        }
    }
    let graph_iri = dataset_metadata_graph_iri(&dataset.id);
    let _ = store.graph_store_put(Some(&graph_iri), &ttl, RdfFormat::Turtle);
    Ok(())
}

/// Build the DCAT/ADMS/VoID/VCARD metadata Turtle for a dataset (no I/O).
pub fn build_dataset_metadata_ttl(
    base_url: &str,
    dataset: &Dataset,
    graph_entries: &[DatasetGraphEntry],
) -> String {
    // Canonical dataset IRI per the styleguide (§3.3): `{base}/dataset/{id}`
    // (singular). This MUST match the catalogue (`dcat/catalog.rs`), the version
    // registry and the API-service registry, otherwise a dataset's descriptive
    // metadata splits across two IRIs and its node renders incomplete when browsed.
    let dataset_iri = dataset_iri(base_url, &dataset.id);
    let mut ttl = String::new();

    ttl.push_str("@prefix dcat: <http://www.w3.org/ns/dcat#> .\n");
    ttl.push_str("@prefix dct:  <http://purl.org/dc/terms/> .\n");
    ttl.push_str("@prefix void: <http://rdfs.org/ns/void#> .\n");
    ttl.push_str("@prefix adms: <http://www.w3.org/ns/adms#> .\n");
    ttl.push_str("@prefix vcard: <http://www.w3.org/2006/vcard/ns#> .\n");
    ttl.push_str("@prefix xsd:  <http://www.w3.org/2001/XMLSchema#> .\n");
    ttl.push_str("@prefix ots:  <https://opentriplestore.org/ns#> .\n\n");

    ttl.push_str(&format!(
        "<{}> a dcat:Dataset, void:Dataset ;\n",
        dataset_iri
    ));
    ttl.push_str(&format!(
        "    dct:title \"{}\" ;\n",
        escape_ttl_string(&dataset.name)
    ));
    // Required by the dataset-structure SHACL model: stable identity + visibility.
    ttl.push_str(&format!(
        "    dct:identifier \"{}\" ;\n",
        escape_ttl_string(&dataset.id)
    ));
    ttl.push_str(&format!(
        "    ots:visibility \"{}\" ;\n",
        dataset.visibility.as_str()
    ));

    if let Some(desc) = &dataset.description {
        ttl.push_str(&format!(
            "    dct:description \"{}\" ;\n",
            escape_ttl_string(desc)
        ));
    }
    if let Some(lic) = &dataset.license {
        if !lic.is_empty() {
            ttl.push_str(&format!("    dct:license <{}> ;\n", escape_sparql_iri(lic)));
        }
    }

    // Themes (stored as JSON array of IRI strings)
    if let Some(themes_json) = &dataset.themes {
        if let Ok(themes) = serde_json::from_str::<Vec<String>>(themes_json) {
            for theme in &themes {
                if !theme.is_empty() {
                    ttl.push_str(&format!(
                        "    dcat:theme <{}> ;\n",
                        escape_sparql_iri(theme)
                    ));
                }
            }
        }
    }

    // Keywords (stored as JSON array of plain strings)
    if let Some(kw_json) = &dataset.keywords {
        if let Ok(keywords) = serde_json::from_str::<Vec<String>>(kw_json) {
            for kw in &keywords {
                if !kw.is_empty() {
                    ttl.push_str(&format!(
                        "    dcat:keyword \"{}\"@en ;\n",
                        escape_ttl_string(kw)
                    ));
                }
            }
        }
    }

    if let Some(status) = &dataset.adms_status {
        if !status.is_empty() {
            ttl.push_str(&format!(
                "    adms:status <{}> ;\n",
                escape_sparql_iri(status)
            ));
        }
    }
    if let Some(notes) = &dataset.version_notes {
        if !notes.is_empty() {
            ttl.push_str(&format!(
                "    adms:versionNotes \"{}\" ;\n",
                escape_ttl_string(notes)
            ));
        }
    }
    if let Some(spatial) = &dataset.spatial {
        if !spatial.is_empty() {
            ttl.push_str(&format!(
                "    dct:spatial <{}> ;\n",
                escape_sparql_iri(spatial)
            ));
        }
    }

    let landing = dataset
        .landing_page
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or(&dataset_iri);
    ttl.push_str(&format!(
        "    dcat:landingPage <{}> ;\n",
        escape_sparql_iri(landing)
    ));

    ttl.push_str(&format!(
        "    dct:issued \"{}\"^^xsd:dateTime ;\n",
        dataset.created_at
    ));
    ttl.push_str(&format!(
        "    dct:modified \"{}\"^^xsd:dateTime ;\n",
        dataset.updated_at
    ));
    ttl.push_str(&format!(
        "    void:sparqlEndpoint <{}/sparql> ;\n",
        base_url
    ));

    // Contact point as blank node
    let has_contact = dataset
        .contact_name
        .as_deref()
        .filter(|s| !s.is_empty())
        .is_some()
        || dataset
            .contact_email
            .as_deref()
            .filter(|s| !s.is_empty())
            .is_some()
        || dataset
            .contact_url
            .as_deref()
            .filter(|s| !s.is_empty())
            .is_some();

    if has_contact {
        ttl.push_str("    dcat:contactPoint _:cp .\n\n");
        ttl.push_str("_:cp a vcard:Organization ;\n");
        if let Some(name) = dataset.contact_name.as_deref().filter(|s| !s.is_empty()) {
            ttl.push_str(&format!("    vcard:fn \"{}\" ;\n", escape_ttl_string(name)));
        }
        if let Some(email) = dataset.contact_email.as_deref().filter(|s| !s.is_empty()) {
            ttl.push_str(&format!(
                "    vcard:hasEmail <mailto:{}> ;\n",
                escape_sparql_iri(email)
            ));
        }
        if let Some(url) = dataset.contact_url.as_deref().filter(|s| !s.is_empty()) {
            ttl.push_str(&format!(
                "    vcard:hasURL <{}> ;\n",
                escape_sparql_iri(url)
            ));
        }
        ttl.push_str("    .\n");
    } else {
        ttl.push_str("    .\n");
    }

    // Per-graph role triples: void:subset + ots:graphRole for each registered graph.
    for entry in graph_entries {
        if !entry.graph_iri.starts_with("urn:system:") {
            ttl.push_str(&format!(
                "<{}> void:subset <{}> .\n",
                dataset_iri, entry.graph_iri
            ));
            if let Some(role) = entry.graph_role {
                let role_iri = graph_role_iri(role);
                ttl.push_str(&format!(
                    "<{}> ots:graphRole <{}> .\n",
                    entry.graph_iri, role_iri
                ));
            }
        }
    }

    ttl
}

/// Rewrite every dataset's DCAT metadata graph so pre-existing datasets adopt the
/// canonical **singular** dataset IRI (`{base}/dataset/{id}`, styleguide §3.3).
///
/// Older builds wrote the subject as `{base}/datasets/{id}` (plural) while the
/// catalogue, version registry and API-service registry always used singular —
/// so a dataset's descriptive metadata (title, members, contact) lived under a
/// different IRI than the triples pointing at it, and its node rendered split and
/// incomplete when browsed. `write_dataset_metadata_graph` does a PUT (clear then
/// load), so re-running it simply replaces the old plural-subject triples. Cheap
/// (a handful of datasets) and idempotent — safe to run on every boot.
pub fn reconcile_all_dataset_metadata(
    store: &TripleStore,
    base_url: &str,
    db: &crate::auth::db::AuthDb,
) {
    let datasets = match db.list_datasets() {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!("dataset metadata reconcile: list failed: {e}");
            return;
        }
    };
    let mut migrated = 0usize;
    for ds in &datasets {
        // Only rewrite datasets still carrying the OLD plural subject. Rewriting
        // is expensive (clear + reload + graph-index refresh per dataset), so the
        // guard keeps this a cheap no-op on every boot after the one-time
        // migration — and leaves brand-new (already-singular) datasets untouched.
        let graph_iri = dataset_metadata_graph_iri(&ds.id);
        let plural_subject = format!("{}/datasets/{}", base_url.trim_end_matches('/'), ds.id);
        if !graph_has_subject(store, &graph_iri, &plural_subject) {
            continue;
        }
        let entries = db.list_dataset_graph_entries(&ds.id).unwrap_or_default();
        write_dataset_metadata_graph(store, base_url, ds, &entries);
        migrated += 1;
    }
    if migrated > 0 {
        tracing::info!(
            "dataset metadata reconcile: migrated {migrated} dataset metadata graph(s) onto the \
             canonical singular dataset IRI scheme"
        );
    }
}

// ─── Model / Vocabulary / Instance reframe migration ────────────────────────────

/// System graph recording which one-shot content migrations have run.
const MIGRATIONS_GRAPH: &str = "urn:system:migrations";
/// Sentinel subject for the Model/Vocabulary reframe + drop-"ontology" migration.
const REFRAME_MIGRATION_IRI: &str = "urn:system:migration:model-vocabulary-reframe";

/// One-time content migration for the Model / Vocabulary / Instance reframe and
/// the drop of the legacy `https://opentriplestore.org/ontology/` IRI base.
///
/// Two parts:
///  1. **Every-boot, idempotent** rewrite of legacy `…/ontology/` predicates in
///     the tiny `urn:system:audit` and `urn:system:validation-layer` graphs to the
///     new `…/ns#` base (a no-op once done; these graphs are small).
///  2. **One-shot** re-detection of stored `model` graphs: a property/SKOS-only
///     graph (R-Box) classified `model` under the old detector is moved to
///     `vocabulary`, and the owning dataset's metadata graph is re-emitted so its
///     `graphRole` triple reflects the new role. Guarded by a sentinel triple so
///     graph contents are scanned at most once.
///
/// Dataset metadata graphs additionally self-heal via `audit_dataset_metadata`
/// (the old `…/ontology/visibility` IRI fails the refreshed dataset-structure
/// shape, triggering a re-emit with the new IRIs), and the DCAT catalogue is
/// generated on the fly — so this function only has to handle the role flips and
/// the two system graphs.
pub fn migrate_model_vocabulary_reframe(
    store: &TripleStore,
    base_url: &str,
    db: &crate::auth::db::AuthDb,
) {
    // Gate the whole one-shot migration — including the legacy-IRI rewrite —
    // behind the applied sentinel. The rewrite is idempotent, but each of its
    // `store.update()`s triggers a full graph-index rebuild (a scan of every
    // graph, including a dataset's multi-million-triple `…/ifcowl` lift), so
    // running it on every boot was a recurring full-store scan for no effect.
    if graph_has_subject(store, MIGRATIONS_GRAPH, REFRAME_MIGRATION_IRI) {
        return;
    }
    rewrite_legacy_ontology_iris(store);
    let reclassified = reclassify_model_graphs_to_vocabulary(store, base_url, db);
    if reclassified > 0 {
        tracing::info!(
            "model/vocabulary reframe: reclassified {reclassified} property graph(s) \
             model→vocabulary"
        );
    }
    let mark = format!(
        "INSERT DATA {{ GRAPH <{MIGRATIONS_GRAPH}> {{ <{REFRAME_MIGRATION_IRI}> \
         <urn:system:migration#applied> true }} }}"
    );
    if let Err(e) = store.update(&mark) {
        // Non-fatal but observable: if the sentinel write fails the migration
        // simply re-runs next boot (it is idempotent), but a persistent failure
        // means it never marks done — surface it instead of swallowing.
        tracing::warn!("model/vocabulary reframe: failed to record applied marker: {e}");
    }
}

/// Rewrite the two known legacy `…/ontology/` predicates to the `…/ns#` base in
/// the system graphs that materialise them. Idempotent (no-op once migrated).
fn rewrite_legacy_ontology_iris(store: &TripleStore) {
    const AUDIT_GRAPH: &str = "urn:system:audit";
    const VALIDATION_GRAPH: &str = "urn:system:validation-layer";
    let updates = [
        format!(
            "DELETE {{ GRAPH <{AUDIT_GRAPH}> {{ ?s <https://opentriplestore.org/ontology/auditStatus> ?o }} }} \
             INSERT {{ GRAPH <{AUDIT_GRAPH}> {{ ?s <https://opentriplestore.org/ns#auditStatus> ?o }} }} \
             WHERE  {{ GRAPH <{AUDIT_GRAPH}> {{ ?s <https://opentriplestore.org/ontology/auditStatus> ?o }} }}"
        ),
        format!(
            "DELETE {{ GRAPH <{VALIDATION_GRAPH}> {{ ?s <https://opentriplestore.org/ontology/validatedBy> ?o }} }} \
             INSERT {{ GRAPH <{VALIDATION_GRAPH}> {{ ?s <https://opentriplestore.org/ns#validatedBy> ?o }} }} \
             WHERE  {{ GRAPH <{VALIDATION_GRAPH}> {{ ?s <https://opentriplestore.org/ontology/validatedBy> ?o }} }}"
        ),
    ];
    for q in &updates {
        if let Err(e) = store.update(q) {
            tracing::warn!("legacy-IRI rewrite update failed (non-fatal): {e}");
        }
    }
}

/// Re-run content detection over every per-graph role currently stored as
/// `model`; flip to `vocabulary` when the graph is really R-Box (properties /
/// SKOS) with no class anchor. Never demotes a real class graph. Returns the
/// number of graphs reclassified.
fn reclassify_model_graphs_to_vocabulary(
    store: &TripleStore,
    base_url: &str,
    db: &crate::auth::db::AuthDb,
) -> usize {
    let datasets = match db.list_datasets() {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!("model/vocabulary reframe: list datasets failed: {e}");
            return 0;
        }
    };
    let mut reclassified = 0usize;
    for ds in &datasets {
        let entries = db.list_dataset_graph_entries(&ds.id).unwrap_or_default();
        let mut changed = false;
        for e in &entries {
            if e.graph_role == Some(GraphKind::Model)
                && detect_graph_role(store, &e.graph_iri) == Some(GraphKind::Vocabulary)
                && db
                    .set_dataset_graph_role(&ds.id, &e.graph_iri, Some(GraphKind::Vocabulary))
                    .is_ok()
            {
                reclassified += 1;
                changed = true;
            }
        }
        if changed {
            // Re-emit metadata so the stored graphRole triples match the new roles.
            let updated = db.list_dataset_graph_entries(&ds.id).unwrap_or_default();
            write_dataset_metadata_graph(store, base_url, ds, &updated);
        }
    }
    reclassified
}

/// Detect the [`GraphKind`] of a single named graph by scanning its quads.
/// Returns `None` for an empty or unclassifiable graph.
fn detect_graph_role(store: &TripleStore, graph_iri: &str) -> Option<GraphKind> {
    use oxigraph::model::{GraphNameRef, NamedNodeRef, Quad};
    let g = NamedNodeRef::new(graph_iri).ok()?;
    let quads: Vec<Quad> = store
        .store()
        .quads_for_pattern(None, None, None, Some(GraphNameRef::NamedNode(g)))
        .filter_map(|r| r.ok())
        .collect();
    if quads.is_empty() {
        return None;
    }
    crate::kind_detector::detect(&quads).to_graph_role()
}

/// True if `subject_iri` appears as a subject in the named graph `graph_iri`.
/// A cheap targeted lookup via the quad API (no full scan).
fn graph_has_subject(store: &TripleStore, graph_iri: &str, subject_iri: &str) -> bool {
    use oxigraph::model::{GraphNameRef, NamedNodeRef, NamedOrBlankNodeRef};
    let s = match NamedNodeRef::new(subject_iri) {
        Ok(n) => n,
        Err(_) => return false,
    };
    let g = match NamedNodeRef::new(graph_iri) {
        Ok(n) => n,
        Err(_) => return false,
    };
    store
        .store()
        .quads_for_pattern(
            Some(NamedOrBlankNodeRef::NamedNode(s)),
            None,
            None,
            Some(GraphNameRef::NamedNode(g)),
        )
        .next()
        .is_some()
}

fn escape_ttl_string(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

/// The IRI of a graph-role individual in the Open Triplestore vocabulary
/// (`https://opentriplestore.org/ns#{Role}`). Single source of truth, also used
/// by the DCAT catalogue (`dcat::catalog`) so the two never drift.
pub fn graph_role_iri(role: GraphKind) -> &'static str {
    match role {
        GraphKind::Instances => "https://opentriplestore.org/ns#Instances",
        GraphKind::Model => "https://opentriplestore.org/ns#Model",
        GraphKind::Vocabulary => "https://opentriplestore.org/ns#Vocabulary",
        GraphKind::Shapes => "https://opentriplestore.org/ns#Shapes",
        GraphKind::Entailment => "https://opentriplestore.org/ns#Entailment",
        GraphKind::System => "https://opentriplestore.org/ns#System",
    }
}
