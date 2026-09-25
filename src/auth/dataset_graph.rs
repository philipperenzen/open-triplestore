use crate::auth::middleware::AuthenticatedUser;
use crate::auth::models::{Dataset, DatasetGraphEntry, GraphKind};
use crate::store::engine::TripleStore;
use crate::store::escape_sparql_iri;
use oxigraph::io::RdfFormat;

/// Named graph IRI for a dataset's DCAT metadata.
pub fn dataset_metadata_graph_iri(dataset_id: &str) -> String {
    format!("urn:system:metadata:dataset:{}", dataset_id)
}

/// Canonical dataset IRI per the styleguide (§3.3): `{base}/dataset/{id}`
/// (singular). This is the single source of truth for the dataset's own IRI and
/// the prefix of its owned graph namespace (`{base}/dataset/{id}/...`). It MUST
/// match the catalogue, version registry and API-service registry; the bulk
/// import write boundary also keys off this prefix, so any divergence would let
/// a caller's "namespaced" target graph fall outside the gate.
pub fn dataset_iri(base_url: &str, dataset_id: &str) -> String {
    format!("{}/dataset/{}", base_url.trim_end_matches('/'), dataset_id)
}

/// Named graph IRI for a dataset's "default graph": where DEFAULT-graph (and
/// blank-node-graph) triples from a dataset-scoped quad import are routed so they
/// fall under the per-graph write boundary instead of the shared global default
/// graph. Lives under the dataset's owned namespace, so the bulk-import authorize
/// gate admits it (`g.starts_with("{base}/dataset/{id}/")`). `default` is neither
/// an auto-split role suffix nor a `urn:system:` graph, so it cannot collide.
pub fn dataset_default_graph_iri(base_url: &str, dataset_id: &str) -> String {
    format!("{}/default", dataset_iri(base_url, dataset_id))
}

/// True iff `graph_iri` lies inside `dataset_id`'s own reserved graph namespace —
/// either the canonical HTTP prefix `{base}/dataset/{id}/` or the
/// `urn:dataset:{id}:` URN prefix (shapes, RML mappings/output, the namespaced
/// default graph). Both prefixes carry a trailing delimiter so one dataset id can
/// never prefix-match another (`d1` vs `d12`).
pub fn dataset_owns_graph(base_url: &str, dataset_id: &str, graph_iri: &str) -> bool {
    let http_ns = format!("{}/", dataset_iri(base_url, dataset_id));
    let urn_ns = format!("urn:dataset:{dataset_id}:");
    graph_iri.starts_with(&http_ns) || graph_iri.starts_with(&urn_ns)
}

/// Named graph IRI where a dataset's SHACL-AF inference materialises when the
/// run spans several data graphs and no single one of them is "the" graph.
/// Inside the dataset's own reserved namespace, so it can never be another
/// tenant's graph and [`dataset_holds_graph`] admits it.
pub fn dataset_inference_graph_iri(dataset_id: &str) -> String {
    format!("urn:dataset:{dataset_id}:inferred")
}

/// True iff `graph_iri` lies in the model registry's own graph namespace,
/// `{base}/data-model/`: every version graph (`{base}/data-model/{id}/version/{v}`),
/// its sub-graphs and the copies the seeder keeps aside (`…/version/{v}-kept-{n}`)
/// live under it. The registry builds these names as `{base_url}/data-model/…`
/// without trimming, so both spellings of a base URL with a trailing slash count.
fn in_model_registry_namespace(base_url: &str, graph_iri: &str) -> bool {
    graph_iri.starts_with(&format!("{}/data-model/", base_url.trim_end_matches('/')))
        || graph_iri.starts_with(&format!("{base_url}/data-model/"))
}

/// True iff `graph_iri` belongs to the model registry: the registry graph
/// itself, any graph under `{base}/data-model/`, or a graph a version record
/// names as its base graph or a sub-graph (seed bundles and LOV installs keep
/// their content in graphs named after the vocabulary, not the registry).
///
/// Such graphs are managed only through the data-model API, which enforces
/// their owners and licences (no altered copy of a no-derivatives work). A
/// dataset may not claim, write or delete them. Fails closed: a registry
/// lookup that errors, or a name that is not a valid IRI, counts as held.
pub fn graph_held_by_model_registry(store: &TripleStore, base_url: &str, graph_iri: &str) -> bool {
    graph_iri == crate::data_models::registry::REGISTRY_GRAPH
        || in_model_registry_namespace(base_url, graph_iri)
        || crate::data_models::registry::graph_held_by_version(store, graph_iri)
}

/// True iff `graph_iri` is one of the graphs the server keeps for `dataset_id`
/// outside its namespace: its DCAT metadata and validation-report graphs, its
/// assets graph (`{base}/datasets/{id}/assets`, plural), its property-states
/// graph and its entailment graphs (`urn:entailment:{regime}:{id}`).
pub fn dataset_well_known_graph(base_url: &str, dataset_id: &str, graph_iri: &str) -> bool {
    graph_iri == dataset_metadata_graph_iri(dataset_id)
        || graph_iri == dataset_reports_graph_iri(dataset_id)
        || graph_iri == crate::server::routes::assets_graph_iri(base_url, dataset_id)
        || graph_iri
            == crate::server::routes::assets_graph_iri(base_url.trim_end_matches('/'), dataset_id)
        || graph_iri == crate::property_states::states_graph(dataset_id)
        || graph_iri
            .strip_prefix("urn:entailment:")
            .and_then(|rest| rest.split_once(':'))
            .is_some_and(|(_, id)| id == dataset_id)
}

/// The id of the dataset whose namespace (`{base}/dataset/{id}/…` or
/// `urn:dataset:{id}:…`) holds `graph_iri`, if any.
pub fn namespace_dataset_id<'a>(base_url: &str, graph_iri: &'a str) -> Option<&'a str> {
    let http_root = format!("{}/dataset/", base_url.trim_end_matches('/'));
    let id = if let Some(rest) = graph_iri.strip_prefix(&http_root) {
        rest.split_once('/').map(|(id, _)| id)
    } else if let Some(rest) = graph_iri.strip_prefix("urn:dataset:") {
        rest.split_once(':').map(|(id, _)| id)
    } else {
        None
    };
    id.filter(|id| !id.is_empty())
}

/// Prefixes of graphs the server names for its own features — the SHACL
/// Studio Library's shape graphs, datasources, their mappings, runs and dry
/// runs, its operational `urn:ots:` graphs and its `urn:config:` settings
/// (the mapping gates). Those features attach them to a dataset themselves; a
/// caller naming one never may.
const SERVER_GRAPH_PREFIXES: [&str; 7] = [
    "urn:shapes:",
    "urn:source:",
    "urn:mapping:",
    "urn:run:",
    "urn:dryrun:",
    "urn:ots:",
    "urn:config:",
];

/// True iff `graph_iri` is in a *reserved* namespace: owned by another
/// dataset (`urn:dataset:{other}:*`, `{base}/dataset/{other}/*`, and its
/// assets, property-states and entailment graphs), the system
/// (`urn:system:*`), the model registry (`{base}/data-model/*`) or a server
/// feature ([`SERVER_GRAPH_PREFIXES`]). A non-admin may never register or
/// write such a graph for `dataset_id`; the dataset's own well-known graphs
/// ([`dataset_well_known_graph`]) are not reserved for it.
pub fn graph_in_foreign_reserved_namespace(
    base_url: &str,
    dataset_id: &str,
    graph_iri: &str,
) -> bool {
    if graph_iri.starts_with("urn:system:") {
        return true;
    }
    if in_model_registry_namespace(base_url, graph_iri) {
        return true;
    }
    if dataset_well_known_graph(base_url, dataset_id, graph_iri) {
        return false;
    }
    if SERVER_GRAPH_PREFIXES
        .iter()
        .any(|p| graph_iri.starts_with(p))
        || graph_iri.starts_with("urn:entailment:")
    {
        return true;
    }
    let assets_root = format!("{}/datasets/", base_url.trim_end_matches('/'));
    if graph_iri.starts_with(&assets_root)
        || graph_iri.starts_with(&format!("{base_url}/datasets/"))
    {
        return true;
    }
    if let Some(rest) = graph_iri.strip_prefix("urn:dataset:") {
        return match rest.split_once(':') {
            Some((other, _)) => other != dataset_id,
            None => true, // malformed `urn:dataset:` with no id segment → reserved
        };
    }
    let datasets_root = format!("{}/dataset/", base_url.trim_end_matches('/'));
    if let Some(rest) = graph_iri.strip_prefix(&datasets_root) {
        let other = rest.split('/').next().unwrap_or("");
        return other != dataset_id;
    }
    false
}

/// Whether the store holds any triple in `graph_iri`. Fails closed: a store
/// error, or a name that is not an IRI, counts as holding data.
pub fn graph_holds_data(store: &TripleStore, graph_iri: &str) -> bool {
    use oxigraph::model::{GraphNameRef, NamedNodeRef};
    let Ok(g) = NamedNodeRef::new(graph_iri) else {
        return true;
    };
    store
        .store()
        .quads_for_pattern(None, None, None, Some(GraphNameRef::NamedNode(g)))
        .next()
        .is_some()
}

/// Whether the graph ACL names `graph_iri`: someone was granted access to it
/// (`POST /api/admin/acl/graphs`), so it is managed there, even while it holds
/// no data. Fails closed: a lookup error counts as named.
pub fn graph_named_in_acl(db: &crate::auth::db::AuthDb, graph_iri: &str) -> bool {
    db.list_graph_acl_rules(Some(graph_iri))
        .map(|rules| !rules.is_empty())
        .unwrap_or(true)
}

/// Whether `user` may write `graph_iri` directly, outside any dataset: an
/// admin, or a graph-ACL write grant — what a Graph Store or SPARQL UPDATE
/// write of it requires. A dataset role never counts: it is authority over
/// the dataset's own graphs, not over a graph someone else made.
pub fn may_write_graph_directly(
    db: &crate::auth::db::AuthDb,
    user: &AuthenticatedUser,
    graph_iri: &str,
) -> bool {
    user.is_admin()
        || db
            .check_graph_permission(&user.user_id, user.role.as_str(), graph_iri, "write")
            .unwrap_or(false)
}

/// Whether `dataset_id` holds `graph_iri` as one of its own graphs: inside its
/// namespace, one of its well-known graphs, or registered to it. Every path
/// that writes on the dataset's authority writes only such graphs, and a
/// graph reaches the registry of a dataset only through
/// [`gate_dataset_graph_target`]. A lookup error counts as not held.
pub fn dataset_holds_graph(
    db: &crate::auth::db::AuthDb,
    base_url: &str,
    dataset_id: &str,
    graph_iri: &str,
) -> bool {
    dataset_owns_graph(base_url, dataset_id, graph_iri)
        || dataset_well_known_graph(base_url, dataset_id, graph_iri)
        || db.dataset_has_graph(dataset_id, graph_iri).unwrap_or(false)
}

/// The ids of the datasets that hold `graph_iri` ([`dataset_holds_graph`]):
/// the ones it is registered to, and the one whose namespace it is in (when
/// that dataset exists). A lookup error yields none.
pub fn datasets_holding_graph(
    db: &crate::auth::db::AuthDb,
    base_url: &str,
    graph_iri: &str,
) -> Vec<String> {
    let mut ids = db.datasets_with_graph(graph_iri).unwrap_or_default();
    if let Some(id) = namespace_dataset_id(base_url, graph_iri) {
        if !ids.iter().any(|i| i == id) && matches!(db.get_dataset(id), Ok(Some(_))) {
            ids.push(id.to_string());
        }
    }
    ids
}

/// Authorize a non-admin caller naming `graph_iri` as a write/registration target
/// for `dataset_id`.
///
/// Closes the cross-tenant graph-claim vector: registration (`POST /datasets/:id/
/// graphs`) and RML output both feed `dataset_graphs`, and `get_accessible_graph_iris`
/// then treats any graph registered to an accessible dataset as readable — so a
/// writer who attached *another tenant's* graph IRI to their own dataset could read
/// (or, via RML, write) it. The rule, mirroring the bulk-import write boundary: the
/// graph must be the dataset's own namespaced graph, or a non-reserved external IRI
/// that no other dataset uses (registered to it, or as its shapes graph).
/// Admins bypass (the caller checks `is_admin`). Returns `Err(message)` on
/// rejection (map to HTTP 403); fails closed on a lookup error.
///
/// This is the namespace boundary only (it has no store, so it can neither see
/// which graphs a model version names nor whether a graph holds data). A path
/// that names a target graph calls [`gate_dataset_graph_target`], which adds
/// both.
pub fn authorize_dataset_graph_target(
    db: &crate::auth::db::AuthDb,
    base_url: &str,
    dataset_id: &str,
    graph_iri: &str,
) -> Result<(), String> {
    // The dataset's own reserved namespace is always allowed.
    if dataset_owns_graph(base_url, dataset_id, graph_iri) {
        return Ok(());
    }
    // Reserved namespaces of other datasets / the system are never claimable.
    if graph_in_foreign_reserved_namespace(base_url, dataset_id, graph_iri) {
        return Err(graph_boundary_error(dataset_id, graph_iri));
    }
    // A non-reserved external graph is allowed only if no OTHER dataset uses it
    // (a lookup error fails closed — treat as foreign).
    if graph_used_by_other_dataset(db, dataset_id, graph_iri) {
        return Err(graph_boundary_error(dataset_id, graph_iri));
    }
    Ok(())
}

fn graph_boundary_error(dataset_id: &str, graph_iri: &str) -> String {
    format!(
        "Target graph <{graph_iri}> is outside dataset '{dataset_id}'. A dataset may only use its \
         own namespaced graphs or an unclaimed external graph — not a graph owned by another \
         dataset, the system, the model registry or a server feature such as the SHACL Studio \
         Library."
    )
}

/// How a dataset holds a graph a caller named for it, as
/// [`gate_dataset_graph_target`] found it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphClaim {
    /// Already registered to the dataset.
    Registered,
    /// Inside the dataset's namespace, one of its well-known graphs, or a
    /// graph that holds no data yet: the dataset makes it
    /// (`origin = 'created'`).
    Created,
    /// A graph that already holds data, which the caller may write directly
    /// (an admin, or a graph-ACL write grant): the dataset takes it over
    /// (`origin = 'adopted'`).
    Adopted,
}

impl GraphClaim {
    /// The `dataset_graphs.origin` a new registration records.
    pub fn origin(self) -> Option<&'static str> {
        match self {
            GraphClaim::Registered => None,
            GraphClaim::Created => Some("created"),
            GraphClaim::Adopted => Some("adopted"),
        }
    }
}

/// Whether a write that `claim` let through should also register
/// `graph_iri` to `dataset_id`, when the dataset did not name the graph
/// itself (a mapping's `rml:graphMap`, a linked shapes graph). For a
/// non-admin, always: the gate applied every rule. An admin's claim skips
/// those rules, and registering would hand the graph to the dataset's
/// editors, so it registers only a graph a non-admin could have claimed as
/// new: empty, outside every reserved namespace, used by no other dataset and
/// named in no graph-ACL rule. Any other graph the admin writes stays theirs;
/// they can attach it explicitly (`POST /api/datasets/{id}/graphs`).
pub fn claim_registers_with_dataset(
    db: &crate::auth::db::AuthDb,
    base_url: &str,
    dataset_id: &str,
    graph_iri: &str,
    claim: GraphClaim,
    is_admin: bool,
) -> bool {
    if !is_admin {
        return true;
    }
    match claim {
        GraphClaim::Registered => true,
        GraphClaim::Adopted => false,
        GraphClaim::Created => {
            authorize_dataset_graph_target(db, base_url, dataset_id, graph_iri).is_ok()
                && !graph_named_in_acl(db, graph_iri)
        }
    }
}

/// Register `graph_iri` to `dataset_id` the way `claim` found it: with its
/// origin when the claim is new, as a plain (idempotent) registration when it
/// was already registered.
pub fn register_claimed_graph(
    db: &crate::auth::db::AuthDb,
    dataset_id: &str,
    graph_iri: &str,
    claim: GraphClaim,
) -> anyhow::Result<()> {
    match claim.origin() {
        Some(origin) => db.add_dataset_graph_with_origin(dataset_id, graph_iri, origin),
        None => db.add_dataset_graph(dataset_id, graph_iri),
    }
}

/// The gate for every path that names `graph_iri` as a graph of `dataset_id`:
/// registering it (`POST /datasets/:id/graphs`), or writing it through the
/// dataset's RML mapping, commit, LDES sync, version restore or shapes
/// upload.
///
/// * For everyone, admins included: the name must be a valid IRI and must not
///   be a model-registry graph ([`graph_held_by_model_registry`]). Registering
///   one would let the dataset's bulk import overwrite it past the registry's
///   licence checks, expose withheld and private versions to the dataset's
///   readers, hide the model from everyone else, and let a later detach or
///   dataset delete wipe it. Models are managed through the data-model API.
/// * For non-admins, in addition, the namespace boundary of
///   [`authorize_dataset_graph_target`], and: a graph that is not the
///   dataset's own and already holds data is taken over only by a caller who
///   may write it directly ([`may_write_graph_directly`]). Everything a
///   dataset holds becomes writable by its editors (bulk import, RML, RDF
///   Patch, the SHACL Studio) and deletable with it, so attaching an admin's
///   or another user's graph would hand out write and delete rights nobody
///   granted.
///
/// Returns how the dataset holds the graph ([`GraphClaim`]; record it with
/// [`register_claimed_graph`]) or `Err(message)` on rejection (map to HTTP
/// 403); fails closed on a registry, dataset or store lookup error.
pub fn gate_dataset_graph_target(
    store: &TripleStore,
    db: &crate::auth::db::AuthDb,
    base_url: &str,
    dataset_id: &str,
    graph_iri: &str,
    user: &AuthenticatedUser,
) -> Result<GraphClaim, String> {
    if oxigraph::model::NamedNode::new(graph_iri).is_err() {
        return Err(format!(
            "Target graph <{graph_iri}> is not a valid IRI, so it cannot be a graph of dataset \
             '{dataset_id}'."
        ));
    }
    refuse_model_registry_graph(store, base_url, dataset_id, graph_iri)?;
    let registered = db.dataset_has_graph(dataset_id, graph_iri).map_err(|e| {
        format!("Could not check whether graph <{graph_iri}> belongs to '{dataset_id}': {e}")
    })?;
    if user.is_admin() {
        return Ok(if registered {
            GraphClaim::Registered
        } else if graph_holds_data(store, graph_iri) {
            GraphClaim::Adopted
        } else {
            GraphClaim::Created
        });
    }
    authorize_dataset_graph_target(db, base_url, dataset_id, graph_iri)?;
    if registered {
        return Ok(GraphClaim::Registered);
    }
    if dataset_owns_graph(base_url, dataset_id, graph_iri)
        || dataset_well_known_graph(base_url, dataset_id, graph_iri)
    {
        return Ok(GraphClaim::Created);
    }
    // A graph with no data yet is the dataset's to create — unless the graph
    // ACL already hands it to someone (a grant made before its data arrives):
    // then it is theirs, as a graph that holds data is.
    let holds_data = graph_holds_data(store, graph_iri);
    if !holds_data && !graph_named_in_acl(db, graph_iri) {
        return Ok(GraphClaim::Created);
    }
    if may_write_graph_directly(db, user, graph_iri) {
        return Ok(if holds_data {
            GraphClaim::Adopted
        } else {
            GraphClaim::Created
        });
    }
    Err(format!(
        "Graph <{graph_iri}> already holds data, or access to it is granted through the graph \
         ACL, and you may not write it, so it cannot become a graph of dataset '{dataset_id}': a \
         dataset's editors can overwrite and delete its graphs. Ask an admin to attach it, or to \
         grant you write access to it."
    ))
}

/// The boundary of a write on dataset authority into a graph the caller
/// names (bulk IFC and CityJSON imports): the graph must be held by
/// `dataset_id` ([`dataset_holds_graph`]) and used by no other dataset, and
/// is never a model-registry graph (admins included). Admins may otherwise
/// write any graph. `Err(message)` maps to HTTP 403; fails closed.
pub fn authorize_dataset_write_target(
    store: &TripleStore,
    db: &crate::auth::db::AuthDb,
    base_url: &str,
    dataset_id: &str,
    graph_iri: &str,
    is_admin: bool,
) -> Result<(), String> {
    refuse_model_registry_graph(store, base_url, dataset_id, graph_iri)?;
    if is_admin {
        return Ok(());
    }
    let in_scope = dataset_owns_graph(base_url, dataset_id, graph_iri)
        || db.dataset_has_graph(dataset_id, graph_iri).unwrap_or(false);
    if !in_scope || graph_used_by_other_dataset(db, dataset_id, graph_iri) {
        return Err(format!(
            "Target graph <{graph_iri}> is outside dataset '{dataset_id}'"
        ));
    }
    Ok(())
}

/// `Err(message)` when `graph_iri` is a model-registry graph
/// ([`graph_held_by_model_registry`]; fails closed). For a write into a graph
/// that is already registered to the dataset, where the registration gate
/// ([`gate_dataset_graph_target`]) no longer runs: a registration made before
/// registry graphs were refused must not become a way to overwrite one.
pub fn refuse_model_registry_graph(
    store: &TripleStore,
    base_url: &str,
    dataset_id: &str,
    graph_iri: &str,
) -> Result<(), String> {
    if graph_held_by_model_registry(store, base_url, graph_iri) {
        return Err(format!(
            "Target graph <{graph_iri}> belongs to the model registry, so it cannot be a graph of \
             dataset '{dataset_id}'. Models and their versions are managed through the data-model \
             API."
        ));
    }
    Ok(())
}

/// Whether a dataset operation — detaching a graph, deleting the dataset or
/// deleting its organisation — must leave the stored graph `graph_iri` in
/// place and remove only the dataset's reference to it:
///
/// * a `urn:system:` graph, except the dataset's own metadata and validation
///   report graphs (`urn:system:metadata:dataset:{id}`,
///   `urn:system:reports:dataset:{id}`);
/// * a SHACL Studio Library graph (`urn:shapes:`), which goes only with its
///   Library entry;
/// * a model-registry graph ([`graph_held_by_model_registry`]; fails closed).
///
/// Being registered to the dataset is not enough to delete a graph: rows made
/// before registration refused registry graphs, or through a path that does
/// not gate registration, must not turn a detach into a wipe of the model
/// registry, a no-derivatives copy the seeder keeps, or another user's model.
pub fn graph_kept_on_dataset_delete(
    store: &TripleStore,
    base_url: &str,
    dataset_id: &str,
    graph_iri: &str,
) -> bool {
    if graph_iri.starts_with("urn:system:") {
        let own_system_graph = graph_iri == dataset_metadata_graph_iri(dataset_id)
            || graph_iri == dataset_reports_graph_iri(dataset_id);
        if !own_system_graph {
            return true;
        }
    }
    if graph_iri.starts_with("urn:shapes:") {
        return true;
    }
    graph_held_by_model_registry(store, base_url, graph_iri)
}

/// Named graph IRI where a dataset's SHACL validation reports are kept (it is
/// registered to the dataset, so deleting the dataset deletes it).
fn dataset_reports_graph_iri(dataset_id: &str) -> String {
    format!("urn:system:reports:dataset:{dataset_id}")
}

/// Whether a dataset other than `dataset_id` still uses `graph_iri`: registered
/// to it, or set as its shapes graph (the shapes graph lives only in the
/// `datasets` table). Fails closed: a lookup error counts as used.
pub fn graph_used_by_other_dataset(
    db: &crate::auth::db::AuthDb,
    dataset_id: &str,
    graph_iri: &str,
) -> bool {
    if !matches!(
        db.graph_has_other_dataset_refs(graph_iri, dataset_id),
        Ok(false)
    ) {
        return true;
    }
    match db.list_datasets() {
        Ok(all) => all
            .iter()
            .any(|d| d.id != dataset_id && d.shapes_graph_iri.as_deref() == Some(graph_iri)),
        Err(_) => true,
    }
}

/// Whether `dataset_id`'s shapes graph setting `graph_iri` names a graph the
/// dataset deletes with itself: one in its own namespace, or left behind in
/// the namespace of a dataset that no longer exists. Any other shapes graph
/// the dataset only links (one it wrote outside its namespace is registered
/// to it, and deleted by that registration's rule).
pub fn shapes_graph_goes_with_dataset(
    db: &crate::auth::db::AuthDb,
    base_url: &str,
    dataset_id: &str,
    graph_iri: &str,
) -> bool {
    dataset_owns_graph(base_url, dataset_id, graph_iri)
        || namespace_dataset_id(base_url, graph_iri)
            .is_some_and(|owner| matches!(db.get_dataset(owner), Ok(None)))
}

/// Whether a detach or delete of `dataset_id` by `actor` may delete the
/// stored graph `graph_iri` (the caller has already ruled out the graphs
/// [`graph_kept_on_dataset_delete`] keeps and those another dataset uses).
/// Only a graph that is the dataset's own goes:
///
/// * inside its namespace, or one of its well-known graphs;
/// * inside the namespace of a dataset that no longer exists (the last user
///   of a shapes graph a deleted dataset left behind);
/// * a graph the dataset created (`origin = 'created'`: it held no data when
///   the dataset claimed it);
/// * or any graph `actor` may delete directly anyway
///   ([`may_write_graph_directly`]: an admin, or a graph-ACL write grant).
///
/// Everything else — a graph adopted with someone else's write authority, a
/// registration older than origin tracking, a graph the server attached (a
/// source run) or a shapes graph the dataset only links — loses only the
/// dataset's reference. A lookup error keeps the graph.
fn dataset_may_delete_graph(
    db: &crate::auth::db::AuthDb,
    base_url: &str,
    dataset_id: &str,
    graph_iri: &str,
    actor: &AuthenticatedUser,
) -> bool {
    if dataset_owns_graph(base_url, dataset_id, graph_iri)
        || dataset_well_known_graph(base_url, dataset_id, graph_iri)
    {
        return true;
    }
    if let Some(owner) = namespace_dataset_id(base_url, graph_iri) {
        if matches!(db.get_dataset(owner), Ok(None)) {
            return true;
        }
    }
    if matches!(
        db.dataset_graph_origin(dataset_id, graph_iri),
        Ok(Some(Some(ref o))) if o == "created"
    ) {
        return true;
    }
    may_write_graph_directly(db, actor, graph_iri)
}

/// Of `candidates` (graphs registered to `dataset_id`, and its shapes graph),
/// the ones a detach or dataset delete by `actor` may drop from the store:
/// each is neither kept by [`graph_kept_on_dataset_delete`] nor still used by
/// another dataset ([`graph_used_by_other_dataset`]), and is the dataset's
/// own (`dataset_may_delete_graph`). Duplicates are dropped. Call it while
/// the dataset's registrations still exist: it reads their recorded origin.
pub fn graphs_deletable_with_dataset(
    store: &TripleStore,
    db: &crate::auth::db::AuthDb,
    base_url: &str,
    dataset_id: &str,
    candidates: &[String],
    actor: &AuthenticatedUser,
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for g in candidates {
        if g.is_empty() || out.contains(g) {
            continue;
        }
        if graph_kept_on_dataset_delete(store, base_url, dataset_id, g) {
            tracing::info!(
                "dataset '{dataset_id}': graph <{g}> is kept in the store (a system, SHACL Studio \
                 or model-registry graph); only the dataset's reference to it is removed"
            );
            continue;
        }
        if graph_used_by_other_dataset(db, dataset_id, g) {
            continue;
        }
        if !dataset_may_delete_graph(db, base_url, dataset_id, g, actor) {
            tracing::info!(
                "dataset '{dataset_id}': graph <{g}> is kept in the store (the dataset did not \
                 create it, and the caller may not delete it directly); only the dataset's \
                 reference to it is removed"
            );
            continue;
        }
        out.push(g.clone());
    }
    out
}

/// The `app_settings` key [`release_model_registry_claims`] sets once it has
/// swept every registration.
const REGISTRY_CLAIMS_MARKER: &str = "cleanup.dataset_registry_claims.v1";

/// Whether [`release_model_registry_claims`] has completed on this identity
/// database (a lookup error counts as not yet).
pub fn model_registry_claims_released(db: &crate::auth::db::AuthDb) -> bool {
    matches!(db.get_app_setting(REGISTRY_CLAIMS_MARKER), Ok(Some(_)))
}

/// One-time boot cleanup: `dataset_graphs` rows naming a model-registry graph,
/// made before the registration gate refused them. They still made the
/// graph dataset-scoped for reads (`get_accessible_graph_iris`), hid it from
/// everyone else, and made it one the dataset holds. Each such row goes; a
/// shapes-role row stays in effect for validation as a SHACL Studio binding,
/// which only reads the graph. Datasets that lost a row get their metadata
/// graph rewritten.
///
/// A dataset's `shapes_graph_iri` naming a registry graph is left as it is:
/// it does not scope reads, the paths that could write or delete it refuse
/// registry graphs, and clearing it would weaken the dataset's validation
/// (the `shacl_on_write` gate refuses any result, keeps an on-write report
/// and checks source runs; a binding does not).
///
/// Runs once: a marker in `app_settings` records a sweep in which every
/// registry lookup answered. A lookup that fails removes nothing (unlike the
/// fail-closed [`graph_held_by_model_registry`]) and leaves the marker unset,
/// so the next boot tries again. Returns the number of rows removed.
pub fn release_model_registry_claims(
    store: &TripleStore,
    db: &crate::auth::db::AuthDb,
    base_url: &str,
) -> anyhow::Result<usize> {
    if db.get_app_setting(REGISTRY_CLAIMS_MARKER)?.is_some() {
        return Ok(0);
    }
    let mut undecided = false;
    let mut held = |g: &str| -> bool {
        if g == crate::data_models::registry::REGISTRY_GRAPH
            || in_model_registry_namespace(base_url, g)
        {
            return true;
        }
        match crate::data_models::registry::graph_held_by_version_checked(store, g) {
            Some(h) => h,
            None => {
                undecided |= oxigraph::model::NamedNode::new(g).is_ok();
                false
            }
        }
    };
    let bind = |dataset_id: &str, g: &str| {
        let target = crate::shacl_studio::bindings::dataset_target_iri(base_url, dataset_id);
        if let Err(e) = crate::shacl_studio::bindings::add_binding(store, &target, g) {
            tracing::warn!("could not keep <{g}> as a validation binding of '{dataset_id}': {e}");
        }
    };
    let mut removed = 0usize;
    let mut touched: Vec<String> = Vec::new();
    for (dataset_id, g, role) in db.list_all_dataset_graph_rows()? {
        if !held(&g) {
            continue;
        }
        if role == Some(GraphKind::Shapes) {
            bind(&dataset_id, &g);
        }
        if db.remove_dataset_graph(&dataset_id, &g)? {
            tracing::warn!(
                "dataset '{dataset_id}': removed its registration of model-registry graph <{g}>, \
                 made before registry graphs were refused"
            );
            removed += 1;
            if !touched.contains(&dataset_id) {
                touched.push(dataset_id);
            }
        }
    }
    for id in &touched {
        if let Ok(Some(d)) = db.get_dataset(id) {
            let entries = db.list_dataset_graph_entries(id).unwrap_or_default();
            write_dataset_metadata_graph(store, base_url, &d, &entries);
        }
    }
    if undecided {
        tracing::warn!(
            "registry-claim cleanup: a model-registry lookup failed; it runs again next boot"
        );
    } else {
        db.set_app_setting(REGISTRY_CLAIMS_MARKER, &chrono::Utc::now().to_rfc3339())?;
    }
    Ok(removed)
}

/// Write (or overwrite) the DCAT/ADMS/VoID/VCARD metadata named graph for a dataset.
/// Silently ignores errors so that metadata graph failures never abort the main operation.
///
/// Pass `graph_entries` (from `db.list_dataset_graph_entries`) to include
/// per-graph role triples in the metadata. Pass an empty slice if not available.
pub fn write_dataset_metadata_graph(
    store: &TripleStore,
    base_url: &str,
    dataset: &Dataset,
    graph_entries: &[DatasetGraphEntry],
) {
    let graph_iri = dataset_metadata_graph_iri(&dataset.id);
    let ttl = build_dataset_metadata_ttl(base_url, dataset, graph_entries);
    let _ = store.graph_store_put(Some(&graph_iri), &ttl, RdfFormat::Turtle);
}

/// Like [`write_dataset_metadata_graph`], but validates the built metadata against
/// the built-in **dataset-structure** SHACL model first. Returns the
/// non-conforming `ValidationReport` (and writes nothing) when the metadata
/// violates the model; otherwise writes and returns `Ok`. Used by the dataset
/// create/update API so non-conforming dataset metadata is rejected (HTTP 422).
pub fn write_dataset_metadata_graph_checked(
    store: &TripleStore,
    base_url: &str,
    dataset: &Dataset,
    graph_entries: &[DatasetGraphEntry],
) -> Result<(), crate::shacl::report::ValidationReport> {
    let ttl = build_dataset_metadata_ttl(base_url, dataset, graph_entries);
    if let Some(report) = crate::auth::dataset_audit::validate_metadata(store, &ttl) {
        if !report.conforms {
            return Err(report);
        }
    }
    let graph_iri = dataset_metadata_graph_iri(&dataset.id);
    let _ = store.graph_store_put(Some(&graph_iri), &ttl, RdfFormat::Turtle);
    Ok(())
}

/// Build the DCAT/ADMS/VoID/VCARD metadata Turtle for a dataset (no I/O).
pub fn build_dataset_metadata_ttl(
    base_url: &str,
    dataset: &Dataset,
    graph_entries: &[DatasetGraphEntry],
) -> String {
    // Canonical dataset IRI per the styleguide (§3.3): `{base}/dataset/{id}`
    // (singular). This MUST match the catalogue (`dcat/catalog.rs`), the version
    // registry and the API-service registry, otherwise a dataset's descriptive
    // metadata splits across two IRIs and its node renders incomplete when browsed.
    let dataset_iri = dataset_iri(base_url, &dataset.id);
    let mut ttl = String::new();

    ttl.push_str("@prefix dcat: <http://www.w3.org/ns/dcat#> .\n");
    ttl.push_str("@prefix dct:  <http://purl.org/dc/terms/> .\n");
    ttl.push_str("@prefix void: <http://rdfs.org/ns/void#> .\n");
    ttl.push_str("@prefix adms: <http://www.w3.org/ns/adms#> .\n");
    ttl.push_str("@prefix vcard: <http://www.w3.org/2006/vcard/ns#> .\n");
    ttl.push_str("@prefix xsd:  <http://www.w3.org/2001/XMLSchema#> .\n");
    ttl.push_str("@prefix ots:  <https://opentriplestore.org/ns#> .\n\n");

    ttl.push_str(&format!(
        "<{}> a dcat:Dataset, void:Dataset ;\n",
        dataset_iri
    ));
    ttl.push_str(&format!(
        "    dct:title \"{}\" ;\n",
        escape_ttl_string(&dataset.name)
    ));
    // Required by the dataset-structure SHACL model: stable identity + visibility.
    ttl.push_str(&format!(
        "    dct:identifier \"{}\" ;\n",
        escape_ttl_string(&dataset.id)
    ));
    ttl.push_str(&format!(
        "    ots:visibility \"{}\" ;\n",
        dataset.visibility.as_str()
    ));

    if let Some(desc) = &dataset.description {
        ttl.push_str(&format!(
            "    dct:description \"{}\" ;\n",
            escape_ttl_string(desc)
        ));
    }
    if let Some(lic) = &dataset.license {
        if !lic.is_empty() {
            ttl.push_str(&format!("    dct:license <{}> ;\n", escape_sparql_iri(lic)));
        }
    }

    // Themes (stored as JSON array of IRI strings)
    if let Some(themes_json) = &dataset.themes {
        if let Ok(themes) = serde_json::from_str::<Vec<String>>(themes_json) {
            for theme in &themes {
                if !theme.is_empty() {
                    ttl.push_str(&format!(
                        "    dcat:theme <{}> ;\n",
                        escape_sparql_iri(theme)
                    ));
                }
            }
        }
    }

    // Keywords (stored as JSON array of plain strings)
    if let Some(kw_json) = &dataset.keywords {
        if let Ok(keywords) = serde_json::from_str::<Vec<String>>(kw_json) {
            for kw in &keywords {
                if !kw.is_empty() {
                    ttl.push_str(&format!(
                        "    dcat:keyword \"{}\"@en ;\n",
                        escape_ttl_string(kw)
                    ));
                }
            }
        }
    }

    if let Some(status) = &dataset.adms_status {
        if !status.is_empty() {
            ttl.push_str(&format!(
                "    adms:status <{}> ;\n",
                escape_sparql_iri(status)
            ));
        }
    }
    if let Some(notes) = &dataset.version_notes {
        if !notes.is_empty() {
            ttl.push_str(&format!(
                "    adms:versionNotes \"{}\" ;\n",
                escape_ttl_string(notes)
            ));
        }
    }
    if let Some(spatial) = &dataset.spatial {
        if !spatial.is_empty() {
            ttl.push_str(&format!(
                "    dct:spatial <{}> ;\n",
                escape_sparql_iri(spatial)
            ));
        }
    }

    let landing = dataset
        .landing_page
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or(&dataset_iri);
    ttl.push_str(&format!(
        "    dcat:landingPage <{}> ;\n",
        escape_sparql_iri(landing)
    ));

    ttl.push_str(&format!(
        "    dct:issued \"{}\"^^xsd:dateTime ;\n",
        dataset.created_at
    ));
    ttl.push_str(&format!(
        "    dct:modified \"{}\"^^xsd:dateTime ;\n",
        dataset.updated_at
    ));
    ttl.push_str(&format!(
        "    void:sparqlEndpoint <{}/sparql> ;\n",
        base_url
    ));

    // Contact point as blank node
    let has_contact = dataset
        .contact_name
        .as_deref()
        .filter(|s| !s.is_empty())
        .is_some()
        || dataset
            .contact_email
            .as_deref()
            .filter(|s| !s.is_empty())
            .is_some()
        || dataset
            .contact_url
            .as_deref()
            .filter(|s| !s.is_empty())
            .is_some();

    if has_contact {
        ttl.push_str("    dcat:contactPoint _:cp .\n\n");
        ttl.push_str("_:cp a vcard:Organization ;\n");
        if let Some(name) = dataset.contact_name.as_deref().filter(|s| !s.is_empty()) {
            ttl.push_str(&format!("    vcard:fn \"{}\" ;\n", escape_ttl_string(name)));
        }
        if let Some(email) = dataset.contact_email.as_deref().filter(|s| !s.is_empty()) {
            ttl.push_str(&format!(
                "    vcard:hasEmail <mailto:{}> ;\n",
                escape_sparql_iri(email)
            ));
        }
        if let Some(url) = dataset.contact_url.as_deref().filter(|s| !s.is_empty()) {
            ttl.push_str(&format!(
                "    vcard:hasURL <{}> ;\n",
                escape_sparql_iri(url)
            ));
        }
        ttl.push_str("    .\n");
    } else {
        ttl.push_str("    .\n");
    }

    // Per-graph role triples: void:subset + ots:graphRole for each registered graph.
    for entry in graph_entries {
        if !entry.graph_iri.starts_with("urn:system:") {
            ttl.push_str(&format!(
                "<{}> void:subset <{}> .\n",
                dataset_iri, entry.graph_iri
            ));
            if let Some(role) = entry.graph_role {
                let role_iri = graph_role_iri(role);
                ttl.push_str(&format!(
                    "<{}> ots:graphRole <{}> .\n",
                    entry.graph_iri, role_iri
                ));
            }
        }
    }

    ttl
}

/// Rewrite every dataset's DCAT metadata graph so pre-existing datasets adopt the
/// canonical **singular** dataset IRI (`{base}/dataset/{id}`, styleguide §3.3).
///
/// Older builds wrote the subject as `{base}/datasets/{id}` (plural) while the
/// catalogue, version registry and API-service registry always used singular —
/// so a dataset's descriptive metadata (title, members, contact) lived under a
/// different IRI than the triples pointing at it, and its node rendered split and
/// incomplete when browsed. `write_dataset_metadata_graph` does a PUT (clear then
/// load), so re-running it simply replaces the old plural-subject triples. Cheap
/// (a handful of datasets) and idempotent — safe to run on every boot.
pub fn reconcile_all_dataset_metadata(
    store: &TripleStore,
    base_url: &str,
    db: &crate::auth::db::AuthDb,
) {
    let datasets = match db.list_datasets() {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!("dataset metadata reconcile: list failed: {e}");
            return;
        }
    };
    let mut migrated = 0usize;
    for ds in &datasets {
        // Only rewrite datasets still carrying the OLD plural subject. Rewriting
        // is expensive (clear + reload + graph-index refresh per dataset), so the
        // guard keeps this a cheap no-op on every boot after the one-time
        // migration — and leaves brand-new (already-singular) datasets untouched.
        let graph_iri = dataset_metadata_graph_iri(&ds.id);
        let plural_subject = format!("{}/datasets/{}", base_url.trim_end_matches('/'), ds.id);
        if !graph_has_subject(store, &graph_iri, &plural_subject) {
            continue;
        }
        let entries = db.list_dataset_graph_entries(&ds.id).unwrap_or_default();
        write_dataset_metadata_graph(store, base_url, ds, &entries);
        migrated += 1;
    }
    if migrated > 0 {
        tracing::info!(
            "dataset metadata reconcile: migrated {migrated} dataset metadata graph(s) onto the \
             canonical singular dataset IRI scheme"
        );
    }
}

// ─── Model / Vocabulary / Instance reframe migration ────────────────────────────

/// System graph recording which one-shot content migrations have run.
const MIGRATIONS_GRAPH: &str = "urn:system:migrations";
/// Sentinel subject for the Model/Vocabulary reframe + drop-"ontology" migration.
const REFRAME_MIGRATION_IRI: &str = "urn:system:migration:model-vocabulary-reframe";

/// One-time content migration for the Model / Vocabulary / Instance reframe and
/// the drop of the legacy `https://opentriplestore.org/ontology/` IRI base.
///
/// Two parts:
///  1. **Every-boot, idempotent** rewrite of legacy `…/ontology/` predicates in
///     the tiny `urn:system:audit` and `urn:system:validation-layer` graphs to the
///     new `…/ns#` base (a no-op once done; these graphs are small).
///  2. **One-shot** re-detection of stored `model` graphs: a property/SKOS-only
///     graph (R-Box) classified `model` under the old detector is moved to
///     `vocabulary`, and the owning dataset's metadata graph is re-emitted so its
///     `graphRole` triple reflects the new role. Guarded by a sentinel triple so
///     graph contents are scanned at most once.
///
/// Dataset metadata graphs additionally self-heal via `audit_dataset_metadata`
/// (the old `…/ontology/visibility` IRI fails the refreshed dataset-structure
/// shape, triggering a re-emit with the new IRIs), and the DCAT catalogue is
/// generated on the fly — so this function only has to handle the role flips and
/// the two system graphs.
pub fn migrate_model_vocabulary_reframe(
    store: &TripleStore,
    base_url: &str,
    db: &crate::auth::db::AuthDb,
) {
    // Gate the whole one-shot migration — including the legacy-IRI rewrite —
    // behind the applied sentinel. The rewrite is idempotent, but each of its
    // `store.update()`s triggers a full graph-index rebuild (a scan of every
    // graph, including a dataset's multi-million-triple `…/ifcowl` lift), so
    // running it on every boot was a recurring full-store scan for no effect.
    if graph_has_subject(store, MIGRATIONS_GRAPH, REFRAME_MIGRATION_IRI) {
        return;
    }
    rewrite_legacy_ontology_iris(store);
    let reclassified = reclassify_model_graphs_to_vocabulary(store, base_url, db);
    if reclassified > 0 {
        tracing::info!(
            "model/vocabulary reframe: reclassified {reclassified} property graph(s) \
             model→vocabulary"
        );
    }
    let mark = format!(
        "INSERT DATA {{ GRAPH <{MIGRATIONS_GRAPH}> {{ <{REFRAME_MIGRATION_IRI}> \
         <urn:system:migration#applied> true }} }}"
    );
    if let Err(e) = store.update(&mark) {
        // Non-fatal but observable: if the sentinel write fails the migration
        // simply re-runs next boot (it is idempotent), but a persistent failure
        // means it never marks done — surface it instead of swallowing.
        tracing::warn!("model/vocabulary reframe: failed to record applied marker: {e}");
    }
}

/// Rewrite the two known legacy `…/ontology/` predicates to the `…/ns#` base in
/// the system graphs that materialise them. Idempotent (no-op once migrated).
fn rewrite_legacy_ontology_iris(store: &TripleStore) {
    const AUDIT_GRAPH: &str = "urn:system:audit";
    const VALIDATION_GRAPH: &str = "urn:system:validation-layer";
    let updates = [
        format!(
            "DELETE {{ GRAPH <{AUDIT_GRAPH}> {{ ?s <https://opentriplestore.org/ontology/auditStatus> ?o }} }} \
             INSERT {{ GRAPH <{AUDIT_GRAPH}> {{ ?s <https://opentriplestore.org/ns#auditStatus> ?o }} }} \
             WHERE  {{ GRAPH <{AUDIT_GRAPH}> {{ ?s <https://opentriplestore.org/ontology/auditStatus> ?o }} }}"
        ),
        format!(
            "DELETE {{ GRAPH <{VALIDATION_GRAPH}> {{ ?s <https://opentriplestore.org/ontology/validatedBy> ?o }} }} \
             INSERT {{ GRAPH <{VALIDATION_GRAPH}> {{ ?s <https://opentriplestore.org/ns#validatedBy> ?o }} }} \
             WHERE  {{ GRAPH <{VALIDATION_GRAPH}> {{ ?s <https://opentriplestore.org/ontology/validatedBy> ?o }} }}"
        ),
    ];
    for q in &updates {
        if let Err(e) = store.update(q) {
            tracing::warn!("legacy-IRI rewrite update failed (non-fatal): {e}");
        }
    }
}

/// Re-run content detection over every per-graph role currently stored as
/// `model`; flip to `vocabulary` when the graph is really R-Box (properties /
/// SKOS) with no class anchor. Never demotes a real class graph. Returns the
/// number of graphs reclassified.
fn reclassify_model_graphs_to_vocabulary(
    store: &TripleStore,
    base_url: &str,
    db: &crate::auth::db::AuthDb,
) -> usize {
    let datasets = match db.list_datasets() {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!("model/vocabulary reframe: list datasets failed: {e}");
            return 0;
        }
    };
    let mut reclassified = 0usize;
    for ds in &datasets {
        let entries = db.list_dataset_graph_entries(&ds.id).unwrap_or_default();
        let mut changed = false;
        for e in &entries {
            if e.graph_role == Some(GraphKind::Model)
                && detect_graph_role(store, &e.graph_iri) == Some(GraphKind::Vocabulary)
                && db
                    .set_dataset_graph_role(&ds.id, &e.graph_iri, Some(GraphKind::Vocabulary))
                    .is_ok()
            {
                reclassified += 1;
                changed = true;
            }
        }
        if changed {
            // Re-emit metadata so the stored graphRole triples match the new roles.
            let updated = db.list_dataset_graph_entries(&ds.id).unwrap_or_default();
            write_dataset_metadata_graph(store, base_url, ds, &updated);
        }
    }
    reclassified
}

/// Detect the [`GraphKind`] of a single named graph by scanning its quads.
/// Returns `None` for an empty or unclassifiable graph.
fn detect_graph_role(store: &TripleStore, graph_iri: &str) -> Option<GraphKind> {
    use oxigraph::model::{GraphNameRef, NamedNodeRef, Quad};
    let g = NamedNodeRef::new(graph_iri).ok()?;
    let quads: Vec<Quad> = store
        .store()
        .quads_for_pattern(None, None, None, Some(GraphNameRef::NamedNode(g)))
        .filter_map(|r| r.ok())
        .collect();
    if quads.is_empty() {
        return None;
    }
    crate::kind_detector::detect(&quads).to_graph_role()
}

/// True if `subject_iri` appears as a subject in the named graph `graph_iri`.
/// A cheap targeted lookup via the quad API (no full scan).
fn graph_has_subject(store: &TripleStore, graph_iri: &str, subject_iri: &str) -> bool {
    use oxigraph::model::{GraphNameRef, NamedNodeRef, NamedOrBlankNodeRef};
    let s = match NamedNodeRef::new(subject_iri) {
        Ok(n) => n,
        Err(_) => return false,
    };
    let g = match NamedNodeRef::new(graph_iri) {
        Ok(n) => n,
        Err(_) => return false,
    };
    store
        .store()
        .quads_for_pattern(
            Some(NamedOrBlankNodeRef::NamedNode(s)),
            None,
            None,
            Some(GraphNameRef::NamedNode(g)),
        )
        .next()
        .is_some()
}

fn escape_ttl_string(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

/// The IRI of a graph-role individual in the Open Triplestore vocabulary
/// (`https://opentriplestore.org/ns#{Role}`). Single source of truth, also used
/// by the DCAT catalogue (`dcat::catalog`) so the two never drift.
pub fn graph_role_iri(role: GraphKind) -> &'static str {
    match role {
        GraphKind::Instances => "https://opentriplestore.org/ns#Instances",
        GraphKind::Model => "https://opentriplestore.org/ns#Model",
        GraphKind::Vocabulary => "https://opentriplestore.org/ns#Vocabulary",
        GraphKind::Shapes => "https://opentriplestore.org/ns#Shapes",
        GraphKind::Entailment => "https://opentriplestore.org/ns#Entailment",
        GraphKind::System => "https://opentriplestore.org/ns#System",
        GraphKind::DomainValues => "https://opentriplestore.org/ns#DomainValues",
        GraphKind::Linkset => "https://opentriplestore.org/ns#Linkset",
        GraphKind::Provenance => "https://opentriplestore.org/ns#Provenance",
        GraphKind::Catalog => "https://opentriplestore.org/ns#Catalog",
    }
}
