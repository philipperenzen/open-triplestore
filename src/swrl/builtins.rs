//! The SWRL built-ins (SWRL submission §8, namespace `swrlb:`), evaluated in
//! Rust over the bindings of a rule body.
//!
//! Every built-in is a predicate over its arguments. Which arguments may be
//! unbound when it is reached decides how it is evaluated:
//!
//! - **check** — every argument is bound: the built-in holds or it does not;
//! - **bind** — the first argument is unbound and the rest are bound: it is
//!   computed (`add(?z, ?x, 1)` binds `?z`), the SWRLAPI and Pellet
//!   convention. A few have one value per solution: `tokenize` binds each
//!   token in turn;
//! - **split** — the constructors `yearMonthDuration`, `dayTimeDuration`,
//!   `dateTime`, `date`, `time` and `anyURI` also run backwards: a bound first
//!   argument binds its unbound components;
//! - **enumerate** — `member` binds each element of a list, `sublist` each
//!   contiguous sublist, a `DataOneOf` range each listed value;
//! - **solve** — `add`, `subtract`, `unaryPlus`, `unaryMinus`, `booleanNot`
//!   and `equal` bind one unbound operand when the rest are bound.
//!
//! Any other pattern (two unbound addends, `lessThan` with an unbound side) has
//! infinitely many solutions and is refused when the rule is compiled, naming
//! the built-in and the unbound arguments. A built-in applied to values of the
//! wrong type (`add` over a string) simply does not hold.
//!
//! Lists (§8.7) are RDF lists read from the graphs the rule reads. The
//! constructive list built-ins (`listConcat`, `listIntersection`,
//! `listSubtraction`, `rest` of a new list, `sublist` enumeration) mint
//! deterministic nodes `urn:ots:swrl:list:<hash>` from the elements, so a
//! rule that builds the same list twice builds the same node and the fixed
//! point still terminates; the cells of a minted list are written with the
//! head triples that use it.

use std::collections::{HashMap, HashSet};

use oxigraph::model::{Literal, NamedNode, Term};
use oxsdatatypes::{
    Date, DateTime, DayTimeDuration, Decimal, Double, Duration, Time, YearMonthDuration,
};
use regex::Regex;
use sha2::{Digest, Sha256};

use super::datarange;
use super::expr::DataRange;
use super::values::{
    bool_literal, compare_terms, int_literal, string_literal, terms_equal, xsd, Num, Value,
};

/// The SWRL built-in namespace.
pub const SWRLB: &str = "http://www.w3.org/2003/11/swrlb#";
const RDF_NIL: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#nil";
/// The prefix of minted list nodes.
pub const LIST_PREFIX: &str = "urn:ots:swrl:list:";

/// How a built-in may be evaluated (see the module docs).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Shape {
    /// Holds or not; every argument must be bound.
    Pred,
    /// The first argument is a function of the rest.
    Func,
    /// [`Shape::Func`], and also splits a bound first argument.
    Ctor,
    /// Its own patterns (`equal`, `member`, `sublist`, `empty`).
    Special,
}

/// Every §8 built-in: its local name, shape and arity (`None` = any number
/// at least the minimum).
const BUILTINS: &[(&str, Shape, usize, Option<usize>)] = &[
    // §8.1 comparisons
    ("equal", Shape::Special, 2, Some(2)),
    ("notEqual", Shape::Pred, 2, Some(2)),
    ("lessThan", Shape::Pred, 2, Some(2)),
    ("lessThanOrEqual", Shape::Pred, 2, Some(2)),
    ("greaterThan", Shape::Pred, 2, Some(2)),
    ("greaterThanOrEqual", Shape::Pred, 2, Some(2)),
    // §8.2 math
    ("add", Shape::Func, 2, None),
    ("subtract", Shape::Func, 3, Some(3)),
    ("multiply", Shape::Func, 2, None),
    ("divide", Shape::Func, 3, Some(3)),
    ("integerDivide", Shape::Func, 3, Some(3)),
    ("mod", Shape::Func, 3, Some(3)),
    ("pow", Shape::Func, 3, Some(3)),
    ("unaryPlus", Shape::Func, 2, Some(2)),
    ("unaryMinus", Shape::Func, 2, Some(2)),
    ("abs", Shape::Func, 2, Some(2)),
    ("ceiling", Shape::Func, 2, Some(2)),
    ("floor", Shape::Func, 2, Some(2)),
    ("round", Shape::Func, 2, Some(2)),
    ("roundHalfToEven", Shape::Func, 2, Some(3)),
    ("sin", Shape::Func, 2, Some(2)),
    ("cos", Shape::Func, 2, Some(2)),
    ("tan", Shape::Func, 2, Some(2)),
    // §8.3 boolean
    ("booleanNot", Shape::Func, 2, Some(2)),
    // §8.4 strings
    ("stringEqualIgnoreCase", Shape::Pred, 2, Some(2)),
    ("stringConcat", Shape::Func, 1, None),
    ("substring", Shape::Func, 3, Some(4)),
    ("stringLength", Shape::Func, 2, Some(2)),
    ("normalizeSpace", Shape::Func, 2, Some(2)),
    ("upperCase", Shape::Func, 2, Some(2)),
    ("lowerCase", Shape::Func, 2, Some(2)),
    ("translate", Shape::Func, 4, Some(4)),
    ("contains", Shape::Pred, 2, Some(2)),
    ("containsIgnoreCase", Shape::Pred, 2, Some(2)),
    ("startsWith", Shape::Pred, 2, Some(2)),
    ("endsWith", Shape::Pred, 2, Some(2)),
    ("substringBefore", Shape::Func, 3, Some(3)),
    ("substringAfter", Shape::Func, 3, Some(3)),
    ("matches", Shape::Pred, 2, Some(3)),
    ("replace", Shape::Func, 4, Some(5)),
    ("tokenize", Shape::Func, 3, Some(4)),
    // §8.5 dates, times and durations
    ("yearMonthDuration", Shape::Ctor, 3, Some(3)),
    ("dayTimeDuration", Shape::Ctor, 5, Some(5)),
    ("dateTime", Shape::Ctor, 7, Some(8)),
    ("date", Shape::Ctor, 4, Some(5)),
    ("time", Shape::Ctor, 4, Some(5)),
    ("addYearMonthDurations", Shape::Func, 2, None),
    ("subtractYearMonthDurations", Shape::Func, 3, Some(3)),
    ("multiplyYearMonthDuration", Shape::Func, 3, Some(3)),
    ("divideYearMonthDurations", Shape::Func, 3, Some(3)),
    ("addDayTimeDurations", Shape::Func, 2, None),
    ("subtractDayTimeDurations", Shape::Func, 3, Some(3)),
    ("multiplyDayTimeDurations", Shape::Func, 3, Some(3)),
    ("divideDayTimeDuration", Shape::Func, 3, Some(3)),
    ("subtractDates", Shape::Func, 3, Some(3)),
    ("subtractTimes", Shape::Func, 3, Some(3)),
    ("addYearMonthDurationToDateTime", Shape::Func, 3, Some(3)),
    ("addDayTimeDurationToDateTime", Shape::Func, 3, Some(3)),
    (
        "subtractYearMonthDurationFromDateTime",
        Shape::Func,
        3,
        Some(3),
    ),
    (
        "subtractDayTimeDurationFromDateTime",
        Shape::Func,
        3,
        Some(3),
    ),
    ("addYearMonthDurationToDate", Shape::Func, 3, Some(3)),
    ("addDayTimeDurationToDate", Shape::Func, 3, Some(3)),
    ("subtractYearMonthDurationFromDate", Shape::Func, 3, Some(3)),
    ("subtractDayTimeDurationFromDate", Shape::Func, 3, Some(3)),
    ("addDayTimeDurationToTime", Shape::Func, 3, Some(3)),
    ("subtractDayTimeDurationFromTime", Shape::Func, 3, Some(3)),
    (
        "subtractDateTimesYieldingYearMonthDuration",
        Shape::Func,
        3,
        Some(3),
    ),
    (
        "subtractDateTimesYieldingDayTimeDuration",
        Shape::Func,
        3,
        Some(3),
    ),
    // §8.6 URIs
    ("resolveURI", Shape::Func, 3, Some(3)),
    ("anyURI", Shape::Ctor, 7, Some(7)),
    // §8.7 lists
    ("listConcat", Shape::Func, 1, None),
    ("listIntersection", Shape::Func, 3, Some(3)),
    ("listSubtraction", Shape::Func, 3, Some(3)),
    ("member", Shape::Special, 2, Some(2)),
    ("length", Shape::Func, 2, Some(2)),
    ("first", Shape::Func, 2, Some(2)),
    ("rest", Shape::Func, 2, Some(2)),
    ("sublist", Shape::Special, 2, Some(2)),
    ("empty", Shape::Special, 1, Some(1)),
];

/// Spellings implementations use for the same built-ins (the submission's
/// text and its RDF disagree on a trailing `s`).
fn canonical(local: &str) -> &str {
    match local {
        "multiplyYearMonthDurations" => "multiplyYearMonthDuration",
        "divideYearMonthDuration" => "divideYearMonthDurations",
        "multiplyDayTimeDuration" => "multiplyDayTimeDurations",
        "divideDayTimeDurations" => "divideDayTimeDuration",
        other => other,
    }
}

fn lookup(local: &str) -> Option<(Shape, usize, Option<usize>)> {
    let local = canonical(local);
    BUILTINS
        .iter()
        .find(|(n, ..)| *n == local)
        .map(|(_, s, min, max)| (*s, *min, *max))
}

/// The local name of a `swrlb:` built-in this engine evaluates, or an error
/// naming it.
pub(crate) fn resolve(iri: &str, arity: usize) -> Result<String, String> {
    let Some(local) = iri.strip_prefix(SWRLB) else {
        return Err(format!(
            "Unsupported SWRL builtin '{iri}': only the swrlb: built-ins of the SWRL \
             submission §8 are evaluated; refusing the rule rather than running it without \
             that condition"
        ));
    };
    let Some((_, min, max)) = lookup(local) else {
        return Err(format!(
            "Unsupported SWRL builtin '{iri}': swrlb:{local} is not a §8 built-in"
        ));
    };
    if arity < min || max.is_some_and(|m| arity > m) {
        let wanted = match max {
            Some(m) if m == min => format!("{min}"),
            Some(m) => format!("{min} to {m}"),
            None => format!("at least {min}"),
        };
        return Err(format!(
            "SWRL builtin swrlb:{local} takes {wanted} argument(s), found {arity}"
        ));
    }
    Ok(canonical(local).to_string())
}

/// Built-ins that can solve for one unbound operand (see the module docs).
fn solvable(local: &str) -> bool {
    matches!(
        local,
        "add" | "subtract" | "unaryPlus" | "unaryMinus" | "booleanNot"
    )
}

/// Whether `local` can be evaluated when only the arguments flagged in
/// `bound` are bound; an error explains why not.
pub(crate) fn check_pattern(local: &str, bound: &[bool]) -> Result<(), String> {
    let (shape, ..) = lookup(local).ok_or_else(|| format!("unknown built-in {local}"))?;
    let unbound: Vec<usize> = (0..bound.len()).filter(|i| !bound[*i]).collect();
    if unbound.is_empty() {
        return Ok(());
    }
    let rest_bound = bound.iter().skip(1).all(|b| *b);
    let ok = match shape {
        Shape::Pred => false,
        Shape::Func => rest_bound || (solvable(local) && bound[0] && unbound.len() == 1),
        Shape::Ctor => rest_bound || bound[0],
        Shape::Special => match local {
            "equal" => unbound.len() == 1,
            "member" => bound[1],
            "sublist" => bound[0],
            "empty" => true,
            _ => false,
        },
    };
    if ok {
        return Ok(());
    }
    let which = unbound
        .iter()
        .map(|i| (i + 1).to_string())
        .collect::<Vec<_>>()
        .join(", ");
    // One unbound operand of a function whose first argument is bound: a
    // finite question the engine answers only for the solvable built-ins.
    if shape == Shape::Func && bound[0] && unbound.len() == 1 {
        return Err(format!(
            "SWRL builtin swrlb:{local} cannot run with argument {which} unbound: it computes \
             its first argument from the others and does not solve for another one. Bind it \
             in a class, property or earlier built-in atom"
        ));
    }
    let hint = match shape {
        Shape::Pred => "it only tests bound values",
        Shape::Func => "it binds its first argument from the others",
        Shape::Ctor => {
            "it binds its first argument from its components, or the components from the first"
        }
        Shape::Special => match local {
            "member" | "sublist" => "it enumerates over a bound list",
            _ => "it binds one side from the other",
        },
    };
    Err(format!(
        "SWRL builtin swrlb:{local} cannot run with argument(s) {which} unbound: {hint}, and \
         this pattern has infinitely many solutions. Bind them in a class, property or \
         earlier built-in atom"
    ))
}

/// Reads RDF list cells from the graphs a rule reads.
pub(crate) trait ListSource {
    /// `(rdf:first, rdf:rest)` of a list cell, when it is one.
    fn cell(&self, node: &Term) -> Option<(Term, Term)>;
}

/// State shared by the built-in calls of one rule evaluation.
pub(crate) struct EvalCtx<'a> {
    lists: &'a dyn ListSource,
    /// Lists minted by this run: node IRI → elements.
    pub(crate) minted: HashMap<String, Vec<Term>>,
    regexes: HashMap<(String, String), Option<Regex>>,
}

impl<'a> EvalCtx<'a> {
    pub(crate) fn new(lists: &'a dyn ListSource) -> Self {
        EvalCtx {
            lists,
            minted: HashMap::new(),
            regexes: HashMap::new(),
        }
    }

    /// The elements of the list `t`, if it is one.
    pub(crate) fn list(&self, t: &Term) -> Option<Vec<Term>> {
        if matches!(t, Term::NamedNode(n) if n.as_str() == RDF_NIL) {
            return Some(Vec::new());
        }
        if let Term::NamedNode(n) = t {
            if let Some(items) = self.minted.get(n.as_str()) {
                return Some(items.clone());
            }
        }
        if matches!(t, Term::Literal(_)) {
            return None;
        }
        let mut items = Vec::new();
        let mut seen = HashSet::new();
        let mut cell = t.clone();
        loop {
            if matches!(&cell, Term::NamedNode(n) if n.as_str() == RDF_NIL) {
                return Some(items);
            }
            if let Term::NamedNode(n) = &cell {
                if let Some(rest) = self.minted.get(n.as_str()) {
                    items.extend(rest.iter().cloned());
                    return Some(items);
                }
            }
            if !seen.insert(cell.clone()) || items.len() > 1_000_000 {
                return None;
            }
            let (first, rest) = self.lists.cell(&cell)?;
            items.push(first);
            cell = rest;
        }
    }

    /// A deterministic node for the list of `items` (`rdf:nil` when empty).
    pub(crate) fn mint(&mut self, items: Vec<Term>) -> Term {
        if items.is_empty() {
            return NamedNode::new_unchecked(RDF_NIL).into();
        }
        let mut head = None;
        for i in 0..items.len() {
            let iri = list_iri(&items[i..]);
            self.minted
                .entry(iri.clone())
                .or_insert_with(|| items[i..].to_vec());
            if i == 0 {
                head = Some(iri);
            }
        }
        NamedNode::new_unchecked(head.expect("non-empty")).into()
    }

    fn regex(&mut self, pattern: &str, flags: &str) -> Option<Regex> {
        self.regexes
            .entry((pattern.to_string(), flags.to_string()))
            .or_insert_with(|| xpath_regex(pattern, flags).ok())
            .clone()
    }
}

/// The IRI of the minted list holding `items`.
pub(crate) fn list_iri(items: &[Term]) -> String {
    let mut h = Sha256::new();
    for item in items {
        h.update(item.to_string().as_bytes());
        h.update(b"\n");
    }
    let digest = h.finalize();
    format!("{LIST_PREFIX}{}", hex::encode(&digest[..16]))
}

/// The cells of the minted list `node` and of minted lists among its
/// elements, as `(subject, rdf:first|rdf:rest, object)` triples.
pub(crate) fn minted_cells(ctx: &EvalCtx<'_>, node: &str, out: &mut Vec<(Term, String, Term)>) {
    let mut stack = vec![node.to_string()];
    let mut done = HashSet::new();
    const FIRST: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#first";
    const REST: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#rest";
    while let Some(n) = stack.pop() {
        if !done.insert(n.clone()) {
            continue;
        }
        let Some(items) = ctx.minted.get(&n) else {
            continue;
        };
        let subject: Term = NamedNode::new_unchecked(n.clone()).into();
        let first = items[0].clone();
        if let Term::NamedNode(f) = &first {
            if ctx.minted.contains_key(f.as_str()) {
                stack.push(f.as_str().to_string());
            }
        }
        let rest: Term = if items.len() == 1 {
            NamedNode::new_unchecked(RDF_NIL).into()
        } else {
            let r = list_iri(&items[1..]);
            stack.push(r.clone());
            NamedNode::new_unchecked(r).into()
        };
        out.push((subject.clone(), FIRST.to_string(), first));
        out.push((subject, REST.to_string(), rest));
    }
}

/// Compile an XPath regular expression (`fn:matches` flags `s`, `m`, `i`,
/// `x`, `q`) to a Rust regex. XPath-only constructs (`\i`, `\c`, class
/// subtraction) are refused rather than read as something else.
pub(crate) fn xpath_regex(pattern: &str, flags: &str) -> Result<Regex, String> {
    let mut prefix = String::new();
    let mut literal = false;
    let mut strip_ws = false;
    for f in flags.chars() {
        match f {
            's' | 'm' | 'i' => prefix.push(f),
            'x' => strip_ws = true,
            'q' => literal = true,
            other => return Err(format!("unknown regular-expression flag '{other}'")),
        }
    }
    let body = if literal {
        regex::escape(pattern)
    } else {
        for bad in ["\\i", "\\I", "\\c", "\\C", "-["] {
            if pattern.contains(bad) {
                return Err(format!(
                    "the XPath regular-expression construct '{bad}' is not supported"
                ));
            }
        }
        if strip_ws {
            // XPath `x`: whitespace outside character classes is removed.
            let mut out = String::new();
            let mut in_class = false;
            let mut escaped = false;
            for c in pattern.chars() {
                if escaped {
                    out.push(c);
                    escaped = false;
                    continue;
                }
                match c {
                    '\\' => {
                        escaped = true;
                        out.push(c);
                    }
                    '[' => {
                        in_class = true;
                        out.push(c);
                    }
                    ']' => {
                        in_class = false;
                        out.push(c);
                    }
                    c if c.is_whitespace() && !in_class => {}
                    c => out.push(c),
                }
            }
            out
        } else {
            pattern.to_string()
        }
    };
    let full = if prefix.is_empty() {
        body
    } else {
        format!("(?{prefix}){body}")
    };
    Regex::new(&full).map_err(|e| format!("invalid regular expression: {e}"))
}

/// An XPath `fn:replace` replacement string in Rust's syntax: `$N` stays a
/// group reference, `\$` and `\\` are literal; anything else is invalid.
fn xpath_replacement(rep: &str) -> Option<String> {
    let mut out = String::new();
    let mut chars = rep.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next() {
                Some('$') => out.push_str("$$"),
                Some('\\') => out.push('\\'),
                _ => return None,
            },
            '$' => {
                let mut digits = String::new();
                while let Some(d) = chars.peek().filter(|d| d.is_ascii_digit()) {
                    digits.push(*d);
                    chars.next();
                }
                if digits.is_empty() {
                    return None;
                }
                out.push_str(&format!("${{{digits}}}"));
            }
            c => out.push(c),
        }
    }
    Some(out)
}

/// Check the constant arguments a built-in reads at compile time: a regular
/// expression that cannot compile refuses the rule.
pub(crate) fn check_constants(local: &str, args: &[Option<Term>]) -> Result<(), String> {
    let (pat, flags) = match local {
        "matches" => (1, 2),
        "replace" => (2, 4),
        "tokenize" => (2, 3),
        _ => return Ok(()),
    };
    let flag = args
        .get(flags)
        .and_then(|a| a.as_ref())
        .and_then(Value::string);
    if let Some(Some(p)) = args.get(pat).map(|a| a.as_ref().and_then(Value::string)) {
        xpath_regex(&p, flag.as_deref().unwrap_or(""))
            .map_err(|e| format!("swrlb:{local}: pattern \"{p}\": {e}"))?;
    }
    if local == "replace" {
        if let Some(Some(r)) = args.get(3).map(|a| a.as_ref().and_then(Value::string)) {
            xpath_replacement(&r)
                .ok_or_else(|| format!("swrlb:replace: invalid replacement string \"{r}\""))?;
        }
    }
    Ok(())
}

// ─── evaluation ───────────────────────────────────────────────────────────

/// Evaluate built-in `local` (resolved by [`resolve`]) over `args` (`None`
/// for an unbound argument). Returns one complete argument list per
/// solution: none when it does not hold.
pub(crate) fn eval(local: &str, args: &[Option<Term>], ctx: &mut EvalCtx<'_>) -> Vec<Vec<Term>> {
    let Some((shape, ..)) = lookup(local) else {
        return Vec::new();
    };
    let all: Option<Vec<Term>> = args.iter().cloned().collect();
    match shape {
        Shape::Pred => match all {
            Some(a) if predicate(local, &a, ctx) => vec![a],
            _ => Vec::new(),
        },
        Shape::Special => special(local, args, ctx),
        Shape::Func | Shape::Ctor => {
            let rest: Option<Vec<Term>> = args[1..].iter().cloned().collect();
            match (&args[0], rest) {
                (first, Some(rest)) => {
                    let values = function(local, &rest, ctx);
                    match first {
                        None => values
                            .into_iter()
                            .map(|v| {
                                let mut a = vec![v];
                                a.extend(rest.iter().cloned());
                                a
                            })
                            .collect(),
                        Some(f) => {
                            let list_valued = list_valued(local);
                            let holds = values.iter().any(|v| {
                                if list_valued {
                                    lists_equal(ctx, v, f)
                                } else {
                                    terms_equal(v, f)
                                }
                            });
                            if holds {
                                vec![all.expect("every argument bound")]
                            } else {
                                Vec::new()
                            }
                        }
                    }
                }
                (Some(first), None) if shape == Shape::Ctor => split(local, first, args),
                (Some(first), None) => solve(local, first, args),
                (None, None) => Vec::new(),
            }
        }
    }
}

fn list_valued(local: &str) -> bool {
    matches!(
        local,
        "listConcat" | "listIntersection" | "listSubtraction" | "rest"
    )
}

fn lists_equal(ctx: &EvalCtx<'_>, a: &Term, b: &Term) -> bool {
    match (ctx.list(a), ctx.list(b)) {
        (Some(x), Some(y)) => {
            x.len() == y.len() && x.iter().zip(&y).all(|(p, q)| terms_equal(p, q))
        }
        _ => false,
    }
}

fn contains_term(items: &[Term], t: &Term) -> bool {
    items.iter().any(|i| terms_equal(i, t))
}

fn s(t: &Term) -> Option<String> {
    Value::string(t)
}

/// A built-in that only tests (all arguments bound).
fn predicate(local: &str, a: &[Term], ctx: &mut EvalCtx<'_>) -> bool {
    use std::cmp::Ordering::*;
    match local {
        "notEqual" => !terms_equal(&a[0], &a[1]),
        "lessThan" => compare_terms(&a[0], &a[1]) == Some(Less),
        "lessThanOrEqual" => matches!(compare_terms(&a[0], &a[1]), Some(Less | Equal)),
        "greaterThan" => compare_terms(&a[0], &a[1]) == Some(Greater),
        "greaterThanOrEqual" => matches!(compare_terms(&a[0], &a[1]), Some(Greater | Equal)),
        "stringEqualIgnoreCase" => match (s(&a[0]), s(&a[1])) {
            (Some(x), Some(y)) => x.to_lowercase() == y.to_lowercase(),
            _ => false,
        },
        "contains" => matches!((s(&a[0]), s(&a[1])), (Some(x), Some(y)) if x.contains(&y)),
        "containsIgnoreCase" => matches!((s(&a[0]), s(&a[1])),
            (Some(x), Some(y)) if x.to_lowercase().contains(&y.to_lowercase())),
        "startsWith" => matches!((s(&a[0]), s(&a[1])), (Some(x), Some(y)) if x.starts_with(&y)),
        "endsWith" => matches!((s(&a[0]), s(&a[1])), (Some(x), Some(y)) if x.ends_with(&y)),
        "matches" => {
            let flags = a.get(2).and_then(s).unwrap_or_default();
            match (s(&a[0]), s(&a[1])) {
                (Some(input), Some(p)) => ctx.regex(&p, &flags).is_some_and(|r| r.is_match(&input)),
                _ => false,
            }
        }
        _ => false,
    }
}

/// `equal`, `member`, `sublist` and `empty`.
fn special(local: &str, args: &[Option<Term>], ctx: &mut EvalCtx<'_>) -> Vec<Vec<Term>> {
    match local {
        "equal" => match (&args[0], &args[1]) {
            (Some(a), Some(b)) if terms_equal(a, b) => vec![vec![a.clone(), b.clone()]],
            (Some(a), None) => vec![vec![a.clone(), a.clone()]],
            (None, Some(b)) => vec![vec![b.clone(), b.clone()]],
            _ => Vec::new(),
        },
        "member" => {
            let Some(list) = args[1].as_ref() else {
                return Vec::new();
            };
            let Some(items) = ctx.list(list) else {
                return Vec::new();
            };
            match &args[0] {
                Some(e) if contains_term(&items, e) => vec![vec![e.clone(), list.clone()]],
                Some(_) => Vec::new(),
                None => {
                    let mut out: Vec<Vec<Term>> = Vec::new();
                    for i in items {
                        if !out.iter().any(|o| o[0] == i) {
                            out.push(vec![i, list.clone()]);
                        }
                    }
                    out
                }
            }
        }
        "sublist" => {
            let Some(list) = args[0].as_ref() else {
                return Vec::new();
            };
            let Some(items) = ctx.list(list) else {
                return Vec::new();
            };
            match &args[1] {
                Some(sub) => {
                    let Some(sub_items) = ctx.list(sub) else {
                        return Vec::new();
                    };
                    let found = sub_items.is_empty()
                        || items
                            .windows(sub_items.len())
                            .any(|w| w.iter().zip(&sub_items).all(|(p, q)| terms_equal(p, q)));
                    if found {
                        vec![vec![list.clone(), sub.clone()]]
                    } else {
                        Vec::new()
                    }
                }
                None => {
                    let mut out = vec![vec![list.clone(), ctx.mint(Vec::new())]];
                    for start in 0..items.len() {
                        for end in start + 1..=items.len() {
                            let node = ctx.mint(items[start..end].to_vec());
                            if !out.iter().any(|o| o[1] == node) {
                                out.push(vec![list.clone(), node]);
                            }
                        }
                    }
                    out
                }
            }
        }
        "empty" => match &args[0] {
            Some(l) if ctx.list(l).is_some_and(|i| i.is_empty()) => vec![vec![l.clone()]],
            Some(_) => Vec::new(),
            None => vec![vec![ctx.mint(Vec::new())]],
        },
        _ => Vec::new(),
    }
}

/// Solve for the one unbound operand of `add`, `subtract`, `unaryPlus`,
/// `unaryMinus` or `booleanNot`, the first argument bound.
fn solve(local: &str, first: &Term, args: &[Option<Term>]) -> Vec<Vec<Term>> {
    let Some(hole) = (1..args.len()).find(|i| args[*i].is_none()) else {
        return Vec::new();
    };
    let num = |i: usize| args[i].as_ref().and_then(Value::num);
    let value: Option<Term> = (|| -> Option<Term> {
        let r = Value::num(first);
        Some(match local {
            "add" => {
                let mut x = r?;
                for i in (1..args.len()).filter(|i| *i != hole) {
                    x = Num::sub(x, num(i)?)?;
                }
                x.to_literal().into()
            }
            "subtract" if hole == 1 => Num::add(r?, num(2)?)?.to_literal().into(),
            "subtract" => Num::sub(num(1)?, r?)?.to_literal().into(),
            "unaryPlus" => r?.to_literal().into(),
            "unaryMinus" => r?.neg()?.to_literal().into(),
            "booleanNot" => bool_literal(!Value::boolean(first)?),
            _ => return None,
        })
    })();
    match value {
        Some(v) => {
            let mut out: Vec<Term> = args
                .iter()
                .map(|a| a.clone().unwrap_or(v.clone()))
                .collect();
            out[hole] = v;
            vec![out]
        }
        None => Vec::new(),
    }
}

/// The values of a function's first argument computed from the rest.
fn function(local: &str, rest: &[Term], ctx: &mut EvalCtx<'_>) -> Vec<Term> {
    let one = |v: Option<Term>| v.into_iter().collect::<Vec<_>>();
    match local {
        "tokenize" => tokenize(rest, ctx),
        "listConcat" => {
            let mut items = Vec::new();
            for l in rest {
                match ctx.list(l) {
                    Some(i) => items.extend(i),
                    None => return Vec::new(),
                }
            }
            vec![ctx.mint(items)]
        }
        "listIntersection" | "listSubtraction" => {
            let (Some(a), Some(b)) = (ctx.list(&rest[0]), ctx.list(&rest[1])) else {
                return Vec::new();
            };
            let keep = local == "listIntersection";
            let items: Vec<Term> = a
                .into_iter()
                .filter(|i| contains_term(&b, i) == keep)
                .collect();
            vec![ctx.mint(items)]
        }
        "length" => one(ctx.list(&rest[0]).map(|i| int_literal(i.len() as i64))),
        "first" => one(ctx.list(&rest[0]).and_then(|i| i.first().cloned())),
        "rest" => {
            // The stored rest cell when the list is in the data; a minted
            // one when it is not.
            if let Some((_, r)) = ctx.lists.cell(&rest[0]) {
                return vec![r];
            }
            match ctx.list(&rest[0]) {
                Some(items) if !items.is_empty() => vec![ctx.mint(items[1..].to_vec())],
                _ => Vec::new(),
            }
        }
        _ => one(scalar(local, rest)),
    }
}

fn tokenize(rest: &[Term], ctx: &mut EvalCtx<'_>) -> Vec<Term> {
    let (Some(input), Some(p)) = (s(&rest[0]), s(&rest[1])) else {
        return Vec::new();
    };
    let flags = rest.get(2).and_then(s).unwrap_or_default();
    let Some(re) = ctx.regex(&p, &flags) else {
        return Vec::new();
    };
    if input.is_empty() || re.is_match("") {
        return Vec::new();
    }
    let mut out: Vec<Term> = Vec::new();
    for tok in re.split(&input) {
        let t = string_literal(tok);
        if !out.contains(&t) {
            out.push(t);
        }
    }
    out
}

fn nums(ts: &[Term]) -> Option<Vec<Num>> {
    ts.iter().map(Value::num).collect()
}

fn lit(lex: String, dt: &str) -> Term {
    Literal::new_typed_literal(lex, xsd(dt)).into()
}

fn duration(t: &Term) -> Option<Duration> {
    match Value::of_term(t) {
        Value::Duration(d) => Some(d),
        _ => None,
    }
}

fn ym(t: &Term) -> Option<YearMonthDuration> {
    YearMonthDuration::try_from(duration(t)?).ok()
}

fn dt_dur(t: &Term) -> Option<DayTimeDuration> {
    DayTimeDuration::try_from(duration(t)?).ok()
}

fn date_time(t: &Term) -> Option<DateTime> {
    match Value::of_term(t) {
        Value::DateTime(d) => Some(d),
        _ => None,
    }
}

fn date(t: &Term) -> Option<Date> {
    match Value::of_term(t) {
        Value::Date(d) => Some(d),
        _ => None,
    }
}

fn time(t: &Term) -> Option<Time> {
    match Value::of_term(t) {
        Value::Time(d) => Some(d),
        _ => None,
    }
}

fn ym_lit(d: YearMonthDuration) -> Term {
    lit(d.to_string(), "yearMonthDuration")
}

fn dt_lit(d: DayTimeDuration) -> Term {
    lit(d.to_string(), "dayTimeDuration")
}

/// A number as a decimal (for duration arithmetic).
fn decimal(n: Num) -> Option<Decimal> {
    match n {
        Num::I(i) => Some(Decimal::from(i)),
        Num::D(d) => Some(d),
        Num::F(f) => Decimal::try_from(f).ok(),
        Num::Db(d) => Decimal::try_from(d).ok(),
    }
}

/// A function built-in of scalar values.
fn scalar(local: &str, a: &[Term]) -> Option<Term> {
    let n = |i: usize| a.get(i).and_then(Value::num);
    let f = |t: Num| -> Term { t.to_literal().into() };
    let double = |x: f64| -> Term { Num::Db(Double::from(x)).to_literal().into() };
    Some(match local {
        "add" => {
            let mut acc = Num::from_i64(0);
            for x in nums(a)? {
                acc = Num::add(acc, x)?;
            }
            f(acc)
        }
        "multiply" => {
            let mut acc = Num::from_i64(1);
            for x in nums(a)? {
                acc = Num::mul(acc, x)?;
            }
            f(acc)
        }
        "subtract" => f(Num::sub(n(0)?, n(1)?)?),
        "divide" => f(Num::div(n(0)?, n(1)?)?),
        "integerDivide" => f(Num::idiv(n(0)?, n(1)?)?),
        "mod" => f(Num::rem(n(0)?, n(1)?)?),
        "pow" => {
            let (b, e) = (n(0)?, n(1)?);
            match (b, e.as_i64()) {
                (Num::I(_), Some(k)) if (0..=4096).contains(&k) && matches!(e, Num::I(_)) => {
                    let mut acc = Num::from_i64(1);
                    for _ in 0..k {
                        acc = Num::mul(acc, b)?;
                    }
                    f(acc)
                }
                _ => double(b.to_f64().powf(e.to_f64())),
            }
        }
        "unaryPlus" => f(n(0)?),
        "unaryMinus" => f(n(0)?.neg()?),
        "abs" => f(n(0)?.abs()?),
        "ceiling" => f(n(0)?.ceil()?),
        "floor" => f(n(0)?.floor()?),
        "round" => f(n(0)?.round()?),
        "roundHalfToEven" => {
            let p = match a.get(1) {
                Some(t) => Value::num(t)?.as_i64()?,
                None => 0,
            };
            f(n(0)?.round_half_even(p)?)
        }
        "sin" => double(n(0)?.to_f64().sin()),
        "cos" => double(n(0)?.to_f64().cos()),
        "tan" => double(n(0)?.to_f64().tan()),
        "booleanNot" => bool_literal(!Value::boolean(&a[0])?),
        // §8.4: string arguments are plain, xsd:string-family, language-tagged
        // or xsd:anyURI literals; stringConcat also takes any other literal by
        // its lexical form, as fn:concat does.
        "stringConcat" => {
            let mut out = String::new();
            for t in a {
                match t {
                    Term::Literal(l) => out.push_str(l.value()),
                    _ => return None,
                }
            }
            string_literal(out)
        }
        "substring" => {
            let src: Vec<char> = s(&a[0])?.chars().collect();
            let start = n(1)?.to_f64();
            let len = match a.get(2) {
                Some(t) => Value::num(t)?.to_f64(),
                None => f64::INFINITY,
            };
            let from = xpath_round(start);
            let to = from + xpath_round(len);
            let out: String = src
                .iter()
                .enumerate()
                .filter(|(i, _)| {
                    let p = (*i + 1) as f64;
                    p >= from && p < to
                })
                .map(|(_, c)| *c)
                .collect();
            string_literal(out)
        }
        "stringLength" => int_literal(s(&a[0])?.chars().count() as i64),
        "normalizeSpace" => {
            string_literal(s(&a[0])?.split_whitespace().collect::<Vec<_>>().join(" "))
        }
        "upperCase" => string_literal(s(&a[0])?.to_uppercase()),
        "lowerCase" => string_literal(s(&a[0])?.to_lowercase()),
        "translate" => {
            let (src, map, trans) = (s(&a[0])?, s(&a[1])?, s(&a[2])?);
            let map: Vec<char> = map.chars().collect();
            let trans: Vec<char> = trans.chars().collect();
            let out: String = src
                .chars()
                .filter_map(|c| match map.iter().position(|m| *m == c) {
                    Some(i) => trans.get(i).copied(),
                    None => Some(c),
                })
                .collect();
            string_literal(out)
        }
        "substringBefore" => {
            let (x, y) = (s(&a[0])?, s(&a[1])?);
            string_literal(x.find(&y).map(|i| x[..i].to_string()).unwrap_or_default())
        }
        "substringAfter" => {
            let (x, y) = (s(&a[0])?, s(&a[1])?);
            string_literal(
                x.find(&y)
                    .map(|i| x[i + y.len()..].to_string())
                    .unwrap_or_default(),
            )
        }
        "replace" => {
            let (input, p, rep) = (s(&a[0])?, s(&a[1])?, s(&a[2])?);
            let flags = a.get(3).and_then(s).unwrap_or_default();
            let re = xpath_regex(&p, &flags).ok()?;
            if re.is_match("") {
                return None;
            }
            let rep = if flags.contains('q') {
                rep.replace('$', "$$")
            } else {
                xpath_replacement(&rep)?
            };
            string_literal(re.replace_all(&input, rep.as_str()).into_owned())
        }
        // §8.5
        "addYearMonthDurations" => {
            let mut acc = YearMonthDuration::new(0);
            for t in a {
                acc = acc.checked_add(ym(t)?)?;
            }
            ym_lit(acc)
        }
        "subtractYearMonthDurations" => ym_lit(ym(&a[0])?.checked_sub(ym(&a[1])?)?),
        "multiplyYearMonthDuration" => {
            let months = Num::from_i64(ym(&a[0])?.months() + 12 * ym(&a[0])?.years());
            let r = Num::mul(months, n(1)?)?.round()?.as_i64()?;
            ym_lit(YearMonthDuration::new(r))
        }
        "divideYearMonthDurations" => {
            let d = ym(&a[0])?;
            let months = Num::from_i64(d.months() + 12 * d.years());
            match ym(&a[1]) {
                Some(other) => {
                    let o = Num::from_i64(other.months() + 12 * other.years());
                    f(Num::div(months, o)?)
                }
                None => {
                    let r = Num::div(months, n(1)?)?.round()?.as_i64()?;
                    ym_lit(YearMonthDuration::new(r))
                }
            }
        }
        "addDayTimeDurations" => {
            let mut acc = DayTimeDuration::new(0);
            for t in a {
                acc = acc.checked_add(dt_dur(t)?)?;
            }
            dt_lit(acc)
        }
        "subtractDayTimeDurations" => dt_lit(dt_dur(&a[0])?.checked_sub(dt_dur(&a[1])?)?),
        "multiplyDayTimeDurations" => {
            let secs = Num::D(dt_dur(&a[0])?.as_seconds());
            dt_lit(DayTimeDuration::new(decimal(Num::mul(secs, n(1)?)?)?))
        }
        "divideDayTimeDuration" => {
            let secs = Num::D(dt_dur(&a[0])?.as_seconds());
            match dt_dur(&a[1]) {
                Some(other) => f(Num::div(secs, Num::D(other.as_seconds()))?),
                None => dt_lit(DayTimeDuration::new(decimal(Num::div(secs, n(1)?)?)?)),
            }
        }
        "subtractDates" => dt_lit(date(&a[0])?.checked_sub(date(&a[1])?)?),
        "subtractTimes" => dt_lit(time(&a[0])?.checked_sub(time(&a[1])?)?),
        "addYearMonthDurationToDateTime" => {
            dt_lit_dt(date_time(&a[0])?.checked_add_year_month_duration(ym(&a[1])?)?)
        }
        "addDayTimeDurationToDateTime" => {
            dt_lit_dt(date_time(&a[0])?.checked_add_day_time_duration(dt_dur(&a[1])?)?)
        }
        "subtractYearMonthDurationFromDateTime" => {
            dt_lit_dt(date_time(&a[0])?.checked_sub_year_month_duration(ym(&a[1])?)?)
        }
        "subtractDayTimeDurationFromDateTime" => {
            dt_lit_dt(date_time(&a[0])?.checked_sub_day_time_duration(dt_dur(&a[1])?)?)
        }
        "addYearMonthDurationToDate" => lit(
            date(&a[0])?
                .checked_add_year_month_duration(ym(&a[1])?)?
                .to_string(),
            "date",
        ),
        "addDayTimeDurationToDate" => lit(
            date(&a[0])?
                .checked_add_day_time_duration(dt_dur(&a[1])?)?
                .to_string(),
            "date",
        ),
        "subtractYearMonthDurationFromDate" => lit(
            date(&a[0])?
                .checked_sub_year_month_duration(ym(&a[1])?)?
                .to_string(),
            "date",
        ),
        "subtractDayTimeDurationFromDate" => lit(
            date(&a[0])?
                .checked_sub_day_time_duration(dt_dur(&a[1])?)?
                .to_string(),
            "date",
        ),
        "addDayTimeDurationToTime" => lit(
            time(&a[0])?
                .checked_add_day_time_duration(dt_dur(&a[1])?)?
                .to_string(),
            "time",
        ),
        "subtractDayTimeDurationFromTime" => lit(
            time(&a[0])?
                .checked_sub_day_time_duration(dt_dur(&a[1])?)?
                .to_string(),
            "time",
        ),
        "subtractDateTimesYieldingDayTimeDuration" => {
            dt_lit(date_time(&a[0])?.checked_sub(date_time(&a[1])?)?)
        }
        "subtractDateTimesYieldingYearMonthDuration" => {
            let (x, y) = (date_time(&a[0])?, date_time(&a[1])?);
            ym_lit(YearMonthDuration::new(whole_months_between(x, y)?))
        }
        // §8.6
        "resolveURI" => {
            let (rel, base) = (uri_text(&a[0])?, uri_text(&a[1])?);
            let base = oxiri::Iri::parse(base).ok()?;
            lit(base.resolve(&rel).ok()?.into_inner(), "anyURI")
        }
        // constructors, forwards
        "yearMonthDuration" => {
            let months = Num::add(Num::mul(n(0)?, Num::from_i64(12))?, n(1)?)?.as_i64()?;
            ym_lit(YearMonthDuration::new(months))
        }
        "dayTimeDuration" => {
            let mut secs = Num::mul(n(0)?, Num::from_i64(86_400))?;
            secs = Num::add(secs, Num::mul(n(1)?, Num::from_i64(3_600))?)?;
            secs = Num::add(secs, Num::mul(n(2)?, Num::from_i64(60))?)?;
            secs = Num::add(secs, n(3)?)?;
            dt_lit(DayTimeDuration::new(decimal(secs)?))
        }
        "dateTime" => {
            let tz = timezone_text(a.get(6))?;
            let lex = format!(
                "{}-{:02}-{:02}T{:02}:{:02}:{}{tz}",
                year_text(n(0)?.as_i64()?),
                n(1)?.as_i64()?,
                n(2)?.as_i64()?,
                n(3)?.as_i64()?,
                n(4)?.as_i64()?,
                seconds_text(n(5)?)?
            );
            let v: DateTime = lex.parse().ok()?;
            dt_lit_dt(v)
        }
        "date" => {
            let tz = timezone_text(a.get(3))?;
            let lex = format!(
                "{}-{:02}-{:02}{tz}",
                year_text(n(0)?.as_i64()?),
                n(1)?.as_i64()?,
                n(2)?.as_i64()?
            );
            let v: Date = lex.parse().ok()?;
            lit(v.to_string(), "date")
        }
        "time" => {
            let tz = timezone_text(a.get(3))?;
            let lex = format!(
                "{:02}:{:02}:{}{tz}",
                n(0)?.as_i64()?,
                n(1)?.as_i64()?,
                seconds_text(n(2)?)?
            );
            let v: Time = lex.parse().ok()?;
            lit(v.to_string(), "time")
        }
        "anyURI" => {
            let part = |i: usize| {
                a.get(i).and_then(|t| match t {
                    Term::Literal(l) => Some(l.value().to_string()),
                    _ => None,
                })
            };
            let (scheme, host, port, path, query, fragment) =
                (part(0)?, part(1)?, part(2)?, part(3)?, part(4)?, part(5)?);
            let mut out = format!("{scheme}:");
            if !host.is_empty() {
                out.push_str("//");
                out.push_str(&host);
                if !port.is_empty() {
                    out.push(':');
                    out.push_str(&port);
                }
            }
            out.push_str(&path);
            if !query.is_empty() {
                out.push('?');
                out.push_str(&query);
            }
            if !fragment.is_empty() {
                out.push('#');
                out.push_str(&fragment);
            }
            oxiri::Iri::parse(out.as_str()).ok()?;
            lit(out, "anyURI")
        }
        _ => return None,
    })
}

fn dt_lit_dt(d: DateTime) -> Term {
    lit(d.to_string(), "dateTime")
}

/// `fn:round` on a double (halves toward positive infinity), keeping
/// infinities.
fn xpath_round(x: f64) -> f64 {
    if x.is_finite() {
        (x + 0.5).floor()
    } else {
        x
    }
}

fn uri_text(t: &Term) -> Option<String> {
    match t {
        Term::NamedNode(n) => Some(n.as_str().to_string()),
        _ => Value::string(t),
    }
}

fn year_text(y: i64) -> String {
    if y < 0 {
        format!("-{:04}", -y)
    } else {
        format!("{y:04}")
    }
}

fn seconds_text(s: Num) -> Option<String> {
    let d = decimal(s)?;
    if d.is_negative() {
        return None;
    }
    let text = d.to_string();
    let int_len = text.split('.').next().map(str::len).unwrap_or(0);
    Some(if int_len < 2 {
        format!("0{text}")
    } else {
        text
    })
}

/// A timezone argument: `""` (none), `"Z"`, `"+01:00"`, or a
/// `dayTimeDuration` offset.
fn timezone_text(t: Option<&Term>) -> Option<String> {
    let Some(t) = t else {
        return Some(String::new());
    };
    if let Some(d) = dt_dur(t) {
        let tz = oxsdatatypes::TimezoneOffset::try_from(d).ok()?;
        return Some(tz.to_string());
    }
    let text = Value::string(t)?;
    let text = text.trim();
    let ok = text.is_empty()
        || text == "Z"
        || regex::Regex::new(r"^[+-](0[0-9]|1[0-4]):[0-5][0-9]$").is_ok_and(|r| r.is_match(text));
    ok.then(|| text.to_string())
}

/// `fn:subtract-dateTimes-yielding-yearMonthDuration` (XPath 1.0 draft):
/// the whole months from `y` to `x`.
fn whole_months_between(x: DateTime, y: DateTime) -> Option<i64> {
    let x = x.adjust(Some(oxsdatatypes::TimezoneOffset::UTC))?;
    let y = y.adjust(Some(oxsdatatypes::TimezoneOffset::UTC))?;
    let (big, small, sign) = if x >= y { (x, y, 1) } else { (y, x, -1) };
    let mut months =
        (big.year() - small.year()) * 12 + i64::from(big.month()) - i64::from(small.month());
    // Not a whole month yet when the day and time have not come round.
    let rest = |d: DateTime| (d.day(), d.hour(), d.minute(), d.second());
    if rest(big) < rest(small) {
        months -= 1;
    }
    Some(sign * months)
}

/// A constructor's components from its bound first argument, checked against
/// the bound ones and binding the rest.
fn split(local: &str, first: &Term, args: &[Option<Term>]) -> Vec<Vec<Term>> {
    let components: Option<Vec<Term>> = (|| -> Option<Vec<Term>> {
        let int = int_literal;
        let secs = |d: Decimal| -> Term {
            let n = Num::D(d);
            match n.as_i64() {
                Some(i) => int_literal(i),
                None => n.to_literal().into(),
            }
        };
        let tz = |o: Option<oxsdatatypes::TimezoneOffset>| -> Term {
            string_literal(o.map(|o| o.to_string()).unwrap_or_default())
        };
        Some(match local {
            "yearMonthDuration" => {
                let d = ym(first)?;
                vec![int(d.years()), int(d.months())]
            }
            "dayTimeDuration" => {
                let d = dt_dur(first)?;
                vec![
                    int(d.days()),
                    int(d.hours()),
                    int(d.minutes()),
                    secs(d.seconds()),
                ]
            }
            "dateTime" => {
                let d = date_time(first)?;
                let mut v = vec![
                    int(d.year()),
                    int(d.month().into()),
                    int(d.day().into()),
                    int(d.hour().into()),
                    int(d.minute().into()),
                    secs(d.second()),
                ];
                if args.len() == 8 {
                    v.push(tz(d.timezone_offset()));
                }
                v
            }
            "date" => {
                let d = date(first)?;
                let mut v = vec![int(d.year()), int(d.month().into()), int(d.day().into())];
                if args.len() == 5 {
                    v.push(tz(d.timezone_offset()));
                }
                v
            }
            "time" => {
                let d = time(first)?;
                let mut v = vec![
                    int(d.hour().into()),
                    int(d.minute().into()),
                    secs(d.second()),
                ];
                if args.len() == 5 {
                    v.push(tz(d.timezone_offset()));
                }
                v
            }
            "anyURI" => {
                let text = uri_text(first)?;
                let iri = oxiri::Iri::parse(text.as_str()).ok()?;
                let (host, port) = match iri.authority() {
                    Some(auth) => {
                        let host_port = auth.rsplit_once('@').map(|(_, h)| h).unwrap_or(auth);
                        // A port follows the last ':' outside an IPv6 literal
                        // (`[::1]:8080`).
                        let split_at = match host_port.rfind(']') {
                            Some(close) => host_port[close..].find(':').map(|i| close + i),
                            None => host_port.rfind(':'),
                        };
                        match split_at {
                            Some(i) if host_port[i + 1..].bytes().all(|b| b.is_ascii_digit()) => {
                                (host_port[..i].to_string(), host_port[i + 1..].to_string())
                            }
                            _ => (host_port.to_string(), String::new()),
                        }
                    }
                    None => (String::new(), String::new()),
                };
                vec![
                    string_literal(iri.scheme()),
                    string_literal(host),
                    string_literal(port),
                    string_literal(iri.path()),
                    string_literal(iri.query().unwrap_or("")),
                    string_literal(iri.fragment().unwrap_or("")),
                ]
            }
            _ => return None,
        })
    })();
    let Some(components) = components else {
        return Vec::new();
    };
    if components.len() + 1 != args.len() {
        return Vec::new();
    }
    let mut out = vec![first.clone()];
    for (arg, value) in args[1..].iter().zip(components) {
        match arg {
            Some(bound) if !terms_equal(bound, &value) => return Vec::new(),
            Some(bound) => out.push(bound.clone()),
            None => out.push(value),
        }
    }
    vec![out]
}

// ─── rule steps ───────────────────────────────────────────────────────────

/// An argument of a step: a generated variable name (without `?`) or a
/// constant term.
#[derive(Debug, Clone)]
pub(crate) enum StepArg {
    Var(String),
    Const(Term),
}

/// What a step evaluates.
#[derive(Debug, Clone)]
pub(crate) enum StepKind {
    /// A built-in, by its resolved local name.
    Builtin(String),
    /// A `DataRangeAtom`.
    Range(DataRange),
}

/// One built-in or data-range atom of a rule body, evaluated after the body's
/// class and property atoms.
#[derive(Debug, Clone)]
pub(crate) struct Step {
    pub(crate) kind: StepKind,
    pub(crate) args: Vec<StepArg>,
    /// How the atom reads in a message.
    pub(crate) label: String,
}

impl Step {
    fn bound_flags(&self, bound: &HashSet<String>) -> Vec<bool> {
        self.args
            .iter()
            .map(|a| match a {
                StepArg::Const(_) => true,
                StepArg::Var(v) => bound.contains(v),
            })
            .collect()
    }

    fn repeated_unbound(&self, bound: &HashSet<String>) -> Option<String> {
        let mut seen = HashSet::new();
        for a in &self.args {
            if let StepArg::Var(v) = a {
                if !bound.contains(v) && !seen.insert(v.clone()) {
                    return Some(v.clone());
                }
            }
        }
        None
    }

    fn pattern(&self, bound: &HashSet<String>) -> Result<(), String> {
        if let Some(v) = self.repeated_unbound(bound) {
            return Err(format!(
                "{}: variable ?{v} occurs unbound in more than one argument",
                self.label
            ));
        }
        let flags = self.bound_flags(bound);
        match &self.kind {
            StepKind::Builtin(local) => check_pattern(local, &flags),
            StepKind::Range(range) => {
                if flags[0] || datarange::enumerate(range).is_some() {
                    Ok(())
                } else {
                    Err(format!(
                        "{}: the data range {range} holds infinitely many values, so its \
                         variable must be bound by a class, property or built-in atom",
                        self.label
                    ))
                }
            }
        }
    }

    fn vars(&self) -> impl Iterator<Item = &str> {
        self.args.iter().filter_map(|a| match a {
            StepArg::Var(v) => Some(v.as_str()),
            _ => None,
        })
    }

    /// Evaluate the step over one binding, returning its extensions.
    pub(crate) fn eval(
        &self,
        binding: &HashMap<String, Term>,
        ctx: &mut EvalCtx<'_>,
    ) -> Vec<HashMap<String, Term>> {
        let args: Vec<Option<Term>> = self
            .args
            .iter()
            .map(|a| match a {
                StepArg::Const(t) => Some(t.clone()),
                StepArg::Var(v) => binding.get(v).cloned(),
            })
            .collect();
        let solutions: Vec<Vec<Term>> = match &self.kind {
            StepKind::Builtin(local) => eval(local, &args, ctx),
            StepKind::Range(range) => match &args[0] {
                Some(v) if datarange::holds(range, v) => vec![vec![v.clone()]],
                Some(_) => Vec::new(),
                None => datarange::enumerate(range)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|l| vec![Term::Literal(l)])
                    .collect(),
            },
        };
        let mut out = Vec::new();
        'solutions: for sol in solutions {
            let mut b = binding.clone();
            for (arg, value) in self.args.iter().zip(sol) {
                if let StepArg::Var(v) = arg {
                    match b.get(v) {
                        Some(existing) if *existing != value && !terms_equal(existing, &value) => {
                            continue 'solutions
                        }
                        Some(_) => {}
                        None => {
                            b.insert(v.clone(), value);
                        }
                    }
                }
            }
            out.push(b);
        }
        out
    }
}

/// Order `steps` so each runs once the variables it needs are bound,
/// starting from `bound` (the variables the body's other atoms bind), which
/// grows by what each step binds. Refuses a step no order can evaluate,
/// with the reason.
pub(crate) fn plan(mut steps: Vec<Step>, bound: &mut HashSet<String>) -> Result<Vec<Step>, String> {
    let mut ordered = Vec::with_capacity(steps.len());
    while !steps.is_empty() {
        match steps.iter().position(|s| s.pattern(bound).is_ok()) {
            Some(i) => {
                let step = steps.remove(i);
                bound.extend(step.vars().map(str::to_string));
                ordered.push(step);
            }
            None => {
                let err = steps[0].pattern(bound).expect_err("no step is ready");
                return Err(err);
            }
        }
    }
    Ok(ordered)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NoLists;
    impl ListSource for NoLists {
        fn cell(&self, _: &Term) -> Option<(Term, Term)> {
            None
        }
    }

    fn int(i: i64) -> Term {
        int_literal(i)
    }

    fn run(local: &str, args: &[Option<Term>]) -> Vec<Vec<Term>> {
        let mut ctx = EvalCtx::new(&NoLists);
        eval(local, args, &mut ctx)
    }

    #[test]
    fn every_builtin_is_known_and_patterns_are_checked() {
        assert_eq!(BUILTINS.len(), 79);
        assert!(resolve(&format!("{SWRLB}add"), 3).is_ok());
        assert!(resolve(&format!("{SWRLB}substring"), 5)
            .unwrap_err()
            .contains("3 to 4"));
        assert!(resolve("http://ex/fn#add", 3)
            .unwrap_err()
            .contains("Unsupported SWRL builtin"));
        assert!(check_pattern("add", &[false, true, true]).is_ok());
        assert!(check_pattern("add", &[true, false, true]).is_ok());
        let err = check_pattern("add", &[false, false, true]).unwrap_err();
        assert!(
            err.contains("infinitely many") && err.contains("1, 2"),
            "{err}"
        );
        assert!(check_pattern("lessThan", &[true, false]).is_err());
        let err = check_pattern("multiply", &[true, false, true]).unwrap_err();
        assert!(err.contains("does not solve"), "{err}");
        assert!(check_pattern(
            "dateTime",
            &[true, false, false, false, false, false, false]
        )
        .is_ok());
        assert!(check_pattern("member", &[false, true]).is_ok());
        assert!(check_pattern("member", &[true, false]).is_err());
    }

    #[test]
    fn bind_check_and_solve() {
        assert_eq!(
            run("add", &[None, Some(int(2)), Some(int(3))]),
            vec![vec![int(5), int(2), int(3)]]
        );
        assert_eq!(
            run("add", &[Some(int(5)), Some(int(2)), Some(int(3))]).len(),
            1
        );
        assert!(run("add", &[Some(int(6)), Some(int(2)), Some(int(3))]).is_empty());
        assert_eq!(
            run("add", &[Some(int(5)), None, Some(int(3))])[0][1],
            int(2)
        );
        assert_eq!(
            run("subtract", &[Some(int(1)), Some(int(3)), None])[0][2],
            int(2)
        );
        assert!(run("add", &[None, Some(string_literal("a")), Some(int(1))]).is_empty());
        assert_eq!(
            run(
                "tokenize",
                &[
                    None,
                    Some(string_literal("a,b,,c")),
                    Some(string_literal(","))
                ]
            )
            .len(),
            4
        );
        let split = run(
            "dateTime",
            &[
                Some(lit("2024-02-29T10:30:05Z".into(), "dateTime")),
                None,
                None,
                None,
                None,
                None,
                None,
                None,
            ],
        );
        assert_eq!(split[0][1], int(2024));
        assert_eq!(split[0][7], string_literal("Z"));
        let built = run(
            "date",
            &[None, Some(int(2024)), Some(int(2)), Some(int(29))],
        );
        assert_eq!(built[0][0], lit("2024-02-29".into(), "date"));
    }

    #[test]
    fn regex_translation() {
        assert!(xpath_regex("a+", "i").unwrap().is_match("AAA"));
        assert!(xpath_regex("a b", "x").unwrap().is_match("ab"));
        assert!(xpath_regex("a.b", "q").unwrap().is_match("a.b"));
        assert!(!xpath_regex("a.b", "q").unwrap().is_match("axb"));
        assert!(xpath_regex("[a-z-[aeiou]]", "").is_err());
        assert!(xpath_regex("a", "z").is_err());
        assert_eq!(xpath_replacement("$1-\\$").unwrap(), "${1}-$$");
        assert!(xpath_replacement("$x").is_none());
    }
}
