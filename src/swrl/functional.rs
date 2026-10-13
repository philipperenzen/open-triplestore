//! OWL 2 functional-syntax rules: `DLSafeRule( Body( … ) Head( … ) )`, as the
//! OWL API writes them (`.ofn`).
//!
//! `Prefix(ex:=<…>)` declarations are honoured (`rdf:`, `rdfs:`, `xsd:`,
//! `owl:`, `swrl:` and `swrlb:` are predeclared). Rules may stand inside an
//! `Ontology( … )` or on their own; every other axiom is skipped, and so is a
//! rule's `Annotation( … )`. Atoms: `ClassAtom`, `ObjectPropertyAtom` (with
//! `ObjectInverseOf`, arguments swapped), `DataPropertyAtom`, `BuiltInAtom`,
//! `SameIndividualAtom`, `DifferentIndividualsAtom`; arguments `Variable(…)`,
//! named individuals and literals (`"v"`, `"v"^^dt`, `"v"@lang`).
//! `ClassAtom` takes any OWL 2 class expression (`ObjectIntersectionOf`,
//! `ObjectUnionOf`, `ObjectComplementOf`, `ObjectOneOf`,
//! `ObjectSome/AllValuesFrom`, `ObjectHasValue`, `ObjectHasSelf`,
//! `ObjectMin/Max/ExactCardinality` and their `Data…` counterparts), and
//! `DataRangeAtom` any data range (a datatype, `DataIntersectionOf`,
//! `DataUnionOf`, `DataComplementOf`, `DataOneOf`, `DatatypeRestriction`). As
//! with the other readers, an unknown atom or expression, or an anonymous
//! individual, refuses the document with a message naming it.

use oxigraph::model::Literal;

use super::engine::{constant_term, Atom, SwrlArg, SwrlRule};
use super::expr::{CardKind, ClassExpr, DataRange, PropExpr};
use super::lexer::{Tok, Tokens};
use super::names::Names;

/// Parse the `DLSafeRule`s of an OWL 2 functional-syntax document.
pub fn parse_swrl_functional(text: &str) -> Result<Vec<SwrlRule>, String> {
    let mut p = Parser {
        toks: Tokens::new(text)?,
        names: Names::default(),
    };
    let mut rules = Vec::new();
    while let Some(tok) = p.toks.peek().cloned() {
        match tok {
            Tok::Name(n) if n == "Prefix" => p.prefix()?,
            Tok::Name(n) if n == "Ontology" => {
                p.toks.next();
                p.toks.expect(&Tok::LParen)?;
                // Ontology IRI and version IRI, when present.
                while matches!(p.toks.peek(), Some(Tok::Iri(_))) {
                    p.toks.next();
                }
                loop {
                    match p.toks.peek().cloned() {
                        Some(Tok::RParen) => {
                            p.toks.next();
                            break;
                        }
                        Some(Tok::Name(n)) if n == "DLSafeRule" => rules.push(p.rule()?),
                        Some(Tok::Name(_)) if p.toks.peek_at(1) == Some(&Tok::LParen) => {
                            p.toks.next();
                            p.toks.skip_group()?;
                        }
                        Some(t) => return Err(p.toks.error(format!("unexpected {t} in Ontology"))),
                        None => return Err(p.toks.error("Ontology( is not closed")),
                    }
                }
            }
            Tok::Name(n) if n == "DLSafeRule" => rules.push(p.rule()?),
            Tok::Name(_) if p.toks.peek_at(1) == Some(&Tok::LParen) => {
                p.toks.next();
                p.toks.skip_group()?;
            }
            t => return Err(p.toks.error(format!("unexpected {t}"))),
        }
    }
    Ok(rules)
}

struct Parser {
    toks: Tokens,
    names: Names,
}

impl Parser {
    /// `Prefix(ex:=<http://ex/>)` or `Prefix(:=<…>)`.
    fn prefix(&mut self) -> Result<(), String> {
        self.toks.next();
        self.toks.expect(&Tok::LParen)?;
        let prefix = match self.toks.next() {
            Some(Tok::Name(n)) if n.ends_with(':') => n.trim_end_matches(':').to_string(),
            other => {
                return Err(self.toks.error(format!(
                    "Prefix needs a prefix name ending in ':', found {}",
                    describe(other.as_ref())
                )))
            }
        };
        self.toks.expect(&Tok::Equals)?;
        let ns = match self.toks.next() {
            Some(Tok::Iri(i)) => i,
            other => {
                return Err(self.toks.error(format!(
                    "Prefix needs a namespace <IRI>, found {}",
                    describe(other.as_ref())
                )))
            }
        };
        self.toks.expect(&Tok::RParen)?;
        self.names.declare(&prefix, &ns);
        Ok(())
    }

    /// An IRI: `<full>` or `prefix:name`.
    fn iri(&mut self, what: &str) -> Result<String, String> {
        match self.toks.next() {
            Some(Tok::Iri(i)) => Ok(self.names.resolve(&i)),
            Some(Tok::Name(n)) if n.contains(':') => {
                self.names.expand(&n).map_err(|e| self.toks.error(e))
            }
            other => Err(self.toks.error(format!(
                "expected the IRI of {what}, found {}",
                describe(other.as_ref())
            ))),
        }
    }

    fn rule(&mut self) -> Result<SwrlRule, String> {
        self.toks.next();
        self.toks.expect(&Tok::LParen)?;
        while matches!(self.toks.peek(), Some(Tok::Name(n)) if n == "Annotation") {
            self.toks.next();
            self.toks.skip_group()?;
        }
        let body = self.atoms("Body")?;
        let head = self.atoms("Head")?;
        self.toks.expect(&Tok::RParen)?;
        if head.is_empty() {
            return Err(self.toks.error("DLSafeRule has an empty Head"));
        }
        Ok(SwrlRule {
            name: None,
            body,
            head,
        })
    }

    fn atoms(&mut self, which: &str) -> Result<Vec<Atom>, String> {
        match self.toks.next() {
            Some(Tok::Name(n)) if n == which => {}
            other => {
                return Err(self.toks.error(format!(
                    "expected {which}( in DLSafeRule, found {}",
                    describe(other.as_ref())
                )))
            }
        }
        self.toks.expect(&Tok::LParen)?;
        let mut atoms = Vec::new();
        loop {
            match self.toks.peek().cloned() {
                Some(Tok::RParen) => {
                    self.toks.next();
                    return Ok(atoms);
                }
                Some(Tok::Name(kind)) => {
                    self.toks.next();
                    atoms.push(self.atom(&kind)?);
                }
                Some(t) => return Err(self.toks.error(format!("expected an atom, found {t}"))),
                None => return Err(self.toks.error(format!("{which}( is not closed"))),
            }
        }
    }

    fn atom(&mut self, kind: &str) -> Result<Atom, String> {
        self.toks.expect(&Tok::LParen)?;
        let atom = match kind {
            "ClassAtom" => {
                let expr = self.class_expr()?;
                let arg = self.arg()?;
                match expr {
                    ClassExpr::Named(class_iri) => Atom::ClassAtom { class_iri, arg },
                    expr => Atom::ClassExpressionAtom { expr, arg },
                }
            }
            "ObjectPropertyAtom" => {
                let inverse =
                    matches!(self.toks.peek(), Some(Tok::Name(n)) if n == "ObjectInverseOf");
                let property = if inverse {
                    self.toks.next();
                    self.toks.expect(&Tok::LParen)?;
                    let p = self.iri("an object property")?;
                    self.toks.expect(&Tok::RParen)?;
                    p
                } else {
                    self.iri("an object property")?
                };
                let (a, b) = (self.arg()?, self.arg()?);
                let (arg1, arg2) = if inverse { (b, a) } else { (a, b) };
                Atom::ObjectPropertyAtom {
                    property,
                    arg1,
                    arg2,
                }
            }
            "DataPropertyAtom" => Atom::DataPropertyAtom {
                property: self.iri("a data property")?,
                arg1: self.arg()?,
                arg2: self.arg()?,
            },
            "SameIndividualAtom" => Atom::SameIndividualAtom {
                arg1: self.arg()?,
                arg2: self.arg()?,
            },
            "DifferentIndividualsAtom" => Atom::DifferentIndividualsAtom {
                arg1: self.arg()?,
                arg2: self.arg()?,
            },
            "BuiltInAtom" => {
                let builtin = self.iri("a built-in")?;
                let mut args = Vec::new();
                while self.toks.peek() != Some(&Tok::RParen) {
                    args.push(self.arg()?);
                }
                if args.is_empty() {
                    return Err(self.toks.error("BuiltInAtom needs at least one argument"));
                }
                Atom::BuiltinAtom { builtin, args }
            }
            "DataRangeAtom" => {
                let range = self.data_range()?;
                Atom::DataRangeAtom {
                    range,
                    arg: self.arg()?,
                }
            }
            other => {
                return Err(self.toks.error(format!(
                    "Unknown SWRL atom {other}(; refusing the document rather than running \
                     its rule without that atom"
                )))
            }
        };
        self.toks.expect(&Tok::RParen).map_err(|_| {
            self.toks
                .error(format!("{kind} has more arguments than it takes"))
        })?;
        Ok(atom)
    }

    /// Whether the next tokens open a constructor `Name(`.
    fn at_constructor(&self) -> Option<String> {
        match (self.toks.peek(), self.toks.peek_at(1)) {
            (Some(Tok::Name(n)), Some(Tok::LParen)) if !n.contains(':') => Some(n.clone()),
            _ => None,
        }
    }

    /// An object property expression: a property or `ObjectInverseOf(p)`.
    fn prop_expr(&mut self) -> Result<PropExpr, String> {
        if self.at_constructor().as_deref() == Some("ObjectInverseOf") {
            self.toks.next();
            self.toks.expect(&Tok::LParen)?;
            let p = self.iri("an object property")?;
            self.toks.expect(&Tok::RParen)?;
            return Ok(PropExpr::Inverse(p));
        }
        Ok(PropExpr::Named(self.iri("an object property")?))
    }

    fn cardinality(&mut self) -> Result<u64, String> {
        match self.toks.next() {
            Some(Tok::Number(n)) => n
                .parse::<u64>()
                .map_err(|_| self.toks.error(format!("'{n}' is not a cardinality"))),
            other => Err(self.toks.error(format!(
                "expected a cardinality, found {}",
                describe(other.as_ref())
            ))),
        }
    }

    fn individual(&mut self) -> Result<String, String> {
        match self.arg()? {
            SwrlArg::Individual(i) => Ok(i),
            _ => Err(self.toks.error("expected a named individual")),
        }
    }

    fn literal(&mut self) -> Result<Literal, String> {
        match self.arg()? {
            arg @ SwrlArg::Literal { .. } => match constant_term(&arg)? {
                oxigraph::model::Term::Literal(l) => Ok(l),
                _ => unreachable!("a literal argument is a literal"),
            },
            _ => Err(self.toks.error("expected a literal")),
        }
    }

    /// Expressions up to the closing parenthesis.
    fn until_close<T>(
        &mut self,
        mut item: impl FnMut(&mut Self) -> Result<T, String>,
    ) -> Result<Vec<T>, String> {
        let mut out = Vec::new();
        while self.toks.peek() != Some(&Tok::RParen) {
            if self.toks.peek().is_none() {
                return Err(self.toks.error("an expression is not closed"));
            }
            out.push(item(self)?);
        }
        Ok(out)
    }

    /// An OWL 2 class expression.
    fn class_expr(&mut self) -> Result<ClassExpr, String> {
        let Some(kind) = self.at_constructor() else {
            return Ok(ClassExpr::Named(self.iri("a class")?));
        };
        self.toks.next();
        self.toks.expect(&Tok::LParen)?;
        let card = |k: &str| match k {
            "Min" => CardKind::Min,
            "Max" => CardKind::Max,
            _ => CardKind::Exact,
        };
        let expr = match kind.as_str() {
            "ObjectIntersectionOf" => {
                ClassExpr::IntersectionOf(self.until_close(Self::class_expr)?)
            }
            "ObjectUnionOf" => ClassExpr::UnionOf(self.until_close(Self::class_expr)?),
            "ObjectComplementOf" => ClassExpr::ComplementOf(Box::new(self.class_expr()?)),
            "ObjectOneOf" => ClassExpr::OneOf(self.until_close(Self::individual)?),
            "ObjectSomeValuesFrom" => {
                let p = self.prop_expr()?;
                ClassExpr::SomeValuesFrom(p, Box::new(self.class_expr()?))
            }
            "ObjectAllValuesFrom" => {
                let p = self.prop_expr()?;
                ClassExpr::AllValuesFrom(p, Box::new(self.class_expr()?))
            }
            "ObjectHasValue" => {
                let p = self.prop_expr()?;
                ClassExpr::HasValue(p, self.individual()?)
            }
            "ObjectHasSelf" => ClassExpr::HasSelf(self.prop_expr()?),
            k @ ("ObjectMinCardinality" | "ObjectMaxCardinality" | "ObjectExactCardinality") => {
                let n = self.cardinality()?;
                let prop = self.prop_expr()?;
                let filler = if self.toks.peek() == Some(&Tok::RParen) {
                    None
                } else {
                    Some(Box::new(self.class_expr()?))
                };
                ClassExpr::Cardinality {
                    kind: card(&k["Object".len()..k.len() - "Cardinality".len()]),
                    n,
                    prop,
                    filler,
                }
            }
            "DataSomeValuesFrom" | "DataAllValuesFrom" => {
                let p = self.iri("a data property")?;
                if self.at_constructor().is_none()
                    && !matches!(self.toks.peek_at(1), Some(Tok::RParen))
                {
                    return Err(self.toks.error(format!(
                        "{kind} over several data properties is not supported"
                    )));
                }
                let r = self.data_range()?;
                if kind == "DataSomeValuesFrom" {
                    ClassExpr::DataSomeValuesFrom(p, r)
                } else {
                    ClassExpr::DataAllValuesFrom(p, r)
                }
            }
            "DataHasValue" => {
                let p = self.iri("a data property")?;
                ClassExpr::DataHasValue(p, self.literal()?)
            }
            k @ ("DataMinCardinality" | "DataMaxCardinality" | "DataExactCardinality") => {
                let n = self.cardinality()?;
                let prop = self.iri("a data property")?;
                let range = if self.toks.peek() == Some(&Tok::RParen) {
                    None
                } else {
                    Some(self.data_range()?)
                };
                ClassExpr::DataCardinality {
                    kind: card(&k["Data".len()..k.len() - "Cardinality".len()]),
                    n,
                    prop,
                    range,
                }
            }
            other => {
                return Err(self
                    .toks
                    .error(format!("Unknown class expression {other}( in a ClassAtom")))
            }
        };
        self.toks.expect(&Tok::RParen).map_err(|_| {
            self.toks
                .error(format!("{kind} has more operands than it takes"))
        })?;
        Ok(expr)
    }

    /// An OWL 2 data range.
    fn data_range(&mut self) -> Result<DataRange, String> {
        let Some(kind) = self.at_constructor() else {
            return Ok(DataRange::Datatype(self.iri("a datatype")?));
        };
        self.toks.next();
        self.toks.expect(&Tok::LParen)?;
        let range = match kind.as_str() {
            "DataIntersectionOf" => DataRange::IntersectionOf(self.until_close(Self::data_range)?),
            "DataUnionOf" => DataRange::UnionOf(self.until_close(Self::data_range)?),
            "DataComplementOf" => DataRange::ComplementOf(Box::new(self.data_range()?)),
            "DataOneOf" => DataRange::OneOf(self.until_close(Self::literal)?),
            "DatatypeRestriction" => {
                let dt = self.iri("a datatype")?;
                let facets = self.until_close(|p| {
                    let f = p.iri("a facet")?;
                    Ok((f, p.literal()?))
                })?;
                if facets.is_empty() {
                    return Err(self.toks.error("DatatypeRestriction without a facet"));
                }
                DataRange::Restriction(dt, facets)
            }
            other => {
                return Err(self
                    .toks
                    .error(format!("Unknown data range {other}( in a DataRangeAtom")))
            }
        };
        self.toks.expect(&Tok::RParen).map_err(|_| {
            self.toks
                .error(format!("{kind} has more operands than it takes"))
        })?;
        Ok(range)
    }

    /// `Variable(<iri>)`, a named individual, or a literal.
    fn arg(&mut self) -> Result<SwrlArg, String> {
        match self.toks.peek().cloned() {
            Some(Tok::Name(n)) if n == "Variable" => {
                self.toks.next();
                self.toks.expect(&Tok::LParen)?;
                // A variable is an identity: an undeclared prefix still names one.
                let name = match self.toks.next() {
                    Some(Tok::Iri(i)) => self.names.resolve(&i),
                    Some(Tok::Name(n)) => self.names.expand(&n).unwrap_or(n),
                    other => {
                        return Err(self.toks.error(format!(
                            "Variable needs an IRI, found {}",
                            describe(other.as_ref())
                        )))
                    }
                };
                self.toks.expect(&Tok::RParen)?;
                Ok(SwrlArg::Variable(name))
            }
            Some(Tok::Str(value)) => {
                self.toks.next();
                Ok(match self.toks.peek().cloned() {
                    Some(Tok::DoubleCaret) => {
                        self.toks.next();
                        SwrlArg::Literal {
                            value,
                            datatype: Some(self.iri("a datatype")?),
                            language: None,
                        }
                    }
                    Some(Tok::Lang(lang)) => {
                        self.toks.next();
                        SwrlArg::Literal {
                            value,
                            datatype: None,
                            language: Some(lang),
                        }
                    }
                    _ => SwrlArg::Literal {
                        value,
                        datatype: None,
                        language: None,
                    },
                })
            }
            Some(Tok::BlankNode(b)) => Err(self.toks.error(format!(
                "{b} is an anonymous individual, which rules do not support"
            ))),
            Some(Tok::Iri(_)) | Some(Tok::Name(_)) => {
                Ok(SwrlArg::Individual(self.iri("an individual")?))
            }
            other => Err(self.toks.error(format!(
                "expected a Variable, individual or literal, found {}",
                describe(other.as_ref())
            ))),
        }
    }
}

fn describe(tok: Option<&Tok>) -> String {
    tok.map(|t| t.to_string())
        .unwrap_or_else(|| "the end of the input".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = r#"
Prefix(:=<http://ex/>)
Prefix(ex:=<http://ex/>)
Ontology(<http://ex/onto>
  Declaration(Class(:Person))
  SubClassOf(:Adult :Person)
  DLSafeRule(
    Annotation(rdfs:label "adults")
    Body(
      ClassAtom(:Person Variable(<urn:swrl#x>))
      DataPropertyAtom(:age Variable(<urn:swrl#x>) Variable(<urn:swrl#a>))
      BuiltInAtom(swrlb:greaterThan Variable(<urn:swrl#a>) "17"^^xsd:integer)
      ObjectPropertyAtom(ObjectInverseOf(ex:parentOf) Variable(<urn:swrl#x>) ex:mum)
    )
    Head(ClassAtom(:Adult Variable(<urn:swrl#x>)))
  )
)"#;

    #[test]
    fn reads_a_rule_inside_an_ontology() {
        let rules = parse_swrl_functional(DOC).unwrap();
        assert_eq!(rules.len(), 1);
        let r = &rules[0];
        assert_eq!(r.body.len(), 4);
        assert!(
            matches!(&r.body[0], Atom::ClassAtom { class_iri, .. } if class_iri == "http://ex/Person")
        );
        assert!(matches!(&r.body[2], Atom::BuiltinAtom { builtin, args }
            if builtin == "http://www.w3.org/2003/11/swrlb#greaterThan"
            && matches!(&args[1], SwrlArg::Literal { datatype: Some(d), .. }
                if d == "http://www.w3.org/2001/XMLSchema#integer")));
        // ObjectInverseOf(parentOf)(?x, mum) is parentOf(mum, ?x).
        assert!(
            matches!(&r.body[3], Atom::ObjectPropertyAtom { arg1: SwrlArg::Individual(m), .. }
            if m == "http://ex/mum")
        );
    }

    #[test]
    fn reads_class_expressions_and_data_ranges() {
        let doc = r#"Prefix(:=<http://ex/>)
DLSafeRule(Body(
    ClassAtom(ObjectIntersectionOf(:A ObjectSomeValuesFrom(ObjectInverseOf(:p) :B)) Variable(<urn:v#x>))
    DataPropertyAtom(:age Variable(<urn:v#x>) Variable(<urn:v#a>))
    DataRangeAtom(DatatypeRestriction(xsd:integer xsd:minInclusive "18"^^xsd:integer) Variable(<urn:v#a>))
    ClassAtom(ObjectMinCardinality(2 :q) Variable(<urn:v#x>))
  ) Head(ClassAtom(:Adult Variable(<urn:v#x>))))"#;
        let rules = parse_swrl_functional(doc).unwrap();
        let body = &rules[0].body;
        assert!(
            matches!(&body[0], Atom::ClassExpressionAtom { expr: ClassExpr::IntersectionOf(xs), .. }
            if matches!(&xs[1], ClassExpr::SomeValuesFrom(PropExpr::Inverse(p), _) if p == "http://ex/p"))
        );
        assert!(
            matches!(&body[2], Atom::DataRangeAtom { range: DataRange::Restriction(_, f), .. } if f.len() == 1)
        );
        assert!(matches!(
            &body[3],
            Atom::ClassExpressionAtom {
                expr: ClassExpr::Cardinality {
                    kind: CardKind::Min,
                    n: 2,
                    filler: None,
                    ..
                },
                ..
            }
        ));
    }

    #[test]
    fn refuses_what_it_cannot_read() {
        let head = "Head(ClassAtom(<http://ex/B> Variable(<urn:v#x>)))";
        let cases = [
            (
                format!("DLSafeRule(Body(ClassAtom(ObjectFancyOf(<http://ex/A>) Variable(<urn:v#x>))) {head})"),
                "Unknown class expression",
            ),
            (
                format!("DLSafeRule(Body(DataRangeAtom(DataFancyOf(xsd:integer) Variable(<urn:v#x>))) {head})"),
                "Unknown data range",
            ),
            (
                format!("DLSafeRule(Body(FancyAtom(Variable(<urn:v#x>))) {head})"),
                "Unknown SWRL atom",
            ),
            (
                format!("DLSafeRule(Body(ClassAtom(<http://ex/A> Variable(<urn:v#x>) Variable(<urn:v#y>))) {head})"),
                "more arguments",
            ),
            (
                format!("DLSafeRule(Body(ClassAtom(ex:A Variable(<urn:v#x>))) {head})"),
                "Unknown prefix",
            ),
            (
                format!("DLSafeRule(Body(ClassAtom(<http://ex/A> _:b1)) {head})"),
                "anonymous individual",
            ),
        ];
        for (doc, wanted) in cases {
            let err = parse_swrl_functional(&doc).unwrap_err();
            assert!(err.contains(wanted), "expected '{wanted}' in: {err}");
        }
    }
}
