//! The ShEx 2.1 validation engine (§5.2–§5.5, §5.8).
//!
//! **Typing.** `satisfies(n, @L)` is decided for one strongly connected
//! component of the label graph at a time ([`super::check`]). Inside a
//! component every reference is positive (the negation requirement), so the
//! component's typing is a greatest fixpoint: an *exploration* assumes each
//! node/label pair it meets conforms, records who read that assumption, and
//! when a pair turns out not to conform re-evaluates its readers until
//! nothing changes. A reference into another component — always a lower
//! stratum, and the only kind a `NOT` or an `EXTRA` predicate may make —
//! runs that component's exploration to completion first, so negation only
//! ever sees decided results (stratified negation, §5.2's
//! `completeTypingOn`). Decided pairs are cached for the whole validation.
//!
//! **Partitions.** A shape's neighbourhood is split between its triple
//! constraints and the remainder (§5.5.2). Each arc whose direction and
//! predicate some triple constraint names must be matched by one of the
//! constraints its value satisfies; an arc no constraint's value accepts may
//! stay unmatched only when its predicate is `EXTRA`; a `CLOSED` shape allows
//! no other arcs out. Whether an assignment of arcs to constraints satisfies
//! the triple expression's groups, choices and cardinalities is decided with
//! derivatives of the expression read as a regular bag expression, searching
//! over the arcs with more than one candidate constraint.
//!
//! **Semantic actions** run only the `http://shex.io/extensions/Test/`
//! extension: `print(…)` succeeds and `fail(…)` fails. No other action's code
//! is ever interpreted or executed; actions of other extensions succeed.

use std::collections::{HashMap, HashSet, VecDeque};
use std::rc::Rc;

use oxigraph::model::{NamedNode, Term};

use super::ast::*;
use super::check::ResolvedSchema;
use super::data::Data;
use super::pattern;
use super::xsd;

/// The Test semantic-action extension, the only one evaluated.
pub const TEST_EXTENSION: &str = "http://shex.io/extensions/Test/";

/// How many node/shape evaluations may be nested (a chain of references
/// through the data). Deeper than this, validation stops with an error
/// instead of a verdict; [`super::validate_on_large_stack`] gives the
/// engine the stack this depth needs.
pub const MAX_DEPTH: usize = 20_000;

type Pair = (Term, Label);

struct Prov {
    value: bool,
    readers: HashSet<Pair>,
}

struct Exploration {
    component: usize,
    state: HashMap<Pair, Prov>,
    current: Vec<Pair>,
    queue: VecDeque<Pair>,
}

/// One triple constraint of a compiled shape.
struct CTc {
    predicate: String,
    inverse: bool,
    value_expr: Option<ShapeExpr>,
    min: u32,
    max: Max,
    acts_ok: bool,
}

/// A shape with its inclusions expanded, ready to match.
struct CompiledShape {
    tcs: Vec<CTc>,
    rbe: Rbe,
    out_keys: HashMap<String, Vec<usize>>,
    in_keys: HashMap<String, Vec<usize>>,
    extra: HashSet<String>,
    closed: bool,
    acts_ok: bool,
}

/// What a shape map entry validates a node against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShapeSel {
    Label(Label),
    Start,
}

/// The engine for one validation: a schema, the data, and what has been
/// decided so far.
pub struct Engine<'a> {
    schema: &'a ResolvedSchema,
    data: &'a dyn Data,
    externs: Rc<HashMap<Label, ShapeExpr>>,
    act_code: HashMap<String, String>,
    decided: HashMap<Pair, bool>,
    explorations: Vec<Exploration>,
    compiled: HashMap<usize, Rc<CompiledShape>>,
    start_acts_ok: bool,
    depth: usize,
    aborted: Option<String>,
    /// Nesting of explanations: an explanation follows references, which
    /// may cycle, so it stops at a fixed depth.
    explaining: usize,
}

impl<'a> Engine<'a> {
    pub fn new(schema: &'a ResolvedSchema, data: &'a dyn Data) -> Self {
        let mut e = Engine {
            schema,
            data,
            externs: Rc::new(HashMap::new()),
            act_code: HashMap::new(),
            decided: HashMap::new(),
            explorations: Vec::new(),
            compiled: HashMap::new(),
            start_acts_ok: true,
            depth: 0,
            aborted: None,
            explaining: 0,
        };
        e.start_acts_ok = e.acts_ok(&schema.start_acts);
        e
    }

    /// Definitions for `EXTERNAL` shapes (§5.3.2 leaves the mechanism to
    /// the implementation; without one an external shape is not satisfied).
    pub fn with_externs(mut self, externs: HashMap<Label, ShapeExpr>) -> Self {
        self.externs = Rc::new(externs);
        self
    }

    /// Code for semantic actions written without any (`%<name>%`), by
    /// action name.
    pub fn with_action_code(mut self, code: HashMap<String, String>) -> Self {
        self.act_code = code;
        self.start_acts_ok = self.acts_ok(&self.schema.start_acts);
        self
    }

    /// Set when a validation went deeper than [`MAX_DEPTH`]: its verdicts
    /// are not to be trusted.
    pub fn aborted(&self) -> Option<&str> {
        self.aborted.as_deref()
    }

    /// Whether `n` conforms to `sel`, and why not when it does not.
    pub fn validate(&mut self, n: &Term, sel: &ShapeSel) -> Result<(), String> {
        if !self.start_acts_ok {
            return Err("a start action failed".into());
        }
        match sel {
            ShapeSel::Start => {
                let schema = self.schema;
                let Some(start) = &schema.start else {
                    return Err("the schema has no start shape".into());
                };
                if self.sat(n, start) {
                    Ok(())
                } else {
                    Err(self.explain(n, start, 0))
                }
            }
            ShapeSel::Label(l) => {
                if self.schema.shape(l).is_none() {
                    return Err(format!("shape {l} is not defined in the schema"));
                }
                if self.check_ref(n, l) {
                    Ok(())
                } else {
                    let se = ShapeExpr::Ref(l.clone());
                    Err(self.explain(n, &se, 0))
                }
            }
        }
    }

    // ── shape expressions ────────────────────────────────────────────────

    fn sat(&mut self, n: &Term, se: &ShapeExpr) -> bool {
        match se {
            ShapeExpr::NodeConstraint(nc) => {
                node_constraint(n, nc).is_ok() && self.acts_ok(&nc.sem_acts)
            }
            ShapeExpr::Shape(s) => self.matches_shape(n, s),
            ShapeExpr::Or(v) => v.iter().any(|e| self.sat(n, e)),
            ShapeExpr::And(v) => v.iter().all(|e| self.sat(n, e)),
            ShapeExpr::Not(e) => !self.sat(n, e),
            // An EXTERNAL reached other than through its label has no
            // definition to consult.
            ShapeExpr::External => false,
            ShapeExpr::Ref(l) => self.check_ref(n, l),
        }
    }

    fn check_ref(&mut self, n: &Term, l: &Label) -> bool {
        let pair = (n.clone(), l.clone());
        if let Some(&v) = self.decided.get(&pair) {
            return v;
        }
        let Some(&component) = self.schema.component.get(l) else {
            return false;
        };
        if let Some(top) = self.explorations.last_mut() {
            if top.component == component {
                let reader = top.current.last().cloned();
                if let Some(p) = top.state.get_mut(&pair) {
                    if let Some(r) = reader {
                        p.readers.insert(r);
                    }
                    return p.value;
                }
                top.state.insert(
                    pair.clone(),
                    Prov {
                        value: true,
                        readers: reader.into_iter().collect(),
                    },
                );
                return self.eval_pair(&pair);
            }
        }
        // A new component: decide it completely before answering.
        self.explorations.push(Exploration {
            component,
            state: HashMap::from([(
                pair.clone(),
                Prov {
                    value: true,
                    readers: HashSet::new(),
                },
            )]),
            current: Vec::new(),
            queue: VecDeque::new(),
        });
        self.eval_pair(&pair);
        while let Some(p) = self.explorations.last_mut().unwrap().queue.pop_front() {
            let still_true = self.explorations.last().unwrap().state[&p].value;
            if still_true {
                self.eval_pair(&p);
            }
        }
        let done = self.explorations.pop().unwrap();
        for (p, prov) in done.state {
            self.decided.insert(p, prov.value);
        }
        self.decided[&pair]
    }

    /// Evaluate `pair`'s declaration under the current assumptions; when it
    /// fails, flip it and queue its readers.
    fn eval_pair(&mut self, pair: &Pair) -> bool {
        let (n, l) = pair;
        if self.aborted.is_some() || self.depth >= MAX_DEPTH {
            self.aborted.get_or_insert_with(|| {
                format!(
                    "validation stopped: references through the data nest deeper than {MAX_DEPTH}"
                )
            });
            let top = self.explorations.last_mut().unwrap();
            let prov = top.state.get_mut(pair).unwrap();
            prov.value = false;
            return false;
        }
        self.depth += 1;
        self.explorations
            .last_mut()
            .unwrap()
            .current
            .push(pair.clone());
        let schema = self.schema;
        let v = match schema.shape(l) {
            Some(ShapeExpr::External) => {
                let externs = self.externs.clone();
                match externs.get(l) {
                    Some(def) => self.sat(n, def),
                    None => false,
                }
            }
            Some(se) => self.sat(n, se),
            None => false,
        };
        self.depth -= 1;
        let top = self.explorations.last_mut().unwrap();
        top.current.pop();
        let prov = top.state.get_mut(pair).unwrap();
        if !v && prov.value {
            prov.value = false;
            let readers: Vec<Pair> = prov.readers.drain().collect();
            top.queue.extend(readers);
        }
        top.state[pair].value
    }

    // ── shapes ───────────────────────────────────────────────────────────

    fn compiled(&mut self, s: &Shape) -> Rc<CompiledShape> {
        let key = s as *const Shape as usize;
        if let Some(c) = self.compiled.get(&key) {
            return c.clone();
        }
        let mut tcs = Vec::new();
        let rbe = match &s.expression {
            Some(te) => self.compile_te(te, &mut tcs, &mut HashSet::new()),
            None => Rbe::Empty,
        };
        let mut out_keys: HashMap<String, Vec<usize>> = HashMap::new();
        let mut in_keys: HashMap<String, Vec<usize>> = HashMap::new();
        for (i, tc) in tcs.iter().enumerate() {
            let m = if tc.inverse {
                &mut in_keys
            } else {
                &mut out_keys
            };
            m.entry(tc.predicate.clone()).or_default().push(i);
        }
        let c = Rc::new(CompiledShape {
            tcs,
            rbe,
            out_keys,
            in_keys,
            extra: s.extra.iter().cloned().collect(),
            closed: s.closed,
            acts_ok: self.acts_ok(&s.sem_acts),
        });
        self.compiled.insert(key, c.clone());
        c
    }

    fn compile_te(
        &self,
        te: &TripleExpr,
        tcs: &mut Vec<CTc>,
        including: &mut HashSet<Label>,
    ) -> Rbe {
        match te {
            TripleExpr::TripleConstraint(tc) => {
                let i = tcs.len();
                tcs.push(CTc {
                    predicate: tc.predicate.clone(),
                    inverse: tc.inverse,
                    value_expr: tc.value_expr.as_deref().cloned(),
                    min: tc.min,
                    max: tc.max,
                    acts_ok: self.acts_ok(&tc.sem_acts),
                });
                Rbe::Sym {
                    tc: i,
                    min: tc.min,
                    max: tc.max,
                }
            }
            TripleExpr::EachOf(g) | TripleExpr::OneOf(g) => {
                let parts: Vec<Rbe> = g
                    .expressions
                    .iter()
                    .map(|e| self.compile_te(e, tcs, including))
                    .collect();
                let body = if !self.acts_ok(&g.sem_acts) {
                    Rbe::Fail
                } else if matches!(te, TripleExpr::EachOf(_)) {
                    Rbe::and(parts)
                } else {
                    Rbe::or(parts)
                };
                Rbe::rep(body, g.min, g.max)
            }
            TripleExpr::Ref(l) => {
                let Some(t) = self.schema.triple_exprs.get(l) else {
                    return Rbe::Fail;
                };
                if !including.insert(l.clone()) {
                    return Rbe::Fail;
                }
                let r = self.compile_te(t, tcs, including);
                including.remove(l);
                r
            }
        }
    }

    /// The arcs of `n`'s neighbourhood that must be matched, each with the
    /// constraints that accept it — or `Err` with the reason no partition
    /// can exist.
    fn arcs_to_match(
        &mut self,
        n: &Term,
        cs: &CompiledShape,
        why: bool,
    ) -> Result<Vec<Vec<usize>>, String> {
        let mut to_match = Vec::new();
        for (p, o) in self.data.arcs_out(n) {
            match cs.out_keys.get(p.as_str()) {
                Some(idxs) => {
                    let cands: Vec<usize> = idxs
                        .iter()
                        .copied()
                        .filter(|&i| self.tc_accepts(&cs.tcs[i], &o))
                        .collect();
                    if cands.is_empty() {
                        if !cs.extra.contains(p.as_str()) {
                            if !why {
                                return Err(String::new());
                            }
                            let reason = self.value_failure(&cs.tcs[idxs[0]], &o);
                            return Err(format!("{p} {}: {reason}", show(&o)));
                        }
                    } else {
                        to_match.push(cands);
                    }
                }
                None if cs.closed => {
                    return Err(format!(
                        "the shape is CLOSED and no triple constraint allows {p} {}",
                        show(&o)
                    ));
                }
                None => {}
            }
        }
        for (s, p) in self.data.arcs_in(n) {
            if let Some(idxs) = cs.in_keys.get(p.as_str()) {
                let cands: Vec<usize> = idxs
                    .iter()
                    .copied()
                    .filter(|&i| self.tc_accepts(&cs.tcs[i], &s))
                    .collect();
                if cands.is_empty() {
                    if !cs.extra.contains(p.as_str()) {
                        if !why {
                            return Err(String::new());
                        }
                        let reason = self.value_failure(&cs.tcs[idxs[0]], &s);
                        return Err(format!("^{p} from {}: {reason}", show(&s)));
                    }
                } else {
                    to_match.push(cands);
                }
            }
        }
        to_match.sort_by_key(Vec::len);
        Ok(to_match)
    }

    fn matches_shape(&mut self, n: &Term, s: &Shape) -> bool {
        let cs = self.compiled(s);
        let Ok(to_match) = self.arcs_to_match(n, &cs, false) else {
            return false;
        };
        let mut failed = HashSet::new();
        search(&cs.rbe, &to_match, 0, &mut failed) && cs.acts_ok
    }

    fn tc_accepts(&mut self, tc: &CTc, value: &Term) -> bool {
        tc.acts_ok
            && match &tc.value_expr {
                None => true,
                Some(ve) => self.sat(value, ve),
            }
    }

    fn value_failure(&mut self, tc: &CTc, value: &Term) -> String {
        if !tc.acts_ok {
            return "a semantic action failed".into();
        }
        match &tc.value_expr {
            Some(ve) => self.explain(value, ve, 1),
            None => "rejected".into(),
        }
    }

    // ── semantic actions ─────────────────────────────────────────────────

    fn acts_ok(&self, acts: &[SemAct]) -> bool {
        acts.iter().all(|a| {
            if !a.name.starts_with(TEST_EXTENSION) {
                return true;
            }
            let code = a
                .code
                .as_deref()
                .or_else(|| self.act_code.get(&a.name).map(String::as_str));
            match code {
                None => true,
                Some(c) => matches!(test_action(c), Some(TestAction::Print)),
            }
        })
    }

    // ── explanations ─────────────────────────────────────────────────────

    /// Why `n` does not satisfy `se` (called only when it does not).
    fn explain(&mut self, n: &Term, se: &ShapeExpr, _depth: usize) -> String {
        if self.explaining >= 6 {
            return "does not conform".into();
        }
        self.explaining += 1;
        let why = self.explain_inner(n, se);
        self.explaining -= 1;
        why
    }

    fn explain_inner(&mut self, n: &Term, se: &ShapeExpr) -> String {
        let depth = self.explaining;
        let schema = self.schema;
        match se {
            ShapeExpr::NodeConstraint(nc) => match node_constraint(n, nc) {
                Err(e) => e,
                Ok(()) => "does not conform".into(),
            },
            ShapeExpr::Ref(l) => match schema.shape(l) {
                Some(ShapeExpr::External) => {
                    if self.externs.contains_key(l) {
                        format!("{} does not satisfy the external shape {l}", show(n))
                    } else {
                        format!("{l} is EXTERNAL and no definition is available")
                    }
                }
                Some(def) => {
                    let why = self.explain(n, def, depth + 1);
                    format!("{} does not conform to {l}: {why}", show(n))
                }
                None => format!("shape {l} is not defined"),
            },
            ShapeExpr::Not(_) => format!("{} matches the negated shape expression", show(n)),
            ShapeExpr::And(v) => {
                for e in v {
                    if !self.sat(n, e) {
                        return self.explain(n, e, depth + 1);
                    }
                }
                "does not conform".into()
            }
            ShapeExpr::Or(v) => {
                let parts: Vec<String> = v.iter().map(|e| self.explain(n, e, depth + 1)).collect();
                format!("no alternative matches ({})", parts.join("; "))
            }
            ShapeExpr::External => "an EXTERNAL shape has no definition".into(),
            ShapeExpr::Shape(s) => self.explain_shape(n, s),
        }
    }

    fn explain_shape(&mut self, n: &Term, s: &Shape) -> String {
        let cs = self.compiled(s);
        let to_match = match self.arcs_to_match(n, &cs, true) {
            Err(e) => return e,
            Ok(m) => m,
        };
        let mut counts = vec![0u32; cs.tcs.len()];
        for cands in &to_match {
            for &i in cands {
                counts[i] += 1;
            }
        }
        for (i, tc) in cs.tcs.iter().enumerate() {
            let dir = if tc.inverse { "^" } else { "" };
            if counts[i] < tc.min {
                return format!(
                    "expected at least {} {dir}<{}> matching its value expression, found {}",
                    tc.min, tc.predicate, counts[i]
                );
            }
            if !tc.max.allows(counts[i])
                && cs
                    .tcs
                    .iter()
                    .filter(|t| t.predicate == tc.predicate)
                    .count()
                    == 1
            {
                let Max::Bounded(m) = tc.max else {
                    unreachable!()
                };
                return format!(
                    "expected at most {m} {dir}<{}>, found {}",
                    tc.predicate, counts[i]
                );
            }
        }
        let mut failed = HashSet::new();
        if search(&cs.rbe, &to_match, 0, &mut failed) && !cs.acts_ok {
            return "a semantic action failed".into();
        }
        "no partition of the neighbourhood matches the triple expression".into()
    }
}

/// A term as reports show it: an IRI bare in angle brackets, a literal or
/// blank node in N-Triples form.
pub fn show(t: &Term) -> String {
    t.to_string()
}

// ── the Test extension ───────────────────────────────────────────────────

enum TestAction {
    Print,
    Fail,
}

/// `print(x)` / `fail(x)` where `x` is `s`, `p`, `o`, `n` or a string. Anything
/// else is not Test-extension code and fails.
fn test_action(code: &str) -> Option<TestAction> {
    let c = code.trim();
    let (verb, rest) = match c.strip_prefix("print") {
        Some(r) => (TestAction::Print, r),
        None => (TestAction::Fail, c.strip_prefix("fail")?),
    };
    let arg = rest.trim().strip_prefix('(')?.strip_suffix(')')?.trim();
    let ok = matches!(arg, "s" | "p" | "o" | "n")
        || (arg.len() >= 2 && arg.starts_with('"') && arg.ends_with('"'));
    ok.then_some(verb)
}

// ── node constraints ─────────────────────────────────────────────────────

fn lexical(n: &Term) -> String {
    match n {
        Term::NamedNode(i) => i.as_str().to_string(),
        Term::BlankNode(b) => b.as_str().to_string(),
        Term::Literal(l) => l.value().to_string(),
        #[cfg(feature = "rdf-12")]
        Term::Triple(t) => t.to_string(),
    }
}

/// Whether `n` satisfies `nc`, with the first violated part otherwise.
pub fn node_constraint(n: &Term, nc: &NodeConstraint) -> Result<(), String> {
    let shown = show(n);
    if let Some(k) = nc.node_kind {
        let ok = match k {
            NodeKind::Iri => n.is_named_node(),
            NodeKind::BNode => n.is_blank_node(),
            NodeKind::Literal => n.is_literal(),
            NodeKind::NonLiteral => n.is_named_node() || n.is_blank_node(),
        };
        if !ok {
            return Err(format!("{shown} is not of node kind {}", k.name()));
        }
    }
    if let Some(dt) = &nc.datatype {
        let Term::Literal(l) = n else {
            return Err(format!("{shown} is not a literal of datatype <{dt}>"));
        };
        if l.datatype().as_str() != dt {
            return Err(format!(
                "{shown} has datatype <{}>, not <{dt}>",
                l.datatype().as_str()
            ));
        }
        if !xsd::lexical_valid(l.value(), dt) {
            return Err(format!(
                "\"{}\" is not a valid lexical form of <{dt}>",
                l.value()
            ));
        }
    }
    for f in &nc.string_facets {
        let lex = lexical(n);
        let len = lex.chars().count() as u64;
        match f {
            StringFacet::Length(v) if len != *v => {
                return Err(format!("{shown} has length {len}, not LENGTH {v}"))
            }
            StringFacet::MinLength(v) if len < *v => {
                return Err(format!(
                    "{shown} is shorter than MINLENGTH {v} ({len} characters)"
                ))
            }
            StringFacet::MaxLength(v) if len > *v => {
                return Err(format!(
                    "{shown} is longer than MAXLENGTH {v} ({len} characters)"
                ))
            }
            StringFacet::Pattern(p, flags) => match pattern::compile(p, flags.as_deref()) {
                Ok(re) if re.is_match(&lex) => {}
                Ok(_) => return Err(format!("{shown} does not match /{p}/")),
                Err(e) => return Err(e),
            },
            _ => {}
        }
    }
    if !nc.numeric_facets.is_empty() {
        let Term::Literal(l) = n else {
            return Err(format!("{shown} is not a numeric literal"));
        };
        let dt = l.datatype().as_str();
        let Some(num) = xsd::numeric_value(l.value(), dt).filter(|_| xsd::is_numeric_datatype(dt))
        else {
            return Err(format!("{shown} is not a valid numeric literal"));
        };
        for f in &nc.numeric_facets {
            use std::cmp::Ordering::*;
            let cmp = |b: &xsd::Numeric| num.partial_cmp_value(b);
            let (ok, what) = match f {
                NumericFacet::MinInclusive(b) => (
                    matches!(cmp(b), Some(Greater | Equal)),
                    format!("MININCLUSIVE {b}"),
                ),
                NumericFacet::MinExclusive(b) => {
                    (matches!(cmp(b), Some(Greater)), format!("MINEXCLUSIVE {b}"))
                }
                NumericFacet::MaxInclusive(b) => (
                    matches!(cmp(b), Some(Less | Equal)),
                    format!("MAXINCLUSIVE {b}"),
                ),
                NumericFacet::MaxExclusive(b) => {
                    (matches!(cmp(b), Some(Less)), format!("MAXEXCLUSIVE {b}"))
                }
                NumericFacet::TotalDigits(v) | NumericFacet::FractionDigits(v) => {
                    let total = matches!(f, NumericFacet::TotalDigits(_));
                    let name = if total {
                        "TOTALDIGITS"
                    } else {
                        "FRACTIONDIGITS"
                    };
                    match (&num, xsd::is_decimal_derived(dt)) {
                        (xsd::Numeric::Decimal(d), true) => {
                            let digits = if total {
                                d.total_digits()
                            } else {
                                d.fraction_digits()
                            };
                            (digits <= *v, format!("{name} {v} ({digits} digits)"))
                        }
                        _ => (false, format!("{name} {v} (not an xsd:decimal value)")),
                    }
                }
            };
            if !ok {
                return Err(format!("{shown} violates {what}"));
            }
        }
    }
    if let Some(values) = &nc.values {
        if !values.iter().any(|v| value_matches(n, v)) {
            return Err(format!("{shown} is not in the value set"));
        }
    }
    Ok(())
}

fn lang_of(n: &Term) -> Option<&str> {
    match n {
        Term::Literal(l) => l.language(),
        _ => None,
    }
}

/// RFC 4647 basic filtering: `range` matches `tag` itself or a prefix of it
/// ending at a '-'; the empty range (`@~`) matches every language tag.
fn lang_matches(tag: &str, range: &str) -> bool {
    if range.is_empty() || range == "*" {
        return true;
    }
    let t = tag.to_ascii_lowercase();
    let r = range.to_ascii_lowercase();
    t == r || (t.starts_with(&r) && t.as_bytes().get(r.len()) == Some(&b'-'))
}

fn iri_of(n: &Term) -> Option<&str> {
    match n {
        Term::NamedNode(i) => Some(i.as_str()),
        _ => None,
    }
}

fn literal_lexical(n: &Term) -> Option<&str> {
    match n {
        Term::Literal(l) => Some(l.value()),
        _ => None,
    }
}

fn value_matches(n: &Term, v: &ValueSetValue) -> bool {
    match v {
        ValueSetValue::Object(ObjectValue::Iri(i)) => iri_of(n) == Some(i.as_str()),
        ValueSetValue::Object(ObjectValue::Literal(ol)) => match n {
            Term::Literal(l) => {
                l.value() == ol.value
                    && l.datatype().as_str() == ol.datatype_iri()
                    && match (&ol.language, l.language()) {
                        (Some(a), Some(b)) => a.eq_ignore_ascii_case(b),
                        (None, None) => true,
                        _ => false,
                    }
            }
            _ => false,
        },
        ValueSetValue::Language(t) => lang_of(n).is_some_and(|l| l.eq_ignore_ascii_case(t)),
        ValueSetValue::IriStem(s) => iri_of(n).is_some_and(|i| i.starts_with(s.as_str())),
        ValueSetValue::LiteralStem(s) => {
            literal_lexical(n).is_some_and(|l| l.starts_with(s.as_str()))
        }
        ValueSetValue::LanguageStem(s) => lang_of(n).is_some_and(|l| lang_matches(l, s)),
        ValueSetValue::IriStemRange(stem, ex) => {
            let in_stem = match stem {
                Stem::Wildcard => true,
                Stem::Value(s) => iri_of(n).is_some_and(|i| i.starts_with(s.as_str())),
            };
            in_stem
                && !ex.iter().any(|e| match e {
                    Exclusion::Value(v) => iri_of(n) == Some(v.as_str()),
                    Exclusion::Stem(s) => iri_of(n).is_some_and(|i| i.starts_with(s.as_str())),
                })
        }
        ValueSetValue::LiteralStemRange(stem, ex) => {
            let in_stem = match stem {
                Stem::Wildcard => true,
                Stem::Value(s) => literal_lexical(n).is_some_and(|l| l.starts_with(s.as_str())),
            };
            in_stem
                && !ex.iter().any(|e| match e {
                    Exclusion::Value(v) => literal_lexical(n) == Some(v.as_str()),
                    Exclusion::Stem(s) => {
                        literal_lexical(n).is_some_and(|l| l.starts_with(s.as_str()))
                    }
                })
        }
        ValueSetValue::LanguageStemRange(stem, ex) => {
            let in_stem = match stem {
                Stem::Wildcard => true,
                Stem::Value(s) => lang_of(n).is_some_and(|l| lang_matches(l, s)),
            };
            in_stem
                && !ex.iter().any(|e| match e {
                    Exclusion::Value(v) => lang_of(n).is_some_and(|l| l.eq_ignore_ascii_case(v)),
                    Exclusion::Stem(s) => lang_of(n).is_some_and(|l| lang_matches(l, s)),
                })
        }
    }
}

/// Discovery for a request without a shape map: the nodes that have an arc
/// some triple constraint of `se` names (subjects for forward constraints,
/// objects for inverse ones).
pub fn candidate_nodes(schema: &ResolvedSchema, data: &dyn Data, se: &ShapeExpr) -> Vec<Term> {
    let mut preds = Vec::new();
    collect_preds(schema, se, &mut preds, &mut HashSet::new());
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for (p, inverse) in preds {
        let Ok(p) = NamedNode::new(p) else { continue };
        let nodes = if inverse {
            data.objects(None, &p)
        } else {
            data.subjects(&p, None)
        };
        for n in nodes {
            if seen.insert(n.clone()) {
                out.push(n);
            }
        }
    }
    out
}

fn collect_preds(
    schema: &ResolvedSchema,
    se: &ShapeExpr,
    out: &mut Vec<(String, bool)>,
    seen: &mut HashSet<Label>,
) {
    match se {
        ShapeExpr::Or(v) | ShapeExpr::And(v) => {
            v.iter().for_each(|e| collect_preds(schema, e, out, seen))
        }
        ShapeExpr::Not(_) | ShapeExpr::NodeConstraint(_) | ShapeExpr::External => {}
        ShapeExpr::Ref(l) => {
            if seen.insert(l.clone()) {
                if let Some(d) = schema.shape(l) {
                    collect_preds(schema, d, out, seen);
                }
            }
        }
        ShapeExpr::Shape(s) => {
            if let Some(te) = &s.expression {
                collect_te_preds(schema, te, out, seen);
            }
        }
    }
}

fn collect_te_preds(
    schema: &ResolvedSchema,
    te: &TripleExpr,
    out: &mut Vec<(String, bool)>,
    seen: &mut HashSet<Label>,
) {
    match te {
        TripleExpr::EachOf(g) | TripleExpr::OneOf(g) => g
            .expressions
            .iter()
            .for_each(|e| collect_te_preds(schema, e, out, seen)),
        TripleExpr::TripleConstraint(tc) => {
            if !out
                .iter()
                .any(|(p, i)| *p == tc.predicate && *i == tc.inverse)
            {
                out.push((tc.predicate.clone(), tc.inverse));
            }
        }
        TripleExpr::Ref(l) => {
            if seen.insert(l.clone()) {
                if let Some(t) = schema.triple_exprs.get(l) {
                    collect_te_preds(schema, t, out, seen);
                }
            }
        }
    }
}

// ── regular bag expressions ──────────────────────────────────────────────

/// A triple expression read as a regular bag expression over triple
/// constraint indices.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Rbe {
    Fail,
    Empty,
    Sym { tc: usize, min: u32, max: Max },
    And(Vec<Rbe>),
    Or(Vec<Rbe>),
    Rep { e: Box<Rbe>, min: u32, max: Max },
}

impl Rbe {
    fn and(parts: Vec<Rbe>) -> Rbe {
        let mut out = Vec::new();
        for p in parts {
            match p {
                Rbe::Fail => return Rbe::Fail,
                Rbe::Empty => {}
                Rbe::And(v) => out.extend(v),
                other => out.push(other),
            }
        }
        match out.len() {
            0 => Rbe::Empty,
            1 => out.pop().unwrap(),
            _ => Rbe::And(out),
        }
    }

    fn or(parts: Vec<Rbe>) -> Rbe {
        let mut out: Vec<Rbe> = Vec::new();
        for p in parts {
            match p {
                Rbe::Fail => {}
                Rbe::Or(v) => {
                    for x in v {
                        if !out.contains(&x) {
                            out.push(x);
                        }
                    }
                }
                other => {
                    if !out.contains(&other) {
                        out.push(other);
                    }
                }
            }
        }
        match out.len() {
            0 => Rbe::Fail,
            1 => out.pop().unwrap(),
            _ => Rbe::Or(out),
        }
    }

    fn rep(e: Rbe, min: u32, max: Max) -> Rbe {
        if max == Max::Bounded(0) {
            return Rbe::Empty;
        }
        match e {
            Rbe::Empty => Rbe::Empty,
            Rbe::Fail if min == 0 => Rbe::Empty,
            Rbe::Fail => Rbe::Fail,
            e if min == 1 && max == Max::Bounded(1) => e,
            e => Rbe::Rep {
                e: Box::new(e),
                min,
                max,
            },
        }
    }

    fn nullable(&self) -> bool {
        match self {
            Rbe::Fail => false,
            Rbe::Empty => true,
            Rbe::Sym { min, .. } => *min == 0,
            Rbe::And(v) => v.iter().all(Rbe::nullable),
            Rbe::Or(v) => v.iter().any(Rbe::nullable),
            Rbe::Rep { e, min, .. } => *min == 0 || e.nullable(),
        }
    }

    fn dec(max: Max) -> Option<Max> {
        match max {
            Max::Unbounded => Some(Max::Unbounded),
            Max::Bounded(0) => None,
            Max::Bounded(m) => Some(Max::Bounded(m - 1)),
        }
    }

    /// The bag derivative by one occurrence of `tc`.
    fn deriv(&self, t: usize) -> Rbe {
        match self {
            Rbe::Fail | Rbe::Empty => Rbe::Fail,
            Rbe::Sym { tc, min, max } => {
                if *tc != t {
                    return Rbe::Fail;
                }
                match Rbe::dec(*max) {
                    None => Rbe::Fail,
                    Some(Max::Bounded(0)) => Rbe::Empty,
                    Some(m) => Rbe::Sym {
                        tc: *tc,
                        min: min.saturating_sub(1),
                        max: m,
                    },
                }
            }
            Rbe::And(v) => {
                let mut alts = Vec::new();
                for (i, part) in v.iter().enumerate() {
                    let d = part.deriv(t);
                    if d == Rbe::Fail {
                        continue;
                    }
                    let mut w = v.clone();
                    w[i] = d;
                    alts.push(Rbe::and(w));
                }
                Rbe::or(alts)
            }
            Rbe::Or(v) => Rbe::or(v.iter().map(|p| p.deriv(t)).collect()),
            Rbe::Rep { e, min, max } => {
                let Some(m) = Rbe::dec(*max) else {
                    return Rbe::Fail;
                };
                let d = e.deriv(t);
                if d == Rbe::Fail {
                    return Rbe::Fail;
                }
                Rbe::and(vec![d, Rbe::rep((**e).clone(), min.saturating_sub(1), m)])
            }
        }
    }
}

/// Is there an assignment of each arc (from `k` on) to one of its candidate
/// constraints that the expression accepts?
fn search(rbe: &Rbe, arcs: &[Vec<usize>], k: usize, failed: &mut HashSet<(usize, Rbe)>) -> bool {
    if k == arcs.len() {
        return rbe.nullable();
    }
    if failed.contains(&(k, rbe.clone())) {
        return false;
    }
    for &tc in &arcs[k] {
        let d = rbe.deriv(tc);
        if d != Rbe::Fail && search(&d, arcs, k + 1, failed) {
            return true;
        }
    }
    failed.insert((k, rbe.clone()));
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sym(tc: usize, min: u32, max: u32) -> Rbe {
        Rbe::Sym {
            tc,
            min,
            max: Max::Bounded(max),
        }
    }

    #[test]
    fn bag_derivatives_count_and_shuffle() {
        // (a ; b){2}
        let e = Rbe::rep(
            Rbe::and(vec![sym(0, 1, 1), sym(1, 1, 1)]),
            2,
            Max::Bounded(2),
        );
        let arcs = vec![vec![0], vec![1], vec![0], vec![1]];
        assert!(search(&e, &arcs, 0, &mut HashSet::new()));
        let arcs = vec![vec![0], vec![0], vec![1]];
        assert!(!search(&e, &arcs, 0, &mut HashSet::new()));
    }

    #[test]
    fn choice_takes_one_branch() {
        // a | (b ; c)
        let e = Rbe::or(vec![
            sym(0, 1, 1),
            Rbe::and(vec![sym(1, 1, 1), sym(2, 1, 1)]),
        ]);
        assert!(search(&e, &[vec![0]], 0, &mut HashSet::new()));
        assert!(search(&e, &[vec![1], vec![2]], 0, &mut HashSet::new()));
        assert!(!search(
            &e,
            &[vec![0], vec![1], vec![2]],
            0,
            &mut HashSet::new()
        ));
    }

    #[test]
    fn an_arc_with_two_candidates_goes_where_it_fits() {
        // p {1} ; p {1} with two arcs each accepted by both constraints.
        let e = Rbe::and(vec![sym(0, 1, 1), sym(1, 1, 1)]);
        assert!(search(
            &e,
            &[vec![0, 1], vec![0, 1]],
            0,
            &mut HashSet::new()
        ));
        assert!(!search(&e, &[vec![0, 1]], 0, &mut HashSet::new()));
    }

    #[test]
    fn test_extension_code() {
        assert!(matches!(test_action(" print(s) "), Some(TestAction::Print)));
        assert!(matches!(test_action("fail(\"x\")"), Some(TestAction::Fail)));
        assert!(test_action("system(\"rm -rf /\")").is_none());
    }
}
