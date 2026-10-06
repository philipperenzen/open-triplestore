//! RDFS entailment via forward-chaining materialization (RDF 1.1
//! Semantics §8–9).
//!
//! The entailment patterns `rdfD2` and `rdfs1`–`rdfs13` run as SPARQL INSERT
//! operations in one fixed-point loop, over the asserted data and the
//! derivations so far. Every inferred triple is written to the
//! `target_graph` named graph, leaving asserted data untouched.
//!
//! The RDF and RDFS axiomatic triples are written to the target graph first,
//! so they take part in the loop like asserted triples (for example
//! `rdfs:Resource rdfs:subClassOf ex:C` reaches every resource). Two parts of
//! the closure are infinite and are bounded by the data instead (decision
//! D11, documented in `docs/rdfs-entailment.md`):
//!
//! - the container-membership axioms are written for `rdf:_1` … `rdf:_n`,
//!   where `n` is the largest index the graphs in scope use;
//! - `rdfs1` declares the recognized datatypes ([`recognized_datatypes`])
//!   that literals in scope use, plus `xsd:string` and `rdf:langString`.
//!
//! `rdfD1` (a blank node standing for each typed literal's value) is opt-in
//! ([`RdfsMaterializer::with_rdfd1`]): for every well-typed literal of a
//! recognized datatype it writes one blank node per data value — named after
//! the value, so equal values written differently share it and a re-run
//! writes the same node — with `rdf:type` its datatypes, and copies each
//! triple with the literal as object onto that node. The patterns then run
//! over those nodes as over any resource (ranges, subclasses, `rdfs:Literal`).
//! It is off by default: it writes two triples per literal-valued triple, all
//! of them existential restatements of what the data says.
//!
//! The rest of D-entailment — that equal values written differently entail
//! each other — is matched at query time ([`super::value_match`]), as no
//! materialiser can write every lexical form of a value.
//!
//! Patterns whose conclusion has a literal subject (`rdfs3` / `rdfs4b` on a
//! literal object) are generalized triples and are not stored; what they
//! would make unsatisfiable is checked instead: after the fixed point an
//! ill-typed literal of a recognized datatype, or a literal outside a
//! recognized datatype its property's range gives it, ends the run in
//! `ReasoningError::Inconsistency` (the derived triples stay).

use std::time::Instant;
use tracing::{debug, info};

use super::common::{count_graph, ReasoningError, ReasoningReport};
use crate::store::TripleStore;

// ─── Namespace constants ──────────────────────────────────────────────────────

const RDF_NS: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
const RDFS_NS: &str = "http://www.w3.org/2000/01/rdf-schema#";
const XSD_NS: &str = "http://www.w3.org/2001/XMLSchema#";

/// Maximum fixed-point iterations (safety valve).
const MAX_ITERATIONS: usize = 500;

/// One entailment pattern: its name, the conclusion and the premises, in
/// SPARQL with the `rdf:` / `rdfs:` prefixes.
type Rule = (&'static str, &'static str, &'static str);

/// The patterns that join on schema triples; they run every round.
const SCHEMA_RULES: &[Rule] = &[
    ("rdfs2", "?x rdf:type ?c", "?p rdfs:domain ?c . ?x ?p ?y"),
    // A literal object would be a literal subject: not an RDF triple.
    (
        "rdfs3",
        "?y rdf:type ?c",
        "?p rdfs:range ?c . ?x ?p ?y . FILTER(!isLiteral(?y))",
    ),
    (
        "rdfs5",
        "?p rdfs:subPropertyOf ?r",
        "?p rdfs:subPropertyOf ?q . ?q rdfs:subPropertyOf ?r",
    ),
    (
        "rdfs6",
        "?p rdfs:subPropertyOf ?p",
        "?p rdf:type rdf:Property",
    ),
    // `?p = ?q` would only restate the premise.
    (
        "rdfs7",
        "?x ?q ?y",
        "?p rdfs:subPropertyOf ?q . ?x ?p ?y . FILTER(?p != ?q)",
    ),
    (
        "rdfs8",
        "?c rdfs:subClassOf rdfs:Resource",
        "?c rdf:type rdfs:Class",
    ),
    (
        "rdfs9",
        "?x rdf:type ?d",
        "?c rdfs:subClassOf ?d . ?x rdf:type ?c . FILTER(?c != ?d)",
    ),
    ("rdfs10", "?c rdfs:subClassOf ?c", "?c rdf:type rdfs:Class"),
    (
        "rdfs11",
        "?c rdfs:subClassOf ?e",
        "?c rdfs:subClassOf ?d . ?d rdfs:subClassOf ?e",
    ),
    (
        "rdfs12",
        "?p rdfs:subPropertyOf rdfs:member",
        "?p rdf:type rdfs:ContainerMembershipProperty",
    ),
    (
        "rdfs13",
        "?d rdfs:subClassOf rdfs:Literal",
        "?d rdf:type rdfs:Datatype",
    ),
];

/// The patterns with a single unrestricted premise: each scans every triple
/// in scope, so they run only when the schema rules have reached their fixed
/// point, and the loop goes on while they add anything.
const DATA_RULES: &[Rule] = &[
    (
        "rdfD2",
        "?p rdf:type rdf:Property",
        "{ SELECT DISTINCT ?p WHERE { ?s ?p ?o } }",
    ),
    (
        "rdfs4a",
        "?x rdf:type rdfs:Resource",
        "{ SELECT DISTINCT ?x WHERE { ?x ?p ?y } }",
    ),
    (
        "rdfs4b",
        "?y rdf:type rdfs:Resource",
        "{ SELECT DISTINCT ?y WHERE { ?x ?p ?y . FILTER(!isLiteral(?y)) } }",
    ),
];

/// `rdf:x` / `rdfs:x` to a full IRI.
fn expand(curie: &str) -> String {
    match curie.split_once(':') {
        Some(("rdf", local)) => format!("{RDF_NS}{local}"),
        Some(("rdfs", local)) => format!("{RDFS_NS}{local}"),
        _ => curie.to_string(),
    }
}

/// The vocabulary properties with their axiomatic domain and range
/// (RDF 1.1 Semantics §9.1), as `(property, domain, range)`.
const DOMAIN_RANGE: &[(&str, &str, &str)] = &[
    ("rdf:type", "rdfs:Resource", "rdfs:Class"),
    ("rdfs:domain", "rdf:Property", "rdfs:Class"),
    ("rdfs:range", "rdf:Property", "rdfs:Class"),
    ("rdfs:subPropertyOf", "rdf:Property", "rdf:Property"),
    ("rdfs:subClassOf", "rdfs:Class", "rdfs:Class"),
    ("rdf:subject", "rdf:Statement", "rdfs:Resource"),
    ("rdf:predicate", "rdf:Statement", "rdfs:Resource"),
    ("rdf:object", "rdf:Statement", "rdfs:Resource"),
    ("rdfs:member", "rdfs:Resource", "rdfs:Resource"),
    ("rdf:first", "rdf:List", "rdfs:Resource"),
    ("rdf:rest", "rdf:List", "rdf:List"),
    ("rdfs:seeAlso", "rdfs:Resource", "rdfs:Resource"),
    ("rdfs:isDefinedBy", "rdfs:Resource", "rdfs:Resource"),
    ("rdfs:comment", "rdfs:Resource", "rdfs:Literal"),
    ("rdfs:label", "rdfs:Resource", "rdfs:Literal"),
    ("rdf:value", "rdfs:Resource", "rdfs:Resource"),
];

/// The RDF and RDFS axiomatic triples (RDF 1.1 Semantics §8.1, §9.1), with
/// the container-membership properties `rdf:_1` … `rdf:_max_member` (D11).
pub(crate) fn axiomatic_triples(max_member: u32) -> Vec<(String, String, String)> {
    let t = |s: &str, p: &str, o: &str| (expand(s), expand(p), expand(o));
    let mut out = Vec::new();
    // RDF: the vocabulary properties, and rdf:nil.
    for local in [
        "type",
        "subject",
        "predicate",
        "object",
        "first",
        "rest",
        "value",
    ] {
        out.push(t(&format!("rdf:{local}"), "rdf:type", "rdf:Property"));
    }
    out.push(t("rdf:nil", "rdf:type", "rdf:List"));
    // RDFS: domains and ranges, then the class and property hierarchy.
    for (p, d, r) in DOMAIN_RANGE {
        out.push(t(p, "rdfs:domain", d));
        out.push(t(p, "rdfs:range", r));
    }
    for c in ["rdf:Alt", "rdf:Bag", "rdf:Seq"] {
        out.push(t(c, "rdfs:subClassOf", "rdfs:Container"));
    }
    out.push(t(
        "rdfs:ContainerMembershipProperty",
        "rdfs:subClassOf",
        "rdf:Property",
    ));
    out.push(t("rdfs:Datatype", "rdfs:subClassOf", "rdfs:Class"));
    out.push(t("rdfs:isDefinedBy", "rdfs:subPropertyOf", "rdfs:seeAlso"));
    for n in 1..=max_member {
        let m = format!("rdf:_{n}");
        out.push(t(&m, "rdf:type", "rdf:Property"));
        out.push(t(&m, "rdf:type", "rdfs:ContainerMembershipProperty"));
        out.push(t(&m, "rdfs:domain", "rdfs:Resource"));
        out.push(t(&m, "rdfs:range", "rdfs:Resource"));
    }
    out
}

/// The datatypes this store recognizes (the `D` of D-entailment): the
/// OWL 2 datatype map's types and `rdf:langString`. `rdfs:Literal` is a
/// class, not a datatype IRI, and is left out.
pub(crate) fn recognized_datatypes() -> Vec<&'static str> {
    let mut d: Vec<&'static str> = super::datatypes::Dt::ANY
        .iter()
        .map(|dt| dt.iri())
        .filter(|iri| !iri.ends_with("rdf-schema#Literal"))
        .collect();
    d.push("http://www.w3.org/1999/02/22-rdf-syntax-ns#langString");
    d
}

// ─── Materializer ─────────────────────────────────────────────────────────────

/// Forward-chaining RDFS materializer.
///
/// Writes inferred triples into `target_graph`.  On the next call, previously
/// entailed triples are already present and will not be double-counted.
pub struct RdfsMaterializer<'a> {
    store: &'a TripleStore,
    target_graph: String,
    /// When set, the rules read ONLY these graphs (plus the target graph).
    /// Without it they read the unnamed default graph, as they always did.
    sources: Option<Vec<String>>,
    /// The recognized datatypes (`D`); `None`: [`recognized_datatypes`].
    recognized: Option<std::collections::HashSet<String>>,
    /// Apply `rdfD1` (see the module documentation).
    rdfd1: bool,
}

impl<'a> RdfsMaterializer<'a> {
    /// Restrict the rules to `sources` (plus the target graph). Without a
    /// scope the rules read the unnamed default graph only, so a dataset's
    /// named graphs — and the model version it conforms to — were invisible to
    /// materialisation; this is what `POST /api/reasoning/materialize` sets
    /// from `source_graphs` or the dataset's conformance layer.
    pub fn with_sources(mut self, sources: Vec<String>) -> Self {
        self.sources = Some(sources);
        self
    }

    /// Recognize only `datatypes` (plus `xsd:string` and `rdf:langString`,
    /// which every RDF interpretation recognizes): the `D` of D-entailment.
    /// Without it, every datatype of [`recognized_datatypes`]. A literal of
    /// an unrecognized datatype is not checked and gets no `rdfD1` node.
    #[allow(dead_code)] // library surface: the W3C RDF 1.1 Semantics runner sets `D`
    pub fn with_recognized_datatypes<I, S>(mut self, datatypes: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut d: std::collections::HashSet<String> =
            datatypes.into_iter().map(Into::into).collect();
        d.insert(format!("{XSD_NS}string"));
        d.insert(format!("{RDF_NS}langString"));
        self.recognized = Some(d);
        self
    }

    /// Also apply `rdfD1` (off by default; see the module documentation).
    pub fn with_rdfd1(mut self, on: bool) -> Self {
        self.rdfd1 = on;
        self
    }

    /// Whether `iri` is a recognized datatype of this run.
    fn recognizes(&self, iri: &str) -> bool {
        match &self.recognized {
            Some(d) => d.contains(iri),
            None => recognized_datatypes().contains(&iri),
        }
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
            // Unscoped: the unnamed default graph and the target graph, so a
            // rule sees the previous rounds' derivations.
            None => self
                .store
                .update_over(sparql, std::slice::from_ref(&self.target_graph)),
        }
    }

    /// The graphs the rules read, for a direct quad scan.
    fn graphs(&self) -> Vec<oxigraph::model::GraphName> {
        use oxigraph::model::{GraphName, NamedNode};
        let mut v: Vec<GraphName> = Vec::new();
        if self.sources.is_none() {
            v.push(GraphName::DefaultGraph);
        }
        let names = self
            .scope()
            .unwrap_or_else(|| vec![self.target_graph.clone()]);
        v.extend(
            names
                .into_iter()
                .filter_map(|g| NamedNode::new(g).ok().map(GraphName::NamedNode)),
        );
        v
    }

    /// Create a materializer targeting a named graph (pass [`RDFS_ENTAILMENT_GRAPH`]
    /// for the standard `urn:entailment:rdfs`).
    pub fn with_target(store: &'a TripleStore, target_graph: impl Into<String>) -> Self {
        Self {
            store,
            target_graph: target_graph.into(),
            sources: None,
            recognized: None,
            rdfd1: false,
        }
    }

    /// Run the RDFS patterns to their fixed point and return a summary.
    pub fn materialize(&self) -> Result<ReasoningReport, ReasoningError> {
        let start = Instant::now();
        // `triples_added` is the delta this run produced, not the graph's size
        // afterwards, so a re-run that infers nothing reports 0.
        let initial = count_graph(self.store, &self.target_graph)?;
        info!("RDFS materialization → <{}>", self.target_graph);

        // The axiomatic triples and rdfs1 first: no pattern derives a new
        // container-membership property or a new literal, so one scan of the
        // data bounds both.
        self.write_axioms()?;

        let mut iterations = 0usize;
        loop {
            iterations += 1;
            let before = count_graph(self.store, &self.target_graph)?;
            for rule in SCHEMA_RULES {
                self.apply(rule)?;
            }
            let mut added = count_graph(self.store, &self.target_graph)?.saturating_sub(before);
            if added == 0 {
                for rule in DATA_RULES {
                    self.apply(rule)?;
                }
                if self.rdfd1 {
                    self.apply_rdfd1()?;
                }
                added = count_graph(self.store, &self.target_graph)?.saturating_sub(before);
            }
            debug!("RDFS iteration {}: +{} triples", iterations, added);
            if added == 0 {
                break;
            }
            if iterations >= MAX_ITERATIONS {
                return Err(ReasoningError::NotConverged {
                    regime: "rdfs".to_string(),
                    iterations,
                });
            }
        }

        // The derived triples stay; the report is the inconsistency.
        self.check_datatypes()?;

        let final_count = count_graph(self.store, &self.target_graph)?;
        info!(
            "RDFS materialization complete: {} triples in {} iterations ({} ms)",
            final_count,
            iterations,
            start.elapsed().as_millis()
        );
        Ok(ReasoningReport {
            regime: "rdfs".to_string(),
            triples_added: final_count.saturating_sub(initial),
            iterations,
            elapsed_ms: start.elapsed().as_millis() as u64,
            target_graph: self.target_graph.clone(),
            ..Default::default()
        })
    }

    /// Write the axiomatic triples (with `rdf:_1` … `rdf:_n` for the largest
    /// `n` in scope) and the `rdfs1` declarations of the recognized datatypes
    /// in use, plus `xsd:string` and `rdf:langString`.
    fn write_axioms(&self) -> Result<(), ReasoningError> {
        use oxigraph::model::{NamedNode, Quad, Term};
        let member = |iri: &str| -> u32 {
            iri.strip_prefix(RDF_NS)
                .and_then(|l| l.strip_prefix('_'))
                .filter(|n| !n.starts_with('0'))
                .and_then(|n| n.parse::<u32>().ok())
                .unwrap_or(0)
        };
        let mut max_member = 0u32;
        let mut used: std::collections::HashSet<String> = [
            "http://www.w3.org/2001/XMLSchema#string",
            "http://www.w3.org/1999/02/22-rdf-syntax-ns#langString",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        for g in self.graphs() {
            for quad in self
                .store
                .store()
                .quads_for_pattern(None, None, None, Some(g.as_ref()))
            {
                let quad = quad.map_err(|e| ReasoningError::Store(e.to_string()))?;
                if let oxigraph::model::NamedOrBlankNode::NamedNode(n) = &quad.subject {
                    max_member = max_member.max(member(n.as_str()));
                }
                max_member = max_member.max(member(quad.predicate.as_str()));
                match &quad.object {
                    Term::NamedNode(n) => max_member = max_member.max(member(n.as_str())),
                    Term::Literal(l) => {
                        used.insert(l.datatype().as_str().to_string());
                    }
                    _ => {}
                }
            }
        }
        let target = NamedNode::new(self.target_graph.clone())
            .map_err(|e| ReasoningError::Store(e.to_string()))?;
        let quad = |s: &str, p: &str, o: &str| {
            Quad::new(
                NamedNode::new_unchecked(s),
                NamedNode::new_unchecked(p),
                NamedNode::new_unchecked(o),
                target.clone(),
            )
        };
        let mut quads: Vec<Quad> = axiomatic_triples(max_member)
            .iter()
            .map(|(s, p, o)| quad(s, p, o))
            .collect();
        // rdfs1: each recognized datatype in use is an rdfs:Datatype.
        let rdf_type = expand("rdf:type");
        let datatype = expand("rdfs:Datatype");
        for d in recognized_datatypes() {
            if used.contains(d) && self.recognizes(d) {
                quads.push(quad(d, &rdf_type, &datatype));
            }
        }
        self.store.insert_quads(quads)?;
        Ok(())
    }

    /// A SELECT over the same graphs the patterns read.
    fn rows(
        &self,
        sparql: &str,
        vars: &[&str],
    ) -> Result<Vec<Vec<Option<oxigraph::model::Term>>>, ReasoningError> {
        let res = match self.scope() {
            Some(scope) => self.store.query_scoped(sparql, &scope),
            None => self
                .store
                .query_over(sparql, std::slice::from_ref(&self.target_graph)),
        }?;
        let mut out = Vec::new();
        if let oxigraph::sparql::QueryResults::Solutions(sols) = res {
            for sol in sols {
                let sol = sol.map_err(|e| ReasoningError::Query(e.to_string()))?;
                out.push(vars.iter().map(|v| sol.get(*v).cloned()).collect());
            }
        }
        Ok(out)
    }

    /// The datatype clashes that make the graph unsatisfiable for an RDFS
    /// interpretation recognizing [`recognized_datatypes`] (RDF 1.1 Semantics
    /// §7, §9): an ill-typed literal of a recognized datatype, and a literal
    /// whose value is outside a recognized datatype its property's range, or
    /// a superclass of that range, gives it (`rdfs3`, then `rdfs9`).
    fn check_datatypes(&self) -> Result<(), ReasoningError> {
        use super::datatypes::{self, Dt};
        use oxigraph::model::Term;
        for g in self.graphs() {
            for quad in self
                .store
                .store()
                .quads_for_pattern(None, None, None, Some(g.as_ref()))
            {
                let quad = quad.map_err(|e| ReasoningError::Store(e.to_string()))?;
                if let Term::Literal(l) = &quad.object {
                    if Dt::from_any_iri(l.datatype().as_str()).is_some()
                        && self.recognizes(l.datatype().as_str())
                        && datatypes::literal_value(l).is_none()
                    {
                        return Err(ReasoningError::inconsistency(
                            "ill-typed-literal",
                            format!("{l} is not in the lexical space of its datatype"),
                        ));
                    }
                }
            }
        }
        let q = format!(
            "SELECT DISTINCT ?lt ?d WHERE {{ ?p <{RDFS_NS}range> ?c . ?x ?p ?lt . FILTER(isLiteral(?lt)) \
             {{ BIND(?c AS ?d) }} UNION {{ ?c <{RDFS_NS}subClassOf> ?d }} }}"
        );
        let lang_string = format!("{RDF_NS}langString");
        for r in self.rows(&q, &["lt", "d"])? {
            let (Some(Term::Literal(lt)), Some(Term::NamedNode(d))) = (&r[0], &r[1]) else {
                continue;
            };
            if !self.recognizes(d.as_str()) || !self.recognizes(lt.datatype().as_str()) {
                continue;
            }
            let clash = if d.as_str() == lang_string {
                lt.language().is_none()
            } else {
                match (Dt::from_any_iri(d.as_str()), datatypes::literal_value(lt)) {
                    (Some(dt), Some(v)) => datatypes::in_value_space(&v, dt) == Some(false),
                    _ => false,
                }
            };
            if clash {
                return Err(ReasoningError::inconsistency(
                    "datatype-clash",
                    format!("{lt} is in the range {d}, which does not hold its value"),
                ));
            }
        }
        // A resource typed with two recognized datatypes whose value spaces
        // are disjoint (xsd:integer and xsd:string) can denote no value; nor
        // can a datatype be a subclass of one disjoint from it (every
        // datatype here has values).
        let q = format!(
            "SELECT DISTINCT ?d1 ?d2 WHERE {{ \
             {{ ?x <{RDF_NS}type> ?d1 . \
                FILTER(STRSTARTS(STR(?d1), \"http://www.w3.org/2001/XMLSchema#\")) \
                ?x <{RDF_NS}type> ?d2 . FILTER(STR(?d1) < STR(?d2)) }} \
             UNION {{ ?d1 <{RDFS_NS}subClassOf> ?d2 . \
                FILTER(STRSTARTS(STR(?d1), \"http://www.w3.org/2001/XMLSchema#\")) }} }}"
        );
        for r in self.rows(&q, &["d1", "d2"])? {
            let (Some(Term::NamedNode(a)), Some(Term::NamedNode(b))) = (&r[0], &r[1]) else {
                continue;
            };
            if !self.recognizes(a.as_str()) || !self.recognizes(b.as_str()) {
                continue;
            }
            if let (Some(x), Some(y)) = (Dt::from_any_iri(a.as_str()), Dt::from_any_iri(b.as_str()))
            {
                if x.disjoint(y) {
                    return Err(ReasoningError::inconsistency(
                        "datatype-clash",
                        format!(
                            "the disjoint datatypes {a} and {b} share an instance or a subclass"
                        ),
                    ));
                }
            }
        }
        Ok(())
    }

    /// `rdfD1`: for each well-typed literal of a recognized datatype in
    /// object position, the blank node of its value, typed with the
    /// literal's datatype, takes the literal's place in a copy of the triple.
    /// The node's label is a digest of the value, so equal values share it
    /// and re-runs write the same node.
    fn apply_rdfd1(&self) -> Result<(), ReasoningError> {
        use super::datatypes;
        use oxigraph::model::{BlankNode, NamedNode, Quad, QuadRef, Term};
        use sha2::{Digest, Sha256};
        let target = NamedNode::new(self.target_graph.clone())
            .map_err(|e| ReasoningError::Store(e.to_string()))?;
        let graphs = self.graphs();
        let rdf_type = NamedNode::new_unchecked(expand("rdf:type"));
        let mut nodes: std::collections::HashMap<oxigraph::model::Literal, BlankNode> =
            std::collections::HashMap::new();
        let mut new: std::collections::HashSet<Quad> = std::collections::HashSet::new();
        let present = |q: &Quad| {
            graphs.iter().any(|g| {
                self.store
                    .store()
                    .contains(QuadRef::new(
                        &q.subject,
                        &q.predicate,
                        &q.object,
                        g.as_ref(),
                    ))
                    .unwrap_or(false)
            })
        };
        for g in &graphs {
            for quad in self
                .store
                .store()
                .quads_for_pattern(None, None, None, Some(g.as_ref()))
            {
                let quad = quad.map_err(|e| ReasoningError::Store(e.to_string()))?;
                let Term::Literal(lit) = &quad.object else {
                    continue;
                };
                if !self.recognizes(lit.datatype().as_str()) {
                    continue;
                }
                let node = match nodes.get(lit) {
                    Some(n) => n.clone(),
                    None => {
                        let Some(value) = datatypes::literal_value(lit) else {
                            continue; // ill-typed: an inconsistency, reported later
                        };
                        let digest = Sha256::digest(format!("{value:?}").as_bytes());
                        let node = BlankNode::new_unchecked(format!(
                            "rdfD1{}",
                            hex::encode(&digest[..16])
                        ));
                        nodes.insert(lit.clone(), node.clone());
                        node
                    }
                };
                let typed = Quad::new(
                    node.clone(),
                    rdf_type.clone(),
                    NamedNode::new_unchecked(lit.datatype().as_str()),
                    target.clone(),
                );
                if !present(&typed) {
                    new.insert(typed);
                }
                let copy = Quad::new(
                    quad.subject.clone(),
                    quad.predicate.clone(),
                    node,
                    target.clone(),
                );
                if !present(&copy) {
                    new.insert(copy);
                }
            }
        }
        if !new.is_empty() {
            self.store.insert_quads(new.into_iter().collect())?;
        }
        Ok(())
    }

    /// Run one pattern as an INSERT into the target graph.
    fn apply(&self, (name, head, body): &Rule) -> Result<(), ReasoningError> {
        let q = format!(
            "PREFIX rdf: <{RDF_NS}>\nPREFIX rdfs: <{RDFS_NS}>\n\
             INSERT {{ GRAPH <{tg}> {{ {head} }} }} WHERE {{ {body} }}",
            tg = self.target_graph
        );
        self.run_update(&q)
            .map_err(|e| ReasoningError::Query(format!("RDFS pattern {name}: {e}")))
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reasoning::common::RDFS_ENTAILMENT_GRAPH;
    use crate::store::TripleStore;
    use oxigraph::io::RdfFormat;

    fn store_with(ttl: &str) -> TripleStore {
        let s = TripleStore::in_memory().unwrap();
        s.load_str(ttl, RdfFormat::Turtle, None).unwrap();
        s
    }

    fn ask(store: &TripleStore, sparql: &str) -> bool {
        match store.query(sparql).unwrap() {
            oxigraph::sparql::QueryResults::Boolean(b) => b,
            _ => false,
        }
    }

    #[test]
    fn test_rdfs_subclass_type_propagation() {
        let s = store_with(
            "@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
             @prefix rdf:  <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
             @prefix ex:   <http://example.org/> .
             ex:Student rdfs:subClassOf ex:Person .
             ex:alice   rdf:type        ex:Student .",
        );
        RdfsMaterializer::with_target(&s, RDFS_ENTAILMENT_GRAPH)
            .materialize()
            .unwrap();
        assert!(ask(
            &s,
            "ASK { GRAPH <urn:entailment:rdfs> \
             { <http://example.org/alice> \
               <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> \
               <http://example.org/Person> } }"
        ));
    }

    #[test]
    fn test_rdfs_subclass_transitivity() {
        let s = store_with(
            "@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
             @prefix ex:   <http://example.org/> .
             ex:PhD     rdfs:subClassOf ex:Student .
             ex:Student rdfs:subClassOf ex:Person .",
        );
        RdfsMaterializer::with_target(&s, RDFS_ENTAILMENT_GRAPH)
            .materialize()
            .unwrap();
        assert!(ask(
            &s,
            "ASK { GRAPH <urn:entailment:rdfs> \
             { <http://example.org/PhD> \
               <http://www.w3.org/2000/01/rdf-schema#subClassOf> \
               <http://example.org/Person> } }"
        ));
    }

    #[test]
    fn test_rdfs_domain() {
        let s = store_with(
            "@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
             @prefix rdf:  <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
             @prefix ex:   <http://example.org/> .
             ex:teaches rdfs:domain ex:Professor .
             ex:bob     ex:teaches  ex:cs101 .",
        );
        RdfsMaterializer::with_target(&s, RDFS_ENTAILMENT_GRAPH)
            .materialize()
            .unwrap();
        assert!(ask(
            &s,
            "ASK { GRAPH <urn:entailment:rdfs> \
             { <http://example.org/bob> \
               <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> \
               <http://example.org/Professor> } }"
        ));
    }

    #[test]
    fn test_rdfs_range() {
        let s = store_with(
            "@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
             @prefix rdf:  <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
             @prefix ex:   <http://example.org/> .
             ex:teaches rdfs:range ex:Course .
             ex:bob     ex:teaches ex:cs101 .",
        );
        RdfsMaterializer::with_target(&s, RDFS_ENTAILMENT_GRAPH)
            .materialize()
            .unwrap();
        assert!(ask(
            &s,
            "ASK { GRAPH <urn:entailment:rdfs> \
             { <http://example.org/cs101> \
               <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> \
               <http://example.org/Course> } }"
        ));
    }

    #[test]
    fn test_rdfs_subproperty() {
        let s = store_with(
            "@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
             @prefix ex:   <http://example.org/> .
             ex:fatherOf rdfs:subPropertyOf ex:parentOf .
             ex:bob      ex:fatherOf        ex:alice .",
        );
        RdfsMaterializer::with_target(&s, RDFS_ENTAILMENT_GRAPH)
            .materialize()
            .unwrap();
        assert!(ask(
            &s,
            "ASK { GRAPH <urn:entailment:rdfs> \
             { <http://example.org/bob> \
               <http://example.org/parentOf> \
               <http://example.org/alice> } }"
        ));
    }

    #[test]
    fn test_rdfs1_datatype_subclass_literal() {
        let s = store_with(
            "@prefix ex:  <http://example.org/> .
             @prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
             ex:bob ex:age \"42\"^^xsd:integer .",
        );
        RdfsMaterializer::with_target(&s, RDFS_ENTAILMENT_GRAPH)
            .materialize()
            .unwrap();
        // rdfs1 declares xsd:integer an rdfs:Datatype; rdfs13 then makes it a
        // subclass of rdfs:Literal.
        assert!(ask(
            &s,
            "ASK { GRAPH <urn:entailment:rdfs> \
             { <http://www.w3.org/2001/XMLSchema#integer> \
               <http://www.w3.org/2000/01/rdf-schema#subClassOf> \
               <http://www.w3.org/2000/01/rdf-schema#Literal> } }"
        ));
    }

    #[test]
    fn test_rdfs4a_subject_resource() {
        let s = store_with(
            "@prefix ex: <http://example.org/> .
             ex:bob ex:knows ex:alice .",
        );
        RdfsMaterializer::with_target(&s, RDFS_ENTAILMENT_GRAPH)
            .materialize()
            .unwrap();
        assert!(ask(
            &s,
            "ASK { GRAPH <urn:entailment:rdfs> \
             { <http://example.org/bob> \
               <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> \
               <http://www.w3.org/2000/01/rdf-schema#Resource> } }"
        ));
    }

    #[test]
    fn test_rdfs4b_object_resource() {
        let s = store_with(
            "@prefix ex: <http://example.org/> .
             ex:bob ex:knows ex:alice .",
        );
        RdfsMaterializer::with_target(&s, RDFS_ENTAILMENT_GRAPH)
            .materialize()
            .unwrap();
        assert!(ask(
            &s,
            "ASK { GRAPH <urn:entailment:rdfs> \
             { <http://example.org/alice> \
               <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> \
               <http://www.w3.org/2000/01/rdf-schema#Resource> } }"
        ));
    }

    #[test]
    fn test_rdfs6_property_self_subproperty() {
        let s = store_with(
            "@prefix rdf:  <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
             @prefix ex:   <http://example.org/> .
             ex:knows rdf:type rdf:Property .",
        );
        RdfsMaterializer::with_target(&s, RDFS_ENTAILMENT_GRAPH)
            .materialize()
            .unwrap();
        assert!(ask(
            &s,
            "ASK { GRAPH <urn:entailment:rdfs> \
             { <http://example.org/knows> \
               <http://www.w3.org/2000/01/rdf-schema#subPropertyOf> \
               <http://example.org/knows> } }"
        ));
    }

    #[test]
    fn test_rdfs8_class_subclass_resource() {
        let s = store_with(
            "@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
             @prefix ex:   <http://example.org/> .
             ex:Person rdfs:subClassOf ex:Agent .
             ex:Person a rdfs:Class .",
        );
        RdfsMaterializer::with_target(&s, RDFS_ENTAILMENT_GRAPH)
            .materialize()
            .unwrap();
        assert!(ask(
            &s,
            "ASK { GRAPH <urn:entailment:rdfs> \
             { <http://example.org/Person> \
               <http://www.w3.org/2000/01/rdf-schema#subClassOf> \
               <http://www.w3.org/2000/01/rdf-schema#Resource> } }"
        ));
    }

    #[test]
    fn test_rdfs10_class_self_subclass() {
        let s = store_with(
            "@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
             @prefix ex:   <http://example.org/> .
             ex:Person a rdfs:Class .",
        );
        RdfsMaterializer::with_target(&s, RDFS_ENTAILMENT_GRAPH)
            .materialize()
            .unwrap();
        assert!(ask(
            &s,
            "ASK { GRAPH <urn:entailment:rdfs> \
             { <http://example.org/Person> \
               <http://www.w3.org/2000/01/rdf-schema#subClassOf> \
               <http://example.org/Person> } }"
        ));
    }

    #[test]
    fn test_rdfs12_container_membership_property() {
        let s = store_with(
            "@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
             @prefix rdf:  <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
             rdf:_1 a rdfs:ContainerMembershipProperty .",
        );
        RdfsMaterializer::with_target(&s, RDFS_ENTAILMENT_GRAPH)
            .materialize()
            .unwrap();
        assert!(ask(
            &s,
            "ASK { GRAPH <urn:entailment:rdfs> \
             { <http://www.w3.org/1999/02/22-rdf-syntax-ns#_1> \
               <http://www.w3.org/2000/01/rdf-schema#subPropertyOf> \
               <http://www.w3.org/2000/01/rdf-schema#member> } }"
        ));
    }

    #[test]
    fn test_rdfs13_datatype_subclass_literal() {
        let s = store_with(
            "@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
             @prefix xsd:  <http://www.w3.org/2001/XMLSchema#> .
             xsd:integer a rdfs:Datatype .",
        );
        RdfsMaterializer::with_target(&s, RDFS_ENTAILMENT_GRAPH)
            .materialize()
            .unwrap();
        assert!(ask(
            &s,
            "ASK { GRAPH <urn:entailment:rdfs> \
             { <http://www.w3.org/2001/XMLSchema#integer> \
               <http://www.w3.org/2000/01/rdf-schema#subClassOf> \
               <http://www.w3.org/2000/01/rdf-schema#Literal> } }"
        ));
    }
}
