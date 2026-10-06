//! Runner for the shexTest suite, vendored under `tests/fixtures/shextest`
//! (see PROVENANCE.md and LICENSE there). Its results are development and
//! regression results, not a conformance claim.
//!
//! Four sections, each driven by its upstream JSON-LD manifest (read as plain
//! JSON):
//!
//!   - `validation/` — `sht:ValidationTest` must conform, `sht:ValidationFailure`
//!     must not (the suite's "logic-conformant" level). The schema is parsed,
//!     its imports resolved from the vendored files, its requirements checked,
//!     and the focus node (or the shape map) validated against the data parsed
//!     with its labels and lexical forms intact. External semantic-action code
//!     and EXTERNAL shape definitions come from the files the test names.
//!   - `schemas/` — `sht:RepresentationTest`: the ShExC and ShExJ forms parse
//!     to the same schema, and so does the ShExR form where there is one
//!     (blank-node labels compared up to renaming).
//!   - `negativeSyntax/` — the ShExC parser must reject the document.
//!   - `negativeStructure/` — parsing or the schema requirements must reject it.
//!
//! Tests carrying a ShEx 2.next trait (`Extends`, `MultiExtends`,
//! `ExtendsDiamond`, `Abstract`) are skipped: ShEx 2.1 has no EXTENDS or
//! ABSTRACT. They are the runner-side skips in the baseline below.
//!
//! A separate test runs every validation case a second time through the
//! store (`TripleStore`, a graph-scoped `validate_in`, as the HTTP routes do).
//! The store keeps typed literals exactly as written (vendor/README.md), so
//! every case answers there as it does on the parsed data: `STORE_DIVERGENCES`
//! is empty. It listed 40 lexical-form cases before the store kept literals
//! as written.
//!
//! Gap policy (two-way ratchet): every test not in `KNOWN_FAILURES` must pass,
//! and every listed test must still fail — so silent regressions *and* silent
//! fixes both turn the suite red, keeping the list honest. The same holds for
//! `STORE_DIVERGENCES`.

#![cfg(feature = "shex")]

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use open_triplestore::shex::ast::{Label, Schema, SemAct, ShapeExpr};
use open_triplestore::shex::check::{self, ImportResolver};
use open_triplestore::shex::{
    shexc, shexj, shexr, validate_on_large_stack, Engine, GraphData, GraphScope, ShapeSel,
};
use open_triplestore::store::TripleStore;
use oxigraph::io::{RdfFormat, RdfParser};
use oxigraph::model::{BlankNode, Graph, GraphName, Literal, NamedNode, Quad, Term};
use serde_json::Value;

const FIXTURES: &str = "tests/fixtures/shextest";
const UPSTREAM: &str = "https://raw.githubusercontent.com/shexSpec/shexTest/master/";

/// The ShEx 2.next traits; tests carrying one are not ShEx 2.1 tests.
const NEXT_TRAITS: &[&str] = &["Extends", "MultiExtends", "ExtendsDiamond", "Abstract"];

/// Representation tests of ShEx 2.next schemas (`EXTENDS` / `ABSTRACT`) that
/// upstream does not tag with a 2.next trait. Skipped like the tagged ones;
/// `only_shex_next_tests_are_skipped` checks each one's ShExC uses a 2.next
/// keyword.
const NEXT_UNTAGGED: &[&str] = &[
    "schemas/1dot2SameExtends",
    "schemas/1dotExtendsBnode",
    "schemas/1dotExtendsInverse",
    "schemas/1dotInverseExtends",
    "schemas/extends-abstract-closed-diamond",
    "schemas/extends-abstract-or",
];

/// Tests that currently fail, with the gap they sit behind. Keep sorted.
/// Removing an entry requires the test to actually pass (the ratchet asserts
/// both directions). Keys are `<section>/<test name>`.
///
/// Empirical baseline: 1795 pass / 0 known-fail / 122 aux skips
const KNOWN_FAILURES: &[(&str, &str)] = &[];

/// Validation cases whose answer through the store differs from the
/// engine's answer on the parsed data, and why. Keep sorted.
const STORE_DIVERGENCES: &[(&str, &str)] = &[];

/// Pass floors per section (validation, schemas, negativeSyntax,
/// negativeStructure).
const FLOORS: [usize; 4] = [1209, 462, 105, 19];

#[derive(Debug, PartialEq)]
enum Outcome {
    Pass,
    Fail(String),
    Skip,
}

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURES)
}

/// The vendored file an upstream IRI names.
fn file_for(iri: &str) -> Option<PathBuf> {
    iri.strip_prefix(UPSTREAM).map(|rel| fixtures().join(rel))
}

fn read(iri: &str) -> Result<String, String> {
    let path = file_for(iri).ok_or_else(|| format!("{iri} is not in the suite"))?;
    std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))
}

fn resolve(base: &str, rel: &str) -> String {
    shexc::resolve_iri(Some(base), rel).unwrap()
}

fn entries(section: &str) -> (String, Vec<Value>) {
    let text = std::fs::read_to_string(fixtures().join(section).join("manifest.jsonld")).unwrap();
    let m: Value = serde_json::from_str(&text).unwrap();
    let g = m.get("@graph").and_then(|g| g.get(0)).unwrap_or(&m);
    let base = format!("{UPSTREAM}{section}/manifest");
    (base, g["entries"].as_array().unwrap().clone())
}

fn traits(e: &Value) -> Vec<String> {
    match &e["trait"] {
        Value::String(s) => vec![s.clone()],
        Value::Array(a) => a
            .iter()
            .filter_map(|t| t.as_str().map(str::to_string))
            .collect(),
        _ => vec![],
    }
}

fn is_next(e: &Value) -> bool {
    traits(e).iter().any(|t| NEXT_TRAITS.contains(&t.as_str()))
}

/// Parse a schema file by its extension, with its own IRI as base.
fn parse_schema_file(iri: &str) -> Result<Schema, String> {
    let text = read(iri)?;
    if iri.ends_with(".json") {
        shexj::parse(&text, Some(iri))
    } else if iri.ends_with(".ttl") {
        let g = parse_graph(&text, iri)?;
        shexr::from_graph(&g)
    } else {
        shexc::parse(&text, Some(iri))
    }
}

/// `IMPORT <x>` names a schema by IRI without extension; the suite holds it
/// as `x.shex` (or `x.json`).
struct SuiteImports;

impl ImportResolver for SuiteImports {
    fn resolve(&self, iri: &str) -> Result<Schema, String> {
        for ext in [".shex", ".json"] {
            let candidate = format!("{iri}{ext}");
            if file_for(&candidate).is_some_and(|p| p.exists()) {
                return parse_schema_file(&candidate);
            }
        }
        Err(format!("no schema file for <{iri}>"))
    }
}

fn strip_ext(iri: &str) -> &str {
    for ext in [".shex", ".json", ".ttl"] {
        if let Some(s) = iri.strip_suffix(ext) {
            return s;
        }
    }
    iri
}

fn parse_graph(text: &str, base: &str) -> Result<Graph, String> {
    let mut g = Graph::new();
    // Lenient: the suite's data uses language tags the Turtle grammar
    // allows but BCP 47 well-formedness checks reject (`@fr-be-fbcl`).
    let parser = RdfParser::from_format(RdfFormat::Turtle)
        .with_base_iri(base)
        .map_err(|e| e.to_string())?
        .lenient();
    for q in parser.for_slice(text.as_bytes()) {
        let q = q.map_err(|e| format!("data: {e}"))?;
        g.insert(&oxigraph::model::Triple::from(q));
    }
    Ok(g)
}

/// A manifest node value (`focus`, map `node`) as a term.
fn term(v: &Value, base: &str) -> Result<Term, String> {
    match v {
        Value::String(s) => Ok(match s.strip_prefix("_:") {
            Some(b) => BlankNode::new(b).map_err(|e| e.to_string())?.into(),
            None => NamedNode::new(resolve(base, s))
                .map_err(|e| e.to_string())?
                .into(),
        }),
        Value::Object(o) => {
            let value = o["@value"].as_str().ok_or("literal without @value")?;
            if let Some(lang) = o.get("@language").and_then(Value::as_str) {
                return Ok(Literal::new_language_tagged_literal(value, lang)
                    .map_err(|e| e.to_string())?
                    .into());
            }
            Ok(match o.get("@type").and_then(Value::as_str) {
                Some(t) => Literal::new_typed_literal(
                    value,
                    NamedNode::new(resolve(base, t)).map_err(|e| e.to_string())?,
                )
                .into(),
                None => Literal::new_simple_literal(value).into(),
            })
        }
        other => Err(format!("bad node {other}")),
    }
}

fn shape_sel(v: Option<&Value>, base: &str) -> ShapeSel {
    match v.and_then(Value::as_str) {
        None => ShapeSel::Start,
        Some(s) => ShapeSel::Label(match s.strip_prefix("_:") {
            Some(b) => Label::BNode(b.to_string()),
            None => Label::Iri(resolve(base, s)),
        }),
    }
}

/// What a validation case needs, prepared once for both runs.
struct Case {
    schema: check::ResolvedSchema,
    data: Graph,
    data_iri: String,
    /// `(node, shape, expected conformance)`.
    checks: Vec<(Term, ShapeSel, bool)>,
    externs: HashMap<Label, ShapeExpr>,
    act_code: HashMap<String, String>,
}

fn prepare(e: &Value, base: &str) -> Result<Case, String> {
    let a = &e["action"];
    let expect = e["@type"].as_str() == Some("sht:ValidationTest");
    let schema_iri = resolve(base, a["schema"].as_str().ok_or("no schema")?);
    let schema = parse_schema_file(&schema_iri)?;
    let schema = check::resolve(schema, Some(strip_ext(&schema_iri)), &SuiteImports)?;
    let data_iri = resolve(base, a["data"].as_str().ok_or("no data")?);
    let data = parse_graph(&read(&data_iri)?, &data_iri)?;
    let mut checks = Vec::new();
    if let Some(map) = a.get("map").and_then(Value::as_str) {
        let map_iri = resolve(base, map);
        let pairs: Value = serde_json::from_str(&read(&map_iri)?).map_err(|e| e.to_string())?;
        let result_iri = resolve(
            base,
            e["result"].as_str().ok_or("a map test has no result")?,
        );
        let results: Value =
            serde_json::from_str(&read(&result_iri)?).map_err(|e| e.to_string())?;
        for p in pairs.as_array().ok_or("map is not an array")? {
            let node = term(&p["node"], &map_iri)?;
            let shape = shape_sel(p.get("shape"), &map_iri);
            let key = p["node"].as_str().unwrap_or_default();
            let want = results[key]
                .as_array()
                .and_then(|rs| {
                    rs.iter()
                        .find(|r| r["shape"] == p["shape"])
                        .and_then(|r| r["result"].as_bool())
                })
                .ok_or_else(|| format!("no expected result for {key}"))?;
            checks.push((node, shape, want));
        }
    } else {
        let focus = term(a.get("focus").ok_or("no focus")?, base)?;
        checks.push((focus, shape_sel(a.get("shape"), base), expect));
    }
    let mut externs = HashMap::new();
    if let Some(x) = a.get("shapeExterns").and_then(Value::as_str) {
        let x = resolve(base, x);
        for d in parse_schema_file(&x)?.shapes {
            externs.insert(d.label, d.expr);
        }
    }
    let mut act_code = HashMap::new();
    if let Some(s) = a.get("semActs").and_then(Value::as_str) {
        let s = resolve(base, s);
        let acts: Vec<SemAct> = shexc::parse(&read(&s)?, Some(&s))?.start_acts;
        for act in acts {
            if let Some(code) = act.code {
                act_code.insert(act.name, code);
            }
        }
    }
    Ok(Case {
        schema,
        data,
        data_iri,
        checks,
        externs,
        act_code,
    })
}

fn run_case(case: &Case, data: &dyn open_triplestore::shex::Data) -> Outcome {
    let mut engine = Engine::new(&case.schema, data)
        .with_externs(case.externs.clone())
        .with_action_code(case.act_code.clone());
    for (node, shape, want) in &case.checks {
        let got = engine.validate(node, shape);
        if got.is_ok() != *want {
            return Outcome::Fail(format!(
                "{node} @ {shape:?}: expected {}, got {got:?}",
                if *want { "conformant" } else { "nonconformant" }
            ));
        }
    }
    Outcome::Pass
}

fn validation_case(e: &Value, base: &str) -> Outcome {
    if is_next(e) {
        return Outcome::Skip;
    }
    if std::env::var_os("SHEXTEST_TRACE").is_some() {
        eprintln!("case {}", e["name"]);
    }
    match prepare(e, base) {
        Err(err) => Outcome::Fail(format!("setup: {err}")),
        // As the server does: validation recurses through the data.
        Ok(case) => validate_on_large_stack(|| run_case(&case, &GraphData(&case.data))),
    }
}

/// The same case through a store, as the server reads it.
fn store_case(e: &Value, base: &str) -> Outcome {
    if is_next(e) {
        return Outcome::Skip;
    }
    let Ok(case) = prepare(e, base) else {
        return Outcome::Skip;
    };
    let store = TripleStore::in_memory().unwrap();
    let g = NamedNode::new(format!("{}#graph", case.data_iri)).unwrap();
    let quads: Vec<Quad> = case
        .data
        .iter()
        .map(|t| t.into_owned().in_graph(GraphName::NamedNode(g.clone())))
        .collect();
    store.insert_quads(quads).unwrap();
    let scope = GraphScope::Graphs(vec![g]);
    validate_on_large_stack(|| {
        let data = open_triplestore::shex::StoreData {
            store: &store,
            scope: &scope,
        };
        run_case(&case, &data)
    })
}

/// Relabel blank nodes in order of first appearance, so two parses of one
/// schema compare equal (`PartialEq`, numbers by value) whatever labels
/// their parsers chose.
fn canonical(s: &Schema) -> Schema {
    use open_triplestore::shex::ast::{ShapeExpr as SE, TripleExpr};
    struct Relabel(HashMap<String, String>);
    impl Relabel {
        fn label(&mut self, l: &mut Label) {
            if let Label::BNode(b) = l {
                let n = self.0.len();
                *b = self
                    .0
                    .entry(b.clone())
                    .or_insert_with(|| format!("b{n}"))
                    .clone();
            }
        }
        fn se(&mut self, e: &mut SE) {
            match e {
                SE::Or(v) | SE::And(v) => v.iter_mut().for_each(|x| self.se(x)),
                SE::Not(x) => self.se(x),
                SE::Ref(l) => self.label(l),
                SE::Shape(sh) => {
                    if let Some(t) = &mut sh.expression {
                        self.te(t);
                    }
                }
                SE::NodeConstraint(_) | SE::External => {}
            }
        }
        fn te(&mut self, t: &mut TripleExpr) {
            match t {
                TripleExpr::EachOf(g) | TripleExpr::OneOf(g) => {
                    if let Some(l) = &mut g.id {
                        self.label(l);
                    }
                    g.expressions.iter_mut().for_each(|x| self.te(x));
                }
                TripleExpr::TripleConstraint(tc) => {
                    if let Some(l) = &mut tc.id {
                        self.label(l);
                    }
                    if let Some(v) = &mut tc.value_expr {
                        self.se(v);
                    }
                }
                TripleExpr::Ref(l) => self.label(l),
            }
        }
    }
    let mut out = s.clone();
    for d in &mut out.shapes {
        sort_extra(&mut d.expr);
    }
    let mut r = Relabel(HashMap::new());
    for d in &mut out.shapes {
        r.label(&mut d.label);
        r.se(&mut d.expr);
    }
    if let Some(st) = &mut out.start {
        r.se(st);
    }
    out
}

/// `EXTRA` is a set: ShExR's `sx:extra` values come in no order.
fn sort_extra(e: &mut ShapeExpr) {
    use open_triplestore::shex::ast::TripleExpr;
    fn te(t: &mut TripleExpr) {
        match t {
            TripleExpr::EachOf(g) | TripleExpr::OneOf(g) => g.expressions.iter_mut().for_each(te),
            TripleExpr::TripleConstraint(tc) => {
                if let Some(v) = &mut tc.value_expr {
                    sort_extra(v);
                }
            }
            TripleExpr::Ref(_) => {}
        }
    }
    match e {
        ShapeExpr::Or(v) | ShapeExpr::And(v) => v.iter_mut().for_each(sort_extra),
        ShapeExpr::Not(x) => sort_extra(x),
        ShapeExpr::Shape(sh) => {
            sh.extra.sort();
            if let Some(t) = &mut sh.expression {
                te(t);
            }
        }
        _ => {}
    }
}

/// ShExR cannot tell a labelled triple expression's definition (`$l`) from an
/// inclusion of it (`&l`), so ShExR is compared with every inclusion replaced
/// by what it includes and triple-expression labels dropped: the same
/// schema, semantically.
fn expanded(s: &Schema) -> Schema {
    use open_triplestore::shex::ast::TripleExpr;
    let mut defs: HashMap<Label, TripleExpr> = HashMap::new();
    fn collect(t: &TripleExpr, defs: &mut HashMap<Label, TripleExpr>) {
        if let Some(id) = t.id() {
            defs.insert(id.clone(), t.clone());
        }
        if let TripleExpr::EachOf(g) | TripleExpr::OneOf(g) = t {
            g.expressions.iter().for_each(|x| collect(x, defs));
        }
    }
    fn se_tes(e: &ShapeExpr, defs: &mut HashMap<Label, TripleExpr>) {
        match e {
            ShapeExpr::Or(v) | ShapeExpr::And(v) => v.iter().for_each(|x| se_tes(x, defs)),
            ShapeExpr::Not(x) => se_tes(x, defs),
            ShapeExpr::Shape(sh) => {
                if let Some(t) = &sh.expression {
                    collect(t, defs);
                }
            }
            _ => {}
        }
    }
    for d in &s.shapes {
        se_tes(&d.expr, &mut defs);
    }
    fn exp_te(t: &mut TripleExpr, defs: &HashMap<Label, TripleExpr>, depth: usize) {
        if let TripleExpr::Ref(l) = t {
            if let (Some(d), true) = (defs.get(l), depth < 8) {
                *t = d.clone();
            }
        }
        match t {
            TripleExpr::EachOf(g) | TripleExpr::OneOf(g) => {
                g.id = None;
                g.expressions
                    .iter_mut()
                    .for_each(|x| exp_te(x, defs, depth + 1));
            }
            TripleExpr::TripleConstraint(tc) => tc.id = None,
            TripleExpr::Ref(_) => {}
        }
    }
    fn exp_se(e: &mut ShapeExpr, defs: &HashMap<Label, TripleExpr>) {
        match e {
            ShapeExpr::Or(v) | ShapeExpr::And(v) => v.iter_mut().for_each(|x| exp_se(x, defs)),
            ShapeExpr::Not(x) => exp_se(x, defs),
            ShapeExpr::Shape(sh) => {
                if let Some(t) = &mut sh.expression {
                    exp_te(t, defs, 0);
                }
            }
            _ => {}
        }
    }
    let mut out = s.clone();
    for d in &mut out.shapes {
        exp_se(&mut d.expr, &defs);
    }
    if let Some(st) = &mut out.start {
        exp_se(st, &defs);
    }
    out
}

fn representation_case(e: &Value, base: &str) -> Outcome {
    let key = format!("schemas/{}", e["name"].as_str().unwrap_or_default());
    if is_next(e) || NEXT_UNTAGGED.contains(&key.as_str()) {
        return Outcome::Skip;
    }
    let shex_iri = resolve(base, e["shex"].as_str().unwrap_or_default());
    let c = match parse_schema_file(&shex_iri) {
        Ok(s) => s,
        Err(err) => return Outcome::Fail(format!("ShExC: {err}")),
    };
    if let Some(j) = e["json"].as_str() {
        let json_iri = resolve(base, j);
        let text = match read(&json_iri) {
            Ok(t) => t,
            Err(err) => return Outcome::Fail(err),
        };
        // Relative IRIs in the ShExJ resolve against the ShExC document.
        match shexj::parse(&text, Some(&shex_iri)) {
            Ok(js) if canonical(&js) == canonical(&c) => {}
            Ok(js) => {
                return Outcome::Fail(format!(
                    "ShExC and ShExJ differ:\n  C: {:?}\n  J: {:?}",
                    canonical(&c).shapes,
                    canonical(&js).shapes
                ))
            }
            Err(err) => return Outcome::Fail(format!("ShExJ: {err}")),
        }
    }
    if let Some(t) = e["ttl"].as_str() {
        let ttl_iri = resolve(base, t);
        let parsed = read(&ttl_iri)
            .and_then(|text| parse_graph(&text, &shex_iri))
            .and_then(|g| shexr::from_graph(&g));
        match parsed {
            Ok(r) if canonical(&expanded(&r)) == canonical(&expanded(&c)) => {}
            Ok(r) => {
                return Outcome::Fail(format!(
                    "ShExC and ShExR differ:\n  C: {:?}\n  R: {:?}",
                    canonical(&expanded(&c)).shapes,
                    canonical(&expanded(&r)).shapes
                ))
            }
            Err(err) => return Outcome::Fail(format!("ShExR: {err}")),
        }
    }
    Outcome::Pass
}

fn negative_syntax_case(e: &Value, base: &str) -> Outcome {
    let iri = resolve(base, e["shex"].as_str().unwrap_or_default());
    match read(&iri).map(|t| shexc::parse(&t, Some(&iri))) {
        Ok(Err(_)) => Outcome::Pass,
        Ok(Ok(_)) => Outcome::Fail("parsed, but must be rejected".into()),
        Err(err) => Outcome::Fail(err),
    }
}

fn negative_structure_case(e: &Value, base: &str) -> Outcome {
    let iri = resolve(base, e["shex"].as_str().unwrap_or_default());
    let text = match read(&iri) {
        Ok(t) => t,
        Err(err) => return Outcome::Fail(err),
    };
    match shexc::parse(&text, Some(&iri))
        .and_then(|s| check::resolve(s, Some(strip_ext(&iri)), &SuiteImports))
    {
        Err(_) => Outcome::Pass,
        Ok(_) => Outcome::Fail("accepted, but breaks a schema requirement".into()),
    }
}

/// Run one section and apply the ratchet; returns (pass, skip).
fn ratchet(
    section: &str,
    list: &[(&str, &str)],
    run: fn(&Value, &str) -> Outcome,
) -> (usize, usize) {
    let (base, entries) = entries(section);
    let known: BTreeMap<&str, &str> = list.iter().copied().collect();
    let (mut pass, mut skip) = (0, 0);
    let mut regressions = Vec::new();
    let mut fixed = Vec::new();
    let mut seen = BTreeSet::new();
    for e in &entries {
        let name = e["name"].as_str().unwrap_or_default();
        let key = format!("{section}/{name}");
        seen.insert(key.clone());
        match run(e, &base) {
            Outcome::Skip => skip += 1,
            Outcome::Pass => {
                pass += 1;
                if known.contains_key(key.as_str()) {
                    fixed.push(key);
                }
            }
            Outcome::Fail(why) => {
                if !known.contains_key(key.as_str()) {
                    regressions.push(format!("{key}: {why}"));
                }
            }
        }
    }
    let stale: Vec<&&str> = known
        .keys()
        .filter(|k| k.starts_with(&format!("{section}/")) && !seen.contains(**k))
        .collect();
    eprintln!(
        "{section}: {pass} pass, {} fail, {skip} skipped",
        entries.len() - pass - skip
    );
    assert!(
        regressions.is_empty() && fixed.is_empty() && stale.is_empty(),
        "{section}: {} unexpected failures, {} listed failures now pass, {} listed tests do not exist\n\
         failures:\n  {}\nnow passing (remove from the list):\n  {}\nmissing:\n  {:?}",
        regressions.len(),
        fixed.len(),
        stale.len(),
        regressions.join("\n  "),
        fixed.join("\n  "),
        stale
    );
    (pass, skip)
}

fn section_failures<'a>(section: &str) -> Vec<(&'a str, &'a str)> {
    KNOWN_FAILURES
        .iter()
        .copied()
        .filter(|(k, _)| k.starts_with(&format!("{section}/")))
        .collect()
}

#[test]
fn shextest_validation() {
    let (pass, _) = ratchet(
        "validation",
        &section_failures("validation"),
        validation_case,
    );
    assert!(
        pass >= FLOORS[0],
        "validation: {pass} < floor {}",
        FLOORS[0]
    );
}

#[test]
fn shextest_representation() {
    let (pass, _) = ratchet("schemas", &section_failures("schemas"), representation_case);
    assert!(pass >= FLOORS[1], "schemas: {pass} < floor {}", FLOORS[1]);
}

#[test]
fn shextest_negative_syntax() {
    let (pass, _) = ratchet(
        "negativeSyntax",
        &section_failures("negativeSyntax"),
        negative_syntax_case,
    );
    assert!(
        pass >= FLOORS[2],
        "negativeSyntax: {pass} < floor {}",
        FLOORS[2]
    );
}

#[test]
fn shextest_negative_structure() {
    let (pass, _) = ratchet(
        "negativeStructure",
        &section_failures("negativeStructure"),
        negative_structure_case,
    );
    assert!(
        pass >= FLOORS[3],
        "negativeStructure: {pass} < floor {}",
        FLOORS[3]
    );
}

/// Every validation case the engine passes also passes through the store,
/// except the listed divergences.
#[test]
fn shextest_validation_through_the_store() {
    let (base, entries) = entries("validation");
    let known: BTreeMap<&str, &str> = STORE_DIVERGENCES.iter().copied().collect();
    let listed_failures: BTreeSet<String> = section_failures("validation")
        .iter()
        .map(|(k, _)| k.to_string())
        .collect();
    let mut unexpected = Vec::new();
    let mut fixed = Vec::new();
    for e in &entries {
        let key = format!("validation/{}", e["name"].as_str().unwrap_or_default());
        if listed_failures.contains(&key) {
            continue;
        }
        match store_case(e, &base) {
            Outcome::Skip => {}
            Outcome::Pass if known.contains_key(key.as_str()) => fixed.push(key),
            Outcome::Pass => {}
            Outcome::Fail(why) if !known.contains_key(key.as_str()) => {
                unexpected.push(format!("{key}: {why}"))
            }
            Outcome::Fail(_) => {}
        }
    }
    eprintln!(
        "validation through the store: {} listed divergences",
        known.len()
    );
    assert!(
        unexpected.is_empty() && fixed.is_empty(),
        "{} unexpected divergences, {} listed divergences now agree\n  {}\nnow agree:\n  {}",
        unexpected.len(),
        fixed.len(),
        unexpected.join("\n  "),
        fixed.join("\n  ")
    );
}

/// The skips are exactly the 2.next-tagged tests at the pinned commit: 100
/// validation and 16 representation tests. A changed count means the corpus
/// or the skip rule changed.
#[test]
fn only_shex_next_tests_are_skipped() {
    for (section, want) in [("validation", 100), ("schemas", 16)] {
        let (_, entries) = entries(section);
        let skipped = entries.iter().filter(|e| is_next(e)).count();
        assert_eq!(skipped, want, "{section}: 2.next-tagged tests");
    }
    let (base, schemas) = entries("schemas");
    for key in NEXT_UNTAGGED {
        let e = schemas
            .iter()
            .find(|e| format!("schemas/{}", e["name"].as_str().unwrap_or_default()) == *key)
            .unwrap_or_else(|| panic!("{key} is not in the suite"));
        assert!(
            !is_next(e),
            "{key} is tagged now: drop it from NEXT_UNTAGGED"
        );
        let text = read(&resolve(&base, e["shex"].as_str().unwrap())).unwrap();
        assert!(
            text.contains("EXTENDS") || text.contains("ABSTRACT") || text.contains("extends"),
            "{key} uses no ShEx 2.next keyword"
        );
    }
    for section in ["negativeSyntax", "negativeStructure"] {
        let (_, entries) = entries(section);
        assert!(
            !entries.iter().any(is_next),
            "{section} has no 2.next-tagged tests"
        );
    }
}
