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
use crate::spec_import::ids::{parse_xml, El};

const NAME: &str = "konclude";
const KB: &str = "urn:ots:kb";
const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const RDFS_SUB_CLASS_OF: &str = "http://www.w3.org/2000/01/rdf-schema#subClassOf";
const OWL_EQUIVALENT_CLASS: &str = "http://www.w3.org/2002/07/owl#equivalentClass";
const OWL_SAME_AS: &str = "http://www.w3.org/2002/07/owl#sameAs";
/// Entailment sub-checks per request before the answer is `unknown`.
const MAX_SUBCHECKS: usize = 64;

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
                        return Err(ReasoningError::Backend {
                            backend: NAME.into(),
                            detail: format!("exited with {status}: {tail}"),
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
        let log = self.run(
            &[
                "owllinkfile".into(),
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
        let text = FsWriter::new().ontology(&input.ontology, &[]);
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
        out.inferred
            .extend(self.property_assertions(&dir.0, &input.ontology, deadline)?);
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
                for a in &mapped.ontology.axioms {
                    match negations(&a.kind, &mut n) {
                        Some(subs) => kbs.extend(subs),
                        None => unknown = true,
                    }
                }
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
        let premise = FsWriter::new().ontology(&input.ontology, &[]);
        let mut req = Self::load(&dir.0, "premise.ofn", &premise, KB)?;
        req.push_str(&format!(
            "<IsKBSatisfiable kb=\"{KB}\"/>\n<ReleaseKB kb=\"{KB}\"/>\n"
        ));
        let mut extra_kbs = Vec::new();
        for (k, extra) in kbs.iter().enumerate().filter(|(_, e)| !e.is_empty()) {
            let kb = format!("{KB}:{k}");
            let text = FsWriter::new().ontology(&input.ontology, extra);
            req.push_str(&Self::load(&dir.0, &format!("check-{k}.ofn"), &text, &kb)?);
            req.push_str(&format!(
                "<IsKBSatisfiable kb=\"{kb}\"/>\n<ReleaseKB kb=\"{kb}\"/>\n"
            ));
            extra_kbs.push(k);
        }
        let (resp, version) = self.owllink(&dir.0, &req, deadline)?;
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
    el.attrs
        .iter()
        .find(|(a, _)| a == k)
        .map(|(_, v)| v.as_str())
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
