//! What a principal may read, for the Studio's read-side checks. The shapes
//! catalogue shows a graph's shape IRIs, labels, target classes and paths; a
//! pipeline run's report carries its data graphs' focus nodes and values. Both
//! are data from those graphs, so both follow the rule a `/sparql` query is
//! scoped to ([`crate::auth::acl::readable_graph_iris`]): the graphs of
//! datasets the principal can access (a private graph only for its dataset's
//! writers) plus graph-ACL read grants. Admins read every graph.
//!
//! A Library shape graph is also readable by whoever the Library shows its
//! entry to ([`can_access_set`]): `GET /api/shacl/shape-graphs/{id}/turtle`
//! already serves it to them.

use std::collections::HashSet;

use crate::auth::db::AuthDb;
use crate::auth::middleware::AuthenticatedUser;
use crate::auth::models::Dataset;

use super::access::can_access_set;
use super::models::{ShapeGraph, TargetKind, ValidationPipeline};
use super::store::ShaclStudioStore;

/// A principal's read authority, resolved once per request (or scheduled run).
pub struct ReadScope {
    user_id: Option<String>,
    admin: bool,
    orgs: Vec<String>,
    graphs: HashSet<String>,
}

impl ReadScope {
    /// The signed-in caller of a request.
    pub fn for_user(auth_db: &AuthDb, user: &AuthenticatedUser) -> anyhow::Result<Self> {
        Self::resolve(auth_db, &user.user_id, user.role.as_str(), user.is_admin())
    }

    /// A pipeline's creator, for a run nobody is present at (the scheduler).
    /// A creator who is no longer an active user reads only what anyone may:
    /// public datasets, public grants, public Library entries.
    pub fn for_creator(auth_db: &AuthDb, created_by: Option<&str>) -> anyhow::Result<Self> {
        let user = match created_by {
            Some(id) => auth_db.get_user_by_id(id)?.filter(|u| u.is_active),
            None => None,
        };
        match user {
            Some(u) => Self::resolve(auth_db, &u.id, u.role.as_str(), u.is_admin()),
            None => Ok(Self {
                user_id: None,
                admin: false,
                orgs: Vec::new(),
                graphs: crate::auth::acl::readable_graph_iris(auth_db, None)?,
            }),
        }
    }

    fn resolve(auth_db: &AuthDb, user_id: &str, role: &str, admin: bool) -> anyhow::Result<Self> {
        let graphs = if admin {
            HashSet::new()
        } else {
            crate::auth::acl::readable_graph_iris(auth_db, Some((user_id, role)))?
        };
        Ok(Self {
            user_id: Some(user_id.to_string()),
            admin,
            orgs: auth_db.get_user_org_ids(user_id).unwrap_or_default(),
            graphs,
        })
    }

    /// Whether the principal may read `graph_iri` by the `/sparql` rule. A
    /// Library shape graph's own rule is [`Self::may_read_set`].
    pub fn may_read_graph(&self, graph_iri: &str) -> bool {
        self.admin || self.graphs.contains(graph_iri)
    }

    /// Whether the principal may read a Library shape graph.
    pub fn may_read_set(&self, set: &ShapeGraph) -> bool {
        self.admin || can_access_set(set, self.user_id.as_deref(), &self.orgs)
    }

    /// Whether the principal may access `dataset`.
    pub fn may_read_dataset(&self, auth_db: &AuthDb, dataset: &Dataset) -> anyhow::Result<bool> {
        Ok(self.admin || auth_db.can_access_dataset(self.user_id.as_deref(), dataset)?)
    }
}

/// The first part of `pipeline`'s scope that `reader` may not read, named for
/// a refusal; `None` when they may read all of it. The scope is every dataset
/// it names (legacy `dataset_ids` and dataset targets), every data graph it
/// resolves to ([`super::exec::resolve_data_graphs`]: named graphs, the
/// graphs of those datasets, shape graphs validated as data) and every shape
/// graph it composes (`shape_graph_ids`). A dataset or shape graph that no
/// longer exists is no part of it: it contributes nothing to a run.
///
/// Shapes bound to a target in the validation layer are not checked: they
/// come with the target, whoever validates it.
pub fn pipeline_unreadable(
    auth_db: &AuthDb,
    studio: &ShaclStudioStore,
    pipeline: &ValidationPipeline,
    reader: &ReadScope,
) -> anyhow::Result<Option<String>> {
    let dataset_ids = pipeline.dataset_ids.iter().chain(
        pipeline
            .targets
            .iter()
            .filter(|t| t.kind == TargetKind::Dataset)
            .map(|t| &t.id),
    );
    for id in dataset_ids {
        if let Some(ds) = auth_db.get_dataset(id)? {
            if !reader.may_read_dataset(auth_db, &ds)? {
                return Ok(Some(format!("dataset '{id}'")));
            }
        }
    }

    let set_ids = pipeline.shape_graph_ids.iter().chain(
        pipeline
            .targets
            .iter()
            .filter(|t| t.kind == TargetKind::ShapeGraph)
            .map(|t| &t.id),
    );
    for id in set_ids {
        if let Some(set) = studio.get_shape_graph(id)? {
            if !reader.may_read_set(&set) {
                return Ok(Some(format!("shape graph '{id}'")));
            }
        }
    }

    for g in super::exec::resolve_data_graphs(auth_db, studio, pipeline) {
        if reader.may_read_graph(&g) {
            continue;
        }
        let library_readable = studio
            .get_shape_graph_by_iri(&g)?
            .is_some_and(|set| reader.may_read_set(&set));
        if !library_readable {
            return Ok(Some(format!("graph <{g}>")));
        }
    }
    Ok(None)
}
