//! REST handlers for managing endpoint ACL rules, graph ACL rules, and
//! triple security labels.  All routes require admin privileges.
//!
//! Every mutating action here changes access policy, so each success is written
//! to the append-only audit trail (`AclGranted` / `AclRevoked` /
//! `EndpointAclChanged` / `TripleLabelChanged`) — granting another principal
//! access to a private graph, or locking a user out with a deny rule, must be
//! reconstructable after the fact.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    Extension, Json,
};
use serde::Deserialize;
use uuid::Uuid;

use super::audit::{AuditEventBuilder, AuditEventType, AuditOutcome};
use super::middleware::AuthenticatedUser;
use crate::server::AppState;

// ─── Endpoint ACL ─────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct CreateEndpointAclRule {
    pub principal_type: String,
    pub principal_id: String,
    pub path_pattern: String,
    pub http_methods: Option<String>,
    pub effect: String,
    pub priority: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateEndpointAclRule {
    pub path_pattern: String,
    pub http_methods: Option<String>,
    pub effect: String,
    pub priority: Option<i64>,
}

/// GET /api/admin/acl/endpoints
pub async fn list_endpoint_acl_rules(
    State(state): State<AppState>,
    Extension(_user): Extension<AuthenticatedUser>,
) -> impl IntoResponse {
    match state.auth_db.list_endpoint_acl_rules() {
        Ok(rules) => Json(rules).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("{{\"error\":\"{e}\"}}"),
        )
            .into_response(),
    }
}

/// POST /api/admin/acl/endpoints
pub async fn create_endpoint_acl_rule(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Json(body): Json<CreateEndpointAclRule>,
) -> impl IntoResponse {
    let id = Uuid::new_v4().to_string();
    let methods = body.http_methods.as_deref().unwrap_or("*");
    let priority = body.priority.unwrap_or(0);
    match state.auth_db.create_endpoint_acl_rule(
        &id,
        &body.principal_type,
        &body.principal_id,
        &body.path_pattern,
        methods,
        &body.effect,
        priority,
        &user.user_id,
    ) {
        Ok(rule) => {
            state.audit.log(
                AuditEventBuilder::new(AuditEventType::EndpointAclChanged, AuditOutcome::Success)
                    .actor_id(user.user_id.clone())
                    .resource("endpoint_acl", id.clone())
                    .action("create")
                    .details(serde_json::json!({
                        "principal_type": body.principal_type.clone(),
                        "principal_id": body.principal_id.clone(),
                        "path_pattern": body.path_pattern.clone(),
                        "http_methods": methods,
                        "effect": body.effect.clone(),
                    })),
            );
            (StatusCode::CREATED, Json(rule)).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("{{\"error\":\"{e}\"}}"),
        )
            .into_response(),
    }
}

/// PUT /api/admin/acl/endpoints/:id
pub async fn update_endpoint_acl_rule(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(id): Path<String>,
    Json(body): Json<UpdateEndpointAclRule>,
) -> impl IntoResponse {
    let methods = body.http_methods.as_deref().unwrap_or("*");
    let priority = body.priority.unwrap_or(0);
    match state.auth_db.update_endpoint_acl_rule(
        &id,
        &body.path_pattern,
        methods,
        &body.effect,
        priority,
    ) {
        Ok(()) => {
            state.audit.log(
                AuditEventBuilder::new(AuditEventType::EndpointAclChanged, AuditOutcome::Success)
                    .actor_id(user.user_id.clone())
                    .resource("endpoint_acl", id.clone())
                    .action("update")
                    .details(serde_json::json!({
                        "path_pattern": body.path_pattern.clone(),
                        "http_methods": methods,
                        "effect": body.effect.clone(),
                    })),
            );
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("{{\"error\":\"{e}\"}}"),
        )
            .into_response(),
    }
}

/// DELETE /api/admin/acl/endpoints/:id
pub async fn delete_endpoint_acl_rule(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.auth_db.delete_endpoint_acl_rule(&id) {
        Ok(()) => {
            state.audit.log(
                AuditEventBuilder::new(AuditEventType::EndpointAclChanged, AuditOutcome::Success)
                    .actor_id(user.user_id.clone())
                    .resource("endpoint_acl", id.clone())
                    .action("delete"),
            );
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("{{\"error\":\"{e}\"}}"),
        )
            .into_response(),
    }
}

// ─── Graph ACL ────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct GraphAclFilter {
    pub graph_iri: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct GrantGraphPermission {
    pub graph_iri: String,
    pub principal_type: String,
    pub principal_id: String,
    pub permission: String,
}

/// GET /api/admin/acl/graphs?graph_iri=...
pub async fn list_graph_acl_rules(
    State(state): State<AppState>,
    Extension(_user): Extension<AuthenticatedUser>,
    Query(filter): Query<GraphAclFilter>,
) -> impl IntoResponse {
    match state
        .auth_db
        .list_graph_acl_rules(filter.graph_iri.as_deref())
    {
        Ok(rules) => Json(rules).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("{{\"error\":\"{e}\"}}"),
        )
            .into_response(),
    }
}

/// POST /api/admin/acl/graphs
pub async fn grant_graph_permission(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Json(body): Json<GrantGraphPermission>,
) -> impl IntoResponse {
    let id = Uuid::new_v4().to_string();
    match state.auth_db.grant_graph_permission(
        &id,
        &body.graph_iri,
        &body.principal_type,
        &body.principal_id,
        &body.permission,
        &user.user_id,
    ) {
        Ok(rule) => {
            state.audit.log(
                AuditEventBuilder::new(AuditEventType::AclGranted, AuditOutcome::Success)
                    .actor_id(user.user_id.clone())
                    .resource("graph_acl", id.clone())
                    .action("grant")
                    .details(serde_json::json!({
                        "graph_iri": body.graph_iri.clone(),
                        "principal_type": body.principal_type.clone(),
                        "principal_id": body.principal_id.clone(),
                        "permission": body.permission.clone(),
                    })),
            );
            (StatusCode::CREATED, Json(rule)).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("{{\"error\":\"{e}\"}}"),
        )
            .into_response(),
    }
}

/// DELETE /api/admin/acl/graphs/:id
pub async fn revoke_graph_permission(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.auth_db.revoke_graph_permission(&id) {
        Ok(()) => {
            state.audit.log(
                AuditEventBuilder::new(AuditEventType::AclRevoked, AuditOutcome::Success)
                    .actor_id(user.user_id.clone())
                    .resource("graph_acl", id.clone())
                    .action("revoke"),
            );
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("{{\"error\":\"{e}\"}}"),
        )
            .into_response(),
    }
}

// ─── Triple Security Labels ───────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct TripleLabelFilter {
    pub graph_iri: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CreateTripleLabel {
    pub subject_iri: String,
    pub predicate_iri: String,
    pub object_value: String,
    pub graph_iri: String,
    pub label_graph_iri: String,
}

/// GET /api/admin/acl/triples?graph_iri=...
pub async fn list_triple_security_labels(
    State(state): State<AppState>,
    Extension(_user): Extension<AuthenticatedUser>,
    Query(filter): Query<TripleLabelFilter>,
) -> impl IntoResponse {
    match state
        .auth_db
        .list_triple_security_labels(filter.graph_iri.as_deref())
    {
        Ok(labels) => Json(labels).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("{{\"error\":\"{e}\"}}"),
        )
            .into_response(),
    }
}

/// POST /api/admin/acl/triples
pub async fn create_triple_security_label(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Json(body): Json<CreateTripleLabel>,
) -> impl IntoResponse {
    let id = Uuid::new_v4().to_string();
    match state.auth_db.create_triple_security_label(
        &id,
        &body.subject_iri,
        &body.predicate_iri,
        &body.object_value,
        &body.graph_iri,
        &body.label_graph_iri,
    ) {
        Ok(label) => {
            state.audit.log(
                AuditEventBuilder::new(AuditEventType::TripleLabelChanged, AuditOutcome::Success)
                    .actor_id(user.user_id.clone())
                    .resource("triple_label", id.clone())
                    .action("create")
                    .details(serde_json::json!({
                        "graph_iri": body.graph_iri.clone(),
                        "subject_iri": body.subject_iri.clone(),
                        "predicate_iri": body.predicate_iri.clone(),
                        "label_graph_iri": body.label_graph_iri.clone(),
                    })),
            );
            (StatusCode::CREATED, Json(label)).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("{{\"error\":\"{e}\"}}"),
        )
            .into_response(),
    }
}

/// DELETE /api/admin/acl/triples/:id
pub async fn delete_triple_security_label(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.auth_db.delete_triple_security_label(&id) {
        Ok(()) => {
            state.audit.log(
                AuditEventBuilder::new(AuditEventType::TripleLabelChanged, AuditOutcome::Success)
                    .actor_id(user.user_id.clone())
                    .resource("triple_label", id.clone())
                    .action("delete"),
            );
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("{{\"error\":\"{e}\"}}"),
        )
            .into_response(),
    }
}
