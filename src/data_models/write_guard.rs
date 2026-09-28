//! Direct writes into the graphs of registered model versions.
//!
//! The registry's own API refuses every way of altering content whose licence
//! allows no altered copies, and marks a copy it lets someone edit as possibly
//! modified. A SPARQL Update or a Graph Store Protocol request writes graphs
//! directly, past that API. The server's write authorization therefore calls
//! [`check`] for every graph such a request names, before anything runs, and
//! [`mark`] once the request is authorized, before its write runs:
//!
//! * a graph of a version whose licence record allows no altered copies
//!   (IMBOR, a no-derivatives LOV install, a seed-bundle model declared so) is
//!   refused;
//! * a graph of any other version with a licence record may be written, and
//!   its record stops calling the content unchanged first. Marking before the
//!   write means a write cut short by a crash still leaves a record that says
//!   "may have been modified", never a false "unchanged".
//!
//! Each graph costs one registry query ([`registry::attributed_version_of_graph`]).
//!
//! A write whose graphs cannot be named in advance (a variable `GRAPH ?g`,
//! `CLEAR ALL`), which only admins may run, is followed by
//! [`reverify_checked_copies`]: every copy a check vouched for is compared with
//! the digest recorded at that check, and a changed one is marked. Content
//! whose licence allows no altered copies is then withheld from everyone who
//! may not write its entry: the seeded IMBOR until the next start restores it;
//! a no-derivatives seed-bundle model until its graphs hold its files' triples
//! again; a no-derivatives LOV install until a super-admin deletes it and
//! installs it again.

use super::registry::{self, AttributedVersion};
use super::{content_digest, vocab_files};
use crate::store::engine::StoreError;
use crate::store::TripleStore;

/// Whether a direct write into `graph_iri` may run: `Err` with the refusal for
/// a graph of a version whose licence allows no altered copies; otherwise the
/// attributed version the graph belongs to, if any, for [`mark`].
pub fn check(
    store: &TripleStore,
    base_url: &str,
    graph_iri: &str,
) -> Result<Option<AttributedVersion>, String> {
    match registry::attributed_version_of_graph(store, base_url, graph_iri) {
        Some(v) if v.attribution.no_derivatives => Err(refusal(graph_iri, &v)),
        other => Ok(other),
    }
}

/// Why a direct write into a no-derivatives version's graph is refused.
pub fn refusal(graph_iri: &str, v: &AttributedVersion) -> String {
    format!(
        "Writing graph <{graph_iri}> is refused: it holds version '{}' of registry entry '{}', \
         {}, whose licence allows no altered copies. Build on it in a model of your own instead.",
        v.version,
        v.data_model_id,
        vocab_files::source_phrase(&v.attribution.file)
    )
}

/// Mark the records of the versions a direct write is about to change as
/// possibly modified (each once; a record that already says so is left
/// alone).
pub fn mark(store: &TripleStore, versions: &[AttributedVersion]) -> Result<(), StoreError> {
    let mut done: Vec<&str> = Vec::new();
    for v in versions {
        if done.contains(&v.record_iri.as_str()) || !v.attribution.unchanged {
            continue;
        }
        done.push(&v.record_iri);
        registry::mark_possibly_modified(store, &v.record_iri, vocab_files::written_stored_copy)?;
        tracing::info!(
            "model '{}' version '{}': written directly; its licence record now says the copy \
             may have been modified",
            v.data_model_id,
            v.version
        );
    }
    Ok(())
}

/// After a write whose graphs could not be named in advance: compare every
/// copy a check vouched for (a record calling its content unchanged, with the
/// digest recorded at the check) with that digest, and mark each one that
/// changed as possibly modified. Returns how many were marked.
pub fn reverify_checked_copies(store: &TripleStore) -> usize {
    let mut marked = 0usize;
    for c in registry::checked_copies(store) {
        let graphs = content_digest::version_graphs(&c.graph_iri, &c.sub_graphs);
        match content_digest::graphs_digest(store, &graphs) {
            Ok(d) if d == c.content_digest => {}
            Ok(_) => {
                match registry::mark_possibly_modified(
                    store,
                    &c.record_iri,
                    vocab_files::written_stored_copy,
                ) {
                    Ok(()) => {
                        marked += 1;
                        tracing::warn!(
                            "<{}>: its stored copy changed in a write that named no graph; its \
                             licence record now says it may have been modified",
                            c.record_iri
                        );
                    }
                    Err(e) => tracing::warn!("<{}>: could not be marked: {e}", c.record_iri),
                }
            }
            Err(e) => tracing::warn!("<{}>: could not be re-checked: {e}", c.graph_iri),
        }
    }
    marked
}
