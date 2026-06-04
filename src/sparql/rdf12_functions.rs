//! SPARQL 1.2 / RDF-star built-in function registration.
#![allow(dead_code)]
//!
//! Oxigraph 0.4 with `rdf-star` (enabled via the `rdf-12` crate feature, and
//! already unconditionally active inside `spargebra`/`spareval` as shipped)
//! handles the native SPARQL 1.2 built-ins — `TRIPLE()`, `SUBJECT()`,
//! `PREDICATE()`, `OBJECT()`, and `isTRIPLE()` — at the query-parser /
//! evaluator level without any custom-function registration.
//!
//! This module provides:
//!
//! 1. A list of the canonical IRIs so they can be referenced elsewhere.
//! 2. A fallback registration of the five functions as *custom* SPARQL
//!    functions under alternative IRIs (matching some legacy or non-standard
//!    client tooling), gated behind `#[cfg(feature = "rdf-12")]`.
//! 3. A JSON serializer for `Term::Triple` results (used in `routes.rs`).

use oxrdf::NamedNode;
use std::sync::Arc;

// ─── Canonical SPARQL 1.2 / RDF 1.2 IRIs ────────────────────────────────────

pub const RDF_TRIPLE: &str    = "http://www.w3.org/1999/02/22-rdf-syntax-ns#triple";
pub const RDF_SUBJECT: &str   = "http://www.w3.org/1999/02/22-rdf-syntax-ns#subject";
pub const RDF_PREDICATE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#predicate";
pub const RDF_OBJECT: &str    = "http://www.w3.org/1999/02/22-rdf-syntax-ns#object";
pub const RDF_IS_TRIPLE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#isTriple";

// ─── Custom function registration (rdf-12 feature) ───────────────────────────

/// Custom function handler type (mirrors `geo::functions`).
type FnHandler = Arc<dyn Fn(&[oxrdf::Term]) -> Option<oxrdf::Term> + Send + Sync>;

/// Returns `(IRI, handler)` pairs for the five SPARQL 1.2 built-ins exposed
/// as *custom* functions under alternative namespace IRIs for compatibility
/// with tools that do not yet speak native SPARQL 1.2.
///
/// These are registered in `store::engine::TripleStore::query_options()` when
/// the `rdf-12` feature is active.
#[cfg(feature = "rdf-12")]
pub fn all_functions() -> Vec<(NamedNode, FnHandler)> {
    use oxrdf::{Literal, Term, Triple};

    let mut fns: Vec<(NamedNode, FnHandler)> = Vec::new();

    // ─── TRIPLE(s, p, o) → triple term ───────────────────────────────────────
    fns.push((
        NamedNode::new(RDF_TRIPLE).expect("RDF_TRIPLE is a valid IRI"),
        Arc::new(|args: &[Term]| {
            if args.len() != 3 {
                return None;
            }
            let s = match &args[0] {
                Term::NamedNode(nn) => oxrdf::Subject::NamedNode(nn.clone()),
                Term::BlankNode(bn) => oxrdf::Subject::BlankNode(bn.clone()),
                _ => return None,
            };
            let p = match &args[1] {
                Term::NamedNode(nn) => nn.clone(),
                _ => return None,
            };
            let o = args[2].clone();
            Some(Term::Triple(Box::new(Triple::new(s, p, o))))
        }),
    ));

    // ─── SUBJECT(tt) → term ──────────────────────────────────────────────────
    fns.push((
        NamedNode::new(RDF_SUBJECT).expect("RDF_SUBJECT is a valid IRI"),
        Arc::new(|args: &[Term]| {
            if let Some(Term::Triple(tt)) = args.first() {
                Some(match tt.subject.clone() {
                    oxrdf::Subject::NamedNode(nn) => Term::NamedNode(nn),
                    oxrdf::Subject::BlankNode(bn) => Term::BlankNode(bn),
                    oxrdf::Subject::Triple(inner) => Term::Triple(inner),
                })
            } else {
                None
            }
        }),
    ));

    // ─── PREDICATE(tt) → named node ──────────────────────────────────────────
    fns.push((
        NamedNode::new(RDF_PREDICATE).expect("RDF_PREDICATE is a valid IRI"),
        Arc::new(|args: &[Term]| {
            if let Some(Term::Triple(tt)) = args.first() {
                Some(Term::NamedNode(tt.predicate.clone()))
            } else {
                None
            }
        }),
    ));

    // ─── OBJECT(tt) → term ───────────────────────────────────────────────────
    fns.push((
        NamedNode::new(RDF_OBJECT).expect("RDF_OBJECT is a valid IRI"),
        Arc::new(|args: &[Term]| {
            if let Some(Term::Triple(tt)) = args.first() {
                Some(tt.object.clone())
            } else {
                None
            }
        }),
    ));

    // ─── isTRIPLE(term) → boolean ────────────────────────────────────────────
    fns.push((
        NamedNode::new(RDF_IS_TRIPLE).expect("RDF_IS_TRIPLE is a valid IRI"),
        Arc::new(|args: &[Term]| {
            let is_triple = matches!(args.first(), Some(Term::Triple(_)));
            Some(Term::Literal(Literal::from(is_triple)))
        }),
    ));

    fns
}

// ─── SPARQL 1.2 ADJUST function ──────────────────────────────────────────────

/// IRI for the SPARQL 1.2 ADJUST function.
pub const SPARQL_ADJUST: &str = "http://www.w3.org/ns/sparql#adjust";

/// Returns the SPARQL 1.2 ADJUST function as a custom function handler.
///
/// `ADJUST(dateTime, duration)` adds a duration to a dateTime value.
/// `ADJUST(dateTime, timezone)` adjusts the timezone of a dateTime.
///
/// This implements the W3C SPARQL 1.2 Working Draft ADJUST function.
pub fn adjust_function() -> (NamedNode, FnHandler) {
    use oxrdf::{Literal, Term};

    (
        NamedNode::new_unchecked(SPARQL_ADJUST),
        Arc::new(|args: &[Term]| {
            if args.len() != 2 {
                return None;
            }

            let dt_lit = match &args[0] {
                Term::Literal(lit) => lit,
                _ => return None,
            };

            let tz_lit = match &args[1] {
                Term::Literal(lit) => lit,
                _ => return None,
            };

            let dt_str = dt_lit.value();
            let tz_str = tz_lit.value();

            // Parse the dateTime
            let dt = chrono::NaiveDateTime::parse_from_str(dt_str, "%Y-%m-%dT%H:%M:%S")
                .or_else(|_| chrono::NaiveDateTime::parse_from_str(dt_str, "%Y-%m-%dT%H:%M:%S%.f"))
                .ok();

            // Also try parsing as DateTime with timezone
            let dt_with_tz = chrono::DateTime::parse_from_rfc3339(dt_str).ok();

            // Try to interpret the second argument as a timezone offset (e.g., "+05:00", "-03:00", "Z")
            let target_tz = parse_timezone(tz_str);

            if let Some(tz) = target_tz {
                // Adjust timezone
                if let Some(dt_tz) = dt_with_tz {
                    let adjusted = dt_tz.with_timezone(&tz);
                    let result = adjusted.to_rfc3339();
                    return Some(Term::Literal(Literal::new_typed_literal(
                        result,
                        NamedNode::new_unchecked("http://www.w3.org/2001/XMLSchema#dateTime"),
                    )));
                } else if let Some(naive) = dt {
                    // Apply timezone to naive datetime
                    let adjusted = naive.and_utc().with_timezone(&tz);
                    let result = adjusted.to_rfc3339();
                    return Some(Term::Literal(Literal::new_typed_literal(
                        result,
                        NamedNode::new_unchecked("http://www.w3.org/2001/XMLSchema#dateTime"),
                    )));
                }
            }

            // Try to interpret as xsd:dayTimeDuration (e.g., "PT5H", "P1D", "-PT2H30M")
            if let Some(duration) = parse_xsd_duration(tz_str) {
                if let Some(dt_tz) = dt_with_tz {
                    let adjusted = dt_tz + duration;
                    let result = adjusted.to_rfc3339();
                    return Some(Term::Literal(Literal::new_typed_literal(
                        result,
                        NamedNode::new_unchecked("http://www.w3.org/2001/XMLSchema#dateTime"),
                    )));
                } else if let Some(naive) = dt {
                    let adjusted = naive + duration;
                    let result = adjusted.format("%Y-%m-%dT%H:%M:%S").to_string();
                    return Some(Term::Literal(Literal::new_typed_literal(
                        result,
                        NamedNode::new_unchecked("http://www.w3.org/2001/XMLSchema#dateTime"),
                    )));
                }
            }

            None
        }),
    )
}

/// Parse a timezone string like "+05:00", "-03:00", "Z", "UTC".
fn parse_timezone(s: &str) -> Option<chrono::FixedOffset> {
    let s = s.trim();
    if s == "Z" || s.eq_ignore_ascii_case("UTC") {
        return chrono::FixedOffset::east_opt(0);
    }
    // Parse +HH:MM or -HH:MM
    if (s.starts_with('+') || s.starts_with('-')) && s.len() >= 5 {
        let sign = if s.starts_with('+') { 1 } else { -1 };
        let parts: Vec<&str> = s[1..].split(':').collect();
        if parts.len() == 2 {
            let hours: i32 = parts[0].parse().ok()?;
            let minutes: i32 = parts[1].parse().ok()?;
            let total_seconds = sign * (hours * 3600 + minutes * 60);
            return chrono::FixedOffset::east_opt(total_seconds);
        }
    }
    None
}

/// Parse an xsd:dayTimeDuration string like "PT5H", "P1DT2H30M", "-PT2H30M".
fn parse_xsd_duration(s: &str) -> Option<chrono::Duration> {
    let s = s.trim();
    let (negative, s) = if let Some(rest) = s.strip_prefix('-') {
        (true, rest)
    } else {
        (false, s)
    };

    let s = s.strip_prefix('P')?;
    let mut total_seconds: i64 = 0;

    // Parse days (before T)
    let (days_part, time_part) = if let Some(t_pos) = s.find('T') {
        (&s[..t_pos], &s[t_pos + 1..])
    } else {
        (s, "")
    };

    // Parse days
    if let Some(d_pos) = days_part.find('D') {
        let days: i64 = days_part[..d_pos].parse().ok()?;
        total_seconds += days * 86400;
    }

    // Parse time components
    let mut remaining = time_part;
    if let Some(h_pos) = remaining.find('H') {
        let hours: i64 = remaining[..h_pos].parse().ok()?;
        total_seconds += hours * 3600;
        remaining = &remaining[h_pos + 1..];
    }
    if let Some(m_pos) = remaining.find('M') {
        let minutes: i64 = remaining[..m_pos].parse().ok()?;
        total_seconds += minutes * 60;
        remaining = &remaining[m_pos + 1..];
    }
    if let Some(s_pos) = remaining.find('S') {
        let secs: f64 = remaining[..s_pos].parse().ok()?;
        total_seconds += secs as i64;
    }

    if negative {
        total_seconds = -total_seconds;
    }

    Some(chrono::Duration::seconds(total_seconds))
}

/// Serialize a `Term::Triple` as a SPARQL Results JSON object.
///
/// ```json
/// { "type": "triple", "value": { "subject": {...}, "predicate": {...}, "object": {...} } }
/// ```
///
/// Used in `server::routes::format_term`.
#[cfg(feature = "rdf-12")]
pub fn triple_term_to_json(tt: &oxrdf::Triple) -> serde_json::Value {
    use oxrdf::Term;

    fn fmt(t: &Term) -> serde_json::Value {
        match t {
            Term::NamedNode(nn) => serde_json::json!({"type":"uri","value":nn.as_str()}),
            Term::BlankNode(bn) => serde_json::json!({"type":"bnode","value":bn.as_str()}),
            Term::Literal(lit) => {
                let mut obj = serde_json::json!({
                    "type": "literal",
                    "value": lit.value(),
                });
                if let Some(lang) = lit.language() {
                    obj["xml:lang"] = serde_json::json!(lang);
                } else {
                    let dt = lit.datatype().as_str();
                    if dt != "http://www.w3.org/2001/XMLSchema#string" {
                        obj["datatype"] = serde_json::json!(dt);
                    }
                }
                obj
            }
            Term::Triple(inner) => triple_term_to_json(inner),
        }
    }

    let subj: Term = match tt.subject.clone() {
        oxrdf::Subject::NamedNode(nn) => Term::NamedNode(nn),
        oxrdf::Subject::BlankNode(bn) => Term::BlankNode(bn),
        oxrdf::Subject::Triple(inner) => Term::Triple(inner),
    };

    serde_json::json!({
        "type": "triple",
        "value": {
            "subject":   fmt(&subj),
            "predicate": fmt(&Term::NamedNode(tt.predicate.clone())),
            "object":    fmt(&tt.object),
        }
    })
}
