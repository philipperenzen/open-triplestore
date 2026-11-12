use crate::auth::models::User;
use crate::store::changes::Compaction;
use crate::store::engine::TripleStore;
use crate::store::escape_sparql_iri;
use oxigraph::io::RdfFormat;

/// What every user profile graph's IRI starts with. The change log keys on
/// it: a row of such a graph never carries the profile's values
/// (`crate::store::changes::is_payload_withheld_graph`).
pub const PROFILE_GRAPH_PREFIX: &str = "urn:system:user:";

/// Named graph IRI for a user's FOAF/VCARD profile.
pub fn user_profile_graph_iri(user_id: &str) -> String {
    format!("{PROFILE_GRAPH_PREFIX}{user_id}")
}

/// Remove the profile values the change log still holds: rows recorded
/// before profile graphs were recorded without a payload, and rows the log
/// of a restored backup brought back. Run once at start, after the store and
/// the identity database are open (a restore always ends in a start).
///
/// A profile graph an administrator registered to a dataset is left alone:
/// that dataset's delta versions are replayed from those rows, and the
/// registration put the profile under the dataset's versioning on purpose.
/// When the registrations cannot be read nothing is removed, and the next
/// start tries again.
pub fn withhold_recorded_profile_payloads(store: &TripleStore, auth_db: &crate::auth::db::AuthDb) {
    let keep: Vec<String> = match auth_db.list_all_dataset_graph_rows() {
        Ok(rows) => rows
            .into_iter()
            .map(|(_, graph, _)| graph)
            .filter(|g| g.starts_with(PROFILE_GRAPH_PREFIX))
            .collect(),
        Err(e) => {
            tracing::warn!("change log: profile values not removed, dataset graphs unread: {e}");
            return;
        }
    };
    let did = match store.changes().withhold_recorded_payloads(&keep) {
        Ok(did) => did,
        Err(e) => {
            tracing::warn!("change log: profile values not removed: {e}");
            return;
        }
    };
    if did.rows > 0 {
        tracing::info!(
            "change log: removed the recorded values of {} user profile graph row(s)",
            did.rows
        );
    }
    match did.compaction {
        Compaction::NotNeeded => {}
        Compaction::Done => tracing::info!("change log: compacted after the removal"),
        Compaction::TooLarge(bytes) => tracing::warn!(
            "change log: {} MiB is too large to compact at start, so its unused space may \
             still hold copies of the removed profile values. To clear them, stop the server \
             and run VACUUM on changes/changes.db in the data directory",
            bytes / (1024 * 1024)
        ),
        Compaction::Failed(e) => tracing::warn!(
            "change log: not compacted after the removal ({e}); the next start tries again"
        ),
    }
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

    let display = user
        .display_name
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or(&user.username);
    ttl.push_str(&format!(
        "    foaf:name \"{}\" ;\n",
        escape_ttl_string(display)
    ));
    ttl.push_str(&format!(
        "    foaf:nick \"{}\" ;\n",
        escape_ttl_string(&user.username)
    ));
    ttl.push_str(&format!(
        "    foaf:depiction <{}/api/users/{}/avatar> ;\n",
        base_url, user.id
    ));

    if user.is_public {
        // Only expose contact details when the profile is public
        ttl.push_str(&format!(
            "    vcard:hasEmail <mailto:{}> ;\n",
            escape_sparql_iri(&user.email)
        ));
        if let Some(bio) = user.bio.as_deref().filter(|s| !s.is_empty()) {
            ttl.push_str(&format!(
                "    foaf:title \"{}\" ;\n",
                escape_ttl_string(bio)
            ));
        }
        if let Some(website) = user.website.as_deref().filter(|s| !s.is_empty()) {
            ttl.push_str(&format!(
                "    foaf:homepage <{}> ;\n",
                escape_sparql_iri(website)
            ));
        }
        if let Some(phone) = user.phone.as_deref().filter(|s| !s.is_empty()) {
            ttl.push_str(&format!(
                "    vcard:hasTelephone \"{}\" ;\n",
                escape_ttl_string(phone)
            ));
        }
        if let Some(org) = user.organization.as_deref().filter(|s| !s.is_empty()) {
            ttl.push_str(&format!(
                "    vcard:organization-name \"{}\" ;\n",
                escape_ttl_string(org)
            ));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::db::AuthDb;
    use crate::auth::models::{OwnerType, SystemRole, Visibility};
    use crate::store::changes::{parse_nquads, Extent, DEFAULT_MAX_PAYLOAD, DEFAULT_MAX_SCAN};

    /// The values an earlier version recorded go at start, except those of a
    /// profile graph an administrator put under a dataset's versioning.
    #[test]
    fn recorded_profile_values_go_unless_a_dataset_versions_the_graph() {
        let store = TripleStore::in_memory()
            .unwrap()
            .with_change_capture(DEFAULT_MAX_SCAN, DEFAULT_MAX_PAYLOAD);
        let db = AuthDb::in_memory().unwrap();
        db.create_user(
            "adm",
            "admin",
            "admin@test.com",
            "hash",
            SystemRole::SuperAdmin,
        )
        .unwrap();
        db.create_dataset(
            "ds",
            "people",
            None,
            OwnerType::User,
            "adm",
            Visibility::Private,
            None,
        )
        .unwrap();
        let purged = user_profile_graph_iri("purged");
        let versioned = user_profile_graph_iri("versioned");
        db.add_dataset_graph("ds", &versioned).unwrap();
        let name = "<http://ex/u> <http://xmlns.com/foaf/0.1/name> \"Ada Example\"";
        for (seq, g) in [(1, &purged), (2, &versioned)] {
            store
                .graph_store_put(Some(g), &format!("{name} ."), RdfFormat::Turtle)
                .unwrap();
            // The row as a version before the rule recorded it.
            store
                .changes()
                .plant_payload(seq, &parse_nquads(&format!("{name} <{g}> .")), &[]);
        }

        withhold_recorded_profile_payloads(&store, &db);

        let rows = store.changes().rows_after(0, 10);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].graph_iri.as_deref(), Some(purged.as_str()));
        assert_eq!(rows[0].extent, Extent::Counts);
        assert!(rows[0].added.is_none() && rows[0].removed.is_none());
        assert_eq!(rows[0].added_n, 1);
        assert_eq!(rows[1].graph_iri.as_deref(), Some(versioned.as_str()));
        assert_eq!(rows[1].extent, Extent::Full);
        assert!(rows[1].added.as_deref().unwrap().contains("Ada Example"));
    }
}
