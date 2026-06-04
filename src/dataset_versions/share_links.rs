//! OTS-minted scoped share links (Unified Accounts plan, Phase 6).
//!
//! Replaces ad-hoc client-side form "login codes" with a server-verified,
//! expiring, revocable token scoped to a dataset (+ optional graph) and a
//! permission. Mint requires dataset write access; redeem is public (that is the
//! point of a share link) and returns the link's scope so the client can gate
//! access. Only the SHA-256 of the token is stored.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use chrono::{Duration, Utc};
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use crate::auth::jwt::hash_token;
use crate::auth::middleware::AuthenticatedUser;
use crate::auth::models::ShareLink;
use crate::server::error::AppError;
use crate::server::AppState;

#[derive(Deserialize)]
pub struct MintRequest {
    #[serde(default)]
    pub permission: Option<String>,
    #[serde(default)]
    pub graph: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub ttl_hours: Option<i64>,
}

/// POST /api/datasets/:dataset_id/share-links — mint a scoped share link.
pub async fn mint_share_link(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(dataset_id): Path<String>,
    Json(body): Json<MintRequest>,
) -> Result<impl IntoResponse, AppError> {
    let ds = state
        .auth_db
        .get_dataset(&dataset_id)
        .map_err(|e| AppError::Internal(e.to_string()))?
        .ok_or_else(|| AppError::NotFound(format!("Dataset '{dataset_id}' not found")))?;
    if !state
        .auth_db
        .can_write_dataset(&user.user_id, &ds)
        .map_err(|e| AppError::Internal(e.to_string()))?
    {
        return Err(AppError::Unauthorized("Write access to this dataset required".to_string()));
    }

    let permission = match body.permission.as_deref() {
        None | Some("read") => "read".to_string(),
        Some("submit") => "submit".to_string(),
        Some(other) => return Err(AppError::BadRequest(format!("invalid permission '{other}'"))),
    };

    // Opaque 64-hex token; only its hash is stored.
    let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    let expires_at = body
        .ttl_hours
        .filter(|h| *h > 0)
        .map(|h| (Utc::now() + Duration::hours(h)).to_rfc3339());

    let link = ShareLink {
        id: Uuid::new_v4().to_string(),
        token_hash: hash_token(&token),
        dataset_id: dataset_id.clone(),
        graph: body.graph.clone().filter(|s| !s.is_empty()),
        permission,
        label: body.label.clone(),
        created_by: Some(user.user_id.clone()),
        expires_at: expires_at.clone(),
        revoked: false,
        created_at: Utc::now().to_rfc3339(),
    };
    state
        .auth_db
        .insert_share_link(&link)
        .map_err(|e| AppError::Internal(e.to_string()))?;

    Ok((
        StatusCode::CREATED,
        Json(json!({
            "token": token,
            "dataset_id": dataset_id,
            "permission": link.permission,
            "expires_at": expires_at,
        })),
    ))
}

#[derive(Deserialize)]
pub struct RedeemRequest {
    pub token: String,
}

/// POST /api/share-links/redeem — validate a share token and return its scope.
pub async fn redeem_share_link(
    State(state): State<AppState>,
    Json(body): Json<RedeemRequest>,
) -> Result<impl IntoResponse, AppError> {
    let hash = hash_token(body.token.trim());
    let link = state
        .auth_db
        .get_share_link_by_token_hash(&hash)
        .map_err(|e| AppError::Internal(e.to_string()))?
        .ok_or_else(|| AppError::Unauthorized("Invalid share link".to_string()))?;

    if link.revoked {
        return Err(AppError::Unauthorized("Share link revoked".to_string()));
    }
    if let Some(exp) = &link.expires_at {
        if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(exp) {
            if Utc::now() > dt.with_timezone(&Utc) {
                return Err(AppError::Unauthorized("Share link expired".to_string()));
            }
        }
    }

    Ok(Json(json!({
        "valid": true,
        "dataset_id": link.dataset_id,
        "graph": link.graph,
        "permission": link.permission,
        "label": link.label,
        "expires_at": link.expires_at,
    })))
}

#[cfg(test)]
mod tests {
    use crate::auth::db::AuthDb;
    use crate::auth::jwt::hash_token;
    use crate::auth::models::{OwnerType, ShareLink, Visibility};
    use std::sync::Arc;

    #[test]
    fn share_link_store_and_lookup_by_hash() {
        let db = Arc::new(AuthDb::in_memory().unwrap());
        db.create_dataset("ds1", "DS", None, OwnerType::User, "u1", Visibility::Private, None)
            .unwrap();

        let token = "share-token-abc";
        let link = ShareLink {
            id: "s1".to_string(),
            token_hash: hash_token(token),
            dataset_id: "ds1".to_string(),
            graph: None,
            permission: "read".to_string(),
            label: Some("Public form".to_string()),
            created_by: Some("u1".to_string()),
            expires_at: None,
            revoked: false,
            created_at: "2026-01-01T00:00:00Z".to_string(),
        };
        db.insert_share_link(&link).unwrap();

        let got = db.get_share_link_by_token_hash(&hash_token(token)).unwrap().unwrap();
        assert_eq!(got.dataset_id, "ds1");
        assert_eq!(got.permission, "read");
        // The raw token is never stored — only its hash matches.
        assert!(db.get_share_link_by_token_hash("not-a-hash").unwrap().is_none());
        assert!(db.get_share_link_by_token_hash(&hash_token("wrong")).unwrap().is_none());
    }
}
