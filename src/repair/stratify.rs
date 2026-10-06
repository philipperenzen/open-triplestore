//! Stratification (`docs/notes/repair-layer-design.md` §3.4): computed, not
//! declared.
//!
//! Rule `S` depends on rule `R` positively when `R`'s head produces a
//! predicate `S`'s body reads; negatively — `S` must run in a strictly higher
//! stratum — when (a) `R`'s head or retract mentions a predicate inside a
//! `NOT EXISTS` / `MINUS` of `S`, or (b) `R` retracts a predicate `S` reads
//! (consumers after the retractor), or `S` retracts a predicate `R` produces
//! (producers before the retractor). A variable predicate mentions every
//! predicate. The head guard the chase appends is not a negation here, and a
//! rule never depends on itself (a self-loop is the definition of a
//! restricted step; a replacement rule is its own guard). A cycle through a
//! negative edge is unstratifiable: the rule set is refused, naming the
//! rules. Policy deletions are left out and form the last stratum; report
//! rules emit nothing and are counted after the chase.

use super::rules::{Policy, PreparedRule};

/// Rule indexes by stratum.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Strata {
    /// The strata, lowest first.
    pub main: Vec<Vec<usize>>,
    /// Policy deletions (`closed-delete`, `maxCount-keep-lexmin`, `Rewrite`).
    pub last: Vec<usize>,
    /// `ots:Report` rules.
    pub report: Vec<usize>,
}

impl Strata {
    /// The stratum of every rule (the last stratum after the main ones).
    pub fn stratum_of(&self, rule: usize) -> Option<usize> {
        self.main
            .iter()
            .position(|s| s.contains(&rule))
            .or_else(|| self.last.contains(&rule).then_some(self.main.len()))
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Edge {
    Positive,
    Negative,
}

/// The dependency edges `from → to` between the rules in `nodes`.
fn edges(rules: &[PreparedRule], nodes: &[usize]) -> Vec<(usize, usize, Edge)> {
    let mut out = Vec::new();
    for &r in nodes {
        for &s in nodes {
            if r == s {
                continue;
            }
            let (dr, ds) = (&rules[r].deps, &rules[s].deps);
            let negative = dr.produces.intersects(&ds.negated)
                || dr.retracts.intersects(&ds.negated)
                || dr.retracts.intersects(&ds.reads)
                || dr.produces.intersects(&ds.retracts);
            if negative {
                out.push((r, s, Edge::Negative));
            } else if dr.produces.intersects(&ds.reads) {
                out.push((r, s, Edge::Positive));
            }
        }
    }
    out
}

/// Tarjan's strongly connected components, in a deterministic order.
fn sccs(n: usize, adj: &[Vec<usize>]) -> Vec<Vec<usize>> {
    struct T<'a> {
        adj: &'a [Vec<usize>],
        index: Vec<Option<usize>>,
        low: Vec<usize>,
        on: Vec<bool>,
        stack: Vec<usize>,
        next: usize,
        out: Vec<Vec<usize>>,
    }
    fn visit(t: &mut T<'_>, v: usize) {
        t.index[v] = Some(t.next);
        t.low[v] = t.next;
        t.next += 1;
        t.stack.push(v);
        t.on[v] = true;
        for &w in &t.adj[v] {
            match t.index[w] {
                None => {
                    visit(t, w);
                    t.low[v] = t.low[v].min(t.low[w]);
                }
                Some(iw) if t.on[w] => t.low[v] = t.low[v].min(iw),
                _ => {}
            }
        }
        if Some(t.low[v]) == t.index[v] {
            let mut comp = Vec::new();
            while let Some(w) = t.stack.pop() {
                t.on[w] = false;
                comp.push(w);
                if w == v {
                    break;
                }
            }
            comp.sort_unstable();
            t.out.push(comp);
        }
    }
    let mut t = T {
        adj,
        index: vec![None; n],
        low: vec![0; n],
        on: vec![false; n],
        stack: Vec::new(),
        next: 0,
        out: Vec::new(),
    };
    for v in 0..n {
        if t.index[v].is_none() {
            visit(&mut t, v);
        }
    }
    t.out
}

/// Stratify `rules`. `Err` names the rules of a cycle through a negative
/// edge.
pub fn stratify(rules: &[PreparedRule]) -> Result<Strata, String> {
    let mut strata = Strata::default();
    let mut nodes: Vec<usize> = Vec::new();
    for (i, r) in rules.iter().enumerate() {
        if r.spec.policy == Policy::Report {
            strata.report.push(i);
        } else if r.is_policy_deletion() {
            strata.last.push(i);
        } else {
            nodes.push(i);
        }
    }
    let local: std::collections::HashMap<usize, usize> =
        nodes.iter().enumerate().map(|(i, r)| (*r, i)).collect();
    let es = edges(rules, &nodes);
    let n = nodes.len();
    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (a, b, _) in &es {
        adj[local[a]].push(local[b]);
    }
    for a in &mut adj {
        a.sort_unstable();
        a.dedup();
    }
    let comps = sccs(n, &adj);
    let mut comp_of = vec![0usize; n];
    for (c, comp) in comps.iter().enumerate() {
        for &v in comp {
            comp_of[v] = c;
        }
    }
    // A negative edge inside one component is a cycle through negation.
    for (a, b, e) in &es {
        if *e == Edge::Negative && comp_of[local[a]] == comp_of[local[b]] {
            let mut names: Vec<String> = comps[comp_of[local[a]]]
                .iter()
                .map(|v| format!("<{}>", rules[nodes[*v]].spec.iri))
                .collect();
            names.sort();
            return Err(format!(
                "the rule set cannot be stratified: {} depend on each other through negation or a retract \
                 (<{}> → <{}>); a rule may not, directly or through others, both feed and negate (or \
                 retract) the same predicate",
                names.join(", "),
                rules[*a].spec.iri,
                rules[*b].spec.iri
            ));
        }
    }
    // Longest path over the condensation, a negative edge adding one. The
    // condensation is acyclic, so relaxing every edge until nothing moves
    // terminates.
    let mut level = vec![0usize; comps.len()];
    loop {
        let mut changed = false;
        for (a, b, e) in &es {
            let (ca, cb) = (comp_of[local[a]], comp_of[local[b]]);
            if ca == cb {
                continue;
            }
            let need = level[ca] + usize::from(*e == Edge::Negative);
            if level[cb] < need {
                level[cb] = need;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    let top = level.iter().copied().max().unwrap_or(0);
    let mut main: Vec<Vec<usize>> = vec![Vec::new(); if n == 0 { 0 } else { top + 1 }];
    for (v, &r) in nodes.iter().enumerate() {
        main[level[comp_of[v]]].push(r);
    }
    main.retain(|s| !s.is_empty());
    strata.main = main;
    Ok(strata)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repair::rules::{prepare, MergeMode, Origin, RuleSpec};

    fn rule(iri: &str, construct: &str) -> RuleSpec {
        RuleSpec::new(iri, construct, Origin::Authored)
    }

    fn prepared(specs: Vec<RuleSpec>) -> Vec<PreparedRule> {
        specs.into_iter().map(|s| prepare(s).unwrap()).collect()
    }

    #[test]
    fn producers_come_before_negators() {
        let rules = prepared(vec![
            rule(
                "urn:negator",
                "CONSTRUCT { ?x <urn:orphan> true } WHERE { ?x a <urn:Part> FILTER NOT EXISTS { ?x <urn:partOf> ?y } }",
            ),
            rule(
                "urn:producer",
                "CONSTRUCT { ?x <urn:partOf> ?y } WHERE { ?y <urn:hasPart> ?x }",
            ),
        ]);
        let s = stratify(&rules).unwrap();
        assert_eq!(s.main, vec![vec![1], vec![0]]);
        assert_eq!(s.stratum_of(1), Some(0));
        assert_eq!(s.stratum_of(0), Some(1));
    }

    #[test]
    fn positive_recursion_shares_a_stratum() {
        let rules = prepared(vec![rule(
            "urn:closure",
            "CONSTRUCT { ?x <urn:hasPart> ?z } WHERE { ?x <urn:hasPart> ?y . ?y <urn:hasPart> ?z }",
        )]);
        assert_eq!(stratify(&rules).unwrap().main, vec![vec![0]]);
    }

    /// The note's example: a closure rule that produces and consumes
    /// `hasPart`, with a rule retracting `hasPart`.
    #[test]
    fn a_retract_cycle_is_refused_naming_the_rules() {
        let mut retractor = rule(
            "urn:retractor",
            "CONSTRUCT { ?x <urn:mark> true } WHERE { ?x <urn:hasPart> ?y . ?y <urn:mark> true }",
        );
        retractor.retract = Some("?x <urn:hasPart> ?y".into());
        let rules = prepared(vec![
            rule(
                "urn:closure",
                "CONSTRUCT { ?x <urn:hasPart> ?z } WHERE { ?x <urn:hasPart> ?y . ?y <urn:hasPart> ?z }",
            ),
            retractor,
            rule(
                "urn:marker",
                "CONSTRUCT { ?x <urn:mark> true } WHERE { ?x <urn:hasPart> ?y }",
            ),
        ]);
        let err = stratify(&rules).unwrap_err();
        assert!(err.contains("cannot be stratified"), "{err}");
        assert!(
            err.contains("urn:closure") && err.contains("urn:retractor"),
            "{err}"
        );
    }

    #[test]
    fn negation_through_a_cycle_is_refused() {
        let rules = prepared(vec![
            rule(
                "urn:a",
                "CONSTRUCT { ?x <urn:p> true } WHERE { ?x a <urn:T> FILTER NOT EXISTS { ?x <urn:q> true } }",
            ),
            rule(
                "urn:b",
                "CONSTRUCT { ?x <urn:q> true } WHERE { ?x a <urn:T> FILTER NOT EXISTS { ?x <urn:p> true } }",
            ),
        ]);
        assert!(stratify(&rules).is_err());
    }

    /// `NOT EXISTS { ?s ?p ?o }` mentions every predicate, so it cannot
    /// escape the check.
    #[test]
    fn a_variable_predicate_in_a_negation_mentions_everything() {
        let rules = prepared(vec![
            rule(
                "urn:a",
                "CONSTRUCT { ?x <urn:p> true } WHERE { ?x a <urn:T> FILTER NOT EXISTS { ?x ?any ?y . ?y a <urn:U> } }",
            ),
            rule(
                "urn:b",
                "CONSTRUCT { ?x <urn:q> ?x } WHERE { ?x <urn:p> true }",
            ),
        ]);
        assert!(stratify(&rules).is_err());
    }

    #[test]
    fn replacement_rules_sit_between_producers_and_consumers() {
        let mut normalise = rule(
            "urn:normalise",
            "PREFIX ex: <http://example.org/> CONSTRUCT { ?x ex:length ?m ; ex:unit \"m\" } WHERE { ?x ex:length ?v ; ex:unit \"mm\" BIND(?v / 1000 AS ?m) }",
        );
        normalise.retract = Some("?x ex:length ?v ; ex:unit \"mm\"".into());
        let rules = prepared(vec![
            rule(
                "urn:consumer",
                "PREFIX ex: <http://example.org/> CONSTRUCT { ?x ex:long true } WHERE { ?x ex:length ?l ; ex:unit \"m\" FILTER(?l > 10) }",
            ),
            normalise,
            rule(
                "urn:producer",
                "PREFIX ex: <http://example.org/> CONSTRUCT { ?x ex:unit \"mm\" } WHERE { ?x a ex:MetricThing }",
            ),
        ]);
        let s = stratify(&rules).unwrap();
        let at = |r| s.stratum_of(r).unwrap();
        assert!(at(2) < at(1), "producer before the retractor: {s:?}");
        assert!(at(1) < at(0), "consumer after the retractor: {s:?}");
    }

    #[test]
    fn policy_deletions_and_report_rules_are_set_apart() {
        let mut merge = rule(
            "urn:merge",
            "CONSTRUCT { } WHERE { ?x <urn:key> ?k . ?y <urn:key> ?k FILTER(?x != ?y) }",
        );
        merge.equate = Some(("x".into(), "y".into()));
        merge.merge_mode = MergeMode::Rewrite;
        let mut report = rule(
            "urn:report",
            "CONSTRUCT { ?x <urn:p> true } WHERE { ?x a <urn:T> }",
        );
        report.policy = Policy::Report;
        let rules = prepared(vec![
            merge,
            report,
            rule(
                "urn:plain",
                "CONSTRUCT { ?x <urn:q> true } WHERE { ?x a <urn:T> }",
            ),
        ]);
        let s = stratify(&rules).unwrap();
        assert_eq!(s.last, vec![0]);
        assert_eq!(s.report, vec![1]);
        assert_eq!(s.main, vec![vec![2]]);
        assert_eq!(s.stratum_of(0), Some(1));
    }
}
