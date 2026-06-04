use crate::auth::models::User;
use crate::store::engine::TripleStore;
use crate::store::escape_sparql_iri;
use oxigraph::io::RdfFormat;

/// Named graph IRI for a user's FOAF/VCARD profile.
pub fn user_profile_graph_iri(user_id: &str) -> String {
    format!("urn:system:user:{}", user_id)
}

/// Write (or overwrite) the FOAF/VCARD profile named graph for a user.
/// Silently ignores errors so that profile graph failures never abort the main operation.
pub fn write_user_profile_graph(store: &TripleStore, base_url: &str, user: &User) {
    let graph_iri = user_profile_graph_iri(&user.id);
    // Escape the username before it becomes part of an `<...>` IRI — usernames are
    // user-chosen and only length-validated, so a `>`/space/control char would
    // otherwise break out of the IRI and inject triples into the profile graph.
    let person_iri = format!("{}/users/{}", base_url, escape_sparql_iri(&user.username));
    let mut ttl = String::new();

    ttl.push_str("@prefix foaf:  <http://xmlns.com/foaf/0.1/> .\n");
    ttl.push_str("@prefix vcard: <http://www.w3.org/2006/vcard/ns#> .\n\n");

    ttl.push_str(&format!("<{}> a foaf:Person ;\n", person_iri));

    let display = user.display_name.as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or(&user.username);
    ttl.push_str(&format!("    foaf:name \"{}\" ;\n", escape_ttl_string(display)));
    ttl.push_str(&format!("    foaf:nick \"{}\" ;\n", escape_ttl_string(&user.username)));
    ttl.push_str(&format!("    foaf:depiction <{}/api/users/{}/avatar> ;\n", base_url, user.id));

    if user.is_public {
        // Only expose contact details when the profile is public
        ttl.push_str(&format!("    vcard:hasEmail <mailto:{}> ;\n", escape_sparql_iri(&user.email)));
        if let Some(bio) = user.bio.as_deref().filter(|s| !s.is_empty()) {
            ttl.push_str(&format!("    foaf:title \"{}\" ;\n", escape_ttl_string(bio)));
        }
        if let Some(website) = user.website.as_deref().filter(|s| !s.is_empty()) {
            ttl.push_str(&format!("    foaf:homepage <{}> ;\n", escape_sparql_iri(website)));
        }
        if let Some(phone) = user.phone.as_deref().filter(|s| !s.is_empty()) {
            ttl.push_str(&format!("    vcard:hasTelephone \"{}\" ;\n", escape_ttl_string(phone)));
        }
        if let Some(org) = user.organization.as_deref().filter(|s| !s.is_empty()) {
            ttl.push_str(&format!("    vcard:organization-name \"{}\" ;\n", escape_ttl_string(org)));
        }
    }

    ttl.push_str("    .\n");

    let _ = store.graph_store_put(Some(&graph_iri), &ttl, RdfFormat::Turtle);
}

fn escape_ttl_string(s: &str) -> String {
    s.replace('\\', "\\\\")
     .replace('"', "\\\"")
     .replace('\n', "\\n")
     .replace('\r', "\\r")
}
