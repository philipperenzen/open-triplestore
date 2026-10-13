//! What a run checks beyond the mapping's own shape: that every column the
//! mapping names exists in its logical source, and what happens to a row
//! whose values cannot become the terms the mapping asks for.
//!
//! **Data errors** (R2RML §4.3) are a value that would make an invalid RDF
//! term: an IRI-typed term map whose value is not a valid IRI, or a literal
//! whose `rr:datatype` overrides the natural one and whose value is not in
//! that datatype's lexical space. R2RML says to abort and report. That is
//! the default here ([`OnDataError::Abort`]): the run writes nothing and its
//! error names the offending rows. A run may instead opt into
//! [`OnDataError::Skip`], which leaves out the offending terms — as this
//! engine used to, silently — and reports every row it skipped from.

use std::collections::HashSet;

use serde::Serialize;
use utoipa::ToSchema;

use super::model::*;
use super::terms::FN_MINT_IRI;

/// How many offending rows a report names. The count is always complete.
pub const DATA_ERROR_SAMPLE: usize = 10;

/// What a run does when a row raises a data error.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum OnDataError {
    /// Stop, write nothing, and report the first offending rows (R2RML §4.3).
    #[default]
    Abort,
    /// Leave the offending terms out, keep going, and report every row
    /// skipped from.
    Skip,
}

impl OnDataError {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "abort" | "strict" => Some(Self::Abort),
            "skip" | "lenient" => Some(Self::Skip),
            _ => None,
        }
    }
}

/// The rows of a run that raised data errors.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DataErrors {
    /// Rows that raised at least one data error.
    pub rows: u64,
    /// The first [`DATA_ERROR_SAMPLE`] of them: the triples map, the row's
    /// position in its source, and what went wrong.
    pub first: Vec<String>,
}

impl DataErrors {
    pub fn is_empty(&self) -> bool {
        self.rows == 0
    }

    /// Whether the sample is full — what an aborting run waits for before it
    /// stops reading.
    pub fn sample_full(&self) -> bool {
        self.first.len() >= DATA_ERROR_SAMPLE
    }

    /// Record the data errors one row raised. `row` counts from 1 in the
    /// order the source delivered it.
    pub fn record(&mut self, triples_map: &str, row: u64, errors: Vec<String>) {
        if errors.is_empty() {
            return;
        }
        self.rows += 1;
        if !self.sample_full() {
            self.first
                .push(format!("<{triples_map}> row {row}: {}", errors.join("; ")));
        }
    }

    pub fn merge(&mut self, other: DataErrors) {
        self.rows += other.rows;
        for line in other.first {
            if !self.sample_full() {
                self.first.push(line);
            }
        }
    }

    /// The error an aborting run fails with.
    pub fn abort_message(&self) -> String {
        let more = if self.rows > self.first.len() as u64 {
            format!(" (the first {} shown)", self.first.len())
        } else {
            String::new()
        };
        format!(
            "data error: {} row{} would generate an invalid RDF term{more}, so the run was \
             aborted and nothing was written (R2RML §4.3). {}. To leave those terms out and \
             have the rows reported instead, run with onDataError \"skip\" \
             (?on_data_error=skip on the file-mapping endpoints).",
            self.rows,
            if self.rows == 1 { "" } else { "s" },
            self.first.join(" | ")
        )
    }
}

/// One column a triples map needs from its logical source, and where the
/// mapping names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Needed {
    pub column: String,
    pub place: String,
    /// An `otsfn:mintIri` `{x_slug}` placeholder, which column `x` satisfies.
    pub slug: bool,
}

const MINT_PLACE: &str = "an otsfn:mintIri template";

/// Every column `tm` reads from its own rows — its term maps, function
/// parameters and the child side of its joins — plus the parent side of every
/// join that names `tm` as its parent.
pub fn needed_columns(mapping: &RmlMapping, tm: &TriplesMap) -> Vec<Needed> {
    let mut out: Vec<Needed> = Vec::new();
    let mut add = |column: String, place: &str| {
        if !out.iter().any(|n| n.column == column) {
            out.push(Needed {
                slug: place == MINT_PLACE && column.ends_with("_slug"),
                column,
                place: place.to_string(),
            });
        }
    };
    let term = |add: &mut dyn FnMut(String, &str), t: &TermMap, place: &str| {
        for c in t.referenced_columns() {
            add(c, place);
        }
    };
    let function = |add: &mut dyn FnMut(String, &str), f: &FunctionMap, place: &str| {
        for args in f.params.values() {
            for a in args {
                if let FunctionArg::Reference(c) = a {
                    add(c.clone(), place);
                }
            }
        }
        if f.function == FN_MINT_IRI {
            for a in f.params.values().flatten() {
                if let FunctionArg::Constant(t) = a {
                    for c in template_columns(t) {
                        add(c, MINT_PLACE);
                    }
                }
            }
        }
    };

    let sm = &tm.subject_map;
    match &sm.function {
        Some(f) => function(&mut add, f, "the subject map's function"),
        None => term(&mut add, &sm.term_map, "the subject map"),
    }
    for g in &sm.graph_maps {
        term(&mut add, g, "a subject graph map");
    }
    for pom in &tm.predicate_object_maps {
        for p in &pom.predicate_maps {
            term(&mut add, p, "a predicate map");
        }
        for g in &pom.graph_maps {
            term(&mut add, g, "a predicate-object graph map");
        }
        for o in &pom.object_maps {
            match o {
                ObjectMap::Term(t) => term(&mut add, t, "an object map"),
                ObjectMap::Function(f) => function(&mut add, f, "an object map's function"),
                ObjectMap::Ref(r) => {
                    for j in &r.joins {
                        for c in j.child.referenced_columns() {
                            add(c, "rr:child of a join condition");
                        }
                    }
                }
            }
        }
    }
    for other in &mapping.triples_maps {
        for r in other.refs().filter(|r| r.parent_triples_map == tm.iri) {
            for j in &r.joins {
                for c in j.parent.referenced_columns() {
                    add(c, "rr:parent of a join condition");
                }
            }
        }
    }
    out
}

/// Refuse a triples map that names a column its logical source does not
/// have (R2RML §6: "the referenced columns of all term maps of a triples map
/// must be column names that exist in the term map's logical table"), and a
/// relational result with two columns of one name (R2RML §5.2).
pub fn check_columns(
    mapping: &RmlMapping,
    tm: &TriplesMap,
    available: &[String],
) -> Result<(), String> {
    let mut seen: HashSet<&str> = HashSet::new();
    if tm.logical_source.reference_formulation == ReferenceFormulation::Sql {
        if let Some(dup) = available.iter().find(|c| !seen.insert(c.as_str())) {
            return Err(format!(
                "the logical source of TriplesMap <{}> returns two columns named \"{dup}\"; \
                 R2RML requires distinct column names (§5.2) — alias one of them",
                tm.iri
            ));
        }
    } else {
        seen = available.iter().map(String::as_str).collect();
    }
    let missing: Vec<Needed> = needed_columns(mapping, tm)
        .into_iter()
        .filter(|n| {
            !seen.contains(n.column.as_str())
                && !(n.slug
                    && n.column
                        .strip_suffix("_slug")
                        .is_some_and(|base| seen.contains(base)))
        })
        .collect();
    if missing.is_empty() {
        return Ok(());
    }
    let mut columns: Vec<&str> = available.iter().map(String::as_str).collect();
    let shown = columns.len().min(30);
    let more = columns.len() - shown;
    columns.truncate(shown);
    let names: Vec<String> = missing
        .iter()
        .map(|n| format!("\"{}\" (in {})", n.column, n.place))
        .collect();
    Err(format!(
        "TriplesMap <{}> reads {} that its logical source does not have: {}. The source has {}{}",
        tm.iri,
        if missing.len() == 1 {
            "a column"
        } else {
            "columns"
        },
        names.join(", "),
        if columns.is_empty() {
            "no columns".to_string()
        } else {
            columns
                .iter()
                .map(|c| format!("\"{c}\""))
                .collect::<Vec<_>>()
                .join(", ")
        },
        if more > 0 {
            format!(" and {more} more")
        } else {
            String::new()
        }
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rml::parse_rml;

    const PFX: &str = "@prefix rr: <http://www.w3.org/ns/r2rml#> .\n\
@prefix rml: <http://semweb.mmlab.be/ns/rml#> .\n\
@prefix fnml: <http://semweb.mmlab.be/ns/fnml#> .\n\
@prefix fno: <https://w3id.org/function/ontology#> .\n\
@prefix fn: <https://w3id.org/open-triplestore/fn#> .\n\
@prefix ex: <http://example.org/> .\n";

    fn mapping() -> RmlMapping {
        parse_rml(&format!(
            "{PFX}
             ex:C a rr:TriplesMap ;
               rml:logicalSource [ rml:source <urn:source:s> ; rr:tableName \"child\" ] ;
               rr:subjectMap [ rr:template \"http://x/c{{cid}}\" ; rr:graphMap [ rr:template \"http://g/{{g}}\" ] ] ;
               rr:predicateObjectMap [ rr:predicateMap [ rr:template \"http://p/{{k}}\" ] ;
                 rr:objectMap [ rr:column \"v\" ] ;
                 rr:objectMap [ rr:parentTriplesMap ex:P ;
                                rr:joinCondition [ rr:child \"pid\" ; rr:parent \"id\" ] ] ;
                 rr:objectMap [ fnml:functionValue [
                   rr:predicateObjectMap [ rr:predicate fno:executes ; rr:object fn:mintIri ] ;
                   rr:predicateObjectMap [ rr:predicate fn:template ; rr:object \"http://m/{{name_slug}}\" ] ] ] ] .
             ex:P a rr:TriplesMap ;
               rml:logicalSource [ rml:source <urn:source:s> ; rr:tableName \"parent\" ] ;
               rr:subjectMap [ rr:template \"http://x/p{{id}}\" ] ."
        ))
        .unwrap()
    }

    fn columns(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn every_place_a_column_is_named_is_checked() {
        let m = mapping();
        let child = m.find("http://example.org/C").unwrap();
        let mut needed: Vec<String> = needed_columns(&m, child)
            .into_iter()
            .map(|n| n.column)
            .collect();
        needed.sort();
        assert_eq!(needed, ["cid", "g", "k", "name_slug", "pid", "v"]);
        // The parent needs its own subject column and the join's parent side.
        let parent = m.find("http://example.org/P").unwrap();
        let mut needed: Vec<String> = needed_columns(&m, parent)
            .into_iter()
            .map(|n| n.column)
            .collect();
        needed.sort();
        assert_eq!(needed, ["id"]);

        // A `{name_slug}` mintIri placeholder is satisfied by `name`.
        check_columns(&m, child, &columns(&["cid", "g", "k", "name", "pid", "v"])).unwrap();
        let err = check_columns(&m, child, &columns(&["cid", "g", "k", "name", "v"])).unwrap_err();
        assert!(
            err.contains("\"pid\" (in rr:child of a join condition)"),
            "{err}"
        );
    }

    #[test]
    fn a_relational_result_must_not_repeat_a_column() {
        let m = mapping();
        let parent = m.find("http://example.org/P").unwrap();
        let err = check_columns(&m, parent, &columns(&["id", "x", "x"])).unwrap_err();
        assert!(err.contains("two columns named \"x\""), "{err}");
    }

    #[test]
    fn a_report_counts_every_row_and_names_the_first_few() {
        let mut d = DataErrors::default();
        for row in 1..=15 {
            d.record("http://example.org/M", row, vec![format!("bad {row}")]);
        }
        d.record("http://example.org/M", 16, Vec::new());
        assert_eq!(d.rows, 15);
        assert_eq!(d.first.len(), DATA_ERROR_SAMPLE);
        assert!(d.sample_full());
        let msg = d.abort_message();
        assert!(
            msg.contains("15 rows") && msg.contains("the first 10 shown"),
            "{msg}"
        );
        assert!(msg.contains("<http://example.org/M> row 1: bad 1"), "{msg}");
        assert_eq!(OnDataError::parse(" Skip "), Some(OnDataError::Skip));
        assert_eq!(OnDataError::parse("nope"), None);
    }
}
