//! `GET /api/browse/resource/context` — where a resource belongs.
//!
//! The resource page shows what is said about a resource. This says where it
//! sits: which model defines it, which classes it is an instance of and the
//! models that define those, which dataset holds it and the model that dataset
//! declares it conforms to, and, for a class, where its instances are.
//!
//! Everything is read from graphs the caller may read; a model, a dataset or
//! an instance count behind a graph the caller cannot read is left out, so the
//! answer shows no more than the caller could work out from `/sparql` and the
//! registry.

use std::collections::{BTreeMap, HashMap, HashSet};

use axum::extract::{Query, State};
use axum::response::IntoResponse;
use axum::{Extension, Json};
use oxigraph::model::Term;
use oxigraph::sparql::QueryResults;
use serde::Deserialize;
use serde_json::{json, Value};

use super::error::AppError;
use super::routes::validate_iri;
use super::AppState;
use crate::auth::middleware::AuthenticatedUser;
use crate::auth::models::Dataset;
use crate::data_models::registry::{self as dmr, GraphVersion};
use crate::store::TripleStore;

#[derive(Debug, Deserialize)]
pub struct ResourceContextParams {
    pub iri: String,
    /// The graph the page is scoped to, when it is. A version graph of a model
    /// makes that version the one named for the model.
    pub graph: Option<String>,
}

/// Classes listed under "instance of": enough for any real resource, and a
/// bound on the lookups a resource typed with hundreds of classes would cost.
const MAX_TYPES: usize = 12;

const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";

/// Namespaces of the modelling languages. A resource typed `owl:Class` or
/// `skos:Concept` is a term of a model, not an instance of one: those types
/// are left out of "instance of".
const LANGUAGE_NS: [&str; 5] = [
    "http://www.w3.org/1999/02/22-rdf-syntax-ns#",
    "http://www.w3.org/2000/01/rdf-schema#",
    "http://www.w3.org/2002/07/owl#",
    "http://www.w3.org/2004/02/skos/core#",
    "http://www.w3.org/ns/shacl#",
];

fn is_language_term(iri: &str) -> bool {
    LANGUAGE_NS.iter().any(|ns| iri.starts_with(ns))
}

/// The value of an IRI binding, `None` for anything else.
fn iri_of(term: Option<&Term>) -> Option<String> {
    match term {
        Some(Term::NamedNode(n)) => Some(n.as_str().to_string()),
        _ => None,
    }
}

/// Run a SELECT and hand each row's bindings to `row`.
fn select(store: &TripleStore, query: &str, mut row: impl FnMut(&[Option<Term>])) {
    if let Ok(QueryResults::Solutions(solutions)) = store.query(query) {
        for sol in solutions.flatten() {
            row(sol.values());
        }
    }
}

/// Order two version strings: the numbered parts as numbers (`0.10.0` is
/// after `0.9.1`), and a pre-release (`1.0.0-rc1`) before the release it
/// leads up to. Parts that are not numbers compare as text.
fn version_order(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering::{Equal, Greater, Less};
    let split = |v: &str| -> (Vec<String>, Option<String>) {
        let (core, pre) = match v.split_once('-') {
            Some((core, pre)) => (core, Some(pre.to_string())),
            None => (v, None),
        };
        (core.split('.').map(str::to_string).collect(), pre)
    };
    let (core_a, pre_a) = split(a);
    let (core_b, pre_b) = split(b);
    for i in 0..core_a.len().max(core_b.len()) {
        let ord = match (core_a.get(i), core_b.get(i)) {
            (None, None) => Equal,
            (None, Some(_)) => Less,
            (Some(_), None) => Greater,
            (Some(x), Some(y)) => match (x.parse::<u64>(), y.parse::<u64>()) {
                (Ok(nx), Ok(ny)) => nx.cmp(&ny),
                _ => x.cmp(y),
            },
        };
        if ord != Equal {
            return ord;
        }
    }
    match (pre_a, pre_b) {
        (None, None) => Equal,
        (Some(_), None) => Less,
        (None, Some(_)) => Greater,
        (Some(x), Some(y)) => x.cmp(&y),
    }
}

/// One entry per model out of the versions that hold a term: the version the
/// page is scoped to when it is among them, else the latest published one,
/// else the highest. `other_versions` says how many more hold it.
fn models_json(versions: &[&GraphVersion], scoped_graph: Option<&str>) -> Vec<Value> {
    let mut by_model: BTreeMap<&str, Vec<&GraphVersion>> = BTreeMap::new();
    for v in versions {
        let held = by_model.entry(v.model_id.as_str()).or_default();
        if !held.iter().any(|h| h.version == v.version) {
            held.push(v);
        }
    }
    let mut out = Vec::new();
    for (model_id, mut held) in by_model {
        held.sort_by(|a, b| version_order(&b.version, &a.version));
        let scoped = scoped_graph.and_then(|g| {
            versions
                .iter()
                .find(|v| v.model_id == model_id && v.graph == g)
                .map(|v| v.version.as_str())
        });
        let chosen = held
            .iter()
            .find(|v| Some(v.version.as_str()) == scoped)
            .or_else(|| held.iter().find(|v| v.latest_published))
            .unwrap_or(&held[0]);
        out.push(json!({
            "model_id": model_id,
            "title": chosen.title,
            "kind": chosen.kind,
            "version": chosen.version,
            "status": chosen.status,
            "other_versions": held.len() - 1,
        }));
    }
    out
}

fn dataset_json(ds: &Dataset, titles: &HashMap<String, String>) -> Value {
    let conforms_to = ds.conforms_to_model.as_ref().map(|model_id| {
        json!({
            "model_id": model_id,
            "version": ds.conforms_to_version,
            "title": titles.get(model_id),
        })
    });
    json!({ "id": ds.id, "name": ds.name, "conforms_to": conforms_to })
}

/// GET /api/browse/resource/context
pub async fn browse_resource_context(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    Query(params): Query<ResourceContextParams>,
) -> Result<impl IntoResponse, AppError> {
    // A blank node has no place in a registry and cannot be asked about by name.
    if params.iri.starts_with("_:") {
        return Ok(Json(json!({ "iri": params.iri })));
    }
    validate_iri(&params.iri)?;
    if let Some(ref g) = params.graph {
        validate_iri(g)?;
    }

    let user_id = user.as_deref().map(|u| u.user_id.clone());
    let is_admin = user.as_deref().map(|u| u.is_admin()).unwrap_or(false);
    let cached = state
        .auth_db
        .get_accessible_graph_iris_cached(user_id.as_deref())
        .map_err(|e| AppError::Internal(e.to_string()))?;

    let _permit = state
        .browse_semaphore
        .clone()
        .acquire_owned()
        .await
        .map_err(|_| AppError::Internal("browse concurrency limiter closed".to_string()))?;

    let task_state = state.clone();
    let context = tokio::task::spawn_blocking(move || {
        let visible: &HashSet<String> = if is_admin { &cached.1 } else { &cached.0 };
        resource_context(
            &task_state,
            &params.iri,
            params.graph.as_deref(),
            visible,
            user_id.as_deref(),
        )
    })
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?;
    Ok(Json(context))
}

fn resource_context(
    state: &AppState,
    iri: &str,
    scoped_graph: Option<&str>,
    visible: &HashSet<String>,
    user_id: Option<&str>,
) -> Value {
    let store = &state.store;

    // Graphs that describe the resource, and the classes it is typed with there.
    let mut subject_graphs: Vec<String> = Vec::new();
    select(
        store,
        &format!("SELECT DISTINCT ?g WHERE {{ GRAPH ?g {{ <{iri}> ?p ?o }} }}"),
        |row| {
            if let Some(g) = iri_of(row[0].as_ref()).filter(|g| visible.contains(g)) {
                subject_graphs.push(g);
            }
        },
    );
    let mut types: Vec<String> = Vec::new();
    select(
        store,
        &format!("SELECT DISTINCT ?g ?c WHERE {{ GRAPH ?g {{ <{iri}> <{RDF_TYPE}> ?c }} }}"),
        |row| {
            let (Some(g), Some(c)) = (iri_of(row[0].as_ref()), iri_of(row[1].as_ref())) else {
                return;
            };
            if visible.contains(&g) && !is_language_term(&c) && !types.contains(&c) {
                types.push(c);
            }
        },
    );
    types.sort();
    types.truncate(MAX_TYPES);

    // Graphs that describe each of those classes.
    let mut type_graphs: HashMap<String, Vec<String>> = HashMap::new();
    if !types.is_empty() {
        let values: Vec<String> = types.iter().map(|c| format!("<{c}>")).collect();
        select(
            store,
            &format!(
                "SELECT DISTINCT ?c ?g WHERE {{ VALUES ?c {{ {} }} GRAPH ?g {{ ?c ?p ?o }} }}",
                values.join(" ")
            ),
            |row| {
                let (Some(c), Some(g)) = (iri_of(row[0].as_ref()), iri_of(row[1].as_ref())) else {
                    return;
                };
                if visible.contains(&g) {
                    type_graphs.entry(c).or_default().push(g);
                }
            },
        );
    }

    // Where the resource is used as a class: instances per graph.
    let mut instance_graphs: Vec<(String, u64)> = Vec::new();
    select(
        store,
        &format!(
            "SELECT ?g (COUNT(DISTINCT ?s) AS ?n) WHERE {{ GRAPH ?g {{ ?s <{RDF_TYPE}> <{iri}> }} }} GROUP BY ?g"
        ),
        |row| {
            let Some(g) = iri_of(row[0].as_ref()).filter(|g| visible.contains(g)) else {
                return;
            };
            let n = match row[1].as_ref() {
                Some(Term::Literal(l)) => l.value().parse::<u64>().unwrap_or(0),
                _ => 0,
            };
            if n > 0 {
                instance_graphs.push((g, n));
            }
        },
    );

    // One registry query for every graph met above.
    let mut all_graphs: Vec<String> = subject_graphs.clone();
    all_graphs.extend(type_graphs.values().flatten().cloned());
    all_graphs.extend(instance_graphs.iter().map(|(g, _)| g.clone()));
    all_graphs.sort();
    all_graphs.dedup();
    let versions = dmr::versions_of_graphs(store, &all_graphs).unwrap_or_default();
    let versions_in = |graphs: &[String]| -> Vec<&GraphVersion> {
        versions
            .iter()
            .filter(|v| graphs.contains(&v.graph))
            .collect()
    };

    // The datasets holding a graph, for the caller who may open them.
    let mut dataset_cache: HashMap<String, Option<Dataset>> = HashMap::new();
    let mut datasets_of = |graph: &str| -> Vec<Dataset> {
        let ids = state.auth_db.datasets_with_graph(graph).unwrap_or_default();
        let mut out = Vec::new();
        for id in ids {
            let ds = dataset_cache.entry(id.clone()).or_insert_with(|| {
                state.auth_db.get_dataset(&id).ok().flatten().filter(|ds| {
                    state
                        .auth_db
                        .can_access_dataset(user_id, ds)
                        .unwrap_or(false)
                })
            });
            if let Some(ds) = ds {
                out.push(ds.clone());
            }
        }
        out
    };

    let defined_in_versions = versions_in(&subject_graphs);
    let defined_in = models_json(&defined_in_versions, scoped_graph);

    let mut holding: Vec<Dataset> = Vec::new();
    for g in &subject_graphs {
        for ds in datasets_of(g) {
            if !holding.iter().any(|h| h.id == ds.id) {
                holding.push(ds);
            }
        }
    }

    // Instances by where they are: a dataset, a model version, or a loose graph.
    let mut by_dataset: BTreeMap<String, (Dataset, u64)> = BTreeMap::new();
    let mut by_model: Vec<(Vec<&GraphVersion>, u64)> = Vec::new();
    let mut elsewhere: u64 = 0;
    for (g, n) in &instance_graphs {
        let in_datasets = datasets_of(g);
        if let Some(ds) = in_datasets.into_iter().next() {
            by_dataset
                .entry(ds.id.clone())
                .and_modify(|(_, total)| *total += n)
                .or_insert((ds, *n));
            continue;
        }
        let in_versions = versions_in(std::slice::from_ref(g));
        if in_versions.is_empty() {
            elsewhere += n;
        } else {
            by_model.push((in_versions, *n));
        }
    }

    // Titles for the models datasets say they conform to, and the datasets that
    // say they conform to a model defining this resource.
    let defining_ids: HashSet<&str> = defined_in_versions
        .iter()
        .map(|v| v.model_id.as_str())
        .collect();
    let mut titles: HashMap<String, String> = versions
        .iter()
        .map(|v| (v.model_id.clone(), v.title.clone()))
        .collect();
    let accessible = if defining_ids.is_empty() {
        Vec::new()
    } else {
        state
            .auth_db
            .list_accessible_datasets(user_id)
            .unwrap_or_default()
    };
    let conforming: Vec<&Dataset> = accessible
        .iter()
        .filter(|ds| {
            ds.conforms_to_model
                .as_deref()
                .is_some_and(|m| defining_ids.contains(m))
        })
        .collect();
    let wanted_titles: Vec<String> = holding
        .iter()
        .chain(by_dataset.values().map(|(ds, _)| ds))
        .filter_map(|ds| ds.conforms_to_model.clone())
        .filter(|m| !titles.contains_key(m))
        .collect();
    for model_id in wanted_titles {
        if let Some(record) = dmr::get_data_model(store, &state.base_url, &model_id) {
            titles.insert(model_id, record.title);
        }
    }

    let instance_of: Vec<Value> = types
        .iter()
        .map(|c| {
            let graphs = type_graphs.get(c).cloned().unwrap_or_default();
            json!({ "iri": c, "defined_in": models_json(&versions_in(&graphs), None) })
        })
        .collect();

    let mut sources: Vec<Value> = by_dataset
        .values()
        .map(|(ds, n)| json!({ "dataset": dataset_json(ds, &titles), "count": n }))
        .collect();
    for (in_versions, n) in &by_model {
        if let Some(model) = models_json(in_versions, scoped_graph).into_iter().next() {
            sources.push(json!({ "model": model, "count": n }));
        }
    }
    let total: u64 = instance_graphs.iter().map(|(_, n)| n).sum();

    json!({
        "iri": iri,
        "defined_in": defined_in,
        "instance_of": instance_of,
        "datasets": holding.iter().map(|ds| dataset_json(ds, &titles)).collect::<Vec<_>>(),
        "conforming_datasets": conforming
            .iter()
            .map(|ds| json!({ "id": ds.id, "name": ds.name, "version": ds.conforms_to_version }))
            .collect::<Vec<_>>(),
        "instances": {
            "total": total,
            "sources": sources,
            "elsewhere": elsewhere,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_order_by_number_not_by_text() {
        use std::cmp::Ordering::*;
        assert_eq!(version_order("0.10.0", "0.9.1"), Greater);
        assert_eq!(version_order("1.0", "1.0.1"), Less);
        assert_eq!(version_order("2.0.0", "2.0.0"), Equal);
        assert_eq!(version_order("1.0.0-rc1", "1.0.0"), Less);
        assert_eq!(version_order("1.0.0-rc2", "1.0.0-rc1"), Greater);
    }

    #[test]
    fn terms_of_the_modelling_languages_are_not_classes_of_data() {
        assert!(is_language_term("http://www.w3.org/2002/07/owl#Class"));
        assert!(is_language_term(
            "http://www.w3.org/2004/02/skos/core#Concept"
        ));
        assert!(!is_language_term("http://example.org/def#Bridge"));
    }
}
