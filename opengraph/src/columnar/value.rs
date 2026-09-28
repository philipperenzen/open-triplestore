//! SPARQL values and expression evaluation for the columnar evaluator: the
//! value semantics of SPARQL 1.1 §17 on decoded terms, with the same xsd
//! datatype implementations oxigraph uses (`oxsdatatypes`), so numeric,
//! boolean and dateTime comparisons and arithmetic agree with the engine.
//! Anything outside the implemented subset evaluates to `Err(Decline)` and
//! the whole query is declined to the engine — never a different answer.

use std::cmp::Ordering;

use oxrdf::vocab::{rdf, xsd};
use oxrdf::{Literal, NamedNode, NamedNodeRef, Term};
use oxsdatatypes::{Boolean, Date, DateTime, Decimal, Double, Float, Integer, Time};

/// The evaluator cannot represent this: decline the query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decline(pub &'static str);

/// A SPARQL evaluation error (unbound, type error): the expression has no
/// value; a `FILTER` drops the row, an `ORDER BY` key sorts first, a
/// `BIND` leaves the variable unbound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EvalError;

#[derive(Debug, Clone, PartialEq)]
pub enum Numeric {
    Int(Integer),
    Dec(Decimal),
    Flt(Float),
    Dbl(Double),
}

impl Numeric {
    fn as_double(&self) -> Double {
        match self {
            Numeric::Int(i) => Double::from(*i),
            Numeric::Dec(d) => Double::from(*d),
            Numeric::Flt(f) => Double::from(*f),
            Numeric::Dbl(d) => *d,
        }
    }

    fn as_float(&self) -> Float {
        match self {
            Numeric::Int(i) => Float::from(*i),
            Numeric::Dec(d) => Float::from(*d),
            Numeric::Flt(f) => *f,
            Numeric::Dbl(d) => Float::from(*d),
        }
    }

    fn as_decimal(&self) -> Option<Decimal> {
        match self {
            Numeric::Int(i) => Some(Decimal::from(*i)),
            Numeric::Dec(d) => Some(*d),
            _ => None,
        }
    }

    /// SPARQL 17.4.1 comparison with type promotion.
    pub fn compare(&self, other: &Numeric) -> Option<Ordering> {
        match (self, other) {
            (Numeric::Int(a), Numeric::Int(b)) => a.partial_cmp(b),
            (Numeric::Dbl(_), _) | (_, Numeric::Dbl(_)) => {
                self.as_double().partial_cmp(&other.as_double())
            }
            (Numeric::Flt(_), _) | (_, Numeric::Flt(_)) => {
                self.as_float().partial_cmp(&other.as_float())
            }
            _ => self.as_decimal()?.partial_cmp(&other.as_decimal()?),
        }
    }

    fn arith(
        &self,
        other: &Numeric,
        int: impl Fn(Integer, Integer) -> Option<Integer>,
        dec: impl Fn(Decimal, Decimal) -> Option<Decimal>,
        flt: impl Fn(Float, Float) -> Float,
        dbl: impl Fn(Double, Double) -> Double,
    ) -> Result<Numeric, EvalError> {
        match (self, other) {
            (Numeric::Int(a), Numeric::Int(b)) => int(*a, *b).map(Numeric::Int).ok_or(EvalError),
            (Numeric::Dbl(_), _) | (_, Numeric::Dbl(_)) => {
                Ok(Numeric::Dbl(dbl(self.as_double(), other.as_double())))
            }
            (Numeric::Flt(_), _) | (_, Numeric::Flt(_)) => {
                Ok(Numeric::Flt(flt(self.as_float(), other.as_float())))
            }
            _ => dec(
                self.as_decimal().ok_or(EvalError)?,
                other.as_decimal().ok_or(EvalError)?,
            )
            .map(Numeric::Dec)
            .ok_or(EvalError),
        }
    }

    pub fn add(&self, o: &Numeric) -> Result<Numeric, EvalError> {
        self.arith(
            o,
            |a, b| a.checked_add(b),
            |a, b| a.checked_add(b),
            |a, b| a + b,
            |a, b| a + b,
        )
    }

    pub fn sub(&self, o: &Numeric) -> Result<Numeric, EvalError> {
        self.arith(
            o,
            |a, b| a.checked_sub(b),
            |a, b| a.checked_sub(b),
            |a, b| a - b,
            |a, b| a - b,
        )
    }

    pub fn mul(&self, o: &Numeric) -> Result<Numeric, EvalError> {
        self.arith(
            o,
            |a, b| a.checked_mul(b),
            |a, b| a.checked_mul(b),
            |a, b| a * b,
            |a, b| a * b,
        )
    }

    /// SPARQL: integer / integer is a decimal division.
    pub fn div(&self, o: &Numeric) -> Result<Numeric, EvalError> {
        match (self, o) {
            (Numeric::Int(a), Numeric::Int(b)) => Decimal::from(*a)
                .checked_div(Decimal::from(*b))
                .map(Numeric::Dec)
                .ok_or(EvalError),
            _ => self.arith(
                o,
                |_, _| None,
                |a, b| a.checked_div(b),
                |a, b| a / b,
                |a, b| a / b,
            ),
        }
    }

    pub fn neg(&self) -> Result<Numeric, EvalError> {
        Ok(match self {
            Numeric::Int(i) => Numeric::Int(i.checked_neg().ok_or(EvalError)?),
            Numeric::Dec(d) => Numeric::Dec(d.checked_neg().ok_or(EvalError)?),
            Numeric::Flt(f) => Numeric::Flt(-*f),
            Numeric::Dbl(d) => Numeric::Dbl(-*d),
        })
    }

    pub fn abs(&self) -> Result<Numeric, EvalError> {
        Ok(match self {
            Numeric::Int(i) => Numeric::Int(i.checked_abs().ok_or(EvalError)?),
            Numeric::Dec(d) => Numeric::Dec(d.checked_abs().ok_or(EvalError)?),
            Numeric::Flt(f) => Numeric::Flt(f.abs()),
            Numeric::Dbl(d) => Numeric::Dbl(d.abs()),
        })
    }

    pub fn ceil(&self) -> Result<Numeric, EvalError> {
        Ok(match self {
            Numeric::Int(i) => Numeric::Int(*i),
            Numeric::Dec(d) => Numeric::Dec(d.checked_ceil().ok_or(EvalError)?),
            Numeric::Flt(f) => Numeric::Flt(f.ceil()),
            Numeric::Dbl(d) => Numeric::Dbl(d.ceil()),
        })
    }

    pub fn floor(&self) -> Result<Numeric, EvalError> {
        Ok(match self {
            Numeric::Int(i) => Numeric::Int(*i),
            Numeric::Dec(d) => Numeric::Dec(d.checked_floor().ok_or(EvalError)?),
            Numeric::Flt(f) => Numeric::Flt(f.floor()),
            Numeric::Dbl(d) => Numeric::Dbl(d.floor()),
        })
    }

    pub fn round(&self) -> Result<Numeric, EvalError> {
        Ok(match self {
            Numeric::Int(i) => Numeric::Int(*i),
            Numeric::Dec(d) => Numeric::Dec(d.checked_round().ok_or(EvalError)?),
            Numeric::Flt(f) => Numeric::Flt(f.round()),
            Numeric::Dbl(d) => Numeric::Dbl(d.round()),
        })
    }

    pub fn to_term(&self) -> Term {
        match self {
            Numeric::Int(i) => Literal::new_typed_literal(i.to_string(), xsd::INTEGER).into(),
            Numeric::Dec(d) => Literal::new_typed_literal(d.to_string(), xsd::DECIMAL).into(),
            Numeric::Flt(f) => Literal::new_typed_literal(f.to_string(), xsd::FLOAT).into(),
            Numeric::Dbl(d) => Literal::new_typed_literal(d.to_string(), xsd::DOUBLE).into(),
        }
    }

    pub fn is_int(&self) -> bool {
        matches!(self, Numeric::Int(_))
    }
}

/// A decoded term, typed for evaluation.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Iri(NamedNode),
    Blank(String),
    /// A simple literal or `xsd:string`.
    Str(String),
    Lang(String, String),
    Num(Numeric),
    Bool(bool),
    DateTime(DateTime),
    Date(Date),
    Time(Time),
    /// Any other typed literal: comparable only by `sameTerm` / equality.
    Typed(String, NamedNode),
}

impl Value {
    /// Decode a term. Literals whose lexical form does not fit their
    /// datatype stay `Typed` (an ill-typed literal compares by identity).
    pub fn from_term(term: &Term) -> Result<Self, Decline> {
        Ok(match term {
            Term::NamedNode(n) => Value::Iri(n.clone()),
            Term::BlankNode(b) => Value::Blank(b.as_str().to_string()),
            Term::Literal(l) => {
                if let Some(lang) = l.language() {
                    return Ok(Value::Lang(l.value().to_string(), lang.to_string()));
                }
                let dt = l.datatype();
                let v = l.value();
                if dt == xsd::STRING {
                    Value::Str(v.to_string())
                } else if dt == xsd::INTEGER
                    || dt == xsd::LONG
                    || dt == xsd::INT
                    || dt == xsd::SHORT
                    || dt == xsd::BYTE
                    || dt == xsd::NON_NEGATIVE_INTEGER
                    || dt == xsd::POSITIVE_INTEGER
                    || dt == xsd::NON_POSITIVE_INTEGER
                    || dt == xsd::NEGATIVE_INTEGER
                    || dt == xsd::UNSIGNED_LONG
                    || dt == xsd::UNSIGNED_INT
                    || dt == xsd::UNSIGNED_SHORT
                    || dt == xsd::UNSIGNED_BYTE
                {
                    match v.parse::<Integer>() {
                        Ok(i) => Value::Num(Numeric::Int(i)),
                        Err(_) => Value::Typed(v.to_string(), dt.into_owned()),
                    }
                } else if dt == xsd::DECIMAL {
                    match v.parse::<Decimal>() {
                        Ok(d) => Value::Num(Numeric::Dec(d)),
                        Err(_) => Value::Typed(v.to_string(), dt.into_owned()),
                    }
                } else if dt == xsd::DOUBLE {
                    match v.parse::<Double>() {
                        Ok(d) => Value::Num(Numeric::Dbl(d)),
                        Err(_) => Value::Typed(v.to_string(), dt.into_owned()),
                    }
                } else if dt == xsd::FLOAT {
                    match v.parse::<Float>() {
                        Ok(f) => Value::Num(Numeric::Flt(f)),
                        Err(_) => Value::Typed(v.to_string(), dt.into_owned()),
                    }
                } else if dt == xsd::BOOLEAN {
                    match v.parse::<Boolean>() {
                        Ok(b) => Value::Bool(b.into()),
                        Err(_) => Value::Typed(v.to_string(), dt.into_owned()),
                    }
                } else if dt == xsd::DATE_TIME {
                    match v.parse::<DateTime>() {
                        Ok(d) => Value::DateTime(d),
                        Err(_) => Value::Typed(v.to_string(), dt.into_owned()),
                    }
                } else if dt == xsd::DATE {
                    match v.parse::<Date>() {
                        Ok(d) => Value::Date(d),
                        Err(_) => Value::Typed(v.to_string(), dt.into_owned()),
                    }
                } else if dt == xsd::TIME {
                    match v.parse::<Time>() {
                        Ok(t) => Value::Time(t),
                        Err(_) => Value::Typed(v.to_string(), dt.into_owned()),
                    }
                } else {
                    // Every other datatype (a tagless rdf:langString cannot
                    // occur in a parsed term) is opaque here.
                    Value::Typed(v.to_string(), dt.into_owned())
                }
            }
            #[allow(unreachable_patterns)]
            _ => return Err(Decline("quoted triples")),
        })
    }

    pub fn to_term(&self) -> Term {
        match self {
            Value::Iri(n) => Term::NamedNode(n.clone()),
            Value::Blank(b) => Term::BlankNode(oxrdf::BlankNode::new_unchecked(b.clone())),
            Value::Str(s) => Literal::new_simple_literal(s.clone()).into(),
            Value::Lang(s, l) => {
                Literal::new_language_tagged_literal_unchecked(s.clone(), l.clone()).into()
            }
            Value::Num(n) => n.to_term(),
            Value::Bool(b) => Literal::from(*b).into(),
            Value::DateTime(d) => Literal::new_typed_literal(d.to_string(), xsd::DATE_TIME).into(),
            Value::Date(d) => Literal::new_typed_literal(d.to_string(), xsd::DATE).into(),
            Value::Time(t) => Literal::new_typed_literal(t.to_string(), xsd::TIME).into(),
            Value::Typed(s, dt) => Literal::new_typed_literal(s.clone(), dt.clone()).into(),
        }
    }

    pub fn is_literal(&self) -> bool {
        !matches!(self, Value::Iri(_) | Value::Blank(_))
    }

    /// Effective boolean value (SPARQL 17.2.2).
    pub fn ebv(&self) -> Result<bool, EvalError> {
        match self {
            Value::Bool(b) => Ok(*b),
            Value::Str(s) => Ok(!s.is_empty()),
            // A language-tagged literal has no effective boolean value: only
            // xsd:string and the plain literal do (SPARQL 17.2.2).
            Value::Num(n) => Ok(match n {
                Numeric::Int(i) => *i != Integer::from(0),
                Numeric::Dec(d) => *d != Decimal::from(0),
                Numeric::Flt(f) => !(f.is_nan() || *f == Float::from(0.0)),
                Numeric::Dbl(d) => !(d.is_nan() || *d == Double::from(0.0)),
            }),
            // An ill-typed literal, a language tag, an IRI, a blank node
            // and every opaque datatype are type errors.
            _ => Err(EvalError),
        }
    }

    /// The `str()` of a value.
    pub fn str(&self) -> String {
        match self {
            Value::Iri(n) => n.as_str().to_string(),
            Value::Blank(b) => b.clone(),
            Value::Str(s) | Value::Lang(s, _) | Value::Typed(s, _) => s.clone(),
            Value::Num(n) => match n.to_term() {
                Term::Literal(l) => l.value().to_string(),
                _ => unreachable!(),
            },
            Value::Bool(b) => b.to_string(),
            Value::DateTime(d) => d.to_string(),
            Value::Date(d) => d.to_string(),
            Value::Time(t) => t.to_string(),
        }
    }

    pub fn datatype(&self) -> Option<NamedNode> {
        Some(match self {
            Value::Str(_) => xsd::STRING.into_owned(),
            Value::Lang(_, _) => rdf::LANG_STRING.into_owned(),
            Value::Num(n) => match n {
                Numeric::Int(_) => xsd::INTEGER.into_owned(),
                Numeric::Dec(_) => xsd::DECIMAL.into_owned(),
                Numeric::Flt(_) => xsd::FLOAT.into_owned(),
                Numeric::Dbl(_) => xsd::DOUBLE.into_owned(),
            },
            Value::Bool(_) => xsd::BOOLEAN.into_owned(),
            Value::DateTime(_) => xsd::DATE_TIME.into_owned(),
            Value::Date(_) => xsd::DATE.into_owned(),
            Value::Time(_) => xsd::TIME.into_owned(),
            Value::Typed(_, dt) => dt.clone(),
            Value::Iri(_) | Value::Blank(_) => return None,
        })
    }

    /// `=` per SPARQL 17.4.1.7: value equality for comparable types, term
    /// equality otherwise; an error when the types cannot be compared.
    pub fn equals(&self, other: &Value) -> Result<bool, EvalError> {
        // Identical terms are equal whatever their datatype — including the
        // ill-typed and the opaque, where no value comparison is possible.
        if self.same_term(other) {
            return Ok(true);
        }
        // A literal against an IRI or a blank node is simply unequal, whatever
        // the literal is — an ill-typed one included.
        if !self.is_literal() || !other.is_literal() {
            return Ok(false);
        }
        // A language-tagged literal can only be equal to another one: against
        // any typed literal it is simply unequal, an ill-typed one included.
        if matches!(self, Value::Lang(_, _)) != matches!(other, Value::Lang(_, _)) {
            return Ok(false);
        }
        // Between two typed literals, an ill-typed one poisons the comparison.
        if self.is_ill_typed() || other.is_ill_typed() {
            return Err(EvalError);
        }
        match (self, other) {
            (Value::Num(a), Value::Num(b)) => Ok(a.compare(b) == Some(Ordering::Equal)),
            (Value::Str(a), Value::Str(b)) => Ok(a == b),
            (Value::Lang(a, la), Value::Lang(b, lb)) => Ok(a == b && la.eq_ignore_ascii_case(lb)),
            (Value::Bool(a), Value::Bool(b)) => Ok(a == b),
            (Value::DateTime(a), Value::DateTime(b)) => a
                .partial_cmp(b)
                .map(|o| o == Ordering::Equal)
                .ok_or(EvalError),
            (Value::Date(a), Value::Date(b)) => a
                .partial_cmp(b)
                .map(|o| o == Ordering::Equal)
                .ok_or(EvalError),
            (Value::Time(a), Value::Time(b)) => a
                .partial_cmp(b)
                .map(|o| o == Ordering::Equal)
                .ok_or(EvalError),
            (Value::Iri(a), Value::Iri(b)) => Ok(a == b),
            (Value::Blank(a), Value::Blank(b)) => Ok(a == b),
            (Value::Typed(a, da), Value::Typed(b, db)) => {
                if da == db && a == b {
                    Ok(true)
                } else {
                    // Same unknown datatype, different lexical forms: unknown.
                    Err(EvalError)
                }
            }
            // Two literals this module holds as values whose kinds differ — a
            // number against a string, a date against a boolean — are unequal
            // rather than incomparable. Only an opaque datatype is an error.
            (a, b) if a.is_valued_literal() && b.is_valued_literal() => Ok(false),
            _ => Err(EvalError),
        }
    }

    /// `<` and friends per 17.4.1: numeric, string, boolean, dateTime;
    /// anything else is a type error.
    pub fn compare(&self, other: &Value) -> Result<Ordering, EvalError> {
        // The same term is never less or greater than itself, whatever it is:
        // this is what makes `?v <= ?v` hold for every term, as in the engine.
        if self.same_term(other) {
            return Ok(Ordering::Equal);
        }
        match (self, other) {
            (Value::Num(a), Value::Num(b)) => a.compare(b).ok_or(EvalError),
            (Value::Str(a), Value::Str(b)) => Ok(a.cmp(b)),
            // Two literals in the same language compare by lexical form.
            (Value::Lang(a, la), Value::Lang(b, lb)) if la.eq_ignore_ascii_case(lb) => Ok(a.cmp(b)),
            (Value::Bool(a), Value::Bool(b)) => Ok(a.cmp(b)),
            (Value::DateTime(a), Value::DateTime(b)) => a.partial_cmp(b).ok_or(EvalError),
            (Value::Date(a), Value::Date(b)) => a.partial_cmp(b).ok_or(EvalError),
            (Value::Time(a), Value::Time(b)) => a.partial_cmp(b).ok_or(EvalError),
            _ => Err(EvalError),
        }
    }

    /// A literal of a datatype we decode by value whose lexical form did
    /// not parse. The engine treats these as type errors everywhere.
    fn is_ill_typed(&self) -> bool {
        matches!(self, Value::Typed(_, dt) if is_known_datatype(dt.as_ref()))
    }

    /// A literal this module holds as a decoded value, rather than as an
    /// opaque lexical form with a datatype.
    fn is_valued_literal(&self) -> bool {
        matches!(
            self,
            Value::Num(_)
                | Value::Str(_)
                | Value::Lang(_, _)
                | Value::Bool(_)
                | Value::DateTime(_)
                | Value::Date(_)
                | Value::Time(_)
        )
    }

    /// `sameTerm`.
    pub fn same_term(&self, other: &Value) -> bool {
        self.to_term() == other.to_term()
    }
}

/// A datatype this module decodes by value. A literal carrying one of
/// these whose lexical form does not parse is *ill-typed*: it reaches
/// [`Value::Typed`], which is otherwise only used for datatypes we treat as
/// opaque, and every operation on it must be a type error.
fn is_known_datatype(dt: NamedNodeRef<'_>) -> bool {
    is_numeric_datatype(dt)
        || dt == xsd::BOOLEAN
        || dt == xsd::DATE_TIME
        || dt == xsd::DATE
        || dt == xsd::TIME
}

fn is_numeric_datatype(dt: NamedNodeRef<'_>) -> bool {
    dt == xsd::INTEGER
        || dt == xsd::DECIMAL
        || dt == xsd::DOUBLE
        || dt == xsd::FLOAT
        || dt == xsd::LONG
        || dt == xsd::INT
        || dt == xsd::SHORT
        || dt == xsd::BYTE
        || dt == xsd::NON_NEGATIVE_INTEGER
        || dt == xsd::POSITIVE_INTEGER
        || dt == xsd::NON_POSITIVE_INTEGER
        || dt == xsd::NEGATIVE_INTEGER
        || dt == xsd::UNSIGNED_LONG
        || dt == xsd::UNSIGNED_INT
        || dt == xsd::UNSIGNED_SHORT
        || dt == xsd::UNSIGNED_BYTE
}

/// Total order for `ORDER BY` (SPARQL 15.1): unbound, blank nodes, IRIs,
/// literals; literals by value when comparable, else by type then lexical
/// form, so the order is deterministic. Ties keep the input order.
pub fn order_cmp(a: Option<&Value>, b: Option<&Value>) -> Ordering {
    fn class(v: &Value) -> u8 {
        match v {
            Value::Blank(_) => 1,
            Value::Iri(_) => 2,
            _ => 3,
        }
    }
    match (a, b) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Less,
        (Some(_), None) => Ordering::Greater,
        (Some(a), Some(b)) => {
            let (ca, cb) = (class(a), class(b));
            if ca != cb {
                return ca.cmp(&cb);
            }
            match (a, b) {
                (Value::Blank(x), Value::Blank(y)) => x.cmp(y),
                (Value::Iri(x), Value::Iri(y)) => x.as_str().cmp(y.as_str()),
                _ => match a.compare(b) {
                    Ok(o) => o,
                    Err(_) => {
                        // Incomparable literals: by lexical form, then datatype
                        // IRI, then language — the engine's order.
                        let da = a
                            .datatype()
                            .map(|d| d.as_str().to_string())
                            .unwrap_or_default();
                        let db = b
                            .datatype()
                            .map(|d| d.as_str().to_string())
                            .unwrap_or_default();
                        a.str()
                            .cmp(&b.str())
                            .then_with(|| da.cmp(&db))
                            .then_with(|| match (a, b) {
                                (Value::Lang(_, la), Value::Lang(_, lb)) => la.cmp(lb),
                                _ => Ordering::Equal,
                            })
                    }
                },
            }
        }
    }
}
