//! End-to-end tests for Spark's question orientation and store-verified
//! invented-IRI check, driving the real chat endpoint against a scripted mock
//! LLM gateway.
//!
//! The gateway is a tiny local HTTP server speaking the OpenAI
//! `/v1/chat/completions` shape: each request pops the next scripted reply and
//! records the request body, so a test can both steer the retrieval loop
//! (reply with `SPARQL:` directives) and inspect the exact system prompt the
//! server built. Tests serialise on one lock — the gateway, its script queue,
//! and the `LLM_GATEWAY_URL` environment variable are process-wide.

mod common;

use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};

use axum::body::Body;
use axum::extract::State;
use axum::http::{header, Method, Request};
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{json, Value};
use tower::ServiceExt as _;

use open_triplestore::auth::models::{OwnerType, Visibility};
use open_triplestore::data_models::models::{DataModelVersion, VersionStatus};
use open_triplestore::data_models::registry;
use open_triplestore::kind_detector::RegistryKind;
use open_triplestore::server::AppState;

// ─── Scripted mock gateway ─────────────────────────────────────────────────────

struct Gateway {
    scripts: Mutex<VecDeque<String>>,
    prompts: Mutex<Vec<Value>>,
}

async fn completions(State(gw): State<&'static Gateway>, Json(body): Json<Value>) -> Json<Value> {
    gw.prompts.lock().unwrap().push(body);
    let reply = gw
        .scripts
        .lock()
        .unwrap()
        .pop_front()
        .unwrap_or_else(|| "I have nothing further to add.".to_string());
    Json(json!({"choices": [{"message": {"content": reply}}]}))
}

/// Start the gateway once for the whole test binary (on its own runtime thread,
/// so it outlives every per-test tokio runtime) and point `LLM_GATEWAY_URL` at
/// it before any request is made.
fn gateway() -> &'static Gateway {
    static GW: OnceLock<&'static Gateway> = OnceLock::new();
    GW.get_or_init(|| {
        let gw: &'static Gateway = Box::leak(Box::new(Gateway {
            scripts: Mutex::new(VecDeque::new()),
            prompts: Mutex::new(Vec::new()),
        }));
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async move {
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                tx.send(listener.local_addr().unwrap()).unwrap();
                let app = Router::new()
                    .route("/v1/chat/completions", post(completions))
                    .with_state(gw);
                axum::serve(listener, app).await.unwrap();
            });
        });
        let addr = rx.recv().unwrap();
        std::env::set_var("LLM_GATEWAY_URL", format!("http://{addr}"));
        gw
    })
}

/// One lock serialises the tests: they share the gateway's script queue and
/// the process environment.
fn test_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|p| p.into_inner())
}

fn script(gw: &Gateway, replies: &[&str]) {
    let mut s = gw.scripts.lock().unwrap();
    s.clear();
    *s = replies.iter().map(|r| r.to_string()).collect();
    gw.prompts.lock().unwrap().clear();
}

// ─── Platform fixture ──────────────────────────────────────────────────────────

const INSTANCES: &str = "http://ex.org/g/instances";
const DEFS: &str = "http://ex.org/g/defs";

/// A small platform: one public dataset with an instance graph and a
/// definitions graph, the latter registered as the published version of a
/// "Bruggenstandaard" vocabulary. The instance graph types subjects into NINE
/// classes with a deliberate frequency skew, so the 8-slot vocabulary sample
/// always excludes `def#Zeldzaam` — a REAL class that the sampled-vocabulary
/// window does not know about.
fn seed_platform(state: &AppState) {
    state
        .auth_db
        .create_organisation("o1", "Acme", "acme", None, None)
        .unwrap();
    state
        .auth_db
        .create_dataset(
            "d1",
            "Bruggen",
            None,
            OwnerType::Organisation,
            "o1",
            Visibility::Public,
            None,
        )
        .unwrap();
    state.auth_db.add_dataset_graph("d1", INSTANCES).unwrap();
    state.auth_db.add_dataset_graph("d1", DEFS).unwrap();

    let mut instances = String::new();
    for i in 1..=3 {
        instances.push_str(&format!(
            "<http://ex.org/id/b{i}> a <http://ex.org/def#Brug> ; \
             <http://www.w3.org/2004/02/skos/core#prefLabel> \"Brug {i}\" .\n"
        ));
    }
    for class in 2..=8 {
        for i in 1..=2 {
            instances.push_str(&format!(
                "<http://ex.org/id/k{class}x{i}> a <http://ex.org/def#Klasse{class}> .\n"
            ));
        }
    }
    instances.push_str("<http://ex.org/id/z1> a <http://ex.org/def#Zeldzaam> .\n");
    state
        .store
        .update(&format!(
            "INSERT DATA {{ GRAPH <{INSTANCES}> {{ {instances} }} }}"
        ))
        .unwrap();

    state
        .store
        .update(&format!(
            "INSERT DATA {{ GRAPH <{DEFS}> {{
               <http://ex.org/def#Brug> a <http://www.w3.org/2002/07/owl#Class> ;
                 <http://www.w3.org/2000/01/rdf-schema#label> \"Brug\" .
               <http://ex.org/def#Zeldzaam> a <http://www.w3.org/2002/07/owl#Class> ;
                 <http://www.w3.org/2000/01/rdf-schema#label> \"Zeldzame klasse\" .
             }} }}"
        ))
        .unwrap();

    const BASE: &str = "http://localhost:7878";
    registry::insert_data_model(
        &state.store,
        BASE,
        "bruggenstandaard",
        "Bruggenstandaard",
        "http://ex.org/def#",
        None,
        true,
        None,
        None,
        None,
        "2026-08-27T00:00:00Z",
    )
    .unwrap();
    registry::set_data_model_kind(&state.store, BASE, "bruggenstandaard", RegistryKind::Vocabulary)
        .unwrap();
    registry::insert_version(
        &state.store,
        BASE,
        &DataModelVersion {
            data_model_id: "bruggenstandaard".into(),
            version: "1.0.0".into(),
            status: VersionStatus::Published,
            graph_iri: DEFS.into(),
            sub_graphs: Vec::new(),
            created_at: "2026-08-27T00:00:00Z".into(),
            created_by: None,
            derived_from: None,
            notes: None,
            branch: None,
            sub_graph_status: Vec::new(),
        },
    )
    .unwrap();
    registry::update_latest_published(&state.store, BASE, "bruggenstandaard", "1.0.0").unwrap();
}

async fn chat_turn(state: AppState, token: &str, question: &str) -> Value {
    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/llm/chat")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::from(
            json!({"messages": [{"role": "user", "content": question}]}).to_string(),
        ))
        .unwrap();
    let resp = common::test_app(state).oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 200, "chat turn must succeed");
    common::body_json(resp.into_body()).await
}

// ─── Tests ─────────────────────────────────────────────────────────────────────

/// The system prompt must orient the model on what the question names: the
/// registered model WITH its published graph, and the store-verified location
/// of an IRI the user pasted. This is what was missing when Spark, asked about
/// definitions the user linked explicitly, guessed an instance graph and
/// invented namespaces.
#[tokio::test]
async fn prompt_carries_registered_models_and_pasted_iri_locations() {
    let _serial = test_lock();
    let gw = gateway();
    script(gw, &["Dat is Brug 1.", "Dat is Brug 1."]);
    let (state, token) = common::admin_state();
    seed_platform(&state);

    let resp = chat_turn(
        state,
        &token,
        "Welke gegevens zijn er over http://ex.org/id/b1 en wat voor object is dat?",
    )
    .await;
    assert_eq!(resp["answer"], "Dat is Brug 1.");

    let prompts = gw.prompts.lock().unwrap();
    let system = prompts[0]["messages"][0]["content"].as_str().unwrap();
    assert!(
        system.contains("## Registered models & vocabularies"),
        "prompt must list registered models"
    );
    assert!(
        system.contains(
            "\"Bruggenstandaard\" (vocabulary, namespace http://ex.org/def#) — definitions in \
             graph <http://ex.org/g/defs> (version 1.0.0)"
        ),
        "the model's published graph must be named as the place to query: {system}"
    );
    assert!(
        system.contains("# WHERE THIS CONVERSATION'S NAMES OCCUR"),
        "prompt must carry the orientation section"
    );
    assert!(
        system.contains("- <http://ex.org/id/b1> occurs as subject in <http://ex.org/g/instances>"),
        "the pasted IRI must be located in its graph: {system}"
    );
}

/// A query naming an IRI that occurs nowhere fails fast with the IRI named —
/// but a REAL term that merely fell outside the 8-class vocabulary sample runs
/// normally. On the previous sample-membership check both failed alike, which
/// burned every retrieval round on false "does not exist" errors.
#[tokio::test]
async fn invented_iris_fail_fast_while_real_unsampled_iris_run() {
    let _serial = test_lock();
    let gw = gateway();
    script(
        gw,
        &[
            "SPARQL:\nSELECT ?s WHERE { GRAPH <http://ex.org/g/instances> { ?s a <http://ex.org/def#Verzonnen> } }",
            "SPARQL:\nSELECT ?s WHERE { GRAPH <http://ex.org/g/instances> { ?s a <http://ex.org/def#Zeldzaam> } }",
            "Er is één zeldzaam object.",
        ],
    );
    let (state, token) = common::admin_state();
    seed_platform(&state);

    let resp = chat_turn(state, &token, "Hoeveel zeldzame objecten zijn er?").await;
    let queries = resp["queries"].as_array().unwrap();
    assert_eq!(
        queries.len(),
        2,
        "both rounds must be recorded: {queries:?}"
    );
    assert_eq!(queries[0]["ok"], false);
    let error = queries[0]["error"].as_str().unwrap();
    assert!(
        error.contains("occur nowhere") && error.contains("<http://ex.org/def#Verzonnen>"),
        "the invented IRI must be named in a store-verified error: {error}"
    );
    assert_eq!(
        queries[1]["ok"], true,
        "a real class outside the vocabulary sample must run: {queries:?}"
    );
    assert_eq!(queries[1]["rows"].as_array().unwrap().len(), 1);
    assert_eq!(resp["answer"], "Er is één zeldzaam object.");
}

/// An IRI the user pasted is never rejected as invented — even when it truly
/// does not exist. The honest outcome for an absent-but-asked-about IRI is a
/// query that runs and finds nothing, not an error claiming the user made the
/// IRI up.
#[tokio::test]
async fn pasted_iris_are_exempt_from_the_invented_iri_check() {
    let _serial = test_lock();
    let gw = gateway();
    script(
        gw,
        &[
            "SPARQL:\nSELECT ?s WHERE { GRAPH <http://ex.org/g/instances> { ?s a <http://ex.org/def#NietBestaand> } }",
            "Daar is niets over te vinden.",
        ],
    );
    let (state, token) = common::admin_state();
    seed_platform(&state);

    let resp = chat_turn(
        state,
        &token,
        "Zijn er objecten van het type http://ex.org/def#NietBestaand in de data?",
    )
    .await;
    let queries = resp["queries"].as_array().unwrap();
    assert_eq!(queries.len(), 1, "{queries:?}");
    assert_eq!(
        queries[0]["ok"], true,
        "a pasted IRI must run to an honest empty result, not an error: {queries:?}"
    );
    assert_eq!(queries[0]["rows"].as_array().unwrap().len(), 0);
    assert_eq!(resp["answer"], "Daar is niets over te vinden.");
}
