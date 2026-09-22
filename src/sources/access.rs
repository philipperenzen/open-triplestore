//! Who may reach the sources API.
//!
//! The registry is admin territory: a datasource holds a pointer to a
//! production credential and a run writes instance data. The one non-admin
//! principal is the **mapping proposer** — a service that runs against the
//! store and nothing else, holding an API token scoped `sources:read` and
//! `mappings:propose`. Those scopes name routes, not a rung of the capability
//! ladder: `sources:read` reads the registry, profiles, mappings, runs,
//! tickets and the mapping gates, with a datasource's location, credential
//! reference and raw rows withheld ([`super::model::SourceResponse::scrubbed`],
//! and `/preview` refused outright); `mappings:propose` creates a mapping in
//! the `proposed` state, refines one that is still proposed, and dry-runs it.
//! Approving, running, deleting and every other write stay with admins.

use axum::extract::Request;
use axum::http::{Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::auth::middleware::AuthenticatedUser;
use crate::auth::models::ApiScope;

/// The mutations that proposing consists of: creating or refining a mapping,
/// and dry-running one. Consulted by the write-scope check as well, so a
/// `mappings:propose` token — which carries no general write grant — reaches
/// these and nothing else.
pub fn is_proposer_mutation(method: &Method, path: &str) -> bool {
    let path = path.trim_end_matches('/');
    match *method {
        Method::POST => {
            path == "/api/mappings"
                || (path.starts_with("/api/sources/") && path.ends_with("/dry-run"))
        }
        Method::PUT => path
            .strip_prefix("/api/mappings/")
            .is_some_and(|rest| !rest.is_empty() && !rest.contains('/')),
        _ => false,
    }
}

/// What `sources:read` may read. Raw rows are not metadata: `/preview` stays
/// with admins, whatever the scope — and so does the review queue, whose
/// items carry a snapshot of the subject as the candidate graph describes
/// it. The decisions on a mapping (`/api/mappings/:id/reviews`) are the
/// proposer's training data and are readable.
pub fn is_readable_by_proposer(path: &str) -> bool {
    let path = path.trim_end_matches('/');
    if path.ends_with("/preview") || path.starts_with("/api/reviews") {
        return false;
    }
    if path.starts_with("/api/sources/") && path.ends_with("/reviews") {
        return false;
    }
    path.starts_with("/api/sources")
        || path.starts_with("/api/mappings")
        || path.starts_with("/api/runs")
        || path.starts_with("/api/tickets")
        || (path.starts_with("/api/models/") && path.ends_with("/profile"))
}

/// A POST that computes and writes nothing, open to `sources:read`:
/// calibration over the recorded decisions (or over points in the body).
pub fn is_proposer_query(method: &Method, path: &str) -> bool {
    *method == Method::POST && path.trim_end_matches('/') == "/api/sources/calibration"
}

/// The sources router's gate: admins pass; a scoped service token passes the
/// routes its scopes name; everyone else is refused.
pub async fn guard(req: Request, next: Next) -> Result<Response, Response> {
    let user = req
        .extensions()
        .get::<AuthenticatedUser>()
        .ok_or_else(|| (StatusCode::UNAUTHORIZED, "Authentication required").into_response())?;
    if user.is_admin() {
        return Ok(next.run(req).await);
    }
    let path = req.uri().path();
    let allowed = match *req.method() {
        Method::GET | Method::HEAD => {
            user.has_scope(ApiScope::SourcesRead) && is_readable_by_proposer(path)
        }
        ref method => {
            (user.has_scope(ApiScope::SourcesRead) && is_proposer_query(method, path))
                || (user.has_scope(ApiScope::MappingsPropose) && is_proposer_mutation(method, path))
        }
    };
    if !allowed {
        return Err((
            StatusCode::FORBIDDEN,
            "Admin access required; a service token needs `sources:read` to read here and \
             `mappings:propose` to propose a mapping",
        )
            .into_response());
    }
    Ok(next.run(req).await)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proposing_is_a_mapping_and_a_dry_run_and_nothing_else() {
        assert!(is_proposer_mutation(&Method::POST, "/api/mappings"));
        assert!(is_proposer_mutation(
            &Method::PUT,
            "/api/mappings/products-map"
        ));
        assert!(is_proposer_mutation(
            &Method::POST,
            "/api/sources/legacy/dry-run"
        ));
        for (m, p) in [
            (Method::POST, "/api/mappings/convert"),
            (Method::POST, "/api/mappings/x/decisions"),
            (Method::DELETE, "/api/mappings/x"),
            (Method::POST, "/api/sources"),
            (Method::POST, "/api/sources/legacy/runs"),
            (Method::PUT, "/api/sources/gates"),
            (Method::POST, "/api/runs/r/promote"),
            (Method::POST, "/api/sources/legacy/profile"),
            (Method::PUT, "/api/mappings/x/rml"),
        ] {
            assert!(!is_proposer_mutation(&m, p), "{m} {p}");
        }
    }

    #[test]
    fn reading_covers_metadata_and_never_rows() {
        for p in [
            "/api/sources",
            "/api/sources/legacy",
            "/api/sources/legacy/profile",
            "/api/sources/legacy/introspect",
            "/api/sources/gates",
            "/api/mappings/x/rml",
            "/api/runs/r",
            "/api/tickets/t",
            "/api/models/m/versions/1.0.0/profile",
        ] {
            assert!(is_readable_by_proposer(p), "{p}");
        }
        assert!(is_readable_by_proposer("/api/mappings/x/reviews"));
        assert!(is_readable_by_proposer("/api/mappings/x/provenance"));
        assert!(!is_readable_by_proposer("/api/sources/legacy/preview"));
        assert!(!is_readable_by_proposer("/api/sources/legacy/reviews"));
        assert!(!is_readable_by_proposer("/api/reviews/r"));
        assert!(!is_readable_by_proposer("/api/models/m/versions/1.0.0"));
        assert!(!is_readable_by_proposer("/api/datasets"));
    }

    #[test]
    fn calibration_is_a_query_not_a_mutation() {
        assert!(is_proposer_query(&Method::POST, "/api/sources/calibration"));
        assert!(!is_proposer_query(&Method::GET, "/api/sources/calibration"));
        assert!(!is_proposer_query(
            &Method::POST,
            "/api/sources/legacy/runs"
        ));
        assert!(!is_proposer_mutation(
            &Method::POST,
            "/api/sources/calibration"
        ));
    }
}
