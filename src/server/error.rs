//! HTTP error handling for the SPARQL endpoint.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

/// Application error type that converts to HTTP responses.
#[derive(Debug)]
pub enum AppError {
    /// 400 Bad Request — malformed query or data
    BadRequest(String),
    /// 401 Unauthorized — authentication required
    Unauthorized(String),
    /// 403 Forbidden — authenticated but lacking required role/permission
    Forbidden(String),
    /// 404 Not Found
    NotFound(String),
    /// 409 Conflict — optimistic-concurrency failure; body carries the current revision
    Conflict(serde_json::Value),
    /// 415 Unsupported Media Type
    UnsupportedMediaType(String),
    /// 422 Unprocessable Entity — SHACL validation failed
    ValidationFailed(crate::shacl::report::ValidationReport),
    /// 500 Internal Server Error — message is logged server-side only
    Internal(String),
}

impl AppError {
    /// A short human-readable message for logging/notification (not the HTTP body).
    pub fn message(&self) -> String {
        match self {
            AppError::BadRequest(m)
            | AppError::Unauthorized(m)
            | AppError::Forbidden(m)
            | AppError::NotFound(m)
            | AppError::UnsupportedMediaType(m)
            | AppError::Internal(m) => m.clone(),
            AppError::Conflict(v) => v.to_string(),
            AppError::ValidationFailed(_) => "SHACL validation failed".to_string(),
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        match self {
            AppError::ValidationFailed(report) => {
                let body = serde_json::json!({
                    "error": "SHACL validation failed",
                    "conforms": report.conforms,
                    "results": report.results.iter().map(|r| {
                        serde_json::json!({
                            "severity": format!("{:?}", r.severity),
                            "focusNode": r.focus_node,
                            "path": r.path,
                            "value": r.value,
                            "message": r.message,
                            "sourceShape": r.source_shape,
                            "sourceConstraint": r.source_constraint,
                        })
                    }).collect::<Vec<_>>(),
                });
                (StatusCode::UNPROCESSABLE_ENTITY, axum::Json(body)).into_response()
            }
            AppError::Conflict(body) => {
                (StatusCode::CONFLICT, axum::Json(body)).into_response()
            }
            other => {
                let (status, message) = match other {
                    AppError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg),
                    AppError::Unauthorized(msg) => (StatusCode::UNAUTHORIZED, msg),
                    AppError::Forbidden(msg) => (StatusCode::FORBIDDEN, msg),
                    AppError::NotFound(msg) => (StatusCode::NOT_FOUND, msg),
                    AppError::UnsupportedMediaType(msg) => (StatusCode::UNSUPPORTED_MEDIA_TYPE, msg),
                    AppError::Internal(msg) => {
                        tracing::error!("Internal server error: {}", msg);
                        (StatusCode::INTERNAL_SERVER_ERROR, "Internal server error".to_string())
                    }
                    AppError::ValidationFailed(_) | AppError::Conflict(_) => unreachable!(),
                };
                (status, message).into_response()
            }
        }
    }
}

impl From<crate::store::engine::StoreError> for AppError {
    fn from(err: crate::store::engine::StoreError) -> Self {
        match err {
            crate::store::engine::StoreError::Parse(msg) => AppError::BadRequest(msg),
            crate::store::engine::StoreError::UnsupportedFormat(msg) => {
                AppError::UnsupportedMediaType(msg)
            }
            crate::store::engine::StoreError::GraphNotFound(msg) => AppError::NotFound(msg),
            crate::store::engine::StoreError::Evaluation(e) => {
                AppError::BadRequest(format!("Query evaluation error: {e}"))
            }
            crate::store::engine::StoreError::SparqlSyntax(e) => {
                AppError::BadRequest(format!("SPARQL syntax error: {}", e))
            }
            other => AppError::Internal(other.to_string()),
        }
    }
}
