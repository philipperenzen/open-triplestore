//! Canonical OPM in and out.
//!
//! The server links a property node to its item with `ots:propertyOf` /
//! `ots:propertyPredicate` (see the module docs of [`super`]). Other OPM tools
//! expect `<item> <kind> <property>`. Export writes that canonical link and
//! leaves out the server's own bookkeeping; import reads it (and the
//! server's own form), so states move between datasets and tools without
//! loss: state and property IRIs, values, validity, recording time,
//! attribution, reliability, deletion, documentation, notes and derivations.

use std::collections::{BTreeMap, HashMap, HashSet};

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use oxigraph::io::{JsonLdProfileSet, RdfFormat, RdfParser, RdfSerializer};
use oxigraph::model::{GraphNameRef, NamedNode, QuadRef, Term, TripleRef};
use serde::Deserialize;

use super::*;

/// Most triples one import may carry.
const MAX_IMPORT_TRIPLES: usize = 500_000;
/// Rejected states listed in an import report.
const MAX_REPORTED: usize = 100;

fn negotiate(accept: Option<&str>) -> (RdfFormat, &'static str) {
    let a = accept.unwrap_or("").to_ascii_lowercase();
    if a.contains("application/n-triples") {
        (RdfFormat::NTriples, "application/n-triples")
    } else if a.contains("application/ld+json") {
        (
            RdfFormat::JsonLd {
                profile: JsonLdProfileSet::empty(),
            },
            "application/ld+json",
        )
    } else if a.contains("application/rdf+xml") {
        (RdfFormat::RdfXml, "application/rdf+xml")
    } else {
        (RdfFormat::Turtle, "text/turtle; charset=utf-8")
    }
}

fn content_format(content_type: Option<&str>) -> Result<RdfFormat, ApiErr> {
    let ct = content_type
        .unwrap_or("text/turtle")
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    match ct.as_str() {
        "" | "text/turtle" | "application/x-turtle" => Ok(RdfFormat::Turtle),
        "application/n-triples" | "text/plain" => Ok(RdfFormat::NTriples),
        "application/trig" => Ok(RdfFormat::TriG),
        "application/n-quads" => Ok(RdfFormat::NQuads),
        "application/rdf+xml" => Ok(RdfFormat::RdfXml),
        "application/ld+json" | "application/json" => Ok(RdfFormat::JsonLd {
            profile: JsonLdProfileSet::empty(),
        }),
        other => Err((
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            format!("`{other}` is not an RDF format this import reads"),
        )),
    }
}

/// GET /api/datasets/:id/properties/export
///
/// The dataset's property states as canonical OPM: `<item> <kind>
/// <property>`, `<property> a opm:Property ; opm:hasPropertyState <state>`,
/// each state with `schema:value`, `prov:generatedAtTime`,
/// `prov:wasAttributedTo`, its OPM classes, `opm:documentation`, notes,
/// derivations and `ots:validFrom`. Calculations kept in the states graph
/// come along. States whose value lives in a graph withheld from the caller
/// are left out.
pub async fn export_states(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    Path(dataset_id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ApiErr> {
    let uid = user.as_ref().map(|Extension(u)| u.user_id.as_str());
    let ds = visible_dataset(&state, uid, &dataset_id)?;
    let scope = read_scope(&state, uid, &ds)?;
    let states = states_graph(&dataset_id);
    let quads = if scope.graphs.contains(&states) {
        state
            .store
            .quads_for_graph(NamedNode::new(&states).map_err(e500)?.as_ref().into())
            .map_err(e500)?
    } else {
        Vec::new()
    };
    let (format, media) = negotiate(headers.get(header::ACCEPT).and_then(|v| v.to_str().ok()));
    let body = canonical_document(&quads, &scope.hidden, format).map_err(e500)?;
    Ok(([(header::CONTENT_TYPE, media)], body).into_response())
}

/// Serialise a states graph as canonical OPM.
pub(crate) fn canonical_document(
    quads: &[oxigraph::model::Quad],
    hidden_graphs: &HashSet<String>,
    format: RdfFormat,
) -> Result<Vec<u8>, String> {
    let property_of = format!("{OTS}propertyOf");
    let property_predicate = format!("{OTS}propertyPredicate");
    let data_graph = format!("{OTS}dataGraph");
    let has_state = format!("{OPM}hasPropertyState");
    let derived_from = format!("{PROV}wasDerivedFrom");
    let key = |t: &Term| t.to_string();

    let mut hidden_states: HashSet<String> = HashSet::new();
    type Link = (
        Option<oxigraph::model::NamedOrBlankNode>,
        Option<Term>,
        Option<Term>,
    );
    let mut links: BTreeMap<String, Link> = BTreeMap::new();
    let mut prop_states: HashMap<String, Vec<String>> = HashMap::new();
    let mut seq_owner: HashMap<String, String> = HashMap::new();
    for q in quads {
        let s = q.subject.to_string();
        let p = q.predicate.as_str();
        if p == data_graph {
            if let Term::NamedNode(g) = &q.object {
                if hidden_graphs.contains(g.as_str()) {
                    hidden_states.insert(s.clone());
                }
            }
        } else if p == property_of {
            let l = links.entry(s).or_default();
            l.0 = Some(q.subject.clone());
            l.1 = Some(q.object.clone());
        } else if p == property_predicate {
            let l = links.entry(s).or_default();
            l.0 = Some(q.subject.clone());
            l.2 = Some(q.object.clone());
        } else if p == has_state {
            prop_states.entry(s).or_default().push(key(&q.object));
        } else if p == derived_from {
            seq_owner.insert(key(&q.object), s);
        }
    }
    let visible_prop = |prop: &str| {
        prop_states
            .get(prop)
            .is_some_and(|sts| sts.iter().any(|st| !hidden_states.contains(st)))
    };

    let mut ser = RdfSerializer::from_format(format);
    for (prefix, ns) in [
        ("opm", OPM),
        ("schema", SCHEMA),
        ("prov", PROV),
        ("ots", OTS),
        ("xsd", XSD),
        ("rdfs", RDFS),
        ("rdf", RDF),
    ] {
        ser = ser.with_prefix(prefix, ns).map_err(|e| e.to_string())?;
    }
    let mut out = ser.for_writer(Vec::new());
    for (prop, (node, item, kind)) in &links {
        let (Some(node), Some(Term::NamedNode(item)), Some(Term::NamedNode(kind))) =
            (node, item, kind)
        else {
            continue;
        };
        if !visible_prop(prop) {
            continue;
        }
        out.serialize_triple(TripleRef::new(item, kind, node))
            .map_err(|e| e.to_string())?;
    }
    for q in quads {
        let p = q.predicate.as_str();
        if p == property_of || p == property_predicate || p == data_graph {
            continue;
        }
        let s = q.subject.to_string();
        if hidden_states.contains(&s)
            || (links.contains_key(&s) && !visible_prop(&s))
            || (p == has_state && hidden_states.contains(&key(&q.object)))
            || seq_owner
                .get(&s)
                .is_some_and(|owner| hidden_states.contains(owner))
        {
            continue;
        }
        out.serialize_triple(TripleRef::new(&q.subject, &q.predicate, &q.object))
            .map_err(|e| e.to_string())?;
    }
    out.finish().map_err(|e| e.to_string())
}

#[derive(Debug, Deserialize)]
pub struct ImportQuery {
    /// The data graph the imported current values go to (default: the
    /// dataset's instances graph).
    pub graph: Option<String>,
}

/// One state ready to write.
struct ImportedState {
    iri: String,
    stored: StoredState,
    seq: Option<(String, Vec<String>)>,
}

/// POST /api/datasets/:id/properties/import
///
/// Canonical OPM in (Turtle by default; N-Triples, TriG, N-Quads, RDF/XML
/// or JSON-LD by `Content-Type`; named graphs are merged). Every
/// `<item> <kind> <property>` with `opm:hasPropertyState` (and the server's
/// own `ots:propertyOf` form) is read; each state keeps its IRI (a blank
/// node gets one), a state already in the dataset is skipped, and a state
/// without `prov:generatedAtTime` — or, unless it is `opm:Deleted`, without
/// a value — is rejected, as OPM requires. Per property the newest-recorded
/// state becomes current (OPM: "the most recently defined") and its value
/// the plain triple in the data graph. One commit.
pub async fn import_states(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(dataset_id): Path<String>,
    Query(q): Query<ImportQuery>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<impl IntoResponse, ApiErr> {
    let ds = writable_dataset(&state, &user, &dataset_id)?;
    let format = content_format(
        headers
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
    )?;
    let data_graph = match &q.graph {
        Some(g) => registered_data_graph(&state, &dataset_id, g)?,
        None => default_data_graph(&state, &dataset_id).ok_or_else(|| {
            bad("the dataset has no graph to hold the current values; register one or pass `graph`")
        })?,
    };

    // Parse into a scratch store, every quad in its default graph.
    let scratch = oxigraph::store::Store::new().map_err(e500)?;
    let mut count = 0usize;
    for quad in RdfParser::from_format(format).for_slice(&body) {
        let quad = quad.map_err(|e| bad(format!("the body does not parse: {e}")))?;
        count += 1;
        if count > MAX_IMPORT_TRIPLES {
            return Err((
                StatusCode::PAYLOAD_TOO_LARGE,
                format!("an import carries at most {MAX_IMPORT_TRIPLES} triples"),
            ));
        }
        scratch
            .insert(QuadRef::new(
                quad.subject.as_ref(),
                quad.predicate.as_ref(),
                quad.object.as_ref(),
                GraphNameRef::DefaultGraph,
            ))
            .map_err(e500)?;
    }
    let solutions = |q: &str| -> Result<Vec<QuerySolution>, ApiErr> {
        let results = oxigraph::sparql::SparqlEvaluator::new()
            .parse_query(q)
            .map_err(e500)?
            .on_store(&scratch)
            .execute()
            .map_err(e500)?;
        match results {
            oxigraph::sparql::QueryResults::Solutions(s) => {
                s.collect::<Result<Vec<_>, _>>().map_err(e500)
            }
            _ => Ok(Vec::new()),
        }
    };
    let found = collect_states(&solutions(&states_select())?);
    if found.is_empty() {
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            "no OPM property states found: expected `<item> <kind> <property>` with `<property> opm:hasPropertyState <state>`".to_string(),
        ));
    }
    // Derivation sequences, members in order.
    let mut seqs: HashMap<String, Vec<(u32, Term)>> = HashMap::new();
    for row in solutions(&format!(
        "PREFIX prov: <{PROV}> SELECT ?seq ?n ?m WHERE {{ ?s prov:wasDerivedFrom ?seq . ?seq ?n ?m . FILTER(STRSTARTS(STR(?n), \"{RDF}_\")) }}"
    ))? {
        let (Some(seq), Some(Term::NamedNode(n)), Some(m)) = (row.get("seq"), row.get("n"), row.get("m")) else {
            continue;
        };
        if let Some(i) = n.as_str().strip_prefix(&format!("{RDF}_")).and_then(|i| i.parse().ok()) {
            seqs.entry(seq.to_string()).or_default().push((i, m.clone()));
        }
    }

    let states_g = ensure_states_graph(&state, &dataset_id)?;
    let scope = read_scope(&state, Some(&user.user_id), &ds)?;
    let existing_states: HashSet<String> = read_states(&state, &scope, None, None)?
        .into_iter()
        .filter(|s| !s.canonical)
        .map(|s| s.iri)
        .collect();

    // Blank nodes get IRIs; the map keeps derivation members pointing at them.
    let mut minted: HashMap<String, String> = HashMap::new();
    let mut name = |raw: &str, kind: &str| -> String {
        if let Some(b) = raw.strip_prefix("_:") {
            minted
                .entry(b.to_string())
                .or_insert_with(|| format!("urn:ots:{kind}:{}", uuid::Uuid::new_v4()))
                .clone()
        } else {
            raw.to_string()
        }
    };
    let mut rejected: Vec<serde_json::Value> = Vec::new();
    let mut duplicates = 0usize;
    let mut by_property: BTreeMap<(String, String), (String, Vec<ImportedState>)> = BTreeMap::new();
    for s in found {
        let reject = |reason: &str| serde_json::json!({ "state": s.iri, "reason": reason });
        if s.entity.starts_with("_:") {
            rejected.push(reject(
                "the item is a blank node; it cannot be addressed afterwards",
            ));
            continue;
        }
        if s.recorded_at.is_empty() || instant(&s.recorded_at).is_none() {
            rejected.push(reject("no prov:generatedAtTime (OPM requires one)"));
            continue;
        }
        if !s.deleted && s.value.is_none() {
            rejected.push(reject("no schema:value and not opm:Deleted"));
            continue;
        }
        if matches!(s.value, Some(Term::BlankNode(_))) {
            rejected.push(reject("the value is a blank node"));
            continue;
        }
        let iri = name(&s.iri, "property-state");
        if existing_states.contains(&iri) {
            duplicates += 1;
            continue;
        }
        let seq = match s.derived_from.as_ref() {
            Some(raw) => {
                let key = if raw.starts_with("_:") {
                    raw.clone()
                } else {
                    format!("<{raw}>")
                };
                let mut members = seqs.get(&key).cloned().unwrap_or_default();
                members.sort_by_key(|(i, _)| *i);
                let members: Vec<String> = members
                    .iter()
                    .map(|(_, t)| name(&term_string(t), "property-state"))
                    .collect();
                Some((name(raw, "derivation"), members))
            }
            None => None,
        };
        let prop = name(&s.prop, "property");
        by_property
            .entry((s.entity.clone(), s.property.clone()))
            .or_insert_with(|| (prop, Vec::new()))
            .1
            .push(ImportedState {
                iri,
                stored: s,
                seq,
            });
    }
    let errors = rejected.len();
    rejected.truncate(MAX_REPORTED);

    let agent = agent_iri(&state, &user.user_id);
    let lit = |s: &str| format!("\"{}\"", escape_sparql_literal(s));
    let mut statements = Vec::new();
    let mut touched_graphs: Vec<String> = vec![data_graph.clone()];
    let mut imported = 0usize;
    for ((entity, property), (imported_prop, list)) in &by_property {
        let current_existing = {
            let (_, existing) = super::states_of_property(&state, &scope, entity, property)?;
            existing
                .into_iter()
                .filter(|s| !s.canonical)
                .collect::<Vec<_>>()
        };
        // A property the dataset already has keeps its node; a new one keeps
        // the imported node's IRI unless that IRI is the server's node of
        // another property.
        let prop = if current_existing.is_empty() {
            let owner = managed_property_node(&state, &states_g, entity, property)?;
            if owner == property_iri(entity, property)
                && !node_in_use(&state, &states_g, imported_prop)?
            {
                imported_prop.clone()
            } else {
                owner
            }
        } else {
            managed_property_node(&state, &states_g, entity, property)?
        };
        let e = escape_sparql_iri(entity);
        let p = escape_sparql_iri(property);
        let prop_e = escape_sparql_iri(&prop);
        let dg = escape_sparql_iri(&data_graph);
        let mut triples = vec![format!(
            "<{prop_e}> a opm:Property ; ots:propertyOf <{e}> ; ots:propertyPredicate <{p}>"
        )];
        // The newest-recorded state of all, old and new, is current.
        let winner_new = list.iter().max_by_key(|s| s.stored.recorded_key());
        let winner_old = current_existing.iter().max_by_key(|s| s.recorded_key());
        let new_wins = match (winner_new, winner_old) {
            (Some(n), Some(o)) => n.stored.recorded_key() > o.recorded_key(),
            (Some(_), None) => true,
            _ => false,
        };
        for s in list {
            let st = escape_sparql_iri(&s.iri);
            let r = &s.stored;
            let is_winner = new_wins && winner_new.is_some_and(|w| w.iri == s.iri);
            triples.push(format!("<{prop_e}> opm:hasPropertyState <{st}>"));
            triples.push(format!(
                "<{st}> a opm:PropertyState, {}",
                if is_winner {
                    "opm:CurrentPropertyState"
                } else {
                    "opm:OutdatedPropertyState"
                }
            ));
            triples.push(format!(
                "<{st}> prov:generatedAtTime \"{}\"^^xsd:dateTime",
                r.recorded_at
            ));
            triples.push(format!(
                "<{st}> ots:validFrom \"{}\"^^xsd:dateTime",
                r.valid_from
            ));
            triples.push(format!("<{st}> ots:dataGraph <{dg}>"));
            match &r.attributed_to {
                Some(who) if NamedNode::new(who).is_ok() => triples.push(format!(
                    "<{st}> prov:wasAttributedTo <{}>",
                    escape_sparql_iri(who)
                )),
                _ => triples.push(format!(
                    "<{st}> prov:wasAttributedTo <{}>",
                    escape_sparql_iri(&agent)
                )),
            }
            if let Some(v) = &r.value {
                triples.push(format!("<{st}> schema:value {v}"));
            }
            if r.deleted {
                triples.push(format!("<{st}> a opm:Deleted"));
            }
            for rel in &r.reliability {
                triples.push(format!("<{st}> a opm:{}", rel.class()));
            }
            for d in &r.documentation {
                if NamedNode::new(d).is_ok() {
                    triples.push(format!(
                        "<{st}> opm:documentation <{}>",
                        escape_sparql_iri(d)
                    ));
                }
            }
            if let Some(n) = &r.note {
                triples.push(format!("<{st}> rdfs:comment {}", lit(n)));
            }
            if let Some(x) = &r.expression {
                triples.push(format!("<{st}> opm:expression {}", lit(x)));
            }
            if let Some(c) = r
                .calculation
                .as_ref()
                .filter(|c| NamedNode::new(c.as_str()).is_ok())
            {
                triples.push(format!("<{st}> ots:calculation <{}>", escape_sparql_iri(c)));
            }
            if let Some(rev) = r
                .revision_of
                .as_ref()
                .filter(|c| NamedNode::new(c.as_str()).is_ok())
            {
                triples.push(format!(
                    "<{st}> prov:wasRevisionOf <{}>",
                    escape_sparql_iri(rev)
                ));
            }
            if let Some((seq, members)) = &s.seq {
                let seq = escape_sparql_iri(seq);
                triples.push(format!("<{st}> prov:wasDerivedFrom <{seq}>"));
                triples.push(format!("<{seq}> a rdf:Seq"));
                for (i, m) in members.iter().enumerate() {
                    if NamedNode::new(m.as_str()).is_ok() {
                        triples.push(format!("<{seq}> rdf:_{} <{}>", i + 1, escape_sparql_iri(m)));
                    }
                }
            }
            imported += 1;
        }
        statements.push(format!(
            "{}\nINSERT DATA {{ GRAPH <{}> {{\n  {} .\n}} }}",
            prefixes(),
            escape_sparql_iri(&states_g),
            triples.join(" .\n  ")
        ));
        if new_wins {
            let winner = winner_new.expect("new wins");
            // Every older current state of the property is outdated now.
            for old in current_existing.iter().filter(|s| s.current) {
                let st = escape_sparql_iri(&old.iri);
                statements.push(format!(
                    "{}\nDELETE DATA {{ GRAPH <{sg}> {{ <{st}> a opm:CurrentPropertyState }} }} ;\nINSERT DATA {{ GRAPH <{sg}> {{ <{st}> a opm:OutdatedPropertyState }} }}",
                    prefixes(),
                    sg = escape_sparql_iri(&states_g),
                ));
                if let Some(g) = old.data_graph.as_ref().filter(|g| **g != data_graph) {
                    if registered_data_graph(&state, &dataset_id, g).is_ok() {
                        statements.push(replace_plain(g, &e, &p, None));
                        touched_graphs.push(g.clone());
                    }
                }
            }
            let value = if winner.stored.deleted {
                None
            } else {
                winner.stored.value.as_ref().map(|v| v.to_string())
            };
            statements.push(replace_plain(&data_graph, &e, &p, value.as_deref()));
        }
    }
    let properties = by_property.len();
    super::apply_writes(
        &state,
        &dataset_id,
        &user.user_id,
        statements,
        touched_graphs,
        format!("OPM import: {imported} states of {properties} properties"),
        imported,
    )
    .await?;
    Ok((
        StatusCode::OK,
        Json(serde_json::json!({
            "dataset_id": dataset_id,
            "data_graph": data_graph,
            "states_graph": states_g,
            "properties": properties,
            "imported_states": imported,
            "skipped_duplicates": duplicates,
            "rejected_count": errors,
            "rejected": rejected,
        })),
    ))
}

fn prefixes() -> String {
    format!(
        "PREFIX opm: <{OPM}>\nPREFIX schema: <{SCHEMA}>\nPREFIX prov: <{PROV}>\nPREFIX ots: <{OTS}>\nPREFIX xsd: <{XSD}>\nPREFIX rdfs: <{RDFS}>\nPREFIX rdf: <{RDF}>"
    )
}

/// Replace the plain value(s) of `<e> <p>` in `graph` (escaped IRIs) with
/// `value` (SPARQL term text), or remove them. A canonical property node
/// linked there stays.
fn replace_plain(graph: &str, e: &str, p: &str, value: Option<&str>) -> String {
    let g = escape_sparql_iri(graph);
    let insert = value
        .map(|v| format!("INSERT {{ GRAPH <{g}> {{ <{e}> <{p}> {v} }} }}"))
        .unwrap_or_default();
    format!(
        "{}\nDELETE {{ GRAPH <{g}> {{ <{e}> <{p}> ?old }} }}\n{insert}\nWHERE {{ OPTIONAL {{ GRAPH <{g}> {{ <{e}> <{p}> ?old FILTER NOT EXISTS {{ ?old opm:hasPropertyState ?anyState }} }} }} }}",
        prefixes()
    )
}

/// Whether `node` is already the server's node of some property.
fn node_in_use(state: &AppState, states: &str, node: &str) -> Result<bool, ApiErr> {
    let Ok(n) = NamedNode::new(node) else {
        return Ok(true);
    };
    Ok(state
        .store
        .store()
        .quads_for_pattern(
            Some(n.as_ref().into()),
            Some(
                NamedNode::new(format!("{OTS}propertyOf"))
                    .map_err(e500)?
                    .as_ref(),
            ),
            None,
            Some(NamedNode::new(states).map_err(e500)?.as_ref().into()),
        )
        .next()
        .is_some())
}
