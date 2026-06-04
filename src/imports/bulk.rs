//! Bulk import core: parse N files in parallel, then a single bulk insert.

use oxigraph::io::RdfFormat;
use oxigraph::model::{GraphName, NamedNode, Quad};
use serde::{Deserialize, Serialize};

use crate::kind_detector;
use crate::store::TripleStore;
use crate::vocabularies::upload::{format_from_filename, format_from_media_type, parse_quads};

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

    let force_target = input.merge_into_target || !is_quad_format(format);

    // auto_split only applies to triple formats (force_target is true for those).
    let do_split = input.auto_split && force_target;

    let mut touched: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut out: Vec<Quad> = Vec::with_capacity(parsed.len());

    for q in parsed {
        let final_graph = if do_split {
            let base = match &target_node {
                Some(GraphName::NamedNode(nn)) => nn.as_str().to_string(),
                _ => return Err("auto_split requires a target_graph IRI".to_string()),
            };
            let role = kind_detector::classify_quad_role(&q);
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
            match q.graph_name {
                GraphName::NamedNode(_) => q.graph_name.clone(),
                GraphName::DefaultGraph | GraphName::BlankNode(_) => {
                    target_node.clone().unwrap_or(GraphName::DefaultGraph)
                }
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
/// Per-file parse errors are recorded in the result without aborting siblings;
/// only store-level errors propagate as `Err`.
pub fn parse_and_load_bulk(
    store: &TripleStore,
    inputs: Vec<InputFile>,
    before_replace: impl FnOnce(&[GraphChange]) -> Result<(), String>,
) -> Result<BulkResult, String> {
    use rayon::prelude::*;

    let total_files = inputs.len();

    // Parse in parallel — pure CPU, no store access. Order matches `inputs`.
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
            .collect::<Result<_, String>>()?;
        // Let the caller archive the soon-to-be-cleared graphs first.
        before_replace(&changes)?;
        let refs: Vec<&str> = replace_list.iter().map(|s| s.as_str()).collect();
        store
            .bulk_delete_graphs(&refs)
            .map_err(|e| format!("Failed to clear target graphs: {e}"))?;
    }

    let total_quads = all_quads.len();
    if !all_quads.is_empty() {
        store
            .bulk_insert_quads(all_quads, &graph_list)
            .map_err(|e| format!("Failed to insert quads: {e}"))?;
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
            auto_split: false,
            replace,
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
        parse_and_load_bulk(&store, vec![input], |graphs| {
            archived = graphs.to_vec();
            Ok(())
        })
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
        parse_and_load_bulk(&store, vec![seed], |_| Ok(())).unwrap();
        assert_eq!(store.count_graph(Some(G)).unwrap(), 1);

        let input = ttl_input("a.ttl", body, G, true);
        let mut changes: Vec<GraphChange> = vec![];
        parse_and_load_bulk(&store, vec![input], |g| {
            changes = g.to_vec();
            Ok(())
        })
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
        parse_and_load_bulk(&store, vec![input], |_| {
            archive_called = true;
            Ok(())
        })
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
        parse_and_load_bulk(&store, vec![replace_f, merge_f], |graphs| {
            archived = graphs.iter().map(|c| c.graph.clone()).collect();
            Ok(())
        })
        .unwrap();

        assert_eq!(archived, vec![G.to_string()]);
        assert_eq!(store.count_graph(Some(G)).unwrap(), 1); // cleared + new
        assert_eq!(store.count_graph(Some(g_merge)).unwrap(), 2); // old + new2
    }
}
