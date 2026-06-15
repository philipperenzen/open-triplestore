//! Full DCAT 2 catalog generation from the dataset registry and store.
//!
//! Produces a Turtle document containing:
//! - `dcat:Catalog` with publisher, license, language
//! - `dcat:Dataset` per registered dataset with distributions, themes, keywords
//! - `void:Dataset` statistics (triple counts, distinct subjects, etc.)
//! - `dcat:Distribution` per SPARQL endpoint and per asset
//! - Organization metadata from dataset owners

use std::fmt::Write;
use std::sync::Arc;

use crate::auth::db::AuthDb;
use crate::auth::models::{Dataset, Organisation, OwnerType};
use crate::store::engine::TripleStore;

use super::vocabulary::*;

/// Generate a full DCAT 2 catalog as Turtle.
///
/// This replaces the simpler VoID-only description with a comprehensive
/// W3C DCAT 2 catalog that includes VoID statistics.
pub fn generate_dcat_catalog(
    base_url: &str,
    store: &TripleStore,
    auth_db: &Arc<AuthDb>,
    user_id: Option<&str>,
) -> String {
    let mut out = String::with_capacity(4096);

    // Prefix declarations
    writeln!(out, "@prefix dcat: <{DCAT}> .").unwrap();
    writeln!(out, "@prefix dct:  <{DCT}> .").unwrap();
    writeln!(out, "@prefix void: <{VOID}> .").unwrap();
    writeln!(out, "@prefix foaf: <{FOAF}> .").unwrap();
    writeln!(out, "@prefix prov: <{PROV}> .").unwrap();
    writeln!(out, "@prefix org:  <{ORG}> .").unwrap();
    writeln!(out, "@prefix adms: <{ADMS}> .").unwrap();
    writeln!(out, "@prefix schema: <{SCHEMA}> .").unwrap();
    writeln!(out, "@prefix vcard: <{VCARD}> .").unwrap();
    writeln!(out, "@prefix xsd:  <{XSD}> .").unwrap();
    writeln!(out, "@prefix sd:   <{SD}> .").unwrap();
    writeln!(out).unwrap();

    // ── Catalog ──────────────────────────────────────────────────────────

    let total_triples = store.len().unwrap_or(0);
    let datasets: Vec<Dataset> = auth_db
        .list_datasets()
        .unwrap_or_default()
        .into_iter()
        .filter(|ds| auth_db.can_access_dataset(user_id, ds).unwrap_or(false))
        .collect();

    writeln!(out, "<{base_url}/catalog>").unwrap();
    writeln!(out, "    a dcat:Catalog ;").unwrap();
    writeln!(out, "    dct:title \"Open Triplestore Catalog\" ;").unwrap();
    writeln!(
        out,
        "    dct:description \"DCAT 2 catalog for the Open Triplestore instance.\" ;"
    )
    .unwrap();
    writeln!(
        out,
        "    dct:language <http://id.loc.gov/vocabulary/iso639-1/en> ;"
    )
    .unwrap();
    writeln!(out, "    foaf:homepage <{base_url}/> ;").unwrap();

    // Link datasets
    if datasets.is_empty() {
        // Still emit the root dataset
        writeln!(out, "    dcat:dataset <{base_url}/dataset> .").unwrap();
    } else {
        for (i, ds) in datasets.iter().enumerate() {
            let comma = if i < datasets.len() - 1 { " ," } else { " ." };
            writeln!(
                out,
                "    dcat:dataset <{base_url}/dataset/{id}>{comma}",
                id = ds.id
            )
            .unwrap();
        }
    }
    writeln!(out).unwrap();

    // ── Root VoID dataset (aggregate) ────────────────────────────────────

    let graph_count = store.named_graphs().map(|g| g.len()).unwrap_or(0);
    let distinct_subjects = count_via_sparql(
        store,
        "SELECT (COUNT(DISTINCT ?s) AS ?c) WHERE { ?s ?p ?o }",
    );
    let distinct_predicates = count_via_sparql(
        store,
        "SELECT (COUNT(DISTINCT ?p) AS ?c) WHERE { ?s ?p ?o }",
    );
    let distinct_objects = count_via_sparql(
        store,
        "SELECT (COUNT(DISTINCT ?o) AS ?c) WHERE { ?s ?p ?o }",
    );

    writeln!(out, "<{base_url}/dataset>").unwrap();
    writeln!(out, "    a void:Dataset, dcat:Dataset ;").unwrap();
    writeln!(out, "    dct:title \"Open Triplestore\" ;").unwrap();
    writeln!(out, "    void:sparqlEndpoint <{base_url}/sparql> ;").unwrap();
    writeln!(out, "    void:uriSpace \"{base_url}/resource/\" ;").unwrap();
    writeln!(out, "    void:triples {total_triples} ;").unwrap();
    writeln!(out, "    void:distinctSubjects {distinct_subjects} ;").unwrap();
    writeln!(out, "    void:distinctObjects {distinct_objects} ;").unwrap();
    writeln!(out, "    void:properties {distinct_predicates} ;").unwrap();
    writeln!(out, "    void:documents {graph_count} ;").unwrap();
    writeln!(out, "    dcat:distribution [").unwrap();
    writeln!(out, "        a dcat:Distribution ;").unwrap();
    writeln!(out, "        dcat:accessURL <{base_url}/sparql> ;").unwrap();
    writeln!(out, "        dct:title \"SPARQL Endpoint\" ;").unwrap();
    writeln!(out, "        dcat:mediaType <https://www.iana.org/assignments/media-types/application/sparql-query>").unwrap();
    writeln!(out, "    ] ;").unwrap();
    writeln!(out, "    dcat:landingPage <{base_url}/> .").unwrap();
    writeln!(out).unwrap();

    // ── Per-dataset entries ──────────────────────────────────────────────

    for ds in &datasets {
        emit_dataset_entry(&mut out, base_url, store, auth_db, ds);
    }

    // ── SPARQL Service Description ──────────────────────────────────────

    writeln!(out, "<{base_url}/sparql>").unwrap();
    writeln!(out, "    a sd:Service ;").unwrap();
    writeln!(out, "    sd:endpoint <{base_url}/sparql> ;").unwrap();
    writeln!(
        out,
        "    sd:supportedLanguage sd:SPARQL11Query, sd:SPARQL11Update ."
    )
    .unwrap();
    writeln!(out).unwrap();

    out
}

/// Generate a DCAT 2 catalog scoped to a single organisation as Turtle.
///
/// Only datasets owned by the org and accessible to `user_id` are included.
/// Unauthenticated callers see only `Public` datasets; authenticated callers
/// additionally see `Members`/`Private` datasets they have access to.
pub fn generate_org_dcat_catalog(
    org: &Organisation,
    base_url: &str,
    store: &TripleStore,
    auth_db: &Arc<AuthDb>,
    user_id: Option<&str>,
) -> String {
    let mut out = String::with_capacity(2048);

    writeln!(out, "@prefix dcat: <{DCAT}> .").unwrap();
    writeln!(out, "@prefix dct:  <{DCT}> .").unwrap();
    writeln!(out, "@prefix void: <{VOID}> .").unwrap();
    writeln!(out, "@prefix foaf: <{FOAF}> .").unwrap();
    writeln!(out, "@prefix org:  <{ORG}> .").unwrap();
    writeln!(out, "@prefix adms: <{ADMS}> .").unwrap();
    writeln!(out, "@prefix schema: <{SCHEMA}> .").unwrap();
    writeln!(out, "@prefix vcard: <{VCARD}> .").unwrap();
    writeln!(out, "@prefix xsd:  <{XSD}> .").unwrap();
    writeln!(out, "@prefix sd:   <{SD}> .").unwrap();
    writeln!(out).unwrap();

    let datasets: Vec<Dataset> = auth_db
        .list_datasets_by_org(&org.id)
        .unwrap_or_default()
        .into_iter()
        .filter(|ds| auth_db.can_access_dataset(user_id, ds).unwrap_or(false))
        .collect();

    let org_uri = format!("{base_url}/org/{}", org.id);
    let catalog_uri = format!("{base_url}/{}/catalog", org.slug);

    writeln!(out, "<{catalog_uri}>").unwrap();
    writeln!(out, "    a dcat:Catalog ;").unwrap();
    writeln!(
        out,
        "    dct:title \"{}\" ;",
        escape_turtle(&format!("{} Catalog", org.name))
    )
    .unwrap();
    writeln!(out, "    dct:publisher <{org_uri}> ;").unwrap();
    writeln!(out, "    foaf:homepage <{base_url}/{}/> ;", org.slug).unwrap();

    if datasets.is_empty() {
        writeln!(out, "    dcat:dataset <{base_url}/dataset> .").unwrap();
    } else {
        for (i, ds) in datasets.iter().enumerate() {
            let comma = if i < datasets.len() - 1 { " ," } else { " ." };
            writeln!(
                out,
                "    dcat:dataset <{base_url}/dataset/{id}>{comma}",
                id = ds.id
            )
            .unwrap();
        }
    }
    writeln!(out).unwrap();

    // Organisation metadata
    writeln!(out, "<{org_uri}>").unwrap();

    // RDF type — always foaf:Organization, plus the specialised W3C ORG type.
    let org_type = org.org_type.as_deref().unwrap_or("FormalOrganization");
    match org_type {
        "OrganizationalUnit" => {
            writeln!(out, "    a foaf:Organization, org:OrganizationalUnit ;").unwrap();
        }
        "Organization" => {
            writeln!(out, "    a foaf:Organization ;").unwrap();
        }
        _ => {
            // Default: FormalOrganization
            writeln!(out, "    a foaf:Organization, org:FormalOrganization ;").unwrap();
        }
    }

    writeln!(out, "    foaf:name \"{}\" ;", escape_turtle(&org.name)).unwrap();
    if let Some(ref desc) = org.description {
        writeln!(out, "    dct:description \"{}\" ;", escape_turtle(desc)).unwrap();
    }
    if let Some(ref hp) = org.homepage {
        writeln!(out, "    foaf:homepage <{}> ;", hp).unwrap();
    }
    if let Some(ref ident) = org.identifier {
        writeln!(out, "    dct:identifier \"{}\" ;", escape_turtle(ident)).unwrap();
    }

    // vCard contact point (blank node)
    let has_contact =
        org.contact_name.is_some() || org.contact_email.is_some() || org.contact_url.is_some();
    if has_contact {
        writeln!(out, "    dcat:contactPoint [").unwrap();
        writeln!(out, "        a vcard:Organization ;").unwrap();
        if let Some(ref cn) = org.contact_name {
            writeln!(out, "        vcard:fn \"{}\" ;", escape_turtle(cn)).unwrap();
        }
        if let Some(ref ce) = org.contact_email {
            writeln!(out, "        vcard:hasEmail <mailto:{}> ;", ce).unwrap();
        }
        if let Some(ref cu) = org.contact_url {
            writeln!(out, "        vcard:hasURL <{}> ;", cu).unwrap();
        }
        // Remove trailing semicolon from last property — write final line without it
        writeln!(out, "    ] ;").unwrap();
    }

    // Close org node — strip the trailing semicolon from the last property and use a period.
    // We use a sentinel comment line then post-process, but the simpler approach is to track
    // whether any trailing properties follow. Since SPARQL Turtle allows a trailing semicolon
    // before the period, we just write the dct:publisher back-reference and close with ' .'
    writeln!(out, "    dct:publisher <{catalog_uri}> .").unwrap();
    writeln!(out).unwrap();

    // Per-dataset entries
    for ds in &datasets {
        emit_dataset_entry(&mut out, base_url, store, auth_db, ds);
    }

    // SPARQL Service Description
    writeln!(out, "<{base_url}/sparql>").unwrap();
    writeln!(out, "    a sd:Service ;").unwrap();
    writeln!(out, "    sd:endpoint <{base_url}/sparql> ;").unwrap();
    writeln!(
        out,
        "    sd:supportedLanguage sd:SPARQL11Query, sd:SPARQL11Update ."
    )
    .unwrap();
    writeln!(out).unwrap();

    out
}

/// Emit a `dcat:Dataset` entry for a single registered dataset.
fn emit_dataset_entry(
    out: &mut String,
    base_url: &str,
    store: &TripleStore,
    auth_db: &Arc<AuthDb>,
    ds: &Dataset,
) {
    let ds_uri = format!("{base_url}/dataset/{}", ds.id);

    writeln!(out, "<{ds_uri}>").unwrap();
    writeln!(out, "    a dcat:Dataset, void:Dataset ;").unwrap();
    writeln!(out, "    dct:title \"{}\" ;", escape_turtle(&ds.name)).unwrap();
    if let Some(ref desc) = ds.description {
        writeln!(out, "    dct:description \"{}\" ;", escape_turtle(desc)).unwrap();
    }
    writeln!(out, "    dct:issued \"{}\"^^xsd:dateTime ;", ds.created_at).unwrap();
    writeln!(
        out,
        "    dct:modified \"{}\"^^xsd:dateTime ;",
        ds.updated_at
    )
    .unwrap();

    // Visibility
    let access_rights = match ds.visibility {
        crate::auth::models::Visibility::Public => {
            "http://publications.europa.eu/resource/authority/access-right/PUBLIC"
        }
        crate::auth::models::Visibility::Members => {
            "http://publications.europa.eu/resource/authority/access-right/RESTRICTED"
        }
        crate::auth::models::Visibility::Private => {
            "http://publications.europa.eu/resource/authority/access-right/NON_PUBLIC"
        }
    };
    writeln!(out, "    dct:accessRights <{access_rights}> ;").unwrap();

    // Publisher / creator from owner
    match ds.owner_type {
        OwnerType::Organisation => {
            if let Ok(Some(org)) = auth_db.get_organisation(&ds.owner_id) {
                let org_uri = format!("{base_url}/org/{}", org.id);
                writeln!(out, "    dct:publisher <{org_uri}> ;").unwrap();
            }
        }
        OwnerType::User => {
            let user_uri = format!("{base_url}/user/{}", ds.owner_id);
            writeln!(out, "    dct:creator <{user_uri}> ;").unwrap();
        }
        OwnerType::Group => {
            let group_uri = format!("{base_url}/group/{}", ds.owner_id);
            writeln!(out, "    dct:publisher <{group_uri}> ;").unwrap();
        }
    }

    // Per-graph VoID statistics + role-typed subsets
    let graph_entries = auth_db
        .list_dataset_graph_entries(&ds.id)
        .unwrap_or_default();
    let mut ds_triple_count: usize = 0;
    for entry in &graph_entries {
        let count = count_graph_triples(store, &entry.graph_iri);
        ds_triple_count += count;
        // Emit void:subset link for each registered graph.
        if !entry.graph_iri.starts_with("urn:system:") {
            writeln!(out, "    void:subset <{}> ;", entry.graph_iri).unwrap();
        }
    }
    writeln!(out, "    void:triples {ds_triple_count} ;").unwrap();

    // SPARQL endpoint distribution
    writeln!(out, "    dcat:distribution [").unwrap();
    writeln!(out, "        a dcat:Distribution ;").unwrap();
    writeln!(out, "        dcat:accessURL <{base_url}/sparql> ;").unwrap();
    writeln!(out, "        dct:title \"SPARQL Endpoint\"").unwrap();
    writeln!(out, "    ] ;").unwrap();

    // Graph Store distribution
    writeln!(out, "    dcat:distribution [").unwrap();
    writeln!(out, "        a dcat:Distribution ;").unwrap();
    writeln!(out, "        dcat:accessURL <{base_url}/store> ;").unwrap();
    writeln!(out, "        dct:title \"Graph Store HTTP Protocol\" ;").unwrap();
    writeln!(
        out,
        "        dcat:mediaType <https://www.iana.org/assignments/media-types/text/turtle>"
    )
    .unwrap();
    writeln!(out, "    ] ;").unwrap();

    // SHACL conformance (shapes graph)
    if ds.shacl_on_write {
        if let Some(ref shapes_iri) = ds.shapes_graph_iri {
            writeln!(out, "    dct:conformsTo <{shapes_iri}> ;").unwrap();
        }
    }

    // Model conformance (links instance data to its data model).
    // `conforms_to_model` is a data-model registry id; the registry serves
    // model IRIs under /data-model/ (see src/data_models/registry.rs), so the
    // conformance link must dereference there, not /ontology/.
    if let Some(ref onto_id) = ds.conforms_to_model {
        if let Some(ref onto_ver) = ds.conforms_to_version {
            writeln!(
                out,
                "    dct:conformsTo <{base_url}/data-model/{onto_id}/version/{onto_ver}> ;"
            )
            .unwrap();
        } else {
            writeln!(
                out,
                "    dct:conformsTo <{base_url}/data-model/{onto_id}> ;"
            )
            .unwrap();
        }
    }

    // DCAT metadata fields
    if let Some(ref lic) = ds.license {
        if !lic.is_empty() {
            writeln!(out, "    dct:license <{lic}> ;").unwrap();
        }
    }
    if let Some(ref themes_json) = ds.themes {
        if let Ok(themes) = serde_json::from_str::<Vec<String>>(themes_json) {
            for theme in &themes {
                if !theme.is_empty() {
                    writeln!(out, "    dcat:theme <{theme}> ;").unwrap();
                }
            }
        }
    }
    if let Some(ref kw_json) = ds.keywords {
        if let Ok(keywords) = serde_json::from_str::<Vec<String>>(kw_json) {
            for kw in &keywords {
                if !kw.is_empty() {
                    writeln!(out, "    dcat:keyword \"{}\"@en ;", escape_turtle(kw)).unwrap();
                }
            }
        }
    }
    if let Some(ref status) = ds.adms_status {
        if !status.is_empty() {
            writeln!(out, "    adms:status <{status}> ;").unwrap();
        }
    }
    if let Some(ref notes) = ds.version_notes {
        if !notes.is_empty() {
            writeln!(out, "    adms:versionNotes \"{}\" ;", escape_turtle(notes)).unwrap();
        }
    }
    if let Some(ref spatial) = ds.spatial {
        if !spatial.is_empty() {
            writeln!(out, "    dct:spatial <{spatial}> ;").unwrap();
        }
    }

    // Contact point
    let has_contact = ds
        .contact_name
        .as_deref()
        .filter(|s| !s.is_empty())
        .is_some()
        || ds
            .contact_email
            .as_deref()
            .filter(|s| !s.is_empty())
            .is_some()
        || ds
            .contact_url
            .as_deref()
            .filter(|s| !s.is_empty())
            .is_some();
    if has_contact {
        writeln!(out, "    dcat:contactPoint [").unwrap();
        writeln!(out, "        a vcard:Organization ;").unwrap();
        if let Some(name) = ds.contact_name.as_deref().filter(|s| !s.is_empty()) {
            writeln!(out, "        vcard:fn \"{}\" ;", escape_turtle(name)).unwrap();
        }
        if let Some(email) = ds.contact_email.as_deref().filter(|s| !s.is_empty()) {
            writeln!(out, "        vcard:hasEmail <mailto:{email}> ;").unwrap();
        }
        if let Some(url) = ds.contact_url.as_deref().filter(|s| !s.is_empty()) {
            writeln!(out, "        vcard:hasURL <{url}>").unwrap();
        }
        writeln!(out, "    ] ;").unwrap();
    }

    // Geospatial distributions — only when the dataset actually carries geometry.
    // DCAT 2 §4.3/§5.3: advertise the OGC API – Features, 3D Tiles, and viewer
    // services as `dcat:Distribution`/`dcat:accessService` nodes so harvesters can
    // discover the spatial access paths alongside the SPARQL/GSP ones above.
    // Exclude the verbose ifcOWL lift graph (`…/ifcowl`) just like the viewer-feed
    // and geo-stats handlers do (routes.rs): it is the full 1:1 IFC schema (millions
    // of triples), carries none of the geometry the probe looks for, and its
    // unbounded scan would dominate the per-dataset capability check run here.
    let data_graphs: Vec<String> = graph_entries
        .iter()
        .filter(|e| !e.graph_iri.starts_with("urn:system:"))
        .filter(|e| !e.graph_iri.ends_with("/ifcowl"))
        .map(|e| e.graph_iri.clone())
        .collect();
    let geo = crate::geo::viewer_feed::dataset_geo_stats(store, &data_graphs);
    if geo.has_coordinates || geo.has_3d {
        // OGC API – Features landing for this dataset (GeoJSON FeatureCollection).
        writeln!(out, "    dcat:distribution [").unwrap();
        writeln!(out, "        a dcat:Distribution ;").unwrap();
        writeln!(out, "        dct:title \"OGC API – Features (GeoJSON)\" ;").unwrap();
        writeln!(
            out,
            "        dcat:accessURL <{base_url}/api/ogc/collections/{id}/items> ;",
            id = ds.id
        )
        .unwrap();
        writeln!(out, "        dcat:mediaType <https://www.iana.org/assignments/media-types/application/geo+json> ;").unwrap();
        writeln!(
            out,
            "        dct:conformsTo <http://www.opengis.net/spec/ogcapi-features-1/1.0/conf/core> ;"
        )
        .unwrap();
        // General OGC API – Features service endpoint backing this distribution.
        writeln!(out, "        dcat:accessService [").unwrap();
        writeln!(out, "            a dcat:DataService ;").unwrap();
        writeln!(out, "            dct:title \"OGC API – Features\" ;").unwrap();
        writeln!(out, "            dcat:endpointURL <{base_url}/api/ogc> ;").unwrap();
        writeln!(
            out,
            "            dct:conformsTo <http://www.opengis.net/spec/ogcapi-features-1/1.0/conf/core>"
        )
        .unwrap();
        writeln!(out, "        ]").unwrap();
        writeln!(out, "    ] ;").unwrap();

        // 3D Tiles tileset — only when the dataset has volumetric/model 3D data.
        if geo.has_3d {
            writeln!(out, "    dcat:distribution [").unwrap();
            writeln!(out, "        a dcat:Distribution ;").unwrap();
            writeln!(out, "        dct:title \"OGC 3D Tiles 1.1\" ;").unwrap();
            writeln!(
                out,
                "        dcat:accessURL <{base_url}/api/datasets/{id}/3dtiles/tileset.json> ;",
                id = ds.id
            )
            .unwrap();
            writeln!(out, "        dcat:mediaType \"application/json\" ;").unwrap();
            writeln!(
                out,
                "        dct:conformsTo <https://docs.ogc.org/cs/22-025r4/22-025r4.html>"
            )
            .unwrap();
            writeln!(out, "    ] ;").unwrap();
        }

        // Viewer-feed JSON (per-element geometry + 3D-file references).
        writeln!(out, "    dcat:distribution [").unwrap();
        writeln!(out, "        a dcat:Distribution ;").unwrap();
        writeln!(out, "        dct:title \"Viewer Feed (JSON)\" ;").unwrap();
        writeln!(
            out,
            "        dcat:accessURL <{base_url}/api/datasets/{id}/viewer-feed> ;",
            id = ds.id
        )
        .unwrap();
        writeln!(out, "        dcat:mediaType \"application/json\"").unwrap();
        writeln!(out, "    ] ;").unwrap();

        // TODO(dcat §6.4.3): when this dataset has registered versions, advertise
        // the version-scoped geometry endpoints via `dct:hasVersion` on each
        // distribution. Dataset version records are not readily available in this
        // generation pass, so the per-version geometry links are deferred.
    }

    let landing = ds
        .landing_page
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or(base_url);
    writeln!(out, "    dcat:landingPage <{landing}> .").unwrap();
    writeln!(out).unwrap();

    // Emit per-graph role triples (ots:graphRole on each void:subset).
    for entry in &graph_entries {
        if !entry.graph_iri.starts_with("urn:system:") {
            if let Some(role) = entry.graph_role {
                let role_iri = crate::auth::dataset_graph::graph_role_iri(role);
                writeln!(out, "<{}>", entry.graph_iri).unwrap();
                writeln!(
                    out,
                    "    <https://opentriplestore.org/ns#graphRole> <{role_iri}> ."
                )
                .unwrap();
                writeln!(out).unwrap();
            }
        }
    }

    // Emit organisation metadata if publisher is org
    if ds.owner_type == OwnerType::Organisation {
        if let Ok(Some(org)) = auth_db.get_organisation(&ds.owner_id) {
            let org_uri = format!("{base_url}/org/{}", org.id);
            writeln!(out, "<{org_uri}>").unwrap();
            let org_type = org.org_type.as_deref().unwrap_or("FormalOrganization");
            match org_type {
                "OrganizationalUnit" => {
                    writeln!(out, "    a foaf:Organization, org:OrganizationalUnit ;").unwrap();
                }
                "Organization" => {
                    writeln!(out, "    a foaf:Organization ;").unwrap();
                }
                _ => {
                    writeln!(out, "    a foaf:Organization, org:FormalOrganization ;").unwrap();
                }
            }
            writeln!(out, "    foaf:name \"{}\" ;", escape_turtle(&org.name)).unwrap();
            if let Some(ref desc) = org.description {
                writeln!(out, "    dct:description \"{}\" ;", escape_turtle(desc)).unwrap();
            }
            if let Some(ref hp) = org.homepage {
                writeln!(out, "    foaf:homepage <{}> ;", hp).unwrap();
            }
            if let Some(ref ident) = org.identifier {
                writeln!(out, "    dct:identifier \"{}\" ;", escape_turtle(ident)).unwrap();
            }
            writeln!(out, "    .").unwrap();
            writeln!(out).unwrap();
        }
    }
}

/// Run a COUNT SPARQL query and return the count, or 0 on error.
fn count_via_sparql(store: &TripleStore, sparql: &str) -> usize {
    match store.query(sparql) {
        Ok(oxigraph::sparql::QueryResults::Solutions(mut solutions)) => {
            if let Some(Ok(solution)) = solutions.next() {
                if let Some(oxigraph::model::Term::Literal(lit)) = solution.get(0) {
                    lit.value().parse::<usize>().unwrap_or(0)
                } else {
                    0
                }
            } else {
                0
            }
        }
        _ => 0,
    }
}

/// Count triples in a specific named graph.
///
/// Reads the maintained O(1) per-graph count index instead of a GRAPH-wrapped
/// `COUNT(*)` SPARQL. The SPARQL form does not match the fast-count fast path
/// (which only recognises a bare default-graph BGP), so it routes through the
/// store and scans the whole graph — for an IFC dataset's multi-million-triple
/// `…/ifcowl` graph that is a full scan per dataset on every uncached catalog
/// render. The cached count is value-identical for a single named graph (no
/// RDF-merge dedup); fall back to a direct quad count only if the graph is not
/// yet in the index.
fn count_graph_triples(store: &TripleStore, graph_iri: &str) -> usize {
    store
        .graph_count_cached(Some(graph_iri))
        .or_else(|| store.count_graph(Some(graph_iri)).ok())
        .unwrap_or(0)
}

/// Escape string for Turtle literal (double-quote delimited).
fn escape_turtle(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::models::Visibility;

    /// Build an in-memory dataset with the given model-conformance fields.
    fn dataset_with_conformance(
        db: &Arc<AuthDb>,
        onto: Option<&str>,
        ver: Option<&str>,
    ) -> Dataset {
        db.create_dataset(
            "ds-1",
            "Library Catalogue 2025",
            None,
            OwnerType::User,
            "u1",
            Visibility::Public,
            None,
        )
        .unwrap();
        db.update_dataset_conformance("ds-1", onto, ver).unwrap();
        db.get_dataset("ds-1").unwrap().unwrap()
    }

    /// The conformance link must dereference at the model registry's `/data-model/`
    /// path, matching src/data_models/registry.rs — never the legacy `/ontology/`.
    #[test]
    fn conforms_to_uses_data_model_path_with_version() {
        let db = Arc::new(AuthDb::in_memory().unwrap());
        let store = TripleStore::in_memory().unwrap();
        let ds = dataset_with_conformance(&db, Some("library-catalogue-model"), Some("2.1.0"));

        let mut out = String::new();
        emit_dataset_entry(&mut out, "http://example.org", &store, &db, &ds);

        assert!(
            out.contains("dct:conformsTo <http://example.org/data-model/library-catalogue-model/version/2.1.0>"),
            "expected /data-model/ versioned conformance IRI, got:\n{out}"
        );
        assert!(
            !out.contains("/ontology/library-catalogue-model"),
            "must not emit legacy /ontology/ conformance path, got:\n{out}"
        );
    }

    #[test]
    fn conforms_to_uses_data_model_path_without_version() {
        let db = Arc::new(AuthDb::in_memory().unwrap());
        let store = TripleStore::in_memory().unwrap();
        let ds = dataset_with_conformance(&db, Some("library-catalogue-model"), None);

        let mut out = String::new();
        emit_dataset_entry(&mut out, "http://example.org", &store, &db, &ds);

        assert!(
            out.contains("dct:conformsTo <http://example.org/data-model/library-catalogue-model>"),
            "expected /data-model/ unversioned conformance IRI, got:\n{out}"
        );
        assert!(
            !out.contains("/ontology/library-catalogue-model"),
            "must not emit legacy /ontology/ conformance path, got:\n{out}"
        );
    }
}
