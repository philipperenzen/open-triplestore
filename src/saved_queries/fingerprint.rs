//! Order-insensitive fingerprint of a SPARQL result set.
//!
//! Used to decide whether a saved query "yields the same results" against a new
//! dataset version as it did against the previous one. SELECT bindings and
//! CONSTRUCT/DESCRIBE triples are canonicalised and sorted before hashing, so a
//! mere change in row order (a query without `ORDER BY`) is not reported as a
//! change; only a genuine difference in the multiset of results is.

use oxigraph::sparql::QueryResults;
use sha2::{Digest, Sha256};

/// A row/triple separator unlikely to appear in term syntax.
const ROW_SEP: char = '\u{1e}';
const CELL_SEP: char = '\u{1f}';

fn hash_hex(s: &str) -> String {
    let mut h = Sha256::new();
    h.update(s.as_bytes());
    format!("{:x}", h.finalize())
}

/// Outcome of fingerprinting a result set.
pub struct Fingerprint {
    /// Hex SHA-256 over the canonicalised, sorted results.
    pub hash: String,
    /// Number of solutions (SELECT) or triples (CONSTRUCT/DESCRIBE); `None` for ASK.
    pub rowcount: Option<i64>,
}

/// Consume a `QueryResults` and compute its fingerprint.
///
/// `Err` carries an evaluation error message (the query ran but a solution
/// failed to materialise) so the caller can record it as a broken test.
pub fn fingerprint(results: QueryResults) -> Result<Fingerprint, String> {
    match results {
        QueryResults::Boolean(b) => Ok(Fingerprint {
            hash: hash_hex(if b { "ASK:true" } else { "ASK:false" }),
            rowcount: None,
        }),
        QueryResults::Solutions(sols) => {
            let mut rows: Vec<String> = Vec::new();
            for sol in sols {
                let sol = sol.map_err(|e| e.to_string())?;
                let mut cells: Vec<String> = Vec::with_capacity(sol.values().len());
                for (i, term) in sol.values().iter().enumerate() {
                    let rendered = term.as_ref().map(|t| t.to_string()).unwrap_or_default();
                    cells.push(format!("{i}={rendered}"));
                }
                rows.push(cells.join(&CELL_SEP.to_string()));
            }
            let count = rows.len() as i64;
            rows.sort();
            Ok(Fingerprint {
                hash: hash_hex(&rows.join(&ROW_SEP.to_string())),
                rowcount: Some(count),
            })
        }
        QueryResults::Graph(triples) => {
            let mut lines: Vec<String> = Vec::new();
            for t in triples {
                let t = t.map_err(|e| e.to_string())?;
                lines.push(t.to_string());
            }
            let count = lines.len() as i64;
            lines.sort();
            Ok(Fingerprint {
                hash: hash_hex(&lines.join(&ROW_SEP.to_string())),
                rowcount: Some(count),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::TripleStore;

    fn fp(store: &TripleStore, q: &str) -> Fingerprint {
        fingerprint(store.query(q).unwrap()).unwrap()
    }

    #[test]
    fn row_order_does_not_change_fingerprint() {
        let store = TripleStore::in_memory().unwrap();
        store
            .update(
                "INSERT DATA { GRAPH <urn:g> { \
                 <urn:a> <urn:p> \"1\" . <urn:b> <urn:p> \"2\" . <urn:c> <urn:p> \"3\" . } }",
            )
            .unwrap();
        let a = fp(
            &store,
            "SELECT ?s WHERE { GRAPH <urn:g> { ?s ?p ?o } } ORDER BY ASC(?s)",
        );
        let b = fp(
            &store,
            "SELECT ?s WHERE { GRAPH <urn:g> { ?s ?p ?o } } ORDER BY DESC(?s)",
        );
        assert_eq!(a.hash, b.hash, "ordering must not affect the fingerprint");
        assert_eq!(a.rowcount, Some(3));
    }

    #[test]
    fn different_data_changes_fingerprint() {
        let store = TripleStore::in_memory().unwrap();
        store
            .update("INSERT DATA { GRAPH <urn:g> { <urn:a> <urn:p> \"1\" . } }")
            .unwrap();
        let before = fp(&store, "SELECT * WHERE { GRAPH <urn:g> { ?s ?p ?o } }");
        store
            .update("INSERT DATA { GRAPH <urn:g> { <urn:b> <urn:p> \"2\" . } }")
            .unwrap();
        let after = fp(&store, "SELECT * WHERE { GRAPH <urn:g> { ?s ?p ?o } }");
        assert_ne!(before.hash, after.hash);
        assert_eq!(before.rowcount, Some(1));
        assert_eq!(after.rowcount, Some(2));
    }

    #[test]
    fn ask_fingerprint_is_stable() {
        let store = TripleStore::in_memory().unwrap();
        let t = fp(&store, "ASK { }");
        assert_eq!(t.rowcount, None);
        // empty store ASK {} is true; same query is stable
        let t2 = fp(&store, "ASK { }");
        assert_eq!(t.hash, t2.hash);
    }
}
