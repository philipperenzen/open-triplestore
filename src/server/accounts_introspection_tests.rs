//! Introspection surface for the account platform (Unified Accounts):
//! memberships in GET /api/auth/me and the per-dataset permission probe
//! GET /api/datasets/:id/permissions/me that resource servers (validation
//! platform, form service) authorize against.

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{header, Method, Request, StatusCode};
    use http_body_util::BodyExt as _;
    use tower::ServiceExt as _;

    use crate::auth::jwt::issue_access_token;
    use crate::auth::models::{GraphKind, OwnerType, Role, SystemRole, Visibility};
    use crate::auth::password;
    use crate::server::{build_router, AppState};

    fn state() -> AppState {
        AppState::test_default_with_store(crate::store::TripleStore::in_memory().unwrap())
    }

    async fn get_json(
        state: &AppState,
        uri: &str,
        bearer: Option<&str>,
    ) -> (StatusCode, serde_json::Value) {
        let mut builder = Request::builder().method(Method::GET).uri(uri);
        if let Some(token) = bearer {
            builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        let resp = build_router(state.clone(), "", vec![])
            .oneshot(builder.body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = resp.status();
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json = serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);
        (status, json)
    }

    fn seed_user(state: &AppState, name: &str, role: SystemRole) -> (String, String) {
        let id = uuid::Uuid::new_v4().to_string();
        let hash = password::hash_password("s3cret-password-123").unwrap();
        state
            .auth_db
            .create_user(&id, name, &format!("{name}@example.org"), &hash, role)
            .unwrap();
        let token = issue_access_token(&state.jwt_config, &id, name, role.as_str()).unwrap();
        (id, token)
    }

    #[tokio::test]
    async fn me_includes_org_and_group_memberships() {
        let st = state();
        let (uid, tok) = seed_user(&st, "carol", SystemRole::User);
        let org = st
            .auth_db
            .create_organisation("org-1", "Org A", "org-a", None, None)
            .unwrap();
        st.auth_db
            .add_org_member(&uid, &org.id, Role::Member)
            .unwrap();
        let group = st
            .auth_db
            .create_group("grp-1", &org.id, "Team 1", None)
            .unwrap();
        st.auth_db
            .add_group_member(&uid, &group.id, Role::Member)
            .unwrap();

        let (s, body) = get_json(&st, "/api/auth/me", Some(&tok)).await;
        assert_eq!(s, StatusCode::OK, "{body}");
        let orgs = body["organisations"]
            .as_array()
            .expect("organisations array");
        assert_eq!(orgs.len(), 1, "{body}");
        assert_eq!(orgs[0]["slug"], "org-a");
        assert_eq!(orgs[0]["role"], "member");
        let groups = body["groups"].as_array().expect("groups array");
        assert_eq!(groups.len(), 1, "{body}");
        assert_eq!(groups[0]["org_slug"], "org-a");
        assert_eq!(groups[0]["name"], "Team 1");
        assert_eq!(groups[0]["id"], "grp-1");
    }

    #[tokio::test]
    async fn me_without_memberships_has_empty_arrays() {
        let st = state();
        let (_uid, tok) = seed_user(&st, "solo", SystemRole::User);
        let (s, body) = get_json(&st, "/api/auth/me", Some(&tok)).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(body["organisations"], serde_json::json!([]));
        assert_eq!(body["groups"], serde_json::json!([]));
    }

    #[tokio::test]
    async fn permissions_me_reflects_effective_access() {
        let st = state();
        let (owner_id, owner_tok) = seed_user(&st, "owner", SystemRole::User);
        let (_other_id, other_tok) = seed_user(&st, "other", SystemRole::User);
        let (_admin_id, admin_tok) = seed_user(&st, "root", SystemRole::Admin);

        let private = st
            .auth_db
            .create_dataset(
                "ds-private",
                "Private data",
                None,
                OwnerType::User,
                &owner_id,
                Visibility::Private,
                Some(GraphKind::Instances),
            )
            .unwrap();
        let public = st
            .auth_db
            .create_dataset(
                "ds-public",
                "Public data",
                None,
                OwnerType::User,
                &owner_id,
                Visibility::Public,
                Some(GraphKind::Instances),
            )
            .unwrap();

        // Owner: full control of their private dataset.
        let (s, body) = get_json(
            &st,
            &format!("/api/datasets/{}/permissions/me", private.id),
            Some(&owner_tok),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{body}");
        assert_eq!(body["read"], true);
        assert_eq!(body["write"], true);
        assert_eq!(body["manage"], true);

        // Unrelated user: the private dataset is indistinguishable from absent.
        let (s, _) = get_json(
            &st,
            &format!("/api/datasets/{}/permissions/me", private.id),
            Some(&other_tok),
        )
        .await;
        assert_eq!(s, StatusCode::NOT_FOUND);

        // System admin bypass: full access without any grant.
        let (s, body) = get_json(
            &st,
            &format!("/api/datasets/{}/permissions/me", private.id),
            Some(&admin_tok),
        )
        .await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(body["manage"], true);

        // Public dataset, anonymous: readable, never writable.
        let (s, body) = get_json(
            &st,
            &format!("/api/datasets/{}/permissions/me", public.id),
            None,
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{body}");
        assert_eq!(body["read"], true);
        assert_eq!(body["write"], false);

        // Unknown id: plain 404.
        let (s, _) = get_json(&st, "/api/datasets/nope/permissions/me", None).await;
        assert_eq!(s, StatusCode::NOT_FOUND);
    }
}
