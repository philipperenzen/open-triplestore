//! OWL 2 RL profile — forward-chaining materialization.
//!
//! Implements the W3C OWL 2 Profiles §4.3 RL/RDF rules (Tables 4–9) as
//! SPARQL INSERT operations executed in a fixed-point loop: 75 of the 78
//! rules run ([`IMPLEMENTED_RULES`]); the 3 that do not are listed with
//! their reason in [`UNIMPLEMENTED_RULES`], and `tests/owl2_rl_conformance.rs`
//! pins both lists against the specification's inventory.
//!
//! Inconsistency-detection rules raise `ReasoningError::Inconsistency` rather
//! than inserting triples.
//!
//! Lists (`owl:intersectionOf`, `owl:propertyChainAxiom`, `owl:hasKey`) are
//! matched at their exact length, one INSERT per length present, so a member
//! may be any term, a blank-node class expression included.
//!
//! A property position may hold an inverse property expression
//! `[ owl:inverseOf P ]`, a blank node. No RDF triple has a blank-node
//! predicate, so a premise `?u PE ?v` also reads `?v P ?u` ([`pe`]) and a
//! conclusion `(x, [ owl:inverseOf P ], y)` is written as `y P x` ([`pe_head`]).
//!
//! `eq-ref` (every term is `owl:sameAs` itself) runs only when asked for
//! ([`Owl2RLReasoner::with_eq_ref`]): it adds about one triple per term. The
//! inconsistencies it leads to (`x owl:differentFrom x`, a member listed twice
//! in `owl:AllDifferent`) are found either way.
//!
//! # Usage
//! ```no_run
//! # use open_triplestore::reasoning::owl2_rl::Owl2RLReasoner;
//! # use open_triplestore::store::TripleStore;
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! # let store = TripleStore::in_memory()?;
//! let report = Owl2RLReasoner::new(&store)
//!     .with_target("urn:entailment:owl2-rl")
//!     .materialize()?;
//! # Ok(())
//! # }
//! ```
#![allow(dead_code)]

use std::time::Instant;
use tracing::{debug, info};

use super::common::{count_graph, ReasoningError, ReasoningReport, OWL2_RL_ENTAILMENT_GRAPH};
use super::identity::IdentityPolicy;
use crate::store::TripleStore;

// ─── Namespace constants ──────────────────────────────────────────────────────

const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const RDFS_DOMAIN: &str = "http://www.w3.org/2000/01/rdf-schema#domain";
const RDFS_RANGE: &str = "http://www.w3.org/2000/01/rdf-schema#range";
const RDFS_SUB_CLASS_OF: &str = "http://www.w3.org/2000/01/rdf-schema#subClassOf";
const RDFS_SUB_PROPERTY_OF: &str = "http://www.w3.org/2000/01/rdf-schema#subPropertyOf";
const OWL_SAME_AS: &str = "http://www.w3.org/2002/07/owl#sameAs";
const OWL_DIFFERENT_FROM: &str = "http://www.w3.org/2002/07/owl#differentFrom";
const OWL_EQUIV_CLASS: &str = "http://www.w3.org/2002/07/owl#equivalentClass";
const OWL_EQUIV_PROP: &str = "http://www.w3.org/2002/07/owl#equivalentProperty";
const OWL_INVERSE_OF: &str = "http://www.w3.org/2002/07/owl#inverseOf";
const OWL_DISJOINT_WITH: &str = "http://www.w3.org/2002/07/owl#disjointWith";
const OWL_FUNCTIONAL_PROP: &str = "http://www.w3.org/2002/07/owl#FunctionalProperty";
const OWL_INV_FUNCTIONAL: &str = "http://www.w3.org/2002/07/owl#InverseFunctionalProperty";
const OWL_SYMMETRIC_PROP: &str = "http://www.w3.org/2002/07/owl#SymmetricProperty";
const OWL_ASYMMETRIC_PROP: &str = "http://www.w3.org/2002/07/owl#AsymmetricProperty";
const OWL_IRREFLEXIVE_PROP: &str = "http://www.w3.org/2002/07/owl#IrreflexiveProperty";
const OWL_TRANSITIVE_PROP: &str = "http://www.w3.org/2002/07/owl#TransitiveProperty";
const OWL_NOTHING: &str = "http://www.w3.org/2002/07/owl#Nothing";
const OWL_HAS_VALUE: &str = "http://www.w3.org/2002/07/owl#hasValue";
const OWL_ON_PROPERTY: &str = "http://www.w3.org/2002/07/owl#onProperty";
const OWL_SOME_VALUES_FROM: &str = "http://www.w3.org/2002/07/owl#someValuesFrom";
const OWL_ALL_VALUES_FROM: &str = "http://www.w3.org/2002/07/owl#allValuesFrom";
const OWL_MAX_CARDINALITY: &str = "http://www.w3.org/2002/07/owl#maxCardinality";
const OWL_MAX_QUAL_CARD: &str = "http://www.w3.org/2002/07/owl#maxQualifiedCardinality";
const OWL_INTERSECTION_OF: &str = "http://www.w3.org/2002/07/owl#intersectionOf";
const OWL_UNION_OF: &str = "http://www.w3.org/2002/07/owl#unionOf";
const OWL_ONE_OF: &str = "http://www.w3.org/2002/07/owl#oneOf";
const OWL_PROP_CHAIN_AXIOM: &str = "http://www.w3.org/2002/07/owl#propertyChainAxiom";
const OWL_MEMBERS: &str = "http://www.w3.org/2002/07/owl#members";
const OWL_ALL_DISJOINT: &str = "http://www.w3.org/2002/07/owl#AllDisjointClasses";
const OWL_COMPLEMENT_OF: &str = "http://www.w3.org/2002/07/owl#complementOf";
const OWL_HAS_KEY: &str = "http://www.w3.org/2002/07/owl#hasKey";
const OWL_THING: &str = "http://www.w3.org/2002/07/owl#Thing";
const OWL_CLASS: &str = "http://www.w3.org/2002/07/owl#Class";
const OWL_OBJECT_PROPERTY: &str = "http://www.w3.org/2002/07/owl#ObjectProperty";
const OWL_DATATYPE_PROPERTY: &str = "http://www.w3.org/2002/07/owl#DatatypeProperty";
const OWL_ANNOTATION_PROPERTY: &str = "http://www.w3.org/2002/07/owl#AnnotationProperty";
const OWL_ON_CLASS: &str = "http://www.w3.org/2002/07/owl#onClass";
const OWL_SOURCE_INDIVIDUAL: &str = "http://www.w3.org/2002/07/owl#sourceIndividual";
const OWL_ASSERTION_PROPERTY: &str = "http://www.w3.org/2002/07/owl#assertionProperty";
const OWL_TARGET_INDIVIDUAL: &str = "http://www.w3.org/2002/07/owl#targetIndividual";
const OWL_TARGET_VALUE: &str = "http://www.w3.org/2002/07/owl#targetValue";
const RDF_FIRST: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#first";
const RDF_REST: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#rest";
const RDF_NIL: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#nil";
const OWL_PROP_DISJOINT_WITH: &str = "http://www.w3.org/2002/07/owl#propertyDisjointWith";
const OWL_ALL_DISJOINT_PROPS: &str = "http://www.w3.org/2002/07/owl#AllDisjointProperties";
const OWL_ALL_DIFFERENT: &str = "http://www.w3.org/2002/07/owl#AllDifferent";
const OWL_DISTINCT_MEMBERS: &str = "http://www.w3.org/2002/07/owl#distinctMembers";

/// The annotation properties `prp-ap` declares (OWL 2 Profiles §4.3, Table 5).
const ANNOTATION_PROPERTIES: &[&str] = &[
    "http://www.w3.org/2000/01/rdf-schema#label",
    "http://www.w3.org/2000/01/rdf-schema#comment",
    "http://www.w3.org/2000/01/rdf-schema#seeAlso",
    "http://www.w3.org/2000/01/rdf-schema#isDefinedBy",
    "http://www.w3.org/2002/07/owl#deprecated",
    "http://www.w3.org/2002/07/owl#versionInfo",
    "http://www.w3.org/2002/07/owl#priorVersion",
    "http://www.w3.org/2002/07/owl#backwardCompatibleWith",
    "http://www.w3.org/2002/07/owl#incompatibleWith",
];

/// Longest list (intersection, chain, key) the n-ary rules match.
const MAX_LIST_LEN: usize = 64;

const MAX_ITERATIONS: usize = 500;

/// The OWL 2 RL/RDF rules this engine runs, by their specification names
/// (OWL 2 Profiles §4.3, Tables 4–9). With [`UNIMPLEMENTED_RULES`] this is
/// exactly the specification's 78 rules — `tests/owl2_rl_conformance.rs`
/// asserts it, so a rule cannot appear or disappear without the record.
pub const IMPLEMENTED_RULES: &[&str] = &[
    // Table 4 — equality
    "eq-ref",
    "eq-sym",
    "eq-trans",
    "eq-rep-s",
    "eq-rep-p",
    "eq-rep-o",
    "eq-diff1",
    "eq-diff2",
    "eq-diff3",
    // Table 5 — property axioms
    "prp-ap",
    "prp-dom",
    "prp-rng",
    "prp-fp",
    "prp-ifp",
    "prp-irp",
    "prp-symp",
    "prp-asyp",
    "prp-trp",
    "prp-spo1",
    "prp-spo2",
    "prp-eqp1",
    "prp-eqp2",
    "prp-pdw",
    "prp-adp",
    "prp-inv1",
    "prp-inv2",
    "prp-key",
    "prp-npa1",
    "prp-npa2",
    // Table 6 — classes
    "cls-thing",
    "cls-nothing1",
    "cls-nothing2",
    "cls-int1",
    "cls-int2",
    "cls-uni",
    "cls-com",
    "cls-svf1",
    "cls-svf2",
    "cls-avf",
    "cls-hv1",
    "cls-hv2",
    "cls-maxc1",
    "cls-maxc2",
    "cls-maxqc1",
    "cls-maxqc2",
    "cls-maxqc3",
    "cls-maxqc4",
    "cls-oo",
    // Table 7 — class axioms
    "cax-sco",
    "cax-eqc1",
    "cax-eqc2",
    "cax-dw",
    "cax-adc",
    // Table 8 — datatypes
    "dt-type1",
    "dt-not-type",
    // Table 9 — schema
    "scm-cls",
    "scm-sco",
    "scm-eqc1",
    "scm-eqc2",
    "scm-op",
    "scm-dp",
    "scm-spo",
    "scm-eqp1",
    "scm-eqp2",
    "scm-dom1",
    "scm-dom2",
    "scm-rng1",
    "scm-rng2",
    "scm-hv",
    "scm-svf1",
    "scm-svf2",
    "scm-avf1",
    "scm-avf2",
    "scm-int",
    "scm-uni",
];

/// The RL/RDF rules this engine does not run, each with the reason. Kept
/// next to [`IMPLEMENTED_RULES`] so the two lists together are the whole
/// specification and the documentation cannot drift from the code.
pub const UNIMPLEMENTED_RULES: &[(&str, &str)] = &[
    ("dt-type2", "typing every literal with its datatype needs literal subjects, which an RDF graph cannot hold"),
    ("dt-eq", "owl:sameAs between literals with equal values needs literal subjects, and SPARQL joins compare terms, not values"),
    ("dt-diff", "owl:differentFrom between literals needs literal subjects"),
];

/// The datatypes of the OWL 2 RL datatype map (OWL 2 Profiles §4.2): what
/// `dt-type1` declares as `rdfs:Datatype`.
const RL_DATATYPES: &[&str] = &[
    "http://www.w3.org/1999/02/22-rdf-syntax-ns#PlainLiteral",
    "http://www.w3.org/1999/02/22-rdf-syntax-ns#XMLLiteral",
    "http://www.w3.org/2000/01/rdf-schema#Literal",
    "http://www.w3.org/2001/XMLSchema#decimal",
    "http://www.w3.org/2001/XMLSchema#integer",
    "http://www.w3.org/2001/XMLSchema#nonNegativeInteger",
    "http://www.w3.org/2001/XMLSchema#nonPositiveInteger",
    "http://www.w3.org/2001/XMLSchema#positiveInteger",
    "http://www.w3.org/2001/XMLSchema#negativeInteger",
    "http://www.w3.org/2001/XMLSchema#long",
    "http://www.w3.org/2001/XMLSchema#int",
    "http://www.w3.org/2001/XMLSchema#short",
    "http://www.w3.org/2001/XMLSchema#byte",
    "http://www.w3.org/2001/XMLSchema#unsignedLong",
    "http://www.w3.org/2001/XMLSchema#unsignedInt",
    "http://www.w3.org/2001/XMLSchema#unsignedShort",
    "http://www.w3.org/2001/XMLSchema#unsignedByte",
    "http://www.w3.org/2001/XMLSchema#float",
    "http://www.w3.org/2001/XMLSchema#double",
    "http://www.w3.org/2001/XMLSchema#string",
    "http://www.w3.org/2001/XMLSchema#normalizedString",
    "http://www.w3.org/2001/XMLSchema#token",
    "http://www.w3.org/2001/XMLSchema#language",
    "http://www.w3.org/2001/XMLSchema#Name",
    "http://www.w3.org/2001/XMLSchema#NCName",
    "http://www.w3.org/2001/XMLSchema#NMTOKEN",
    "http://www.w3.org/2001/XMLSchema#boolean",
    "http://www.w3.org/2001/XMLSchema#hexBinary",
    "http://www.w3.org/2001/XMLSchema#base64Binary",
    "http://www.w3.org/2001/XMLSchema#anyURI",
    "http://www.w3.org/2001/XMLSchema#dateTime",
    "http://www.w3.org/2001/XMLSchema#dateTimeStamp",
];
const RDFS_DATATYPE: &str = "http://www.w3.org/2000/01/rdf-schema#Datatype";

// ─── Property expressions and lists ──────────────────────────────────────────

/// A SPARQL variable name made from `parts` (`?p`, `?u` …), for the helper
/// variables of one [`pe`] / [`pe_head`] use. Distinct arguments give distinct
/// names, so several uses in one rule do not share their helpers.
fn helper_var(prefix: &str, parts: &[&str]) -> String {
    let mut v = format!("?{prefix}");
    for p in parts {
        v.push('_');
        v.extend(p.chars().filter(|c| c.is_ascii_alphanumeric()));
    }
    v
}

/// The premise `{s} PE {o}` for the property expression bound to the
/// variable `p`: a plain triple, or — when `p` is a blank
/// `[ owl:inverseOf Q ]` — the triple `{o} Q {s}`. `p` must be bound by the
/// rest of the rule (the axiom that names it).
fn pe(p: &str, s: &str, o: &str) -> String {
    let q = helper_var("inv", &[p, s, o]);
    format!(
        "{{ {s} {p} {o} }} UNION \
         {{ {p} <{OWL_INVERSE_OF}> {q} . FILTER(isBlank({p})) {o} {q} {s} }}"
    )
}

/// The conclusion `{s} PE {o}` for the property expression bound to `p`:
/// returns the WHERE-clause addition and the head triple. A blank
/// `[ owl:inverseOf Q ]` conclusion is written `{o} Q {s}`; a head that is
/// still not an RDF triple (a blank predicate without `owl:inverseOf`, a
/// literal subject) is filtered out rather than left to the store to drop.
fn pe_head(p: &str, s: &str, o: &str) -> (String, String) {
    let q = helper_var("hq", &[p, s, o]);
    let hs = helper_var("hs", &[p, s, o]);
    let hp = helper_var("hp", &[p, s, o]);
    let ho = helper_var("ho", &[p, s, o]);
    let clause = format!(
        "OPTIONAL {{ {p} <{OWL_INVERSE_OF}> {q} . FILTER(isBlank({p})) }} \
         BIND(IF(BOUND({q}), {o}, {s}) AS {hs}) \
         BIND(IF(BOUND({q}), {q}, {p}) AS {hp}) \
         BIND(IF(BOUND({q}), {s}, {o}) AS {ho}) \
         FILTER(isIRI({hp}) && !isLiteral({hs}))"
    );
    (clause, format!("{hs} {hp} {ho}"))
}

/// The `rdf:first` / `rdf:rest` pattern of a list of exactly `n` cells
/// starting at `head`, binding its members to `?{m}0` … `?{m}{n-1}`.
fn list_n(head: &str, m: &str, n: usize) -> String {
    let mut out = String::new();
    let mut cell = head.to_string();
    for i in 0..n {
        let next = if i + 1 == n {
            format!("<{RDF_NIL}>")
        } else {
            format!("?{m}_cell{}", i + 1)
        };
        out.push_str(&format!(
            "{cell} <{RDF_FIRST}> ?{m}{i} ; <{RDF_REST}> {next} . "
        ));
        cell = next;
    }
    out
}

// ─── Reasoner ─────────────────────────────────────────────────────────────────

/// OWL 2 RL forward-chaining reasoner.
pub struct Owl2RLReasoner<'a> {
    store: &'a TripleStore,
    target_graph: String,
    /// When set, the rules read ONLY these graphs (plus the target graph).
    /// Without it they read the unnamed default graph plus the target graph
    /// (`TripleStore::update_over`), so rules see their own consequences.
    sources: Option<Vec<String>>,
    /// If `true`, inconsistency rules raise `ReasoningError::Inconsistency`.
    pub detect_inconsistency: bool,
    /// Fixed-point rounds before the run fails with `NotConverged`.
    max_iterations: usize,
    /// What to do with `owl:sameAs`: `sameas-off` skips the Table 4 equality
    /// rules. The raw engine defaults to `sameas-full`; the per-dataset policy
    /// is applied by the entailment layer (see `crate::entailment`).
    identity: IdentityPolicy,
    /// Run `eq-ref`: write `x owl:sameAs x` for every term. Off by default.
    eq_ref: bool,
}

impl<'a> Owl2RLReasoner<'a> {
    /// Restrict the rules to `sources` (plus the target graph). Without a
    /// scope the rules read the unnamed default graph and the target graph, so
    /// a dataset's named graphs — and the model version it conforms to — are
    /// invisible to materialisation; this is what `POST /api/reasoning/materialize` sets
    /// from `source_graphs` or the dataset's conformance layer.
    pub fn with_sources(mut self, sources: Vec<String>) -> Self {
        self.sources = Some(sources);
        self
    }

    fn scope(&self) -> Option<Vec<String>> {
        self.sources.as_ref().map(|s| {
            let mut g = s.clone();
            if !g.contains(&self.target_graph) {
                g.push(self.target_graph.clone());
            }
            g
        })
    }

    fn run_update(&self, sparql: &str) -> Result<(), crate::store::engine::StoreError> {
        match self.scope() {
            Some(scope) => self.store.update_scoped(sparql, &scope),
            None => self
                .store
                .update_over(sparql, std::slice::from_ref(&self.target_graph)),
        }
    }

    fn run_query(
        &self,
        sparql: &str,
    ) -> Result<oxigraph::sparql::QueryResults<'static>, crate::store::engine::StoreError> {
        match self.scope() {
            Some(scope) => self.store.query_scoped(sparql, &scope),
            None => self
                .store
                .query_over(sparql, std::slice::from_ref(&self.target_graph)),
        }
    }

    pub fn new(store: &'a TripleStore) -> Self {
        Self {
            store,
            target_graph: OWL2_RL_ENTAILMENT_GRAPH.to_string(),
            sources: None,
            detect_inconsistency: true,
            max_iterations: MAX_ITERATIONS,
            identity: IdentityPolicy::Full,
            eq_ref: false,
        }
    }

    /// Run `eq-ref`: every subject, predicate and non-literal object becomes
    /// `owl:sameAs` itself. Off by default, because it adds about one triple
    /// per term and no other rule needs those triples to fire; `sameas-off`
    /// skips it with the other equality rules.
    pub fn with_eq_ref(mut self, on: bool) -> Self {
        self.eq_ref = on;
        self
    }

    /// Fail with [`ReasoningError::NotConverged`] after `n` rounds without a
    /// fixed point (default 500) instead of running on.
    pub fn with_max_iterations(mut self, n: usize) -> Self {
        self.max_iterations = n.max(1);
        self
    }

    pub fn with_target(mut self, graph: impl Into<String>) -> Self {
        self.target_graph = graph.into();
        self
    }

    /// Apply an identity policy: under `sameas-off` the Table 4 equality rules
    /// (`eq-sym`, `eq-trans`, `eq-rep-s/p/o`) do not run, so `owl:sameAs`
    /// stays data and nothing is propagated across it. Whether linkset graphs
    /// are premises at all is decided by the caller when it chooses the
    /// sources ([`crate::entailment::reasoning_sources`]).
    pub fn with_identity_policy(mut self, policy: IdentityPolicy) -> Self {
        self.identity = policy;
        self
    }

    /// Run all RL rules to fixed point, then check consistency.
    pub fn materialize(&self) -> Result<ReasoningReport, ReasoningError> {
        let start = Instant::now();
        let mut iterations = 0usize;
        // Report the delta this run produced, not the graph's final size.
        let initial = count_graph(self.store, &self.target_graph)?;

        info!("OWL 2 RL materialization → <{}>", self.target_graph);

        // The axiomatic rules have no premises, so they run once per run:
        // dt-type1 (the datatype map), prp-ap, cls-thing and cls-nothing1.
        self.rule_dt_type1()?;
        self.rule_axiomatic()?;

        loop {
            iterations += 1;
            let before = count_graph(self.store, &self.target_graph)?;

            // Table 9 — Schema (must run first to populate scm triples)
            self.rule_scm_cls()?;
            self.rule_scm_op()?;
            self.rule_scm_dp()?;
            self.rule_scm_sco()?;
            self.rule_scm_spo()?;
            self.rule_scm_eqc1()?;
            self.rule_scm_eqc2()?;
            self.rule_scm_eqp1()?;
            self.rule_scm_eqp2()?;
            self.rule_scm_dom1()?;
            self.rule_scm_dom2()?;
            self.rule_scm_rng1()?;
            self.rule_scm_rng2()?;
            self.rule_scm_hv()?;
            self.rule_scm_svf1()?;
            self.rule_scm_svf2()?;
            self.rule_scm_avf1()?;
            self.rule_scm_avf2()?;
            self.rule_scm_int()?;
            self.rule_scm_uni()?;

            // Table 7 — Class axioms
            self.rule_cax_sco()?;
            self.rule_cax_eqc1()?;
            self.rule_cax_eqc2()?;
            self.rule_cax_adc()?;

            // Table 5 — Property axioms
            self.rule_prp_dom()?;
            self.rule_prp_rng()?;
            self.rule_prp_symp()?;
            self.rule_prp_trp()?;
            self.rule_prp_spo1()?;
            self.rule_prp_spo2()?;
            self.rule_prp_eqp()?;
            self.rule_prp_inv1()?;
            self.rule_prp_inv2()?;
            self.rule_prp_fp()?;
            self.rule_prp_ifp()?;
            self.rule_prp_key()?;

            // Table 6 — Classes
            self.rule_cls_int1()?;
            self.rule_cls_int2()?;
            self.rule_cls_uni()?;
            self.rule_cls_svf1()?;
            self.rule_cls_svf2()?;
            self.rule_cls_avf()?;
            self.rule_cls_hv1()?;
            self.rule_cls_hv2()?;
            self.rule_cls_maxc2()?;
            self.rule_cls_maxqc1()?;
            self.rule_cls_maxqc2()?;
            self.rule_cls_maxqc3()?;
            self.rule_cls_maxqc4()?;
            self.rule_cls_oo()?;

            // Table 4 — Equality. Skipped under `sameas-off`: owl:sameAs is
            // then plain data (still derivable by prp-fp/ifp/key, never
            // propagated). The rules only ever match owl:sameAs — a typed
            // correspondence (prov:specializationOf, skos:exactMatch, …)
            // is never a premise here, whatever the policy.
            if self.identity.propagates_same_as() {
                if self.eq_ref {
                    self.rule_eq_ref()?;
                }
                self.rule_eq_sym()?;
                self.rule_eq_trans()?;
                self.rule_eq_rep_s()?;
                self.rule_eq_rep_p()?;
                self.rule_eq_rep_o()?;
            }

            let after = count_graph(self.store, &self.target_graph)?;
            let added = after.saturating_sub(before);
            debug!("OWL 2 RL iteration {}: +{} triples", iterations, added);
            if added == 0 {
                break;
            }
            if iterations >= self.max_iterations {
                return Err(ReasoningError::NotConverged {
                    regime: "owl2-rl".to_string(),
                    iterations,
                });
            }
        }

        // Consistency checks (after fixed point)
        if self.detect_inconsistency {
            self.check_consistency()?;
        }

        let final_count = count_graph(self.store, &self.target_graph)?;
        info!(
            "OWL 2 RL materialization complete: {} triples in {} iterations ({} ms)",
            final_count,
            iterations,
            start.elapsed().as_millis()
        );

        Ok(ReasoningReport {
            regime: "owl2-rl".to_string(),
            triples_added: final_count.saturating_sub(initial),
            iterations,
            elapsed_ms: start.elapsed().as_millis() as u64,
            target_graph: self.target_graph.clone(),
        })
    }

    /// Run inconsistency checks.  Returns `Err(Inconsistency)` if any are triggered.
    pub fn check_consistency(&self) -> Result<(), ReasoningError> {
        self.rule_dt_not_type()?;
        self.rule_eq_diff1()?;
        self.rule_eq_diff23()?;
        self.rule_prp_irp()?;
        self.rule_prp_asyp()?;
        self.rule_prp_pdw()?;
        self.rule_prp_adp()?;
        self.rule_prp_npa1()?;
        self.rule_prp_npa2()?;
        self.rule_cls_nothing2()?;
        self.rule_cls_maxc1()?;
        self.rule_cls_com()?;
        self.rule_cax_dw()?;
        Ok(())
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // Table 4 — Semantics of Equality
    // ═══════════════════════════════════════════════════════════════════════════

    /// eq-sym: ?x owl:sameAs ?y → ?y owl:sameAs ?x
    fn rule_eq_sym(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?y <{OWL_SAME_AS}> ?x }} }}
               WHERE  {{ ?x <{OWL_SAME_AS}> ?y . FILTER(?x != ?y) }}"#,
            tg = self.target_graph
        );
        self.run_update(&q)?;
        Ok(())
    }

    /// eq-trans: ?x owl:sameAs ?y . ?y owl:sameAs ?z → ?x owl:sameAs ?z
    fn rule_eq_trans(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?x <{OWL_SAME_AS}> ?z }} }}
               WHERE  {{ ?x <{OWL_SAME_AS}> ?y . ?y <{OWL_SAME_AS}> ?z . FILTER(?x != ?z) }}"#,
            tg = self.target_graph
        );
        self.run_update(&q)?;
        Ok(())
    }

    /// eq-rep-s: ?s owl:sameAs ?s' . ?s ?p ?o → ?s' ?p ?o
    fn rule_eq_rep_s(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?sp ?p ?o }} }}
               WHERE  {{ ?s <{OWL_SAME_AS}> ?sp . ?s ?p ?o .
                         FILTER(?s != ?sp) FILTER(?p != <{OWL_SAME_AS}>) }}"#,
            tg = self.target_graph
        );
        self.run_update(&q)?;
        Ok(())
    }

    /// eq-rep-p: ?p owl:sameAs ?p' . ?s ?p ?o → ?s ?p' ?o
    fn rule_eq_rep_p(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?s ?pp ?o }} }}
               WHERE  {{ ?p <{OWL_SAME_AS}> ?pp . ?s ?p ?o .
                         FILTER(?p != ?pp) FILTER(isIRI(?pp)) }}"#,
            tg = self.target_graph
        );
        self.run_update(&q)?;
        Ok(())
    }

    /// eq-rep-o: ?o owl:sameAs ?o' . ?s ?p ?o → ?s ?p ?o'
    fn rule_eq_rep_o(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?s ?p ?op }} }}
               WHERE  {{ ?o <{OWL_SAME_AS}> ?op . ?s ?p ?o . FILTER(?o != ?op) }}"#,
            tg = self.target_graph
        );
        self.run_update(&q)?;
        Ok(())
    }

    /// eq-diff1: ?x owl:sameAs ?y . ?x owl:differentFrom ?y → INCONSISTENCY.
    /// `?x owl:differentFrom ?x` is caught too: eq-ref makes every term
    /// `owl:sameAs` itself, whether or not those triples are written.
    fn rule_eq_diff1(&self) -> Result<(), ReasoningError> {
        let q = format!(
            "ASK {{ {{ ?x <{OWL_SAME_AS}> ?y . ?x <{OWL_DIFFERENT_FROM}> ?y }} \
                    UNION {{ ?x <{OWL_DIFFERENT_FROM}> ?x }} }}"
        );
        if self.ask(&q)? {
            return Err(ReasoningError::inconsistency(
                "eq-diff1",
                "owl:sameAs and owl:differentFrom on the same pair",
            ));
        }
        Ok(())
    }

    /// eq-ref: every term is `owl:sameAs` itself. Literals cannot be
    /// subjects, so a literal object gets no triple. Opt-in (`with_eq_ref`).
    fn rule_eq_ref(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?s <{OWL_SAME_AS}> ?s . ?p <{OWL_SAME_AS}> ?p }} }}
               WHERE  {{ ?s ?p ?o }} ;
               INSERT {{ GRAPH <{tg}> {{ ?o <{OWL_SAME_AS}> ?o }} }}
               WHERE  {{ ?s ?p ?o . FILTER(!isLiteral(?o)) }}"#,
            tg = self.target_graph
        );
        self.run_update(&q)?;
        Ok(())
    }

    /// eq-diff2 / eq-diff3: `?x a owl:AllDifferent ; owl:members` (eq-diff2)
    /// or `owl:distinctMembers` (eq-diff3) `(?z1 … ?zn)` with `?zi owl:sameAs
    /// ?zj` for two different positions → INCONSISTENCY. The same individual
    /// at two positions counts (eq-ref).
    fn rule_eq_diff23(&self) -> Result<(), ReasoningError> {
        for (rule, members) in [
            ("eq-diff2", OWL_MEMBERS),
            ("eq-diff3", OWL_DISTINCT_MEMBERS),
        ] {
            let q = format!(
                "ASK {{ ?a <{RDF_TYPE}> <{OWL_ALL_DIFFERENT}> ; <{members}> ?l . \
                        ?l <{RDF_REST}>* ?ci . ?ci <{RDF_FIRST}> ?zi . \
                        ?l <{RDF_REST}>* ?cj . ?cj <{RDF_FIRST}> ?zj . \
                        FILTER(?ci != ?cj) \
                        FILTER(sameTerm(?zi, ?zj) || EXISTS {{ ?zi <{OWL_SAME_AS}> ?zj }}) }}"
            );
            if self.ask(&q)? {
                return Err(ReasoningError::inconsistency(
                    rule,
                    "two members of an owl:AllDifferent are owl:sameAs",
                ));
            }
        }
        Ok(())
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // Table 5 — Semantics of Property Axioms
    // ═══════════════════════════════════════════════════════════════════════════

    /// prp-dom: ?p rdfs:domain ?c . ?x ?p ?y → ?x rdf:type ?c
    fn rule_prp_dom(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?x <{RDF_TYPE}> ?c }} }}
               WHERE  {{ ?p <{RDFS_DOMAIN}> ?c . {xy} }}"#,
            tg = self.target_graph,
            xy = pe("?p", "?x", "?y"),
        );
        self.run_update(&q)?;
        Ok(())
    }

    /// prp-rng: ?p rdfs:range ?c . ?x ?p ?y → ?y rdf:type ?c
    fn rule_prp_rng(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?y <{RDF_TYPE}> ?c }} }}
               WHERE  {{ ?p <{RDFS_RANGE}> ?c . {xy} FILTER(isIRI(?y) || isBlank(?y)) }}"#,
            tg = self.target_graph,
            xy = pe("?p", "?x", "?y"),
        );
        self.run_update(&q)?;
        Ok(())
    }

    /// prp-fp: ?p FunctionalProperty . ?x ?p ?y1 . ?x ?p ?y2 → ?y1 owl:sameAs ?y2
    fn rule_prp_fp(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?y1 <{OWL_SAME_AS}> ?y2 }} }}
               WHERE  {{ ?p <{RDF_TYPE}> <{OWL_FUNCTIONAL_PROP}> .
                         {a} {b} FILTER(?y1 != ?y2) }}"#,
            tg = self.target_graph,
            a = pe("?p", "?x", "?y1"),
            b = pe("?p", "?x", "?y2"),
        );
        self.run_update(&q)?;
        Ok(())
    }

    /// prp-ifp: ?p InverseFunctionalProperty . ?x1 ?p ?y . ?x2 ?p ?y → ?x1 owl:sameAs ?x2
    fn rule_prp_ifp(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?x1 <{OWL_SAME_AS}> ?x2 }} }}
               WHERE  {{ ?p <{RDF_TYPE}> <{OWL_INV_FUNCTIONAL}> .
                         {a} {b} FILTER(?x1 != ?x2) }}"#,
            tg = self.target_graph,
            a = pe("?p", "?x1", "?y"),
            b = pe("?p", "?x2", "?y"),
        );
        self.run_update(&q)?;
        Ok(())
    }

    /// prp-irp: ?p IrreflexiveProperty . ?x ?p ?x → INCONSISTENCY
    fn rule_prp_irp(&self) -> Result<(), ReasoningError> {
        let q = format!(
            "ASK {{ ?p <{RDF_TYPE}> <{OWL_IRREFLEXIVE_PROP}> . {xx} }}",
            xx = pe("?p", "?x", "?x"),
        );
        if self.ask(&q)? {
            return Err(ReasoningError::inconsistency(
                "prp-irp",
                "IrreflexiveProperty has reflexive triple",
            ));
        }
        Ok(())
    }

    /// prp-symp: ?p SymmetricProperty . ?x ?p ?y → ?y ?p ?x
    fn rule_prp_symp(&self) -> Result<(), ReasoningError> {
        let (bind, head) = pe_head("?p", "?y", "?x");
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ {head} }} }}
               WHERE  {{ ?p <{RDF_TYPE}> <{OWL_SYMMETRIC_PROP}> . {xy} {bind} }}"#,
            tg = self.target_graph,
            xy = pe("?p", "?x", "?y"),
        );
        self.run_update(&q)?;
        Ok(())
    }

    /// prp-asyp: ?p AsymmetricProperty . ?x ?p ?y . ?y ?p ?x → INCONSISTENCY
    fn rule_prp_asyp(&self) -> Result<(), ReasoningError> {
        let q = format!(
            "ASK {{ ?p <{RDF_TYPE}> <{OWL_ASYMMETRIC_PROP}> . {xy} {yx} }}",
            xy = pe("?p", "?x", "?y"),
            yx = pe("?p", "?y", "?x"),
        );
        if self.ask(&q)? {
            return Err(ReasoningError::inconsistency(
                "prp-asyp",
                "AsymmetricProperty violation",
            ));
        }
        Ok(())
    }

    /// prp-trp: ?p TransitiveProperty . ?x ?p ?y . ?y ?p ?z → ?x ?p ?z.
    /// A cycle derives the reflexive triple (`a p b . b p a → a p a`).
    fn rule_prp_trp(&self) -> Result<(), ReasoningError> {
        let (bind, head) = pe_head("?p", "?x", "?z");
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ {head} }} }}
               WHERE  {{ ?p <{RDF_TYPE}> <{OWL_TRANSITIVE_PROP}> .
                         {xy} {yz} {bind} }}"#,
            tg = self.target_graph,
            xy = pe("?p", "?x", "?y"),
            yz = pe("?p", "?y", "?z"),
        );
        self.run_update(&q)?;
        Ok(())
    }

    /// prp-spo1: ?p1 rdfs:subPropertyOf ?p2 . ?x ?p1 ?y → ?x ?p2 ?y.
    /// Either side may be an inverse property expression.
    fn rule_prp_spo1(&self) -> Result<(), ReasoningError> {
        let (bind, head) = pe_head("?p2", "?x", "?y");
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ {head} }} }}
               WHERE  {{ ?p1 <{RDFS_SUB_PROPERTY_OF}> ?p2 . FILTER(?p1 != ?p2)
                         {xy} {bind} }}"#,
            tg = self.target_graph,
            xy = pe("?p1", "?x", "?y"),
        );
        self.run_update(&q)?;
        Ok(())
    }

    /// prp-eqp1 / prp-eqp2: ?p1 owl:equivalentProperty ?p2 . ?x ?p1 ?y →
    /// ?x ?p2 ?y, and ?x ?p2 ?y → ?x ?p1 ?y.
    fn rule_prp_eqp(&self) -> Result<(), ReasoningError> {
        for (from, to) in [("?p1", "?p2"), ("?p2", "?p1")] {
            let (bind, head) = pe_head(to, "?x", "?y");
            let q = format!(
                r#"INSERT {{ GRAPH <{tg}> {{ {head} }} }}
                   WHERE  {{ ?p1 <{OWL_EQUIV_PROP}> ?p2 . FILTER(?p1 != ?p2)
                             {xy} {bind} }}"#,
                tg = self.target_graph,
                xy = pe(from, "?x", "?y"),
            );
            self.run_update(&q)?;
        }
        Ok(())
    }

    /// prp-pdw: ?p1 owl:propertyDisjointWith ?p2 . ?x ?p1 ?y . ?x ?p2 ?y → INCONSISTENCY
    fn rule_prp_pdw(&self) -> Result<(), ReasoningError> {
        let q = format!(
            "ASK {{ ?p1 <{OWL_PROP_DISJOINT_WITH}> ?p2 . {a} {b} }}",
            a = pe("?p1", "?x", "?y"),
            b = pe("?p2", "?x", "?y"),
        );
        if self.ask(&q)? {
            return Err(ReasoningError::inconsistency(
                "prp-pdw",
                "two disjoint properties (owl:propertyDisjointWith) link the same pair",
            ));
        }
        Ok(())
    }

    /// prp-adp: `?x a owl:AllDisjointProperties ; owl:members (?p1 … ?pn)`
    /// and `?u ?pi ?y . ?u ?pj ?y` for two different positions → INCONSISTENCY
    fn rule_prp_adp(&self) -> Result<(), ReasoningError> {
        let q = format!(
            "ASK {{ ?a <{RDF_TYPE}> <{OWL_ALL_DISJOINT_PROPS}> ; <{OWL_MEMBERS}> ?l . \
                    ?l <{RDF_REST}>* ?ci . ?ci <{RDF_FIRST}> ?pi . \
                    ?l <{RDF_REST}>* ?cj . ?cj <{RDF_FIRST}> ?pj . \
                    FILTER(?ci != ?cj) {a} {b} }}",
            a = pe("?pi", "?u", "?y"),
            b = pe("?pj", "?u", "?y"),
        );
        if self.ask(&q)? {
            return Err(ReasoningError::inconsistency(
                "prp-adp",
                "two members of an owl:AllDisjointProperties link the same pair",
            ));
        }
        Ok(())
    }

    /// prp-inv1: ?p1 owl:inverseOf ?p2 . ?x ?p1 ?y → ?y ?p2 ?x
    fn rule_prp_inv1(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?y ?p2 ?x }} }}
               WHERE  {{ ?p1 <{OWL_INVERSE_OF}> ?p2 . ?x ?p1 ?y }}"#,
            tg = self.target_graph
        );
        self.run_update(&q)?;
        Ok(())
    }

    /// prp-inv2: ?p1 owl:inverseOf ?p2 . ?x ?p2 ?y → ?y ?p1 ?x
    fn rule_prp_inv2(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?y ?p1 ?x }} }}
               WHERE  {{ ?p1 <{OWL_INVERSE_OF}> ?p2 . ?x ?p2 ?y }}"#,
            tg = self.target_graph
        );
        self.run_update(&q)?;
        Ok(())
    }

    /// prp-spo2: `?p owl:propertyChainAxiom (?p1 … ?pn) . ?u0 ?p1 ?u1 … ?u(n-1)
    /// ?pn ?un → ?u0 ?p ?un`, one INSERT per chain length in scope. Each link
    /// may be an inverse property expression.
    fn rule_prp_spo2(&self) -> Result<(), ReasoningError> {
        for n in self.list_lengths(OWL_PROP_CHAIN_AXIOM)? {
            let mut links = String::new();
            for i in 0..n {
                links.push_str(&pe(
                    &format!("?m{i}"),
                    &format!("?u{i}"),
                    &format!("?u{}", i + 1),
                ));
                links.push(' ');
            }
            let (bind, head) = pe_head("?p", "?u0", &format!("?u{n}"));
            let q = format!(
                r#"INSERT {{ GRAPH <{tg}> {{ {head} }} }}
                   WHERE {{ ?p <{OWL_PROP_CHAIN_AXIOM}> ?l . {list} {links} {bind} }}"#,
                tg = self.target_graph,
                list = list_n("?l", "m", n),
            );
            self.run_update(&q)?;
        }
        Ok(())
    }

    /// The distinct lengths (1 ..= [`MAX_LIST_LEN`]) of the lists that are
    /// objects of `predicate` in scope, for the rules that match a list at its
    /// exact length.
    fn list_lengths(&self, predicate: &str) -> Result<Vec<usize>, ReasoningError> {
        let q = format!(
            "SELECT ?l (COUNT(DISTINCT ?cell) AS ?n) \
             WHERE {{ ?s <{predicate}> ?l . ?l <{RDF_REST}>* ?cell . ?cell <{RDF_FIRST}> ?f }} \
             GROUP BY ?l"
        );
        let mut lengths = Vec::new();
        if let oxigraph::sparql::QueryResults::Solutions(rows) = self.run_query(&q)? {
            for row in rows {
                let row = row.map_err(|e| ReasoningError::Query(e.to_string()))?;
                if let Some(oxigraph::model::Term::Literal(n)) = row.get("n") {
                    if let Ok(n) = n.value().parse::<usize>() {
                        if (1..=MAX_LIST_LEN).contains(&n) {
                            lengths.push(n);
                        }
                    }
                }
            }
        }
        lengths.sort_unstable();
        lengths.dedup();
        Ok(lengths)
    }

    /// prp-key: `?c owl:hasKey (?p1 … ?pn) . ?x a ?c . ?y a ?c . ?x ?pi ?zi .
    /// ?y ?pi ?zi` for every key property → `?x owl:sameAs ?y`, one INSERT per
    /// key length in scope. The class may be a blank-node expression and a
    /// key property an inverse property expression; every member is a
    /// condition, so a key is never weakened by a member it cannot read. As
    /// before, only named individuals are merged (a key applies to named
    /// individuals, OWL 2 Structural Specification §9.5).
    fn rule_prp_key(&self) -> Result<(), ReasoningError> {
        for n in self.list_lengths(OWL_HAS_KEY)? {
            let mut keys = String::new();
            for i in 0..n {
                let (m, z) = (format!("?m{i}"), format!("?z{i}"));
                keys.push_str(&pe(&m, "?x", &z));
                keys.push(' ');
                keys.push_str(&pe(&m, "?y", &z));
                keys.push(' ');
            }
            let q = format!(
                r#"INSERT {{ GRAPH <{tg}> {{ ?x <{OWL_SAME_AS}> ?y }} }}
                   WHERE {{
                       ?c <{OWL_HAS_KEY}> ?l . {list}
                       ?x <{RDF_TYPE}> ?c .
                       {keys}
                       ?y <{RDF_TYPE}> ?c .
                       FILTER(?x != ?y) FILTER(isIRI(?x)) FILTER(isIRI(?y))
                   }}"#,
                tg = self.target_graph,
                list = list_n("?l", "m", n),
            );
            self.run_update(&q)?;
        }
        Ok(())
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // Table 8 — Semantics of Datatypes
    // ═══════════════════════════════════════════════════════════════════════════

    /// dt-type1: every datatype of the OWL 2 RL datatype map is an `rdfs:Datatype`.
    fn rule_dt_type1(&self) -> Result<(), ReasoningError> {
        let triples: String = RL_DATATYPES
            .iter()
            .map(|dt| format!("<{dt}> <{RDF_TYPE}> <{RDFS_DATATYPE}> . "))
            .collect();
        let q = format!(
            "INSERT DATA {{ GRAPH <{tg}> {{ {triples} }} }}",
            tg = self.target_graph
        );
        self.run_update(&q)?;
        Ok(())
    }

    /// dt-not-type: a literal whose lexical form is not in the lexical space
    /// of its datatype (`"abc"^^xsd:integer`) makes the ontology inconsistent.
    /// Every XSD-typed literal in scope is checked with the same lexical rules
    /// SHACL's `sh:datatype` uses; oxigraph keeps an ill-formed typed literal
    /// as its lexical form plus datatype, so it is found here. The scan reads
    /// the quad index directly (the scoped graphs, or the default and target graphs
    /// when unscoped) rather than a SPARQL query: a DISTINCT-over-FILTER query
    /// would take the sharded mirror path, and this check must not depend on
    /// it.
    fn rule_dt_not_type(&self) -> Result<(), ReasoningError> {
        use oxigraph::model::{GraphNameRef, NamedNodeRef, Term};
        let graphs: Vec<Option<String>> = match self.scope() {
            Some(scope) => scope.into_iter().map(Some).collect(),
            None => vec![None, Some(self.target_graph.clone())],
        };
        for graph in graphs {
            let graph_ref = match &graph {
                Some(g) => match NamedNodeRef::new(g) {
                    Ok(nn) => GraphNameRef::NamedNode(nn),
                    Err(_) => continue,
                },
                None => GraphNameRef::DefaultGraph,
            };
            for quad in self
                .store
                .store()
                .quads_for_pattern(None, None, None, Some(graph_ref))
            {
                let quad = quad.map_err(|e| ReasoningError::Store(e.to_string()))?;
                if let Term::Literal(lit) = &quad.object {
                    if lit
                        .datatype()
                        .as_str()
                        .starts_with("http://www.w3.org/2001/XMLSchema#")
                        && !crate::shacl::constraints::xsd_lexical_valid(lit)
                    {
                        return Err(ReasoningError::inconsistency(
                            "dt-not-type",
                            format!("{lit} is not in the lexical space of its datatype"),
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    /// prp-npa1: `?x owl:sourceIndividual ?i1 ; owl:assertionProperty ?p ;
    /// owl:targetIndividual ?i2 . ?i1 ?p ?i2` → INCONSISTENCY. The rule has no
    /// `rdf:type owl:NegativePropertyAssertion` premise.
    fn rule_prp_npa1(&self) -> Result<(), ReasoningError> {
        let q = format!(
            "ASK {{ \
               ?npa <{OWL_SOURCE_INDIVIDUAL}> ?s . \
               ?npa <{OWL_ASSERTION_PROPERTY}> ?p . \
               ?npa <{OWL_TARGET_INDIVIDUAL}> ?o . \
               {so} \
             }}",
            so = pe("?p", "?s", "?o"),
        );
        if self.ask(&q)? {
            return Err(ReasoningError::inconsistency(
                "prp-npa1",
                "NegativeObjectPropertyAssertion violated",
            ));
        }
        Ok(())
    }

    /// prp-npa2: `?x owl:sourceIndividual ?i ; owl:assertionProperty ?p ;
    /// owl:targetValue ?lt . ?i ?p ?lt` → INCONSISTENCY
    fn rule_prp_npa2(&self) -> Result<(), ReasoningError> {
        let q = format!(
            "ASK {{ \
               ?npa <{OWL_SOURCE_INDIVIDUAL}> ?s . \
               ?npa <{OWL_ASSERTION_PROPERTY}> ?p . \
               ?npa <{OWL_TARGET_VALUE}> ?v . \
               ?s ?p ?v . \
             }}"
        );
        if self.ask(&q)? {
            return Err(ReasoningError::inconsistency(
                "prp-npa2",
                "NegativeDataPropertyAssertion violated",
            ));
        }
        Ok(())
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // Table 6 — Semantics of Classes
    // ═══════════════════════════════════════════════════════════════════════════

    /// cls-nothing2: ?x rdf:type owl:Nothing → INCONSISTENCY
    fn rule_cls_nothing2(&self) -> Result<(), ReasoningError> {
        let q = format!("ASK {{ ?x <{RDF_TYPE}> <{OWL_NOTHING}> }}");
        if self.ask(&q)? {
            return Err(ReasoningError::inconsistency(
                "cls-nothing2",
                "An individual is an instance of owl:Nothing",
            ));
        }
        Ok(())
    }

    /// cls-int1: `?c owl:intersectionOf (?c1 … ?cn) . ?x a ?ci` for every
    /// member → `?x a ?c`, one INSERT per intersection length in scope.
    fn rule_cls_int1(&self) -> Result<(), ReasoningError> {
        for n in self.list_lengths(OWL_INTERSECTION_OF)? {
            let members: String = (0..n)
                .map(|i| format!("?x <{RDF_TYPE}> ?m{i} . "))
                .collect();
            let q = format!(
                r#"INSERT {{ GRAPH <{tg}> {{ ?x <{RDF_TYPE}> ?c }} }}
                   WHERE {{ ?c <{OWL_INTERSECTION_OF}> ?l . {list} {members} }}"#,
                tg = self.target_graph,
                list = list_n("?l", "m", n),
            );
            self.run_update(&q)?;
        }
        Ok(())
    }

    /// cls-int2: ?c owl:intersectionOf list(c1..cn) . ?x type ?c → ?x type ci for each i
    fn rule_cls_int2(&self) -> Result<(), ReasoningError> {
        // Head element
        let q1 = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?x <{RDF_TYPE}> ?c1 }} }}
               WHERE {{
                   ?c <{OWL_INTERSECTION_OF}> ?list .
                   ?list <{RDF_FIRST}> ?c1 .
                   ?x <{RDF_TYPE}> ?c .
               }}"#,
            tg = self.target_graph
        );
        self.run_update(&q1)?;
        // Rest element(s)
        let q2 = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?x <{RDF_TYPE}> ?cm }} }}
               WHERE {{
                   ?c <{OWL_INTERSECTION_OF}> ?list .
                   ?list <{RDF_REST}>+ ?rest .
                   ?rest <{RDF_FIRST}> ?cm .
                   FILTER(?cm != <{RDF_NIL}>) .
                   ?x <{RDF_TYPE}> ?c .
               }}"#,
            tg = self.target_graph
        );
        self.run_update(&q2)?;
        Ok(())
    }

    /// cls-uni: ?c owl:unionOf list(c1..cn) . ?x type ci → ?x type ?c
    fn rule_cls_uni(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?x <{RDF_TYPE}> ?c }} }}
               WHERE {{
                   ?c <{OWL_UNION_OF}> ?list .
                   ?list (<{RDF_FIRST}>|(<{RDF_REST}>+ /<{RDF_FIRST}>)) ?ci .
                   FILTER(?ci != <{RDF_NIL}>) .
                   ?x <{RDF_TYPE}> ?ci .
               }}"#,
            tg = self.target_graph
        );
        self.run_update(&q)?;
        Ok(())
    }

    /// cls-svf1: ?x owl:someValuesFrom ?y . ?x owl:onProperty ?p .
    ///           ?u ?p ?v . ?v rdf:type ?y → ?u rdf:type ?x
    fn rule_cls_svf1(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?u <{RDF_TYPE}> ?x }} }}
               WHERE  {{ ?x <{OWL_SOME_VALUES_FROM}> ?y . ?x <{OWL_ON_PROPERTY}> ?p .
                         {uv} ?v <{RDF_TYPE}> ?y }}"#,
            tg = self.target_graph,
            uv = pe("?p", "?u", "?v"),
        );
        self.run_update(&q)?;
        Ok(())
    }

    /// cls-svf2: ?x owl:someValuesFrom owl:Thing . ?x owl:onProperty ?p . ?u ?p ?v → ?u rdf:type ?x
    fn rule_cls_svf2(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?u <{RDF_TYPE}> ?x }} }}
               WHERE  {{ ?x <{OWL_SOME_VALUES_FROM}> <{OWL_THING}> .
                         ?x <{OWL_ON_PROPERTY}> ?p . {uv} }}"#,
            tg = self.target_graph,
            uv = pe("?p", "?u", "?v"),
        );
        self.run_update(&q)?;
        Ok(())
    }

    /// cls-avf: ?x owl:allValuesFrom ?y . ?x owl:onProperty ?p .
    ///          ?u rdf:type ?x . ?u ?p ?v → ?v rdf:type ?y
    fn rule_cls_avf(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?v <{RDF_TYPE}> ?y }} }}
               WHERE  {{ ?x <{OWL_ALL_VALUES_FROM}> ?y . ?x <{OWL_ON_PROPERTY}> ?p .
                         ?u <{RDF_TYPE}> ?x . {uv}
                         FILTER(isIRI(?v) || isBlank(?v)) }}"#,
            tg = self.target_graph,
            uv = pe("?p", "?u", "?v"),
        );
        self.run_update(&q)?;
        Ok(())
    }

    /// cls-hv1: ?x owl:hasValue ?y . ?x owl:onProperty ?p . ?u rdf:type ?x → ?u ?p ?y
    fn rule_cls_hv1(&self) -> Result<(), ReasoningError> {
        let (bind, head) = pe_head("?p", "?u", "?y");
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ {head} }} }}
               WHERE  {{ ?x <{OWL_HAS_VALUE}> ?y . ?x <{OWL_ON_PROPERTY}> ?p .
                         ?u <{RDF_TYPE}> ?x . {bind} }}"#,
            tg = self.target_graph
        );
        self.run_update(&q)?;
        Ok(())
    }

    /// cls-hv2: ?x owl:hasValue ?y . ?x owl:onProperty ?p . ?u ?p ?y → ?u rdf:type ?x
    fn rule_cls_hv2(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?u <{RDF_TYPE}> ?x }} }}
               WHERE  {{ ?x <{OWL_HAS_VALUE}> ?y . ?x <{OWL_ON_PROPERTY}> ?p . {uy} }}"#,
            tg = self.target_graph,
            uy = pe("?p", "?u", "?y"),
        );
        self.run_update(&q)?;
        Ok(())
    }

    /// cls-maxc1: maxCardinality "0"^^xsd:nonNegativeInteger . type ?x . ?u ?p ?y → INCONSISTENCY
    fn rule_cls_maxc1(&self) -> Result<(), ReasoningError> {
        // Handle both xsd:nonNegativeInteger (OWL standard) and xsd:integer (common Turtle)
        let q = format!(
            r#"ASK {{
                {{ ?x <{OWL_MAX_CARDINALITY}> "0"^^<http://www.w3.org/2001/XMLSchema#nonNegativeInteger> }}
                UNION
                {{ ?x <{OWL_MAX_CARDINALITY}> "0"^^<http://www.w3.org/2001/XMLSchema#integer> }}
                ?x <{OWL_ON_PROPERTY}> ?p .
                ?u <{RDF_TYPE}> ?x .
                {uy}
            }}"#,
            uy = pe("?p", "?u", "?y"),
        );
        if self.ask(&q)? {
            return Err(ReasoningError::inconsistency(
                "cls-maxc1",
                "owl:maxCardinality 0 violated",
            ));
        }
        Ok(())
    }

    /// cls-maxc2: maxCardinality "1" . type ?x . ?u ?p ?y1 . ?u ?p ?y2 → y1 owl:sameAs y2
    fn rule_cls_maxc2(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?y1 <{OWL_SAME_AS}> ?y2 }} }}
               WHERE {{
                   ?x <{OWL_MAX_CARDINALITY}> "1"^^<http://www.w3.org/2001/XMLSchema#nonNegativeInteger> .
                   ?x <{OWL_ON_PROPERTY}> ?p .
                   ?u <{RDF_TYPE}> ?x . {a} {b}
                   FILTER(?y1 != ?y2)
               }}"#,
            tg = self.target_graph,
            a = pe("?p", "?u", "?y1"),
            b = pe("?p", "?u", "?y2"),
        );
        self.run_update(&q)?;
        Ok(())
    }

    /// cls-maxqc1: maxQualifiedCardinality 0 + onClass + type + qualified property → INCONSISTENCY
    fn rule_cls_maxqc1(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"ASK {{
                ?x <{OWL_MAX_QUAL_CARD}> "0"^^<http://www.w3.org/2001/XMLSchema#nonNegativeInteger> .
                ?x <{OWL_ON_PROPERTY}> ?p .
                ?x <{OWL_ON_CLASS}> ?c .
                ?u <{RDF_TYPE}> ?x . {uy}
                ?y <{RDF_TYPE}> ?c .
            }}"#,
            uy = pe("?p", "?u", "?y"),
        );
        if self.ask(&q)? {
            return Err(ReasoningError::inconsistency(
                "cls-maxqc1",
                "owl:maxQualifiedCardinality 0 violated",
            ));
        }
        Ok(())
    }

    /// cls-maxqc2: maxQualifiedCardinality 0 + onClass owl:Thing → INCONSISTENCY (same as maxc1)
    fn rule_cls_maxqc2(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"ASK {{
                ?x <{OWL_MAX_QUAL_CARD}> "0"^^<http://www.w3.org/2001/XMLSchema#nonNegativeInteger> .
                ?x <{OWL_ON_PROPERTY}> ?p .
                ?x <{OWL_ON_CLASS}> <{OWL_THING}> .
                ?u <{RDF_TYPE}> ?x . {uy}
            }}"#,
            uy = pe("?p", "?u", "?y"),
        );
        if self.ask(&q)? {
            return Err(ReasoningError::inconsistency(
                "cls-maxqc2",
                "owl:maxQualifiedCardinality 0 (Thing) violated",
            ));
        }
        Ok(())
    }

    /// cls-maxqc3: maxQualifiedCardinality 1 + onClass + two qualified fillers → sameAs
    fn rule_cls_maxqc3(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?y1 <{OWL_SAME_AS}> ?y2 }} }}
               WHERE {{
                   ?x <{OWL_MAX_QUAL_CARD}> "1"^^<http://www.w3.org/2001/XMLSchema#nonNegativeInteger> .
                   ?x <{OWL_ON_PROPERTY}> ?p .
                   ?x <{OWL_ON_CLASS}> ?c .
                   ?u <{RDF_TYPE}> ?x .
                   {a} ?y1 <{RDF_TYPE}> ?c .
                   {b} ?y2 <{RDF_TYPE}> ?c .
                   FILTER(?y1 != ?y2)
               }}"#,
            tg = self.target_graph,
            a = pe("?p", "?u", "?y1"),
            b = pe("?p", "?u", "?y2"),
        );
        self.run_update(&q)?;
        Ok(())
    }

    /// cls-maxqc4: maxQualifiedCardinality 1 + onClass owl:Thing + two fillers → sameAs
    fn rule_cls_maxqc4(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?y1 <{OWL_SAME_AS}> ?y2 }} }}
               WHERE {{
                   ?x <{OWL_MAX_QUAL_CARD}> "1"^^<http://www.w3.org/2001/XMLSchema#nonNegativeInteger> .
                   ?x <{OWL_ON_PROPERTY}> ?p .
                   ?x <{OWL_ON_CLASS}> <{OWL_THING}> .
                   ?u <{RDF_TYPE}> ?x .
                   {a} {b}
                   FILTER(?y1 != ?y2)
               }}"#,
            tg = self.target_graph,
            a = pe("?p", "?u", "?y1"),
            b = pe("?p", "?u", "?y2"),
        );
        self.run_update(&q)?;
        Ok(())
    }

    /// cls-com: ?c1 owl:complementOf ?c2 . ?x type ?c1 . ?x type ?c2 → INCONSISTENCY
    fn rule_cls_com(&self) -> Result<(), ReasoningError> {
        let q = format!(
            "ASK {{ ?c1 <{OWL_COMPLEMENT_OF}> ?c2 . ?x <{RDF_TYPE}> ?c1 . ?x <{RDF_TYPE}> ?c2 }}"
        );
        if self.ask(&q)? {
            return Err(ReasoningError::inconsistency(
                "cls-com",
                "owl:complementOf violated: individual is member of both classes",
            ));
        }
        Ok(())
    }

    /// cls-oo: ?c owl:oneOf list(x1..xn) → xi rdf:type ?c for each xi
    fn rule_cls_oo(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?xi <{RDF_TYPE}> ?c }} }}
               WHERE {{
                   ?c <{OWL_ONE_OF}> ?list .
                   ?list (<{RDF_FIRST}>|(<{RDF_REST}>+ /<{RDF_FIRST}>)) ?xi .
                   FILTER(?xi != <{RDF_NIL}>)
               }}"#,
            tg = self.target_graph
        );
        self.run_update(&q)?;
        Ok(())
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // Table 7 — Semantics of Class Axioms
    // ═══════════════════════════════════════════════════════════════════════════

    /// cax-sco: ?c1 rdfs:subClassOf ?c2 . ?x type ?c1 → ?x type ?c2
    fn rule_cax_sco(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?x <{RDF_TYPE}> ?c2 }} }}
               WHERE  {{
                   ?c1 <{RDFS_SUB_CLASS_OF}> ?c2 .
                   {{ ?x <{RDF_TYPE}> ?c1 }} UNION {{ GRAPH <{tg}> {{ ?x <{RDF_TYPE}> ?c1 }} }}
                   FILTER(?c1 != ?c2)
               }}"#,
            tg = self.target_graph
        );
        self.run_update(&q)?;
        Ok(())
    }

    /// cax-eqc1: ?c1 owl:equivalentClass ?c2 . ?x type ?c1 → ?x type ?c2
    fn rule_cax_eqc1(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?x <{RDF_TYPE}> ?c2 }} }}
               WHERE  {{ ?c1 <{OWL_EQUIV_CLASS}> ?c2 . ?x <{RDF_TYPE}> ?c1 }}"#,
            tg = self.target_graph
        );
        self.run_update(&q)?;
        Ok(())
    }

    /// cax-eqc2: ?c1 owl:equivalentClass ?c2 . ?x type ?c2 → ?x type ?c1
    fn rule_cax_eqc2(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?x <{RDF_TYPE}> ?c1 }} }}
               WHERE  {{ ?c1 <{OWL_EQUIV_CLASS}> ?c2 . ?x <{RDF_TYPE}> ?c2 }}"#,
            tg = self.target_graph
        );
        self.run_update(&q)?;
        Ok(())
    }

    /// cax-adc: `?x a owl:AllDisjointClasses ; owl:members (?c1 … ?cn)` →
    /// `?ci owl:disjointWith ?cj` for every two positions. Blank-node class
    /// expressions are members too.
    fn rule_cax_adc(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?c1 <{OWL_DISJOINT_WITH}> ?c2 }} }}
               WHERE {{
                   ?adc <{RDF_TYPE}> <{OWL_ALL_DISJOINT}> .
                   ?adc <{OWL_MEMBERS}> ?l .
                   ?l <{RDF_REST}>* ?ci . ?ci <{RDF_FIRST}> ?c1 .
                   ?l <{RDF_REST}>* ?cj . ?cj <{RDF_FIRST}> ?c2 .
                   FILTER(?ci != ?cj)
               }}"#,
            tg = self.target_graph
        );
        self.run_update(&q)?;
        Ok(())
    }

    /// cax-dw: ?c1 owl:disjointWith ?c2 . ?x type ?c1 . ?x type ?c2 → INCONSISTENCY
    fn rule_cax_dw(&self) -> Result<(), ReasoningError> {
        let q = format!(
            "ASK {{ ?c1 <{OWL_DISJOINT_WITH}> ?c2 . ?x <{RDF_TYPE}> ?c1 . ?x <{RDF_TYPE}> ?c2 }}"
        );
        if self.ask(&q)? {
            return Err(ReasoningError::inconsistency(
                "cax-dw",
                "owl:disjointWith violated",
            ));
        }
        Ok(())
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // Table 9 — Semantics of the Schema Vocabulary
    // ═══════════════════════════════════════════════════════════════════════════

    /// scm-sco: ?c1 rdfs:subClassOf ?c2 . ?c2 rdfs:subClassOf ?c3 → ?c1 rdfs:subClassOf ?c3
    fn rule_scm_sco(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?c1 <{RDFS_SUB_CLASS_OF}> ?c3 }} }}
               WHERE  {{ ?c1 <{RDFS_SUB_CLASS_OF}> ?c2 .
                         ?c2 <{RDFS_SUB_CLASS_OF}> ?c3 }}"#,
            tg = self.target_graph
        );
        self.run_update(&q)?;
        Ok(())
    }

    /// scm-eqc1: ?c1 owl:equivalentClass ?c2 → ?c1 rdfs:subClassOf ?c2 . ?c2 rdfs:subClassOf ?c1
    fn rule_scm_eqc1(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?c1 <{RDFS_SUB_CLASS_OF}> ?c2 . ?c2 <{RDFS_SUB_CLASS_OF}> ?c1 }} }}
               WHERE  {{ ?c1 <{OWL_EQUIV_CLASS}> ?c2 }}"#,
            tg = self.target_graph
        );
        self.run_update(&q)?;
        Ok(())
    }

    /// scm-eqc2: ?c1 rdfs:subClassOf ?c2 . ?c2 rdfs:subClassOf ?c1 → ?c1 owl:equivalentClass ?c2
    fn rule_scm_eqc2(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?c1 <{OWL_EQUIV_CLASS}> ?c2 }} }}
               WHERE  {{ ?c1 <{RDFS_SUB_CLASS_OF}> ?c2 .
                         ?c2 <{RDFS_SUB_CLASS_OF}> ?c1 }}"#,
            tg = self.target_graph
        );
        self.run_update(&q)?;
        Ok(())
    }

    /// scm-spo: ?p1 rdfs:subPropertyOf ?p2 . ?p2 rdfs:subPropertyOf ?p3 → ?p1 rdfs:subPropertyOf ?p3
    fn rule_scm_spo(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?p1 <{RDFS_SUB_PROPERTY_OF}> ?p3 }} }}
               WHERE  {{ ?p1 <{RDFS_SUB_PROPERTY_OF}> ?p2 .
                         ?p2 <{RDFS_SUB_PROPERTY_OF}> ?p3 }}"#,
            tg = self.target_graph
        );
        self.run_update(&q)?;
        Ok(())
    }

    /// scm-eqp1: ?p1 owl:equivalentProperty ?p2 → ?p1 rdfs:subPropertyOf ?p2 . ?p2 rdfs:subPropertyOf ?p1
    fn rule_scm_eqp1(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?p1 <{RDFS_SUB_PROPERTY_OF}> ?p2 . ?p2 <{RDFS_SUB_PROPERTY_OF}> ?p1 }} }}
               WHERE  {{ ?p1 <{OWL_EQUIV_PROP}> ?p2 }}"#,
            tg = self.target_graph
        );
        self.run_update(&q)?;
        Ok(())
    }

    /// scm-eqp2: ?p1 subPropertyOf ?p2 . ?p2 subPropertyOf ?p1 → equivalentProperty
    fn rule_scm_eqp2(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?p1 <{OWL_EQUIV_PROP}> ?p2 }} }}
               WHERE  {{ ?p1 <{RDFS_SUB_PROPERTY_OF}> ?p2 .
                         ?p2 <{RDFS_SUB_PROPERTY_OF}> ?p1 }}"#,
            tg = self.target_graph
        );
        self.run_update(&q)?;
        Ok(())
    }

    /// scm-dom1: ?p rdfs:domain ?c1 . ?c1 rdfs:subClassOf ?c2 → ?p rdfs:domain ?c2
    fn rule_scm_dom1(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?p <{RDFS_DOMAIN}> ?c2 }} }}
               WHERE  {{ ?p <{RDFS_DOMAIN}> ?c1 . ?c1 <{RDFS_SUB_CLASS_OF}> ?c2 }}"#,
            tg = self.target_graph
        );
        self.run_update(&q)?;
        Ok(())
    }

    /// scm-dom2: ?p2 rdfs:domain ?c . ?p1 rdfs:subPropertyOf ?p2 → ?p1 rdfs:domain ?c
    fn rule_scm_dom2(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?p1 <{RDFS_DOMAIN}> ?c }} }}
               WHERE  {{ ?p2 <{RDFS_DOMAIN}> ?c . ?p1 <{RDFS_SUB_PROPERTY_OF}> ?p2 }}"#,
            tg = self.target_graph
        );
        self.run_update(&q)?;
        Ok(())
    }

    /// scm-rng1: ?p rdfs:range ?c1 . ?c1 rdfs:subClassOf ?c2 → ?p rdfs:range ?c2
    fn rule_scm_rng1(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?p <{RDFS_RANGE}> ?c2 }} }}
               WHERE  {{ ?p <{RDFS_RANGE}> ?c1 . ?c1 <{RDFS_SUB_CLASS_OF}> ?c2 }}"#,
            tg = self.target_graph
        );
        self.run_update(&q)?;
        Ok(())
    }

    /// scm-rng2: ?p2 rdfs:range ?c . ?p1 rdfs:subPropertyOf ?p2 → ?p1 rdfs:range ?c
    fn rule_scm_rng2(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?p1 <{RDFS_RANGE}> ?c }} }}
               WHERE  {{ ?p2 <{RDFS_RANGE}> ?c . ?p1 <{RDFS_SUB_PROPERTY_OF}> ?p2 }}"#,
            tg = self.target_graph
        );
        self.run_update(&q)?;
        Ok(())
    }

    /// scm-hv: c1 hasValue i / onProperty p1, c2 hasValue i / onProperty p2 .
    ///         p1 subPropertyOf p2 → c1 subClassOf c2
    fn rule_scm_hv(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?c1 <{RDFS_SUB_CLASS_OF}> ?c2 }} }}
               WHERE  {{ ?c1 <{OWL_HAS_VALUE}> ?i . ?c1 <{OWL_ON_PROPERTY}> ?p1 .
                         ?c2 <{OWL_HAS_VALUE}> ?i . ?c2 <{OWL_ON_PROPERTY}> ?p2 .
                         ?p1 <{RDFS_SUB_PROPERTY_OF}> ?p2 . }}"#,
            tg = self.target_graph
        );
        self.run_update(&q)?;
        Ok(())
    }

    /// scm-svf1: c1 someValuesFrom y1 / onProperty p, c2 someValuesFrom y2 / onProperty p .
    ///           y1 subClassOf y2 → c1 subClassOf c2
    fn rule_scm_svf1(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?c1 <{RDFS_SUB_CLASS_OF}> ?c2 }} }}
               WHERE  {{ ?c1 <{OWL_SOME_VALUES_FROM}> ?y1 . ?c1 <{OWL_ON_PROPERTY}> ?p .
                         ?c2 <{OWL_SOME_VALUES_FROM}> ?y2 . ?c2 <{OWL_ON_PROPERTY}> ?p .
                         ?y1 <{RDFS_SUB_CLASS_OF}> ?y2 . }}"#,
            tg = self.target_graph
        );
        self.run_update(&q)?;
        Ok(())
    }

    /// scm-svf2: c1 someValuesFrom y / onProperty p1, c2 someValuesFrom y / onProperty p2 .
    ///           p1 subPropertyOf p2 → c1 subClassOf c2
    fn rule_scm_svf2(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?c1 <{RDFS_SUB_CLASS_OF}> ?c2 }} }}
               WHERE  {{ ?c1 <{OWL_SOME_VALUES_FROM}> ?y . ?c1 <{OWL_ON_PROPERTY}> ?p1 .
                         ?c2 <{OWL_SOME_VALUES_FROM}> ?y . ?c2 <{OWL_ON_PROPERTY}> ?p2 .
                         ?p1 <{RDFS_SUB_PROPERTY_OF}> ?p2 . }}"#,
            tg = self.target_graph
        );
        self.run_update(&q)?;
        Ok(())
    }

    /// scm-avf1: c1 allValuesFrom y1, c2 allValuesFrom y2, same onProperty .
    ///           y1 subClassOf y2 → c1 subClassOf c2
    fn rule_scm_avf1(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?c1 <{RDFS_SUB_CLASS_OF}> ?c2 }} }}
               WHERE  {{ ?c1 <{OWL_ALL_VALUES_FROM}> ?y1 . ?c1 <{OWL_ON_PROPERTY}> ?p .
                         ?c2 <{OWL_ALL_VALUES_FROM}> ?y2 . ?c2 <{OWL_ON_PROPERTY}> ?p .
                         ?y1 <{RDFS_SUB_CLASS_OF}> ?y2 . }}"#,
            tg = self.target_graph
        );
        self.run_update(&q)?;
        Ok(())
    }

    /// scm-avf2: c1 allValuesFrom y, c2 allValuesFrom y .
    ///           p2 subPropertyOf p1 → c1 subClassOf c2
    fn rule_scm_avf2(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?c1 <{RDFS_SUB_CLASS_OF}> ?c2 }} }}
               WHERE  {{ ?c1 <{OWL_ALL_VALUES_FROM}> ?y . ?c1 <{OWL_ON_PROPERTY}> ?p1 .
                         ?c2 <{OWL_ALL_VALUES_FROM}> ?y . ?c2 <{OWL_ON_PROPERTY}> ?p2 .
                         ?p2 <{RDFS_SUB_PROPERTY_OF}> ?p1 . }}"#,
            tg = self.target_graph
        );
        self.run_update(&q)?;
        Ok(())
    }

    /// scm-cls: ?c a owl:Class → ?c rdfs:subClassOf ?c . ?c owl:equivalentClass ?c .
    /// ?c rdfs:subClassOf owl:Thing . owl:Nothing rdfs:subClassOf ?c
    fn rule_scm_cls(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{
                   ?c <{RDFS_SUB_CLASS_OF}> ?c .
                   ?c <{OWL_EQUIV_CLASS}> ?c .
                   ?c <{RDFS_SUB_CLASS_OF}> <{OWL_THING}> .
                   <{OWL_NOTHING}> <{RDFS_SUB_CLASS_OF}> ?c .
               }} }}
               WHERE {{ ?c <{RDF_TYPE}> <{OWL_CLASS}> }}"#,
            tg = self.target_graph
        );
        self.run_update(&q)?;
        Ok(())
    }

    /// scm-op: ?p a owl:ObjectProperty → ?p rdfs:subPropertyOf ?p . ?p owl:equivalentProperty ?p
    fn rule_scm_op(&self) -> Result<(), ReasoningError> {
        self.reflexive_property_axioms(OWL_OBJECT_PROPERTY)
    }

    /// scm-dp: ?p a owl:DatatypeProperty → ?p rdfs:subPropertyOf ?p . ?p owl:equivalentProperty ?p
    fn rule_scm_dp(&self) -> Result<(), ReasoningError> {
        self.reflexive_property_axioms(OWL_DATATYPE_PROPERTY)
    }

    fn reflexive_property_axioms(&self, kind: &str) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{
                   ?p <{RDFS_SUB_PROPERTY_OF}> ?p .
                   ?p <{OWL_EQUIV_PROP}> ?p .
               }} }}
               WHERE {{ ?p <{RDF_TYPE}> <{kind}> }}"#,
            tg = self.target_graph
        );
        self.run_update(&q)?;
        Ok(())
    }

    /// prp-ap, cls-thing, cls-nothing1: the axiomatic triples — the built-in
    /// annotation properties, and `owl:Thing` / `owl:Nothing` as classes.
    fn rule_axiomatic(&self) -> Result<(), ReasoningError> {
        let mut triples: String = ANNOTATION_PROPERTIES
            .iter()
            .map(|ap| format!("<{ap}> <{RDF_TYPE}> <{OWL_ANNOTATION_PROPERTY}> . "))
            .collect();
        triples.push_str(&format!(
            "<{OWL_THING}> <{RDF_TYPE}> <{OWL_CLASS}> . <{OWL_NOTHING}> <{RDF_TYPE}> <{OWL_CLASS}> . "
        ));
        let q = format!(
            "INSERT DATA {{ GRAPH <{tg}> {{ {triples} }} }}",
            tg = self.target_graph
        );
        self.run_update(&q)?;
        Ok(())
    }

    /// scm-int: `?c owl:intersectionOf (?c1 … ?cn)` → `?c rdfs:subClassOf ?ci`
    fn rule_scm_int(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?c <{RDFS_SUB_CLASS_OF}> ?ci }} }}
               WHERE {{
                   ?c <{OWL_INTERSECTION_OF}> ?list .
                   ?list <{RDF_REST}>*/<{RDF_FIRST}> ?ci .
               }}"#,
            tg = self.target_graph
        );
        self.run_update(&q)?;
        Ok(())
    }

    /// scm-uni: `?c owl:unionOf (?c1 … ?cn)` → `?ci rdfs:subClassOf ?c`
    fn rule_scm_uni(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?ci <{RDFS_SUB_CLASS_OF}> ?c }} }}
               WHERE {{
                   ?c <{OWL_UNION_OF}> ?list .
                   ?list <{RDF_REST}>*/<{RDF_FIRST}> ?ci .
               }}"#,
            tg = self.target_graph
        );
        self.run_update(&q)?;
        Ok(())
    }

    // ─── Helper ───────────────────────────────────────────────────────────────

    fn ask(&self, sparql: &str) -> Result<bool, ReasoningError> {
        match self.run_query(sparql)? {
            oxigraph::sparql::QueryResults::Boolean(b) => Ok(b),
            _ => Ok(false),
        }
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::TripleStore;
    use oxigraph::io::RdfFormat;

    fn store_with(ttl: &str) -> TripleStore {
        let s = TripleStore::in_memory().unwrap();
        let prefixes = "@prefix owl:  <http://www.w3.org/2002/07/owl#> .
                        @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
                        @prefix rdf:  <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
                        @prefix ex:   <http://example.org/> .\n";
        s.load_str(&format!("{}{}", prefixes, ttl), RdfFormat::Turtle, None)
            .unwrap();
        s
    }

    fn ask(store: &TripleStore, sparql: &str) -> bool {
        match store.query(sparql).unwrap() {
            oxigraph::sparql::QueryResults::Boolean(b) => b,
            _ => false,
        }
    }

    const TG: &str = OWL2_RL_ENTAILMENT_GRAPH;

    #[test]
    fn test_rl_same_as_sym() {
        let s = store_with("ex:a owl:sameAs ex:b .");
        Owl2RLReasoner::new(&s).materialize().unwrap();
        assert!(ask(
            &s,
            &format!("ASK {{ GRAPH <{TG}> {{ <http://example.org/b> <{OWL_SAME_AS}> <http://example.org/a> }} }}")
        ));
    }

    #[test]
    fn test_rl_same_as_trans() {
        let s = store_with("ex:a owl:sameAs ex:b . ex:b owl:sameAs ex:c .");
        Owl2RLReasoner::new(&s).materialize().unwrap();
        assert!(ask(
            &s,
            &format!("ASK {{ GRAPH <{TG}> {{ <http://example.org/a> <{OWL_SAME_AS}> <http://example.org/c> }} }}")
        ));
    }

    #[test]
    fn test_rl_transitive() {
        let s = store_with(
            "ex:anc rdf:type owl:TransitiveProperty .
             ex:a   ex:anc ex:b .
             ex:b   ex:anc ex:c .",
        );
        Owl2RLReasoner::new(&s).materialize().unwrap();
        assert!(ask(
            &s,
            &format!(
                "ASK {{ GRAPH <{TG}> {{ <http://example.org/a> \
                 <http://example.org/anc> <http://example.org/c> }} }}"
            )
        ));
    }

    #[test]
    fn test_rl_symmetric() {
        let s = store_with(
            "ex:friend rdf:type owl:SymmetricProperty .
             ex:a ex:friend ex:b .",
        );
        Owl2RLReasoner::new(&s).materialize().unwrap();
        assert!(ask(
            &s,
            &format!(
                "ASK {{ GRAPH <{TG}> {{ <http://example.org/b> \
                 <http://example.org/friend> <http://example.org/a> }} }}"
            )
        ));
    }

    #[test]
    fn test_rl_inverse() {
        let s = store_with("ex:parent owl:inverseOf ex:childOf . ex:a ex:parent ex:b .");
        Owl2RLReasoner::new(&s).materialize().unwrap();
        assert!(ask(
            &s,
            &format!(
                "ASK {{ GRAPH <{TG}> {{ <http://example.org/b> \
                 <http://example.org/childOf> <http://example.org/a> }} }}"
            )
        ));
    }

    #[test]
    fn test_rl_disjoint_inconsistency() {
        let s = store_with(
            "ex:Cat owl:disjointWith ex:Dog .
             ex:x   rdf:type ex:Cat .
             ex:x   rdf:type ex:Dog .",
        );
        assert!(Owl2RLReasoner::new(&s).materialize().is_err());
    }

    #[test]
    fn test_rl_property_chain() {
        let s = store_with(
            "ex:uncle owl:propertyChainAxiom ( ex:parent ex:brother ) .
             ex:alice ex:parent ex:bob .
             ex:bob   ex:brother ex:charlie .",
        );
        Owl2RLReasoner::new(&s).materialize().unwrap();
        assert!(ask(
            &s,
            &format!(
                "ASK {{ GRAPH <{TG}> {{ <http://example.org/alice> \
                 <http://example.org/uncle> <http://example.org/charlie> }} }}"
            )
        ));
    }

    #[test]
    fn test_rl_has_key() {
        let s = store_with(
            "ex:Person owl:hasKey ( ex:ssn ) .
             ex:a rdf:type ex:Person ; ex:ssn \"123\" .
             ex:b rdf:type ex:Person ; ex:ssn \"123\" .",
        );
        Owl2RLReasoner::new(&s).materialize().unwrap();
        assert!(ask(
            &s,
            &format!(
                "ASK {{ GRAPH <{TG}> {{ <http://example.org/a> \
                 <{OWL_SAME_AS}> <http://example.org/b> }} }}"
            )
        ));
    }

    #[test]
    fn test_rl_max_cardinality_1_same_as() {
        let s = store_with(
            "ex:R owl:maxCardinality \"1\"^^<http://www.w3.org/2001/XMLSchema#nonNegativeInteger> ;
                  owl:onProperty ex:spouse .
             ex:alice rdf:type ex:R ; ex:spouse ex:bob ; ex:spouse ex:robert .",
        );
        Owl2RLReasoner::new(&s).materialize().unwrap();
        assert!(ask(
            &s,
            &format!(
                "ASK {{ GRAPH <{TG}> {{ <http://example.org/bob> \
                 <{OWL_SAME_AS}> <http://example.org/robert> }} }}"
            )
        ));
    }

    #[test]
    fn test_rl_complement_of_inconsistency() {
        let s = store_with(
            "ex:Alive owl:complementOf ex:Dead .
             ex:x rdf:type ex:Alive .
             ex:x rdf:type ex:Dead .",
        );
        assert!(Owl2RLReasoner::new(&s).materialize().is_err());
    }

    #[test]
    fn test_rl_all_disjoint_classes() {
        let s = store_with(
            "_:adc rdf:type owl:AllDisjointClasses ;
                   owl:members ( ex:Cat ex:Dog ex:Fish ) .
             ex:x rdf:type ex:Cat .
             ex:x rdf:type ex:Dog .",
        );
        // Should produce pairwise disjointWith, then detect inconsistency
        assert!(Owl2RLReasoner::new(&s).materialize().is_err());
    }

    #[test]
    fn test_rl_scm_cls_subclass_thing() {
        let s = store_with("ex:Person rdf:type owl:Class .");
        Owl2RLReasoner::new(&s).materialize().unwrap();
        assert!(ask(
            &s,
            &format!(
                "ASK {{ GRAPH <{TG}> {{ <http://example.org/Person> \
                 <http://www.w3.org/2000/01/rdf-schema#subClassOf> \
                 <http://www.w3.org/2002/07/owl#Thing> }} }}"
            )
        ));
    }

    #[test]
    fn test_rl_scm_int_intersection_superclass() {
        let s = store_with("ex:AB owl:intersectionOf ( ex:A ex:B ) .");
        Owl2RLReasoner::new(&s).materialize().unwrap();
        // AB ⊑ A and AB ⊑ B
        assert!(ask(
            &s,
            &format!(
                "ASK {{ GRAPH <{TG}> {{ <http://example.org/AB> \
                 <http://www.w3.org/2000/01/rdf-schema#subClassOf> \
                 <http://example.org/A> }} }}"
            )
        ));
    }

    #[test]
    fn test_rl_scm_uni_union_subclass() {
        let s = store_with("ex:AorB owl:unionOf ( ex:A ex:B ) .");
        Owl2RLReasoner::new(&s).materialize().unwrap();
        // A ⊑ AorB
        assert!(ask(
            &s,
            &format!(
                "ASK {{ GRAPH <{TG}> {{ <http://example.org/A> \
                 <http://www.w3.org/2000/01/rdf-schema#subClassOf> \
                 <http://example.org/AorB> }} }}"
            )
        ));
    }
}
