//! Seed the built-in **SHACL-SHACL** meta-shapes: a system shape graph whose
//! Turtle (vendored in `shacl-shacl.ttl`) validates *shapes graphs treated as
//! data* — SHACL-of-SHACL. Runs idempotently at startup so every install can
//! meta-validate shape graphs with no user setup.
//!
//! The graph content is replaced on each boot (a cheap PUT) so it tracks the
//! vendored file across upgrades; the Library [`ShapeGraph`] row is created only
//! once — owner `system`, `Public` (world-readable, admin-only to manage).

use crate::auth::db::AuthDb;
use crate::auth::models::{OwnerType, Visibility};
use crate::store::TripleStore;

use super::models::ShapeSource;
use super::store::ShaclStudioStore;

/// System graph holding the built-in SHACL-SHACL meta-shapes. Public so the
/// validate endpoint and meta-validation pipelines can name it directly.
pub const SHACL_SHACL_GRAPH: &str = "urn:system:shapes:shacl-shacl";

/// The vendored meta-shapes (a focused, engine-evaluable SHACL-SHACL subset —
/// see the file header for why it is a subset of the full W3C shapes).
const SHACL_SHACL_TTL: &str = include_str!("shacl-shacl.ttl");

/// Nominal owner for built-in system artifacts. No real user has this id, so a
/// `Public` set stays world-readable but admin-only to manage (see `access`).
const SYSTEM_OWNER: &str = "system";

/// Idempotently load the meta-shapes graph and ensure its Library shape graph
/// exists. Returns the built-in shape graph's id.
pub fn seed_shacl_shacl(store: &TripleStore, auth_db: &AuthDb) -> anyhow::Result<String> {
    // Always (re)load the graph so it tracks the vendored file across upgrades.
    store
        .graph_store_put(
            Some(SHACL_SHACL_GRAPH),
            SHACL_SHACL_TTL,
            oxigraph::io::RdfFormat::Turtle,
        )
        .map_err(|e| anyhow::anyhow!("seed shacl-shacl graph: {e}"))?;

    let studio = ShaclStudioStore::new(auth_db.pool());
    if let Some(existing) = studio.get_shape_graph_by_iri(SHACL_SHACL_GRAPH)? {
        return Ok(existing.id); // already imported
    }

    let set = studio.create_shape_graph(
        "SHACL-SHACL (meta)",
        Some("Built-in meta-shapes for validating SHACL shape graphs (SHACL-of-SHACL)."),
        OwnerType::User,
        SYSTEM_OWNER,
        Visibility::Public,
        SHACL_SHACL_GRAPH,
        &["meta".to_string(), "builtin".to_string()],
        ShapeSource::Imported,
        None,
    )?;
    let (targets, count) = super::run::analyze_shapes_graph(store, SHACL_SHACL_GRAPH);
    studio.save_shape_graph_revision(
        &set.id,
        SHACL_SHACL_TTL,
        &targets,
        count,
        Some("Built-in"),
        None,
    )?;
    tracing::info!("shacl_studio: seeded built-in SHACL-SHACL meta shape graph");
    Ok(set.id)
}

/// System graph holding the built-in SKOS integrity shapes (S9, S13, S14,
/// S27, S36, S37 and S46 of the SKOS Reference). Public, like the
/// meta-shapes.
pub const SKOS_INTEGRITY_GRAPH: &str = "urn:system:shapes:skos-integrity";

/// The SKOS integrity shapes (see the file header).
pub const SKOS_INTEGRITY_TTL: &str = include_str!("skos-integrity.ttl");

/// Idempotently load the SKOS integrity shapes and ensure their Library
/// shape graph exists, so any validation pipeline or dataset validation can
/// select them as a profile. Returns the built-in shape graph's id.
pub fn seed_skos_integrity(store: &TripleStore, auth_db: &AuthDb) -> anyhow::Result<String> {
    store
        .graph_store_put(
            Some(SKOS_INTEGRITY_GRAPH),
            SKOS_INTEGRITY_TTL,
            oxigraph::io::RdfFormat::Turtle,
        )
        .map_err(|e| anyhow::anyhow!("seed SKOS integrity graph: {e}"))?;

    let studio = ShaclStudioStore::new(auth_db.pool());
    if let Some(existing) = studio.get_shape_graph_by_iri(SKOS_INTEGRITY_GRAPH)? {
        return Ok(existing.id);
    }
    let set = studio.create_shape_graph(
        "SKOS integrity",
        Some(
            "Built-in shapes for the integrity conditions of the SKOS Reference: S9 and \
             S37 (concept schemes, concepts and collections are disjoint), S13 (label \
             properties pairwise disjoint), S14 (one prefLabel per language tag), S27 \
             (related disjoint with broaderTransitive), S36 (memberList items are members) \
             and S46 (exactMatch disjoint with broadMatch and relatedMatch).",
        ),
        OwnerType::User,
        SYSTEM_OWNER,
        Visibility::Public,
        SKOS_INTEGRITY_GRAPH,
        &["skos".to_string(), "builtin".to_string()],
        ShapeSource::Imported,
        None,
    )?;
    let (targets, count) = super::run::analyze_shapes_graph(store, SKOS_INTEGRITY_GRAPH);
    studio.save_shape_graph_revision(
        &set.id,
        SKOS_INTEGRITY_TTL,
        &targets,
        count,
        Some("Built-in"),
        None,
    )?;
    tracing::info!("shacl_studio: seeded built-in SKOS integrity shape graph");
    Ok(set.id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shacl_studio::models::SeverityThreshold;

    fn validate_against_meta(
        store: &TripleStore,
        data_graph: &str,
    ) -> crate::shacl_studio::run::RunOutcome {
        super::super::run::run_validation(
            store,
            &[SHACL_SHACL_GRAPH.to_string()],
            &[data_graph.to_string()],
            SeverityThreshold::Violation,
            false,
        )
        .expect("meta validation runs")
    }

    /// The built-in meta-shapes must be valid *under their own rules* — otherwise
    /// every meta-validation would report spurious violations. This is the
    /// canonical SHACL-SHACL self-consistency check.
    #[test]
    fn shacl_shacl_self_validates() {
        let store = TripleStore::in_memory().unwrap();
        let auth = AuthDb::in_memory().unwrap();
        let id = seed_shacl_shacl(&store, &auth).unwrap();
        assert!(!id.is_empty());

        let outcome = validate_against_meta(&store, SHACL_SHACL_GRAPH);
        assert!(
            outcome.report.conforms,
            "SHACL-SHACL must conform to itself, got {} result(s): {:?}",
            outcome.report.results_count, outcome.report.results
        );
    }

    /// Seeding twice must not create a second built-in set (idempotent).
    #[test]
    fn seed_is_idempotent() {
        let store = TripleStore::in_memory().unwrap();
        let auth = AuthDb::in_memory().unwrap();
        let a = seed_shacl_shacl(&store, &auth).unwrap();
        let b = seed_shacl_shacl(&store, &auth).unwrap();
        assert_eq!(a, b, "re-seeding returns the same shape graph id");
    }

    /// And it must have teeth: a malformed node shape (bad sh:nodeKind via
    /// sh:in, non-boolean sh:closed via sh:datatype) is flagged.
    #[test]
    fn meta_validation_flags_bad_named_shape() {
        let store = TripleStore::in_memory().unwrap();
        let auth = AuthDb::in_memory().unwrap();
        seed_shacl_shacl(&store, &auth).unwrap();

        let bad = r#"
            @prefix sh: <http://www.w3.org/ns/shacl#> .
            @prefix ex: <http://example.org/> .
            ex:BadShape a sh:NodeShape ;
                sh:targetClass ex:Foo ;
                sh:nodeKind sh:Banana ;
                sh:closed "yes" .
        "#;
        store
            .graph_store_put(Some("urn:test:bad"), bad, oxigraph::io::RdfFormat::Turtle)
            .unwrap();

        let outcome = validate_against_meta(&store, "urn:test:bad");
        assert!(
            !outcome.report.conforms,
            "a malformed named shape must not conform"
        );
        assert!(
            outcome.violation_count >= 2,
            "expected ≥2 violations (bad sh:nodeKind + non-boolean sh:closed), got {}: {:?}",
            outcome.violation_count,
            outcome.report.results
        );
    }

    /// Inline (blank-node) property shapes — `sh:property [ … ]`, the dominant
    /// way real shapes are authored — are meta-validated too. The engine
    /// resolves a blank focus node's predicate values through the raw quad
    /// index, so `otsm:PropertyShape` (sh:targetObjectsOf sh:property) reaches
    /// the inline shape's attributes. Here the inline shape has a non-integer
    /// sh:minCount and a bogus sh:nodeKind, both of which must be flagged.
    #[test]
    fn inline_blank_property_shapes_are_meta_validated() {
        let store = TripleStore::in_memory().unwrap();
        let auth = AuthDb::in_memory().unwrap();
        seed_shacl_shacl(&store, &auth).unwrap();

        let bad_inline = r#"
            @prefix sh: <http://www.w3.org/ns/shacl#> .
            @prefix ex: <http://example.org/> .
            ex:Shape a sh:NodeShape ;
                sh:targetClass ex:Foo ;
                sh:property [ sh:path ex:p ; sh:minCount "lots" ; sh:nodeKind sh:Banana ] .
        "#;
        store
            .graph_store_put(
                Some("urn:test:inline"),
                bad_inline,
                oxigraph::io::RdfFormat::Turtle,
            )
            .unwrap();

        let outcome = validate_against_meta(&store, "urn:test:inline");
        assert!(
            !outcome.report.conforms,
            "an inline property shape with a bad sh:minCount/sh:nodeKind must not conform"
        );
        assert!(
            outcome.violation_count >= 2,
            "expected ≥2 violations (non-integer sh:minCount + bad sh:nodeKind), got {}: {:?}",
            outcome.violation_count,
            outcome.report.results
        );
    }

    // ── SKOS integrity shapes ────────────────────────────────────────────

    /// The focus nodes the SKOS integrity shapes report for `data` (Turtle
    /// with the `skos:` and `ex:` prefixes declared).
    fn skos_violations(data: &str) -> Vec<String> {
        let store = TripleStore::in_memory().unwrap();
        let auth = AuthDb::in_memory().unwrap();
        seed_skos_integrity(&store, &auth).unwrap();
        let ttl = format!(
            "@prefix skos: <http://www.w3.org/2004/02/skos/core#> .\n\
             @prefix ex: <http://example.org/> .\n{data}"
        );
        store
            .graph_store_put(Some("urn:test:skos"), &ttl, oxigraph::io::RdfFormat::Turtle)
            .unwrap();
        let outcome = super::super::run::run_validation(
            &store,
            &[SKOS_INTEGRITY_GRAPH.to_string()],
            &["urn:test:skos".to_string()],
            SeverityThreshold::Violation,
            false,
        )
        .expect("SKOS integrity validation runs");
        let mut focus: Vec<String> = outcome
            .report
            .results
            .iter()
            .map(|r| format!("{} {}", r.focus_node, r.message))
            .collect();
        focus.sort();
        focus
    }

    #[test]
    fn skos_integrity_shapes_are_valid_shacl() {
        let store = TripleStore::in_memory().unwrap();
        let auth = AuthDb::in_memory().unwrap();
        seed_shacl_shacl(&store, &auth).unwrap();
        let a = seed_skos_integrity(&store, &auth).unwrap();
        assert_eq!(a, seed_skos_integrity(&store, &auth).unwrap(), "idempotent");
        let outcome = validate_against_meta(&store, SKOS_INTEGRITY_GRAPH);
        assert!(outcome.report.conforms, "{:?}", outcome.report.results);
    }

    #[test]
    fn a_well_formed_scheme_meets_every_condition() {
        let v = skos_violations(
            r#"
            ex:animals a skos:ConceptScheme .
            ex:mammal a skos:Concept ; skos:inScheme ex:animals ;
                skos:prefLabel "mammal"@en , "zoogdier"@nl ; skos:altLabel "mammalian"@en .
            ex:whale a skos:Concept ; skos:broader ex:mammal ; skos:related ex:krill ;
                skos:prefLabel "whale"@en ; skos:hiddenLabel "wale"@en ;
                skos:exactMatch <http://example.com/Whale> ; skos:closeMatch <http://example.com/Cetacean> .
            ex:krill a skos:Concept ; skos:prefLabel "krill"@en .
            ex:big a skos:OrderedCollection ;
                skos:memberList ( ex:whale ex:mammal ) ; skos:member ex:whale , ex:mammal .
            "#,
        );
        assert!(v.is_empty(), "{v:#?}");
    }

    #[test]
    fn s13_labels_are_pairwise_disjoint() {
        let v = skos_violations(
            r#"
            ex:a skos:prefLabel "lion"@en ; skos:altLabel "lion"@en .
            ex:b skos:prefLabel "tiger"@en ; skos:hiddenLabel "tiger"@en .
            ex:c skos:altLabel "puma"@en ; skos:hiddenLabel "puma"@en .
            ex:ok skos:prefLabel "cat"@en ; skos:altLabel "cat"@nl .
            "#,
        );
        assert_eq!(v.len(), 3, "{v:#?}");
        for (s, f) in v.iter().zip(["ex:a", "ex:b", "ex:c"]) {
            let iri = f.replace("ex:", "http://example.org/");
            assert!(s.starts_with(&iri) && s.contains("S13"), "{s}");
        }
    }

    #[test]
    fn s14_one_preferred_label_per_language() {
        let v = skos_violations(
            r#"
            ex:a skos:prefLabel "car"@en , "automobile"@en .
            ex:b skos:prefLabel "bike" , "bicycle" .
            ex:ok skos:prefLabel "boat"@en , "boot"@nl , "ship" .
            "#,
        );
        assert_eq!(v.len(), 2, "{v:#?}");
        assert!(v[0].starts_with("http://example.org/a") && v[0].contains("S14"));
        assert!(v[1].starts_with("http://example.org/b") && v[1].contains("S14"));
    }

    #[test]
    fn s27_related_is_disjoint_with_broader_transitive() {
        let v = skos_violations(
            r#"
            ex:poodle skos:broader ex:dog . ex:dog skos:broader ex:mammal .
            ex:poodle skos:related ex:mammal .
            ex:cat skos:related ex:mouse .
            "#,
        );
        assert!(
            v.iter()
                .any(|s| s.starts_with("http://example.org/poodle") && s.contains("S27")),
            "{v:#?}"
        );
        assert!(
            v.iter().all(|s| !s.contains("http://example.org/cat ")),
            "{v:#?}"
        );
        // Through skos:narrower and the symmetric form of skos:related.
        let v = skos_violations(
            r#"
            ex:mammal skos:narrower ex:dog . ex:mammal skos:related ex:dog .
            "#,
        );
        assert!(v.iter().any(|s| s.contains("S27")), "{v:#?}");
    }

    #[test]
    fn s36_member_list_items_are_members() {
        let v = skos_violations(
            r#"
            ex:list skos:memberList ( ex:a ex:b ) ; skos:member ex:a .
            "#,
        );
        assert_eq!(v.len(), 1, "{v:#?}");
        assert!(
            v[0].starts_with("http://example.org/list") && v[0].contains("http://example.org/b")
        );
    }

    #[test]
    fn s9_and_s37_concepts_schemes_and_collections_are_disjoint() {
        let v = skos_violations(
            r#"
            ex:s a skos:ConceptScheme , skos:Concept .
            ex:c a skos:Collection , skos:ConceptScheme .
            ex:o a skos:OrderedCollection , skos:Concept .
            ex:ok a skos:Collection .
            "#,
        );
        for (f, cond) in [("s", "S9"), ("c", "S37"), ("o", "S37")] {
            assert!(
                v.iter().any(|s| s.starts_with(&format!("http://example.org/{f} "))
                    && s.contains(cond)),
                "{f}: {v:#?}"
            );
        }
        assert!(v.iter().all(|s| !s.contains("example.org/ok")), "{v:#?}");
    }

    #[test]
    fn s46_exact_match_is_disjoint_with_broad_and_related_match() {
        let v = skos_violations(
            r#"
            ex:a skos:exactMatch ex:x ; skos:broadMatch ex:x .
            ex:b skos:exactMatch ex:y . ex:y skos:relatedMatch ex:b .
            ex:c skos:exactMatch ex:z . ex:z skos:narrowMatch ex:c .
            ex:ok skos:exactMatch ex:w ; skos:closeMatch ex:w .
            "#,
        );
        for f in ["a", "b", "c"] {
            assert!(
                v.iter().any(
                    |s| s.starts_with(&format!("http://example.org/{f} ")) && s.contains("S46")
                ),
                "{f}: {v:#?}"
            );
        }
        assert!(v.iter().all(|s| !s.contains("example.org/ok")), "{v:#?}");
    }
}
