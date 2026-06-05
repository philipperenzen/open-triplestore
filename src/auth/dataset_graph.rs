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
    ttl.push_str("@prefix ots:  <https://opentriplestore.org/ontology/> .\n\n");

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

/// True if `subject_iri` appears as a subject in the named graph `graph_iri`.
/// A cheap targeted lookup via the quad API (no full scan).
fn graph_has_subject(store: &TripleStore, graph_iri: &str, subject_iri: &str) -> bool {
    use oxigraph::model::{GraphNameRef, NamedNodeRef, SubjectRef};
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
            Some(SubjectRef::NamedNode(s)),
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

fn graph_role_iri(role: GraphKind) -> &'static str {
    match role {
        GraphKind::Instances => "https://opentriplestore.org/ontology/Instances",
        GraphKind::Model => "https://opentriplestore.org/ontology/Model",
        GraphKind::Vocabulary => "https://opentriplestore.org/ontology/Vocabulary",
        GraphKind::Shapes => "https://opentriplestore.org/ontology/Shapes",
        GraphKind::Entailment => "https://opentriplestore.org/ontology/Entailment",
        GraphKind::System => "https://opentriplestore.org/ontology/System",
    }
}
