//! ShExJ — the JSON representation of ShEx 2.1 (Appendix A).
//!
//! Reads the ShEx 2.1 form, where a top-level shape expression carries its
//! `id`, and the `ShapeDecl` wrapper the shexTest suite and later drafts use
//! (`{"type": "ShapeDecl", "id": …, "shapeExpr": …}`). A `ShapeDecl` marked
//! `abstract`, and `extends`, are ShEx 2.next and refused. Relative IRIs are
//! resolved against `base`.

use serde_json::{Map, Value};

use super::ast::*;
use super::shexc::{check_label_collisions, resolve_iri};
use super::xsd::{Dec, Numeric};

type R<T> = Result<T, String>;

/// Parse a ShExJ document from its JSON text.
pub fn parse(text: &str, base: Option<&str>) -> R<Schema> {
    let v: Value = serde_json::from_str(text).map_err(|e| format!("ShExJ is not JSON: {e}"))?;
    from_value(&v, base)
}

/// Read a ShExJ document already parsed as JSON.
pub fn from_value(v: &Value, base: Option<&str>) -> R<Schema> {
    let r = Reader { base };
    let schema = r.schema(v).map_err(|e| format!("ShExJ: {e}"))?;
    check_label_collisions(&schema)?;
    Ok(schema)
}

struct Reader<'a> {
    base: Option<&'a str>,
}

fn obj(v: &Value) -> R<&Map<String, Value>> {
    v.as_object()
        .ok_or_else(|| format!("expected an object, found {v}"))
}

fn ty(m: &Map<String, Value>) -> &str {
    m.get("type").and_then(Value::as_str).unwrap_or("")
}

fn string(v: &Value) -> R<&str> {
    v.as_str()
        .ok_or_else(|| format!("expected a string, found {v}"))
}

fn array<'v>(m: &'v Map<String, Value>, key: &str) -> R<&'v [Value]> {
    match m.get(key) {
        None => Ok(&[]),
        Some(Value::Array(a)) => Ok(a),
        Some(other) => Err(format!("'{key}' must be an array, found {other}")),
    }
}

impl Reader<'_> {
    fn iri(&self, s: &str) -> R<String> {
        resolve_iri(self.base, s)
    }

    fn label(&self, s: &str) -> R<Label> {
        match s.strip_prefix("_:") {
            Some(b) => Ok(Label::BNode(b.to_string())),
            None => Ok(Label::Iri(self.iri(s)?)),
        }
    }

    fn schema(&self, v: &Value) -> R<Schema> {
        let m = obj(v)?;
        if !matches!(ty(m), "Schema" | "") {
            return Err(format!("the top-level object is a {}, not a Schema", ty(m)));
        }
        let mut s = Schema {
            start_acts: self.sem_acts(m, "startActs")?,
            ..Default::default()
        };
        if let Some(st) = m.get("start") {
            s.start = Some(self.shape_expr(st)?);
        }
        for i in array(m, "imports")? {
            s.imports.push(self.iri(string(i)?)?);
        }
        for d in array(m, "shapes")? {
            s.shapes.push(self.decl(d)?);
        }
        Ok(s)
    }

    fn decl(&self, v: &Value) -> R<ShapeDecl> {
        let m = obj(v)?;
        let id = m
            .get("id")
            .ok_or("a shape declaration has no 'id'")
            .and_then(|i| string(i).map_err(|_| "a shape 'id' must be a string"))?;
        let label = self.label(id)?;
        if ty(m) == "ShapeDecl" {
            if m.get("abstract").and_then(Value::as_bool) == Some(true) {
                return Err(
                    "abstract shapes are ShEx 2.next, which this engine does not implement".into(),
                );
            }
            let se = m.get("shapeExpr").ok_or("a ShapeDecl has no 'shapeExpr'")?;
            return Ok(ShapeDecl {
                label,
                expr: self.shape_expr(se)?,
            });
        }
        Ok(ShapeDecl {
            label,
            expr: self.shape_expr(v)?,
        })
    }

    fn shape_expr(&self, v: &Value) -> R<ShapeExpr> {
        if let Some(s) = v.as_str() {
            return Ok(ShapeExpr::Ref(self.label(s)?));
        }
        let m = obj(v)?;
        match ty(m) {
            "ShapeOr" | "ShapeAnd" => {
                let items = array(m, "shapeExprs")?
                    .iter()
                    .map(|e| self.shape_expr(e))
                    .collect::<R<Vec<_>>>()?;
                if items.len() < 2 {
                    return Err(format!("{} needs at least two shapeExprs", ty(m)));
                }
                Ok(if ty(m) == "ShapeOr" {
                    ShapeExpr::Or(items)
                } else {
                    ShapeExpr::And(items)
                })
            }
            "ShapeNot" => {
                let inner = m.get("shapeExpr").ok_or("ShapeNot has no 'shapeExpr'")?;
                Ok(ShapeExpr::Not(Box::new(self.shape_expr(inner)?)))
            }
            "ShapeExternal" => Ok(ShapeExpr::External),
            "NodeConstraint" => Ok(ShapeExpr::NodeConstraint(self.node_constraint(m)?)),
            "Shape" => Ok(ShapeExpr::Shape(Box::new(self.shape(m)?))),
            "ShapeDecl" => Err("a ShapeDecl may appear only in 'shapes'".into()),
            other => Err(format!("unknown shape expression type '{other}'")),
        }
    }

    fn shape(&self, m: &Map<String, Value>) -> R<Shape> {
        if m.contains_key("extends") || m.contains_key("restricts") {
            return Err("'extends' is ShEx 2.next, which this engine does not implement".into());
        }
        let mut s = Shape {
            closed: m.get("closed").and_then(Value::as_bool).unwrap_or(false),
            ..Default::default()
        };
        for e in array(m, "extra")? {
            s.extra.push(self.iri(string(e)?)?);
        }
        if let Some(e) = m.get("expression") {
            s.expression = Some(self.triple_expr(e)?);
        }
        s.sem_acts = self.sem_acts(m, "semActs")?;
        s.annotations = self.annotations(m)?;
        Ok(s)
    }

    fn cardinality(&self, m: &Map<String, Value>) -> R<(u32, Max)> {
        let min = match m.get("min") {
            None => 1,
            Some(v) => v
                .as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .ok_or_else(|| format!("bad min {v}"))?,
        };
        let max = match m.get("max") {
            None => Max::Bounded(1),
            Some(v) if v.as_i64() == Some(-1) => Max::Unbounded,
            Some(v) => Max::Bounded(
                v.as_u64()
                    .and_then(|n| u32::try_from(n).ok())
                    .ok_or_else(|| format!("bad max {v}"))?,
            ),
        };
        if let Max::Bounded(mx) = max {
            if mx < min {
                return Err(format!("cardinality {{{min},{mx}}} has max < min"));
            }
        }
        Ok((min, max))
    }

    fn triple_expr(&self, v: &Value) -> R<TripleExpr> {
        if let Some(s) = v.as_str() {
            return Ok(TripleExpr::Ref(self.label(s)?));
        }
        let m = obj(v)?;
        let id = m
            .get("id")
            .map(|i| string(i).and_then(|s| self.label(s)))
            .transpose()?;
        let (min, max) = self.cardinality(m)?;
        let sem_acts = self.sem_acts(m, "semActs")?;
        let annotations = self.annotations(m)?;
        match ty(m) {
            "EachOf" | "OneOf" => {
                let expressions = array(m, "expressions")?
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
                Ok(if ty(m) == "EachOf" {
                    TripleExpr::EachOf(g)
                } else {
                    TripleExpr::OneOf(g)
                })
            }
            "TripleConstraint" => {
                let predicate = self.iri(string(
                    m.get("predicate")
                        .ok_or("a TripleConstraint has no 'predicate'")?,
                )?)?;
                let value_expr = m
                    .get("valueExpr")
                    .map(|e| self.shape_expr(e).map(Box::new))
                    .transpose()?;
                Ok(TripleExpr::TripleConstraint(TripleConstraint {
                    id,
                    inverse: m.get("inverse").and_then(Value::as_bool).unwrap_or(false),
                    predicate,
                    value_expr,
                    min,
                    max,
                    sem_acts,
                    annotations,
                }))
            }
            other => Err(format!("unknown triple expression type '{other}'")),
        }
    }

    fn node_constraint(&self, m: &Map<String, Value>) -> R<NodeConstraint> {
        let mut nc = NodeConstraint::default();
        if let Some(k) = m.get("nodeKind") {
            nc.node_kind = Some(match string(k)? {
                "iri" => NodeKind::Iri,
                "bnode" => NodeKind::BNode,
                "nonliteral" => NodeKind::NonLiteral,
                "literal" => NodeKind::Literal,
                other => return Err(format!("unknown nodeKind '{other}'")),
            });
        }
        if let Some(d) = m.get("datatype") {
            nc.datatype = Some(self.iri(string(d)?)?);
        }
        let uint = |k: &str| -> R<Option<u64>> {
            m.get(k)
                .map(|v| {
                    v.as_u64()
                        .ok_or_else(|| format!("'{k}' must be a non-negative integer"))
                })
                .transpose()
        };
        // Facets in the order ShExC writes them is not recoverable from a
        // JSON object; use a fixed order so equal schemas compare equal.
        if let Some(n) = uint("length")? {
            nc.string_facets.push(StringFacet::Length(n));
        }
        if let Some(n) = uint("minlength")? {
            nc.string_facets.push(StringFacet::MinLength(n));
        }
        if let Some(n) = uint("maxlength")? {
            nc.string_facets.push(StringFacet::MaxLength(n));
        }
        if let Some(p) = m.get("pattern") {
            let flags = m
                .get("flags")
                .map(|f| string(f).map(str::to_string))
                .transpose()?;
            nc.string_facets
                .push(StringFacet::Pattern(string(p)?.to_string(), flags));
        }
        for (k, mk) in [
            (
                "mininclusive",
                NumericFacet::MinInclusive as fn(Numeric) -> NumericFacet,
            ),
            ("minexclusive", NumericFacet::MinExclusive),
            ("maxinclusive", NumericFacet::MaxInclusive),
            ("maxexclusive", NumericFacet::MaxExclusive),
        ] {
            if let Some(v) = m.get(k) {
                nc.numeric_facets.push(mk(json_number(v)?));
            }
        }
        if let Some(n) = uint("totaldigits")? {
            nc.numeric_facets.push(NumericFacet::TotalDigits(n));
        }
        if let Some(n) = uint("fractiondigits")? {
            nc.numeric_facets.push(NumericFacet::FractionDigits(n));
        }
        nc.sem_acts = self.sem_acts(m, "semActs")?;
        nc.annotations = self.annotations(m)?;
        if let Some(vs) = m.get("values") {
            let items = vs.as_array().ok_or("'values' must be an array")?;
            nc.values = Some(
                items
                    .iter()
                    .map(|v| {
                        self.value_set_value(v)
                            .map(ValueSetValue::lowercase_languages)
                    })
                    .collect::<R<Vec<_>>>()?,
            );
        }
        Ok(nc)
    }

    fn object_value(&self, v: &Value) -> R<ObjectValue> {
        if let Some(s) = v.as_str() {
            return Ok(ObjectValue::Iri(self.iri(s)?));
        }
        let m = obj(v)?;
        let value = string(m.get("value").ok_or("an object literal has no 'value'")?)?.to_string();
        let language = m
            .get("language")
            .map(|l| string(l).map(str::to_ascii_lowercase))
            .transpose()?;
        let datatype = m
            .get("type")
            .map(|t| string(t).and_then(|t| self.iri(t)))
            .transpose()?
            .filter(|d| d != XSD_STRING && !(language.is_some() && d == RDF_LANG_STRING));
        Ok(ObjectValue::Literal(ObjectLiteral {
            value,
            language,
            datatype,
        }))
    }

    fn value_set_value(&self, v: &Value) -> R<ValueSetValue> {
        if v.is_string() {
            return Ok(ValueSetValue::Object(self.object_value(v)?));
        }
        let m = obj(v)?;
        let stem = |m: &Map<String, Value>, iri: bool| -> R<Stem> {
            match m.get("stem") {
                Some(Value::String(s)) => {
                    Ok(Stem::Value(if iri { self.iri(s)? } else { s.clone() }))
                }
                Some(Value::Object(w)) if ty(w) == "Wildcard" => Ok(Stem::Wildcard),
                other => Err(format!("bad stem {other:?}")),
            }
        };
        let plain_stem = |m: &Map<String, Value>, iri: bool| -> R<String> {
            match stem(m, iri)? {
                Stem::Value(s) => Ok(s),
                Stem::Wildcard => Err("a stem cannot be a Wildcard here".into()),
            }
        };
        let exclusions = |m: &Map<String, Value>, iri: bool, stem_ty: &str| -> R<Vec<Exclusion>> {
            array(m, "exclusions")?
                .iter()
                .map(|e| match e {
                    Value::String(s) => {
                        Ok(Exclusion::Value(if iri { self.iri(s)? } else { s.clone() }))
                    }
                    Value::Object(o) if ty(o) == stem_ty => {
                        Ok(Exclusion::Stem(match o.get("stem") {
                            Some(Value::String(s)) if iri => self.iri(s)?,
                            Some(Value::String(s)) => s.clone(),
                            _ => return Err("an exclusion stem must be a string".into()),
                        }))
                    }
                    other => Err(format!("bad exclusion {other}")),
                })
                .collect()
        };
        match ty(m) {
            "IriStem" => Ok(ValueSetValue::IriStem(plain_stem(m, true)?)),
            "IriStemRange" => Ok(ValueSetValue::IriStemRange(
                stem(m, true)?,
                exclusions(m, true, "IriStem")?,
            )),
            "LiteralStem" => Ok(ValueSetValue::LiteralStem(plain_stem(m, false)?)),
            "LiteralStemRange" => Ok(ValueSetValue::LiteralStemRange(
                stem(m, false)?,
                exclusions(m, false, "LiteralStem")?,
            )),
            "Language" => Ok(ValueSetValue::Language(
                string(
                    m.get("languageTag")
                        .ok_or("a Language has no 'languageTag'")?,
                )?
                .to_ascii_lowercase(),
            )),
            "LanguageStem" => Ok(ValueSetValue::LanguageStem(
                plain_stem(m, false)?.to_ascii_lowercase(),
            )),
            "LanguageStemRange" => Ok(ValueSetValue::LanguageStemRange(
                stem(m, false)?,
                exclusions(m, false, "LanguageStem")?,
            )),
            _ => Ok(ValueSetValue::Object(self.object_value(v)?)),
        }
    }

    fn sem_acts(&self, m: &Map<String, Value>, key: &str) -> R<Vec<SemAct>> {
        array(m, key)?
            .iter()
            .map(|a| {
                let a = obj(a)?;
                Ok(SemAct {
                    name: self.iri(string(a.get("name").ok_or("a SemAct has no 'name'")?)?)?,
                    code: a
                        .get("code")
                        .map(|c| string(c).map(str::to_string))
                        .transpose()?,
                })
            })
            .collect()
    }

    fn annotations(&self, m: &Map<String, Value>) -> R<Vec<Annotation>> {
        array(m, "annotations")?
            .iter()
            .map(|a| {
                let a = obj(a)?;
                Ok(Annotation {
                    predicate: self.iri(string(
                        a.get("predicate")
                            .ok_or("an Annotation has no 'predicate'")?,
                    )?)?,
                    object: self
                        .object_value(a.get("object").ok_or("an Annotation has no 'object'")?)?,
                })
            })
            .collect()
    }
}

/// A JSON number as a facet bound: integers and plain decimals exactly,
/// exponent forms as doubles.
fn json_number(v: &Value) -> R<Numeric> {
    let Value::Number(n) = v else {
        return Err(format!("a numeric facet must be a number, found {v}"));
    };
    let text = n.to_string();
    if text.contains(['e', 'E']) {
        return n
            .as_f64()
            .map(Numeric::Double)
            .ok_or_else(|| format!("bad number {text}"));
    }
    Dec::parse(&text)
        .map(Numeric::Decimal)
        .ok_or_else(|| format!("bad number {text}"))
}
