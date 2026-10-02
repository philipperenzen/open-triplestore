//! The chase against small sandboxes: what it proposes, what it refuses, and
//! that it is deterministic, minimal and idempotent.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use oxigraph::io::RdfFormat;
use oxigraph::model::{GraphName, Quad, Term};

use super::chase::{chase, Budget, ChaseInput, ChaseOutcome};
use super::proposal::{self, Base, ReportInput};
use super::rules::{prepare, GraphScope, MergeMode, Origin, PreparedRule, RuleSpec};
use super::sandbox::empty_sandbox;
use super::stratify::{stratify, Strata};
use crate::store::TripleStore;

const G1: &str = "http://example.org/g1";
const G2: &str = "http://example.org/g2";
const MODEL: &str = "http://example.org/model";
const BASE: &str = "http://localhost";
const EX: &str = "PREFIX ex: <http://example.org/> PREFIX owl: <http://www.w3.org/2002/07/owl#> ";

fn store(trig: &str) -> TripleStore {
    let s = empty_sandbox().unwrap();
    s.load_str(
        &format!("@prefix ex: <http://example.org/> .\n@prefix owl: <http://www.w3.org/2002/07/owl#> .\n@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .\n{trig}"),
        RdfFormat::TriG,
        None,
    )
    .unwrap();
    s
}

fn rule(iri: &str, construct: &str) -> RuleSpec {
    RuleSpec::new(iri, format!("{EX}{construct}"), Origin::Authored)
}

struct Run {
    rules: Vec<PreparedRule>,
    strata: Strata,
    out: ChaseOutcome,
}

fn run_with(
    s: &TripleStore,
    specs: Vec<RuleSpec>,
    semi_naive: bool,
    budget: Option<Budget>,
) -> Run {
    let rules: Vec<PreparedRule> = specs.into_iter().map(|r| prepare(r).unwrap()).collect();
    let strata = stratify(&rules).unwrap();
    let premises = vec![G1.to_string(), G2.to_string(), MODEL.to_string()];
    let targets: BTreeSet<String> = [G1.to_string(), G2.to_string()].into_iter().collect();
    let prefixes = super::run::null_prefixes(BASE);
    let input = ChaseInput {
        sandbox: s,
        rules: &rules,
        strata: &strata,
        premises: &premises,
        targets: &targets,
        null_base: BASE,
        null_prefixes: &prefixes,
        focus: None,
        budget: budget.unwrap_or(Budget {
            rounds: 100,
            nulls: 10_000,
            ops: 50_000,
            deadline: Instant::now() + Duration::from_secs(60),
        }),
        same_as_seeds: &[],
        semi_naive,
    };
    let out = chase(&input).unwrap();
    Run { rules, strata, out }
}

fn run(s: &TripleStore, specs: Vec<RuleSpec>) -> Run {
    run_with(s, specs, true, None)
}

fn added(r: &Run) -> Vec<String> {
    let mut v: Vec<String> = r.out.added.iter().map(|(q, _)| q.to_string()).collect();
    v.sort();
    v
}

fn deleted(r: &Run) -> Vec<String> {
    let mut v: Vec<String> = r.out.deleted.iter().map(|(q, _)| q.to_string()).collect();
    v.sort();
    v
}

fn patch(r: &Run) -> String {
    proposal::build(ReportInput {
        dataset_id: "ds",
        base_url: BASE,
        actor_iri: None,
        base: Base {
            graphs: Vec::new(),
            commit: None,
            sequence: None,
            epoch: None,
            write_generation: 0,
            consistent: true,
            entailment: "off",
            source: "live",
            quads: 0,
        },
        rules: &r.rules,
        strata: &r.strata,
        outcome: &r.out,
        report_only: Vec::new(),
        skipped: Vec::new(),
        rule_graphs: &[],
        validation: None,
        elapsed_ms: 0,
        partial: false,
        withheld_graphs: 0,
        withheld_shapes: 0,
        started_at: "2026-10-01T00:00:00Z".into(),
        dropped: Vec::new(),
    })
    .unwrap()
    .patch
}

/// Apply a proposal's patch to `s` (as the patch route would).
fn apply(s: &TripleStore, patch: &str) {
    let p = crate::rdf_patch::parse(patch).unwrap();
    if !p.ops.is_empty() {
        s.update(&crate::rdf_patch::to_sparql_update(&p)).unwrap();
    }
}

fn closure() -> RuleSpec {
    let mut r = rule(
        "urn:rule:closure",
        "CONSTRUCT { ?x ex:hasPart ?z } WHERE { ?x ex:hasPart ?y . ?y ex:hasPart ?z . FILTER(?x != ?z) }",
    );
    r.graph_scope = GraphScope::Union;
    r
}

const CHAIN: &str = "GRAPH <http://example.org/g1> { ex:a ex:hasPart ex:b . ex:b ex:hasPart ex:c . ex:c ex:hasPart ex:d . }";

/// §3.4 worked example 3: a ground recursive rule reaches its closure, every
/// addition placed in the graph of its first premise, and a second run over
/// the applied proposal proposes nothing.
#[test]
fn a_ground_closure_converges_and_is_idempotent() {
    let s = store(CHAIN);
    let r = run(&s, vec![closure()]);
    assert_eq!(
        added(&r),
        [
            "<http://example.org/a> <http://example.org/hasPart> <http://example.org/c> <http://example.org/g1>",
            "<http://example.org/a> <http://example.org/hasPart> <http://example.org/d> <http://example.org/g1>",
            "<http://example.org/b> <http://example.org/hasPart> <http://example.org/d> <http://example.org/g1>",
        ]
    );
    assert!(r.out.exhausted.is_none());
    let fresh = store(CHAIN);
    apply(&fresh, &patch(&r));
    let again = run(&fresh, vec![closure()]);
    assert!(
        again.out.added.is_empty() && again.out.deleted.is_empty(),
        "{:?}",
        added(&again)
    );
}

/// Semi-naive evaluation finds exactly what naive evaluation finds, reading
/// fewer rows.
#[test]
fn semi_naive_agrees_with_naive() {
    let long: String = (0..12)
        .map(|i| format!("ex:n{i} ex:hasPart ex:n{} .", i + 1))
        .collect();
    let data = format!("GRAPH <{G1}> {{ {long} }}");
    let naive = run_with(&store(&data), vec![closure()], false, None);
    let semi = run_with(&store(&data), vec![closure()], true, None);
    assert_eq!(added(&naive), added(&semi));
    assert_eq!(added(&naive).len(), 12 * 13 / 2 - 12);
    assert_eq!(
        patch(&naive),
        patch(&semi),
        "the same proposal, byte for byte"
    );
    // Rounds after the first ran one delta-bound query per atom.
    assert!(
        semi.out.evaluations > naive.out.evaluations,
        "semi-naive ran {} evaluations, naive {}",
        semi.out.evaluations,
        naive.out.evaluations
    );
}

fn deck_rule() -> RuleSpec {
    let mut r = rule(
        "urn:rule:deck",
        "CONSTRUCT { ?this ex:hasDeck ?w . ?w a ex:Deck } WHERE { GRAPH ?g { ?this a ex:Bridge } }",
    );
    r.nulls = vec!["w".into()];
    r.graph_scope = GraphScope::PerGraph;
    r.message = Some("{?this} needs a deck".into());
    r
}

/// An existential head mints one content-derived witness per focus node,
/// in the focus node's graph; a node that already has a deck gets none.
#[test]
fn an_existential_rule_mints_one_witness_per_focus_node() {
    let data = format!(
        "GRAPH <{G1}> {{ ex:b1 a ex:Bridge . ex:b2 a ex:Bridge ; ex:hasDeck ex:d2 . ex:d2 a ex:Deck . }} \
         GRAPH <{G2}> {{ ex:b3 a ex:Bridge . }}"
    );
    let r = run(&store(&data), vec![deck_rule()]);
    let a = added(&r);
    assert_eq!(a.len(), 4, "two triples for each of b1 and b3: {a:#?}");
    assert!(a.iter().any(|q| q.starts_with(
        "<http://example.org/b1> <http://example.org/hasDeck> <http://localhost/.well-known/genid/"
    ) && q.ends_with(&format!("<{G1}>"))));
    assert!(a.iter().any(|q| q.starts_with(
        "<http://example.org/b3> <http://example.org/hasDeck> <http://localhost/.well-known/genid/"
    ) && q.ends_with(&format!("<{G2}>"))));
    assert!(!a.iter().any(|q| q.starts_with("<http://example.org/b2>")));
    assert_eq!(r.out.nulls, 2);
    // Explained.
    let (_, d) = &r.out.added[0];
    assert!(d.explanation.as_deref().unwrap().ends_with("needs a deck"));
    assert!(!d.premises.is_empty());
    // The same state and rules mint the same IRIs: byte-identical patches.
    let again = run(&store(&data), vec![deck_rule()]);
    assert_eq!(patch(&r), patch(&again));
    // Applied, nothing is left to do.
    let s = store(&data);
    apply(&s, &patch(&r));
    let after = run(&s, vec![deck_rule()]);
    assert!(after.out.added.is_empty());
}

fn key_rule(mode: MergeMode) -> RuleSpec {
    let mut r = rule(
        "urn:rule:key",
        "CONSTRUCT { } WHERE { ?x a ex:Asset ; ex:code ?k . ?y a ex:Asset ; ex:code ?k . FILTER(?x != ?y) FILTER(isIRI(?x)) FILTER(isIRI(?y)) }",
    );
    r.equate = Some(("x".into(), "y".into()));
    r.merge_mode = mode;
    r
}

/// §3.2: a key-based EGD proposes one `owl:sameAs` line, the loser being the
/// greater N-Triples form, in the loser's graph.
#[test]
fn a_key_egd_proposes_one_same_as_line() {
    let data = format!(
        "GRAPH <{G1}> {{ ex:z1 a ex:Asset ; ex:code \"A-1\" . }} GRAPH <{G2}> {{ ex:a1 a ex:Asset ; ex:code \"A-1\" . ex:q a ex:Asset ; ex:code \"B-2\" . }}"
    );
    let r = run(&store(&data), vec![key_rule(MergeMode::SameAsOnly)]);
    assert_eq!(
        added(&r),
        [format!("<http://example.org/z1> <http://www.w3.org/2002/07/owl#sameAs> <http://example.org/a1> <{G1}>")]
    );
    assert_eq!(r.out.merges.len(), 1);
    assert_eq!(r.out.merges[0].reason, "lexmin");
}

/// §4.4 step 1: asserted distinctness and literals are conflicts, never
/// merges.
#[test]
fn distinct_or_literal_terms_are_conflicts() {
    let data = format!(
        "GRAPH <{G1}> {{ ex:x1 a ex:Asset ; ex:code \"K\" . ex:x2 a ex:Asset ; ex:code \"K\" . ex:x1 owl:differentFrom ex:x2 . \
         ex:c1 a ex:Asset ; ex:code \"L\" . ex:c2 a ex:Asset ; ex:code \"L\" . ex:c1 a ex:Steel . ex:c2 a ex:Wood . }} \
         GRAPH <{MODEL}> {{ ex:Steel owl:disjointWith ex:Wood . }}"
    );
    let r = run(&store(&data), vec![key_rule(MergeMode::SameAsOnly)]);
    assert!(r.out.added.is_empty(), "{:?}", added(&r));
    let kinds: Vec<&str> = r.out.conflicts.iter().map(|c| c.kind).collect();
    assert_eq!(kinds, ["distinct", "distinct"]);
    // A functional property with two literal values.
    let mut fp = rule(
        "urn:rule:fp",
        "CONSTRUCT { } WHERE { ?s ex:serial ?a . ?s ex:serial ?b . FILTER(?a != ?b) }",
    );
    fp.equate = Some(("a".into(), "b".into()));
    let s = store(&format!(
        "GRAPH <{G1}> {{ ex:p ex:serial \"1\" , \"2\" . }}"
    ));
    let r = run(&s, vec![fp]);
    assert_eq!(r.out.conflicts.first().map(|c| c.kind), Some("literal"));
}

/// A null merged into a live IRI is substituted everywhere: the witness's
/// triples are proposed for the live IRI instead (§4.4 step 3).
#[test]
fn a_null_merged_into_a_live_term_is_substituted() {
    // Every bridge needs an owner; ex:owner is functional, and b1's owner is
    // known through another path, so the minted owner is merged into it.
    let mut needs_owner = rule(
        "urn:rule:owner",
        "CONSTRUCT { ?this ex:owner ?w . ?w a ex:Org } WHERE { GRAPH ?g { ?this a ex:Bridge } }",
    );
    needs_owner.nulls = vec!["w".into()];
    needs_owner.graph_scope = GraphScope::PerGraph;
    needs_owner.guard = Some("{ ?this ex:owner ?o }".into());
    let mut managed = rule(
        "urn:rule:managed",
        "CONSTRUCT { ?b ex:owner ?org } WHERE { ?b ex:managedBy ?org }",
    );
    managed.priority = 1.0;
    let mut functional = rule(
        "urn:rule:functional-owner",
        "CONSTRUCT { } WHERE { ?s ex:owner ?a . ?s ex:owner ?b . FILTER(?a != ?b) }",
    );
    functional.equate = Some(("a".into(), "b".into()));
    // `managed` runs after `needs_owner` within the round (priority), so both
    // fire in round 1 and the EGD merges the witness in round 2.
    let data = format!("GRAPH <{G1}> {{ ex:b1 a ex:Bridge ; ex:managedBy ex:rws . }}");
    let r = run(&store(&data), vec![needs_owner, managed, functional]);
    let a = added(&r);
    assert!(
        a.contains(&format!(
            "<http://example.org/b1> <http://example.org/owner> <http://example.org/rws> <{G1}>"
        )),
        "{a:#?}"
    );
    assert!(
        a.contains(&format!("<http://example.org/rws> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://example.org/Org> <{G1}>")),
        "the witness's type moved to the live IRI: {a:#?}"
    );
    assert!(
        !a.iter().any(|q| q.contains("/.well-known/genid/")),
        "no null left: {a:#?}"
    );
    assert!(r.out.merges.iter().any(|m| m.reason == "null"));
}

/// §3.4 unit normalisation: a replacement rule adds its head and deletes its
/// retract in one pass, and is idempotent.
#[test]
fn a_replacement_rule_rewrites_values() {
    let mut normalise = rule(
        "urn:rule:mm",
        "CONSTRUCT { ?x ex:length ?m ; ex:unit \"m\" } WHERE { ?x ex:length ?v ; ex:unit \"mm\" . BIND(?v / 1000 AS ?m) }",
    );
    normalise.retract = Some("?x ex:length ?v ; ex:unit \"mm\"".into());
    normalise.destructive = true;
    let data = format!("GRAPH <{G1}> {{ ex:beam ex:length 2500 ; ex:unit \"mm\" . }}");
    let r = run(&store(&data), vec![normalise.clone()]);
    assert_eq!(
        deleted(&r),
        [
            format!("<http://example.org/beam> <http://example.org/length> \"2500\"^^<http://www.w3.org/2001/XMLSchema#integer> <{G1}>"),
            format!("<http://example.org/beam> <http://example.org/unit> \"mm\" <{G1}>"),
        ]
    );
    assert_eq!(
        added(&r),
        [
            format!("<http://example.org/beam> <http://example.org/length> \"2.5\"^^<http://www.w3.org/2001/XMLSchema#decimal> <{G1}>"),
            format!("<http://example.org/beam> <http://example.org/unit> \"m\" <{G1}>"),
        ]
    );
    let s = store(&data);
    apply(&s, &patch(&r));
    assert!(run(&s, vec![normalise]).out.added.is_empty());
}

/// The test plan's "a replacement rule whose output feeds a key EGD": the
/// normalised value makes two assets share a key.
#[test]
fn a_replacement_feeds_a_key_egd() {
    let mut normalise = rule(
        "urn:rule:upper",
        "CONSTRUCT { ?x ex:code ?u } WHERE { ?x ex:code ?c . BIND(UCASE(?c) AS ?u) FILTER(?u != ?c) }",
    );
    normalise.retract = Some("?x ex:code ?c".into());
    let data = format!(
        "GRAPH <{G1}> {{ ex:p1 a ex:Asset ; ex:code \"ab-1\" . ex:p2 a ex:Asset ; ex:code \"AB-1\" . }}"
    );
    let r = run(
        &store(&data),
        vec![normalise, key_rule(MergeMode::SameAsOnly)],
    );
    assert!(
        added(&r).contains(&format!("<http://example.org/p2> <http://www.w3.org/2002/07/owl#sameAs> <http://example.org/p1> <{G1}>")),
        "{:#?}",
        added(&r)
    );
    assert_eq!(
        r.strata.main.len(),
        2,
        "the retractor runs before the EGD that reads ex:code"
    );
}

/// Rewrite merges run in the last stratum and replace the loser's triples.
#[test]
fn a_rewrite_merge_replaces_the_losers_triples() {
    let data = format!(
        "GRAPH <{G1}> {{ ex:z a ex:Asset ; ex:code \"K\" ; ex:label \"zed\" . ex:a a ex:Asset ; ex:code \"K\" . ex:w ex:near ex:z . }}"
    );
    let r = run(&store(&data), vec![key_rule(MergeMode::Rewrite)]);
    let d = deleted(&r);
    assert!(
        d.contains(&format!(
            "<http://example.org/z> <http://example.org/label> \"zed\" <{G1}>"
        )),
        "{d:#?}"
    );
    assert!(
        d.contains(&format!(
            "<http://example.org/w> <http://example.org/near> <http://example.org/z> <{G1}>"
        )),
        "{d:#?}"
    );
    let a = added(&r);
    assert!(
        a.contains(&format!(
            "<http://example.org/a> <http://example.org/label> \"zed\" <{G1}>"
        )),
        "{a:#?}"
    );
    assert!(
        a.contains(&format!(
            "<http://example.org/w> <http://example.org/near> <http://example.org/a> <{G1}>"
        )),
        "{a:#?}"
    );
    // Shared triples (type, code) need no add: the winner has them.
    assert!(!a.iter().any(|q| q.contains("/code>")));
}

/// A head that would land outside the writable graphs is unplaceable, and a
/// trigger that would write a blank node is unexpressible.
#[test]
fn unplaceable_and_unexpressible_triggers_are_reported() {
    let mut into_model = rule(
        "urn:rule:model",
        "CONSTRUCT { ?x ex:tagged true } WHERE { ?x a ex:Bridge }",
    );
    into_model.target_graph = Some(MODEL.into());
    let data = format!("GRAPH <{G1}> {{ ex:b a ex:Bridge . _:anon a ex:Bridge . }}");
    let r = run(&store(&data), vec![into_model]);
    assert!(r.out.added.is_empty());
    assert_eq!(
        r.out.unplaceable.len(),
        1,
        "the IRI's triple would land in a model graph"
    );
    assert_eq!(
        r.out.unexpressible.len(),
        1,
        "the blank node cannot be written at all"
    );
    let bnode = rule(
        "urn:rule:bnode",
        "CONSTRUCT { ?x ex:tagged true } WHERE { ?x a ex:Bridge }",
    );
    let r = run(&store(&data), vec![bnode]);
    assert_eq!(r.out.added.len(), 1, "the IRI subject is tagged");
    assert_eq!(r.out.unexpressible.len(), 1, "the blank node is not");
}

/// A retract that would delete what another rule derived is a conflict.
#[test]
fn a_retract_of_a_derived_quad_is_a_conflict() {
    let adder = rule(
        "urn:rule:add",
        "CONSTRUCT { ?x ex:flag true } WHERE { ?x a ex:T }",
    );
    let mut remover = rule(
        "urn:rule:remove",
        "CONSTRUCT { ?x ex:seen true } WHERE { ?x ex:flag true }",
    );
    remover.retract = Some("?x ex:flag true".into());
    let data = format!("GRAPH <{G1}> {{ ex:t a ex:T . }}");
    let r = run(&store(&data), vec![adder, remover]);
    assert!(
        r.out.conflicts.iter().any(|c| c.kind == "retract-derived"),
        "{:?}",
        r.out.conflicts
    );
}

/// A non-terminating existential rule exhausts its budget and returns what
/// it proposed so far, marked incomplete.
#[test]
fn an_unbounded_rule_exhausts_its_budget() {
    let mut chain = rule(
        "urn:rule:chain",
        "CONSTRUCT { ?x ex:next ?w . ?w a ex:Node } WHERE { ?x a ex:Node }",
    );
    chain.nulls = vec!["w".into()];
    let data = format!("GRAPH <{G1}> {{ ex:n0 a ex:Node . }}");
    let r = run_with(
        &store(&data),
        vec![chain],
        true,
        Some(Budget {
            rounds: 1000,
            nulls: 25,
            ops: 50_000,
            deadline: Instant::now() + Duration::from_secs(60),
        }),
    );
    assert_eq!(r.out.exhausted, Some("nulls"));
    assert!(!r.out.added.is_empty());
    let p = patch(&r);
    assert!(p.contains("H complete \"false\" ."), "{p}");
    let r = run_with(
        &store(&data),
        vec![rule(
            "urn:rule:noop",
            "CONSTRUCT { ?x ex:y true } WHERE { ?x a ex:Node }",
        )],
        true,
        Some(Budget {
            rounds: 100,
            nulls: 10,
            ops: 10,
            deadline: Instant::now() - Duration::from_secs(1),
        }),
    );
    assert_eq!(r.out.exhausted, Some("time"));
}

/// Report rules fire and are counted on the final state, emitting nothing.
#[test]
fn report_rules_are_counted_not_applied() {
    let mut report = rule(
        "urn:rule:report",
        "CONSTRUCT { ?x ex:bad true } WHERE { ?x ex:code ?c FILTER(!STRSTARTS(?c, \"A\")) }",
    );
    report.policy = super::rules::Policy::Report;
    let data = format!(
        "GRAPH <{G1}> {{ ex:a ex:code \"A1\" . ex:b ex:code \"B1\" . ex:c ex:code \"C1\" . }}"
    );
    let r = run(&store(&data), vec![report]);
    assert!(r.out.added.is_empty());
    assert_eq!(r.out.report_triggers, vec![(0, 2)]);
}

/// The patch round-trips through the parser and carries content-derived
/// headers only.
#[test]
fn the_patch_is_content_addressed() {
    let r = run(&store(CHAIN), vec![closure()]);
    let p = patch(&r);
    let parsed = crate::rdf_patch::parse(&p).unwrap();
    let id = parsed.id().unwrap();
    assert!(id.starts_with("urn:ots:proposal:") && id.len() == "urn:ots:proposal:".len() + 32);
    assert!(p.contains("H engine \"ots-chase/0.1\" ."));
    assert!(p.contains("# rule=<urn:rule:closure>"), "{p}");
    // A different rule set is a different proposal.
    let mut other = closure();
    other.priority = 3.0;
    let r2 = run(&store(CHAIN), vec![other]);
    assert_ne!(crate::rdf_patch::parse(&patch(&r2)).unwrap().id(), Some(id));
}

#[allow(dead_code)]
fn quads(s: &TripleStore, g: &str) -> Vec<Quad> {
    s.quads_for_graph(GraphName::NamedNode(oxigraph::model::NamedNode::new(g).unwrap()).as_ref())
        .unwrap()
}

#[allow(dead_code)]
fn term(iri: &str) -> Term {
    Term::NamedNode(oxigraph::model::NamedNode::new(iri).unwrap())
}

// ── SHACL → rules → chase → validator ───────────────────────────────────────

const SHAPES: &str = "http://example.org/shapes";

const BRIDGE_SHAPES: &str = r#"
@prefix ex: <http://example.org/> .
@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
ex:BridgeShape a sh:NodeShape ; sh:targetClass ex:Bridge ;
  sh:property [ sh:path ex:hasDeck ; sh:minCount 1 ; sh:class ex:Deck ] ;
  sh:property [ sh:path ex:status ; sh:hasValue ex:Active ] ;
  sh:property [ sh:path ex:name ; sh:minCount 1 ; sh:datatype xsd:string ] ;
  sh:property [ sh:path ex:span ; sh:maxCount 1 ] .
ex:DeckShape a sh:NodeShape ; sh:targetClass ex:Deck ;
  sh:property [ sh:path ex:material ; sh:minCount 1 ; sh:in ( ex:Steel ) ] .
"#;

struct Pipeline {
    store: TripleStore,
    run: Run,
    report_only: Vec<super::compile::ReportOnly>,
}

fn pipeline(data: &str, policies: super::compile::Policies) -> Pipeline {
    let s = store(data);
    s.load_str(BRIDGE_SHAPES, RdfFormat::Turtle, Some(SHAPES))
        .unwrap();
    let graphs = vec![G1.to_string(), G2.to_string(), MODEL.to_string()];
    let compiled = super::compile::compile_shapes(&s, SHAPES, &graphs, policies).unwrap();
    let run = run(&s, compiled.rules);
    Pipeline {
        store: s,
        run,
        report_only: compiled.report_only,
    }
}

fn violations(s: &TripleStore) -> Vec<(String, String, String)> {
    let graphs = vec![G1.to_string(), G2.to_string(), MODEL.to_string()];
    let report = crate::shacl::validate(s, SHAPES, &graphs).unwrap();
    let mut v: Vec<(String, String, String)> = report
        .results
        .iter()
        .map(|r| {
            (
                r.focus_node.clone(),
                r.path.clone().unwrap_or_default(),
                r.source_constraint
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .to_string(),
            )
        })
        .collect();
    v.sort();
    v
}

/// The compiled rules repair every violation the shapes determine — the
/// validator reports only what was left on purpose — and the compiled set
/// says what it left and why.
#[test]
fn compiled_shapes_repair_what_they_determine() {
    let data = format!(
        "GRAPH <{G1}> {{ ex:b1 a ex:Bridge . ex:b2 a ex:Bridge ; ex:hasDeck ex:d2 . ex:b3 a ex:SubBridge ; ex:span 10, 12 . }} \
         GRAPH <{MODEL}> {{ ex:SubBridge <http://www.w3.org/2000/01/rdf-schema#subClassOf> ex:Bridge . }}"
    );
    let before = violations(&store_with_shapes(&data));
    assert!(before.len() >= 9, "{before:#?}");
    let p = pipeline(&data, Default::default());
    assert!(p.run.out.exhausted.is_none());
    let after = violations(&p.store);
    // Left: every bridge's name (sh:datatype sibling, report-only) and b3's
    // second span (sh:maxCount without the policy).
    assert_eq!(
        after,
        [
            (
                "http://example.org/b1".into(),
                "<http://example.org/name>".into(),
                "sh:minCount".into()
            ),
            (
                "http://example.org/b2".into(),
                "<http://example.org/name>".into(),
                "sh:minCount".into()
            ),
            (
                "http://example.org/b3".into(),
                "<http://example.org/name>".into(),
                "sh:minCount".into()
            ),
            (
                "http://example.org/b3".into(),
                "<http://example.org/span>".into(),
                "sh:maxCount".into()
            ),
        ],
        "everything else is repaired"
    );
    let a = added(&p.run);
    // d2 is typed, not replaced; b2 gets no second deck.
    assert!(a.contains(&format!("<http://example.org/d2> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://example.org/Deck> <{G1}>")), "{a:#?}");
    assert!(
        !a.iter()
            .any(|q| q.starts_with("<http://example.org/b2> <http://example.org/hasDeck>")),
        "{a:#?}"
    );
    // The witness of b1 is a Deck made of steel (sh:in with one member).
    assert!(
        a.iter()
            .filter(|q| q.contains("/material> <http://example.org/Steel>"))
            .count()
            >= 3,
        "{a:#?}"
    );
    // b3 is a SHACL instance of ex:Bridge through the model's subclass axiom.
    assert!(
        a.iter()
            .any(|q| q.starts_with("<http://example.org/b3> <http://example.org/status>")),
        "{a:#?}"
    );
    // What was not repaired, and why.
    let reasons: Vec<&str> = p
        .report_only
        .iter()
        .map(|r| r.source_constraint_component.as_str())
        .collect();
    assert!(
        reasons.contains(&"http://www.w3.org/ns/shacl#MinCountConstraintComponent"),
        "{reasons:?}"
    );
    assert!(
        reasons.contains(&"http://www.w3.org/ns/shacl#MaxCountConstraintComponent"),
        "{reasons:?}"
    );
    assert!(
        reasons.contains(&"http://www.w3.org/ns/shacl#DatatypeConstraintComponent"),
        "{reasons:?}"
    );
    // Every action names the violation it answers.
    let (_, d) = &p.run.out.added[0];
    let rule = &p.run.rules[d.rule];
    assert!(rule.spec.violation.is_some());
    // The second run over the repaired state is a no-op.
    let again = run(
        &p.store,
        p.run.rules.iter().map(|r| r.spec.clone()).collect(),
    );
    assert!(again.out.added.is_empty(), "{:?}", added(&again));
}

fn store_with_shapes(data: &str) -> TripleStore {
    let s = store(data);
    s.load_str(BRIDGE_SHAPES, RdfFormat::Turtle, Some(SHAPES))
        .unwrap();
    s
}

/// The opt-in policies: `maxCount-keep-lexmin` keeps the smallest values,
/// `datatype-relabel` relabels a well-formed literal and leaves an
/// ill-formed one.
#[test]
fn opt_in_policies_make_their_declared_choice() {
    let data = format!(
        "GRAPH <{G1}> {{ ex:b1 a ex:Bridge ; ex:hasDeck ex:d1 ; ex:status ex:Active ; ex:name \"One\"^^xsd:token ; ex:span 12, 10, 11 . \
         ex:d1 a ex:Deck ; ex:material ex:Steel . \
         ex:b2 a ex:Bridge ; ex:hasDeck ex:d1 ; ex:status ex:Active ; ex:name 42 . }}"
    );
    let p = pipeline(
        &data,
        super::compile::Policies {
            max_count_keep_lexmin: true,
            datatype_relabel: true,
            closed_delete: false,
        },
    );
    let d = deleted(&p.run);
    assert_eq!(
        d.iter().filter(|q| q.contains("/span>")).count(),
        2,
        "two of three spans go: {d:#?}"
    );
    assert!(
        d.iter().any(|q| q.contains("/span> \"11\""))
            && d.iter().any(|q| q.contains("/span> \"12\"")),
        "the smallest in N-Triples order stays: {d:#?}"
    );
    let a = added(&p.run);
    assert!(
        a.contains(&format!(
            "<http://example.org/b1> <http://example.org/name> \"One\" <{G1}>"
        )),
        "{a:#?}"
    );
    assert!(d.contains(&format!("<http://example.org/b1> <http://example.org/name> \"One\"^^<http://www.w3.org/2001/XMLSchema#token> <{G1}>")), "{d:#?}");
    // "42"^^xsd:integer relabelled as xsd:string is valid ("42" is a string).
    assert!(
        a.contains(&format!(
            "<http://example.org/b2> <http://example.org/name> \"42\" <{G1}>"
        )),
        "{a:#?}"
    );
    let span_rule = p
        .run
        .rules
        .iter()
        .find(|r| r.spec.iri.contains("maxcount-keep-lexmin"))
        .unwrap();
    assert!(span_rule.spec.destructive && span_rule.is_policy_deletion());
    assert!(
        violations(&p.store).is_empty(),
        "{:#?}",
        violations(&p.store)
    );
}

/// The note's two-graph fixture (§4.6, open question 8): the compiler agrees
/// with the engine on per-graph targets, cross-graph single-hop values and
/// cross-graph typing, and places a witness in its focus node's graph.
#[test]
fn two_graph_fixture_pins_placement_and_confinement() {
    let data = format!(
        "GRAPH <{G1}> {{ ex:b1 a ex:Bridge ; ex:status ex:Active ; ex:name \"b1\" . ex:b2 a ex:Bridge ; ex:status ex:Active ; ex:name \"b2\" ; ex:hasDeck ex:d2 . }} \
         GRAPH <{G2}> {{ ex:b1 ex:hasDeck ex:d1 . ex:d1 ex:material ex:Steel . ex:d2 a ex:Deck ; ex:material ex:Steel . ex:b3 a ex:Bridge ; ex:status ex:Active ; ex:name \"b3\" . }}"
    );
    let s = store_with_shapes(&data);
    // The validator: b1's deck is linked in g2 (single hops union across
    // graphs) and d2 is typed in g2 (sh:class reads every graph); d1 is
    // untyped; b3 has no deck.
    let v = violations(&s);
    assert_eq!(
        v,
        [
            (
                "http://example.org/b1".into(),
                "<http://example.org/hasDeck>".into(),
                "sh:class".into()
            ),
            (
                "http://example.org/b3".into(),
                "<http://example.org/hasDeck>".into(),
                "sh:minCount".into()
            ),
        ],
        "the engine's reading this test pins"
    );
    let p = pipeline(&data, Default::default());
    let a = added(&p.run);
    // b1: no witness (its deck in g2 counts); its deck is typed where the
    // link is, in g2.
    assert!(
        !a.iter()
            .any(|q| q.starts_with("<http://example.org/b1> <http://example.org/hasDeck>")),
        "{a:#?}"
    );
    assert!(a.contains(&format!("<http://example.org/d1> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://example.org/Deck> <{G2}>")), "{a:#?}");
    // b2: nothing (d2 is a Deck by g2).
    assert!(
        !a.iter()
            .any(|q| q.contains("<http://example.org/d2>")
                || q.starts_with("<http://example.org/b2>")),
        "{a:#?}"
    );
    // b3, typed in g2: its witness lands in g2.
    assert!(
        a.iter().any(
            |q| q.starts_with("<http://example.org/b3> <http://example.org/hasDeck>")
                && q.ends_with(&format!("<{G2}>"))
        ),
        "{a:#?}"
    );
    assert!(
        violations(&p.store).is_empty(),
        "{:#?}",
        violations(&p.store)
    );
}

/// §3.6 "sibling-pair census": per vendored shapes graph, how many
/// `sh:minCount` occurrences mint a witness, are ground (a determined
/// value), or are report-only. Printed for the design note; the invariant
/// is that every occurrence lands in exactly one bucket.
#[test]
fn sibling_census_over_the_vendored_shapes() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let files = [
        "examples/seed-bundles/clinical-reference/shapes.ttl",
        "examples/seed-bundles/dqv-quality/shapes.ttl",
        "examples/seed-bundles/layered-reference/shapes.ttl",
        "examples/seed-bundles/nen2660-relations/shapes.ttl",
        "examples/seed-bundles/dqv-quality/quality-profile.ttl",
        "tests/fixtures/example-bridge/shapes-core.ttl",
        "tests/fixtures/example-bridge/shapes-af.ttl",
        "tests/fixtures/example-bridge/shapes-sparql.ttl",
        "tests/fixtures/ogc-geosparql/validator.ttl",
        "examples/seed-bundles/nen2660-imbor/nen2660-shacl.ttl",
    ];
    let mut totals = (0, 0, 0, 0);
    for f in files {
        let path = root.join(f);
        let Ok(ttl) = std::fs::read_to_string(&path) else {
            eprintln!("census: {f}: not present (fetched, not vendored)");
            continue;
        };
        let s = TripleStore::in_memory().unwrap();
        if s.load_str(&ttl, RdfFormat::Turtle, Some(SHAPES)).is_err() {
            eprintln!("census: {f}: does not parse as Turtle");
            continue;
        }
        let Ok(shapes) = crate::shacl::engine::load_shapes(&s, SHAPES) else {
            eprintln!("census: {f}: shapes do not load");
            continue;
        };
        let (t, w, g, r) = super::compile::min_count_census(&shapes);
        assert_eq!(t, w + g + r, "{f}");
        eprintln!("census: {f}: minCount {t} = witness {w} + ground {g} + report {r}");
        totals = (totals.0 + t, totals.1 + w, totals.2 + g, totals.3 + r);
    }
    eprintln!(
        "census: total minCount {} = witness {} + ground {} + report {}",
        totals.0, totals.1, totals.2, totals.3
    );
}

/// SHACL-AF rules imported as TGDs: a template blank node becomes a null (so
/// the rule converges where `/infer` mints a node per round), a named
/// `sh:condition` is checked per focus node, an inline one or a deactivated
/// rule is left out, and the rule IRIs are derived from the rules' text.
#[test]
fn shacl_af_rules_import_as_tgds() {
    let data = format!(
        "GRAPH <{G1}> {{ ex:b1 a ex:Bridge ; ex:status ex:Active . ex:b2 a ex:Bridge . ex:b3 a ex:Bridge . }}"
    );
    let s = store(&data);
    s.load_str(
        r#"@prefix ex: <http://example.org/> .
@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
ex:Active_ a sh:NodeShape ; sh:property [ sh:path ex:status ; sh:minCount 1 ] .
ex:Prefixes sh:declare [ sh:prefix "ex" ; sh:namespace "http://example.org/"^^xsd:anyURI ] .
ex:BridgeShape a sh:NodeShape ; sh:targetClass ex:Bridge ;
  sh:rule [ a sh:SPARQLRule ; sh:prefixes ex:Prefixes ;
    sh:construct "CONSTRUCT { $this ex:hasDeck _:w . _:w a ex:Deck } WHERE { $this a ex:Bridge }" ] ;
  sh:rule [ a sh:TripleRule ; sh:subject sh:this ; sh:predicate ex:checked ; sh:object true ;
    sh:condition ex:Active_ ] ;
  sh:rule [ a sh:TripleRule ; sh:subject sh:this ; sh:predicate ex:inline ; sh:object true ;
    sh:condition [ sh:property [ sh:path ex:status ; sh:minCount 1 ] ] ] ;
  sh:rule [ a sh:TripleRule ; sh:subject sh:this ; sh:predicate ex:off ; sh:object true ;
    sh:deactivated true ] .
"#,
        RdfFormat::Turtle,
        Some(SHAPES),
    )
    .unwrap();
    let graphs = vec![G1.to_string(), G2.to_string(), MODEL.to_string()];
    let import = || super::compile::import_shacl_af(&s, SHAPES, &graphs).unwrap();
    let compiled = import();
    assert_eq!(compiled.rules.len(), 2, "{:#?}", compiled.rules);
    assert!(
        compiled
            .skipped
            .iter()
            .any(|m| m.contains("sh:condition") && m.contains("not a named shape")),
        "{:?}",
        compiled.skipped
    );
    let again = import();
    let iris = |c: &super::compile::Compiled| -> Vec<String> {
        c.rules.iter().map(|r| r.iri.clone()).collect()
    };
    assert_eq!(iris(&compiled), iris(&again), "rule IRIs are stable");
    assert!(compiled.rules.iter().all(|r| r.origin == Origin::ShaclAf));

    let r = run(&s, compiled.rules);
    assert!(r.out.exhausted.is_none());
    let a = added(&r);
    // A deck and its type per bridge, and `checked` only where the
    // condition holds.
    assert_eq!(a.len(), 3 * 2 + 1, "{a:#?}");
    assert_eq!(r.out.nulls, 3);
    assert!(a.contains(&format!(
        "<http://example.org/b1> <http://example.org/checked> \"true\"^^<http://www.w3.org/2001/XMLSchema#boolean> <{G1}>"
    )));
    assert!(!a
        .iter()
        .any(|q| q.contains("http://example.org/inline") || q.contains("http://example.org/off")));
    apply(&s, &patch(&r));
    let second = run(&s, import().rules);
    assert!(added(&second).is_empty(), "{:#?}", added(&second));
}
