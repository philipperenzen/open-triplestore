use crate::auth::models::Organisation;
use crate::store::engine::TripleStore;
use crate::store::escape_sparql_iri;
use oxigraph::io::RdfFormat;

/// Named graph IRI holding an organisation's FOAF/ORG/vCard metadata.
pub fn org_metadata_graph_iri(org_id: &str) -> String {
    format!("urn:system:metadata:org:{}", org_id)
}

/// Canonical linked-data IRI for an organisation (matches the DCAT catalog).
pub fn org_iri(base_url: &str, org_id: &str) -> String {
    format!("{}/org/{}", base_url, org_id)
}

/// Write (or overwrite) the organisation's metadata named graph.
///
/// Emits the org node with FOAF/W3C-ORG typing plus its place in the
/// hierarchy: `org:subOrganizationOf <parent>` and a
/// `org:hasSubOrganization <child>` for every direct child. The result is a
/// self-contained, queryable "organisation knowledge graph" in the triplestore.
///
/// Best-effort: errors are swallowed so a metadata write never aborts the
/// surrounding CRUD operation.
pub fn write_org_metadata_graph(
    store: &TripleStore,
    base_url: &str,
    org: &Organisation,
    children: &[Organisation],
) {
    let graph_iri = org_metadata_graph_iri(&org.id);
    let subject = org_iri(base_url, &org.id);

    let mut ttl = String::new();
    ttl.push_str("@prefix foaf: <http://xmlns.com/foaf/0.1/> .\n");
    ttl.push_str("@prefix org:  <http://www.w3.org/ns/org#> .\n");
    ttl.push_str("@prefix dct:  <http://purl.org/dc/terms/> .\n");
    ttl.push_str("@prefix dcat: <http://www.w3.org/ns/dcat#> .\n");
    ttl.push_str("@prefix vcard: <http://www.w3.org/2006/vcard/ns#> .\n");
    ttl.push_str("@prefix xsd:  <http://www.w3.org/2001/XMLSchema#> .\n\n");

    // Predicate-object clauses, joined so Turtle stays valid regardless of which
    // optional fields are present.
    let mut clauses: Vec<String> = Vec::new();

    // rdf:type — always foaf:Organization, plus the specialised W3C ORG type.
    match org.org_type.as_deref().unwrap_or("FormalOrganization") {
        "OrganizationalUnit" => clauses.push("a foaf:Organization, org:OrganizationalUnit".into()),
        "Organization" => clauses.push("a foaf:Organization".into()),
        _ => clauses.push("a foaf:Organization, org:FormalOrganization".into()),
    }

    clauses.push(format!("foaf:name \"{}\"", escape_ttl(&org.name)));

    if let Some(desc) = org.description.as_deref().filter(|s| !s.is_empty()) {
        clauses.push(format!("dct:description \"{}\"", escape_ttl(desc)));
    }
    if let Some(hp) = org.homepage.as_deref().filter(|s| !s.is_empty()) {
        clauses.push(format!("foaf:homepage <{}>", escape_sparql_iri(hp)));
    }
    if let Some(ident) = org.identifier.as_deref().filter(|s| !s.is_empty()) {
        clauses.push(format!("dct:identifier \"{}\"", escape_ttl(ident)));
    }

    // Hierarchy
    if let Some(parent) = org.parent_org_id.as_deref().filter(|s| !s.is_empty()) {
        clauses.push(format!(
            "org:subOrganizationOf <{}>",
            org_iri(base_url, parent)
        ));
    }
    for child in children {
        clauses.push(format!(
            "org:hasSubOrganization <{}>",
            org_iri(base_url, &child.id)
        ));
    }

    clauses.push(format!("dct:modified \"{}\"^^xsd:dateTime", org.created_at));

    // Contact point as a blank node (only when at least one field is set).
    let has_contact = org
        .contact_name
        .as_deref()
        .filter(|s| !s.is_empty())
        .is_some()
        || org
            .contact_email
            .as_deref()
            .filter(|s| !s.is_empty())
            .is_some()
        || org
            .contact_url
            .as_deref()
            .filter(|s| !s.is_empty())
            .is_some();
    if has_contact {
        let mut cp: Vec<String> = vec!["a vcard:Organization".into()];
        if let Some(n) = org.contact_name.as_deref().filter(|s| !s.is_empty()) {
            cp.push(format!("vcard:fn \"{}\"", escape_ttl(n)));
        }
        if let Some(e) = org.contact_email.as_deref().filter(|s| !s.is_empty()) {
            cp.push(format!("vcard:hasEmail <mailto:{}>", escape_sparql_iri(e)));
        }
        if let Some(u) = org.contact_url.as_deref().filter(|s| !s.is_empty()) {
            cp.push(format!("vcard:hasURL <{}>", escape_sparql_iri(u)));
        }
        clauses.push(format!("dcat:contactPoint [ {} ]", cp.join(" ; ")));
    }

    ttl.push_str(&format!("<{}>\n    ", subject));
    ttl.push_str(&clauses.join(" ;\n    "));
    ttl.push_str(" .\n");

    let _ = store.graph_store_put(Some(&graph_iri), &ttl, RdfFormat::Turtle);
}

/// Remove an organisation's metadata named graph (on delete).
pub fn delete_org_metadata_graph(store: &TripleStore, org_id: &str) {
    let _ = store.graph_store_delete(Some(&org_metadata_graph_iri(org_id)));
}

fn escape_ttl(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}
