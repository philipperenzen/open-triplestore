//! Write-triggered validation pipelines (`trigger_on_write`) as a change-log
//! consumer (docs/notes/delta-versioning-design.md §8.3).
//!
//! The flag was stored and never read. A pipeline that sets it now runs when
//! its data changes: in full when it has no report yet (or cannot narrow the
//! run), else over only the nodes the change touched, merged into its last
//! report — which must then say exactly what a full run would.

mod common;

use common::*;
use open_triplestore::auth::models::{OwnerType, Visibility};
use open_triplestore::server::AppState;
use open_triplestore::shacl_studio::models::{
    ResultsTarget, SeverityThreshold, ShapeSource, ValidationPipeline, WriteTarget,
};
use open_triplestore::shacl_studio::store::ShaclStudioStore;
use open_triplestore::shacl_studio::trigger::{self, Tick};
use open_triplestore::store::changes::{DEFAULT_MAX_PAYLOAD, DEFAULT_MAX_SCAN};
use open_triplestore::store::TripleStore;
use oxigraph::io::RdfFormat;

const DATA: &str = "urn:trig:data";
const SHAPES: &str = "urn:trig:shapes";
const OTHER: &str = "urn:trig:other";

const SHAPES_TTL: &str = r#"
@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix ex: <http://e/> .
ex:PersonShape a sh:NodeShape ;
    sh:targetClass ex:Person ;
    sh:property [ sh:path ex:name ; sh:minCount 1 ] .
"#;

fn studio(state: &AppState) -> ShaclStudioStore {
    ShaclStudioStore::new(state.auth_db.pool())
}

fn setup() -> (AppState, ValidationPipeline) {
    let store = TripleStore::in_memory()
        .unwrap()
        .with_change_capture(DEFAULT_MAX_SCAN, DEFAULT_MAX_PAYLOAD);
    let (state, _token) = admin_state_with_store(store);
    state
        .store
        .load_str(SHAPES_TTL, RdfFormat::Turtle, Some(SHAPES))
        .unwrap();
    let shapes = studio(&state)
        .create_shape_graph(
            "people",
            None,
            OwnerType::User,
            "adm",
            Visibility::Private,
            SHAPES,
            &[],
            ShapeSource::Manual,
            Some("adm"),
        )
        .unwrap();
    let now = chrono::Utc::now().to_rfc3339();
    let p = ValidationPipeline {
        id: uuid::Uuid::new_v4().to_string(),
        name: "on write".into(),
        description: None,
        owner_type: OwnerType::User,
        owner_id: "adm".into(),
        visibility: Visibility::Private,
        targets: vec![],
        dataset_ids: vec![],
        graph_iris: vec![DATA.to_string()],
        target_classes: vec![],
        shape_graph_ids: vec![shapes.id],
        severity_threshold: SeverityThreshold::Violation,
        run_inference: false,
        max_results: None,
        inferred_target: WriteTarget::InPlace,
        inferred_target_graph: None,
        results_target: ResultsTarget::None,
        results_target_graph: None,
        trigger_on_write: true,
        schedule_cron: None,
        gate_writes: false,
        retention: 10,
        last_run_at: None,
        last_conforms: None,
        created_by: Some("adm".into()),
        created_at: now.clone(),
        updated_at: now,
    };
    studio(&state).insert_pipeline(&p).unwrap();
    (state, p)
}

fn tick(state: &AppState) -> Tick {
    trigger::tick(&state.store, &state.auth_db, &state.base_url)
}

/// The focus nodes of the pipeline's latest run, sorted.
fn latest_focus(state: &AppState, p: &ValidationPipeline) -> (String, Vec<String>) {
    let s = studio(state);
    let summary = s
        .list_pipeline_runs(&p.id, 1)
        .unwrap()
        .pop()
        .expect("a run");
    let run = s.get_pipeline_run(&summary.id).unwrap().unwrap();
    let mut focus: Vec<String> = run
        .report
        .unwrap()
        .results
        .into_iter()
        .map(|r| r.focus_node)
        .collect();
    focus.sort();
    (run.triggered_by, focus)
}

#[tokio::test]
async fn a_write_runs_the_pipeline_and_a_later_one_only_over_what_it_touched() {
    let (state, p) = setup();
    // The first tick starts from the log's head: no history is replayed.
    assert_eq!(tick(&state), Tick::default());

    state
        .store
        .load_str(
            "<http://e/alice> a <http://e/Person> .",
            RdfFormat::Turtle,
            Some(DATA),
        )
        .unwrap();
    // No report to merge into yet: a full run.
    assert_eq!(
        tick(&state),
        Tick {
            incremental: 0,
            full: 1
        }
    );
    let (by, focus) = latest_focus(&state, &p);
    assert_eq!(by, "write");
    assert_eq!(focus, vec!["http://e/alice"]);

    // Alice gets a name, Bob arrives without one: an incremental run, and
    // the merged report says what a full run would.
    state
        .store
        .update(&format!(
            "INSERT DATA {{ GRAPH <{DATA}> {{ <http://e/alice> <http://e/name> \"Alice\" . <http://e/bob> a <http://e/Person> }} }}"
        ))
        .unwrap();
    assert_eq!(
        tick(&state),
        Tick {
            incremental: 1,
            full: 0
        }
    );
    assert_eq!(latest_focus(&state, &p).1, vec!["http://e/bob"]);
    let full =
        open_triplestore::shacl::validate(&state.store, SHAPES, &[DATA.to_string()]).unwrap();
    assert_eq!(full.results.len(), 1, "the merge agrees with a full run");

    // Nothing of the pipeline's: no run, but the cursor moves on.
    state
        .store
        .load_str(
            "<http://e/x> <http://e/p> 1 .",
            RdfFormat::Turtle,
            Some(OTHER),
        )
        .unwrap();
    assert_eq!(tick(&state), Tick::default());
    assert_eq!(
        state.store.changes().cursor(trigger::CURSOR).unwrap().seq,
        state.store.changes().last_seq()
    );

    // A change to the shapes: every target may be affected, so a full run.
    state
        .store
        .update(&format!(
            "PREFIX sh: <http://www.w3.org/ns/shacl#> INSERT DATA {{ GRAPH <{SHAPES}> {{ <http://e/PersonShape> sh:property [ sh:path <http://e/age> ; sh:minCount 1 ] }} }}"
        ))
        .unwrap();
    assert_eq!(
        tick(&state),
        Tick {
            incremental: 0,
            full: 1
        }
    );
    assert_eq!(
        latest_focus(&state, &p).1,
        vec!["http://e/alice", "http://e/bob", "http://e/bob"],
        "both lack an age; bob a name too"
    );
}

#[tokio::test]
async fn nothing_runs_while_change_capture_is_off() {
    let (state, _token) = admin_state_with_store(
        TripleStore::in_memory()
            .unwrap()
            .with_change_capture_disabled(),
    );
    assert_eq!(tick(&state), Tick::default());
}
