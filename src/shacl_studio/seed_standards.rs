//! Seed built-in **per-standard SHACL shape graphs + validation pipelines** so a
//! fresh instance ships with a runnable demonstration of SHACL validation across
//! every supported standard, wired to the bundled "Open Triplestore" demo data.
//!
//! Each entry produces:
//! * a system-owned, `Public` Library [`ShapeGraph`] at `urn:system:shapes:std-{key}`
//!   (re-loaded every boot so it tracks this file across upgrades), and
//! * a deterministic [`ValidationPipeline`] (`urn:system:pipeline:std-{key}`)
//!   scoped to that standard's demo graph — inserted once, never overwritten, so
//!   user edits survive re-seeding.
//!
//! Seeding only *creates* the shapes + pipelines; it never *runs* them, so the
//! shapes only need to be valid Turtle here. Protocol/auth-only standards
//! (SPARQL Protocol, Graph Store, Service Description, SHACL-C, JWT/OAuth/SAML)
//! carry no demo data graph of their own; they are validated through the
//! capabilities registry pipeline (every `ots:Standard` must declare title +
//! conformance).

use crate::auth::db::AuthDb;
use crate::auth::models::{OwnerType, Visibility};
use crate::saved_queries::seed_data::DEMO_BASE;
use crate::store::TripleStore;

use super::models::*;
use super::store::ShaclStudioStore;

const SYSTEM_OWNER: &str = "system";

/// Deterministic id for a built-in system pipeline, so re-seeding is idempotent.
pub fn system_pipeline_id(key: &str) -> String {
    format!("urn:system:pipeline:{key}")
}

/// One standard's seed: a stable key, a display name, the shapes Turtle, the
/// demo graph the pipeline validates, and whether inference is meaningful.
struct StdEntry {
    key: &'static str,
    name: &'static str,
    /// Demo graph path under `DEMO_BASE` (e.g. `validation/shacl-data`).
    demo_path: &'static str,
    ttl: &'static str,
    run_inference: bool,
}

/// The shapes graph IRI for a standard key.
fn shapes_graph_iri(key: &str) -> String {
    format!("urn:system:shapes:std-{key}")
}

fn demo_graph(path: &str) -> String {
    format!("{DEMO_BASE}/{path}")
}

/// Idempotently seed every standard's shape graph + pipeline. Returns the number
/// of pipelines that exist afterwards (created or pre-existing).
pub fn seed_standards(store: &TripleStore, auth_db: &AuthDb) -> anyhow::Result<usize> {
    let studio = ShaclStudioStore::new(auth_db.pool());
    let now = chrono::Utc::now().to_rfc3339();
    let mut count = 0usize;

    for e in ENTRIES {
        let graph_iri = shapes_graph_iri(e.key);
        // Always (re)load the shapes graph so it tracks this file across upgrades.
        if let Err(err) =
            store.graph_store_put(Some(&graph_iri), e.ttl, oxigraph::io::RdfFormat::Turtle)
        {
            tracing::warn!("seed_standards: load shapes for {} failed: {err}", e.key);
            continue;
        }

        // Ensure the Library shape-graph row exists (created once).
        let set_id = match studio.get_shape_graph_by_iri(&graph_iri)? {
            Some(s) => s.id,
            None => {
                let set_name = format!("{} (standard)", e.name);
                let set_desc = format!(
                    "Built-in SHACL shapes demonstrating the {} standard against the bundled demo data.",
                    e.name
                );
                let set = studio.create_shape_graph(
                    &set_name,
                    Some(set_desc.as_str()),
                    OwnerType::User,
                    SYSTEM_OWNER,
                    Visibility::Public,
                    &graph_iri,
                    &["standard".to_string(), "builtin".to_string()],
                    ShapeSource::Imported,
                    None,
                )?;
                let (targets, n) = super::run::analyze_shapes_graph(store, &graph_iri);
                studio.save_shape_graph_revision(&set.id, e.ttl, &targets, n, Some("Built-in"), None)?;
                set.id
            }
        };

        // Insert the pipeline once (deterministic id; never clobber user edits).
        let pid = system_pipeline_id(&format!("std-{}", e.key));
        if studio.get_pipeline(&pid)?.is_none() {
            let pipeline = ValidationPipeline {
                id: pid,
                name: format!("Standard: {}", e.name),
                description: Some(format!(
                    "Validates the bundled {} demo graph against the built-in {} shapes.",
                    e.name, e.name
                )),
                owner_type: OwnerType::User,
                owner_id: SYSTEM_OWNER.to_string(),
                visibility: Visibility::Public,
                targets: vec![ValidationTarget {
                    kind: TargetKind::Graph,
                    id: demo_graph(e.demo_path),
                }],
                dataset_ids: vec![],
                graph_iris: vec![],
                target_classes: vec![],
                shape_graph_ids: vec![set_id],
                severity_threshold: SeverityThreshold::Violation,
                run_inference: e.run_inference,
                max_results: None,
                inferred_target: WriteTarget::InPlace,
                inferred_target_graph: None,
                results_target: ResultsTarget::None,
                results_target_graph: None,
                trigger_on_write: false,
                schedule_cron: None,
                gate_writes: false,
                retention: 50,
                last_run_at: None,
                last_conforms: None,
                created_by: Some(SYSTEM_OWNER.to_string()),
                created_at: now.clone(),
                updated_at: now.clone(),
            };
            studio.insert_pipeline(&pipeline)?;
        }
        count += 1;
    }

    tracing::info!("shacl_studio: seeded {} built-in standard shape graphs + pipelines", count);
    Ok(count)
}

// ─── The standard shape definitions (valid SHACL Core / SHACL-AF Turtle) ───────

const ENTRIES: &[StdEntry] = &[
    StdEntry {
        key: "rdf11",
        name: "RDF 1.1",
        demo_path: "core-rdf-sparql/rdf11",
        run_inference: false,
        ttl: r#"@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
# Every typed resource keeps at least its rdf:type (RDF 1.1 well-formedness).
[] a sh:NodeShape ;
   sh:targetSubjectsOf rdf:type ;
   sh:property [ sh:path rdf:type ; sh:minCount 1 ;
                 sh:message "Every described resource should declare an rdf:type." ] .
"#,
    },
    StdEntry {
        key: "rdf12",
        name: "RDF-star / RDF 1.2",
        demo_path: "core-rdf-sparql/rdf-star",
        run_inference: false,
        ttl: r#"@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
# RDF-star annotations attach metadata to quoted triples; assert the annotated
# subjects remain well-typed.
[] a sh:NodeShape ;
   sh:targetSubjectsOf rdf:type ;
   sh:property [ sh:path rdf:type ; sh:minCount 1 ] .
"#,
    },
    StdEntry {
        key: "rdfs",
        name: "RDFS",
        demo_path: "reasoning/rdfs",
        run_inference: false,
        ttl: r#"@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
# Subclass and property declarations should be labelled (RDFS schema hygiene).
[] a sh:NodeShape ;
   sh:targetSubjectsOf rdfs:subClassOf ;
   sh:property [ sh:path rdfs:subClassOf ; sh:minCount 1 ; sh:nodeKind sh:IRI ] .
"#,
    },
    StdEntry {
        key: "owlql",
        name: "OWL 2 QL",
        demo_path: "reasoning/owl-ql",
        run_inference: false,
        ttl: r#"@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
# Object properties should declare a domain and range (QL-friendly modelling).
[] a sh:NodeShape ;
   sh:targetClass owl:ObjectProperty ;
   sh:property [ sh:path rdfs:domain ; sh:nodeKind sh:IRI ] ;
   sh:property [ sh:path rdfs:range ; sh:nodeKind sh:IRI ] .
"#,
    },
    StdEntry {
        key: "owlel",
        name: "OWL 2 EL",
        demo_path: "reasoning/owl-el",
        run_inference: false,
        ttl: r#"@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix owl: <http://www.w3.org/2002/07/owl#> .
# EL classes are IRIs (no anonymous top-level classes in the demo).
[] a sh:NodeShape ;
   sh:targetClass owl:Class ;
   sh:nodeKind sh:IRI ;
   sh:message "OWL 2 EL classes should be named (IRI) resources." .
"#,
    },
    StdEntry {
        key: "owlrl",
        name: "OWL 2 RL",
        demo_path: "reasoning/owl-rl",
        run_inference: false,
        ttl: r#"@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix owl: <http://www.w3.org/2002/07/owl#> .
# Symmetric/transitive property axioms reference IRI properties.
[] a sh:NodeShape ;
   sh:targetClass owl:SymmetricProperty ;
   sh:nodeKind sh:IRI .
"#,
    },
    StdEntry {
        key: "owldl",
        name: "OWL 2 DL",
        demo_path: "reasoning/owl-dl",
        run_inference: false,
        ttl: r#"@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix owl: <http://www.w3.org/2002/07/owl#> .
# Disjointness/cardinality axioms target named classes.
[] a sh:NodeShape ;
   sh:targetClass owl:Class ;
   sh:nodeKind sh:IRI .
"#,
    },
    StdEntry {
        key: "geosparql",
        name: "GeoSPARQL 1.1",
        demo_path: "spatial/geo",
        run_inference: false,
        ttl: r#"@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix geo: <http://www.opengis.net/ont/geosparql#> .
# Every feature with a geometry must actually carry one.
[] a sh:NodeShape ;
   sh:targetSubjectsOf geo:hasGeometry ;
   sh:property [ sh:path geo:hasGeometry ; sh:minCount 1 ;
                 sh:message "A spatial feature must have at least one geometry." ] .
"#,
    },
    StdEntry {
        key: "shaclcore",
        name: "SHACL Core",
        demo_path: "validation/shacl-data",
        run_inference: false,
        ttl: r#"@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix ex: <https://opentriplestore.org/demo/validation#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
# Core constraints: a Person needs a string name and at most one integer age.
ex:CorePersonShape a sh:NodeShape ;
   sh:targetClass ex:Person ;
   sh:property [ sh:path ex:name ; sh:minCount 1 ; sh:datatype xsd:string ;
                 sh:message "Every person needs a name." ] ;
   sh:property [ sh:path ex:age ; sh:maxCount 1 ; sh:datatype xsd:integer ] .
"#,
    },
    StdEntry {
        key: "shacladv",
        name: "SHACL Advanced (SHACL-AF)",
        demo_path: "validation/shacl-data",
        run_inference: false,
        ttl: r#"@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix ex: <https://opentriplestore.org/demo/validation#> .
# Advanced: a SPARQL-based constraint expressing an adulthood business rule.
ex:AdultShape a sh:NodeShape ;
   sh:targetClass ex:Person ;
   sh:sparql [
       a sh:SPARQLConstraint ;
       sh:message "Person must be at least 18." ;
       sh:select "SELECT $this ?value WHERE { $this <https://opentriplestore.org/demo/validation#age> ?value . FILTER(?value < 18) }" ;
   ] .
"#,
    },
    StdEntry {
        key: "shex",
        name: "ShEx",
        demo_path: "validation/shex",
        run_inference: false,
        ttl: r#"@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix shex: <http://www.w3.org/ns/shex#> .
# The ShEx schema, stored as RDF, should declare its shapes.
[] a sh:NodeShape ;
   sh:targetClass shex:Schema ;
   sh:property [ sh:path shex:shapes ; sh:minCount 1 ;
                 sh:message "A ShEx schema should declare at least one shape." ] .
"#,
    },
    StdEntry {
        key: "swrl",
        name: "SWRL",
        demo_path: "rules/swrl",
        run_inference: false,
        ttl: r#"@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix swrl: <http://www.w3.org/2003/11/swrl#> .
# SWRL rules (swrl:Imp) are named, well-formed resources.
[] a sh:NodeShape ;
   sh:targetClass swrl:Imp ;
   sh:nodeKind sh:IRI .
"#,
    },
    StdEntry {
        key: "ldp",
        name: "Linked Data Platform 1.0",
        demo_path: "linked-data/ldp",
        run_inference: false,
        ttl: r#"@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix ldp: <http://www.w3.org/ns/ldp#> .
# A BasicContainer should contain at least one member.
[] a sh:NodeShape ;
   sh:targetClass ldp:BasicContainer ;
   sh:property [ sh:path ldp:contains ; sh:minCount 1 ;
                 sh:message "An LDP container should contain at least one resource." ] .
"#,
    },
    StdEntry {
        key: "dcat",
        name: "DCAT 2 / VoID",
        demo_path: "linked-data/dcat",
        run_inference: false,
        ttl: r#"@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix dcat: <http://www.w3.org/ns/dcat#> .
@prefix dct: <http://purl.org/dc/terms/> .
# Catalogued datasets must be titled (DCAT discovery metadata).
[] a sh:NodeShape ;
   sh:targetClass dcat:Dataset ;
   sh:property [ sh:path dct:title ; sh:minCount 1 ;
                 sh:message "A dcat:Dataset must have a dct:title." ] .
"#,
    },
    StdEntry {
        key: "capabilities",
        name: "Capabilities registry",
        demo_path: "capabilities/capabilities",
        run_inference: false,
        ttl: r#"@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix ots: <https://opentriplestore.org/ns#> .
@prefix dct: <http://purl.org/dc/terms/> .
# Every advertised standard must declare a title and a conformance level. This
# pipeline also covers the protocol/auth-only standards (SPARQL Protocol, Graph
# Store, Service Description, SHACL-C, JWT/OAuth/SAML) recorded in the registry.
ots:StandardShape a sh:NodeShape ;
   sh:targetClass ots:Standard ;
   sh:property [ sh:path dct:title ; sh:minCount 1 ;
                 sh:message "Every standard must have a title." ] ;
   sh:property [ sh:path ots:conformance ; sh:minCount 1 ;
                 sh:message "Every standard must declare a conformance level." ] .
"#,
    },
    StdEntry {
        key: "ots-model",
        name: "Open Triplestore data model",
        demo_path: "ots-ontology/ots-model",
        run_inference: false,
        ttl: r#"@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix otso: <https://opentriplestore.org/ontology/> .
# Every graph-role individual must be labelled (model hygiene for the ots terms).
[] a sh:NodeShape ;
   sh:targetClass otso:GraphRole ;
   sh:property [ sh:path rdfs:label ; sh:minCount 1 ;
                 sh:message "Every graph role must have an rdfs:label." ] .
"#,
    },
    StdEntry {
        key: "ots-vocab",
        name: "Open Triplestore vocabulary",
        demo_path: "ots-ontology/ots-vocabulary",
        run_inference: false,
        ttl: r#"@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix skos: <http://www.w3.org/2004/02/skos/core#> .
# Every concept must carry a preferred label and belong to the scheme.
[] a sh:NodeShape ;
   sh:targetClass skos:Concept ;
   sh:property [ sh:path skos:prefLabel ; sh:minCount 1 ;
                 sh:message "Every concept needs a skos:prefLabel." ] ;
   sh:property [ sh:path skos:inScheme ; sh:minCount 1 ;
                 sh:message "Every concept must be in a concept scheme." ] .
"#,
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    /// Seeding must be idempotent: a second pass creates no duplicate pipelines.
    #[test]
    fn seed_is_idempotent() {
        let store = TripleStore::in_memory().unwrap();
        let auth = AuthDb::in_memory().unwrap();
        let a = seed_standards(&store, &auth).unwrap();
        let b = seed_standards(&store, &auth).unwrap();
        assert_eq!(a, b);
        assert_eq!(a, ENTRIES.len());
        let studio = ShaclStudioStore::new(auth.pool());
        // Exactly one pipeline per entry.
        for e in ENTRIES {
            let pid = system_pipeline_id(&format!("std-{}", e.key));
            assert!(studio.get_pipeline(&pid).unwrap().is_some(), "missing pipeline for {}", e.key);
        }
    }

    /// Every built-in standard shape graph must be valid SHACL — i.e. it must
    /// conform to the SHACL-SHACL meta-shapes (no malformed shapes shipped).
    #[test]
    fn all_standard_shapes_are_valid_shacl() {
        let store = TripleStore::in_memory().unwrap();
        let auth = AuthDb::in_memory().unwrap();
        super::super::seed::seed_shacl_shacl(&store, &auth).unwrap();
        seed_standards(&store, &auth).unwrap();
        for e in ENTRIES {
            let g = shapes_graph_iri(e.key);
            let outcome = super::super::run::run_validation(
                &store,
                &[super::super::seed::SHACL_SHACL_GRAPH.to_string()],
                &[g.clone()],
                SeverityThreshold::Violation,
                false,
            )
            .expect("meta-validation runs");
            assert!(
                outcome.report.conforms,
                "standard shape {} is not valid SHACL: {:?}",
                e.key, outcome.report.results
            );
        }
    }
}
