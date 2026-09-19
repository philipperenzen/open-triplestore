//! What an unauthenticated (or merely signed-in) caller may learn about the
//! accounts on an instance, and which compute endpoints it may spend CPU on.
//!
//! `GET /api/users/public` exists so the UI can put a name and an avatar on the
//! owner chip of something the caller can already see. It used to answer with
//! every active account on the instance, to anybody, which turned an owner-label
//! lookup into an account-enumeration endpoint: a stranger could read off the
//! full staff roster of a private deployment. It now answers with the users the
//! caller could already infer from the resources visible to it — the owners of
//! the datasets it may read, plus the members of the organisations it belongs to
//! (which it may already list through `/api/organisations/:id/members`) — while
//! `GET /api/users`, the admin directory, stays exactly as it was.
//!
//! `POST /api/shaclc/parse` and `POST /api/rml/preview` disclose nothing stored,
//! but they parse and transform caller-supplied input, so anonymous access is
//! free CPU for anyone who finds the host. Both now require a token.

mod common;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use common::*;
use open_triplestore::auth::models::{OwnerType, Role, SystemRole, Visibility};
use open_triplestore::server::AppState;
use serde_json::Value;
use std::collections::BTreeSet;
use tower::ServiceExt as _;

/// An instance with four accounts arranged so that every visibility rule has a
/// witness:
///
/// - `adm` — the super admin; owns nothing.
/// - `owner` — owns one PUBLIC dataset, so anybody can already see that this
///   account exists by reading the dataset's `owner_id`.
/// - `teammate` — owns nothing public, but shares an organisation with
///   `outsider`, who may already list that organisation's members.
/// - `hermit` — owns one PRIVATE dataset and shares nothing with anybody. No
///   caller but an admin has any way to learn that this account exists.
fn roster_state() -> (AppState, String) {
    let (state, admin_token) = admin_state();
    let db = &state.auth_db;

    for (id, username) in [
        ("owner", "owner"),
        ("teammate", "teammate"),
        ("outsider", "outsider"),
        ("hermit", "hermit"),
    ] {
        db.create_user(
            id,
            username,
            &format!("{username}@test.com"),
            "hash",
            SystemRole::User,
        )
        .unwrap();
    }

    db.create_organisation("org-x", "Org X", "org-x", None, None)
        .unwrap();
    db.add_org_member("outsider", "org-x", Role::Member)
        .unwrap();
    db.add_org_member("teammate", "org-x", Role::Member)
        .unwrap();

    db.create_dataset(
        "ds-public",
        "Public dataset",
        None,
        OwnerType::User,
        "owner",
        Visibility::Public,
        None,
    )
    .unwrap();
    db.create_dataset(
        "ds-hermit",
        "Hermit's dataset",
        None,
        OwnerType::User,
        "hermit",
        Visibility::Private,
        None,
    )
    .unwrap();

    (state, admin_token)
}

async fn get(app: &Router, uri: &str, token: Option<&str>) -> (StatusCode, Value) {
    let mut b = Request::builder().method(Method::GET).uri(uri);
    if let Some(t) = token {
        b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    let resp = app
        .clone()
        .oneshot(b.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    (status, body_json(resp.into_body()).await)
}

/// The usernames `GET /api/users/public` reports to this caller.
async fn public_usernames(app: &Router, token: Option<&str>) -> BTreeSet<String> {
    let (status, body) = get(app, "/api/users/public", token).await;
    assert_eq!(status, StatusCode::OK, "/api/users/public → {body}");
    body.as_array()
        .expect("the response is still a JSON array")
        .iter()
        .map(|u| {
            // The shape is part of the contract: the UI reads exactly these three
            // fields, and only the membership of the list is being tightened.
            assert!(u["id"].is_string(), "each entry still has an id: {u}");
            assert!(
                u.get("avatar_key").is_some(),
                "each entry still carries avatar_key (null when unset): {u}"
            );
            assert!(
                u.get("email").is_none() && u.get("role").is_none(),
                "the public projection still leaks neither email nor role: {u}"
            );
            u["username"]
                .as_str()
                .expect("each entry still has a username")
                .to_string()
        })
        .collect()
}

fn names(usernames: &[&str]) -> BTreeSet<String> {
    usernames.iter().map(|s| s.to_string()).collect()
}

#[tokio::test]
async fn an_anonymous_caller_sees_only_the_owner_of_something_public() {
    let (state, _) = roster_state();
    let app = test_app(state);

    // `owner` is inferable from the public dataset's `owner_id`; nobody else is.
    assert_eq!(
        public_usernames(&app, None).await,
        names(&["owner"]),
        "an anonymous caller must not be able to enumerate the accounts"
    );
}

#[tokio::test]
async fn a_signed_in_non_admin_sees_the_same_restricted_list() {
    let (state, _) = roster_state();
    let app = test_app(state);
    let token = mint_token("outsider", "outsider", "user");

    // Signing in adds exactly what signing in reveals: the caller itself, and the
    // organisation co-members it may already list. It does not hand over the
    // roster, so `hermit` stays invisible.
    assert_eq!(
        public_usernames(&app, Some(&token)).await,
        names(&["outsider", "owner", "teammate"]),
        "an ordinary account must not be able to enumerate the accounts either"
    );
}

#[tokio::test]
async fn an_admin_still_sees_every_account() {
    let (state, admin_token) = roster_state();
    let app = test_app(state);

    assert_eq!(
        public_usernames(&app, Some(&admin_token)).await,
        names(&["admin", "hermit", "outsider", "owner", "teammate"]),
        "the admin directory view is unchanged"
    );
}

#[tokio::test]
async fn the_owner_of_a_public_dataset_is_still_labelled() {
    // Regression guard for the reason the endpoint exists: the owner chip on a
    // public dataset page must keep resolving for a logged-out visitor, so the
    // owner's id and username must both survive the tightening.
    let (state, _) = roster_state();
    let app = test_app(state);

    let (status, body) = get(&app, "/api/users/public", None).await;
    assert_eq!(status, StatusCode::OK);
    let entry = body
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["id"] == "owner")
        .expect("the owner of the public dataset is still listed");
    assert_eq!(entry["username"], "owner");
}

#[tokio::test]
async fn the_admin_user_directory_is_unchanged() {
    let (state, admin_token) = roster_state();
    let app = test_app(state);
    let user_token = mint_token("outsider", "outsider", "user");

    let (status, _) = get(&app, "/api/users", None).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "the admin directory still refuses anonymous callers"
    );

    let (status, _) = get(&app, "/api/users", Some(&user_token)).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "the admin directory still refuses a non-admin"
    );

    let (status, body) = get(&app, "/api/users", Some(&admin_token)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body.as_array().expect("array").len(),
        5,
        "an admin still gets every account from the admin directory: {body}"
    );
}

// ─── Anonymous compute ───────────────────────────────────────────────────────

const SHACLC: &str = "PREFIX ex: <http://example.org/>\n\
                      PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>\n\n\
                      shape ex:PersonShape -> ex:Person {\n\
                      \tex:name xsd:string [1..1] ;\n\
                      }\n";

const RML_BOUNDARY: &str = "ots-auth-exposure-boundary";

const RML_MAPPING: &str = r#"
@prefix rml: <http://semweb.mmlab.be/ns/rml#> .
@prefix rr: <http://www.w3.org/ns/r2rml#> .
@prefix ql: <http://semweb.mmlab.be/ns/ql#> .
@prefix ex: <http://example.org/> .

<http://example.org/TriplesMap> a rr:TriplesMap ;
    rml:logicalSource [
        rml:source "data.csv" ;
        rml:referenceFormulation ql:CSV
    ] ;
    rr:subjectMap [ rr:template "http://example.org/{id}" ] ;
    rr:predicateObjectMap [
        rr:predicate ex:name ;
        rr:objectMap [ rml:reference "name" ]
    ] .
"#;

fn rml_preview_body() -> String {
    format!(
        "--{b}\r\nContent-Disposition: form-data; name=\"mapping\"\r\n\r\n{m}\r\n\
         --{b}\r\nContent-Disposition: form-data; name=\"data.csv\"; filename=\"data.csv\"\r\n\
         Content-Type: text/csv\r\n\r\n{c}\r\n--{b}--\r\n",
        b = RML_BOUNDARY,
        m = RML_MAPPING,
        c = "id,name\n1,Alice\n2,Bob\n",
    )
}

async fn post(
    app: &Router,
    uri: &str,
    content_type: &str,
    body: String,
    token: Option<&str>,
) -> StatusCode {
    let mut b = Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header(header::CONTENT_TYPE, content_type);
    if let Some(t) = token {
        b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    app.clone()
        .oneshot(b.body(Body::from(body)).unwrap())
        .await
        .unwrap()
        .status()
}

#[tokio::test]
async fn shaclc_parse_requires_a_token() {
    let (state, token) = admin_state();
    let app = test_app(state);

    assert_eq!(
        post(
            &app,
            "/api/shaclc/parse",
            "text/shaclc",
            SHACLC.to_string(),
            None
        )
        .await,
        StatusCode::UNAUTHORIZED,
        "parsing is work the instance does for a caller, so it is not free"
    );
    assert_eq!(
        post(
            &app,
            "/api/shaclc/parse",
            "text/shaclc",
            SHACLC.to_string(),
            Some(&token)
        )
        .await,
        StatusCode::OK,
        "a signed-in caller still gets a parse"
    );
}

#[tokio::test]
async fn rml_preview_requires_a_token() {
    let (state, token) = admin_state();
    let app = test_app(state);

    assert_eq!(
        post(
            &app,
            "/api/rml/preview",
            &format!("multipart/form-data; boundary={RML_BOUNDARY}"),
            rml_preview_body(),
            None
        )
        .await,
        StatusCode::UNAUTHORIZED,
        "running a mapping is work the instance does for a caller, so it is not free"
    );
    assert_eq!(
        post(
            &app,
            "/api/rml/preview",
            &format!("multipart/form-data; boundary={RML_BOUNDARY}"),
            rml_preview_body(),
            Some(&token)
        )
        .await,
        StatusCode::OK,
        "a signed-in caller still gets a preview"
    );
}
