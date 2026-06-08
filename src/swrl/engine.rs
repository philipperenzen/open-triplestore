//! SWRL rule evaluation engine.
//!
//! Translates SWRL rules to SPARQL INSERT WHERE queries and executes them
//! in a fixed-point loop until no new triples are inferred.

use oxigraph::sparql::Update;
use serde::Serialize;
use tracing::{debug, info, warn};

use crate::store::TripleStore;

/// A SWRL rule with antecedent (body) and consequent (head).
#[derive(Debug, Clone)]
pub struct SwrlRule {
    pub name: Option<String>,
    pub body: Vec<Atom>,
    pub head: Vec<Atom>,
}

/// An atom in a SWRL rule.
#[derive(Debug, Clone)]
#[allow(clippy::enum_variant_names)] // variant names mirror the W3C SWRL atom types
pub enum Atom {
    /// Class membership: Class(?x) → ?x rdf:type Class
    ClassAtom { class_iri: String, arg: SwrlArg },
    /// Object property: prop(?x, ?y) → ?x prop ?y
    ObjectPropertyAtom {
        property: String,
        arg1: SwrlArg,
        arg2: SwrlArg,
    },
    /// Data property: prop(?x, ?y) → ?x prop ?y (where ?y is a literal)
    DataPropertyAtom {
        property: String,
        arg1: SwrlArg,
        arg2: SwrlArg,
    },
    /// owl:sameAs assertion
    SameIndividualAtom { arg1: SwrlArg, arg2: SwrlArg },
    /// owl:differentFrom assertion
    DifferentIndividualsAtom { arg1: SwrlArg, arg2: SwrlArg },
    /// Built-in predicate (math, string, comparison)
    BuiltinAtom { builtin: String, args: Vec<SwrlArg> },
}

/// An argument in a SWRL atom.
#[derive(Debug, Clone, Serialize)]
pub enum SwrlArg {
    /// A SWRL variable (e.g., ?x or urn:swrl:var#x)
    Variable(String),
    /// A named individual IRI
    Individual(String),
    /// A literal value with optional datatype
    Literal {
        value: String,
        datatype: Option<String>,
    },
}

impl SwrlArg {
    /// Convert to a SPARQL expression.
    fn to_sparql(&self) -> String {
        match self {
            SwrlArg::Variable(v) => {
                // Normalize variable names: urn:swrl:var#x → ?x
                let name = v
                    .strip_prefix("urn:swrl:var#")
                    .or_else(|| v.strip_prefix("?"))
                    .unwrap_or(v);
                format!("?{}", name)
            }
            SwrlArg::Individual(iri) => {
                format!("<{}>", iri.trim_start_matches('<').trim_end_matches('>'))
            }
            SwrlArg::Literal {
                value,
                datatype: Some(dt),
            } => format!("\"{}\"^^<{}>", value, dt),
            SwrlArg::Literal {
                value,
                datatype: None,
            } => format!("\"{}\"", value),
        }
    }
}

/// Result of SWRL rule execution.
#[derive(Debug, Clone, Serialize)]
pub struct SwrlExecutionResult {
    /// Number of rules processed.
    pub rules_count: usize,
    /// Total iterations of the fixed-point loop.
    pub iterations: usize,
    /// Total new triples inferred.
    pub triples_inferred: usize,
    /// Per-rule execution details.
    pub rule_results: Vec<RuleResult>,
}

/// Result for a single rule execution.
#[derive(Debug, Clone, Serialize)]
pub struct RuleResult {
    pub rule_name: String,
    pub sparql: String,
    pub success: bool,
    pub error: Option<String>,
}

/// Execute SWRL rules against the store in a fixed-point loop.
///
/// Continues iterating until no new triples are inferred or `max_iterations`
/// is reached. Returns execution statistics.
pub fn execute_rules(
    store: &TripleStore,
    rules: &[SwrlRule],
    max_iterations: usize,
    target_graph: Option<&str>,
) -> Result<SwrlExecutionResult, String> {
    info!(
        "Executing {} SWRL rules (max {} iterations)",
        rules.len(),
        max_iterations
    );

    let mut total_inferred = 0;
    let mut rule_results = Vec::new();
    let mut iteration = 0;

    // Translate rules to SPARQL once
    let sparql_rules: Vec<(String, String)> = rules
        .iter()
        .enumerate()
        .filter_map(|(i, rule)| {
            let name = rule
                .name
                .clone()
                .unwrap_or_else(|| format!("rule_{}", i + 1));
            match rule_to_sparql(rule, target_graph) {
                Ok(sparql) => Some((name, sparql)),
                Err(e) => {
                    warn!("Failed to translate rule {}: {}", name, e);
                    rule_results.push(RuleResult {
                        rule_name: name,
                        sparql: String::new(),
                        success: false,
                        error: Some(e),
                    });
                    None
                }
            }
        })
        .collect();

    // Fixed-point loop
    loop {
        iteration += 1;
        if iteration > max_iterations {
            info!("Reached max iterations ({})", max_iterations);
            break;
        }

        let count_before = count_triples(store);
        debug!("Iteration {}: {} triples before", iteration, count_before);

        for (name, sparql) in &sparql_rules {
            match Update::parse(sparql, None) {
                Ok(update) => {
                    let opts = store.query_options();
                    match store.store().update_opt(update, opts) {
                        Ok(()) => {
                            if iteration == 1 {
                                rule_results.push(RuleResult {
                                    rule_name: name.clone(),
                                    sparql: sparql.clone(),
                                    success: true,
                                    error: None,
                                });
                            }
                        }
                        Err(e) => {
                            warn!("Rule {} failed: {}", name, e);
                            if iteration == 1 {
                                rule_results.push(RuleResult {
                                    rule_name: name.clone(),
                                    sparql: sparql.clone(),
                                    success: false,
                                    error: Some(e.to_string()),
                                });
                            }
                        }
                    }
                }
                Err(e) => {
                    if iteration == 1 {
                        rule_results.push(RuleResult {
                            rule_name: name.clone(),
                            sparql: sparql.clone(),
                            success: false,
                            error: Some(format!("SPARQL parse error: {}", e)),
                        });
                    }
                }
            }
        }

        let count_after = count_triples(store);
        let new_triples = count_after.saturating_sub(count_before);
        total_inferred += new_triples;

        debug!(
            "Iteration {}: {} new triples (total: {})",
            iteration, new_triples, total_inferred
        );

        if new_triples == 0 {
            info!(
                "Fixed point reached after {} iterations ({} triples inferred)",
                iteration, total_inferred
            );
            break;
        }
    }

    // Rebuild graph index after rule execution
    store.rebuild_graph_index();

    Ok(SwrlExecutionResult {
        rules_count: rules.len(),
        iterations: iteration,
        triples_inferred: total_inferred,
        rule_results,
    })
}

/// Translate a SWRL rule to a SPARQL INSERT WHERE query.
fn rule_to_sparql(rule: &SwrlRule, target_graph: Option<&str>) -> Result<String, String> {
    if rule.head.is_empty() {
        return Err("Rule has no head atoms".to_string());
    }

    let mut where_patterns = Vec::new();
    let mut filters = Vec::new();

    // Translate body atoms to WHERE patterns
    for atom in &rule.body {
        match atom {
            Atom::ClassAtom { class_iri, arg } => {
                where_patterns.push(format!(
                    "  {} a <{}> .",
                    arg.to_sparql(),
                    class_iri.trim_start_matches('<').trim_end_matches('>')
                ));
            }
            Atom::ObjectPropertyAtom {
                property,
                arg1,
                arg2,
            }
            | Atom::DataPropertyAtom {
                property,
                arg1,
                arg2,
            } => {
                where_patterns.push(format!(
                    "  {} <{}> {} .",
                    arg1.to_sparql(),
                    property.trim_start_matches('<').trim_end_matches('>'),
                    arg2.to_sparql()
                ));
            }
            Atom::SameIndividualAtom { arg1, arg2 } => {
                where_patterns.push(format!(
                    "  {} <http://www.w3.org/2002/07/owl#sameAs> {} .",
                    arg1.to_sparql(),
                    arg2.to_sparql()
                ));
            }
            Atom::DifferentIndividualsAtom { arg1, arg2 } => {
                where_patterns.push(format!(
                    "  {} <http://www.w3.org/2002/07/owl#differentFrom> {} .",
                    arg1.to_sparql(),
                    arg2.to_sparql()
                ));
            }
            Atom::BuiltinAtom { builtin, args } => {
                if let Some(filter) = builtin_to_filter(builtin, args) {
                    filters.push(filter);
                }
            }
        }
    }

    // Translate head atoms to INSERT patterns
    let mut insert_patterns = Vec::new();
    for atom in &rule.head {
        match atom {
            Atom::ClassAtom { class_iri, arg } => {
                insert_patterns.push(format!(
                    "  {} a <{}> .",
                    arg.to_sparql(),
                    class_iri.trim_start_matches('<').trim_end_matches('>')
                ));
            }
            Atom::ObjectPropertyAtom {
                property,
                arg1,
                arg2,
            }
            | Atom::DataPropertyAtom {
                property,
                arg1,
                arg2,
            } => {
                insert_patterns.push(format!(
                    "  {} <{}> {} .",
                    arg1.to_sparql(),
                    property.trim_start_matches('<').trim_end_matches('>'),
                    arg2.to_sparql()
                ));
            }
            Atom::SameIndividualAtom { arg1, arg2 } => {
                insert_patterns.push(format!(
                    "  {} <http://www.w3.org/2002/07/owl#sameAs> {} .",
                    arg1.to_sparql(),
                    arg2.to_sparql()
                ));
            }
            Atom::DifferentIndividualsAtom { arg1, arg2 } => {
                insert_patterns.push(format!(
                    "  {} <http://www.w3.org/2002/07/owl#differentFrom> {} .",
                    arg1.to_sparql(),
                    arg2.to_sparql()
                ));
            }
            Atom::BuiltinAtom { .. } => {
                // Built-in atoms in head are not standard; skip
                warn!("BuiltinAtom in rule head is not supported");
            }
        }
    }

    let graph_clause = if let Some(g) = target_graph {
        format!("GRAPH <{}> {{\n{}\n  }}", g, insert_patterns.join("\n"))
    } else {
        insert_patterns.join("\n")
    };

    let mut where_clause = where_patterns.join("\n");
    if !filters.is_empty() {
        where_clause.push('\n');
        for f in &filters {
            where_clause.push_str(&format!("  FILTER({}) .\n", f));
        }
    }

    Ok(format!(
        "INSERT {{\n{}\n}} WHERE {{\n{}\n}}",
        graph_clause, where_clause
    ))
}

/// Translate a SWRL built-in predicate to a SPARQL FILTER expression.
fn builtin_to_filter(builtin: &str, args: &[SwrlArg]) -> Option<String> {
    let builtin_local = builtin
        .rsplit_once('#')
        .map(|(_, local)| local)
        .unwrap_or(builtin);

    let sparql_args: Vec<String> = args.iter().map(|a| a.to_sparql()).collect();

    match builtin_local {
        "equal" if sparql_args.len() == 2 => {
            Some(format!("{} = {}", sparql_args[0], sparql_args[1]))
        }
        "notEqual" if sparql_args.len() == 2 => {
            Some(format!("{} != {}", sparql_args[0], sparql_args[1]))
        }
        "lessThan" if sparql_args.len() == 2 => {
            Some(format!("{} < {}", sparql_args[0], sparql_args[1]))
        }
        "lessThanOrEqual" if sparql_args.len() == 2 => {
            Some(format!("{} <= {}", sparql_args[0], sparql_args[1]))
        }
        "greaterThan" if sparql_args.len() == 2 => {
            Some(format!("{} > {}", sparql_args[0], sparql_args[1]))
        }
        "greaterThanOrEqual" if sparql_args.len() == 2 => {
            Some(format!("{} >= {}", sparql_args[0], sparql_args[1]))
        }
        "add" if sparql_args.len() == 3 => Some(format!(
            "{} = {} + {}",
            sparql_args[0], sparql_args[1], sparql_args[2]
        )),
        "subtract" if sparql_args.len() == 3 => Some(format!(
            "{} = {} - {}",
            sparql_args[0], sparql_args[1], sparql_args[2]
        )),
        "multiply" if sparql_args.len() == 3 => Some(format!(
            "{} = {} * {}",
            sparql_args[0], sparql_args[1], sparql_args[2]
        )),
        "divide" if sparql_args.len() == 3 => Some(format!(
            "{} = {} / {}",
            sparql_args[0], sparql_args[1], sparql_args[2]
        )),
        "stringConcat" if sparql_args.len() >= 2 => {
            Some(format!("CONCAT({})", sparql_args.join(", ")))
        }
        "contains" if sparql_args.len() == 2 => {
            Some(format!("CONTAINS({}, {})", sparql_args[0], sparql_args[1]))
        }
        "matches" if sparql_args.len() >= 2 => Some(format!(
            "REGEX({})",
            sparql_args[..std::cmp::min(sparql_args.len(), 3)].join(", ")
        )),
        _ => {
            debug!("Unsupported SWRL builtin: {}", builtin);
            None
        }
    }
}

/// Count total triples in the store (default graph).
fn count_triples(store: &TripleStore) -> usize {
    store
        .store()
        .quads_for_pattern(None, None, None, None)
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rule_to_sparql() {
        let rule = SwrlRule {
            name: Some("test".to_string()),
            body: vec![
                Atom::ClassAtom {
                    class_iri: "http://example.org/Person".to_string(),
                    arg: SwrlArg::Variable("?x".to_string()),
                },
                Atom::ObjectPropertyAtom {
                    property: "http://example.org/knows".to_string(),
                    arg1: SwrlArg::Variable("?x".to_string()),
                    arg2: SwrlArg::Variable("?y".to_string()),
                },
            ],
            head: vec![Atom::ClassAtom {
                class_iri: "http://example.org/Person".to_string(),
                arg: SwrlArg::Variable("?y".to_string()),
            }],
        };

        let sparql = rule_to_sparql(&rule, None).unwrap();
        assert!(sparql.contains("INSERT"));
        assert!(sparql.contains("WHERE"));
        assert!(sparql.contains("http://example.org/Person"));
        assert!(sparql.contains("http://example.org/knows"));
    }

    #[test]
    fn test_builtin_to_filter() {
        let args = vec![
            SwrlArg::Variable("?x".to_string()),
            SwrlArg::Variable("?y".to_string()),
        ];
        assert_eq!(
            builtin_to_filter("http://www.w3.org/2003/11/swrlb#greaterThan", &args),
            Some("?x > ?y".to_string())
        );
    }

    #[test]
    fn test_swrl_arg_to_sparql() {
        assert_eq!(SwrlArg::Variable("?x".to_string()).to_sparql(), "?x");
        assert_eq!(
            SwrlArg::Variable("urn:swrl:var#foo".to_string()).to_sparql(),
            "?foo"
        );
        assert_eq!(
            SwrlArg::Individual("http://example.org/Alice".to_string()).to_sparql(),
            "<http://example.org/Alice>"
        );
    }
}
