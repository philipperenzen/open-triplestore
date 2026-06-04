//! Visibility / ownership checks for shape graphs and pipelines. Deliberately
//! small and self-contained: public is world-readable; otherwise the principal
//! must own the artifact or belong to the owning organisation. `members`
//! visibility is readable by any authenticated user (mirroring the dataset
//! "members" tier). System admins bypass via the `is_admin` argument to the
//! manage check.

use crate::auth::models::{OwnerType, Visibility};

use super::models::{ShapeGraph, ValidationPipeline};

fn is_owner_or_org_member(
    owner_type: OwnerType,
    owner_id: &str,
    user_id: Option<&str>,
    org_ids: &[String],
) -> bool {
    match (user_id, owner_type) {
        (Some(uid), OwnerType::User) => owner_id == uid,
        (Some(_), OwnerType::Organisation) | (Some(_), OwnerType::Group) => {
            org_ids.iter().any(|o| o == owner_id)
        }
        _ => false,
    }
}

fn accessible(
    owner_type: OwnerType,
    owner_id: &str,
    visibility: Visibility,
    user_id: Option<&str>,
    org_ids: &[String],
) -> bool {
    match visibility {
        Visibility::Public => true,
        Visibility::Members => user_id.is_some(),
        Visibility::Private => is_owner_or_org_member(owner_type, owner_id, user_id, org_ids),
    }
}

fn manageable(
    owner_type: OwnerType,
    owner_id: &str,
    user_id: Option<&str>,
    org_ids: &[String],
    is_admin: bool,
) -> bool {
    is_admin || is_owner_or_org_member(owner_type, owner_id, user_id, org_ids)
}

pub fn can_access_set(s: &ShapeGraph, user_id: Option<&str>, org_ids: &[String]) -> bool {
    accessible(s.owner_type, &s.owner_id, s.visibility, user_id, org_ids)
}

pub fn can_manage_set(s: &ShapeGraph, user_id: Option<&str>, org_ids: &[String], is_admin: bool) -> bool {
    manageable(s.owner_type, &s.owner_id, user_id, org_ids, is_admin)
}

pub fn can_access_pipeline(p: &ValidationPipeline, user_id: Option<&str>, org_ids: &[String]) -> bool {
    accessible(p.owner_type, &p.owner_id, p.visibility, user_id, org_ids)
}

pub fn can_manage_pipeline(p: &ValidationPipeline, user_id: Option<&str>, org_ids: &[String], is_admin: bool) -> bool {
    manageable(p.owner_type, &p.owner_id, user_id, org_ids, is_admin)
}
