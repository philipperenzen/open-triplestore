//! ShExR — a schema as RDF in the ShEx vocabulary (`http://www.w3.org/ns/shex#`),
//! the form a schema takes when it is stored in a named graph. This is what
//! a store-resolved `IMPORT <g>` reads.
//!
//! Accepts both the ShEx 2.1 layout (shapes listed directly) and the
//! `sx:ShapeDecl` layout the shexTest suite uses. A triple expression node
//! met a second time is an inclusion of the first (ShExR does not
//! distinguish `$label` from `&label`).

use std::collections::{HashMap, HashSet};

use oxigraph::model::{Graph, NamedNode, NamedOrBlankNode, NamedOrBlankNodeRef, Term, TermRef};

use super::ast::*;
use super::shexc::check_label_collisions;
use super::xsd::{Dec, Numeric};

const SX: &str = "http://www.w3.org/ns/shex#";
const RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";

type R<T> = Result<T, String>;

/// Read the schema in `g` (it must hold exactly one `sx:Schema`).
pub fn from_graph(g: &Graph) -> R<Schema> {
    let r = Reader {
        g,
        declared: HashSet::new(),
        defined_tes: std::cell::RefCell::new(HashSet::new()),
        te_refs: count_te_refs(g),
    };
    r.schema().map_err(|e| format!("ShExR: {e}"))
}

struct Reader<'g> {
    g: &'g Graph,
    declared: HashSet<NamedOrBlankNode>,
    defined_tes: std::cell::RefCell<HashSet<NamedOrBlankNode>>,
    te_refs: HashMap<NamedOrBlankNode, usize>,
}

fn sx(local: &str) -> NamedNode {
    NamedNode::new_unchecked(format!("{SX}{local}"))
}

fn rdf(local: &str) -> NamedNode {
    NamedNode::new_unchecked(format!("{RDF}{local}"))
}

fn as_subject(t: &Term) -> Option<NamedOrBlankNode> {
    match t {
        Term::NamedNode(n) => Some(n.clone().into()),
        Term::BlankNode(b) => Some(b.clone().into()),
        _ => None,
    }
}

fn label_of(n: &NamedOrBlankNode) -> Label {
    match n {
        NamedOrBlankNode::NamedNode(i) => Label::Iri(i.as_str().to_string()),
        NamedOrBlankNode::BlankNode(b) => Label::BNode(b.as_str().to_string()),
    }
}

/// How often each node is the object of `sx:expression` or a member of an
/// `sx:expressions` list: a triple expression used twice is labelled.
fn count_te_refs(g: &Graph) -> HashMap<NamedOrBlankNode, usize> {
    let mut out = HashMap::new();
    for t in g.triples_for_predicate(sx("expression").as_ref()) {
        if let Some(s) = as_subject(&t.object.into_owned()) {
            *out.entry(s).or_insert(0) += 1;
        }
    }
    for t in g.triples_for_predicate(sx("expressions").as_ref()) {
        for item in list_items(g, &t.object.into_owned()).unwrap_or_default() {
            if let Some(s) = as_subject(&item) {
                *out.entry(s).or_insert(0) += 1;
            }
        }
    }
    out
}

fn list_items(g: &Graph, head: &Term) -> R<Vec<Term>> {
    let mut out = Vec::new();
    let mut cur = head.clone();
    let nil = Term::from(rdf("nil"));
    let mut guard = 0;
    while cur != nil {
        guard += 1;
        if guard > 100_000 {
            return Err("an RDF list does not end".into());
        }
        let Some(node) = as_subject(&cur) else {
            return Err(format!("{cur} is not an RDF list"));
        };
        let first = g
            .object_for_subject_predicate(node.as_ref(), rdf("first").as_ref())
            .ok_or_else(|| format!("{cur} is not an RDF list"))?;
        out.push(first.into_owned());
        cur = g
            .object_for_subject_predicate(node.as_ref(), rdf("rest").as_ref())
            .ok_or_else(|| format!("{cur} has no rdf:rest"))?
            .into_owned();
    }
    Ok(out)
}

impl Reader<'_> {
    fn objects(&self, s: NamedOrBlankNodeRef<'_>, p: &str) -> Vec<Term> {
        self.g
            .objects_for_subject_predicate(s, sx(p).as_ref())
            .map(TermRef::into_owned)
            .collect()
    }

    fn one(&self, s: NamedOrBlankNodeRef<'_>, p: &str) -> Option<Term> {
        self.g
            .object_for_subject_predicate(s, sx(p).as_ref())
            .map(TermRef::into_owned)
    }

    /// The members of `p`: an RDF list, or several values.
    fn many(&self, s: NamedOrBlankNodeRef<'_>, p: &str) -> R<Vec<Term>> {
        let vals = self.objects(s, p);
        let mut out = Vec::new();
        for v in vals {
            let is_list = v == Term::from(rdf("nil"))
                || as_subject(&v).is_some_and(|n| {
                    self.g
                        .object_for_subject_predicate(n.as_ref(), rdf("first").as_ref())
                        .is_some()
                });
            if is_list {
                out.extend(list_items(self.g, &v)?);
            } else {
                out.push(v);
            }
        }
        Ok(out)
    }

    fn types(&self, s: NamedOrBlankNodeRef<'_>) -> Vec<String> {
        self.g
            .objects_for_subject_predicate(s, rdf("type").as_ref())
            .filter_map(|t| match t {
                TermRef::NamedNode(n) => n.as_str().strip_prefix(SX).map(str::to_string),
                _ => None,
            })
            .collect()
    }

    fn iri(&self, t: &Term) -> R<String> {
        match t {
            Term::NamedNode(n) => Ok(n.as_str().to_string()),
            // `sx:stem "…"^^xsd:anyURI` and friends.
            Term::Literal(l) => Ok(l.value().to_string()),
            other => Err(format!("expected an IRI, found {other}")),
        }
    }

    fn schema(mut self) -> R<Schema> {
        let roots: Vec<NamedOrBlankNode> = self
            .g
            .subjects_for_predicate_object(rdf("type").as_ref(), sx("Schema").as_ref())
            .map(|s| s.into_owned())
            .collect();
        let root = match roots.as_slice() {
            [r] => r.clone(),
            [] => return Err("the graph holds no sx:Schema".into()),
            _ => return Err("the graph holds more than one sx:Schema".into()),
        };
        let shape_nodes = self.many(root.as_ref(), "shapes")?;
        for s in &shape_nodes {
            if let Some(n) = as_subject(s) {
                self.declared.insert(n);
            }
        }
        let mut schema = Schema::default();
        for a in self.many(root.as_ref(), "startActs")? {
            schema.start_acts.push(self.sem_act(&a)?);
        }
        if let Some(st) = self.one(root.as_ref(), "start") {
            schema.start = Some(self.shape_expr(&st, false)?);
        }
        for i in self.many(root.as_ref(), "imports")? {
            schema.imports.push(self.iri(&i)?);
        }
        for s in &shape_nodes {
            let n = as_subject(s).ok_or("a shape declaration must be an IRI or blank node")?;
            let label = label_of(&n);
            let types = self.types(n.as_ref());
            let expr = if types.iter().any(|t| t == "ShapeDecl") {
                if self
                    .one(n.as_ref(), "abstract")
                    .is_some_and(|v| matches!(&v, Term::Literal(l) if l.value() == "true"))
                {
                    return Err(
                        "abstract shapes are ShEx 2.next, which this engine does not implement"
                            .into(),
                    );
                }
                let se = self
                    .one(n.as_ref(), "shapeExpr")
                    .ok_or_else(|| format!("{label} has no sx:shapeExpr"))?;
                self.shape_expr(&se, false)?
            } else {
                self.shape_expr(s, true)?
            };
            schema.shapes.push(ShapeDecl { label, expr });
        }
        check_label_collisions(&schema)?;
        Ok(schema)
    }

    /// `root` is true when `t` is itself a declaration (2.1 layout).
    fn shape_expr(&self, t: &Term, root: bool) -> R<ShapeExpr> {
        let n = as_subject(t).ok_or_else(|| format!("{t} is not a shape expression"))?;
        if !root && self.declared.contains(&n) {
            return Ok(ShapeExpr::Ref(label_of(&n)));
        }
        let types = self.types(n.as_ref());
        let Some(ty) = types.first() else {
            return Ok(ShapeExpr::Ref(label_of(&n)));
        };
        match ty.as_str() {
            "ShapeOr" | "ShapeAnd" => {
                let items = self
                    .many(n.as_ref(), "shapeExprs")?
                    .iter()
                    .map(|e| self.shape_expr(e, false))
                    .collect::<R<Vec<_>>>()?;
                Ok(if ty == "ShapeOr" {
                    ShapeExpr::Or(items)
                } else {
                    ShapeExpr::And(items)
                })
            }
            "ShapeNot" => {
                let inner = self
                    .one(n.as_ref(), "shapeExpr")
                    .ok_or("sx:ShapeNot has no sx:shapeExpr")?;
                Ok(ShapeExpr::Not(Box::new(self.shape_expr(&inner, false)?)))
            }
            "ShapeExternal" => Ok(ShapeExpr::External),
            "NodeConstraint" => Ok(ShapeExpr::NodeConstraint(self.node_constraint(n.as_ref())?)),
            "Shape" => Ok(ShapeExpr::Shape(Box::new(self.shape(n.as_ref())?))),
            other => Err(format!("sx:{other} is not a shape expression type")),
        }
    }

    fn shape(&self, n: NamedOrBlankNodeRef<'_>) -> R<Shape> {
        if self.one(n, "extends").is_some() {
            return Err("sx:extends is ShEx 2.next, which this engine does not implement".into());
        }
        let mut s = Shape {
            closed: self
                .one(n, "closed")
                .is_some_and(|v| matches!(&v, Term::Literal(l) if l.value() == "true")),
            ..Default::default()
        };
        for e in self.objects(n, "extra") {
            s.extra.push(self.iri(&e)?);
        }
        if let Some(e) = self.one(n, "expression") {
            s.expression = Some(self.triple_expr(&e)?);
        }
        s.sem_acts = self.sem_acts(n)?;
        s.annotations = self.annotations(n)?;
        Ok(s)
    }

    fn int(&self, n: NamedOrBlankNodeRef<'_>, p: &str) -> R<Option<i64>> {
        match self.one(n, p) {
            None => Ok(None),
            Some(Term::Literal(l)) => l
                .value()
                .parse()
                .map(Some)
                .map_err(|_| format!("sx:{p} must be an integer")),
            Some(other) => Err(format!("sx:{p} must be an integer, found {other}")),
        }
    }

    fn triple_expr(&self, t: &Term) -> R<TripleExpr> {
        let n = as_subject(t).ok_or_else(|| format!("{t} is not a triple expression"))?;
        let types = self.types(n.as_ref());
        let labelled = matches!(n, NamedOrBlankNode::NamedNode(_))
            || self.te_refs.get(&n).copied().unwrap_or(0) > 1;
        if types.is_empty() || (labelled && self.defined_tes.borrow().contains(&n)) {
            return Ok(TripleExpr::Ref(label_of(&n)));
        }
        if labelled {
            self.defined_tes.borrow_mut().insert(n.clone());
        }
        let id = labelled.then(|| label_of(&n));
        let min = self.int(n.as_ref(), "min")?.unwrap_or(1);
        let max = match self.int(n.as_ref(), "max")? {
            None => Max::Bounded(1),
            Some(-1) => Max::Unbounded,
            Some(m) => Max::Bounded(u32::try_from(m).map_err(|_| "bad sx:max")?),
        };
        let min = u32::try_from(min).map_err(|_| "bad sx:min")?;
        let sem_acts = self.sem_acts(n.as_ref())?;
        let annotations = self.annotations(n.as_ref())?;
        match types[0].as_str() {
            "EachOf" | "OneOf" => {
                let expressions = self
                    .many(n.as_ref(), "expressions")?
                    .iter()
                    .map(|e| self.triple_expr(e))
                    .collect::<R<Vec<_>>>()?;
                let g = Group {
                    id,
                    expressions,
                    min,
                    max,
                    sem_acts,
                    annotations,
                };
                Ok(if types[0] == "EachOf" {
                    TripleExpr::EachOf(g)
                } else {
                    TripleExpr::OneOf(g)
                })
            }
            "TripleConstraint" => {
                let predicate = self.iri(
                    &self
                        .one(n.as_ref(), "predicate")
                        .ok_or("sx:TripleConstraint has no sx:predicate")?,
                )?;
                let value_expr = self
                    .one(n.as_ref(), "valueExpr")
                    .map(|v| self.shape_expr(&v, false).map(Box::new))
                    .transpose()?;
                Ok(TripleExpr::TripleConstraint(TripleConstraint {
                    id,
                    inverse: self
                        .one(n.as_ref(), "inverse")
                        .is_some_and(|v| matches!(&v, Term::Literal(l) if l.value() == "true")),
                    predicate,
                    value_expr,
                    min,
                    max,
                    sem_acts,
                    annotations,
                }))
            }
            other => Err(format!("sx:{other} is not a triple expression type")),
        }
    }

    fn node_constraint(&self, n: NamedOrBlankNodeRef<'_>) -> R<NodeConstraint> {
        let mut nc = NodeConstraint::default();
        if let Some(k) = self.one(n, "nodeKind") {
            let k = self.iri(&k)?;
            nc.node_kind = Some(match k.strip_prefix(SX) {
                Some("iri") => NodeKind::Iri,
                Some("bnode") => NodeKind::BNode,
                Some("nonliteral") => NodeKind::NonLiteral,
                Some("literal") => NodeKind::Literal,
                _ => return Err(format!("unknown sx:nodeKind <{k}>")),
            });
        }
        if let Some(d) = self.one(n, "datatype") {
            nc.datatype = Some(self.iri(&d)?);
        }
        let uint = |p: &str| -> R<Option<u64>> {
            self.int(n, p)?
                .map(|v| u64::try_from(v).map_err(|_| format!("sx:{p} must not be negative")))
                .transpose()
        };
        if let Some(v) = uint("length")? {
            nc.string_facets.push(StringFacet::Length(v));
        }
        if let Some(v) = uint("minlength")? {
            nc.string_facets.push(StringFacet::MinLength(v));
        }
        if let Some(v) = uint("maxlength")? {
            nc.string_facets.push(StringFacet::MaxLength(v));
        }
        if let Some(Term::Literal(p)) = self.one(n, "pattern") {
            let flags = match self.one(n, "flags") {
                Some(Term::Literal(f)) => Some(f.value().to_string()),
                _ => None,
            };
            nc.string_facets
                .push(StringFacet::Pattern(p.value().to_string(), flags));
        }
        for (p, mk) in [
            (
                "mininclusive",
                NumericFacet::MinInclusive as fn(Numeric) -> NumericFacet,
            ),
            ("minexclusive", NumericFacet::MinExclusive),
            ("maxinclusive", NumericFacet::MaxInclusive),
            ("maxexclusive", NumericFacet::MaxExclusive),
        ] {
            if let Some(Term::Literal(l)) = self.one(n, p) {
                let v = if l.value().contains(['e', 'E']) {
                    Numeric::parse_token(l.value())
                } else {
                    Dec::parse(l.value()).map(Numeric::Decimal)
                }
                .ok_or_else(|| format!("sx:{p} is not a number"))?;
                nc.numeric_facets.push(mk(v));
            }
        }
        if let Some(v) = uint("totaldigits")? {
            nc.numeric_facets.push(NumericFacet::TotalDigits(v));
        }
        if let Some(v) = uint("fractiondigits")? {
            nc.numeric_facets.push(NumericFacet::FractionDigits(v));
        }
        nc.sem_acts = self.sem_acts(n)?;
        nc.annotations = self.annotations(n)?;
        if !self.objects(n, "values").is_empty() {
            nc.values = Some(
                self.many(n, "values")?
                    .iter()
                    .map(|v| self.value(v).map(ValueSetValue::lowercase_languages))
                    .collect::<R<Vec<_>>>()?,
            );
        }
        Ok(nc)
    }

    fn object_value(&self, t: &Term) -> R<ObjectValue> {
        match t {
            Term::NamedNode(n) => Ok(ObjectValue::Iri(n.as_str().to_string())),
            Term::Literal(l) => Ok(ObjectValue::Literal(ObjectLiteral {
                value: l.value().to_string(),
                language: l.language().map(str::to_ascii_lowercase),
                datatype: (l.language().is_none() && l.datatype().as_str() != XSD_STRING)
                    .then(|| l.datatype().as_str().to_string()),
            })),
            other => Err(format!("{other} is not an IRI or literal")),
        }
    }

    fn value(&self, t: &Term) -> R<ValueSetValue> {
        let Some(n) = as_subject(t).filter(|n| matches!(n, NamedOrBlankNode::BlankNode(_))) else {
            return Ok(ValueSetValue::Object(self.object_value(t)?));
        };
        let types = self.types(n.as_ref());
        let ty = types.first().cloned().unwrap_or_default();
        let stem_str = |iri: bool| -> R<Stem> {
            match self.one(n.as_ref(), "stem") {
                Some(Term::Literal(l)) => Ok(Stem::Value(l.value().to_string())),
                Some(Term::NamedNode(i)) if iri => Ok(Stem::Value(i.as_str().to_string())),
                Some(s @ Term::BlankNode(_))
                    if self
                        .types(as_subject(&s).unwrap().as_ref())
                        .iter()
                        .any(|t| t == "Wildcard") =>
                {
                    Ok(Stem::Wildcard)
                }
                other => Err(format!("bad sx:stem {other:?}")),
            }
        };
        let plain = |iri: bool| -> R<String> {
            match stem_str(iri)? {
                Stem::Value(s) => Ok(s),
                Stem::Wildcard => Err("a stem cannot be a Wildcard here".into()),
            }
        };
        let excl = |iri: bool| -> R<Vec<Exclusion>> {
            self.many(n.as_ref(), "exclusion")?
                .iter()
                .map(|e| match e {
                    Term::NamedNode(i) if iri => Ok(Exclusion::Value(i.as_str().to_string())),
                    Term::Literal(l) => Ok(Exclusion::Value(l.value().to_string())),
                    Term::BlankNode(b) => {
                        let s = self
                            .one(b.as_ref().into(), "stem")
                            .ok_or("an exclusion stem has no sx:stem")?;
                        Ok(Exclusion::Stem(self.iri(&s)?))
                    }
                    other => Err(format!("bad exclusion {other}")),
                })
                .collect()
        };
        Ok(match ty.as_str() {
            "IriStem" => ValueSetValue::IriStem(plain(true)?),
            "IriStemRange" => ValueSetValue::IriStemRange(stem_str(true)?, excl(true)?),
            "LiteralStem" => ValueSetValue::LiteralStem(plain(false)?),
            "LiteralStemRange" => ValueSetValue::LiteralStemRange(stem_str(false)?, excl(false)?),
            "Language" => ValueSetValue::Language(match self.one(n.as_ref(), "languageTag") {
                Some(Term::Literal(l)) => l.value().to_string(),
                _ => return Err("sx:Language has no sx:languageTag".into()),
            }),
            "LanguageStem" => ValueSetValue::LanguageStem(plain(false)?),
            "LanguageStemRange" => ValueSetValue::LanguageStemRange(stem_str(false)?, excl(false)?),
            other => return Err(format!("sx:{other} is not a value-set value type")),
        })
    }

    fn sem_act(&self, t: &Term) -> R<SemAct> {
        let n = as_subject(t).ok_or("a semantic action must be a node")?;
        let name = self.iri(
            &self
                .one(n.as_ref(), "name")
                .ok_or("sx:SemAct has no sx:name")?,
        )?;
        let code = match self.one(n.as_ref(), "code") {
            Some(Term::Literal(l)) => Some(l.value().to_string()),
            _ => None,
        };
        Ok(SemAct { name, code })
    }

    fn sem_acts(&self, n: NamedOrBlankNodeRef<'_>) -> R<Vec<SemAct>> {
        self.many(n, "semActs")?
            .iter()
            .map(|a| self.sem_act(a))
            .collect()
    }

    fn annotations(&self, n: NamedOrBlankNodeRef<'_>) -> R<Vec<Annotation>> {
        self.many(n, "annotation")?
            .iter()
            .map(|a| {
                let a = as_subject(a).ok_or("an annotation must be a node")?;
                Ok(Annotation {
                    predicate: self.iri(
                        &self
                            .one(a.as_ref(), "predicate")
                            .ok_or("sx:Annotation has no sx:predicate")?,
                    )?,
                    object: self.object_value(
                        &self
                            .one(a.as_ref(), "object")
                            .ok_or("sx:Annotation has no sx:object")?,
                    )?,
                })
            })
            .collect()
    }
}
