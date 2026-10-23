//! Konclude as an OWL 2 DL backend.
//!
//! [Konclude](https://github.com/konclude/Konclude) (LGPL-3.0) reads OWL 2
//! functional-style syntax or OWL/XML from files only. This bridge
//!
//! 1. writes the input — mapped from RDF by [`super::owl_mapping`] — as a
//!    functional-syntax file ([`super::owl_fs`]);
//! 2. runs `Konclude owllinkfile` on an OWLlink request: `IsKBSatisfiable`
//!    (consistency), `GetSubClassHierarchy` (classification),
//!    `GetFlattenedTypes` and `GetSameIndividuals` per named individual
//!    (realisation);
//! 3. runs `Konclude sparqlfile` with one `SELECT ?x ?y { ?x <p> ?y }` per
//!    object property for the entailed object property assertions;
//! 4. kills either run at the deadline (`OTS_REASONER_TIMEOUT_SECS`).
//!
//! Checked against Konclude v0.7.0-1138: `IsKBConsistent` is not supported
//! (`IsKBSatisfiable` is), an inconsistent KB answers every later query with
//! `UnsatisfiableKBError`, the first synset of a class hierarchy is the
//! unsatisfiable classes, and the SPARQL interface does not return data
//! values entailed by `owl:hasValue`, so data property assertions are not
//! materialised. Konclude ignores annotations.
//!
//! Entailment checks reduce to consistency (OWL 2 Direct Semantics): `O ⊨ α`
//! iff `O ∪ ¬α` is inconsistent, one knowledge base per negated axiom in a
//! single OWLlink request. Axioms without such a reduction (data sub-property,
//! keys, datatype definitions, anonymous individuals) answer `unknown`.
//!
//! Every OWLlink run passes [`CONFIG`], which turns off two ABox optimisations that
//! lose clashes (asserted edges against asymmetric, irreflexive or disjoint
//! properties; an inverse predecessor merged back into an individual).
//!
//! Konclude v0.7.0 still gets some inputs wrong, and the bridge then answers
//! `unknown` instead ([`Gaps`]). It misses the clash ("satisfiable" where the
//! right answer is "unsatisfiable") of some `DataComplementOf` shapes
//! (`∃p.(D ⊓ ¬D)`, `∃p.¬rdfs:Literal`, a complement range against an
//! asserted value), of disjoint data properties, of keys (which it ignores),
//! of float ranges with no float in them, of `rdf:XMLLiteral` values, and of
//! nominals with inverses and counting. So when the input has one of these,
//! an answer that rests on a knowledge base being satisfiable (consistent,
//! not entailed, satisfiable class) is `unknown`, and a run is marked
//! incomplete. The one wrong clash seen is a complement of a number datatype:
//! Konclude reads `¬xsd:positiveInteger` as holding no number at all. Then an
//! answer that rests on a clash is `unknown` too, and a run derives nothing.
//! The bridge's own negations `∀p.¬{v}` and `∃p.¬R` (for a range `R` that is
//! not a number datatype) probe correct, and [`super::owl_fs`] writes
//! `¬rdfs:Literal` as an empty range. A check on which Konclude crashes
//! answers `unknown` as well.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use oxigraph::model::{NamedNode, Term, Triple};

use super::common::ReasoningError;
use super::dl_backend::{CheckOutcome, CheckTask, DlBackend, DlInput, DlOutcome, Tri};
use super::dl_config::DlConfig;
use super::owl_fs::FsWriter;
use super::owl_mapping::map_triples;
use super::owl_model::*;
use super::owl_profile::visit_exprs;
use crate::spec_import::ids::{parse_xml, El};

const NAME: &str = "konclude";
const KB: &str = "urn:ots:kb";
const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const RDFS_SUB_CLASS_OF: &str = "http://www.w3.org/2000/01/rdf-schema#subClassOf";
const OWL_EQUIVALENT_CLASS: &str = "http://www.w3.org/2002/07/owl#equivalentClass";
const OWL_SAME_AS: &str = "http://www.w3.org/2002/07/owl#sameAs";
/// Entailment sub-checks per request before the answer is `unknown`.
const MAX_SUBCHECKS: usize = 64;
/// Why an answer is withheld or a run incomplete when the input keeps a
/// `DataComplementOf` (see the module comment).
const DATA_COMPLEMENT: &str = "the input uses owl:datatypeComplementOf, and Konclude v0.7.0 can \
     miss the inconsistency some data complements cause";
/// … has disjoint data properties: two of them sharing a value never clash
/// for Konclude.
const DISJOINT_DATA: &str = "the input has disjoint data properties, and Konclude v0.7.0 misses \
     the inconsistency of two disjoint data properties sharing a value";
/// … uses `owl:hasKey`, which Konclude v0.7.0 does not reason with at all.
const KEYS: &str = "the input uses owl:hasKey, and Konclude v0.7.0 does not reason with keys";
/// … combines nominals, inverse properties and number restrictions.
const NOMINAL_COUNTING: &str = "the input combines owl:oneOf in class axioms with inverse \
     properties and functional properties or cardinality restrictions, and Konclude v0.7.0 can \
     miss the inconsistency these cause together";
/// … restricts `xsd:float` or `xsd:double`, which Konclude treats as dense.
const FLOAT_FACETS: &str = "the input restricts xsd:float or xsd:double with facets, and \
     Konclude v0.7.0 treats their value spaces as dense, so it misses a range with no float in it";
/// … has `rdf:XMLLiteral` values.
const XML_LITERALS: &str = "the input has rdf:XMLLiteral values, and Konclude v0.7.0 can take \
     two different ones for the same value";
/// … complements a number datatype: the one known source of a wrong clash.
const NUMERIC_COMPLEMENT: &str = "the input or the check complements a number datatype \
     (owl:datatypeComplementOf of xsd:decimal, xsd:integer or a type derived from them), and \
     Konclude v0.7.0 reads such a complement as holding no number at all";
/// Konclude's configuration for the OWLlink runs (`-c`), which decide
/// consistency, classification and types. Its ABox optimisations,
/// the individuals backend cache and individual saturation, lose clashes:
/// with them on, Konclude v0.7.0-1138 calls these consistent:
/// `AsymmetricObjectProperty(p)` with `p(a, b)` and `p(b, a)`;
/// `IrreflexiveObjectProperty(p)` with `p(a, a)`; disjoint `p`, `q` with
/// `p(a, b)` and `q(a, b)`; `a : A ⊓ ∃r⁻.(∃r.B ⊓ ≤1 r)` with `A`, `B`
/// disjoint (WebOnt-description-logic-008 and the other DL98 cases of that
/// shape), although it finds that class unsatisfiable when classifying.
/// With both off they come out right (`dl_konclude_live_abox_clashes`).
/// The SPARQL runs keep the defaults: with individual saturation off,
/// Konclude's SPARQL answers lose entailed property assertions (a property
/// chain's), and by then consistency has been decided.
pub const CONFIG: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<RequestMessage xmlns="http://www.owllink.org/owllink-xml#" xmlns:owl="http://www.w3.org/2002/07/owl#">
<Set key="Konclude.Calculation.Optimization.IndividualsBackendCacheLoading"><Literal>false</Literal></Set>
<Set key="Konclude.Calculation.Optimization.IndividualSaturation"><Literal>false</Literal></Set>
</RequestMessage>
"#;
/// How [`KoncludeBackend::run`] reports a run killed by a signal.
const CRASHED: &str = "crashed";
const XSD_FLOAT: &str = "http://www.w3.org/2001/XMLSchema#float";
const XSD_DOUBLE: &str = "http://www.w3.org/2001/XMLSchema#double";
const XML_LITERAL: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#XMLLiteral";

pub struct KoncludeBackend {
    bin: String,
    timeout: Duration,
    work_dir: PathBuf,
}

/// A scratch directory removed on drop.
struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

impl KoncludeBackend {
    pub fn new(cfg: &DlConfig) -> Self {
        KoncludeBackend {
            bin: cfg.konclude_bin.clone(),
            timeout: cfg.timeout,
            work_dir: cfg.work_dir.clone(),
        }
    }

    fn scratch(&self) -> Result<Scratch, ReasoningError> {
        let dir = self
            .work_dir
            .join(format!("ots-konclude-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).map_err(|e| {
            ReasoningError::Store(format!("Konclude work dir {}: {e}", dir.display()))
        })?;
        Ok(Scratch(dir))
    }

    /// Run Konclude with `args` in `dir` until it exits or `deadline` passes.
    /// Output goes to files, never pipes, so a chatty run cannot deadlock.
    fn run(
        &self,
        args: &[String],
        dir: &Path,
        deadline: Instant,
    ) -> Result<String, ReasoningError> {
        let log_path = dir.join(format!("konclude-{}.log", uuid::Uuid::new_v4()));
        let log = std::fs::File::create(&log_path)
            .map_err(|e| ReasoningError::Store(format!("Konclude log: {e}")))?;
        let err = log
            .try_clone()
            .map_err(|e| ReasoningError::Store(format!("Konclude log: {e}")))?;
        let mut child = Command::new(&self.bin)
            .args(args)
            .current_dir(dir)
            .stdin(Stdio::null())
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(err))
            .spawn()
            .map_err(|e| {
                ReasoningError::Unavailable(format!(
                    "cannot run the Konclude binary `{}` ({e}); set OTS_KONCLUDE_BIN",
                    self.bin
                ))
            })?;
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    let text = std::fs::read_to_string(&log_path).unwrap_or_default();
                    if !status.success() {
                        let tail: String = text
                            .lines()
                            .rev()
                            .take(5)
                            .collect::<Vec<_>>()
                            .into_iter()
                            .rev()
                            .collect::<Vec<_>>()
                            .join(" | ");
                        // No exit code: killed by a signal (SIGSEGV).
                        let how = if status.code().is_none() {
                            CRASHED
                        } else {
                            "exited"
                        };
                        return Err(ReasoningError::Backend {
                            backend: NAME.into(),
                            detail: format!("{how} with {status}: {tail}"),
                        });
                    }
                    return Ok(text);
                }
                Ok(None) if Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(ReasoningError::Timeout {
                        backend: NAME.into(),
                        seconds: self.timeout.as_secs(),
                    });
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(20)),
                Err(e) => {
                    let _ = child.kill();
                    return Err(ReasoningError::Backend {
                        backend: NAME.into(),
                        detail: format!("waiting for Konclude: {e}"),
                    });
                }
            }
        }
    }

    /// Write `files` (name → functional syntax), run `request` (OWLlink body
    /// elements) and return one response element per request element.
    fn owllink(
        &self,
        dir: &Path,
        request: &str,
        deadline: Instant,
    ) -> Result<(Vec<El>, Option<String>), ReasoningError> {
        let req = dir.join("request.xml");
        let resp = dir.join("response.xml");
        let body = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <RequestMessage xmlns=\"http://www.owllink.org/owllink#\" \
             xmlns:owl=\"http://www.w3.org/2002/07/owl#\">\n{request}</RequestMessage>\n"
        );
        std::fs::write(&req, body)
            .map_err(|e| ReasoningError::Store(format!("OWLlink request: {e}")))?;
        let config = dir.join("konclude-config.xml");
        std::fs::write(&config, CONFIG)
            .map_err(|e| ReasoningError::Store(format!("Konclude config: {e}")))?;
        let log = self.run(
            &[
                "owllinkfile".into(),
                "-c".into(),
                config.display().to_string(),
                "-i".into(),
                req.display().to_string(),
                "-o".into(),
                resp.display().to_string(),
            ],
            dir,
            deadline,
        )?;
        let bytes = std::fs::read(&resp).map_err(|e| ReasoningError::Backend {
            backend: NAME.into(),
            detail: format!("no OWLlink response ({e})"),
        })?;
        let doc = parse_xml(&bytes).map_err(|e| ReasoningError::Backend {
            backend: NAME.into(),
            detail: format!("unreadable OWLlink response: {e}"),
        })?;
        Ok((doc.children, version(&log)))
    }

    fn load(dir: &Path, name: &str, text: &str, kb: &str) -> Result<String, ReasoningError> {
        let path = dir.join(name);
        std::fs::write(&path, text).map_err(|e| ReasoningError::Store(format!("{name}: {e}")))?;
        Ok(format!(
            "<CreateKB kb=\"{kb}\"/>\n<LoadOntologies kb=\"{kb}\"><OntologyIRI IRI=\"{}\"/></LoadOntologies>\n",
            xml_attr(&file_iri(&path))
        ))
    }
}

impl DlBackend for KoncludeBackend {
    fn name(&self) -> &'static str {
        NAME
    }

    fn reason(&self, input: &DlInput) -> Result<DlOutcome, ReasoningError> {
        let deadline = Instant::now() + self.timeout;
        let dir = self.scratch()?;
        let (text, gaps) = write_kb(&input.ontology, &[]);
        if !gaps.false_clash.is_empty() {
            // Every subsumption, unsatisfiable class and type Konclude
            // reports may rest on a clash that is not there.
            return Ok(DlOutcome {
                incomplete: true,
                warnings: vec![format!(
                    "{}: nothing is derived, and consistency is unknown",
                    gaps.false_clash.join("; ")
                )],
                ..Default::default()
            });
        }
        let mut req = Self::load(&dir.0, "input.ofn", &text, KB)?;
        req.push_str(&format!(
            "<IsKBSatisfiable kb=\"{KB}\"/>\n<GetSubClassHierarchy kb=\"{KB}\"/>\n"
        ));
        let individuals = named_individuals(&input.ontology);
        for i in &individuals {
            req.push_str(&format!(
                "<GetFlattenedTypes kb=\"{KB}\" direct=\"false\"><owl:NamedIndividual IRI=\"{}\"/></GetFlattenedTypes>\n",
                xml_attr(i)
            ));
        }
        for i in &individuals {
            req.push_str(&format!(
                "<GetSameIndividuals kb=\"{KB}\"><owl:NamedIndividual IRI=\"{}\"/></GetSameIndividuals>\n",
                xml_attr(i)
            ));
        }
        req.push_str(&format!("<ReleaseKB kb=\"{KB}\"/>\n"));
        let (resp, version) = self.owllink(&dir.0, &req, deadline)?;
        let n = individuals.len();
        if resp.len() != 5 + 2 * n {
            return Err(ReasoningError::Backend {
                backend: NAME.into(),
                detail: format!(
                    "expected {} OWLlink responses, got {}",
                    5 + 2 * n,
                    resp.len()
                ),
            });
        }
        expect_ok(&resp[1], "LoadOntologies")?;
        let consistent = boolean(&resp[2])?;
        let mut out = DlOutcome {
            consistent: Some(consistent),
            version,
            ..Default::default()
        };
        if !consistent {
            out.inconsistency =
                Some("Konclude: the ontology is inconsistent (IsKBSatisfiable = false)".into());
            return Ok(out);
        }
        if !gaps.missed_clash.is_empty() {
            // Konclude's "consistent" may be a missed clash, and so may every
            // subsumption or type it did not report.
            out.consistent = None;
            out.incomplete = true;
            out.warnings.push(format!(
                "{}: consistency is unknown and entailments may be missing",
                gaps.missed_clash.join("; ")
            ));
        }
        let (unsat, edges, synsets) = hierarchy(&resp[3])?;
        out.unsatisfiable = unsat
            .iter()
            .filter(|c| c.as_str() != OWL_NOTHING)
            .cloned()
            .collect();
        out.inferred
            .extend(hierarchy_triples(&unsat, &edges, &synsets));
        for (k, ind) in individuals.iter().enumerate() {
            for c in iris_in(&resp[4 + k]) {
                if c != OWL_THING {
                    out.inferred.push(triple(ind, RDF_TYPE, &c));
                }
            }
            for other in iris_in(&resp[4 + n + k]) {
                if &other != ind {
                    out.inferred.push(triple(ind, OWL_SAME_AS, &other));
                }
            }
        }
        match self.property_assertions(&dir.0, &input.ontology, deadline) {
            Ok(triples) => out.inferred.extend(triples),
            // Konclude's SPARQL interface crashes on some inputs (an
            // irreflexive super-property); what OWLlink reported stands.
            Err(ReasoningError::Backend { detail, .. }) if detail.starts_with(CRASHED) => {
                out.incomplete = true;
                out.warnings.push(format!(
                    "Konclude crashed answering the property assertion queries ({detail});                      no entailed object property assertions are reported"
                ));
            }
            Err(e) => return Err(e),
        }
        if input
            .ontology
            .axioms
            .iter()
            .any(|a| matches!(a.kind, AxiomKind::Declaration(EntityKind::DataProperty, _)))
        {
            out.warnings.push(
                "Konclude does not report entailed data property values; only asserted ones are in scope"
                    .into(),
            );
        }
        Ok(out)
    }

    fn check(&self, input: &DlInput, task: &CheckTask) -> Result<CheckOutcome, ReasoningError> {
        let deadline = Instant::now() + self.timeout;
        let dir = self.scratch()?;
        // Each knowledge base: the premise plus extra axioms; the answer is
        // derived from which of them are satisfiable.
        let mut kbs: Vec<Vec<Axiom>> = Vec::new();
        let mut unknown = false;
        // A data complement in the conclusion reaches Konclude inside `¬α`.
        let mut conclusion_gaps = Gaps::default();
        match task {
            CheckTask::Consistency => kbs.push(Vec::new()),
            CheckTask::Satisfiability { class } => {
                kbs.push(vec![Axiom::new(AxiomKind::ClassAssertion(
                    ClassExpr::Class(class.clone()),
                    Individual::Named(fresh(0)),
                ))])
            }
            CheckTask::Entailment { conclusion } => {
                let mapped = map_triples(conclusion);
                if !mapped.unmapped.is_empty() {
                    unknown = true;
                }
                let mut n = 0usize;
                let mut writer = FsWriter::new();
                for a in &mapped.ontology.axioms {
                    writer.axiom(a);
                    match negations(&a.kind, &mut n) {
                        Some(subs) => kbs.extend(subs),
                        None => unknown = true,
                    }
                }
                conclusion_gaps = Gaps::of_complements(&writer);
                if kbs.len() > MAX_SUBCHECKS {
                    let mut o = CheckOutcome::of(Tri::Unknown);
                    o.detail = Some(format!(
                        "the conclusion needs {} consistency checks; at most {MAX_SUBCHECKS} are run",
                        kbs.len()
                    ));
                    return Ok(o);
                }
            }
            CheckTask::Profile => unreachable!("answered by the server"),
        }
        // Premise consistency first: an inconsistent premise entails
        // everything and satisfies nothing.
        let (premise, mut gaps) = write_kb(&input.ontology, &[]);
        let mut req = Self::load(&dir.0, "premise.ofn", &premise, KB)?;
        req.push_str(&format!(
            "<IsKBSatisfiable kb=\"{KB}\"/>\n<ReleaseKB kb=\"{KB}\"/>\n"
        ));
        let mut extra_kbs = Vec::new();
        for (k, extra) in kbs.iter().enumerate().filter(|(_, e)| !e.is_empty()) {
            let kb = format!("{KB}:{k}");
            let (text, mut extra_gaps) = write_kb(&input.ontology, extra);
            // The bridge's own `∀p.¬{v}` and `∃p.¬R` do not hide clashes
            // from Konclude (`dl_konclude_live_data_complements`); a
            // complement of a number datatype still makes clashes up.
            extra_gaps
                .missed_clash
                .retain(|why| *why != DATA_COMPLEMENT);
            gaps.add(extra_gaps);
            req.push_str(&Self::load(&dir.0, &format!("check-{k}.ofn"), &text, &kb)?);
            req.push_str(&format!(
                "<IsKBSatisfiable kb=\"{kb}\"/>\n<ReleaseKB kb=\"{kb}\"/>\n"
            ));
            extra_kbs.push(k);
        }
        let (resp, version) = match self.owllink(&dir.0, &req, deadline) {
            // A crash is no answer either way.
            Err(ReasoningError::Backend { detail, .. }) if detail.starts_with(CRASHED) => {
                let mut o = CheckOutcome::of(Tri::Unknown);
                o.detail = Some(format!("Konclude crashed on this check: {detail}"));
                return Ok(o);
            }
            r => r?,
        };
        if resp.len() != 4 + 4 * extra_kbs.len() {
            return Err(ReasoningError::Backend {
                backend: NAME.into(),
                detail: format!(
                    "expected {} OWLlink responses, got {}",
                    4 + 4 * extra_kbs.len(),
                    resp.len()
                ),
            });
        }
        expect_ok(&resp[1], "LoadOntologies")?;
        let premise_consistent = boolean(&resp[2])?;
        let mut sat = Vec::new();
        for j in 0..extra_kbs.len() {
            expect_ok(&resp[4 + 4 * j + 1], "LoadOntologies")?;
            sat.push(boolean(&resp[4 + 4 * j + 2])?);
        }
        let mut o = match task {
            CheckTask::Consistency => {
                let mut o = CheckOutcome::of(Tri::from_bool(premise_consistent));
                if !premise_consistent {
                    o.inconsistency = Some((
                        "external-reasoner".into(),
                        "Konclude: the ontology is inconsistent (IsKBSatisfiable = false)".into(),
                    ));
                }
                o
            }
            CheckTask::Satisfiability { .. } if !premise_consistent => CheckOutcome::of(Tri::False),
            CheckTask::Satisfiability { .. } => CheckOutcome::of(Tri::from_bool(sat[0])),
            CheckTask::Entailment { .. } if !premise_consistent => CheckOutcome::of(Tri::True),
            CheckTask::Entailment { .. } => {
                if sat.iter().any(|s| *s) {
                    CheckOutcome::of(Tri::False)
                } else if unknown {
                    CheckOutcome::of(Tri::Unknown)
                } else {
                    CheckOutcome::of(Tri::True)
                }
            }
            CheckTask::Profile => unreachable!(),
        };
        o.version = version;
        if unknown && o.result == Tri::Unknown {
            o.detail = Some(
                "part of the conclusion has no consistency reduction (data sub-properties, keys, \
                 datatype definitions, anonymous individuals)"
                    .into(),
            );
        }
        // Inconsistent, entailed and unsatisfiable rest on a clash.
        let rests_on_clash = match task {
            CheckTask::Consistency | CheckTask::Satisfiability { .. } => o.result == Tri::False,
            _ => o.result == Tri::True,
        };
        gaps.add(conclusion_gaps);
        // A "yes"/"no" that rests on Konclude finding no clash can be a missed
        // clash; one that rests on a clash, a clash that is not there.
        let doubts = if rests_on_clash {
            &gaps.false_clash
        } else {
            &gaps.missed_clash
        };
        if !doubts.is_empty() && o.result != Tri::Unknown {
            o.detail = Some(format!(
                "{}, so its \"{}\" answer is not trusted",
                doubts.join("; "),
                if o.result == Tri::True { "yes" } else { "no" }
            ));
            o.result = Tri::Unknown;
        }
        Ok(o)
    }
}

impl KoncludeBackend {
    /// Entailed object property assertions between named individuals, one
    /// SPARQL query per object property.
    fn property_assertions(
        &self,
        dir: &Path,
        ont: &Ontology,
        deadline: Instant,
    ) -> Result<Vec<Triple>, ReasoningError> {
        let props: Vec<&str> = ont
            .declared(EntityKind::ObjectProperty)
            .filter(|p| *p != OWL_TOP_OBJECT_PROPERTY && *p != OWL_BOTTOM_OBJECT_PROPERTY)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        if props.is_empty() {
            return Ok(Vec::new());
        }
        let queries: String = props
            .iter()
            .map(|p| format!("SELECT ?x ?y WHERE {{ ?x <{p}> ?y }}\n"))
            .collect();
        let q = dir.join("assertions.sparql");
        let out = dir.join("assertions.xml");
        std::fs::write(&q, queries)
            .map_err(|e| ReasoningError::Store(format!("SPARQL file: {e}")))?;
        self.run(
            &[
                "sparqlfile".into(),
                "-s".into(),
                q.display().to_string(),
                "-i".into(),
                dir.join("input.ofn").display().to_string(),
                "-o".into(),
                out.display().to_string(),
            ],
            dir,
            deadline,
        )?;
        let text = std::fs::read_to_string(&out).map_err(|e| ReasoningError::Backend {
            backend: NAME.into(),
            detail: format!("no SPARQL answers ({e})"),
        })?;
        let docs = split_documents(&text);
        if docs.len() != props.len() {
            return Err(ReasoningError::Backend {
                backend: NAME.into(),
                detail: format!(
                    "expected {} SPARQL answers, got {}",
                    props.len(),
                    docs.len()
                ),
            });
        }
        let mut triples = Vec::new();
        for (p, doc) in props.iter().zip(docs) {
            let el = parse_xml(doc.as_bytes()).map_err(|e| ReasoningError::Backend {
                backend: NAME.into(),
                detail: format!("unreadable SPARQL answer: {e}"),
            })?;
            for row in descendants(&el, "result") {
                let mut x = None;
                let mut y = None;
                for b in row.children.iter().filter(|c| c.name == "binding") {
                    let uri = b
                        .children
                        .iter()
                        .find(|c| c.name == "uri")
                        .map(|u| unescape(&u.text));
                    match attr(b, "name") {
                        Some("x") => x = uri,
                        Some("y") => y = uri,
                        _ => {}
                    }
                }
                if let (Some(x), Some(y)) = (x, y) {
                    triples.push(triple(&x, p, &y));
                }
            }
        }
        Ok(triples)
    }
}

// ── OWLlink responses ────────────────────────────────────────────────────────

fn attr<'a>(el: &'a El, k: &str) -> Option<&'a str> {
    el.attr(k)
}

fn error_text(el: &El) -> Option<String> {
    if el.name.ends_with("Error") {
        Some(
            attr(el, "error")
                .map(str::to_string)
                .unwrap_or_else(|| el.name.clone()),
        )
    } else {
        None
    }
}

fn expect_ok(el: &El, what: &str) -> Result<(), ReasoningError> {
    match error_text(el) {
        Some(e) => Err(ReasoningError::Backend {
            backend: NAME.into(),
            detail: format!("{what}: {e}"),
        }),
        None => Ok(()),
    }
}

fn boolean(el: &El) -> Result<bool, ReasoningError> {
    if el.name == "BooleanResponse" {
        return match attr(el, "result") {
            Some("true") => Ok(true),
            Some("false") => Ok(false),
            other => Err(ReasoningError::Backend {
                backend: NAME.into(),
                detail: format!("BooleanResponse result {other:?}"),
            }),
        };
    }
    // An inconsistent KB answers later satisfiability queries this way.
    if el.name == "UnsatisfiableKBError" {
        return Ok(false);
    }
    Err(ReasoningError::Backend {
        backend: NAME.into(),
        detail: error_text(el).unwrap_or_else(|| format!("unexpected <{}>", el.name)),
    })
}

/// The IRIs of every `owl:Class` / `owl:NamedIndividual` in `el`.
fn iris_in(el: &El) -> Vec<String> {
    let mut out = Vec::new();
    fn walk(e: &El, out: &mut Vec<String>) {
        if e.name == "Class" || e.name == "NamedIndividual" {
            if let Some(i) = attr(e, "IRI") {
                out.push(i.to_string());
            }
        }
        for c in &e.children {
            walk(c, out);
        }
    }
    walk(el, &mut out);
    out
}

fn descendants<'a>(el: &'a El, name: &str) -> Vec<&'a El> {
    let mut out = Vec::new();
    fn walk<'a>(e: &'a El, name: &str, out: &mut Vec<&'a El>) {
        if e.name == name {
            out.push(e);
        }
        for c in &e.children {
            walk(c, name, out);
        }
    }
    walk(el, name, &mut out);
    out
}

type Synset = Vec<String>;

/// `(unsatisfiable classes, sub-synset → super-synset edges, synsets)` from a
/// `ClassHierarchy` response.
#[allow(clippy::type_complexity)]
fn hierarchy(el: &El) -> Result<(Synset, Vec<(usize, usize)>, Vec<Synset>), ReasoningError> {
    if el.name != "ClassHierarchy" {
        return Err(ReasoningError::Backend {
            backend: NAME.into(),
            detail: error_text(el)
                .unwrap_or_else(|| format!("expected ClassHierarchy, got <{}>", el.name)),
        });
    }
    let mut synsets: Vec<Synset> = Vec::new();
    let mut index: HashMap<Synset, usize> = HashMap::new();
    let mut id = |s: Synset, synsets: &mut Vec<Synset>| -> usize {
        *index.entry(s.clone()).or_insert_with(|| {
            synsets.push(s);
            synsets.len() - 1
        })
    };
    let mut unsat = Vec::new();
    let mut edges = Vec::new();
    for c in &el.children {
        match c.name.as_str() {
            "ClassSynset" => unsat = sorted(iris_in(c)),
            "ClassSubClassesPair" => {
                let mut parts = c.children.iter();
                let Some(sup) = parts.next() else { continue };
                let sup = id(sorted(iris_in(sup)), &mut synsets);
                for subs in parts {
                    for s in subs.children.iter().filter(|x| x.name == "ClassSynset") {
                        let sub = id(sorted(iris_in(s)), &mut synsets);
                        edges.push((sub, sup));
                    }
                }
            }
            _ => {}
        }
    }
    Ok((unsat, edges, synsets))
}

fn sorted(mut v: Vec<String>) -> Vec<String> {
    v.sort();
    v.dedup();
    v
}

/// Named-class consequences of the hierarchy: equivalences inside a synset,
/// the transitive `rdfs:subClassOf` closure (without `⊑ owl:Thing`), and
/// `C ⊑ owl:Nothing` for each unsatisfiable class.
fn hierarchy_triples(
    unsat: &[String],
    edges: &[(usize, usize)],
    synsets: &[Synset],
) -> Vec<Triple> {
    let mut out = Vec::new();
    for s in synsets {
        for a in s {
            for b in s {
                if a != b {
                    out.push(triple(a, OWL_EQUIVALENT_CLASS, b));
                }
            }
        }
    }
    let mut up: BTreeMap<usize, BTreeSet<usize>> = BTreeMap::new();
    for (sub, sup) in edges {
        up.entry(*sub).or_default().insert(*sup);
    }
    for start in 0..synsets.len() {
        let mut seen = BTreeSet::new();
        let mut stack: Vec<usize> = up
            .get(&start)
            .map(|s| s.iter().copied().collect())
            .unwrap_or_default();
        while let Some(s) = stack.pop() {
            if !seen.insert(s) {
                continue;
            }
            stack.extend(up.get(&s).into_iter().flatten().copied());
        }
        for sup in seen {
            for a in &synsets[start] {
                for b in &synsets[sup] {
                    if b != OWL_THING && a != OWL_NOTHING && a != b {
                        out.push(triple(a, RDFS_SUB_CLASS_OF, b));
                    }
                }
            }
        }
    }
    for c in unsat {
        if c != OWL_NOTHING {
            out.push(triple(c, RDFS_SUB_CLASS_OF, OWL_NOTHING));
        }
    }
    out
}

// ── helpers ──────────────────────────────────────────────────────────────────

fn triple(s: &str, p: &str, o: &str) -> Triple {
    Triple::new(
        NamedNode::new_unchecked(s),
        NamedNode::new_unchecked(p),
        Term::NamedNode(NamedNode::new_unchecked(o)),
    )
}

fn version(log: &str) -> Option<String> {
    log.lines()
        .find_map(|l| l.split("Version ").nth(1))
        .map(|v| format!("Konclude {}", v.split_whitespace().next().unwrap_or(v)))
}

fn xml_attr(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn unescape(s: &str) -> String {
    quick_xml::escape::unescape(s.trim())
        .map(|c| c.into_owned())
        .unwrap_or_else(|_| s.trim().to_string())
}

fn file_iri(path: &Path) -> String {
    let p = path.display().to_string();
    format!("file:{}", p.replace(' ', "%20"))
}

/// Split concatenated `<sparql>` documents (one per query).
fn split_documents(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("<sparql") {
        let after = &rest[start..];
        // An empty answer may be `<sparql …/>`; otherwise up to `</sparql>`.
        let open_end = after.find('>').map(|i| i + 1).unwrap_or(after.len());
        let end = if after[..open_end].ends_with("/>") {
            open_end
        } else {
            after
                .find("</sparql>")
                .map(|i| i + "</sparql>".len())
                .unwrap_or(after.len())
        };
        out.push(&after[..end]);
        rest = &after[end..];
    }
    out
}

/// Why Konclude's answer about a knowledge base may be wrong.
#[derive(Default)]
struct Gaps {
    /// It may say "satisfiable" where the knowledge base is not.
    missed_clash: Vec<&'static str>,
    /// It may say "unsatisfiable" where the knowledge base is satisfiable.
    false_clash: Vec<&'static str>,
}

impl Gaps {
    fn of_complements(writer: &FsWriter) -> Gaps {
        let mut g = Gaps::default();
        if writer.wrote_data_complement() {
            g.missed_clash.push(DATA_COMPLEMENT);
        }
        if writer.wrote_numeric_complement() {
            g.false_clash.push(NUMERIC_COMPLEMENT);
        }
        g
    }

    fn add(&mut self, other: Gaps) {
        for (mine, theirs) in [
            (&mut self.missed_clash, other.missed_clash),
            (&mut self.false_clash, other.false_clash),
        ] {
            for why in theirs {
                if !mine.contains(&why) {
                    mine.push(why);
                }
            }
        }
    }
}

/// One knowledge base, the ontology plus `extra`, as functional syntax for
/// Konclude, and the reasons (if any) not to trust Konclude's answer.
fn write_kb(ont: &Ontology, extra: &[Axiom]) -> (String, Gaps) {
    let mut writer = FsWriter::new();
    let text = writer.ontology(ont, extra);
    let mut gaps = Gaps::of_complements(&writer);
    let axioms = || ont.axioms.iter().chain(extra).map(|a| &a.kind);
    if axioms().any(|k| matches!(k, AxiomKind::DisjointDataProperties(_))) {
        gaps.missed_clash.push(DISJOINT_DATA);
    }
    if axioms().any(|k| matches!(k, AxiomKind::HasKey(..))) {
        gaps.missed_clash.push(KEYS);
    }
    if nominals_inverses_counting(axioms) {
        gaps.missed_clash.push(NOMINAL_COUNTING);
    }
    if uses_float_facets(axioms) {
        gaps.missed_clash.push(FLOAT_FACETS);
    }
    if text.contains(XML_LITERAL) {
        gaps.missed_clash.push(XML_LITERALS);
    }
    (text, gaps)
}

/// Whether a class axiom uses `ObjectOneOf` while the knowledge base also has
/// inverse properties and bounds how many neighbours an individual has
/// (functional or inverse functional properties, cardinality restrictions):
/// the combination that needs a tableau's NN-rule. Konclude v0.7.0 misses
/// clashes there (the `one=two` W3C test case).
fn nominals_inverses_counting<'a, I: Iterator<Item = &'a AxiomKind>>(
    axioms: impl Fn() -> I,
) -> bool {
    let (mut nominal, mut inverse, mut counting) = (false, false, false);
    let inv = |p: &ObjectProp| matches!(p, ObjectProp::Inverse(_));
    for k in axioms() {
        match k {
            AxiomKind::FunctionalObjectProperty(p) => {
                counting = true;
                inverse |= inv(p);
            }
            AxiomKind::InverseFunctionalObjectProperty(_) => {
                counting = true;
                inverse = true;
            }
            AxiomKind::InverseObjectProperties(..) | AxiomKind::SymmetricObjectProperty(_) => {
                inverse = true
            }
            AxiomKind::SubObjectPropertyOf(p, q) => inverse |= inv(p) || inv(q),
            AxiomKind::EquivalentObjectProperties(v) => inverse |= v.iter().any(inv),
            _ => {}
        }
        // Assertions name individuals, not nominal classes.
        let tbox = !matches!(k, AxiomKind::ClassAssertion(..));
        visit_exprs(k, &mut |c| match c {
            ClassExpr::OneOf(_) if tbox => nominal = true,
            ClassExpr::SomeValuesFrom(p, _)
            | ClassExpr::AllValuesFrom(p, _)
            | ClassExpr::HasValue(p, _)
            | ClassExpr::HasSelf(p) => inverse |= inv(p),
            ClassExpr::MinCardinality(_, p, _)
            | ClassExpr::MaxCardinality(_, p, _)
            | ClassExpr::ExactCardinality(_, p, _) => {
                counting = true;
                inverse |= inv(p);
            }
            _ => {}
        });
    }
    nominal && inverse && counting
}

/// Whether a datatype restriction on `xsd:float` or `xsd:double` is in use.
fn uses_float_facets<'a, I: Iterator<Item = &'a AxiomKind>>(axioms: impl Fn() -> I) -> bool {
    fn float(r: &DataRange) -> bool {
        match r {
            DataRange::Restriction(d, _) => d == XSD_FLOAT || d == XSD_DOUBLE,
            DataRange::IntersectionOf(v) | DataRange::UnionOf(v) => v.iter().any(float),
            DataRange::ComplementOf(x) => float(x),
            DataRange::Datatype(_) | DataRange::OneOf(_) => false,
        }
    }
    axioms().any(|k| {
        let mut found = match k {
            AxiomKind::DataPropertyRange(_, r) | AxiomKind::DatatypeDefinition(_, r) => float(r),
            _ => false,
        };
        visit_exprs(k, &mut |c| match c {
            ClassExpr::DataSomeValuesFrom(_, r) | ClassExpr::DataAllValuesFrom(_, r) => {
                found |= float(r)
            }
            ClassExpr::DataMinCardinality(_, _, Some(r))
            | ClassExpr::DataMaxCardinality(_, _, Some(r))
            | ClassExpr::DataExactCardinality(_, _, Some(r)) => found |= float(r),
            _ => {}
        });
        found
    })
}

/// Named individuals in the signature (declared or used in assertions).
fn named_individuals(ont: &Ontology) -> Vec<String> {
    let mut out = BTreeSet::new();
    let mut add = |i: &Individual| {
        if let Individual::Named(n) = i {
            out.insert(n.clone());
        }
    };
    for a in &ont.axioms {
        match &a.kind {
            AxiomKind::Declaration(EntityKind::NamedIndividual, i) => {
                add(&Individual::Named(i.clone()))
            }
            AxiomKind::ClassAssertion(_, i)
            | AxiomKind::DataPropertyAssertion(_, i, _)
            | AxiomKind::NegativeDataPropertyAssertion(_, i, _) => add(i),
            AxiomKind::ObjectPropertyAssertion(_, x, y)
            | AxiomKind::NegativeObjectPropertyAssertion(_, x, y) => {
                add(x);
                add(y);
            }
            AxiomKind::SameIndividual(v) | AxiomKind::DifferentIndividuals(v) => {
                v.iter().for_each(&mut add)
            }
            _ => {}
        }
    }
    out.into_iter().collect()
}

fn fresh(n: usize) -> String {
    format!("urn:ots:dl:fresh:{n}")
}

fn next(n: &mut usize) -> Individual {
    *n += 1;
    Individual::Named(fresh(*n))
}

fn ax(k: AxiomKind) -> Axiom {
    Axiom::new(k)
}

fn not(c: &ClassExpr) -> ClassExpr {
    ClassExpr::ComplementOf(Box::new(c.clone()))
}

fn and(a: ClassExpr, b: ClassExpr) -> ClassExpr {
    ClassExpr::IntersectionOf(vec![a, b])
}

fn thing() -> ClassExpr {
    ClassExpr::Class(OWL_THING.into())
}

/// `O ⊨ α` iff every knowledge base `O ∪ k` (k in the result) is
/// inconsistent. `None`: no reduction for this axiom. An empty list: entailed
/// by every ontology (declarations).
fn negations(k: &AxiomKind, n: &mut usize) -> Option<Vec<Vec<Axiom>>> {
    use AxiomKind::*;
    let pairs = |v: &[ClassExpr]| -> Vec<(ClassExpr, ClassExpr)> {
        let mut out = Vec::new();
        for i in 0..v.len() {
            for j in 0..v.len() {
                if i != j {
                    out.push((v[i].clone(), v[j].clone()));
                }
            }
        }
        out
    };
    let named = |i: &Individual| matches!(i, Individual::Named(_));
    Some(match k {
        Declaration(..) => vec![],
        SubClassOf(c, d) => vec![vec![ax(ClassAssertion(and(c.clone(), not(d)), next(n)))]],
        EquivalentClasses(v) => pairs(v)
            .into_iter()
            .map(|(c, d)| vec![ax(ClassAssertion(and(c, not(&d)), next(n)))])
            .collect(),
        DisjointClasses(v) => pairs(v)
            .into_iter()
            .map(|(c, d)| vec![ax(ClassAssertion(and(c, d), next(n)))])
            .collect(),
        DisjointUnion(c, v) => {
            let mut out = negations(
                &EquivalentClasses(vec![
                    ClassExpr::Class(c.clone()),
                    ClassExpr::UnionOf(v.clone()),
                ]),
                n,
            )?;
            out.extend(negations(&DisjointClasses(v.clone()), n)?);
            out
        }
        ClassAssertion(c, i) if named(i) => vec![vec![ax(ClassAssertion(not(c), i.clone()))]],
        ObjectPropertyAssertion(p, a, b) if named(a) && named(b) => vec![vec![ax(ClassAssertion(
            ClassExpr::AllValuesFrom(p.clone(), Box::new(not(&ClassExpr::OneOf(vec![b.clone()])))),
            a.clone(),
        ))]],
        NegativeObjectPropertyAssertion(p, a, b) if named(a) && named(b) => {
            vec![vec![ax(ObjectPropertyAssertion(
                p.clone(),
                a.clone(),
                b.clone(),
            ))]]
        }
        DataPropertyAssertion(p, a, v) if named(a) => vec![vec![ax(ClassAssertion(
            ClassExpr::DataAllValuesFrom(
                vec![p.clone()],
                DataRange::ComplementOf(Box::new(DataRange::OneOf(vec![v.clone()]))),
            ),
            a.clone(),
        ))]],
        NegativeDataPropertyAssertion(p, a, v) if named(a) => {
            vec![vec![ax(DataPropertyAssertion(
                p.clone(),
                a.clone(),
                v.clone(),
            ))]]
        }
        SameIndividual(v) if v.iter().all(named) => {
            let mut out = Vec::new();
            for i in 0..v.len() {
                for j in i + 1..v.len() {
                    out.push(vec![ax(DifferentIndividuals(vec![
                        v[i].clone(),
                        v[j].clone(),
                    ]))]);
                }
            }
            out
        }
        DifferentIndividuals(v) if v.iter().all(named) => {
            let mut out = Vec::new();
            for i in 0..v.len() {
                for j in i + 1..v.len() {
                    out.push(vec![ax(SameIndividual(vec![v[i].clone(), v[j].clone()]))]);
                }
            }
            out
        }
        SubObjectPropertyOf(p, q) => {
            let (a, b) = (next(n), next(n));
            vec![vec![
                ax(ObjectPropertyAssertion(p.clone(), a.clone(), b.clone())),
                ax(NegativeObjectPropertyAssertion(q.clone(), a, b)),
            ]]
        }
        EquivalentObjectProperties(v) => {
            let mut out = Vec::new();
            for p in v {
                for q in v {
                    if p != q {
                        out.extend(negations(&SubObjectPropertyOf(p.clone(), q.clone()), n)?);
                    }
                }
            }
            out
        }
        InverseObjectProperties(p, q) => {
            let mut out = negations(&SubObjectPropertyOf(p.clone(), q.inverse()), n)?;
            out.extend(negations(&SubObjectPropertyOf(q.inverse(), p.clone()), n)?);
            out
        }
        SubObjectPropertyChain(chain, q) => {
            let nodes: Vec<Individual> = (0..=chain.len()).map(|_| next(n)).collect();
            let mut kb: Vec<Axiom> = chain
                .iter()
                .enumerate()
                .map(|(i, p)| {
                    ax(ObjectPropertyAssertion(
                        p.clone(),
                        nodes[i].clone(),
                        nodes[i + 1].clone(),
                    ))
                })
                .collect();
            kb.push(ax(NegativeObjectPropertyAssertion(
                q.clone(),
                nodes[0].clone(),
                nodes[chain.len()].clone(),
            )));
            vec![kb]
        }
        ObjectPropertyDomain(p, c) => vec![vec![ax(ClassAssertion(
            and(
                ClassExpr::SomeValuesFrom(p.clone(), Box::new(thing())),
                not(c),
            ),
            next(n),
        ))]],
        ObjectPropertyRange(p, c) => vec![vec![ax(ClassAssertion(
            ClassExpr::SomeValuesFrom(p.clone(), Box::new(not(c))),
            next(n),
        ))]],
        DataPropertyDomain(p, c) => vec![vec![ax(ClassAssertion(
            and(
                ClassExpr::DataSomeValuesFrom(
                    vec![p.clone()],
                    DataRange::Datatype(RDFS_LITERAL.into()),
                ),
                not(c),
            ),
            next(n),
        ))]],
        DataPropertyRange(p, r) => vec![vec![ax(ClassAssertion(
            ClassExpr::DataSomeValuesFrom(
                vec![p.clone()],
                DataRange::ComplementOf(Box::new(r.clone())),
            ),
            next(n),
        ))]],
        FunctionalObjectProperty(p) => vec![vec![ax(ClassAssertion(
            ClassExpr::MinCardinality(2, p.clone(), None),
            next(n),
        ))]],
        InverseFunctionalObjectProperty(p) => vec![vec![ax(ClassAssertion(
            ClassExpr::MinCardinality(2, p.inverse(), None),
            next(n),
        ))]],
        FunctionalDataProperty(p) => vec![vec![ax(ClassAssertion(
            ClassExpr::DataMinCardinality(2, p.clone(), None),
            next(n),
        ))]],
        ReflexiveObjectProperty(p) => vec![vec![ax(ClassAssertion(
            not(&ClassExpr::HasSelf(p.clone())),
            next(n),
        ))]],
        IrreflexiveObjectProperty(p) => vec![vec![ax(ClassAssertion(
            ClassExpr::HasSelf(p.clone()),
            next(n),
        ))]],
        SymmetricObjectProperty(p) => {
            let (a, b) = (next(n), next(n));
            vec![vec![
                ax(ObjectPropertyAssertion(p.clone(), a.clone(), b.clone())),
                ax(NegativeObjectPropertyAssertion(p.clone(), b, a)),
            ]]
        }
        AsymmetricObjectProperty(p) => {
            let (a, b) = (next(n), next(n));
            vec![vec![
                ax(ObjectPropertyAssertion(p.clone(), a.clone(), b.clone())),
                ax(ObjectPropertyAssertion(p.clone(), b, a)),
            ]]
        }
        TransitiveObjectProperty(p) => {
            let (a, b, c) = (next(n), next(n), next(n));
            vec![vec![
                ax(ObjectPropertyAssertion(p.clone(), a.clone(), b.clone())),
                ax(ObjectPropertyAssertion(p.clone(), b, c.clone())),
                ax(NegativeObjectPropertyAssertion(p.clone(), a, c)),
            ]]
        }
        DisjointObjectProperties(v) => {
            let mut out = Vec::new();
            for i in 0..v.len() {
                for j in i + 1..v.len() {
                    let (a, b) = (next(n), next(n));
                    out.push(vec![
                        ax(ObjectPropertyAssertion(v[i].clone(), a.clone(), b.clone())),
                        ax(ObjectPropertyAssertion(v[j].clone(), a, b)),
                    ]);
                }
            }
            out
        }
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hierarchy_closure_skips_thing_and_marks_unsatisfiable() {
        let synsets = vec![
            vec![OWL_THING.to_string()],
            vec!["http://e/B".to_string()],
            vec!["http://e/A".to_string(), "http://e/A2".to_string()],
        ];
        let edges = vec![(1, 0), (2, 1)];
        let unsat = vec![OWL_NOTHING.to_string(), "http://e/D".to_string()];
        let ts: Vec<String> = hierarchy_triples(&unsat, &edges, &synsets)
            .iter()
            .map(|t| t.to_string())
            .collect();
        assert!(ts.contains(
            &"<http://e/A> <http://www.w3.org/2000/01/rdf-schema#subClassOf> <http://e/B>"
                .to_string()
        ));
        assert!(ts.contains(
            &"<http://e/A2> <http://www.w3.org/2000/01/rdf-schema#subClassOf> <http://e/B>"
                .to_string()
        ));
        assert!(ts.contains(
            &"<http://e/A> <http://www.w3.org/2002/07/owl#equivalentClass> <http://e/A2>"
                .to_string()
        ));
        assert!(ts.contains(&"<http://e/D> <http://www.w3.org/2000/01/rdf-schema#subClassOf> <http://www.w3.org/2002/07/owl#Nothing>".to_string()));
        assert!(!ts.iter().any(|t| t.ends_with("#Thing>")), "{ts:?}");
    }

    #[test]
    fn gaps_name_what_konclude_gets_wrong() {
        let p = || ObjectProp::Named("http://e/p".into());
        let a = || Individual::Named("http://e/a".into());
        let class = |i: &str| ClassExpr::Class(format!("http://e/{i}"));
        let dt = |d: &str| DataRange::Datatype(format!("{XSD}{d}"));
        let gaps = |kinds: Vec<AxiomKind>, extra: Vec<AxiomKind>| {
            let ont = Ontology {
                axioms: kinds.into_iter().map(ax).collect(),
                ..Default::default()
            };
            let extra: Vec<Axiom> = extra.into_iter().map(ax).collect();
            write_kb(&ont, &extra).1
        };
        let nominal =
            || AxiomKind::EquivalentClasses(vec![class("A"), ClassExpr::OneOf(vec![a()])]);
        // Nothing to doubt.
        let g = gaps(
            vec![
                nominal(),
                AxiomKind::FunctionalObjectProperty(p()),
                AxiomKind::AsymmetricObjectProperty(p()),
                AxiomKind::DataPropertyRange("http://e/d".into(), dt("integer")),
            ],
            vec![],
        );
        assert!(g.missed_clash.is_empty() && g.false_clash.is_empty());
        // Each reason for a missed clash, from the ontology or the check.
        for (kinds, extra, why) in [
            (
                vec![AxiomKind::DisjointDataProperties(vec![
                    "http://e/d".into(),
                    "http://e/e".into(),
                ])],
                vec![],
                DISJOINT_DATA,
            ),
            (
                vec![AxiomKind::HasKey(thing(), vec![p()], vec![])],
                vec![],
                KEYS,
            ),
            (
                vec![nominal(), AxiomKind::InverseFunctionalObjectProperty(p())],
                vec![],
                NOMINAL_COUNTING,
            ),
            (
                vec![nominal()],
                vec![AxiomKind::ClassAssertion(
                    ClassExpr::MaxCardinality(1, p().inverse(), None),
                    a(),
                )],
                NOMINAL_COUNTING,
            ),
            (
                vec![AxiomKind::DataPropertyRange(
                    "http://e/d".into(),
                    DataRange::Restriction(
                        XSD_FLOAT.into(),
                        vec![(
                            format!("{XSD}minExclusive"),
                            Literal {
                                lexical: "0".into(),
                                datatype: XSD_FLOAT.into(),
                                lang: None,
                            },
                        )],
                    ),
                )],
                vec![],
                FLOAT_FACETS,
            ),
            (
                vec![AxiomKind::DataPropertyAssertion(
                    "http://e/d".into(),
                    a(),
                    Literal {
                        lexical: "<b/>".into(),
                        datatype: XML_LITERAL.into(),
                        lang: None,
                    },
                )],
                vec![],
                XML_LITERALS,
            ),
        ] {
            let g = gaps(kinds, extra);
            assert_eq!(g.missed_clash, vec![why]);
            assert!(g.false_clash.is_empty(), "{why}");
        }
        // A complement of a number datatype doubts both ways; of a string
        // datatype or of a value list, only "satisfiable".
        let range = |r: DataRange| {
            vec![AxiomKind::DataPropertyRange(
                "http://e/d".into(),
                DataRange::ComplementOf(Box::new(r)),
            )]
        };
        let g = gaps(range(dt("positiveInteger")), vec![]);
        assert_eq!(
            (g.missed_clash, g.false_clash),
            (vec![DATA_COMPLEMENT], vec![NUMERIC_COMPLEMENT])
        );
        for r in [
            dt("string"),
            DataRange::OneOf(vec![Literal {
                lexical: "1".into(),
                datatype: format!("{XSD}integer"),
                lang: None,
            }]),
        ] {
            let g = gaps(range(r), vec![]);
            assert_eq!(g.missed_clash, vec![DATA_COMPLEMENT]);
            assert!(g.false_clash.is_empty());
        }
    }

    #[test]
    fn concatenated_sparql_documents_split() {
        let text = "<?xml version=\"1.0\"?>\n<sparql a=\"b\">\n<results></results>\n</sparql>\n\n<?xml version=\"1.0\"?>\n<sparql x=\"y\">\n</sparql>";
        assert_eq!(split_documents(text).len(), 2);
    }

    #[test]
    fn version_is_read_from_the_log() {
        let log = "{info} >> Reasoner for the SROIQV(D) Description Logic, 64-bit, Version v0.7.0-1138 - 500e11d9 (Jun 18 2021)";
        assert_eq!(version(log).as_deref(), Some("Konclude v0.7.0-1138"));
    }
}
