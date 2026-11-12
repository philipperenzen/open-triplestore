//! Statement-level provenance (docs/notes/delta-versioning-design.md §6).
//!
//! For a dataset that opts in (`dataset_settings`, `provenance: minimal |
//! full`), every change to one of its statements is annotated in RDF 1.2 in
//! the dataset's statement-provenance graph
//! (`urn:ots:statement-provenance:{id}`, registered with the Provenance role):
//!
//! ```text
//! <{base}/statement/{h}> rdf:reifies <<( s p o )>> ; ots:graph <g> .   # once per statement
//! <{base}/statement/{h}> ots:change <{base}/statement/{h}/{seq}> .
//! <{base}/statement/{h}/{seq}> ots:op "add" ;                         # or "remove"
//!     ots:seq "…"^^xsd:integer ; prov:wasGeneratedBy <commit> .
//! ```
//!
//! `{h}` is a 128-bit digest of the statement and its graph, so the reifier
//! is the same IRI every time the statement is touched; each change is its
//! own event node, so a removal and a re-addition keep both histories. The
//! `full` profile adds `prov:wasAssociatedWith <actor>` and `prov:atTime`
//! for readers who cannot join the commit trail. A statement in an
//! entailment graph adds `ots:derivedBy <regime>`. A statement with a blank
//! node is not annotated — a reifier hashed over a blank-node label is stable
//! only inside one store — unless the dataset imports with Skolem IRIs; each
//! skip is recorded as an `ots:ProvenanceGap` event, as is a change the
//! change log could not carry.
//!
//! An annotation is the statement itself, so it is read by whoever reads the
//! statement and nobody else: the annotations of a graph the dataset holds
//! privately go to a second graph, `urn:ots:statement-provenance:{id}:private`,
//! registered private (its readers are the dataset's writers). The flag that
//! counts is the graph's when the change is annotated. A graph made private
//! later takes its annotations along ([`follow_private_graph`]): in the
//! request that makes it private, and again on every tick
//! ([`follow_private_graphs`]), which covers the paths that flag a graph
//! without such a request (a report graph that follows the graph it reports
//! on, a pipeline target) — also for a dataset that no longer records
//! provenance, and while change capture is off. A graph made public
//! again is annotated in the public graph from then on; what was annotated
//! while it was private stays private, since it holds the statements removed
//! in that time.
//!
//! Written by a change-log consumer ([`tick`]) behind the data commit, on its
//! own cursor (`statement-provenance`): the annotations are a separate store
//! write, ids are deterministic, so a replay after a crash rewrites the same
//! quads. A dataset that opts in is annotated from the change log's head on,
//! not retroactively. Nothing happens while change capture is off.

use std::collections::{BTreeSet, HashMap, HashSet};

use oxigraph::model::{
    GraphName, GraphNameRef, Literal, NamedNode, NamedOrBlankNode, NamedOrBlankNodeRef, Quad, Term,
    TermRef,
};
use sha2::{Digest, Sha256};

use crate::dataset_settings::{self, ProvenanceProfile};
use crate::provenance::{private_statement_provenance_graph, statement_provenance_graph};
use crate::server::AppState;
use crate::store::changes::{decode_text, ChangeRow, Extent, State};
use crate::store::QuadOp;

pub const CURSOR: &str = "statement-provenance";
const BATCH: usize = 2_000;
/// Reifiers and gap nodes [`follow_private_graph`] moves per transaction.
const MOVE_BATCH: usize = 500;
/// The `app_settings` key [`split_existing_annotations`] sets once it has
/// gone through every dataset.
const SPLIT_MARKER: &str = "cleanup.statement_provenance_private.v1";
const OTS: &str = "https://opentriplestore.org/ns#";
const PROV: &str = "http://www.w3.org/ns/prov#";
const RDF_REIFIES: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#reifies";
const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const XSD_INTEGER: &str = "http://www.w3.org/2001/XMLSchema#integer";
const XSD_DATETIME: &str = "http://www.w3.org/2001/XMLSchema#dateTime";

/// Register the dataset's statement-provenance graph (Provenance role) if it
/// is not yet.
pub fn ensure_graph(state: &AppState, dataset_id: &str) -> anyhow::Result<String> {
    let g = statement_provenance_graph(dataset_id);
    let known = state
        .auth_db
        .list_dataset_graphs(dataset_id)?
        .iter()
        .any(|x| x == &g);
    if !known {
        state.auth_db.add_dataset_graph(dataset_id, &g)?;
        state.auth_db.set_dataset_graph_role(
            dataset_id,
            &g,
            Some(crate::auth::models::GraphKind::Provenance),
        )?;
    }
    Ok(g)
}

/// Register the dataset's private statement-provenance graph (Provenance
/// role, private) if it is not yet, and make it private again should the
/// flag have been cleared. Call it before the graph's first quad is written:
/// a graph no dataset holds is read by everyone.
pub fn ensure_private_graph(state: &AppState, dataset_id: &str) -> anyhow::Result<String> {
    let g = private_statement_provenance_graph(dataset_id);
    let private = state
        .auth_db
        .list_dataset_graph_entries(dataset_id)?
        .iter()
        .any(|e| e.graph_iri == g && e.private);
    if !private {
        state.auth_db.add_private_dataset_graph(
            dataset_id,
            &g,
            Some(crate::auth::models::GraphKind::Provenance),
        )?;
    }
    Ok(g)
}

/// `graph_iri` is a graph `dataset_id` holds privately: move what the
/// dataset's statement-provenance graph says about it — its statements'
/// reifiers with their change events, and its gap nodes — to the private
/// statement-provenance graph. Returns the quads moved; nothing is written
/// when there is nothing to move. Each batch is one transaction, so a reifier
/// is in one graph or the other, and a call that fails part-way is completed
/// by the next one.
pub fn follow_private_graph(
    state: &AppState,
    dataset_id: &str,
    graph_iri: &str,
) -> anyhow::Result<usize> {
    if crate::provenance::is_any_statement_provenance_graph(graph_iri) {
        return Ok(0);
    }
    let from = nn(&statement_provenance_graph(dataset_id));
    let to = nn(&private_statement_provenance_graph(dataset_id));
    let about = nn(graph_iri);
    let ots_graph = nn(&format!("{OTS}graph"));
    let ots_change = nn(&format!("{OTS}change"));
    let from_ref = GraphNameRef::NamedNode(from.as_ref());
    let store = state.store.store();
    let mut registered = false;
    let mut moved = 0usize;
    loop {
        let subjects = store
            .quads_for_pattern(
                None,
                Some(ots_graph.as_ref()),
                Some(TermRef::NamedNode(about.as_ref())),
                Some(from_ref),
            )
            .take(MOVE_BATCH)
            .map(|q| q.map(|q| q.subject))
            .collect::<Result<Vec<NamedOrBlankNode>, _>>()?;
        if subjects.is_empty() {
            return Ok(moved);
        }
        if !registered {
            ensure_private_graph(state, dataset_id)?;
            registered = true;
        }
        let mut quads: Vec<Quad> = Vec::new();
        for s in &subjects {
            for q in store.quads_for_pattern(Some(s.as_ref()), None, None, Some(from_ref)) {
                let q = q?;
                if q.predicate == ots_change {
                    if let Term::NamedNode(event) = &q.object {
                        for e in store.quads_for_pattern(
                            Some(NamedOrBlankNodeRef::NamedNode(event.as_ref())),
                            None,
                            None,
                            Some(from_ref),
                        ) {
                            quads.push(e?);
                        }
                    }
                }
                quads.push(q);
            }
        }
        let ops: Vec<QuadOp> = quads
            .into_iter()
            .flat_map(|q| {
                let there = Quad::new(
                    q.subject.clone(),
                    q.predicate.clone(),
                    q.object.clone(),
                    to.clone(),
                );
                [QuadOp::Add(there), QuadOp::Remove(q)]
            })
            .collect();
        // Every subject of the batch loses its `ots:graph` quad here, so the
        // next read starts behind it.
        let (_, removed) = state.store.apply_quad_ops(&ops)?;
        moved += removed.len();
    }
}

/// [`follow_private_graph`] for every graph a dataset holds privately beside
/// a statement-provenance graph: whatever flagged the graph, whether or not
/// the dataset still records provenance. One identity-database query and an
/// index probe per such graph when nothing is to move. Returns the quads
/// moved.
pub fn follow_private_graphs(state: &AppState) -> anyhow::Result<usize> {
    let mut moved = 0usize;
    for (dataset_id, graph) in state
        .auth_db
        .list_private_graphs_with_statement_provenance()?
    {
        moved += follow_private_graph(state, &dataset_id, &graph)?;
    }
    Ok(moved)
}

/// Whether [`split_existing_annotations`] has completed on this identity
/// database (a lookup error counts as not yet).
pub fn existing_annotations_split(db: &crate::auth::db::AuthDb) -> bool {
    matches!(db.get_app_setting(SPLIT_MARKER), Ok(Some(_)))
}

/// One-time pass over annotations written before datasets had a private
/// statement-provenance graph, when every graph's annotations went to the one
/// graph all the dataset's readers read. For every dataset with a registered
/// statement-provenance graph, whether it still records provenance or not,
/// the annotations of each graph the dataset does not hold as a non-private
/// graph move to the private graph ([`follow_private_graph`]): a graph it
/// holds privately, and a graph it no longer holds, whose flag nobody can
/// tell any more. Returns the quads moved, and sets the marker
/// ([`existing_annotations_split`]) when every dataset went through.
///
/// What it cannot find: annotations made while a graph was private, of a
/// graph that is public again by now. Nothing tells them from the rest.
pub fn split_existing_annotations(state: &AppState) -> anyhow::Result<usize> {
    let db = &state.auth_db;
    let ots_graph = nn(&format!("{OTS}graph"));
    let mut moved = 0usize;
    for (dataset_id, graph, _) in db.list_all_dataset_graph_rows()? {
        if graph != statement_provenance_graph(&dataset_id) {
            continue;
        }
        let open: HashSet<String> = db
            .list_dataset_graph_entries(&dataset_id)?
            .into_iter()
            .filter(|e| !e.private)
            .map(|e| e.graph_iri)
            .collect();
        let from = nn(&graph);
        let mut annotated: BTreeSet<String> = BTreeSet::new();
        for q in state.store.store().quads_for_pattern(
            None,
            Some(ots_graph.as_ref()),
            None,
            Some(GraphNameRef::NamedNode(from.as_ref())),
        ) {
            if let Term::NamedNode(g) = q?.object {
                if !open.contains(g.as_str()) {
                    annotated.insert(g.into_string());
                }
            }
        }
        for g in annotated {
            moved += follow_private_graph(state, &dataset_id, &g)?;
        }
    }
    db.set_app_setting(SPLIT_MARKER, &chrono::Utc::now().to_rfc3339())?;
    Ok(moved)
}

/// The reifier of statement `(g s p o)`: `{base}/statement/{h}`, `h` the
/// first 128 bits of the SHA-256 of the canonical quad.
pub fn reifier(base_url: &str, quad: &Quad) -> String {
    let g = match &quad.graph_name {
        GraphName::NamedNode(n) => n.as_str().to_string(),
        _ => String::new(),
    };
    let line = format!("<{g}> {} {} {}", quad.subject, quad.predicate, quad.object);
    let digest = Sha256::digest(line.as_bytes());
    let h: String = digest[..16].iter().map(|b| format!("{b:02x}")).collect();
    format!("{}/statement/{h}", base_url.trim_end_matches('/'))
}

fn nn(iri: &str) -> NamedNode {
    NamedNode::new_unchecked(iri)
}

fn has_blank(q: &Quad) -> bool {
    matches!(q.subject, NamedOrBlankNode::BlankNode(_)) || matches!(q.object, Term::BlankNode(_))
}

/// The annotation quads of one change row of `graph` (in `dataset`'s
/// provenance graph `target`), and the number of statements skipped.
fn annotate(
    base_url: &str,
    row: &ChangeRow,
    target: &GraphName,
    profile: ProvenanceProfile,
    out: &mut Vec<Quad>,
) -> usize {
    let base = base_url.trim_end_matches('/');
    let seq = row.seq.unwrap_or_default();
    let int = |n: i64| Term::Literal(Literal::new_typed_literal(n.to_string(), nn(XSD_INTEGER)));
    let mut push = |s: &str, p: &str, o: Term| {
        out.push(Quad::new(nn(s), nn(p), o, target.clone()));
    };
    if row.extent != Extent::Full || row.state != State::Committed {
        // A change the log could not carry: what changed is unknown here.
        let gap = format!("{base}/statement/gap/{}/{seq}", row.epoch);
        push(&gap, RDF_TYPE, nn(&format!("{OTS}ProvenanceGap")).into());
        push(&gap, &format!("{OTS}seq"), int(seq));
        if let Some(g) = &row.graph_iri {
            push(&gap, &format!("{OTS}graph"), nn(g).into());
        }
        return 0;
    }
    let derived_by = row
        .graph_iri
        .as_deref()
        .and_then(|g| g.strip_prefix("urn:entailment:"))
        .and_then(|rest| rest.split_once(':'))
        .map(|(regime, _)| format!("{OTS}regime/{regime}"));
    let mut skipped = 0usize;
    let ops = decode_text(row.added.as_deref())
        .into_iter()
        .map(|q| ("add", q))
        .chain(
            decode_text(row.removed.as_deref())
                .into_iter()
                .map(|q| ("remove", q)),
        );
    for (op, q) in ops {
        if has_blank(&q) {
            skipped += 1;
            continue;
        }
        let r = reifier(base_url, &q);
        let event = format!("{r}/{seq}");
        #[cfg(feature = "rdf-12")]
        {
            let triple = oxigraph::model::Triple::new(
                q.subject.clone(),
                q.predicate.clone(),
                q.object.clone(),
            );
            push(&r, RDF_REIFIES, Term::Triple(Box::new(triple)));
        }
        if let GraphName::NamedNode(g) = &q.graph_name {
            push(&r, &format!("{OTS}graph"), g.clone().into());
        }
        push(&r, &format!("{OTS}change"), nn(&event).into());
        push(
            &event,
            &format!("{OTS}op"),
            Literal::new_simple_literal(op).into(),
        );
        push(&event, &format!("{OTS}seq"), int(seq));
        if let Some(c) = &row.commit_iri {
            push(&event, &format!("{PROV}wasGeneratedBy"), nn(c).into());
        }
        if let Some(regime) = &derived_by {
            push(&event, &format!("{OTS}derivedBy"), nn(regime).into());
        }
        if profile == ProvenanceProfile::Full {
            if let Some(a) = &row.actor_iri {
                push(&event, &format!("{PROV}wasAssociatedWith"), nn(a).into());
            }
            push(
                &event,
                &format!("{PROV}atTime"),
                Term::Literal(Literal::new_typed_literal(
                    row.created_at.clone(),
                    nn(XSD_DATETIME),
                )),
            );
        }
    }
    if skipped > 0 {
        let gap = format!("{base}/statement/gap/{}/{seq}", row.epoch);
        push(&gap, RDF_TYPE, nn(&format!("{OTS}ProvenanceGap")).into());
        push(&gap, &format!("{OTS}seq"), int(seq));
        // Its graph, as on every annotation: what goes with the graph when
        // it is made private is found by it.
        if let Some(g) = &row.graph_iri {
            push(&gap, &format!("{OTS}graph"), nn(g).into());
        }
        push(
            &gap,
            &format!("{OTS}skippedBlankNodeStatements"),
            int(skipped as i64),
        );
    }
    skipped
}

/// The dataset a data graph is annotated for, and how it holds the graph.
struct Owner {
    dataset: String,
    profile: ProvenanceProfile,
    private: bool,
}

/// Move the annotations of graphs that are private by now, then annotate the
/// changes since the consumer's cursor. Blocking. Returns the annotation
/// quads written. The move does not depend on change capture: annotations
/// written while it was on are there after it is turned off.
pub fn tick(state: &AppState) -> usize {
    // Before anything else is written: a tick that cannot move them
    // annotates nothing, and tries again.
    if let Err(e) = follow_private_graphs(state) {
        tracing::warn!("statement provenance of private graphs not moved: {e}");
        return 0;
    }
    let log = state.store.changes();
    if !log.enabled() || !cfg!(feature = "rdf-12") {
        return 0;
    }
    let datasets = dataset_settings::with_provenance(&state.auth_db);
    // Which dataset each data graph belongs to (never a provenance graph),
    // and whether that dataset holds it privately. A dataset whose graphs
    // cannot be listed is not annotated this tick: without the flags its
    // annotations have no graph they may safely go to.
    let mut owner: HashMap<String, Owner> = HashMap::new();
    for (ds, profile) in &datasets {
        match state.auth_db.list_dataset_graph_entries(ds) {
            Ok(entries) => {
                for e in entries {
                    if !crate::provenance::is_any_statement_provenance_graph(&e.graph_iri) {
                        owner.insert(
                            e.graph_iri,
                            Owner {
                                dataset: ds.clone(),
                                profile: *profile,
                                private: e.private,
                            },
                        );
                    }
                }
            }
            Err(e) => tracing::warn!("statement provenance: graphs of {ds} not listed: {e}"),
        }
    }
    let head = log.last_seq();
    let since = match log.cursor(CURSOR) {
        Some(c) if !datasets.is_empty() => c.seq,
        // Nothing to annotate, or the first tick: from the head on.
        _ => {
            let _ = log.set_cursor(CURSOR, head, Some("system"));
            return 0;
        }
    };
    let rows = log.rows_after(since, BATCH);
    let Some(last) = rows.last().and_then(|r| r.seq) else {
        let _ = log.set_cursor(CURSOR, since, Some("system"));
        return 0;
    };
    // Per dataset, the annotations for its readers and those for whoever
    // reads its private graphs.
    let mut by_target: HashMap<(String, bool), Vec<Quad>> = HashMap::new();
    for row in &rows {
        let Some(o) = row.graph_iri.as_deref().and_then(|g| owner.get(g)) else {
            continue;
        };
        let target = if o.private {
            private_statement_provenance_graph(&o.dataset)
        } else {
            statement_provenance_graph(&o.dataset)
        };
        let out = by_target.entry((o.dataset.clone(), o.private)).or_default();
        annotate(
            &state.base_url,
            row,
            &GraphName::NamedNode(nn(&target)),
            o.profile,
            out,
        );
    }
    let mut written = 0usize;
    for ((ds, private), quads) in by_target {
        if quads.is_empty() {
            continue;
        }
        let registered = if private {
            ensure_private_graph(state, &ds)
        } else {
            ensure_graph(state, &ds)
        };
        if let Err(e) = registered {
            tracing::warn!("statement provenance: graph of {ds} not registered: {e}");
            continue;
        }
        // Chunked: an annotation set can be large, and re-applying one is a
        // no-op (the reifiers are named), so a retry completes a part-write.
        let ops: Vec<QuadOp> = quads.into_iter().map(QuadOp::Add).collect();
        match state.store.apply_quad_ops_chunked(&ops) {
            Ok((inserted, _)) => written += inserted.len(),
            Err(e) => {
                tracing::warn!("statement provenance for {ds} not written: {e}");
                // Not past what failed: the next tick tries again.
                return written;
            }
        }
    }
    let _ = log.set_cursor(CURSOR, last, Some("system"));
    written
}
