//! Who sees what of a dataset's history.
//!
//! A dataset's history names who changed it and keeps quads its graphs no
//! longer hold. Two runtime settings bound how far down the dataset roles
//! each goes, both `writers` unless an administrator says otherwise:
//!
//! * `OTS_ACTOR_VISIBILITY` — the author: `actor_iri` on change-feed rows,
//!   the actor and its resolved names on the commit trail, the agents of the
//!   PROV-O document, `created_by` on versions, `owner` on branches, `author`
//!   on patch-log entries.
//! * `OTS_CHANGE_PATCH_ACCESS` — one change as an RDF Patch (or its inverse)
//!   and a cursor on the feed.
//!
//! Each test walks anonymous, reader, writer and administrator over a public
//! and a private dataset. The last two cover what a reader of a public
//! dataset learns about its private graph from the commit trail and the
//! PROV-O document.

mod common;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use common::*;
use open_triplestore::auth::models::{ResourceRole, SystemRole};
use open_triplestore::server::AppState;
use open_triplestore::store::changes::{DEFAULT_MAX_PAYLOAD, DEFAULT_MAX_SCAN};
use open_triplestore::store::TripleStore;
use serde_json::{json, Value};
use tower::ServiceExt as _;

/// The account that makes every change, and the username it resolves to:
/// neither may appear in a response to a caller below the audience.
const AUTHOR: &str = "hist-author";
const AUTHOR_NAME: &str = "quillfeather";

async fn send(
    app: &Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
    content_type: Option<&str>,
    body: String,
) -> (StatusCode, String) {
    let mut b = Request::builder().method(method).uri(uri);
    if let Some(t) = token {
        b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    if let Some(ct) = content_type {
        b = b.header(header::CONTENT_TYPE, ct);
    }
    let resp = app
        .clone()
        .oneshot(b.body(Body::from(body)).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    (status, body_text(resp.into_body()).await)
}

async fn get(app: &Router, uri: &str, token: Option<&str>) -> (StatusCode, String) {
    send(app, Method::GET, uri, token, None, String::new()).await
}

async fn json_call(
    app: &Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
    body: Value,
) -> (StatusCode, String) {
    send(
        app,
        method,
        uri,
        token,
        Some("application/json"),
        body.to_string(),
    )
    .await
}

struct World {
    state: AppState,
    app: Router,
    admin: String,
    writer: String,
    reader: String,
    /// A public dataset: one graph every reader sees, one private graph.
    public: String,
    pub_graph: String,
    pub_private_graph: String,
    /// A private dataset the reader holds a viewer grant on.
    private: String,
    priv_graph: String,
}

impl World {
    /// `(label, token)` of the four callers, lowest first.
    fn callers(&self) -> [(&'static str, Option<&str>); 4] {
        [
            ("anonymous", None),
            ("reader", Some(self.reader.as_str())),
            ("writer", Some(self.writer.as_str())),
            ("admin", Some(self.admin.as_str())),
        ]
    }

    async fn set(&self, name: &str, value: &str) {
        let (st, body) = json_call(
            &self.app,
            Method::PUT,
            &format!("/api/admin/settings/{name}"),
            Some(&self.admin),
            json!({ "value": value }),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{name}={value}: {body}");
    }

    /// The sequence number of the first change to `graph`.
    fn seq_of(&self, graph: &str) -> i64 {
        self.state
            .store
            .changes()
            .rows_after(0, 1000)
            .iter()
            .find(|r| r.graph_iri.as_deref() == Some(graph))
            .and_then(|r| r.seq)
            .unwrap_or_else(|| panic!("no change to {graph}"))
    }
}

async fn dataset(app: &Router, admin: &str, visibility: &str, graphs: &[&str]) -> String {
    // A name of its own: version locks are process-wide and keyed by it.
    let name = format!("ha-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    let (st, body) = json_call(
        app,
        Method::POST,
        "/api/datasets",
        Some(admin),
        json!({ "name": name, "owner_type": "user", "owner_id": "adm", "visibility": visibility }),
    )
    .await;
    assert!(st.is_success(), "create dataset: {st} {body}");
    let id = serde_json::from_str::<Value>(&body).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    for g in graphs {
        let (st, body) = json_call(
            app,
            Method::POST,
            &format!("/api/datasets/{id}/graphs"),
            Some(admin),
            json!({ "graph_iri": g }),
        )
        .await;
        assert!(st.is_success(), "register {g}: {st} {body}");
    }
    id
}

/// Two datasets the author wrote to and cut a version of, so every history
/// route has something that names the author.
async fn world() -> World {
    // The matrix sends more requests than the limiter's burst allows one
    // client, and a 429 says nothing about who may read what.
    std::env::set_var("OTS_RATE_LIMIT_ENABLED", "false");
    let store = TripleStore::in_memory()
        .unwrap()
        .with_change_capture(DEFAULT_MAX_SCAN, DEFAULT_MAX_PAYLOAD);
    let (state, admin) = admin_state_with_store(store);
    let app = test_app(state.clone());
    state
        .auth_db
        .create_user(AUTHOR, AUTHOR_NAME, "author@t.com", "h", SystemRole::User)
        .unwrap();
    state
        .auth_db
        .create_user(
            "hist-reader",
            "reader",
            "reader@t.com",
            "h",
            SystemRole::User,
        )
        .unwrap();
    let writer = mint_token(AUTHOR, AUTHOR_NAME, "user");
    let reader = mint_token("hist-reader", "reader", "user");

    let tag = &uuid::Uuid::new_v4().simple().to_string()[..8];
    let pub_graph = format!("urn:ha:{tag}:open");
    let pub_private_graph = format!("urn:ha:{tag}:held-back");
    let priv_graph = format!("urn:ha:{tag}:closed");
    let public = dataset(&app, &admin, "public", &[&pub_graph, &pub_private_graph]).await;
    let private = dataset(&app, &admin, "private", &[&priv_graph]).await;
    let (st, body) = json_call(
        &app,
        Method::PATCH,
        &format!("/api/datasets/{public}/graphs"),
        Some(&admin),
        json!({ "graph_iri": pub_private_graph, "private": true }),
    )
    .await;
    assert!(st.is_success(), "{body}");

    for ds in [&public, &private] {
        state
            .auth_db
            .set_resource_grant("dataset", ds, "user", AUTHOR, ResourceRole::Editor, "adm")
            .unwrap();
    }
    state
        .auth_db
        .set_resource_grant(
            "dataset",
            &private,
            "user",
            "hist-reader",
            ResourceRole::Viewer,
            "adm",
        )
        .unwrap();
    // A dataset role does not write through the Graph Store or SPARQL Update
    // by itself: those ask for a write grant on the graph.
    for (n, g) in [&pub_graph, &pub_private_graph, &priv_graph]
        .into_iter()
        .enumerate()
    {
        state
            .auth_db
            .grant_graph_permission(&format!("ha-w-{n}"), g, "user", AUTHOR, "write", "adm")
            .unwrap();
    }
    state.auth_db.invalidate_accessible_graphs_cache();

    // The author writes each graph, then cuts a version of each dataset: a
    // commit, a change row, a version, a branch tip and a patch-log entry.
    for g in [&pub_graph, &pub_private_graph, &priv_graph] {
        let (st, body) = send(
            &app,
            Method::POST,
            &format!("/store?graph={}", url_encode(g)),
            Some(&writer),
            Some("text/turtle"),
            format!("<urn:ha:s> <urn:ha:in> <{g}> ."),
        )
        .await;
        assert!(st.is_success(), "write {g}: {st} {body}");
    }
    // (The public dataset's version leaves its private graph out: a patch-log
    // entry over a private graph is withheld from readers whole.)
    for (ds, graphs) in [(&public, &pub_graph), (&private, &priv_graph)] {
        let (st, body) = json_call(
            &app,
            Method::POST,
            &format!("/api/datasets/{ds}/versions"),
            Some(&writer),
            json!({ "version": "1.0.0", "graphs": [graphs] }),
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "cut: {body}");
    }

    World {
        state,
        app,
        admin,
        writer,
        reader,
        public,
        pub_graph,
        pub_private_graph,
        private,
        priv_graph,
    }
}

/// The history routes that can name the author, and a string each answer
/// holds whoever asks — so "no author" is never an empty answer.
fn actor_routes(ds: &str) -> Vec<(String, &'static str)> {
    vec![
        (format!("/api/datasets/{ds}/changes"), "\"rows\":[{"),
        (format!("/api/datasets/{ds}/commits"), "\"commit_id\""),
        (format!("/api/datasets/{ds}/provenance"), "a prov:Activity"),
        (
            format!("/api/datasets/{ds}/versions"),
            "\"version\":\"1.0.0\"",
        ),
        (
            format!("/api/datasets/{ds}/versions/1.0.0"),
            "\"version\":\"1.0.0\"",
        ),
        (format!("/api/datasets/{ds}/branches"), "\"tip_version\""),
        (format!("/api/datasets/{ds}/log"), "\"entries\":[{"),
    ]
}

fn names_the_author(body: &str) -> bool {
    body.contains(AUTHOR) || body.contains(AUTHOR_NAME)
}

/// Every history route of `ds` as `token`: answered and naming the author
/// (`Some(true)`), answered without (`Some(false)`), or refused (`None`).
async fn author_seen(w: &World, ds: &str, who: &str, token: Option<&str>) -> Option<bool> {
    let mut seen: Option<bool> = None;
    for (i, (uri, always)) in actor_routes(ds).into_iter().enumerate() {
        let (st, body) = get(&w.app, &uri, token).await;
        let this = if st == StatusCode::OK {
            assert!(body.contains(always), "{who} {uri}: {body}");
            Some(names_the_author(&body))
        } else {
            assert!(
                matches!(
                    st,
                    StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN | StatusCode::NOT_FOUND
                ),
                "{who} {uri}: {st} {body}"
            );
            assert!(!names_the_author(&body), "{who} {uri}: {body}");
            None
        };
        if i == 0 {
            seen = this;
        } else {
            assert_eq!(this, seen, "{who} {uri}: the routes disagree");
        }
    }
    seen
}

#[tokio::test]
async fn the_author_is_shown_to_writers_unless_the_administrator_widens_it() {
    let w = world().await;

    // Both settings are runtime settings, `writers` out of the box.
    let (st, body) = get(&w.app, "/api/admin/settings", Some(&w.admin)).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let listed: Value = serde_json::from_str(&body).unwrap();
    for name in ["OTS_ACTOR_VISIBILITY", "OTS_CHANGE_PATCH_ACCESS"] {
        let s = listed["settings"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["name"] == name)
            .unwrap_or_else(|| panic!("{name} is not a runtime setting: {body}"));
        assert_eq!(s["value"], "writers", "{s}");
        let (st, _) = json_call(
            &w.app,
            Method::PUT,
            &format!("/api/admin/settings/{name}"),
            Some(&w.admin),
            json!({ "value": "everyone" }),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{name} takes no other value");
        let (st, _) = json_call(
            &w.app,
            Method::PUT,
            &format!("/api/admin/settings/{name}"),
            Some(&w.writer),
            json!({ "value": "readers" }),
        )
        .await;
        assert_eq!(st, StatusCode::FORBIDDEN, "{name} is the administrator's");
    }

    // `(setting, [anonymous, reader, writer, admin] on the public dataset,
    //   the same on the private one)`; `None` is a refused request.
    let yes = Some(true);
    let no = Some(false);
    let table = [
        ("writers", [no, no, yes, yes], [None, no, yes, yes]),
        ("signed-in", [no, yes, yes, yes], [None, yes, yes, yes]),
        ("readers", [yes, yes, yes, yes], [None, yes, yes, yes]),
    ];
    for (i, (level, on_public, on_private)) in table.into_iter().enumerate() {
        // The first row is the default: nothing is set for it.
        if i > 0 {
            w.set("OTS_ACTOR_VISIBILITY", level).await;
        }
        for (ds, want) in [(&w.public, on_public), (&w.private, on_private)] {
            for ((who, token), want) in w.callers().into_iter().zip(want) {
                let got = author_seen(&w, ds, who, token).await;
                assert_eq!(got, want, "{level}: {who} on dataset {ds}");
            }
        }
        // Who sees the author says nothing about who gets the quads.
        let (st, _) = get(
            &w.app,
            &format!(
                "/api/datasets/{}/changes/{}",
                w.public,
                w.seq_of(&w.pub_graph)
            ),
            Some(&w.reader),
        )
        .await;
        assert_eq!(st, StatusCode::FORBIDDEN, "{level}");
    }
}

#[tokio::test]
async fn a_change_is_served_as_a_patch_to_writers_unless_the_administrator_widens_it() {
    let w = world().await;
    let ok = StatusCode::OK;
    let (unauth, forbidden, hidden) = (
        StatusCode::UNAUTHORIZED,
        StatusCode::FORBIDDEN,
        StatusCode::NOT_FOUND,
    );
    // `(setting, [anonymous, reader, writer, admin] fetching a patch of the
    //   public dataset, the same on the private one, the same setting a
    //   cursor on either)`. A private dataset is hidden from an anonymous
    // caller before anything else; a cursor needs an account at any setting.
    let table = [
        (
            "writers",
            [unauth, forbidden, ok, ok],
            [hidden, forbidden, ok, ok],
            [unauth, forbidden, ok, ok],
        ),
        (
            "signed-in",
            [unauth, ok, ok, ok],
            [hidden, ok, ok, ok],
            [unauth, ok, ok, ok],
        ),
        (
            "readers",
            [ok, ok, ok, ok],
            [hidden, ok, ok, ok],
            [unauth, ok, ok, ok],
        ),
    ];
    for (i, (level, on_public, on_private, cursors)) in table.into_iter().enumerate() {
        if i > 0 {
            w.set("OTS_CHANGE_PATCH_ACCESS", level).await;
        }
        for (ds, graph, want) in [
            (&w.public, &w.pub_graph, on_public),
            (&w.private, &w.priv_graph, on_private),
        ] {
            let seq = w.seq_of(graph);
            for (n, ((who, token), want)) in w.callers().into_iter().zip(want).enumerate() {
                for tail in ["", "/inverse"] {
                    let uri = format!("/api/datasets/{ds}/changes/{seq}{tail}");
                    let (st, body) = get(&w.app, &uri, token).await;
                    assert_eq!(st, want, "{level}: {who} {uri}: {body}");
                    // The quads go only where the patch does.
                    assert_eq!(
                        body.contains("<urn:ha:s> <urn:ha:in>"),
                        st == ok,
                        "{level}: {who} {uri}: {body}"
                    );
                }
                // A cursor of the caller's own, named after them.
                let uri = format!("/api/datasets/{ds}/changes/cursors/{who}");
                let (st, body) =
                    json_call(&w.app, Method::PUT, &uri, token, json!({ "seq": 1 })).await;
                assert_eq!(st, cursors[n], "{level}: {who} PUT {uri}: {body}");
            }
        }
        // The listing stays every reader's: which graph changed, and when.
        let (st, body) = get(&w.app, &format!("/api/datasets/{}/changes", w.public), None).await;
        assert_eq!(st, ok, "{level}: {body}");
        assert!(body.contains(&w.pub_graph), "{level}: {body}");
        assert!(!body.contains(&w.pub_private_graph), "{level}: {body}");
        // Who gets the quads says nothing about who sees the author.
        assert!(!names_the_author(&body), "{level}: {body}");
    }

    // A private graph's change stays its writers' at the widest setting.
    let seq = w.seq_of(&w.pub_private_graph);
    for (who, token) in [("anonymous", None), ("reader", Some(w.reader.as_str()))] {
        let (st, body) = get(
            &w.app,
            &format!("/api/datasets/{}/changes/{seq}", w.public),
            token,
        )
        .await;
        assert_eq!(st, hidden, "{who}: {body}");
    }
}

#[tokio::test]
async fn a_cursor_set_under_a_wider_setting_can_still_be_deleted() {
    let w = world().await;
    let uri = format!("/api/datasets/{}/changes/cursors/mirror", w.public);
    let key = format!("dataset:{}:mirror", w.public);

    // Out of the box a reader's cursor is refused, so nothing a reader does
    // holds rows back from the sweep.
    let (st, _) = json_call(
        &w.app,
        Method::PUT,
        &uri,
        Some(&w.reader),
        json!({ "seq": 0 }),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    assert!(w.state.store.changes().cursor(&key).is_none());

    w.set("OTS_CHANGE_PATCH_ACCESS", "signed-in").await;
    let (st, body) = json_call(
        &w.app,
        Method::PUT,
        &uri,
        Some(&w.reader),
        json!({ "seq": 0 }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert!(w.state.store.changes().cursor(&key).is_some());

    // Narrowed again: the reader can no longer refresh it (it expires), and
    // can still take it away.
    w.set("OTS_CHANGE_PATCH_ACCESS", "writers").await;
    let (st, _) = json_call(
        &w.app,
        Method::PUT,
        &uri,
        Some(&w.reader),
        json!({ "seq": 0 }),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, body) = send(
        &w.app,
        Method::DELETE,
        &uri,
        Some(&w.reader),
        None,
        String::new(),
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT, "{body}");
    assert!(w.state.store.changes().cursor(&key).is_none());
}

#[tokio::test]
async fn a_read_only_token_of_a_writer_is_in_the_writers_audience() {
    let w = world().await;
    let (st, body) = json_call(
        &w.app,
        Method::POST,
        "/api/auth/tokens",
        Some(&w.writer),
        json!({ "name": "mirror", "scopes": ["read"] }),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{body}");
    let token = serde_json::from_str::<Value>(&body).unwrap()["token"]
        .as_str()
        .unwrap()
        .to_string();

    // The account's role on the dataset decides, not the credential's scope:
    // the token reads the authors and the patches…
    assert_eq!(
        author_seen(&w, &w.public, "read-only token", Some(&token)).await,
        Some(true)
    );
    let (st, body) = get(
        &w.app,
        &format!(
            "/api/datasets/{}/changes/{}",
            w.public,
            w.seq_of(&w.pub_graph)
        ),
        Some(&token),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    // …and still writes nothing: no data, and no cursor either (a PUT).
    let (st, body) = send(
        &w.app,
        Method::POST,
        &format!("/store?graph={}", url_encode(&w.pub_graph)),
        Some(&token),
        Some("text/turtle"),
        "<urn:ha:x> <urn:ha:p> 1 .".to_string(),
    )
    .await;
    assert!(
        matches!(st, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN),
        "{st} {body}"
    );
    let (st, body) = json_call(
        &w.app,
        Method::PUT,
        &format!("/api/datasets/{}/changes/cursors/mirror", w.public),
        Some(&token),
        json!({ "seq": 1 }),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{body}");
}

#[tokio::test]
async fn the_provenance_document_covers_only_the_graphs_its_reader_may_read() {
    let w = world().await;
    let uri = format!("/api/datasets/{}/provenance", w.public);

    // A reader of the public dataset learns nothing of its private graph: not
    // its name, not the commit that wrote it.
    let (_, commits) = get(
        &w.app,
        &format!("/api/datasets/{}/commits", w.public),
        Some(&w.writer),
    )
    .await;
    let commits: Value = serde_json::from_str(&commits).unwrap();
    let private_commit = commits
        .as_array()
        .unwrap()
        .iter()
        .find(|c| {
            c["affected_graphs"]
                .as_array()
                .is_some_and(|g| g.iter().any(|g| g == w.pub_private_graph.as_str()))
                && c["kind"] == "graph-store"
        })
        .unwrap_or_else(|| panic!("the private graph's commit: {commits}"))["commit_id"]
        .as_str()
        .unwrap()
        .to_string();
    for (who, token) in [("anonymous", None), ("reader", Some(w.reader.as_str()))] {
        let (st, doc) = get(&w.app, &uri, token).await;
        assert_eq!(st, StatusCode::OK, "{who}: {doc}");
        assert!(doc.contains(&format!("<{}>", w.pub_graph)), "{who}: {doc}");
        assert!(!doc.contains(&w.pub_private_graph), "{who}: {doc}");
        assert!(!doc.contains(&private_commit), "{who}: {doc}");
    }
    // Its writers and administrators still get all of it.
    for (who, token) in [("writer", &w.writer), ("admin", &w.admin)] {
        let (st, doc) = get(&w.app, &uri, Some(token)).await;
        assert_eq!(st, StatusCode::OK, "{who}: {doc}");
        assert!(
            doc.contains(&format!("<{}>", w.pub_private_graph)),
            "{who}: {doc}"
        );
        assert!(doc.contains(&private_commit), "{who}: {doc}");
    }
}

#[tokio::test]
async fn a_commit_over_a_public_and_a_private_graph_names_only_the_public_one() {
    let w = world().await;
    // One update, both graphs: the commit belongs in the reader's trail (it
    // changed a graph they read) without naming the graph they do not.
    let (st, body) = send(
        &w.app,
        Method::POST,
        "/sparql",
        Some(&w.writer),
        Some("application/sparql-update"),
        format!(
            "INSERT DATA {{ GRAPH <{}> {{ <urn:ha:both> <urn:ha:p> 1 }} GRAPH <{}> {{ <urn:ha:both> <urn:ha:p> 2 }} }}",
            w.pub_graph, w.pub_private_graph
        ),
    )
    .await;
    assert!(st.is_success(), "{st} {body}");

    let both = |commits: &Value| -> Option<Value> {
        commits
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["kind"] == "sparql")
            .cloned()
    };
    let (_, as_writer) = get(
        &w.app,
        &format!("/api/datasets/{}/commits", w.public),
        Some(&w.writer),
    )
    .await;
    let seen = both(&serde_json::from_str(&as_writer).unwrap())
        .unwrap_or_else(|| panic!("the update's commit: {as_writer}"));
    let graphs: Vec<&str> = seen["affected_graphs"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(
        graphs.contains(&w.pub_graph.as_str()) && graphs.contains(&w.pub_private_graph.as_str()),
        "{seen}"
    );

    for (who, token) in [("anonymous", None), ("reader", Some(w.reader.as_str()))] {
        let (st, body) = get(
            &w.app,
            &format!("/api/datasets/{}/commits", w.public),
            token,
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{who}: {body}");
        let c = both(&serde_json::from_str(&body).unwrap())
            .unwrap_or_else(|| panic!("{who} misses the update's commit: {body}"));
        assert_eq!(c["affected_graphs"], json!([w.pub_graph]), "{who}: {c}");
        assert!(!body.contains(&w.pub_private_graph), "{who}: {body}");
        let (st, doc) = get(
            &w.app,
            &format!("/api/datasets/{}/provenance", w.public),
            token,
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{who}: {doc}");
        assert!(
            doc.contains(seen["commit_id"].as_str().unwrap()),
            "{who}: {doc}"
        );
        assert!(!doc.contains(&w.pub_private_graph), "{who}: {doc}");
    }
}
