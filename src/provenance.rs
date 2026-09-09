//! PROV-O view of a dataset: `GET /api/datasets/:id/provenance`.
//!
//! The commit log already records every data mutation as a `prov:Activity`
//! with its agent, its affected graphs (entities it generated) and, for
//! versioned datasets, the version records with `prov:wasDerivedFrom`. This
//! endpoint assembles the dataset's slice of that trail into one PROV-O
//! document — the dataset and its graphs as entities, the activities that
//! produced them, the agents involved, and the dataset's versions as
//! specialisations — in Turtle, so a client can follow provenance with plain
//! PROV vocabulary and no knowledge of this store's commit model.
//!
//! Statement-level (RDF-star) qualification is deliberately absent: the trail
//! records *which graphs* an activity touched, not which triples; per-triple
//! attribution needs per-write change capture (the RDF Patch roadmap item).

use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::IntoResponse;
use axum::Extension;
use std::fmt::Write as _;

use crate::auth::middleware::AuthenticatedUser;
use crate::auth::models::{Dataset, OwnerType};
use crate::commit_log::{list_commits, CommitQuery, CommitScope};
use crate::server::AppState;

fn esc(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

/// The owner of a dataset as an agent IRI (matches the DCAT catalogue).
pub fn owner_iri(base_url: &str, ds: &Dataset) -> String {
    let base = base_url.trim_end_matches('/');
    match ds.owner_type {
        OwnerType::Organisation => format!("{base}/org/{}", ds.owner_id),
        OwnerType::User => format!("{base}/user/{}", ds.owner_id),
        OwnerType::Group => format!("{base}/group/{}", ds.owner_id),
    }
}

/// The dataset's PROV-O trail as Turtle.
pub fn dataset_provenance_turtle(state: &AppState, ds: &Dataset) -> String {
    let base = state.base_url.trim_end_matches('/');
    let ds_iri = format!("{base}/dataset/{}", ds.id);
    let graphs = state
        .auth_db
        .list_dataset_graphs(&ds.id)
        .unwrap_or_default();
    let commits = list_commits(
        &state.store,
        &CommitScope::Graphs(graphs.clone()),
        &CommitQuery::default(),
    );
    let versions = crate::dataset_versions::registry::list_versions(&state.store, base, &ds.id);

    let mut out = String::new();
    out.push_str("@prefix prov: <http://www.w3.org/ns/prov#> .\n");
    out.push_str("@prefix dcat: <http://www.w3.org/ns/dcat#> .\n");
    out.push_str("@prefix dct: <http://purl.org/dc/terms/> .\n");
    out.push_str("@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n");
    out.push_str("@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .\n");
    out.push_str("@prefix ver: <urn:system:vocab/> .\n\n");

    // The dataset and its owner.
    let owner = owner_iri(base, ds);
    writeln!(out, "<{ds_iri}> a prov:Entity, dcat:Dataset ;").unwrap();
    writeln!(out, "    dct:title \"{}\" ;", esc(&ds.name)).unwrap();
    writeln!(out, "    prov:wasAttributedTo <{owner}> ;").unwrap();
    if let Some(latest) = commits.first() {
        writeln!(
            out,
            "    prov:wasGeneratedBy <{base}/commit/{}> ;",
            latest.commit_id
        )
        .unwrap();
    }
    writeln!(
        out,
        "    prov:generatedAtTime \"{}\"^^xsd:dateTime .",
        ds.created_at
    )
    .unwrap();
    writeln!(out, "<{owner}> a prov:Agent .\n").unwrap();

    // Each graph is an entity that specialises the dataset; its most recent
    // generating activity is the newest commit that touched it.
    for g in &graphs {
        writeln!(out, "<{g}> a prov:Entity ;").unwrap();
        writeln!(out, "    prov:specializationOf <{ds_iri}> ;").unwrap();
        if let Some(c) = commits
            .iter()
            .find(|c| c.affected_graphs.iter().any(|a| a == g))
        {
            writeln!(
                out,
                "    prov:wasGeneratedBy <{base}/commit/{}> ;",
                c.commit_id
            )
            .unwrap();
        }
        writeln!(out, "    prov:wasAttributedTo <{owner}> .").unwrap();
    }
    if !graphs.is_empty() {
        out.push('\n');
    }

    // Activities, newest first, with their agents.
    for c in &commits {
        let iri = format!("{base}/commit/{}", c.commit_id);
        writeln!(out, "<{iri}> a prov:Activity ;").unwrap();
        writeln!(out, "    rdfs:label \"{}\" ;", esc(&c.message)).unwrap();
        writeln!(out, "    ver:kind \"{}\" ;", c.kind.as_str()).unwrap();
        writeln!(out, "    ver:added {} ;", c.added).unwrap();
        writeln!(out, "    ver:removed {} ;", c.removed).unwrap();
        if let Some(v) = &c.version {
            writeln!(out, "    ver:onVersion \"{}\" ;", esc(v)).unwrap();
        }
        if let Some(a) = &c.actor_iri {
            writeln!(out, "    prov:wasAssociatedWith <{a}> ;").unwrap();
        }
        for g in &c.affected_graphs {
            writeln!(out, "    prov:used <{g}> ;").unwrap();
            writeln!(out, "    prov:generated <{g}> ;").unwrap();
        }
        writeln!(
            out,
            "    prov:startedAtTime \"{0}\"^^xsd:dateTime ;",
            c.created_at
        )
        .unwrap();
        writeln!(
            out,
            "    prov:endedAtTime \"{0}\"^^xsd:dateTime .",
            c.created_at
        )
        .unwrap();
        if let Some(a) = &c.actor_iri {
            writeln!(out, "<{a}> a prov:Agent .").unwrap();
        }
    }
    if !commits.is_empty() {
        out.push('\n');
    }

    // Versions: immutable snapshots, derived from their predecessor.
    for v in &versions {
        let iri = format!("{base}/dataset/{}/version/{}", ds.id, v.version);
        writeln!(out, "<{iri}> a prov:Entity ;").unwrap();
        writeln!(out, "    prov:specializationOf <{ds_iri}> ;").unwrap();
        writeln!(out, "    ver:version \"{}\" ;", esc(&v.version)).unwrap();
        if let Some(from) = &v.derived_from {
            writeln!(
                out,
                "    prov:wasRevisionOf <{base}/dataset/{}/version/{}> ;",
                ds.id,
                esc(from)
            )
            .unwrap();
        }
        if let Some(by) = &v.created_by {
            writeln!(out, "    prov:wasAttributedTo <{base}/users/{by}> ;").unwrap();
        }
        writeln!(
            out,
            "    prov:generatedAtTime \"{}\"^^xsd:dateTime .",
            v.created_at
        )
        .unwrap();
    }
    out
}

/// GET /api/datasets/:dataset_id/provenance — the dataset's PROV-O trail.
pub async fn get_dataset_provenance(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    Path(dataset_id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let ds = state
        .auth_db
        .get_dataset(&dataset_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Dataset not found".to_string()))?;
    let uid = user.as_ref().map(|Extension(u)| u.user_id.as_str());
    let visible = state
        .auth_db
        .can_access_dataset(uid, &ds)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if !visible {
        return Err((StatusCode::NOT_FOUND, "Dataset not found".to_string()));
    }
    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/turtle; charset=utf-8")],
        dataset_provenance_turtle(&state, &ds),
    ))
}
