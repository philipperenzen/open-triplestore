//! Preprocessing of the `text:search` / `ft:search` magic property pattern.
//!
//! The SPARQL magic property syntax:
//! ```sparql
//! PREFIX ft: <tag:open-triplestore,2024:ft:>
//! (?s ?score) ft:search("query string" <predicate-iri> limit) .
//! ```
//!
//! is not valid SPARQL, so it must be rewritten *before* the query reaches
//! Oxigraph's parser.  This module detects the pattern and replaces it with a
//! SPARQL `VALUES` clause injected with the Tantivy search results.
//!
//! ## Replacement
//! ```sparql
//! VALUES (?s ?score) {
//!     (<http://ex.org/s1> "0.95"^^<http://www.w3.org/2001/XMLSchema#float>)
//!     (<http://ex.org/s2> "0.87"^^<http://www.w3.org/2001/XMLSchema#float>)
//! }
//! ```
//!
//! Both the historical `text:` spelling (`http://oxigraph.org/text#`) and the
//! documented `ft:` spelling (`tag:open-triplestore,2024:ft:`) are accepted.

use super::index::{GraphScope, GraphScopeOwned, MatchAnchor, MatchCase, TextIndex};
use std::collections::HashSet;
use std::sync::Arc;
use tracing::debug;

const XSD_FLOAT: &str = "http://www.w3.org/2001/XMLSchema#float";

/// Every accepted spelling of the magic property.
///
/// Matched directly rather than normalised to one form with a string replace:
/// a blind replace also rewrites the same text inside a string literal, which
/// would quietly change what a query like `FILTER(CONTAINS(?x, "ft:search"))`
/// asks for.
const MARKERS: [&str; 4] = [
    "text:search",
    "ft:search",
    "<http://oxigraph.org/text#search>",
    "<tag:open-triplestore,2024:ft:search>",
];

/// Detect and expand `text:search` / `ft:search` magic property patterns.
///
/// `scope` restricts hits to the graphs the caller may read. The expansion can
/// collapse to nothing but a `VALUES` clause, which carries no triple pattern
/// for the endpoint's `FROM` scoping to constrain, so this is the only thing
/// keeping private subject IRIs out of the results.
///
/// If no pattern is found the original string is returned unchanged.
pub fn preprocess_text_search(sparql: &str, index: &TextIndex, scope: GraphScope<'_>) -> String {
    if !mentions_text_search(sparql) {
        return sparql.to_string();
    }
    replace_text_search_patterns(sparql, index, scope)
}

/// The first magic-property occurrence in `s`, as `(offset, spelling length)`.
fn next_marker(s: &str) -> Option<(usize, usize)> {
    MARKERS
        .iter()
        .filter_map(|m| s.find(m).map(|pos| (pos, m.len())))
        // Earliest match wins; on a tie the longest spelling does, so the IRI
        // forms are not clipped by the prefixed ones they contain.
        .min_by_key(|&(pos, len)| (pos, std::cmp::Reverse(len)))
}

/// The index-side read boundary for a caller.
///
/// Admins read every registered graph, which is exactly what the SPARQL
/// endpoint's `FROM` branch grants them; everyone else is held to the graph set
/// their query was scoped to.
pub fn graph_scope(is_admin: bool, accessible: Arc<HashSet<String>>) -> GraphScopeOwned {
    if is_admin {
        GraphScopeOwned::All
    } else {
        GraphScopeOwned::Only(accessible)
    }
}

/// Whether `sparql` can be affected by the text index at all.
///
/// Used to decide if a stale index must be rebuilt before the query runs.
/// A rebuild reads the whole store, so a query that neither searches nor
/// filters on text should never trigger one.
pub fn query_uses_text_index(sparql: &str) -> bool {
    if mentions_text_search(sparql) {
        return true;
    }
    let upper = sparql.to_uppercase();
    upper.contains("CONTAINS") || upper.contains("STRSTARTS")
}

/// Cheap pre-check: does this query mention full-text search at all?
///
/// Public because the dirty-index gate treats the two index consumers
/// differently: a `text:search` query NEEDS the index (its expansion is the
/// result set), while `CONTAINS`/`STRSTARTS` push-down is optional.
pub fn mentions_text_search(sparql: &str) -> bool {
    next_marker(sparql).is_some()
}

/// Walk the query string and replace each `text:search` invocation with a
/// `VALUES` clause containing the Tantivy results.
fn replace_text_search_patterns(sparql: &str, index: &TextIndex, scope: GraphScope<'_>) -> String {
    // Pattern: (?s ?score) text:search ("query" [<pred>] [limit]) .
    //
    // We use a simple hand-written parser rather than a regex to avoid
    // pulling in the `regex` crate as a mandatory dependency.
    let mut result = String::with_capacity(sparql.len() + 512);
    let mut remaining = sparql;

    while let Some((pos, marker_len)) = next_marker(remaining) {
        let before = &remaining[..pos];
        let after = &remaining[pos..];

        match try_expand(before, after, marker_len, index, scope) {
            // The `(?s ?score)` tuple belongs to the invocation: it is the
            // *subject* of the magic property, and the `VALUES` clause we
            // generate binds those same variables. Copying it through would
            // leave `(?s ?score) VALUES (?s ?score) {…}` behind — which parses
            // (SPARQL reads `(…)` as an RDF collection) and then silently
            // matches nothing, because no such list exists in the data.
            Some(Expansion {
                values,
                tuple_start,
            }) => {
                result.push_str(&before[..tuple_start]);
                result.push_str(&values);
                match find_pattern_end(after, marker_len) {
                    Some(end) => remaining = &remaining[pos + end..],
                    None => {
                        // Unreachable while `try_expand` and `find_pattern_end`
                        // agree on what an invocation looks like; kept so a
                        // future divergence degrades instead of looping.
                        result.push_str(&after[..marker_len]);
                        remaining = &remaining[pos + marker_len..];
                    }
                }
            }
            None => {
                // Not a valid invocation, keep as-is.
                result.push_str(before);
                result.push_str(&after[..marker_len]);
                remaining = &remaining[pos + marker_len..];
            }
        }
    }

    result.push_str(remaining);
    result
}

/// A successful expansion: the `VALUES` clause, and where in the preceding text
/// the `(?s ?score)` tuple it replaces begins.
struct Expansion {
    values: String,
    tuple_start: usize,
}

/// Try to parse and expand a single `text:search` invocation.
fn try_expand(
    before: &str,
    after: &str,
    marker_len: usize,
    index: &TextIndex,
    scope: GraphScope<'_>,
) -> Option<Expansion> {
    // Find the `(?varS ?varScore)` tuple immediately before `text:search`
    let before_trimmed = before.trim_end();
    let tuple_end = before_trimmed.len();
    let tuple_start = before_trimmed.rfind('(')?;
    let tuple = &before_trimmed[tuple_start..tuple_end];
    if !tuple.starts_with('(') || !tuple.ends_with(')') {
        return None;
    }

    // Extract variable names from the tuple like (?s ?score)
    let inner = &tuple[1..tuple.len() - 1];
    let vars: Vec<&str> = inner.split_whitespace().collect();
    if vars.len() != 2 {
        return None;
    }
    if !vars.iter().all(|v| v.starts_with('?') && v.len() > 1) {
        return None;
    }
    let var_s = vars[0].trim_start_matches('?');
    let var_score = vars[1].trim_start_matches('?');

    // Find the argument list after `text:search`
    let after_trimmed = after[marker_len..].trim_start();
    if !after_trimmed.starts_with('(') {
        return None;
    }
    let arg_end = find_args_end(after_trimmed)?;
    let arg_str = &after_trimmed[1..arg_end];

    // Parse arguments: "query string" [<predicate-iri>] [limit]
    let (query_str, pred_filter, limit) = parse_search_args(arg_str)?;

    debug!(
        "text:search expanding: query='{}' pred={:?} limit={}",
        query_str, pred_filter, limit
    );

    // Execute the search. A failed search must not leave the raw magic
    // property in the query — that is a syntax error at the engine. An empty
    // `VALUES` is the honest answer: the pattern matched nothing.
    let hits = index
        .search(&query_str, pred_filter.as_deref(), scope, limit)
        .unwrap_or_else(|e| {
            debug!("text:search failed ({e}); expanding to no results");
            Vec::new()
        });

    // Build VALUES clause. One row per subject: the same subject can carry
    // several matching literals, and repeating it would multiply every join it
    // takes part in.
    let mut seen = std::collections::HashSet::new();
    let mut values = format!("VALUES (?{var_s} ?{var_score}) {{\n");
    for hit in &hits {
        if !seen.insert(hit.subject.clone()) {
            continue;
        }
        values.push_str(&format!(
            "  (<{}> \"{:.6}\"^^<{XSD_FLOAT}>)\n",
            hit.subject, hit.score
        ));
    }
    values.push('}');

    Some(Expansion {
        values,
        tuple_start,
    })
}

/// Byte offset of the `)` closing the argument list that `s` opens with.
///
/// Quoted strings are skipped so a `)` inside the query text does not end the
/// list early, and nesting is tracked so `CONTAINS(LCASE(?l), "x")` closes on
/// its own paren rather than `LCASE`'s.
fn find_args_end(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    debug_assert_eq!(bytes.first(), Some(&b'('));
    let mut i = 1;
    let mut depth = 1usize;
    let mut quote: Option<u8> = None;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if quote.is_some() => i += 1,
            b'"' | b'\'' => match quote {
                Some(q) if q == bytes[i] => quote = None,
                Some(_) => {}
                None => quote = Some(bytes[i]),
            },
            b'(' if quote.is_none() => depth += 1,
            b')' if quote.is_none() => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Parse `"query" [<pred>] [limit]` from the argument list string.
fn parse_search_args(args: &str) -> Option<(String, Option<String>, usize)> {
    let args = args.trim();

    // Extract the query string (double-quoted)
    if !args.starts_with('"') {
        return None;
    }
    let end_quote = args[1..].find('"')? + 1;
    let query = args[1..end_quote].to_string();
    let rest = args[end_quote + 1..].trim();

    let mut pred_filter: Option<String> = None;
    let mut limit = 10usize;

    // Optional predicate IRI in <...>
    let mut rest = rest;
    if rest.starts_with('<') {
        let close = rest.find('>')?;
        pred_filter = Some(rest[1..close].to_string());
        rest = rest[close + 1..].trim();
    }

    // Optional numeric limit
    if !rest.is_empty() {
        if let Ok(n) = rest.trim().parse::<usize>() {
            limit = n;
        }
    }

    Some((query, pred_filter, limit))
}

/// Find the position *after* the full `text:search (...)` pattern in `after`.
fn find_pattern_end(after: &str, marker_len: usize) -> Option<usize> {
    let stripped = &after[marker_len..];
    let trimmed = stripped.trim_start();
    if !trimmed.starts_with('(') {
        return None;
    }
    let offset = stripped.len() - trimmed.len();
    let close = find_args_end(trimmed)?;
    // Include trailing whitespace and optional dot
    let remainder = trimmed[close + 1..].trim_start();
    let dot_skip = if remainder.starts_with('.') {
        (trimmed[close + 1..].len() - remainder.len()) + 1
    } else {
        0
    };
    Some(marker_len + offset + close + 1 + dot_skip)
}

// ─── CONTAINS / STRSTARTS → Tantivy push-down ────────────────────────────────

/// Detect `FILTER(CONTAINS(?var, "str"))` / `FILTER(STRSTARTS(?var, "str"))`
/// and prepend a `VALUES` clause with the matching subjects.
///
/// The original FILTER is preserved, so the `VALUES` clause only has to be a
/// *superset* of the true answer — it prunes candidates, the FILTER decides.
/// That superset property is the whole safety argument, and it is easy to lose:
///
/// * Tokenized matching is **not** a superset of substring matching, so the
///   candidates come from [`TextIndex::search_substring`] (a regex over whole,
///   un-tokenized literals), never from the relevance index.
/// * The candidate set is only used when the index reports it as complete —
///   a truncated or unrepresentable set leaves the query untouched.
/// * The `VALUES` clause is injected into the top-level group, so it may only
///   be built from a FILTER that also sits in the top-level group. A FILTER
///   inside `OPTIONAL`, `MINUS`, `UNION`, `NOT EXISTS` or a subquery constrains
///   its own group; hoisting it changes the meaning of the query (an `OPTIONAL`
///   would start dropping rows, a `NOT EXISTS` would invert).
///
/// `REGEX` is deliberately not pushed down: SPARQL uses XPath regexes and the
/// index uses `regex-automata`, and a dialect mismatch here does not merely
/// slow a query down, it silently changes its answer.
pub fn preprocess_substring_pushdown(
    sparql: &str,
    index: &TextIndex,
    scope: GraphScope<'_>,
) -> String {
    if !index.substring_pushdown_available() {
        return sparql.to_string();
    }
    let upper = sparql.to_uppercase();
    if !upper.contains("CONTAINS") && !upper.contains("STRSTARTS") {
        return sparql.to_string();
    }

    let Some(where_brace) = find_where_brace(sparql) else {
        return sparql.to_string();
    };

    let Some(call) = find_pushdown_candidate(sparql, where_brace) else {
        return sparql.to_string();
    };

    let Some(subject_var) = find_subject_for_object_var(sparql, &call.var_name) else {
        return sparql.to_string();
    };

    let candidates = match index.search_substring(&call.needle, call.anchor, call.case, scope) {
        Ok(c) => c,
        Err(e) => {
            debug!("substring push-down skipped: {e}");
            return sparql.to_string();
        }
    };
    if !candidates.complete {
        return sparql.to_string();
    }

    debug!(
        "CONTAINS/STRSTARTS push-down: '{}' matched {} candidates via Tantivy",
        call.needle,
        candidates.subjects.len()
    );

    let mut values = format!("VALUES (?{subject_var}) {{\n");
    for subject in &candidates.subjects {
        values.push_str(&format!("  (<{subject}>)\n"));
    }
    values.push_str("}\n");

    let mut new_query = String::with_capacity(sparql.len() + values.len());
    new_query.push_str(&sparql[..where_brace + 1]);
    new_query.push('\n');
    new_query.push_str(&values);
    new_query.push_str(&sparql[where_brace + 1..]);
    new_query
}

/// A substring FILTER that is safe to push down.
struct PushdownCall {
    var_name: String,
    needle: String,
    anchor: MatchAnchor,
    case: MatchCase,
}

/// Find the first `CONTAINS`/`STRSTARTS` filter that sits in the top-level
/// group of the WHERE clause.
///
/// Anything nested — `{`, `OPTIONAL`, `MINUS`, `UNION`, a subquery — ends the
/// search rather than being skipped past: a filter beyond that point may belong
/// to an inner group, and there is no cheap way to tell which from a scan.
fn find_pushdown_candidate(sparql: &str, where_brace: usize) -> Option<PushdownCall> {
    let body = &sparql[where_brace + 1..];
    let upper = body.to_uppercase();
    let bytes = body.as_bytes();

    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            // A nested group starts here; everything past it is out of reach.
            b'{' | b'}' => return None,
            b'"' | b'\'' => {
                let quote = bytes[i];
                i += 1;
                while i < bytes.len() && bytes[i] != quote {
                    if bytes[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
            }
            b'<' => {
                // An IRI: skip it so its text cannot be mistaken for syntax.
                while i < bytes.len() && bytes[i] != b'>' {
                    i += 1;
                }
            }
            b'#' => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            _ => {
                if let Some(call) = parse_substring_call(body, &upper, i) {
                    return Some(call);
                }
            }
        }
        i += 1;
    }
    None
}

/// Parse `CONTAINS(<arg>, "needle")` / `STRSTARTS(<arg>, "needle")` starting at
/// `at`, where `<arg>` is `?var`, `LCASE(?var)` or `UCASE(?var)`.
fn parse_substring_call(body: &str, upper: &str, at: usize) -> Option<PushdownCall> {
    let (name, anchor) = if upper[at..].starts_with("CONTAINS") {
        ("CONTAINS", MatchAnchor::Anywhere)
    } else if upper[at..].starts_with("STRSTARTS") {
        ("STRSTARTS", MatchAnchor::Prefix)
    } else {
        return None;
    };
    // Must be a whole token, not the tail of a longer name.
    if at > 0 && is_name_byte(body.as_bytes()[at - 1]) {
        return None;
    }

    let rest = body[at + name.len()..].trim_start();
    if !rest.starts_with('(') {
        return None;
    }
    let close = find_args_end(rest)?;
    let args = &rest[1..close];

    let (first, second) = split_top_level_comma(args)?;

    // First argument: `?var`, or a case-folding wrapper around one.
    let first = first.trim();
    let upper_first = first.to_uppercase();
    let (var_expr, case) = if let Some(inner) = upper_first
        .strip_prefix("LCASE(")
        .or_else(|| upper_first.strip_prefix("UCASE("))
    {
        if !inner.ends_with(')') {
            return None;
        }
        let open = first.find('(')?;
        let close_paren = first.rfind(')')?;
        (&first[open + 1..close_paren], MatchCase::Insensitive)
    } else {
        (first, MatchCase::Sensitive)
    };
    let var_expr = var_expr.trim();
    if !var_expr.starts_with('?') || var_expr.len() < 2 {
        return None;
    }
    if !var_expr[1..].bytes().all(is_name_byte) {
        return None;
    }

    // Second argument: a plain string literal, nothing computed.
    let second = second.trim();
    let needle = string_literal(second)?;
    if needle.is_empty() {
        return None;
    }
    // `CONTAINS(LCASE(?x), "Foo")` can never match — the folded side is
    // lower-case. Treating it case-insensitively would wrongly widen the
    // candidate set, so leave the query alone.
    let folded_ok = match (case, upper_first.starts_with("LCASE(")) {
        (MatchCase::Sensitive, _) => true,
        (MatchCase::Insensitive, true) => needle == needle.to_lowercase(),
        (MatchCase::Insensitive, false) => needle == needle.to_uppercase(),
    };
    if !folded_ok {
        return None;
    }

    Some(PushdownCall {
        var_name: var_expr[1..].to_string(),
        needle,
        anchor,
        case,
    })
}

fn is_name_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Split `a, b` on the comma that is not inside quotes or nested parentheses.
fn split_top_level_comma(args: &str) -> Option<(&str, &str)> {
    let bytes = args.as_bytes();
    let mut depth = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'"' | b'\'' => {
                let quote = bytes[i];
                i += 1;
                while i < bytes.len() && bytes[i] != quote {
                    if bytes[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
            }
            b'(' => depth += 1,
            b')' => depth = depth.checked_sub(1)?,
            b',' if depth == 0 => return Some((&args[..i], &args[i + 1..])),
            _ => {}
        }
        i += 1;
    }
    None
}

/// The value of a SPARQL string literal, or `None` if `s` is not exactly one.
///
/// A language tag or datatype suffix is rejected rather than ignored: the
/// caller uses the value to prune results, so anything not fully understood
/// must not be pushed down.
fn string_literal(s: &str) -> Option<String> {
    let s = s.trim();
    let quote = s.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 1usize;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'\\' {
            i += 1;
            let esc = *bytes.get(i)?;
            out.push(match esc {
                b'n' => '\n',
                b't' => '\t',
                b'r' => '\r',
                b'"' => '"',
                b'\'' => '\'',
                b'\\' => '\\',
                // An escape we do not model exactly — refuse rather than guess.
                _ => return None,
            });
            i += 1;
            continue;
        }
        if b == quote as u8 {
            // Must be the very end of the argument.
            return if s[i + 1..].trim().is_empty() {
                Some(out)
            } else {
                None
            };
        }
        let ch = s[i..].chars().next()?;
        out.push(ch);
        i += ch.len_utf8();
    }
    None
}

/// Find the subject variable in a triple pattern `?subj <pred> ?obj_var`.
fn find_subject_for_object_var(sparql: &str, obj_var: &str) -> Option<String> {
    let target = format!("?{obj_var}");
    // Simple heuristic: scan for lines containing the object variable in object position
    for line in sparql.lines() {
        let trimmed = line.trim();
        // Skip FILTER lines, PREFIX lines, etc.
        if trimmed.starts_with("FILTER")
            || trimmed.starts_with("PREFIX")
            || trimmed.starts_with("SELECT")
            || trimmed.starts_with("VALUES")
        {
            continue;
        }
        // Check if this line contains our variable as an object
        if ends_with_var(trimmed, &target)
            || trimmed.contains(&format!("{target} ."))
            || trimmed.contains(&format!("{target} ;"))
        {
            // Extract the subject (first token starting with ?)
            let tokens: Vec<&str> = trimmed.split_whitespace().collect();
            if tokens.len() >= 3 && tokens[0].starts_with('?') && tokens[0].len() > 1 {
                let name = tokens[0][1..].trim_end_matches(|c: char| !is_name_byte(c as u8));
                if !name.is_empty() && name != obj_var {
                    return Some(name.to_string());
                }
            }
        }
    }
    None
}

/// `line` ends with exactly `target`, not with a longer variable it prefixes.
fn ends_with_var(line: &str, target: &str) -> bool {
    line.strip_suffix(target)
        .is_some_and(|head| !head.ends_with(|c: char| is_name_byte(c as u8)))
}

/// Find the position of the opening `{` after `WHERE`.
fn find_where_brace(sparql: &str) -> Option<usize> {
    let upper = sparql.to_uppercase();
    let where_pos = upper.find("WHERE")?;
    let after = &sparql[where_pos..];
    let brace_offset = after.find('{')?;
    Some(where_pos + brace_offset)
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    const LABEL: &str = "http://www.w3.org/2000/01/rdf-schema#label";

    fn indexed(triples: &[(&str, &str, &str, &str)]) -> (tempfile::TempDir, TextIndex) {
        let dir = tempfile::tempdir().unwrap();
        let idx = TextIndex::open(dir.path()).unwrap();
        for (s, p, g, o) in triples {
            idx.index_triple(s, p, g, o).unwrap();
        }
        idx.commit().unwrap();
        (dir, idx)
    }

    fn parse(q: &str) -> spargebra::Query {
        spargebra::SparqlParser::new()
            .parse_query(q)
            .unwrap_or_else(|e| panic!("expanded query must be valid SPARQL: {e}\n---\n{q}\n---"))
    }

    #[test]
    fn test_no_pattern_passthrough() {
        let (_d, idx) = indexed(&[]);
        let q = "SELECT ?s WHERE { ?s a :X }";
        assert_eq!(preprocess_text_search(q, &idx, GraphScope::All), q);
    }

    #[test]
    fn test_parse_search_args_basic() {
        let (q, p, l) = parse_search_args(r#""machine learning""#).unwrap();
        assert_eq!(q, "machine learning");
        assert!(p.is_none());
        assert_eq!(l, 10);
    }

    #[test]
    fn test_parse_search_args_with_pred_and_limit() {
        let (q, p, l) =
            parse_search_args(r#""deep learning" <http://www.w3.org/2000/01/rdf-schema#label> 5"#)
                .unwrap();
        assert_eq!(q, "deep learning");
        assert_eq!(p.unwrap(), "http://www.w3.org/2000/01/rdf-schema#label");
        assert_eq!(l, 5);
    }

    #[test]
    fn expansion_replaces_the_tuple_instead_of_keeping_it() {
        // Regression: the tuple used to survive the rewrite, leaving
        // `(?s ?score) VALUES (?s ?score) {…}`. That parses — SPARQL reads
        // `(…)` as an RDF collection — and then matches nothing at all, so
        // every text:search query quietly returned zero rows.
        let (_d, idx) = indexed(&[("http://ex.org/s1", LABEL, "urn:g", "machine learning")]);

        let q = "PREFIX text: <http://oxigraph.org/text#>\n\
                 SELECT ?s ?score WHERE {\n  (?s ?score) text:search (\"machine\") .\n}";
        let out = preprocess_text_search(q, &idx, GraphScope::All);

        assert!(
            !out.contains("(?s ?score) VALUES"),
            "the tuple must be consumed, got:\n{out}"
        );
        assert!(out.contains("<http://ex.org/s1>"), "got:\n{out}");
        parse(&out);
    }

    #[test]
    fn expanded_query_actually_binds_the_variables() {
        let (_d, idx) = indexed(&[("http://ex.org/s1", LABEL, "urn:g", "machine learning")]);
        let q = "PREFIX text: <http://oxigraph.org/text#>\n\
                 SELECT ?s ?score WHERE {\n  (?s ?score) text:search (\"machine\") .\n}";
        let out = preprocess_text_search(q, &idx, GraphScope::All);

        let store = crate::store::TripleStore::in_memory().unwrap();
        let oxigraph::sparql::QueryResults::Solutions(sols) = store.query(&out).unwrap() else {
            panic!("expected solutions");
        };
        let rows: Vec<_> = sols.flatten().collect();
        assert_eq!(rows.len(), 1, "the VALUES clause must bind a row");
        assert_eq!(
            rows[0].get("s").map(|t| t.to_string()),
            Some("<http://ex.org/s1>".to_string())
        );
    }

    #[test]
    fn ft_search_spelling_is_accepted() {
        let (_d, idx) = indexed(&[("http://ex.org/s1", LABEL, "urn:g", "semantic web")]);
        let q = "PREFIX ft: <tag:open-triplestore,2024:ft:>\n\
                 SELECT ?s ?score WHERE {\n  (?s ?score) ft:search(\"semantic\") .\n}";
        let out = preprocess_text_search(q, &idx, GraphScope::All);
        assert!(out.contains("<http://ex.org/s1>"), "got:\n{out}");
        parse(&out);
    }

    #[test]
    fn expansion_is_scoped_to_readable_graphs() {
        let (_d, idx) = indexed(&[
            ("http://ex.org/pub", LABEL, "urn:public", "shared secret"),
            ("http://ex.org/priv", LABEL, "urn:private", "shared secret"),
        ]);
        let readable: HashSet<String> = ["urn:public".to_string()].into_iter().collect();

        let q = "SELECT ?s ?score WHERE {\n  (?s ?score) text:search (\"secret\") .\n}";
        let out = preprocess_text_search(q, &idx, GraphScope::Only(&readable));

        assert!(out.contains("<http://ex.org/pub>"), "got:\n{out}");
        assert!(
            !out.contains("<http://ex.org/priv>"),
            "a private graph's subjects must not leak, got:\n{out}"
        );
    }

    #[test]
    fn no_hits_expands_to_an_empty_values_clause() {
        let (_d, idx) = indexed(&[("http://ex.org/s1", LABEL, "urn:g", "machine learning")]);
        let q = "SELECT ?s ?score WHERE {\n  (?s ?score) text:search (\"nothingmatches\") .\n}";
        let out = preprocess_text_search(q, &idx, GraphScope::All);
        assert!(!out.contains("text:search"), "got:\n{out}");
        parse(&out);
    }

    #[test]
    fn a_closing_paren_inside_the_query_string_is_not_the_end_of_the_args() {
        let (_d, idx) = indexed(&[("http://ex.org/s1", LABEL, "urn:g", "bridge (steel)")]);
        let q = "SELECT ?s ?score WHERE {\n  (?s ?score) text:search (\"steel)\" 5) .\n}";
        let out = preprocess_text_search(q, &idx, GraphScope::All);
        assert!(!out.contains("text:search"), "got:\n{out}");
        parse(&out);
    }

    #[test]
    fn two_invocations_both_expand() {
        let (_d, idx) = indexed(&[
            ("http://ex.org/a", LABEL, "urn:g", "alpha"),
            ("http://ex.org/b", LABEL, "urn:g", "beta"),
        ]);
        let q = "SELECT * WHERE {\n  (?s1 ?c1) text:search (\"alpha\") .\n  \
                 (?s2 ?c2) text:search (\"beta\") .\n}";
        let out = preprocess_text_search(q, &idx, GraphScope::All);
        assert!(out.contains("<http://ex.org/a>"), "got:\n{out}");
        assert!(out.contains("<http://ex.org/b>"), "got:\n{out}");
        parse(&out);
    }

    #[test]
    fn the_iri_form_of_the_property_expands() {
        let (_d, idx) = indexed(&[("http://ex.org/s1", LABEL, "urn:g", "semantic web")]);
        let q = "SELECT ?s ?score WHERE {\n  (?s ?score) \
                 <http://oxigraph.org/text#search> (\"semantic\") .\n}";
        let out = preprocess_text_search(q, &idx, GraphScope::All);
        assert!(out.contains("<http://ex.org/s1>"), "got:\n{out}");
        parse(&out);
    }

    #[test]
    fn the_property_name_inside_a_literal_is_not_rewritten() {
        // Normalising the spellings with a string replace also rewrote them
        // inside string literals, quietly changing what the query asked for.
        let (_d, idx) = indexed(&[]);
        let q = "SELECT ?s WHERE { ?s ?p ?o . FILTER(CONTAINS(?o, \"ft:search\")) }";
        assert_eq!(preprocess_text_search(q, &idx, GraphScope::All), q);
    }

    #[test]
    fn a_malformed_invocation_is_left_alone() {
        let (_d, idx) = indexed(&[]);
        // No tuple in front — not an invocation we understand.
        let q = "SELECT ?s WHERE { ?s text:search ?o }";
        let out = preprocess_text_search(q, &idx, GraphScope::All);
        assert_eq!(out, q);
    }

    // ─── push-down ────────────────────────────────────────────────────────────

    fn pushdown(q: &str, idx: &TextIndex) -> String {
        preprocess_substring_pushdown(q, idx, GraphScope::All)
    }

    #[test]
    fn contains_pushdown_keeps_substring_matches() {
        // The old push-down asked the tokenized index, which does not match
        // `bridge` inside `Drawbridge`, and the resulting VALUES clause then
        // deleted a row the FILTER would have kept.
        let (_d, idx) = indexed(&[
            ("http://ex.org/a", LABEL, "urn:g", "Drawbridge"),
            ("http://ex.org/b", LABEL, "urn:g", "Tunnel"),
        ]);
        let q = "SELECT ?s WHERE {\n  ?s <http://www.w3.org/2000/01/rdf-schema#label> ?l .\n  \
                 FILTER(CONTAINS(?l, \"bridge\"))\n}";
        let out = pushdown(q, &idx);

        assert!(out.contains("VALUES (?s)"), "expected a push-down:\n{out}");
        assert!(
            out.contains("<http://ex.org/a>"),
            "a substring match must survive the push-down:\n{out}"
        );
        assert!(!out.contains("<http://ex.org/b>"), "got:\n{out}");
        parse(&out);
    }

    #[test]
    fn lcase_wrapped_contains_is_matched_case_insensitively() {
        let (_d, idx) = indexed(&[
            ("http://ex.org/a", LABEL, "urn:g", "Waalbrug"),
            ("http://ex.org/b", LABEL, "urn:g", "Tunnel"),
        ]);
        let q = "SELECT ?s WHERE {\n  ?s <http://www.w3.org/2000/01/rdf-schema#label> ?l .\n  \
                 FILTER(CONTAINS(LCASE(?l), \"waalbrug\"))\n}";
        let out = pushdown(q, &idx);
        assert!(out.contains("<http://ex.org/a>"), "got:\n{out}");
        assert!(!out.contains("<http://ex.org/b>"), "got:\n{out}");
        parse(&out);
    }

    #[test]
    fn filter_inside_optional_is_not_hoisted() {
        // Hoisting this FILTER's candidates to the top level turns an OPTIONAL
        // into an inner join: every ?s without a matching label disappears.
        let (_d, idx) = indexed(&[("http://ex.org/a", LABEL, "urn:g", "Drawbridge")]);
        let q = "SELECT ?s WHERE {\n  ?s a <http://ex.org/Thing> .\n  \
                 OPTIONAL { ?s <http://www.w3.org/2000/01/rdf-schema#label> ?l . \
                 FILTER(CONTAINS(?l, \"bridge\")) }\n}";
        assert_eq!(
            pushdown(q, &idx),
            q,
            "must not push down out of an OPTIONAL"
        );
    }

    #[test]
    fn filter_inside_not_exists_is_not_hoisted() {
        // Hoisting inverts the query: it would keep exactly the rows the
        // FILTER NOT EXISTS is there to remove.
        let (_d, idx) = indexed(&[("http://ex.org/a", LABEL, "urn:g", "Drawbridge")]);
        let q = "SELECT ?s WHERE {\n  ?s a <http://ex.org/Thing> .\n  \
                 FILTER NOT EXISTS { ?s <http://www.w3.org/2000/01/rdf-schema#label> ?l . \
                 FILTER(CONTAINS(?l, \"bridge\")) }\n}";
        assert_eq!(pushdown(q, &idx), q, "must not push down out of NOT EXISTS");
    }

    #[test]
    fn filter_inside_union_is_not_hoisted() {
        let (_d, idx) = indexed(&[("http://ex.org/a", LABEL, "urn:g", "Drawbridge")]);
        let q = "SELECT ?s WHERE {\n  \
                 { ?s <http://www.w3.org/2000/01/rdf-schema#label> ?l . \
                 FILTER(CONTAINS(?l, \"bridge\")) }\n  UNION\n  \
                 { ?s a <http://ex.org/Thing> }\n}";
        assert_eq!(
            pushdown(q, &idx),
            q,
            "must not push down out of a UNION arm"
        );
    }

    #[test]
    fn regex_is_never_pushed_down() {
        let (_d, idx) = indexed(&[("http://ex.org/a", LABEL, "urn:g", "Drawbridge")]);
        let q = "SELECT ?s WHERE {\n  ?s <http://www.w3.org/2000/01/rdf-schema#label> ?l .\n  \
                 FILTER(REGEX(?l, \"bridge\"))\n}";
        assert_eq!(
            pushdown(q, &idx),
            q,
            "REGEX dialects differ — leave it alone"
        );
    }

    #[test]
    fn pushdown_is_scoped_to_readable_graphs() {
        let (_d, idx) = indexed(&[
            ("http://ex.org/pub", LABEL, "urn:public", "Drawbridge"),
            ("http://ex.org/priv", LABEL, "urn:private", "Drawbridge"),
        ]);
        let readable: HashSet<String> = ["urn:public".to_string()].into_iter().collect();
        let q = "SELECT ?s WHERE {\n  ?s <http://www.w3.org/2000/01/rdf-schema#label> ?l .\n  \
                 FILTER(CONTAINS(?l, \"bridge\"))\n}";
        let out = preprocess_substring_pushdown(q, &idx, GraphScope::Only(&readable));
        assert!(out.contains("<http://ex.org/pub>"), "got:\n{out}");
        assert!(!out.contains("<http://ex.org/priv>"), "got:\n{out}");
    }

    #[test]
    fn an_incomplete_index_disables_pushdown() {
        use tantivy::tokenizer::MAX_TOKEN_LEN;
        let long = "y".repeat(MAX_TOKEN_LEN + 1);
        let (_d, idx) = indexed(&[
            ("http://ex.org/a", LABEL, "urn:g", "Drawbridge"),
            ("http://ex.org/b", LABEL, "urn:g", &long),
        ]);
        let q = "SELECT ?s WHERE {\n  ?s <http://www.w3.org/2000/01/rdf-schema#label> ?l .\n  \
                 FILTER(CONTAINS(?l, \"bridge\"))\n}";
        assert_eq!(pushdown(q, &idx), q);
    }

    #[test]
    fn contradictory_case_folding_is_not_pushed_down() {
        // `CONTAINS(LCASE(?l), "Bridge")` is unsatisfiable; matching it
        // case-insensitively would hand back rows the FILTER then drops —
        // harmless here, but the rewrite must not pretend to understand it.
        let (_d, idx) = indexed(&[("http://ex.org/a", LABEL, "urn:g", "Drawbridge")]);
        let q = "SELECT ?s WHERE {\n  ?s <http://www.w3.org/2000/01/rdf-schema#label> ?l .\n  \
                 FILTER(CONTAINS(LCASE(?l), \"Bridge\"))\n}";
        assert_eq!(pushdown(q, &idx), q);
    }

    #[test]
    fn strstarts_pushdown_anchors_at_the_start() {
        let (_d, idx) = indexed(&[
            ("http://ex.org/a", LABEL, "urn:g", "Waalbrug"),
            ("http://ex.org/b", LABEL, "urn:g", "de Waalbrug"),
        ]);
        let q = "SELECT ?s WHERE {\n  ?s <http://www.w3.org/2000/01/rdf-schema#label> ?l .\n  \
                 FILTER(STRSTARTS(?l, \"Waalbrug\"))\n}";
        let out = pushdown(q, &idx);
        assert!(out.contains("<http://ex.org/a>"), "got:\n{out}");
        assert!(!out.contains("<http://ex.org/b>"), "got:\n{out}");
        parse(&out);
    }
}
