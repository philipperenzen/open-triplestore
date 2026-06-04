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
}
