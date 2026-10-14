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
//! Written by a change-log consumer ([`tick`]) behind the data commit, on its
//! own cursor (`statement-provenance`): the annotations are a separate store
//! write, ids are deterministic, so a replay after a crash rewrites the same
//! quads. A dataset that opts in is annotated from the change log's head on,
//! not retroactively. Nothing happens while change capture is off.

use std::collections::HashMap;

use oxigraph::model::{GraphName, Literal, NamedNode, NamedOrBlankNode, Quad, Term};
use sha2::{Digest, Sha256};

use crate::dataset_settings::{self, ProvenanceProfile};
use crate::server::AppState;
use crate::store::changes::{decode_text, ChangeRow, Extent, State};

pub const CURSOR: &str = "statement-provenance";
const BATCH: usize = 2_000;
const OTS: &str = "https://opentriplestore.org/ns#";
const PROV: &str = "http://www.w3.org/ns/prov#";
const RDF_REIFIES: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#reifies";
const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const XSD_INTEGER: &str = "http://www.w3.org/2001/XMLSchema#integer";
const XSD_DATETIME: &str = "http://www.w3.org/2001/XMLSchema#dateTime";

/// Register the dataset's statement-provenance graph (Provenance role) if it
/// is not yet.
pub fn ensure_graph(state: &AppState, dataset_id: &str) -> anyhow::Result<String> {
    let g = crate::provenance::statement_provenance_graph(dataset_id);
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
        push(
            &gap,
            &format!("{OTS}skippedBlankNodeStatements"),
            int(skipped as i64),
        );
    }
    skipped
}

/// Annotate the changes since the consumer's cursor. Blocking. Returns the
/// annotation quads written.
pub fn tick(state: &AppState) -> usize {
    let log = state.store.changes();
    if !log.enabled() || !cfg!(feature = "rdf-12") {
        return 0;
    }
    let datasets = dataset_settings::with_provenance(&state.auth_db);
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
    // Which dataset each data graph belongs to (never a provenance graph).
    let mut owner: HashMap<String, (String, ProvenanceProfile)> = HashMap::new();
    for (ds, profile) in &datasets {
        if let Ok(graphs) = state.auth_db.list_dataset_graphs(ds) {
            for g in graphs {
                if !crate::provenance::is_any_statement_provenance_graph(&g) {
                    owner.insert(g, (ds.clone(), *profile));
                }
            }
        }
    }
    let mut by_dataset: HashMap<String, Vec<Quad>> = HashMap::new();
    for row in &rows {
        let Some((ds, profile)) = row.graph_iri.as_deref().and_then(|g| owner.get(g)) else {
            continue;
        };
        let target = GraphName::NamedNode(nn(&crate::provenance::statement_provenance_graph(ds)));
        let out = by_dataset.entry(ds.clone()).or_default();
        annotate(&state.base_url, row, &target, *profile, out);
    }
    let mut written = 0usize;
    for (ds, quads) in by_dataset {
        if quads.is_empty() {
            continue;
        }
        if let Err(e) = ensure_graph(state, &ds) {
            tracing::warn!("statement provenance: graph of {ds} not registered: {e}");
            continue;
        }
        // Chunked: an annotation set can be large, and re-applying one is a
        // no-op (the reifiers are named), so a retry completes a part-write.
        let ops: Vec<crate::store::QuadOp> =
            quads.into_iter().map(crate::store::QuadOp::Add).collect();
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
