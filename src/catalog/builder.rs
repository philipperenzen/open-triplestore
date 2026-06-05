//! Build a DCAT 2 catalog of published data-models and vocabularies as Turtle.

use std::fmt::Write;

use std::sync::Arc;

use crate::auth::db::AuthDb;
use crate::data_models::registry as dm_registry;
use crate::kind_detector::RegistryKind;
use crate::store::TripleStore;

const FORMATS: &[(&str, &str, &str)] = &[
    ("turtle", "text/turtle", "ttl"),
    ("jsonld", "application/ld+json", "jsonld"),
    ("ntriples", "application/n-triples", "nt"),
    ("rdfxml", "application/rdf+xml", "rdf"),
];

fn esc_literal(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

/// Build the catalog as Turtle.
pub fn build_catalog_turtle(
    store: &TripleStore,
    base_url: &str,
    auth_db: &Arc<AuthDb>,
    user_id: Option<&str>,
) -> String {
    let mut out = String::with_capacity(4096);

    writeln!(out, "@prefix dcat: <http://www.w3.org/ns/dcat#> .").unwrap();
    writeln!(out, "@prefix dct: <http://purl.org/dc/terms/> .").unwrap();
    writeln!(out, "@prefix void: <http://rdfs.org/ns/void#> .").unwrap();
    writeln!(out, "@prefix foaf: <http://xmlns.com/foaf/0.1/> .").unwrap();
    writeln!(out, "@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .").unwrap();
    writeln!(out).unwrap();

    let catalog_iri = format!("{base_url}/catalog/registry");
    writeln!(out, "<{catalog_iri}> a dcat:Catalog ;").unwrap();
    writeln!(out, "    dct:title \"Linked Data Registry Catalog\" ;").unwrap();
    writeln!(
        out,
        "    dct:description \"Published data-models and controlled vocabularies.\" ;"
    )
    .unwrap();
    writeln!(out, "    foaf:homepage <{base_url}/> ").unwrap();

    let data_models: Vec<_> = dm_registry::list_data_models(store)
        .into_iter()
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
    let dataset_iris: Vec<String> = data_models
        .iter()
        .filter(|d| d.latest_published.is_some())
        .map(|d| format!("{base_url}/data-model/{}", d.id))
        .collect();

    if dataset_iris.is_empty() {
        writeln!(out, "    .").unwrap();
    } else {
        writeln!(out, ";").unwrap();
        let last = dataset_iris.len() - 1;
        for (i, iri) in dataset_iris.iter().enumerate() {
            let sep = if i == last { "." } else { "," };
            writeln!(out, "    dcat:dataset <{iri}> {sep}").unwrap();
        }
    }
    writeln!(out).unwrap();

    // ── Data models ──────────────────────────────────────────────────────
    for d in data_models.iter().filter(|d| d.latest_published.is_some()) {
        let dataset_iri = format!("{base_url}/data-model/{}", d.id);
        let version = d.latest_published.as_deref().unwrap_or("");
        let triple_count = count_triples(store, &dataset_iri, version);

        writeln!(out, "<{dataset_iri}> a dcat:Dataset, void:Dataset ;").unwrap();
        writeln!(out, "    dct:identifier \"{}\" ;", esc_literal(&d.id)).unwrap();
        writeln!(out, "    dct:title \"{}\"@en ;", esc_literal(&d.title)).unwrap();
        if let Some(desc) = d.description.as_deref().filter(|s| !s.is_empty()) {
            writeln!(out, "    dct:description \"{}\"@en ;", esc_literal(desc)).unwrap();
        }
        // An ontology is an ADMS Ontology asset; a SKOS vocabulary is a CodeList.
        let asset_type = if d.kind == RegistryKind::Vocabulary {
            "http://purl.org/adms/assettype/CodeList"
        } else {
            "http://purl.org/adms/assettype/Ontology"
        };
        writeln!(out, "    dct:type <{asset_type}> ;").unwrap();
        writeln!(out, "    foaf:page <{}> ;", d.namespace).unwrap();
        writeln!(
            out,
            "    dcat:landingPage <{base_url}/data-model/{}> ;",
            d.id
        )
        .unwrap();
        writeln!(out, "    dcat:hasVersion \"{}\" ;", esc_literal(version)).unwrap();
        if !d.created_at.is_empty() {
            writeln!(
                out,
                "    dct:issued \"{}\"^^xsd:dateTime ;",
                esc_literal(&d.created_at)
            )
            .unwrap();
        }
        if triple_count > 0 {
            writeln!(out, "    void:triples {} ;", triple_count).unwrap();
        }

        write_distributions(
            &mut out,
            &format!("{base_url}/api/models/{}/latest/data", d.id),
            &d.title,
        );
    }

    out
}

fn write_distributions(out: &mut String, base_url: &str, title: &str) {
    let last = FORMATS.len() - 1;
    for (i, (fmt, mime, _)) in FORMATS.iter().enumerate() {
        let sep = if i == last { "." } else { ";" };
        writeln!(
            out,
            "    dcat:distribution [ a dcat:Distribution ; \
             dcat:accessURL <{base_url}?format={fmt}> ; \
             dcat:mediaType \"{mime}\" ; \
             dct:title \"{} ({fmt})\" ] {sep}",
            esc_literal(title)
        )
        .unwrap();
    }
    writeln!(out).unwrap();
}

fn count_triples(store: &TripleStore, dataset_iri: &str, version: &str) -> usize {
    if version.is_empty() {
        return 0;
    }
    let graph_prefix = format!("{dataset_iri}/version/{version}");
    let q = format!(
        "SELECT (COUNT(*) AS ?n) WHERE {{ \
         GRAPH ?g {{ ?s ?p ?o }} \
         FILTER(STRSTARTS(STR(?g), \"{graph_prefix}\")) }}"
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
