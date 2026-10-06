//! The DCAT 3 catalogue of the model registry: published data models and
//! vocabularies, each a `dcat:Dataset` with its released versions described
//! per DCAT 3 §11 and one distribution per serialisation the data endpoint
//! actually serves.
//!
//! Built as triples through the dataset catalogue's builder
//! ([`crate::dcat::graph::G`]), so a title or a namespace cannot inject
//! syntax, every IRI is checked, and objects carry their DCAT range classes.

use std::sync::Arc;

use oxigraph::model::{BlankNode, NamedNode, Triple};

use crate::auth::db::AuthDb;
use crate::data_models::models::{DataModelRecord, DataModelVersion, VersionStatus};
use crate::data_models::registry as dm_registry;
use crate::dcat::authority;
use crate::dcat::catalog::{self as dcat_catalog, CatalogOptions};
use crate::dcat::graph::{nn, p, Range, G};
use crate::dcat::vocabulary::*;
use crate::kind_detector::RegistryKind;
use crate::store::TripleStore;

/// The serialisations `GET /api/models/{id}/…/data?format=` serves:
/// `(format parameter, IANA media type, EU file type)`.
const FORMATS: &[(&str, &str, &str)] = &[
    ("turtle", "text/turtle", "RDF_TURTLE"),
    ("ntriples", "application/n-triples", "RDF_N_TRIPLES"),
    ("nquads", "application/n-quads", "RDF_N_QUADS"),
    ("trig", "application/trig", "RDF_TRIG"),
];

/// The registry catalogue as triples, for the caller `user_id`.
pub fn build_registry_catalog(
    opts: &CatalogOptions,
    store: &TripleStore,
    auth_db: &Arc<AuthDb>,
    user_id: Option<&str>,
) -> Vec<Triple> {
    let base = opts.base_url.as_str();
    let tag = opts.lang_tag();
    let mut g = G::new(tag);

    let catalog = nn(&format!("{base}/catalog/registry"));
    g.typ(catalog.clone(), &p(DCAT, "Catalog"));
    g.lang(
        catalog.clone(),
        &p(DCT, "title"),
        "Linked Data Registry Catalog",
        "en",
    );
    g.lang(
        catalog.clone(),
        &p(DCT, "description"),
        "Published data-models and controlled vocabularies.",
        "en",
    );
    let publisher = dcat_catalog::catalog_publisher(&mut g, opts);
    g.add(catalog.clone(), &p(DCT, "publisher"), publisher.clone());
    g.ranged(
        catalog.clone(),
        &p(FOAF, "homepage"),
        &format!("{base}/"),
        Range::Document,
        "homepage",
    );
    g.ranged(
        catalog.clone(),
        &p(DCT, "language"),
        &opts.language_iri(),
        Range::LinguisticSystem,
        "language",
    );

    let models: Vec<DataModelRecord> = dm_registry::list_data_models(store)
        .into_iter()
        .filter(|d| d.latest_published.is_some())
        .filter(|d| {
            auth_db
                .can_access_ontology(
                    user_id,
                    d.is_public,
                    d.owner_type.as_deref(),
                    d.owner_id.as_deref(),
                )
                .unwrap_or(false)
        })
        .collect();

    for d in &models {
        let ds = nn(&dm_registry::data_model_iri(base, &d.id));
        g.add(catalog.clone(), &p(DCAT, "dataset"), ds.clone());
        model_entry(&mut g, opts, store, auth_db, d, &ds, &publisher);
    }
    g.triples
}

fn model_entry(
    g: &mut G,
    opts: &CatalogOptions,
    store: &TripleStore,
    auth_db: &Arc<AuthDb>,
    d: &DataModelRecord,
    ds: &NamedNode,
    catalog_publisher: &NamedNode,
) {
    let base = opts.base_url.as_str();
    g.typ(ds.clone(), &p(DCAT, "Dataset"));
    g.typ(ds.clone(), &p(VOID, "Dataset"));
    g.lit(ds.clone(), &p(DCT, "identifier"), &d.id);
    g.lang(ds.clone(), &p(DCT, "title"), &d.title, "en");
    if let Some(desc) = d.description.as_deref().filter(|s| !s.trim().is_empty()) {
        g.lang(ds.clone(), &p(DCT, "description"), desc, "en");
    }
    // An ontology is an ADMS Ontology asset; a SKOS vocabulary is a CodeList.
    let asset_type = if d.kind == RegistryKind::Vocabulary {
        "http://purl.org/adms/assettype/CodeList"
    } else {
        "http://purl.org/adms/assettype/Ontology"
    };
    g.concept(ds.clone(), &p(DCT, "type"), asset_type, "asset type");
    // The namespace is the model's own documentation page — when it is an IRI.
    g.ranged(
        ds.clone(),
        &p(FOAF, "page"),
        &d.namespace,
        Range::Document,
        "namespace",
    );
    g.ranged(
        ds.clone(),
        &p(DCAT, "landingPage"),
        ds.as_str(),
        Range::Document,
        "landing page",
    );
    g.when(ds.clone(), &p(DCT, "issued"), &d.created_at);
    let owner = match (d.owner_type.as_deref(), d.owner_id.as_deref()) {
        (Some("organisation"), Some(id)) => auth_db
            .get_organisation(id)
            .ok()
            .flatten()
            .map(|o| dcat_catalog::org_agent(g, opts, &o)),
        (Some("user"), Some(id)) => Some(dcat_catalog::user_agent(g, opts, auth_db, id)),
        _ => None,
    }
    .unwrap_or_else(|| catalog_publisher.clone());
    g.add(ds.clone(), &p(DCT, "publisher"), owner);

    let latest = d.latest_published.as_deref().unwrap_or("");
    let triple_count = count_triples(store, ds.as_str(), latest);
    if triple_count > 0 {
        g.int(ds.clone(), &p(VOID, "triples"), triple_count);
    }

    // Versions (DCAT 3 §11): every released version, the newest published as
    // the current one.
    let mut released: Vec<DataModelVersion> = dm_registry::list_versions(store, base, &d.id)
        .into_iter()
        .filter(|v| {
            matches!(
                v.status,
                VersionStatus::Published | VersionStatus::Deprecated
            )
        })
        .collect();
    released.sort_by(|a, b| a.created_at.cmp(&b.created_at));
    let iri_of = |v: &DataModelVersion| {
        NamedNode::new(dm_registry::version_record_iri(base, &d.id, &v.version)).ok()
    };
    for (i, v) in released.iter().enumerate() {
        let Some(vi) = iri_of(v) else {
            continue;
        };
        g.add(ds.clone(), &p(DCAT, "hasVersion"), vi.clone());
        g.typ(vi.clone(), &p(DCAT, "Dataset"));
        g.add(vi.clone(), &p(DCAT, "isVersionOf"), ds.clone());
        g.lit(vi.clone(), &p(DCAT, "version"), &v.version);
        g.lang(
            vi.clone(),
            &p(DCT, "title"),
            &format!("{} {}", d.title, v.version),
            "en",
        );
        if let Some(n) = v.notes.as_deref().filter(|n| !n.trim().is_empty()) {
            g.lang(vi.clone(), &p(ADMS, "versionNotes"), n.trim(), "en");
        }
        g.when(vi.clone(), &p(DCT, "issued"), &v.created_at);
        let status = match v.status {
            VersionStatus::Deprecated => "DEPRECATED",
            _ => "COMPLETED",
        };
        g.concept(
            vi.clone(),
            &p(ADMS, "status"),
            &format!("{}{status}", authority::EU_STATUS),
            "adms:status",
        );
        let prev = v
            .derived_from
            .as_deref()
            .and_then(|f| released[..i].iter().rev().find(|o| o.version == f))
            .or_else(|| released[..i].iter().rev().find(|o| o.branch == v.branch));
        if let Some(pv) = prev.and_then(iri_of) {
            g.add(vi.clone(), &p(DCAT, "previousVersion"), pv);
        }
        distributions(
            g,
            opts,
            &vi,
            &format!("{base}/api/models/{}/versions/{}/data", d.id, v.version),
            &format!("{} {}", d.title, v.version),
        );
    }
    if let Some(cur) = released
        .iter()
        .rev()
        .find(|v| v.version == latest && matches!(v.status, VersionStatus::Published))
        .and_then(iri_of)
    {
        g.add(ds.clone(), &p(DCAT, "hasCurrentVersion"), cur);
    }

    distributions(
        g,
        opts,
        ds,
        &format!("{base}/api/models/{}/latest/data", d.id),
        &d.title,
    );
}

fn distributions(g: &mut G, opts: &CatalogOptions, ds: &NamedNode, data_url: &str, title: &str) {
    for (fmt, mime, filetype) in FORMATS {
        let url = format!("{data_url}?format={fmt}");
        let Some(u) = g.iri(&url, "distribution URL") else {
            continue;
        };
        let d = BlankNode::default();
        g.add(ds.clone(), &p(DCAT, "distribution"), d.clone());
        g.typ(d.clone(), &p(DCAT, "Distribution"));
        g.lang(
            d.clone(),
            &p(DCT, "title"),
            &format!("{title} ({fmt})"),
            "en",
        );
        g.add(d.clone(), &p(DCAT, "accessURL"), u.clone());
        g.add(d.clone(), &p(DCAT, "downloadURL"), u);
        dcat_catalog::media(g, opts, &d, mime, filetype);
    }
}

fn count_triples(store: &TripleStore, dataset_iri: &str, version: &str) -> usize {
    if version.is_empty() {
        return 0;
    }
    let graph_prefix = format!("{dataset_iri}/version/{version}");
    let Ok(prefix) = NamedNode::new(graph_prefix.as_str()) else {
        return 0;
    };
    let q = format!(
        "SELECT (COUNT(*) AS ?n) WHERE {{ \
         GRAPH ?g {{ ?s ?p ?o }} \
         FILTER(STR(?g) = STR({prefix}) || STRSTARTS(STR(?g), CONCAT(STR({prefix}), \"/\"))) }}"
    );
    if let Ok(oxigraph::sparql::QueryResults::Solutions(sols)) = store.query(&q) {
        for row in sols.flatten() {
            if let Some(Some(oxigraph::model::Term::Literal(lit))) = row.values().first() {
                return lit.value().parse().unwrap_or(0);
            }
        }
    }
    0
}
