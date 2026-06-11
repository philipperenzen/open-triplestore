//! Bulk import core: parse N files in parallel, then a single bulk insert.

use oxigraph::io::RdfFormat;
use oxigraph::model::{GraphName, NamedNode, Quad};
use serde::{Deserialize, Serialize};

use crate::auth::models::GraphKind;
use crate::data_models::upload::{format_from_filename, format_from_media_type, parse_quads};
use crate::kind_detector;
use crate::store::TripleStore;

/// One uploaded file plus the routing decision for its quads.
#[derive(Debug)]
pub struct InputFile {
    pub filename: String,
    pub content_type: String,
    pub bytes: Vec<u8>,
    /// Target graph for triples (and for quads when `merge_into_target` is true).
    /// For TriG/N-Quads with `merge_into_target=false`, the file's own graph
    /// names are preserved.
    pub target_graph: Option<String>,
    /// Force every quad into `target_graph` even if the file specifies graphs.
    pub merge_into_target: bool,
    /// For quad-format files (merge off): remap an embedded graph IRI to a
    /// different write target. Key = embedded IRI as it appears in the file,
    /// value = the IRI to write instead. Embedded graphs absent from the map
    /// keep their original name. Never consulted for triple formats or when
    /// `merge_into_target` is true (those force `target_graph`). Applied during
    /// parsing, *before* the authorize gate, so a re-homed graph is what the
    /// boundary checks and what replace/versioning/registration operate on.
    pub graph_remap: std::collections::HashMap<String, String>,
    /// For quad-format files (merge off) with no `target_graph`: where DEFAULT-graph
    /// (and blank-node-graph) triples are routed. The handler sets this to the
    /// dataset's namespaced default graph for non-admin dataset-scoped imports so
    /// those triples land in a named graph the authorize gate covers, instead of the
    /// shared global default graph. `None` keeps the legacy global-default behavior
    /// (admin / unmanaged imports). Distinct from `target_graph`: it is only the
    /// fallback for the unnamed-graph arm and never forces triple-format files.
    pub unnamed_graph_target: Option<String>,
    /// If true, partition triples by detected role into `{target_graph}/{role}` sub-graphs.
    /// Only applies to triple-format files. Quad-format files already carry named graphs.
    pub auto_split: bool,
    /// If true, the graphs this file writes to are cleared (PUT semantics) before
    /// the batch insert. Otherwise the file's quads are merged in (POST semantics).
    pub replace: bool,
}

/// Per-file outcome.
#[derive(Debug, Serialize, Deserialize)]
pub struct FileResult {
    pub filename: String,
    pub status: &'static str, // "ok" | "error"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub graph_iris: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quad_count: Option<usize>,
}

/// A replace-target graph plus whether its incoming triples differ from what is
/// currently stored. `changed == false` means the upload is byte-for-byte the
/// same triple set already present (so the caller can mark it a draft rather
/// than cutting a new version). A graph with no current data is always
/// `changed == true`.
#[derive(Debug, Clone)]
pub struct GraphChange {
    pub graph: String,
    pub changed: bool,
}

/// Why a bulk load failed, so the caller can map it to the right HTTP status.
#[derive(Debug)]
pub enum BulkError {
    /// A target graph fell outside the caller's permitted write scope. The
    /// HTTP handler maps this to 403 Forbidden. No data was written.
    Forbidden(String),
    /// A parse, store, or archival failure (HTTP 400 from the bulk handler).
    Failed(String),
}

impl std::fmt::Display for BulkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BulkError::Forbidden(m) | BulkError::Failed(m) => f.write_str(m),
        }
    }
}

/// Aggregate result of a bulk import.
#[derive(Debug, Serialize)]
pub struct BulkResult {
    pub success: bool,
    pub total_files: usize,
    pub success_count: usize,
    pub failed_count: usize,
    pub total_quads: usize,
    /// All distinct graph IRIs touched across the batch.
    pub graph_iris: Vec<String>,
    pub file_results: Vec<FileResult>,
}

/// Parse one file's bytes, returning the quads remapped to their final graphs.
///
/// Pure CPU work — safe to run inside `spawn_blocking`. Touches no shared state.
fn parse_one(input: &InputFile) -> Result<(Vec<Quad>, Vec<String>), String> {
    let format: RdfFormat = format_from_media_type(&input.content_type)
        .or_else(|| format_from_filename(&input.filename))
        .ok_or_else(|| {
            format!(
                "Cannot detect RDF format from content-type '{}' or filename '{}'",
                input.content_type, input.filename
            )
        })?;

    let parsed = parse_quads(&input.bytes, format)?;

    // Decide a target graph node for triples / merged quads. None means "keep
    // the parsed graph name" (only meaningful for quad formats).
    let target_node: Option<GraphName> = match input.target_graph.as_deref() {
        Some(iri) => {
            Some(GraphName::NamedNode(NamedNode::new(iri).map_err(|e| {
                format!("Invalid target graph IRI '{iri}': {e}")
            })?))
        }
        None => None,
    };

    // Secondary fallback for the unnamed-graph arm (DEFAULT / blank-node graphs)
    // when the file has no `target_graph`. Set by the handler to the dataset's
    // namespaced default graph so these triples are authorized and stay inside the
    // tenant boundary; `None` preserves the legacy global-default behavior.
    let unnamed_graph_node: Option<GraphName> = match input.unnamed_graph_target.as_deref() {
        Some(iri) => {
            Some(GraphName::NamedNode(NamedNode::new(iri).map_err(|e| {
                format!("Invalid unnamed-graph target IRI '{iri}': {e}")
            })?))
        }
        None => None,
    };

    let force_target = input.merge_into_target || !is_quad_format(format);

    // auto_split only applies to triple formats (force_target is true for those).
    let do_split = input.auto_split && force_target;

    // Subject-tree role assignment for auto_split: roles are decided per root
    // subject (with its blank-node closure), not per quad, so RDF lists under
    // sh:in / sh:or / owl:unionOf and annotations like rdfs:label stay in the
    // same sub-graph as their owning shape/class. SHACL subject-trees land in
    // the shapes sub-graph even when the file-level verdict is Model (mixed
    // OWL+SHACL files).
    let split: Option<(String, Vec<GraphKind>)> = if do_split {
        let base = match &target_node {
            Some(GraphName::NamedNode(nn)) => nn.as_str().to_string(),
            _ => return Err("auto_split requires a target_graph IRI".to_string()),
        };
        Some((base, kind_detector::classify_quad_roles(&parsed)))
    } else {
        None
    };

    let mut touched: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut out: Vec<Quad> = Vec::with_capacity(parsed.len());

    for (idx, q) in parsed.into_iter().enumerate() {
        let final_graph = if let Some((base, roles)) = &split {
            let role = roles[idx];
            let sub_iri = format!("{}/{}", base, role.as_str());
            GraphName::NamedNode(
                NamedNode::new(&sub_iri)
                    .map_err(|e| format!("Invalid split graph IRI '{sub_iri}': {e}"))?,
            )
        } else if force_target {
            target_node
                .clone()
                .ok_or_else(|| "No target graph supplied for triple-format file".to_string())?
        } else {
            // Preserve file-declared graph; default-graph quads fall back to target.
            match &q.graph_name {
                // A file-declared named graph may be re-homed via `graph_remap`
                // (used to move an embedded graph under the dataset namespace so
                // the authorize gate admits it). Unmapped graphs keep their name.
                GraphName::NamedNode(nn) => match input.graph_remap.get(nn.as_str()) {
                    Some(to) => GraphName::NamedNode(
                        NamedNode::new(to)
                            .map_err(|e| format!("Invalid remap target IRI '{to}': {e}"))?,
                    ),
                    None => q.graph_name.clone(),
                },
                // No usable graph IRI. Prefer an explicit `target_graph`, then the
                // handler-supplied namespaced default (dataset-scoped imports), and
                // only otherwise fall through to the shared global default graph.
                // A NamedNode result enters `touched` below, so the authorize gate
                // covers it — closing the global-default cross-tenant bypass.
                GraphName::DefaultGraph | GraphName::BlankNode(_) => target_node
                    .clone()
                    .or_else(|| unnamed_graph_node.clone())
                    .unwrap_or(GraphName::DefaultGraph),
            }
        };

        if let GraphName::NamedNode(nn) = &final_graph {
            touched.insert(nn.as_str().to_string());
        }

        out.push(Quad::new(q.subject, q.predicate, q.object, final_graph));
    }

    Ok((out, touched.into_iter().collect()))
}

fn is_quad_format(format: RdfFormat) -> bool {
    matches!(format, RdfFormat::NQuads | RdfFormat::TriG)
}

/// Stable string key for a triple (graph name ignored) used to compare two
/// triple sets for equality. Blank-node identity is not normalised, so two
/// isomorphic-but-relabelled graphs may compare as different — acceptable for
/// the "did this upload change anything" check.
fn triple_key(q: &Quad) -> String {
    format!("{}\t{}\t{}", q.subject, q.predicate, q.object)
}

/// Triple keys for the subset of `quads` whose final graph is `graph`.
fn incoming_triple_keys(quads: &[Quad], graph: &str) -> std::collections::HashSet<String> {
    quads
        .iter()
        .filter(|q| matches!(&q.graph_name, GraphName::NamedNode(nn) if nn.as_str() == graph))
        .map(triple_key)
        .collect()
}

/// Triple keys currently stored in `graph`.
fn live_triple_keys(
    store: &TripleStore,
    graph: &str,
) -> Result<std::collections::HashSet<String>, String> {
    use oxigraph::model::{GraphNameRef, NamedNodeRef};
    let g = NamedNodeRef::new(graph).map_err(|e| format!("Invalid graph IRI '{graph}': {e}"))?;
    let quads = store
        .quads_for_graph(GraphNameRef::NamedNode(g))
        .map_err(|e| format!("Failed to read graph '{graph}': {e}"))?;
    Ok(quads.iter().map(triple_key).collect())
}

/// SHACL write gate consulted per target graph before anything is committed.
///
/// Bulk import has no access to `AppState` (it runs on a plain `TripleStore`
/// inside `spawn_blocking`), so the caller supplies the gate as closures —
/// typically built from `crate::shacl_studio::gate::{import_gates_apply,
/// check_import_gates}` plus the owning dataset's `shacl_on_write` flag.
pub struct WriteGate<'a> {
    /// Cheap pre-check (pipeline/binding/dataset lookups only), invoked once
    /// per touched graph. Quads are only buffered per graph — and `check` only
    /// invoked — for graphs where this returns true, so imports with no gates
    /// configured (the common case, e.g. large IFC loads) pay a few metadata
    /// lookups and nothing else.
    #[allow(clippy::type_complexity)]
    pub applies: Box<dyn Fn(&str) -> bool + Send + Sync + 'a>,
    /// Validate the incoming quads destined for one gated graph. `Err` carries
    /// a human-readable violation summary and aborts the whole batch *before*
    /// any delete or insert, so nothing is half-committed.
    #[allow(clippy::type_complexity)]
    pub check: Box<dyn Fn(&str, &[Quad]) -> Result<(), String> + Send + Sync + 'a>,
}

/// Parse all files (in parallel) and load them into the store with a single
/// bulk-delete + bulk-insert pair.
///
/// Replace is per file (`InputFile::replace`): only the graphs touched by
/// replace-marked files are dropped before insertion; graphs written by
/// merge-only files keep their existing triples. `before_replace` is invoked
/// once with the sorted list of graphs about to be cleared, each tagged with
/// whether its incoming triples differ from the current contents (empty list ⇒
/// not called). This gives the caller a chance to archive them first and to
/// distinguish a real change from an identical re-upload; it runs inside this
/// blocking task, before any deletion.
///
/// `authorize` is the per-graph write boundary. It is invoked once with the
/// sorted set of *every* graph this batch would touch — triple targets,
/// auto-split sub-graphs, and graph names embedded in quad-format files — after
/// parsing resolves the final set but before any delete or insert. Returning
/// `Err` aborts the load with `BulkError::Forbidden` and writes nothing, so a
/// caller can confine an import to graphs the principal may actually write.
///
/// Per-file parse errors are recorded in the result without aborting siblings;
/// only store-level errors (`BulkError::Failed`) and authorization rejections
/// (`BulkError::Forbidden`) propagate as `Err`.
pub fn parse_and_load_bulk(
    store: &TripleStore,
    inputs: Vec<InputFile>,
    authorize: impl FnOnce(&[String]) -> Result<(), String>,
    before_replace: impl FnOnce(&[GraphChange]) -> Result<(), String>,
) -> Result<BulkResult, BulkError> {
    parse_and_load_bulk_gated(store, inputs, authorize, before_replace, None)
}

/// [`parse_and_load_bulk`] with an optional SHACL [`WriteGate`].
///
/// The gate runs after `authorize` and before the replace-delete / insert pair:
/// for each touched graph where `gate.applies` is true, the incoming quads for
/// that graph are validated via `gate.check`; a failure aborts the batch with
/// [`BulkError::Failed`] (mapped to HTTP 400 by the handler) and writes
/// nothing.
pub fn parse_and_load_bulk_gated(
    store: &TripleStore,
    inputs: Vec<InputFile>,
    authorize: impl FnOnce(&[String]) -> Result<(), String>,
    before_replace: impl FnOnce(&[GraphChange]) -> Result<(), String>,
    write_gate: Option<&WriteGate<'_>>,
) -> Result<BulkResult, BulkError> {
    use rayon::prelude::*;

    let total_files = inputs.len();

    // Parse in parallel — pure CPU, no store access. Order matches `inputs`.
    #[allow(clippy::type_complexity)] // (quads, graphs)-or-error per file; clear inline
    let parsed: Vec<Result<(Vec<Quad>, Vec<String>), String>> =
        inputs.par_iter().map(parse_one).collect();

    let mut all_quads: Vec<Quad> = Vec::new();
    let mut touched_graphs: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut replace_graphs: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut file_results: Vec<FileResult> = Vec::with_capacity(total_files);
    let mut success_count = 0usize;
    let mut failed_count = 0usize;

    for (input, res) in inputs.iter().zip(parsed) {
        let filename = input.filename.clone();
        match res {
            Ok((quads, graphs)) => {
                let qc = quads.len();
                for g in &graphs {
                    touched_graphs.insert(g.clone());
                    if input.replace {
                        replace_graphs.insert(g.clone());
                    }
                }
                all_quads.extend(quads);
                success_count += 1;
                file_results.push(FileResult {
                    filename,
                    status: "ok",
                    error: None,
                    graph_iris: graphs,
                    quad_count: Some(qc),
                });
            }
            Err(e) => {
                failed_count += 1;
                file_results.push(FileResult {
                    filename,
                    status: "error",
                    error: Some(e),
                    graph_iris: vec![],
                    quad_count: None,
                });
            }
        }
    }

    let graph_list: Vec<String> = touched_graphs.into_iter().collect();
    let replace_list: Vec<String> = replace_graphs.into_iter().collect();

    // Per-graph write boundary. `graph_list` is the complete, parse-resolved set
    // of graphs this batch would write — so this single gate covers triple
    // targets, auto-split sub-graphs and quad-format embedded graph names alike.
    // It runs before the replace/delete and the insert, so a rejected target
    // neither wipes existing data nor adds new triples.
    authorize(&graph_list).map_err(BulkError::Forbidden)?;

    // SHACL write gate: validate the incoming quads for every gated graph
    // before the first destructive operation (the replace-delete below) and
    // before the insert, so a rejected batch commits nothing. The `applies`
    // pre-check keeps the no-gates path free of any quad buffering.
    if let Some(gate) = write_gate {
        for g in graph_list.iter().filter(|g| (gate.applies)(g)) {
            let incoming: Vec<Quad> = all_quads
                .iter()
                .filter(
                    |q| matches!(&q.graph_name, GraphName::NamedNode(nn) if nn.as_str() == g.as_str()),
                )
                .cloned()
                .collect();
            if incoming.is_empty() {
                continue;
            }
            (gate.check)(g, &incoming).map_err(|e| {
                BulkError::Failed(format!(
                    "SHACL write gate rejected import into graph <{g}>: {e}"
                ))
            })?;
        }
    }

    if !replace_list.is_empty() {
        // Compare each replace target's incoming triples against what is already
        // stored *before* anything is cleared, so the caller can tell an actual
        // change apart from an identical re-upload.
        let changes: Vec<GraphChange> = replace_list
            .iter()
            .map(|g| {
                let incoming = incoming_triple_keys(&all_quads, g);
                let live = live_triple_keys(store, g)?;
                Ok(GraphChange {
                    graph: g.clone(),
                    changed: incoming != live,
                })
            })
            .collect::<Result<_, String>>()
            .map_err(BulkError::Failed)?;
        // Let the caller archive the soon-to-be-cleared graphs first.
        before_replace(&changes).map_err(BulkError::Failed)?;
        let refs: Vec<&str> = replace_list.iter().map(|s| s.as_str()).collect();
        store
            .bulk_delete_graphs(&refs)
            .map_err(|e| BulkError::Failed(format!("Failed to clear target graphs: {e}")))?;
    }

    let total_quads = all_quads.len();
    if !all_quads.is_empty() {
        store
            .bulk_insert_quads(all_quads, &graph_list)
            .map_err(|e| BulkError::Failed(format!("Failed to insert quads: {e}")))?;
    }

    Ok(BulkResult {
        success: failed_count == 0,
        total_files,
        success_count,
        failed_count,
        total_quads,
        graph_iris: graph_list,
        file_results,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const G: &str = "http://example.org/g";

    fn ttl_input(name: &str, body: &str, target: &str, replace: bool) -> InputFile {
        InputFile {
            filename: name.to_string(),
            content_type: "text/turtle".to_string(),
            bytes: body.as_bytes().to_vec(),
            target_graph: Some(target.to_string()),
            merge_into_target: false,
            unnamed_graph_target: None,
            auto_split: false,
            replace,
            graph_remap: std::collections::HashMap::new(),
        }
    }

    /// An N-Quads input with no target graph (embedded graph names preserved
    /// unless remapped). `merge_into_target` is false, so the quad-preserve /
    /// `graph_remap` path in `parse_one` is exercised.
    fn nq_input(name: &str, body: &str) -> InputFile {
        InputFile {
            filename: name.to_string(),
            content_type: "application/n-quads".to_string(),
            bytes: body.as_bytes().to_vec(),
            target_graph: None,
            merge_into_target: false,
            unnamed_graph_target: None,
            auto_split: false,
            replace: false,
            graph_remap: std::collections::HashMap::new(),
        }
    }

    fn seed_one(store: &TripleStore, graph: &str) {
        let q = Quad::new(
            NamedNode::new("http://example.org/old").unwrap(),
            NamedNode::new("http://example.org/p").unwrap(),
            NamedNode::new("http://example.org/o").unwrap(),
            NamedNode::new(graph).unwrap(),
        );
        store
            .bulk_insert_quads(vec![q], &[graph.to_string()])
            .unwrap();
    }

    #[test]
    fn replace_file_clears_target_graph_then_inserts() {
        let store = TripleStore::in_memory().unwrap();
        seed_one(&store, G);
        assert_eq!(store.count_graph(Some(G)).unwrap(), 1);

        let input = ttl_input(
            "a.ttl",
            "<http://example.org/new> <http://example.org/p> <http://example.org/o> .",
            G,
            true,
        );
        let mut archived: Vec<GraphChange> = vec![];
        parse_and_load_bulk(
            &store,
            vec![input],
            |_| Ok(()),
            |graphs| {
                archived = graphs.to_vec();
                Ok(())
            },
        )
        .unwrap();

        // before_replace was handed exactly the graph about to be cleared, and
        // the new triple set differs from the old one.
        assert_eq!(archived.len(), 1);
        assert_eq!(archived[0].graph, G);
        assert!(archived[0].changed);
        // Old triple gone, only the new one remains.
        assert_eq!(store.count_graph(Some(G)).unwrap(), 1);
    }

    #[test]
    fn replace_with_identical_data_is_flagged_unchanged() {
        let store = TripleStore::in_memory().unwrap();
        let body = "<http://example.org/old> <http://example.org/p> <http://example.org/o> .";
        // Seed the graph with exactly what we are about to re-upload.
        let seed = ttl_input("seed.ttl", body, G, false);
        parse_and_load_bulk(&store, vec![seed], |_| Ok(()), |_| Ok(())).unwrap();
        assert_eq!(store.count_graph(Some(G)).unwrap(), 1);

        let input = ttl_input("a.ttl", body, G, true);
        let mut changes: Vec<GraphChange> = vec![];
        parse_and_load_bulk(
            &store,
            vec![input],
            |_| Ok(()),
            |g| {
                changes = g.to_vec();
                Ok(())
            },
        )
        .unwrap();

        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].graph, G);
        assert!(
            !changes[0].changed,
            "identical re-upload should be unchanged"
        );
    }

    #[test]
    fn merge_file_keeps_existing_data_and_skips_archive() {
        let store = TripleStore::in_memory().unwrap();
        seed_one(&store, G);

        let input = ttl_input(
            "a.ttl",
            "<http://example.org/new> <http://example.org/p> <http://example.org/o> .",
            G,
            false,
        );
        let mut archive_called = false;
        parse_and_load_bulk(
            &store,
            vec![input],
            |_| Ok(()),
            |_| {
                archive_called = true;
                Ok(())
            },
        )
        .unwrap();

        // POST semantics: both old and new coexist; archive hook never fired.
        assert!(!archive_called);
        assert_eq!(store.count_graph(Some(G)).unwrap(), 2);
    }

    #[test]
    fn replace_only_clears_replace_files_graphs() {
        let store = TripleStore::in_memory().unwrap();
        let g_merge = "http://example.org/keep";
        seed_one(&store, G);
        seed_one(&store, g_merge);

        let replace_f = ttl_input(
            "replace.ttl",
            "<http://example.org/new> <http://example.org/p> <http://example.org/o> .",
            G,
            true,
        );
        let merge_f = ttl_input(
            "merge.ttl",
            "<http://example.org/new2> <http://example.org/p> <http://example.org/o> .",
            g_merge,
            false,
        );
        let mut archived: Vec<String> = vec![];
        parse_and_load_bulk(
            &store,
            vec![replace_f, merge_f],
            |_| Ok(()),
            |graphs| {
                archived = graphs.iter().map(|c| c.graph.clone()).collect();
                Ok(())
            },
        )
        .unwrap();

        assert_eq!(archived, vec![G.to_string()]);
        assert_eq!(store.count_graph(Some(G)).unwrap(), 1); // cleared + new
        assert_eq!(store.count_graph(Some(g_merge)).unwrap(), 2); // old + new2
    }

    #[test]
    fn authorize_rejection_aborts_before_any_write() {
        let store = TripleStore::in_memory().unwrap();
        seed_one(&store, G); // pre-existing data in the replace target
        assert_eq!(store.count_graph(Some(G)).unwrap(), 1);

        let input = ttl_input(
            "a.ttl",
            "<http://example.org/new> <http://example.org/p> <http://example.org/o> .",
            G,
            true, // replace
        );
        // The authorize gate rejects the target graph.
        let err = parse_and_load_bulk(&store, vec![input], |_| Err("nope".to_string()), |_| Ok(()))
            .unwrap_err();
        assert!(matches!(err, BulkError::Forbidden(_)));
        // The replace target was neither cleared nor appended to: a rejected
        // import must touch nothing.
        assert_eq!(
            store.count_graph(Some(G)).unwrap(),
            1,
            "a rejected import must not clear or modify the target graph"
        );
    }

    #[test]
    fn authorize_sees_quad_embedded_graph_names() {
        let store = TripleStore::in_memory().unwrap();
        // N-Quads embed their own graph name; with merge off the embedded graph is
        // the write target, so the authorize closure must see it (otherwise a
        // quad file could bypass a target-graph-only check).
        let embedded = "http://example.org/embedded";
        let input = nq_input(
            "a.nq",
            &format!("<http://e/s> <http://e/p> <http://e/o> <{embedded}> ."),
        );
        let mut seen: Vec<String> = vec![];
        parse_and_load_bulk(
            &store,
            vec![input],
            |graphs| {
                seen = graphs.to_vec();
                Ok(())
            },
            |_| Ok(()),
        )
        .unwrap();
        assert_eq!(
            seen,
            vec![embedded.to_string()],
            "authorize must receive graph names embedded in quad-format files"
        );
    }

    #[test]
    fn remap_redirects_embedded_graph_and_authorize_sees_target() {
        let store = TripleStore::in_memory().unwrap();
        let embedded = "http://foreign.example/g";
        let target = "http://localhost:7878/dataset/dsA/data";
        let mut input = nq_input(
            "a.nq",
            &format!("<http://e/s> <http://e/p> <http://e/o> <{embedded}> ."),
        );
        input
            .graph_remap
            .insert(embedded.to_string(), target.to_string());

        let mut seen: Vec<String> = vec![];
        parse_and_load_bulk(
            &store,
            vec![input],
            |graphs| {
                seen = graphs.to_vec();
                Ok(())
            },
            |_| Ok(()),
        )
        .unwrap();

        // authorize saw the remapped target — never the embedded foreign name —
        // so the boundary checks the final destination, and the data landed
        // there (the embedded graph is left empty).
        assert_eq!(seen, vec![target.to_string()]);
        assert_eq!(store.count_graph(Some(target)).unwrap(), 1);
        assert_eq!(store.count_graph(Some(embedded)).unwrap(), 0);
    }

    #[test]
    fn remap_leaves_unmapped_embedded_graphs_untouched() {
        let store = TripleStore::in_memory().unwrap();
        let mapped = "http://foreign.example/mapped";
        let unmapped = "http://foreign.example/unmapped";
        let target = "http://localhost:7878/dataset/dsA/data";
        let body = format!(
            "<http://e/s> <http://e/p> <http://e/o> <{mapped}> .\n\
             <http://e/s2> <http://e/p> <http://e/o> <{unmapped}> ."
        );
        let mut input = nq_input("a.nq", &body);
        input
            .graph_remap
            .insert(mapped.to_string(), target.to_string());

        let mut seen: Vec<String> = vec![];
        parse_and_load_bulk(
            &store,
            vec![input],
            |g| {
                seen = g.to_vec();
                Ok(())
            },
            |_| Ok(()),
        )
        .unwrap();

        // Only the mapped graph is re-homed; the unmapped one keeps its name.
        assert_eq!(seen.len(), 2);
        assert!(seen.contains(&target.to_string()));
        assert!(seen.contains(&unmapped.to_string()));
        assert!(!seen.contains(&mapped.to_string()));
        assert_eq!(store.count_graph(Some(target)).unwrap(), 1);
        assert_eq!(store.count_graph(Some(unmapped)).unwrap(), 1);
    }

    #[test]
    fn merge_into_target_ignores_remap() {
        let store = TripleStore::in_memory().unwrap();
        let embedded = "http://foreign.example/g";
        let remap_to = "http://localhost:7878/dataset/dsA/remapped";
        let merge_target = "http://localhost:7878/dataset/dsA/merged";
        let mut input = nq_input(
            "a.nq",
            &format!("<http://e/s> <http://e/p> <http://e/o> <{embedded}> ."),
        );
        input.merge_into_target = true;
        input.target_graph = Some(merge_target.to_string());
        input
            .graph_remap
            .insert(embedded.to_string(), remap_to.to_string());

        let mut seen: Vec<String> = vec![];
        parse_and_load_bulk(
            &store,
            vec![input],
            |g| {
                seen = g.to_vec();
                Ok(())
            },
            |_| Ok(()),
        )
        .unwrap();

        // merge forces everything into `target_graph`; the remap is never read.
        assert_eq!(seen, vec![merge_target.to_string()]);
        assert_eq!(store.count_graph(Some(merge_target)).unwrap(), 1);
        assert_eq!(store.count_graph(Some(remap_to)).unwrap(), 0);
    }

    #[test]
    fn invalid_remap_target_is_per_file_error() {
        let store = TripleStore::in_memory().unwrap();
        let embedded = "http://foreign.example/g";
        // A bad file whose remap value is not a valid IRI, plus a good sibling.
        let mut bad = nq_input(
            "bad.nq",
            &format!("<http://e/s> <http://e/p> <http://e/o> <{embedded}> ."),
        );
        bad.graph_remap
            .insert(embedded.to_string(), "not a valid iri".to_string());
        let good_target = "http://localhost:7878/dataset/dsA/ok";
        let good = ttl_input(
            "good.ttl",
            "<http://e/s> <http://e/p> <http://e/o> .",
            good_target,
            false,
        );

        let res = parse_and_load_bulk(&store, vec![bad, good], |_| Ok(()), |_| Ok(())).unwrap();

        // The bad file is recorded as an error without aborting its sibling.
        assert_eq!(res.failed_count, 1);
        assert_eq!(res.success_count, 1);
        let bad_result = res
            .file_results
            .iter()
            .find(|r| r.filename == "bad.nq")
            .unwrap();
        assert_eq!(bad_result.status, "error");
        assert!(bad_result
            .error
            .as_deref()
            .unwrap()
            .contains("remap target"));
        assert_eq!(store.count_graph(Some(good_target)).unwrap(), 1);
    }

    #[test]
    fn unnamed_target_routes_default_graph_quads() {
        let store = TripleStore::in_memory().unwrap();
        // A plain N-Quads line with NO graph label parses into the default graph.
        // With `unnamed_graph_target` set (as the handler does for a non-admin
        // dataset-scoped import) it must be re-homed into that named graph so the
        // authorize gate sees it and nothing lands in the global default graph.
        let ns_default = "http://localhost:7878/dataset/dsA/default";
        let mut input = nq_input("a.nq", "<http://e/s> <http://e/p> <http://e/o> .");
        input.unnamed_graph_target = Some(ns_default.to_string());

        let mut seen: Vec<String> = vec![];
        parse_and_load_bulk(
            &store,
            vec![input],
            |graphs| {
                seen = graphs.to_vec();
                Ok(())
            },
            |_| Ok(()),
        )
        .unwrap();

        assert_eq!(
            seen,
            vec![ns_default.to_string()],
            "default-graph quads must be routed to (and authorized as) the namespaced default graph"
        );
        assert_eq!(store.count_graph(Some(ns_default)).unwrap(), 1);
        assert_eq!(
            store.count_graph(None).unwrap(),
            0,
            "nothing may leak into the shared global default graph"
        );
    }

    #[test]
    fn unnamed_target_routes_blank_node_graph() {
        let store = TripleStore::in_memory().unwrap();
        // A blank-node graph label is just as unaddressable as the default graph and
        // shares the same arm, so it is routed the same way.
        let ns_default = "http://localhost:7878/dataset/dsA/default";
        let mut input = nq_input("a.nq", "<http://e/s> <http://e/p> <http://e/o> _:bg .");
        input.unnamed_graph_target = Some(ns_default.to_string());

        let mut seen: Vec<String> = vec![];
        parse_and_load_bulk(
            &store,
            vec![input],
            |g| {
                seen = g.to_vec();
                Ok(())
            },
            |_| Ok(()),
        )
        .unwrap();

        assert_eq!(seen, vec![ns_default.to_string()]);
        assert_eq!(store.count_graph(Some(ns_default)).unwrap(), 1);
        assert_eq!(store.count_graph(None).unwrap(), 0);
    }

    #[test]
    fn unnamed_target_ignored_when_target_graph_set() {
        let store = TripleStore::in_memory().unwrap();
        // An explicit `target_graph` still wins for the unnamed-graph arm; the
        // namespaced default is only the fallback when no target was named.
        let target = "http://localhost:7878/dataset/dsA/data";
        let unnamed = "http://localhost:7878/dataset/dsA/default";
        let mut input = nq_input("a.nq", "<http://e/s> <http://e/p> <http://e/o> .");
        input.target_graph = Some(target.to_string());
        input.unnamed_graph_target = Some(unnamed.to_string());

        let mut seen: Vec<String> = vec![];
        parse_and_load_bulk(
            &store,
            vec![input],
            |g| {
                seen = g.to_vec();
                Ok(())
            },
            |_| Ok(()),
        )
        .unwrap();

        assert_eq!(seen, vec![target.to_string()]);
        assert_eq!(store.count_graph(Some(target)).unwrap(), 1);
        assert_eq!(store.count_graph(Some(unnamed)).unwrap(), 0);
        assert_eq!(store.count_graph(None).unwrap(), 0);
    }

    #[test]
    fn auto_split_keeps_closures_with_their_roots() {
        let store = TripleStore::in_memory().unwrap();
        // Mixed OWL + SHACL + instances. Pre-fix, the rdf:first/rdf:rest list
        // spines and the rdfs:label annotations were severed into the wrong
        // sub-graphs; subject-tree splitting keeps each closure intact.
        let ttl = r#"
            @prefix owl: <http://www.w3.org/2002/07/owl#> .
            @prefix sh:  <http://www.w3.org/ns/shacl#> .
            @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
            @prefix ex:  <http://example.org/> .
            ex:Vehicle a owl:Class ; rdfs:label "Vehicle" ; owl:unionOf ( ex:Car ex:Bike ) .
            ex:VehicleShape a sh:NodeShape ;
                rdfs:label "Vehicle shape" ;
                sh:targetClass ex:Vehicle ;
                sh:property [ sh:path ex:kind ; sh:in ( "car" "bike" ) ] .
            ex:v1 a ex:Vehicle ; ex:kind "car" .
        "#;
        let mut input = ttl_input("mixed.ttl", ttl, G, false);
        input.auto_split = true;

        let res = parse_and_load_bulk(&store, vec![input], |_| Ok(()), |_| Ok(())).unwrap();

        let model = format!("{G}/model");
        let shapes = format!("{G}/shapes");
        let instances = format!("{G}/instances");
        // Class tree: type + label + unionOf + 4 list-spine quads.
        assert_eq!(store.count_graph(Some(&model)).unwrap(), 7);
        // Shape tree: type + label + targetClass + property + path + in + 4 spine.
        // Pre-fix the 4 spine quads landed in /instances and the label in /model.
        assert_eq!(store.count_graph(Some(&shapes)).unwrap(), 10);
        // Instance tree only: type + kind.
        assert_eq!(store.count_graph(Some(&instances)).unwrap(), 2);
        // A Model-verdict file still yields a shapes sub-graph (mixed OWL+SHACL).
        assert_eq!(res.graph_iris, vec![instances, model, shapes]);
    }

    #[test]
    fn auto_split_routes_plain_instances_unchanged() {
        let store = TripleStore::in_memory().unwrap();
        let ttl = r#"
            @prefix ex: <http://example.org/> .
            @prefix foaf: <http://xmlns.com/foaf/0.1/> .
            ex:Alice a foaf:Person ; foaf:name "Alice" .
            ex:Bob a foaf:Person .
        "#;
        let mut input = ttl_input("inst.ttl", ttl, G, false);
        input.auto_split = true;
        parse_and_load_bulk(&store, vec![input], |_| Ok(()), |_| Ok(())).unwrap();
        assert_eq!(
            store.count_graph(Some(&format!("{G}/instances"))).unwrap(),
            3
        );
    }

    #[test]
    fn write_gate_not_invoked_when_no_gate_applies() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let store = TripleStore::in_memory().unwrap();
        let applies_calls = AtomicUsize::new(0);
        let check_calls = AtomicUsize::new(0);
        let gate = WriteGate {
            applies: Box::new(|_| {
                applies_calls.fetch_add(1, Ordering::SeqCst);
                false
            }),
            check: Box::new(|_, _| {
                check_calls.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }),
        };
        let input = ttl_input(
            "a.ttl",
            "<http://example.org/new> <http://example.org/p> <http://example.org/o> .",
            G,
            false,
        );
        parse_and_load_bulk_gated(&store, vec![input], |_| Ok(()), |_| Ok(()), Some(&gate))
            .unwrap();
        // The cheap pre-check ran once per touched graph; no quads were
        // buffered or validated, and the data landed.
        assert_eq!(applies_calls.load(Ordering::SeqCst), 1);
        assert_eq!(check_calls.load(Ordering::SeqCst), 0);
        assert_eq!(store.count_graph(Some(G)).unwrap(), 1);
    }

    #[test]
    fn write_gate_rejection_aborts_batch_before_any_write() {
        let store = TripleStore::in_memory().unwrap();
        seed_one(&store, G);
        let g_other = "http://example.org/other";

        // Two files: the gated replace target plus an innocent sibling. A gate
        // failure must abort the WHOLE batch — no clear, no insert anywhere.
        let gated = ttl_input(
            "gated.ttl",
            "<http://example.org/new> <http://example.org/p> <http://example.org/o> .",
            G,
            true, // replace: would clear G if the gate did not fire first
        );
        let sibling = ttl_input(
            "ok.ttl",
            "<http://example.org/s2> <http://example.org/p> <http://example.org/o> .",
            g_other,
            false,
        );
        let gate = WriteGate {
            applies: Box::new(|g| g == G),
            check: Box::new(|_, _| Err("1 validation result(s) — missing ex:name".to_string())),
        };
        let err = parse_and_load_bulk_gated(
            &store,
            vec![gated, sibling],
            |_| Ok(()),
            |_| Ok(()),
            Some(&gate),
        )
        .unwrap_err();

        let BulkError::Failed(msg) = err else {
            panic!("expected BulkError::Failed");
        };
        assert!(msg.contains("SHACL write gate rejected"), "{msg}");
        assert!(msg.contains(G), "error names the gated graph: {msg}");
        assert!(msg.contains("missing ex:name"), "violation summary: {msg}");
        // Replace target untouched (old triple intact), sibling not committed.
        let keys = live_triple_keys(&store, G).unwrap();
        assert_eq!(keys.len(), 1);
        assert!(keys.iter().any(|k| k.contains("http://example.org/old")));
        assert_eq!(store.count_graph(Some(g_other)).unwrap(), 0);
    }

    #[test]
    fn write_gate_checks_only_quads_for_the_gated_graph() {
        let store = TripleStore::in_memory().unwrap();
        let g_other = "http://example.org/other";
        let gated = ttl_input(
            "gated.ttl",
            "<http://example.org/a> <http://example.org/p> <http://example.org/o> .\n\
             <http://example.org/b> <http://example.org/p> <http://example.org/o> .",
            G,
            false,
        );
        let free = ttl_input(
            "free.ttl",
            "<http://example.org/c> <http://example.org/p> <http://example.org/o> .",
            g_other,
            false,
        );

        let seen = std::sync::Mutex::new(Vec::<(String, usize)>::new());
        let gate = WriteGate {
            applies: Box::new(|g| g == G),
            check: Box::new(|g, quads| {
                seen.lock().unwrap().push((g.to_string(), quads.len()));
                Ok(())
            }),
        };
        parse_and_load_bulk_gated(
            &store,
            vec![gated, free],
            |_| Ok(()),
            |_| Ok(()),
            Some(&gate),
        )
        .unwrap();

        // Exactly one check, for the gated graph, with only its two quads.
        assert_eq!(*seen.lock().unwrap(), vec![(G.to_string(), 2)]);
        // A passing gate commits everything.
        assert_eq!(store.count_graph(Some(G)).unwrap(), 2);
        assert_eq!(store.count_graph(Some(g_other)).unwrap(), 1);
    }

    #[test]
    fn no_unnamed_target_keeps_global_default() {
        let store = TripleStore::in_memory().unwrap();
        // Admin / unmanaged imports leave `unnamed_graph_target` unset, preserving the
        // legacy behavior: default-graph quads land in the shared global default graph
        // and are NOT in the authorized graph set (documents the pre-fix path).
        let input = nq_input("a.nq", "<http://e/s> <http://e/p> <http://e/o> .");
        let mut seen: Vec<String> = vec![];
        parse_and_load_bulk(
            &store,
            vec![input],
            |g| {
                seen = g.to_vec();
                Ok(())
            },
            |_| Ok(()),
        )
        .unwrap();

        assert!(
            seen.is_empty(),
            "no named graph is touched, so authorize sees an empty set"
        );
        assert_eq!(store.count_graph(None).unwrap(), 1);
    }
}
