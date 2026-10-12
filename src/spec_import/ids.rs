//! buildingSMART IDS (Information Delivery Specification, 1.0) → SHACL.
//!
//! An IDS document is checked against the IDS 1.0 XSD and the IDS audit rules
//! first ([`read`], [`audit`]); a document that fails either is refused with
//! every problem listed, because a specification that "could not be satisfied,
//! regardless of IFC contents" must not become shapes that quietly pass.
//!
//! Every `ids:specification` then becomes SHACL-SPARQL shapes over the IDS
//! projection of the IFC lift (`crate::ifc::ids_projection`):
//!
//! - **targets** — `sh:targetClass` for each exact class the applicability's
//!   entity facet admits (`…/ifc-ids/IFC4#IFCWALL`), in every schema's
//!   namespace: the `ifcVersion` list decides which names the audit accepts,
//!   and — as the buildingSMART corpus requires — the checks then apply to a
//!   model of any schema. The projection writes no subclass axioms, so a
//!   target matches its class and nothing below it. A pattern or enumeration
//!   is expanded against the schema tables; without an entity facet the
//!   specification targets every concrete class;
//! - **one `sh:sparql` constraint per requirement facet**, selecting `$this`
//!   when the node meets the applicability and the facet's cardinality is
//!   broken: `required` → the facet does not hold; `prohibited` → it holds
//!   (the opposite of required); `optional` → it is present but does not hold;
//! - **existence**: a required specification (the XSD default) gets a shape on
//!   the model node that fails when the model has no applicable entity; a
//!   prohibited one fails on every applicable entity.
//!
//! Facet semantics follow the IDS documentation and test corpus: values are
//! compared by the XSD base of the facet's data type — doubles within the IDS
//! tolerance (`v ± (|v|·1e-6 + 1e-6)`, bound included), ranges without tolerance,
//! strings case-sensitively, booleans as `true`/`false`; patterns are XSD
//! patterns (anchored, translated by [`super::xsd_regex`]); every property set
//! and every property a name restriction matches must satisfy the facet;
//! aggregation and nesting are walked transitively, containment through the
//! element's aggregation ancestors.
//!
//! Each specification shape keeps its IDS source (`ots:idsSpecification`) and
//! a fingerprint of the constraints generated from it, so export can write the
//! specification back unchanged while the shapes are unedited
//! (`super::ids_export`).

use std::collections::BTreeSet;
use std::fmt::Write as _;

use quick_xml::events::Event;
use quick_xml::Reader;
use sha2::{Digest, Sha256};

use super::{xsd_regex, ImportedShapes, SpecImporter, SpecSummary};
use crate::ifc::ids_projection::{class_ns, ATTR_NS, NS as P};
use crate::ifc::schema::{self, AttrKind, Primitive, SchemaId, XsBase};

pub struct IdsImporter;

impl SpecImporter for IdsImporter {
    fn id(&self) -> &'static str {
        "ids"
    }
    fn label(&self) -> &'static str {
        "buildingSMART IDS 1.0"
    }
    fn media_types(&self) -> &'static [&'static str] {
        &["application/xml", "text/xml"]
    }
    fn import(&self, bytes: &[u8]) -> anyhow::Result<ImportedShapes> {
        let root = parse_xml(bytes)?;
        let doc = read(&root)
            .map_err(|e| anyhow::anyhow!("not a valid IDS 1.0 document: {}", e.join("; ")))?;
        let problems = audit(&doc);
        if !problems.is_empty() {
            anyhow::bail!(
                "the IDS fails the IDS audit rules, so no model could satisfy it: {}",
                problems.join("; ")
            );
        }
        Ok(convert(&doc))
    }
}

// ── XML ─────────────────────────────────────────────────────────────────────

pub(crate) const IDS_NS: &str = "http://standards.buildingsmart.org/IDS";
pub(crate) const XS_NS: &str = "http://www.w3.org/2001/XMLSchema";
const XSI_NS: &str = "http://www.w3.org/2001/XMLSchema-instance";
const XML_NS: &str = "http://www.w3.org/XML/1998/namespace";

/// A namespace-aware element.
#[derive(Debug, Default, Clone)]
pub(crate) struct El {
    pub name: String,
    pub ns: String,
    /// (local name, namespace or "", value)
    pub attrs: Vec<(String, String, String)>,
    pub text: String,
    pub children: Vec<El>,
}

impl El {
    pub(crate) fn attr(&self, k: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(a, ns, _)| a == k && ns.is_empty())
            .map(|(_, _, v)| v.as_str())
    }
    fn child(&self, name: &str) -> Option<&El> {
        self.children.iter().find(|c| c.name == name)
    }
    fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a El> + 'a {
        self.children.iter().filter(move |c| c.name == name)
    }

    /// Serialise with the `ids:` / `xs:` prefixes (the namespaces declared on
    /// the outermost element written).
    pub(crate) fn to_xml(&self, out: &mut String, indent: usize, root: bool) {
        let pad = "  ".repeat(indent);
        let tag = self.qname();
        let _ = write!(out, "{pad}<{tag}");
        if root {
            let _ = write!(out, " xmlns:ids=\"{IDS_NS}\" xmlns:xs=\"{XS_NS}\"");
        }
        for (k, ns, v) in &self.attrs {
            let key = if ns.is_empty() {
                k.clone()
            } else if ns == XML_NS {
                format!("xml:{k}")
            } else {
                continue;
            };
            let _ = write!(out, " {key}=\"{}\"", xml_esc(v));
        }
        let text = if self.text.trim().is_empty() {
            ""
        } else {
            self.text.as_str()
        };
        if self.children.is_empty() && text.is_empty() {
            out.push_str("/>\n");
            return;
        }
        out.push('>');
        if self.children.is_empty() {
            let _ = writeln!(out, "{}</{tag}>", xml_esc(text));
            return;
        }
        out.push('\n');
        for c in &self.children {
            c.to_xml(out, indent + 1, false);
        }
        let _ = writeln!(out, "{pad}</{tag}>");
    }

    fn qname(&self) -> String {
        match self.ns.as_str() {
            IDS_NS => format!("ids:{}", self.name),
            XS_NS => format!("xs:{}", self.name),
            _ => self.name.clone(),
        }
    }
}

pub(crate) fn xml_esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn split_qname(q: &str) -> (String, String) {
    let q = q.to_string();
    match q.split_once(':') {
        Some((p, l)) => (p.to_string(), l.to_string()),
        None => (String::new(), q),
    }
}

pub(crate) fn parse_xml(bytes: &[u8]) -> anyhow::Result<El> {
    let mut reader = Reader::from_reader(bytes);
    reader.config_mut().trim_text(false);
    let mut buf = Vec::new();
    // Namespace scopes: (prefix, uri) pushed per element.
    let mut scopes: Vec<Vec<(String, String)>> = Vec::new();
    let mut stack: Vec<El> = vec![El {
        name: "#root".into(),
        ..Default::default()
    }];
    fn resolve(scopes: &[Vec<(String, String)>], prefix: &str) -> Option<String> {
        if prefix == "xml" {
            return Some(XML_NS.to_string());
        }
        for scope in scopes.iter().rev() {
            if let Some((_, uri)) = scope.iter().find(|(p, _)| p == prefix) {
                return Some(uri.clone());
            }
        }
        if prefix.is_empty() {
            Some(String::new())
        } else {
            None
        }
    }
    let open = |e: &quick_xml::events::BytesStart<'_>,
                scopes: &mut Vec<Vec<(String, String)>>|
     -> anyhow::Result<El> {
        let mut scope = Vec::new();
        let mut raw_attrs = Vec::new();
        for a in e.attributes() {
            let a = a.map_err(|e| anyhow::anyhow!("malformed attribute: {e}"))?;
            let key = a.key.as_ref().to_string();
            let value = a
                .normalized_value(quick_xml::XmlVersion::default())
                .map(|v| v.into_owned())
                .unwrap_or_default();
            if key == "xmlns" {
                scope.push((String::new(), value));
            } else if let Some(p) = key.strip_prefix("xmlns:") {
                scope.push((p.to_string(), value));
            } else {
                raw_attrs.push((key, value));
            }
        }
        scopes.push(scope);
        let (prefix, local) = split_qname(e.name().as_ref());
        let ns = resolve(scopes, &prefix)
            .ok_or_else(|| anyhow::anyhow!("undeclared namespace prefix `{prefix}`"))?;
        let mut el = El {
            name: local,
            ns,
            ..Default::default()
        };
        for (key, value) in raw_attrs {
            let (p, l) = split_qname(&key);
            let ans = if p.is_empty() {
                String::new()
            } else {
                resolve(scopes, &p)
                    .ok_or_else(|| anyhow::anyhow!("undeclared namespace prefix `{p}`"))?
            };
            el.attrs.push((l, ans, value));
        }
        Ok(el)
    };
    loop {
        match reader.read_event_into(&mut buf)? {
            Event::Start(e) => {
                let el = open(&e, &mut scopes)?;
                stack.push(el);
            }
            Event::Empty(e) => {
                let el = open(&e, &mut scopes)?;
                scopes.pop();
                stack.last_mut().unwrap().children.push(el);
            }
            Event::End(_) => {
                scopes.pop();
                if stack.len() > 1 {
                    let el = stack.pop().unwrap();
                    stack.last_mut().unwrap().children.push(el);
                }
            }
            Event::Text(t) => {
                let s = t.xml10_content();
                stack.last_mut().unwrap().text.push_str(&s);
            }
            // Entity and character references arrive as their own events.
            Event::GeneralRef(r) => {
                let resolved = match r.resolve_char_ref() {
                    Ok(Some(c)) => c.to_string(),
                    Ok(None) => match r.xml10_content().as_ref() {
                        "amp" => "&".into(),
                        "lt" => "<".into(),
                        "gt" => ">".into(),
                        "quot" => "\"".into(),
                        "apos" => "'".into(),
                        other => anyhow::bail!("undefined entity `&{other};`"),
                    },
                    Err(e) => anyhow::bail!("malformed character reference: {e}"),
                };
                stack.last_mut().unwrap().text.push_str(&resolved);
            }
            Event::CData(c) => {
                let s = c.into_inner();
                stack.last_mut().unwrap().text.push_str(&s);
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    if stack.len() > 1 {
        anyhow::bail!("the document ends inside <{}>", stack.last().unwrap().name);
    }
    let root = stack.pop().ok_or_else(|| anyhow::anyhow!("no XML root"))?;
    root.children
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("empty document"))
}

// ── model ───────────────────────────────────────────────────────────────────

/// An `ids:idsValue`: a simple value or an `xs:restriction`.
#[derive(Debug, Clone)]
pub(crate) enum Value {
    Simple(String),
    Restriction(Box<Restriction>),
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Restriction {
    pub base: Option<String>,
    pub enums: Vec<String>,
    pub patterns: Vec<String>,
    pub min_inclusive: Option<String>,
    pub max_inclusive: Option<String>,
    pub min_exclusive: Option<String>,
    pub max_exclusive: Option<String>,
    pub length: Option<String>,
    pub min_length: Option<String>,
    pub max_length: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Card {
    Required,
    Optional,
    Prohibited,
}

#[derive(Debug, Clone)]
pub(crate) enum Facet {
    Entity {
        name: Value,
        predefined: Option<Value>,
    },
    Attribute {
        name: Value,
        value: Option<Value>,
        card: Card,
    },
    Property {
        pset: Value,
        name: Value,
        data_type: Option<String>,
        value: Option<Value>,
        card: Card,
    },
    Classification {
        system: Option<Value>,
        value: Option<Value>,
        card: Card,
    },
    Material {
        value: Option<Value>,
        card: Card,
    },
    PartOf {
        relation: Option<String>,
        entity: Value,
        predefined: Option<Value>,
        card: Card,
    },
}

impl Facet {
    fn card(&self) -> Card {
        match self {
            Facet::Entity { .. } => Card::Required,
            Facet::Attribute { card, .. }
            | Facet::Property { card, .. }
            | Facet::Classification { card, .. }
            | Facet::Material { card, .. }
            | Facet::PartOf { card, .. } => *card,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Spec {
    pub name: String,
    pub description: Option<String>,
    pub versions: Vec<SchemaId>,
    pub applicability: Vec<Facet>,
    pub card: Card,
    pub requirements: Vec<Facet>,
    /// The specification as written, re-serialised (the export source).
    pub xml: String,
}

#[derive(Debug, Clone)]
pub(crate) struct Doc {
    pub title: String,
    pub description: Option<String>,
    pub info_xml: String,
    pub specs: Vec<Spec>,
}

// ── XSD structure ───────────────────────────────────────────────────────────

struct Check<'a> {
    errors: &'a mut Vec<String>,
}

impl Check<'_> {
    fn err(&mut self, at: &str, msg: impl std::fmt::Display) {
        self.errors.push(format!("{at}: {msg}"));
    }

    /// Only the listed (un-namespaced) attributes may appear; xmlns, xsi: and
    /// xml: attributes are always allowed.
    fn attrs(&mut self, el: &El, at: &str, allowed: &[&str]) {
        for (k, ns, _) in &el.attrs {
            if ns == XSI_NS || ns == XML_NS {
                continue;
            }
            if !ns.is_empty() || !allowed.contains(&k.as_str()) {
                self.err(
                    at,
                    format!("attribute `{k}` is not allowed on <{}>", el.name),
                );
            }
        }
    }

    /// Children must be IDS elements named in `order`, each at most as often
    /// as its bound allows, in that order (`repeat` lets the whole sequence
    /// repeat, as in `requirementsType`).
    fn sequence(&mut self, el: &El, at: &str, order: &[(&str, usize, usize)], repeat: bool) {
        let mut pos = 0usize;
        let mut counts = vec![0usize; order.len()];
        for c in &el.children {
            if c.ns != IDS_NS {
                self.err(at, format!("<{}> is not an IDS element", c.name));
                continue;
            }
            let Some(i) = order.iter().position(|(n, _, _)| *n == c.name) else {
                self.err(at, format!("<{}> is not allowed in <{}>", c.name, el.name));
                continue;
            };
            if i < pos && !repeat {
                self.err(at, format!("<{}> is out of order in <{}>", c.name, el.name));
            }
            pos = pos.max(i);
            counts[i] += 1;
        }
        for (i, (n, min, max)) in order.iter().enumerate() {
            if counts[i] < *min {
                self.err(at, format!("<{}> needs <{n}>", el.name));
            }
            if !repeat && counts[i] > *max {
                self.err(at, format!("<{}> allows at most {max} <{n}>", el.name));
            }
        }
        if !el.text.trim().is_empty() {
            self.err(at, format!("<{}> may not contain text", el.name));
        }
    }

    fn value(&mut self, el: Option<&El>, at: &str) -> Option<Value> {
        let el = el?;
        self.attrs(el, at, &[]);
        let kids: Vec<&El> = el.children.iter().collect();
        if kids.len() != 1 {
            self.err(
                at,
                format!(
                    "<{}> holds exactly one simpleValue or xs:restriction",
                    el.name
                ),
            );
            return None;
        }
        let k = kids[0];
        match (k.ns.as_str(), k.name.as_str()) {
            (IDS_NS, "simpleValue") => {
                self.attrs(k, at, &[]);
                if !k.children.is_empty() {
                    self.err(at, "<simpleValue> holds text only");
                }
                Some(Value::Simple(k.text.clone()))
            }
            (XS_NS, "restriction") => {
                self.attrs(k, at, &["base", "id"]);
                let mut r = Restriction {
                    base: k.attr("base").map(str::to_string),
                    ..Default::default()
                };
                for f in &k.children {
                    if f.ns != XS_NS {
                        self.err(at, format!("<{}> is not an XSD facet", f.name));
                        continue;
                    }
                    if f.name == "annotation" {
                        continue;
                    }
                    let Some(v) = f.attr("value").map(str::to_string) else {
                        self.err(at, format!("<xs:{}> needs a value", f.name));
                        continue;
                    };
                    let one = |slot: &mut Option<String>, c: &mut Check<'_>| {
                        if slot.replace(v.clone()).is_some() {
                            c.err(at, format!("<xs:{}> appears twice", f.name));
                        }
                    };
                    match f.name.as_str() {
                        "enumeration" => r.enums.push(v),
                        "pattern" => r.patterns.push(v),
                        "minInclusive" => one(&mut r.min_inclusive, self),
                        "maxInclusive" => one(&mut r.max_inclusive, self),
                        "minExclusive" => one(&mut r.min_exclusive, self),
                        "maxExclusive" => one(&mut r.max_exclusive, self),
                        "length" => one(&mut r.length, self),
                        "minLength" => one(&mut r.min_length, self),
                        "maxLength" => one(&mut r.max_length, self),
                        other => self.err(
                            at,
                            format!("<xs:{other}> is not supported in an IDS restriction"),
                        ),
                    }
                }
                Some(Value::Restriction(Box::new(r)))
            }
            _ => {
                self.err(
                    at,
                    format!("<{}> holds a simpleValue or xs:restriction", el.name),
                );
                None
            }
        }
    }

    fn card(&mut self, el: &El, at: &str, conditional: bool) -> Card {
        match el.attr("cardinality") {
            None | Some("required") => Card::Required,
            Some("prohibited") => Card::Prohibited,
            Some("optional") if conditional => Card::Optional,
            Some(other) => {
                self.err(
                    at,
                    format!("cardinality `{other}` is not allowed on <{}>", el.name),
                );
                Card::Required
            }
        }
    }

    fn entity(&mut self, el: &El, at: &str, in_requirements: bool) -> Option<Facet> {
        let allowed: &[&str] = if in_requirements {
            &["instructions"]
        } else {
            &[]
        };
        self.attrs(el, at, allowed);
        self.sequence(el, at, &[("name", 1, 1), ("predefinedType", 0, 1)], false);
        let name = self.value(el.child("name"), at)?;
        let predefined = self.value(el.child("predefinedType"), at);
        Some(Facet::Entity { name, predefined })
    }

    fn facet(&mut self, el: &El, at: &str, req: bool) -> Option<Facet> {
        let card_attrs: &[&str] = if req {
            &["cardinality", "instructions", "uri"]
        } else {
            &[]
        };
        match el.name.as_str() {
            "entity" => self.entity(el, at, req),
            "partOf" => {
                let allowed: &[&str] = if req {
                    &["relation", "cardinality", "instructions"]
                } else {
                    &["relation"]
                };
                self.attrs(el, at, allowed);
                self.sequence(el, at, &[("entity", 1, 1)], false);
                let relation = el.attr("relation").map(str::to_string);
                if let Some(r) = &relation {
                    if !matches!(
                        r.as_str(),
                        "IFCRELAGGREGATES"
                            | "IFCRELASSIGNSTOGROUP"
                            | "IFCRELCONTAINEDINSPATIALSTRUCTURE"
                            | "IFCRELNESTS"
                            | "IFCRELVOIDSELEMENT IFCRELFILLSELEMENT"
                    ) {
                        self.err(
                            at,
                            format!("partOf relation `{r}` is not one of the IDS relations"),
                        );
                    }
                }
                let card = if req {
                    self.card(el, at, false)
                } else {
                    Card::Required
                };
                let Facet::Entity { name, predefined } =
                    self.entity(el.child("entity")?, at, false)?
                else {
                    return None;
                };
                Some(Facet::PartOf {
                    relation,
                    entity: name,
                    predefined,
                    card,
                })
            }
            "classification" => {
                self.attrs(el, at, card_attrs);
                self.sequence(el, at, &[("value", 0, 1), ("system", 1, 1)], false);
                let card = if req {
                    self.card(el, at, true)
                } else {
                    Card::Required
                };
                Some(Facet::Classification {
                    value: self.value(el.child("value"), at),
                    system: self.value(el.child("system"), at),
                    card,
                })
            }
            "attribute" => {
                let allowed: &[&str] = if req {
                    &["cardinality", "instructions"]
                } else {
                    &[]
                };
                self.attrs(el, at, allowed);
                self.sequence(el, at, &[("name", 1, 1), ("value", 0, 1)], false);
                let card = if req {
                    self.card(el, at, true)
                } else {
                    Card::Required
                };
                Some(Facet::Attribute {
                    name: self.value(el.child("name"), at)?,
                    value: self.value(el.child("value"), at),
                    card,
                })
            }
            "property" => {
                let allowed: &[&str] = if req {
                    &["dataType", "cardinality", "instructions", "uri"]
                } else {
                    &["dataType"]
                };
                self.attrs(el, at, allowed);
                self.sequence(
                    el,
                    at,
                    &[("propertySet", 1, 1), ("baseName", 1, 1), ("value", 0, 1)],
                    false,
                );
                let data_type = el.attr("dataType").map(str::to_string);
                if let Some(d) = &data_type {
                    if d.is_empty() || !d.bytes().all(|b| b.is_ascii_uppercase()) {
                        self.err(
                            at,
                            format!("dataType `{d}` must be an upper-case IFC type name"),
                        );
                    }
                }
                let card = if req {
                    self.card(el, at, true)
                } else {
                    Card::Required
                };
                Some(Facet::Property {
                    pset: self.value(el.child("propertySet"), at)?,
                    name: self.value(el.child("baseName"), at)?,
                    data_type,
                    value: self.value(el.child("value"), at),
                    card,
                })
            }
            "material" => {
                self.attrs(el, at, card_attrs);
                self.sequence(el, at, &[("value", 0, 1)], false);
                let card = if req {
                    self.card(el, at, true)
                } else {
                    Card::Required
                };
                Some(Facet::Material {
                    value: self.value(el.child("value"), at),
                    card,
                })
            }
            other => {
                self.err(at, format!("<{other}> is not an IDS facet"));
                None
            }
        }
    }
}

/// Validate the document against the IDS 1.0 XSD and build the model.
pub(crate) fn read(root: &El) -> Result<Doc, Vec<String>> {
    let mut errors = Vec::new();
    if root.name != "ids" || root.ns != IDS_NS {
        return Err(vec![format!(
            "the root element is <{}>{}, not <ids> in the {IDS_NS} namespace",
            root.name,
            if root.ns.is_empty() {
                String::new()
            } else {
                format!(" ({})", root.ns)
            }
        )]);
    }
    let mut c = Check {
        errors: &mut errors,
    };
    c.attrs(root, "ids", &[]);
    c.sequence(
        root,
        "ids",
        &[("info", 1, 1), ("specifications", 1, 1)],
        false,
    );
    let info = root.child("info");
    let mut title = String::new();
    let mut description = None;
    let mut info_xml = String::new();
    if let Some(info) = info {
        c.attrs(info, "info", &[]);
        c.sequence(
            info,
            "info",
            &[
                ("title", 1, 1),
                ("copyright", 0, 1),
                ("version", 0, 1),
                ("description", 0, 1),
                ("author", 0, 1),
                ("date", 0, 1),
                ("purpose", 0, 1),
                ("milestone", 0, 1),
            ],
            false,
        );
        title = info
            .child("title")
            .map(|t| t.text.trim().to_string())
            .unwrap_or_default();
        description = info
            .child("description")
            .map(|t| t.text.trim().to_string())
            .filter(|t| !t.is_empty());
        if let Some(a) = info.child("author") {
            let v = a.text.trim();
            let ok = v.split_once('@').is_some_and(|(user, host)| {
                !user.is_empty()
                    && host
                        .split_once('.')
                        .is_some_and(|(h, rest)| !h.is_empty() && !rest.is_empty())
            });
            if !ok {
                c.err("info", format!("author `{v}` is not an e-mail address"));
            }
        }
        if let Some(d) = info.child("date") {
            if !XsBase::Date.lexically_valid(d.text.trim()) {
                c.err(
                    "info",
                    format!("date `{}` is not an xs:date", d.text.trim()),
                );
            }
        }
        info.to_xml(&mut info_xml, 0, true);
    }
    let mut specs = Vec::new();
    if let Some(list) = root.child("specifications") {
        c.attrs(list, "specifications", &[]);
        c.sequence(
            list,
            "specifications",
            &[("specification", 1, usize::MAX)],
            false,
        );
        for (i, s) in list.children_named("specification").enumerate() {
            let at = format!(
                "specification {} ({})",
                i + 1,
                s.attr("name").unwrap_or("unnamed")
            );
            c.attrs(
                s,
                &at,
                &[
                    "name",
                    "ifcVersion",
                    "identifier",
                    "description",
                    "instructions",
                ],
            );
            c.sequence(
                s,
                &at,
                &[("applicability", 1, 1), ("requirements", 0, 1)],
                false,
            );
            let Some(name) = s.attr("name").map(str::to_string) else {
                c.err(&at, "the name attribute is required");
                continue;
            };
            let mut versions = Vec::new();
            match s.attr("ifcVersion") {
                None => c.err(&at, "the ifcVersion attribute is required"),
                Some(v) => {
                    for tok in v.split_whitespace() {
                        match SchemaId::from_ids(tok) {
                            Some(id) if !versions.contains(&id) => versions.push(id),
                            Some(_) => {}
                            None => c.err(
                                &at,
                                format!("ifcVersion `{tok}` is not IFC2X3, IFC4 or IFC4X3_ADD2"),
                            ),
                        }
                    }
                    if versions.is_empty() && v.split_whitespace().next().is_none() {
                        c.err(&at, "ifcVersion is empty");
                    }
                }
            }
            let mut applicability = Vec::new();
            let mut card = Card::Required;
            if let Some(a) = s.child("applicability") {
                c.attrs(a, &at, &["minOccurs", "maxOccurs"]);
                c.sequence(
                    a,
                    &at,
                    &[
                        ("entity", 0, 1),
                        ("partOf", 0, usize::MAX),
                        ("classification", 0, usize::MAX),
                        ("attribute", 0, usize::MAX),
                        ("property", 0, usize::MAX),
                        ("material", 0, usize::MAX),
                    ],
                    false,
                );
                let min = a.attr("minOccurs").unwrap_or("1");
                let max = a.attr("maxOccurs").unwrap_or("1");
                if min.parse::<u64>().is_err() {
                    c.err(
                        &at,
                        format!("minOccurs `{min}` is not a non-negative integer"),
                    );
                }
                if max != "unbounded" && max.parse::<u64>().is_err() {
                    c.err(
                        &at,
                        format!("maxOccurs `{max}` is not a non-negative integer or `unbounded`"),
                    );
                }
                card = if max == "0" {
                    Card::Prohibited
                } else if min == "0" {
                    Card::Optional
                } else {
                    Card::Required
                };
                for f in &a.children {
                    if f.ns == IDS_NS {
                        if let Some(facet) = c.facet(f, &at, false) {
                            applicability.push(facet);
                        }
                    }
                }
            }
            let mut requirements = Vec::new();
            if let Some(r) = s.child("requirements") {
                c.attrs(r, &at, &["description"]);
                c.sequence(
                    r,
                    &at,
                    &[
                        ("entity", 0, usize::MAX),
                        ("partOf", 0, usize::MAX),
                        ("classification", 0, usize::MAX),
                        ("attribute", 0, usize::MAX),
                        ("property", 0, usize::MAX),
                        ("material", 0, usize::MAX),
                    ],
                    true,
                );
                for f in &r.children {
                    if f.ns == IDS_NS {
                        if let Some(facet) = c.facet(f, &at, true) {
                            requirements.push(facet);
                        }
                    }
                }
            }
            let mut xml = String::new();
            s.to_xml(&mut xml, 0, true);
            specs.push(Spec {
                name,
                description: s.attr("description").map(str::to_string),
                versions,
                applicability,
                card,
                requirements,
                xml,
            });
        }
    }
    if errors.is_empty() {
        Ok(Doc {
            title,
            description,
            info_xml,
            specs,
        })
    } else {
        Err(errors)
    }
}

// ── matching names in Rust (for expansion and the audit) ───────────────────

/// Does `s` satisfy the value as a string (simple value, enumeration,
/// pattern, lengths)? Used to expand entity, attribute and predefined-type
/// names against the schema tables.
fn str_matches(v: &Value, s: &str) -> bool {
    match v {
        Value::Simple(x) => x == s,
        Value::Restriction(r) => {
            if !r.enums.is_empty() && !r.enums.iter().any(|e| e == s) {
                return false;
            }
            if !r.patterns.is_empty()
                && !r.patterns.iter().any(|p| {
                    xsd_regex::to_sparql(p)
                        .ok()
                        .and_then(|re| regex::Regex::new(&re).ok())
                        .is_some_and(|re| re.is_match(s))
                })
            {
                return false;
            }
            let len = s.chars().count();
            let n = |o: &Option<String>| o.as_deref().and_then(|v| v.trim().parse::<usize>().ok());
            if n(&r.length).is_some_and(|l| len != l)
                || n(&r.min_length).is_some_and(|l| len < l)
                || n(&r.max_length).is_some_and(|l| len > l)
            {
                return false;
            }
            true
        }
    }
}

/// Every candidate the value admits.
fn expand<'a>(v: &Value, candidates: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut out: Vec<String> = candidates
        .into_iter()
        .filter(|c| str_matches(v, c))
        .map(str::to_string)
        .collect();
    out.sort();
    out.dedup();
    out
}

fn describe_value(v: &Value) -> String {
    match v {
        Value::Simple(s) => format!("\"{s}\""),
        Value::Restriction(r) => {
            let mut parts = Vec::new();
            if !r.enums.is_empty() {
                parts.push(format!("one of {}", r.enums.join(", ")));
            }
            if !r.patterns.is_empty() {
                parts.push(format!("matching {}", r.patterns.join(" or ")));
            }
            for (label, b) in [
                (">=", &r.min_inclusive),
                ("<=", &r.max_inclusive),
                (">", &r.min_exclusive),
                ("<", &r.max_exclusive),
            ] {
                if let Some(b) = b {
                    parts.push(format!("{label} {b}"));
                }
            }
            if let Some(l) = &r.length {
                parts.push(format!("{l} characters"));
            }
            if let Some(l) = &r.min_length {
                parts.push(format!("at least {l} characters"));
            }
            if let Some(l) = &r.max_length {
                parts.push(format!("at most {l} characters"));
            }
            if parts.is_empty() {
                "any value".into()
            } else {
                parts.join(", ")
            }
        }
    }
}

// ── schema helpers ──────────────────────────────────────────────────────────

/// Entity names an IDS entity facet may name in a schema: the schema's
/// entities, plus (IFC2X3) the names of the occurrence/type mapping table.
fn entity_candidates(s: SchemaId) -> Vec<String> {
    let mut v: Vec<String> = s.schema().entity_names().map(str::to_string).collect();
    if s == SchemaId::Ifc2x3 {
        v.extend(schema::ifc2x3_type_map().iter().map(|(n, _, _)| n.clone()));
    }
    v.sort();
    v.dedup();
    v
}

/// Upper-case names of the non-abstract entities (and IFC2X3 mapped names,
/// with the occurrence entity each stands for, so the attribute filters —
/// which know only schema entities — meet them).
fn concrete(s: SchemaId, names: &[String]) -> BTreeSet<String> {
    let sch = s.schema();
    let mut out: BTreeSet<String> = names
        .iter()
        .filter(|n| sch.entity(n).is_none_or(|e| !e.is_abstract))
        .cloned()
        .collect();
    if s == SchemaId::Ifc2x3 {
        for (alias, occurrence, _) in schema::ifc2x3_type_map() {
            if out.contains(alias) {
                out.insert(occurrence.clone());
            }
        }
    }
    out
}

fn all_concrete(s: SchemaId) -> BTreeSet<String> {
    concrete(s, &entity_candidates(s))
}

/// Predefined-type values a class (or mapped name) can carry in a schema:
/// its PredefinedType enumeration, or (IFC2X3) its type object's.
fn predefined_values(s: SchemaId, class: &str) -> Option<Vec<String>> {
    let sch = s.schema();
    let enum_of = |e: &str| -> Option<Vec<String>> {
        let (_, a) = sch.predefined_type_attr(e)?;
        sch.enum_values(&a.ty.to_ascii_uppercase())
            .map(|v| v.to_vec())
    };
    if let Some(v) = enum_of(class) {
        return Some(v);
    }
    if s == SchemaId::Ifc2x3 {
        if let Some((_, _, t)) = schema::ifc2x3_type_map()
            .iter()
            .find(|(n, _, _)| n == class)
        {
            return enum_of(t);
        }
        return enum_of(&format!("{class}TYPE"));
    }
    None
}

/// The XSD base an attribute's declared type compares by.
fn attr_base(s: SchemaId, ty: &str) -> Option<XsBase> {
    let sch = s.schema();
    let upper = ty.to_ascii_uppercase();
    if let Some(d) = schema::ids_datatype(&upper) {
        if d.base.is_some() {
            return d.base;
        }
    }
    match sch.attr_kind(ty) {
        AttrKind::Primitive(Primitive::String) => Some(XsBase::String),
        AttrKind::Primitive(Primitive::Real | Primitive::Number) => Some(XsBase::Double),
        AttrKind::Primitive(Primitive::Integer) => Some(XsBase::Integer),
        AttrKind::Primitive(Primitive::Boolean | Primitive::Logical) => Some(XsBase::Boolean),
        AttrKind::Enum => Some(XsBase::String),
        _ => None,
    }
}

/// Explicit attributes over the schema: (entity, attribute name, declared type).
fn schema_attributes(s: SchemaId) -> Vec<(String, String, String)> {
    let sch = s.schema();
    let mut out = Vec::new();
    for e in sch.entity_names() {
        for (a, derived) in sch.attributes(e) {
            if !derived {
                out.push((e.to_string(), a.name.clone(), a.ty.clone()));
            }
        }
    }
    out
}

// ── audit ───────────────────────────────────────────────────────────────────

/// The IDS audit rules (after the buildingSMART IDS Audit Tool): names must
/// exist in the schemas, values must suit the types they constrain, and the
/// applicability and requirements must be satisfiable together.
pub(crate) fn audit(doc: &Doc) -> Vec<String> {
    let mut problems = Vec::new();
    for spec in &doc.specs {
        let at = format!("specification `{}`", spec.name);
        if spec.applicability.is_empty() {
            problems.push(format!("{at}: the applicability needs at least one facet"));
        }
        if spec.card == Card::Prohibited && !spec.requirements.is_empty() {
            problems.push(format!(
                "{at}: requirements are not allowed when the applicability is prohibited"
            ));
        }
        if spec.card == Card::Optional && spec.requirements.is_empty() {
            problems.push(format!(
                "{at}: an optional specification needs requirements"
            ));
        }
        for &s in &spec.versions {
            let mut app: Option<BTreeSet<String>> = None;
            let mut req: Option<BTreeSet<String>> = None;
            for (facets, filter) in [
                (&spec.applicability, &mut app),
                (&spec.requirements, &mut req),
            ] {
                for f in facets.iter() {
                    if let Some(set) = audit_facet(f, s, &at, &mut problems) {
                        *filter = Some(match filter.take() {
                            Some(cur) => cur.intersection(&set).cloned().collect(),
                            None => set,
                        });
                    }
                }
            }
            if let (Some(a), Some(r)) = (&app, &req) {
                if !a.is_empty() && !r.is_empty() && a.is_disjoint(r) {
                    problems.push(format!(
                        "{at} ({}): no entity can meet both the applicability and the requirements",
                        s.ids_name()
                    ));
                }
            }
            if let Some(a) = &app {
                if a.is_empty() {
                    problems.push(format!(
                        "{at} ({}): no entity can meet the applicability",
                        s.ids_name()
                    ));
                }
            }
        }
    }
    problems.sort();
    problems.dedup();
    problems
}

/// Check one value's restriction against the base type it constrains.
fn audit_value(v: &Value, base: Option<XsBase>, what: &str, at: &str, problems: &mut Vec<String>) {
    let check = |raw: &str, b: XsBase, problems: &mut Vec<String>| {
        if !b.lexically_valid(raw.trim()) {
            problems.push(format!(
                "{at}: {what} value `{raw}` is not a valid {}",
                b.as_str()
            ));
        }
    };
    match v {
        Value::Simple(s) => {
            if let Some(b) = base {
                check(s, b, problems);
            }
        }
        Value::Restriction(r) => {
            let declared = r.base.as_deref();
            if let Some(d) = declared {
                if XsBase::parse(d).is_none() {
                    problems.push(format!(
                        "{at}: {what} restriction base `{d}` is not an IDS base type"
                    ));
                }
            }
            if let Some(b) = base {
                match declared.and_then(XsBase::parse) {
                    Some(d) if d != b => problems.push(format!(
                        "{at}: {what} restriction base is `{}` but the value is a {}",
                        d.as_str(),
                        b.as_str()
                    )),
                    None => problems.push(format!(
                        "{at}: {what} restriction needs base=\"{}\"",
                        b.as_str()
                    )),
                    _ => {}
                }
            }
            let b = declared.and_then(XsBase::parse).or(base);
            for p in &r.patterns {
                if let Err(e) = xsd_regex::to_sparql(p) {
                    problems.push(format!("{at}: {what}: {e}"));
                }
            }
            if let Some(b) = b {
                for e in &r.enums {
                    check(e, b, problems);
                }
                for bound in [
                    &r.min_inclusive,
                    &r.max_inclusive,
                    &r.min_exclusive,
                    &r.max_exclusive,
                ]
                .into_iter()
                .flatten()
                {
                    check(bound, b, problems);
                }
            }
            for l in [&r.length, &r.min_length, &r.max_length]
                .into_iter()
                .flatten()
            {
                check(l, XsBase::Integer, problems);
            }
        }
    }
}

/// Audit one facet in one schema; returns the classes it can apply to (the
/// type filter), when it narrows them.
fn audit_facet(
    f: &Facet,
    s: SchemaId,
    at: &str,
    problems: &mut Vec<String>,
) -> Option<BTreeSet<String>> {
    let sname = s.ids_name();
    match f {
        Facet::Entity { name, predefined }
        | Facet::PartOf {
            entity: name,
            predefined,
            ..
        } => {
            let candidates = entity_candidates(s);
            let matched = match name {
                Value::Simple(n) => {
                    if candidates.binary_search(n).is_err() {
                        problems.push(format!(
                            "{at}: entity `{n}` does not exist in {sname} (entity names are upper case)"
                        ));
                        Vec::new()
                    } else {
                        vec![n.clone()]
                    }
                }
                Value::Restriction(r) => {
                    for e in &r.enums {
                        if candidates.binary_search(e).is_err() {
                            problems.push(format!("{at}: entity `{e}` does not exist in {sname}"));
                        }
                    }
                    let m = expand(name, candidates.iter().map(String::as_str));
                    if m.is_empty() {
                        problems.push(format!(
                            "{at}: the entity restriction matches no {sname} entity"
                        ));
                    }
                    m
                }
            };
            if let Some(pt) = predefined {
                let mut possible: Option<Vec<String>> = None;
                for c in &matched {
                    match predefined_values(s, c) {
                        Some(vals) => {
                            possible = Some(match possible.take() {
                                Some(cur) => cur.into_iter().filter(|v| vals.contains(v)).collect(),
                                None => vals,
                            })
                        }
                        None => {
                            possible = Some(Vec::new());
                        }
                    }
                }
                match possible {
                    Some(p) if p.contains(&"USERDEFINED".to_string()) => {}
                    Some(p) if !p.is_empty() => {
                        if expand(pt, p.iter().map(String::as_str)).is_empty() {
                            problems.push(format!(
                                "{at}: predefinedType {} is not a predefined type of the entity in {sname}",
                                describe_value(pt)
                            ));
                        }
                    }
                    _ if !matched.is_empty() => problems.push(format!(
                        "{at}: the entity has no predefined type in {sname}"
                    )),
                    _ => {}
                }
            }
            if matches!(f, Facet::PartOf { .. }) {
                return None;
            }
            Some(concrete(s, &matched))
        }
        Facet::Attribute { name, value, card } => {
            if *card == Card::Optional && value.is_none() {
                problems.push(format!("{at}: an optional attribute facet needs a value"));
            }
            let attrs = schema_attributes(s);
            let sch = s.schema();
            // With a value only value-typed attributes count: an entity
            // reference, an aggregate or a select has no value to compare.
            let candidates: Vec<&(String, String, String)> = attrs
                .iter()
                .filter(|(_, _, ty)| value.is_none() || attr_base(s, ty).is_some())
                .collect();
            let names: BTreeSet<&str> = candidates.iter().map(|(_, n, _)| n.as_str()).collect();
            let matched: Vec<String> = match name {
                Value::Simple(n) => {
                    if names.contains(n.as_str()) {
                        vec![n.clone()]
                    } else {
                        let why = if value.is_some() && attrs.iter().any(|(_, a, _)| a == n) {
                            "is not a value attribute (an entity reference, a list or a select), so it has no value to check"
                        } else {
                            "is not an explicit attribute of any entity (derived and inverse attributes cannot be checked)"
                        };
                        problems.push(format!("{at}: attribute `{n}` {why} in {sname}"));
                        Vec::new()
                    }
                }
                Value::Restriction(_) => {
                    let m = expand(name, names.iter().copied());
                    if m.is_empty() {
                        problems.push(format!(
                            "{at}: the attribute name restriction matches no attribute in {sname}"
                        ));
                    }
                    m
                }
            };
            if let Some(v) = value {
                let bases: BTreeSet<&'static str> = candidates
                    .iter()
                    .filter(|(_, n, _)| matched.contains(n))
                    .filter_map(|(_, _, ty)| attr_base(s, ty).map(XsBase::as_str))
                    .collect();
                if bases.len() == 1 {
                    let b = XsBase::parse(bases.iter().next().unwrap());
                    audit_value(v, b, "attribute", at, problems);
                } else {
                    audit_value(v, None, "attribute", at, problems);
                }
            }
            let classes: BTreeSet<String> = attrs
                .iter()
                .filter(|(_, n, _)| matched.contains(n))
                .map(|(e, _, _)| e.clone())
                .filter(|e| sch.entity(e).is_some_and(|x| !x.is_abstract))
                .collect();
            Some(classes)
        }
        Facet::Property {
            data_type,
            value,
            card,
            ..
        } => {
            if value.is_some() && data_type.is_none() {
                problems.push(format!("{at}: a property value requires a dataType"));
            }
            if *card == Card::Optional && data_type.is_none() {
                problems.push(format!("{at}: an optional property facet needs a dataType"));
            }
            if *card == Card::Prohibited && data_type.is_some() {
                problems.push(format!(
                    "{at}: a prohibited property facet cannot carry a dataType"
                ));
            }
            if let Some(dt) = data_type {
                match schema::ids_datatype(dt) {
                    Some(d) if d.schemas.contains(&s) => {
                        if let Some(v) = value {
                            audit_value(v, d.base, "property", at, problems);
                        }
                    }
                    Some(_) => {
                        problems.push(format!("{at}: dataType {dt} does not exist in {sname}"))
                    }
                    None => problems.push(format!("{at}: dataType {dt} is not an IFC data type")),
                }
            }
            None
        }
        Facet::Classification { system, value, .. } => {
            if let Some(v) = system {
                audit_value(
                    v,
                    Some(XsBase::String),
                    "classification system",
                    at,
                    problems,
                );
            }
            if let Some(v) = value {
                audit_value(
                    v,
                    Some(XsBase::String),
                    "classification value",
                    at,
                    problems,
                );
            }
            None
        }
        Facet::Material { value, .. } => {
            if let Some(v) = value {
                audit_value(v, Some(XsBase::String), "material", at, problems);
            }
            None
        }
    }
}

// ── SPARQL generation ───────────────────────────────────────────────────────

const XSD: &str = "http://www.w3.org/2001/XMLSchema#";
const TOLERANCE: f64 = 1.0e-6;

fn p(local: &str) -> String {
    format!("<{P}{local}>")
}

fn sparql_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

fn sparql_double(v: f64) -> String {
    format!("\"{v:?}\"^^<{XSD}double>")
}

/// How a facet compares values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Base {
    String,
    Double,
    Integer,
    Boolean,
}

impl Base {
    fn of(b: Option<XsBase>) -> Base {
        match b {
            Some(XsBase::Double) => Base::Double,
            Some(XsBase::Integer) => Base::Integer,
            Some(XsBase::Boolean) => Base::Boolean,
            _ => Base::String,
        }
    }
}

/// `var` equals the IDS value `raw` (tolerance for doubles).
/// The values equal to `v` under the IDS tolerance. The buildingSMART corpus
/// accepts a value exactly on `v ± (|v|·1e-6 + 1e-6)`, so the range is
/// closed, widened by a few ulps so the bound computed in binary floating
/// point does not reject the decimal boundary itself.
pub(crate) fn tolerance_range(v: f64) -> (f64, f64) {
    let lo = v - v.abs() * TOLERANCE - TOLERANCE;
    let hi = v + v.abs() * TOLERANCE + TOLERANCE;
    let slack = |b: f64| b.abs() * 4.0 * f64::EPSILON;
    (lo - slack(lo), hi + slack(hi))
}

fn eq_expr(var: &str, raw: &str, base: Base) -> String {
    let s = raw.trim();
    match base {
        Base::Double => match s.parse::<f64>() {
            Ok(v) => {
                let (lo, hi) = tolerance_range(v);
                format!(
                    "(isNumeric({var}) && {var} >= {} && {var} <= {})",
                    sparql_double(lo),
                    sparql_double(hi)
                )
            }
            Err(_) => "false".into(),
        },
        Base::Integer => match s.parse::<i64>() {
            Ok(n) => format!("(isNumeric({var}) && {var} = {n})"),
            Err(_) => "false".into(),
        },
        Base::Boolean => {
            let b = matches!(s, "true" | "1");
            format!("(datatype({var}) = <{XSD}boolean> && {var} = {b})")
        }
        // IFCLOGICAL compares as a string in the IDS table, and the lift writes
        // T/F as booleans: their lexical forms are "true" / "false".
        Base::String => format!(
            "(isLiteral({var}) && (datatype({var}) = <{XSD}string> || datatype({var}) = <{XSD}boolean>) && STR({var}) = {})",
            sparql_str(raw)
        ),
    }
}

fn cmp_expr(var: &str, op: &str, raw: &str, base: Base) -> String {
    let s = raw.trim();
    match base {
        Base::Double => match s.parse::<f64>() {
            Ok(v) => format!("(isNumeric({var}) && {var} {op} {})", sparql_double(v)),
            Err(_) => "false".into(),
        },
        Base::Integer => match s.parse::<i64>() {
            Ok(n) => format!("(isNumeric({var}) && {var} {op} {n})"),
            Err(_) => "false".into(),
        },
        Base::Boolean => "false".into(),
        Base::String => format!("(isLiteral({var}) && STR({var}) {op} {})", sparql_str(raw)),
    }
}

/// `var` satisfies the value.
fn value_expr(var: &str, v: &Value, base: Base) -> String {
    match v {
        Value::Simple(s) => eq_expr(var, s, base),
        Value::Restriction(r) => {
            let base = r
                .base
                .as_deref()
                .and_then(XsBase::parse)
                .map(|b| Base::of(Some(b)))
                .unwrap_or(base);
            let mut parts = Vec::new();
            if !r.enums.is_empty() {
                parts.push(format!(
                    "({})",
                    r.enums
                        .iter()
                        .map(|e| eq_expr(var, e, base))
                        .collect::<Vec<_>>()
                        .join(" || ")
                ));
            }
            if !r.patterns.is_empty() {
                parts.push(format!(
                    "({})",
                    r.patterns
                        .iter()
                        .map(|pat| match xsd_regex::to_sparql(pat) {
                            Ok(re) => format!(
                                "(isLiteral({var}) && REGEX(STR({var}), {}))",
                                sparql_str(&re)
                            ),
                            Err(_) => "false".into(),
                        })
                        .collect::<Vec<_>>()
                        .join(" || ")
                ));
            }
            for (op, b) in [
                (">=", &r.min_inclusive),
                ("<=", &r.max_inclusive),
                (">", &r.min_exclusive),
                ("<", &r.max_exclusive),
            ] {
                if let Some(b) = b {
                    parts.push(cmp_expr(var, op, b, base));
                }
            }
            for (op, l) in [
                ("=", &r.length),
                (">=", &r.min_length),
                ("<=", &r.max_length),
            ] {
                if let Some(n) = l.as_deref().and_then(|v| v.trim().parse::<u64>().ok()) {
                    parts.push(format!("(isLiteral({var}) && STRLEN(STR({var})) {op} {n})"));
                }
            }
            if parts.is_empty() {
                "true".into()
            } else {
                parts.join(" && ")
            }
        }
    }
}

/// Fresh variable names per facet, so nested patterns never collide.
struct Vars(usize);

impl Vars {
    fn next(&mut self, stem: &str) -> String {
        self.0 += 1;
        format!("?{stem}{}", self.0)
    }
}

/// The schema-qualified class IRIs an entity value admits in the
/// specification's schemas.
/// Every schema a model can be in. A specification's `ifcVersion` decides
/// which names the audit accepts; like the reference implementations (and
/// as the buildingSMART corpus requires), the checks then apply to a model
/// of any schema, so targets name the classes in every schema's namespace.
const MODEL_SCHEMAS: [SchemaId; 3] = [SchemaId::Ifc2x3, SchemaId::Ifc4, SchemaId::Ifc4x3];

fn class_iris(v: &Value, versions: &[SchemaId]) -> Vec<String> {
    let mut names = BTreeSet::new();
    for &s in versions {
        for n in expand(v, entity_candidates(s).iter().map(String::as_str)) {
            if s.schema().entity(&n).is_some_and(|e| e.is_abstract) {
                continue;
            }
            names.insert(n);
        }
    }
    let mut out = Vec::new();
    for s in MODEL_SCHEMAS {
        let ns = class_ns(s);
        out.extend(names.iter().map(|n| format!("<{ns}{n}>")));
    }
    out.sort();
    out.dedup();
    out
}

fn all_class_iris() -> Vec<String> {
    let mut out = Vec::new();
    for s in MODEL_SCHEMAS {
        let ns = class_ns(s);
        out.extend(all_concrete(s).into_iter().map(|n| format!("<{ns}{n}>")));
    }
    out
}

/// `var` (a class IRI) is one the entity value admits in the specification's
/// schemas. A short IN list for simple values and enumerations; a test on
/// the schema namespace and the local name otherwise, because an IN list of
/// every entity the pattern matches can be hundreds of IRIs long, and the
/// query engine recurses on it.
fn class_cond(var: &str, v: &Value, versions: &[SchemaId]) -> String {
    let small = match v {
        Value::Simple(_) => true,
        Value::Restriction(r) => {
            !r.enums.is_empty()
                && r.patterns.is_empty()
                && r.length.is_none()
                && r.min_length.is_none()
                && r.max_length.is_none()
        }
    };
    if small {
        return in_list(var, &class_iris(v, versions));
    }
    let ns = MODEL_SCHEMAS
        .iter()
        .map(|s| format!("STRSTARTS(STR({var}), {})", sparql_str(&class_ns(*s))))
        .collect::<Vec<_>>()
        .join(" || ");
    format!(
        "({ns}) && {}",
        value_expr(&format!("STRAFTER(STR({var}), \"#\")"), v, Base::String)
    )
}

fn in_list(var: &str, iris: &[String]) -> String {
    if iris.is_empty() {
        "false".into()
    } else {
        format!("{var} IN ({})", iris.join(", "))
    }
}

/// The attribute names a facet's name admits in the specification's schemas,
/// with the base each compares by (per schema, deduplicated).
fn attribute_targets(name: &Value, versions: &[SchemaId]) -> Vec<(String, Option<XsBase>)> {
    let mut out: Vec<(String, Option<XsBase>)> = Vec::new();
    for &s in versions {
        for (_, n, ty) in schema_attributes(s) {
            if str_matches(name, &n) {
                let b = attr_base(s, &ty);
                if !out.iter().any(|(x, xb)| *x == n && *xb == b) {
                    out.push((n, b));
                }
            }
        }
    }
    out
}

/// `(MATCH, PRESENT)` for one facet over subject `x`: MATCH holds when the
/// facet is satisfied with required semantics; PRESENT when the thing it is
/// about exists at all (for `optional`).
fn facet_conditions(
    f: &Facet,
    x: &str,
    versions: &[SchemaId],
    vars: &mut Vars,
) -> (String, String) {
    match f {
        Facet::Entity { name, predefined } => {
            let c = vars.next("class");
            let mut m = format!(
                "EXISTS {{ {x} a {c} . FILTER({}) }}",
                class_cond(&c, name, versions)
            );
            if let Some(pt) = predefined {
                let v = vars.next("pt");
                m = format!(
                    "({m} && EXISTS {{ {x} {} {v} . FILTER({}) }})",
                    p("predefinedType"),
                    value_expr(&v, pt, Base::String)
                );
            }
            (m.clone(), m)
        }
        Facet::Attribute { name, value, .. } => {
            let targets = attribute_targets(name, versions);
            if targets.is_empty() {
                return ("false".into(), "false".into());
            }
            let mut names: Vec<&str> = targets.iter().map(|(n, _)| n.as_str()).collect();
            names.dedup();
            let empty = format!("<{P}Empty>");
            let mut matches = Vec::new();
            for (n, b) in &targets {
                let v = vars.next("a");
                let cond = match value {
                    Some(val) => format!(
                        "!sameTerm({v}, {empty}) && {}",
                        value_expr(&v, val, Base::of(*b))
                    ),
                    None => format!("!sameTerm({v}, {empty})"),
                };
                matches.push(format!(
                    "EXISTS {{ {x} <{ATTR_NS}{n}> {v} . FILTER({cond}) }}"
                ));
            }
            let present: Vec<String> = names
                .iter()
                .map(|n| {
                    let v = vars.next("a");
                    format!("EXISTS {{ {x} <{ATTR_NS}{n}> {v} }}")
                })
                .collect();
            (
                format!("({})", matches.join(" || ")),
                format!("({})", present.join(" || ")),
            )
        }
        Facet::Property {
            pset,
            name,
            data_type,
            value,
            ..
        } => {
            let ps = vars.next("ps");
            let psn = vars.next("psn");
            let q = vars.next("q");
            let pn = vars.next("pn");
            let v = vars.next("v");
            let ps_cond = value_expr(&psn, pset, Base::String);
            let pn_cond = value_expr(&pn, name, Base::String);
            let base = Base::of(
                data_type
                    .as_deref()
                    .and_then(schema::ids_datatype)
                    .and_then(|d| d.base),
            );
            let dt = match data_type {
                Some(d) => format!("{q} {} {} . ", p("dataType"), sparql_str(d)),
                None => String::new(),
            };
            let val = match value {
                Some(val) => format!("FILTER({})", value_expr(&v, val, base)),
                None => String::new(),
            };
            let pset_pat = format!(
                "{x} {} {ps} . {ps} {} {psn} . FILTER({ps_cond})",
                p("pset"),
                p("name")
            );
            let prop_pat = format!(
                "{ps} {} {q} . {q} {} {pn} . FILTER({pn_cond})",
                p("property"),
                p("name")
            );
            let m = format!(
                "(EXISTS {{ {pset_pat} }} && NOT EXISTS {{ {pset_pat} FILTER NOT EXISTS {{ {prop_pat} }} }} && NOT EXISTS {{ {pset_pat} {prop_pat} FILTER NOT EXISTS {{ {dt}{q} {} {v} . {val} }} }})",
                p("value")
            );
            let present = format!("EXISTS {{ {pset_pat} {prop_pat} }}");
            (m, present)
        }
        Facet::Classification { system, value, .. } => {
            let c = vars.next("c");
            let mut pat = format!("{x} {} {c} .", p("classification"));
            if let Some(sys) = system {
                let s = vars.next("sys");
                let _ = write!(
                    pat,
                    " {c} {} {s} . FILTER({})",
                    p("system"),
                    value_expr(&s, sys, Base::String)
                );
            }
            if let Some(val) = value {
                let r = vars.next("ref");
                let _ = write!(
                    pat,
                    " {c} {} {r} . FILTER({})",
                    p("reference"),
                    value_expr(&r, val, Base::String)
                );
            }
            let c2 = vars.next("c");
            (
                format!("EXISTS {{ {pat} }}"),
                format!("EXISTS {{ {x} {} {c2} }}", p("classification")),
            )
        }
        Facet::Material { value, .. } => {
            let m = vars.next("m");
            let present = format!("EXISTS {{ {x} {} {m} }}", p("material"));
            let matched = match value {
                Some(val) => {
                    let mv = vars.next("mv");
                    format!(
                        "EXISTS {{ {x} {} {m} . {m} {} {mv} . FILTER({}) }}",
                        p("material"),
                        p("materialValue"),
                        value_expr(&mv, val, Base::String)
                    )
                }
                None => present.clone(),
            };
            (matched, present)
        }
        Facet::PartOf {
            relation,
            entity,
            predefined,
            ..
        } => {
            let path = match relation.as_deref() {
                Some("IFCRELAGGREGATES") => format!("{}+", p("aggregatedIn")),
                Some("IFCRELNESTS") => format!("{}+", p("nestedIn")),
                Some("IFCRELCONTAINEDINSPATIALSTRUCTURE") => {
                    format!("{}*/{}", p("aggregatedIn"), p("containedIn"))
                }
                Some("IFCRELASSIGNSTOGROUP") => p("inGroup"),
                Some(_) => p("voidsFillsIn"),
                None => format!(
                    "({}|{}|{}|{}|{})+",
                    p("aggregatedIn"),
                    p("nestedIn"),
                    p("containedIn"),
                    p("inGroup"),
                    p("voidsFillsIn")
                ),
            };
            let w = vars.next("w");
            let wc = vars.next("wc");
            let mut pat = format!(
                "{x} {path} {w} . {w} a {wc} . FILTER({})",
                class_cond(&wc, entity, versions)
            );
            if let Some(pt) = predefined {
                let v = vars.next("wpt");
                let _ = write!(
                    pat,
                    " {w} {} {v} . FILTER({})",
                    p("predefinedType"),
                    value_expr(&v, pt, Base::String)
                );
            }
            let w2 = vars.next("w");
            (
                format!("EXISTS {{ {pat} }}"),
                format!("EXISTS {{ {x} {path} {w2} }}"),
            )
        }
    }
}

fn describe(f: &Facet) -> String {
    let card = match f.card() {
        Card::Required => "required",
        Card::Optional => "optional",
        Card::Prohibited => "prohibited",
    };
    let body = match f {
        Facet::Entity { name, predefined } => format!(
            "entity {}{}",
            describe_value(name),
            predefined
                .as_ref()
                .map(|p| format!(" with predefined type {}", describe_value(p)))
                .unwrap_or_default()
        ),
        Facet::Attribute { name, value, .. } => format!(
            "attribute {}{}",
            describe_value(name),
            value
                .as_ref()
                .map(|v| format!(" = {}", describe_value(v)))
                .unwrap_or_default()
        ),
        Facet::Property {
            pset,
            name,
            data_type,
            value,
            ..
        } => format!(
            "property {} . {}{}{}",
            describe_value(pset),
            describe_value(name),
            data_type
                .as_ref()
                .map(|d| format!(" [{d}]"))
                .unwrap_or_default(),
            value
                .as_ref()
                .map(|v| format!(" = {}", describe_value(v)))
                .unwrap_or_default()
        ),
        Facet::Classification { system, value, .. } => format!(
            "classification{}{}",
            system
                .as_ref()
                .map(|s| format!(" in system {}", describe_value(s)))
                .unwrap_or_default(),
            value
                .as_ref()
                .map(|v| format!(" with reference {}", describe_value(v)))
                .unwrap_or_default()
        ),
        Facet::Material { value, .. } => format!(
            "material{}",
            value
                .as_ref()
                .map(|v| format!(" {}", describe_value(v)))
                .unwrap_or_default()
        ),
        Facet::PartOf {
            relation,
            entity,
            predefined,
            ..
        } => format!(
            "part of {}{}{}",
            describe_value(entity),
            predefined
                .as_ref()
                .map(|p| format!(" with predefined type {}", describe_value(p)))
                .unwrap_or_default(),
            relation
                .as_ref()
                .map(|r| format!(" via {r}"))
                .unwrap_or_default()
        ),
    };
    format!("{body} ({card})")
}

// ── conversion ──────────────────────────────────────────────────────────────

const SHAPE_NS: &str = "urn:ids:";

fn ttl_str(s: &str) -> String {
    sparql_str(s)
}

/// A Turtle long string.
fn ttl_long(s: &str) -> String {
    format!(
        "\"\"\"{}\"\"\"",
        s.replace('\\', "\\\\").replace('"', "\\\"")
    )
}

/// The fingerprint export compares against: the targets and SPARQL of a
/// specification's shapes, in a canonical order.
pub(crate) fn fingerprint(targets: &[String], selects: &[String]) -> String {
    let mut t: Vec<&String> = targets.iter().collect();
    t.sort();
    let mut s: Vec<&String> = selects.iter().collect();
    s.sort();
    let mut h = Sha256::new();
    for x in t {
        h.update(x.as_bytes());
        h.update([0u8]);
    }
    h.update([1u8]);
    for x in s {
        h.update(x.as_bytes());
        h.update([0u8]);
    }
    hex::encode(h.finalize())
}

pub(crate) fn convert(doc: &Doc) -> ImportedShapes {
    let mut ttl = String::new();
    ttl.push_str("@prefix sh: <http://www.w3.org/ns/shacl#> .\n");
    ttl.push_str("@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n");
    ttl.push_str("@prefix dct: <http://purl.org/dc/terms/> .\n");
    ttl.push_str("@prefix ots: <https://opentriplestore.org/ns#> .\n");
    let _ = writeln!(ttl, "@prefix ifcids: <{P}> .\n");
    let title = if doc.title.is_empty() {
        "IDS import".to_string()
    } else {
        doc.title.clone()
    };
    let _ = writeln!(ttl, "<{SHAPE_NS}spec> dct:title {} ;", ttl_str(&title));
    if let Some(d) = &doc.description {
        let _ = writeln!(ttl, "    dct:description {} ;", ttl_str(d));
    }
    let _ = writeln!(ttl, "    ots:idsInfo {} ;", ttl_long(&doc.info_xml));
    let _ = writeln!(
        ttl,
        "    dct:conformsTo <https://standards.buildingsmart.org/IDS> .\n"
    );

    let mut warnings = Vec::new();
    let mut summaries = Vec::new();
    let mut shape_count = 0usize;
    for (i, spec) in doc.specs.iter().enumerate() {
        let n = i + 1;
        let shape = format!("{SHAPE_NS}spec{n}");
        let versions = &spec.versions;
        let mut vars = Vars(0);
        let entity = spec
            .applicability
            .iter()
            .find(|f| matches!(f, Facet::Entity { .. }));
        let targets = match entity {
            Some(Facet::Entity { name, .. }) => class_iris(name, versions),
            _ => all_class_iris(),
        };
        if entity.is_none() {
            warnings.push(format!(
                "specification `{}` names no entity, so it targets every entity of {}",
                spec.name,
                versions
                    .iter()
                    .map(|v| v.ids_name())
                    .collect::<Vec<_>>()
                    .join(" ")
            ));
        }
        // Applicability over `x`: every facet's MATCH, except that the
        // entity's classes are the targets (for $this) or an explicit class
        // test (for the existence query's ?x), so only its predefined type
        // is a condition here.
        let applicability = |x: &str, vars: &mut Vars| -> String {
            let mut parts = Vec::new();
            for f in &spec.applicability {
                match f {
                    Facet::Entity {
                        predefined: None, ..
                    } => {}
                    Facet::Entity {
                        predefined: Some(pt),
                        ..
                    } => {
                        let v = vars.next("pt");
                        parts.push(format!(
                            "EXISTS {{ {x} {} {v} . FILTER({}) }}",
                            p("predefinedType"),
                            value_expr(&v, pt, Base::String)
                        ));
                    }
                    other => parts.push(facet_conditions(other, x, versions, vars).0),
                }
            }
            parts.join(" && ")
        };
        let plain_targets: Vec<String> = targets
            .iter()
            .map(|t| t.trim_matches(['<', '>']).to_string())
            .collect();
        let mut selects: Vec<String> = Vec::new();
        let mut constraints: Vec<(String, String)> = Vec::new();
        let appl_this = applicability("$this", &mut vars);
        let appl_filter = if appl_this.is_empty() {
            String::new()
        } else {
            format!("FILTER({appl_this}) ")
        };
        if spec.card == Card::Prohibited {
            let q = format!("SELECT $this WHERE {{ {appl_filter}}}");
            constraints.push((
                format!(
                    "the prohibited specification `{}` applies to this entity",
                    spec.name
                ),
                q,
            ));
        } else {
            for f in &spec.requirements {
                let (m, present) = facet_conditions(f, "$this", versions, &mut vars);
                let violation = match f.card() {
                    Card::Required => format!("!({m})"),
                    Card::Prohibited => format!("({m})"),
                    Card::Optional => format!("({present}) && !({m})"),
                };
                let q = format!("SELECT $this WHERE {{ {appl_filter}FILTER({violation}) }}");
                constraints.push((
                    format!("{}: requirement not met — {}", spec.name, describe(f)),
                    q,
                ));
            }
        }
        let _ = writeln!(ttl, "<{shape}> a sh:NodeShape ;");
        let _ = writeln!(ttl, "    sh:name {} ;", ttl_str(&spec.name));
        if let Some(d) = &spec.description {
            let _ = writeln!(ttl, "    sh:description {} ;", ttl_str(d));
        }
        let version_list = versions
            .iter()
            .map(|v| v.ids_name())
            .collect::<Vec<_>>()
            .join(" ");
        let _ = writeln!(
            ttl,
            "    rdfs:comment {} ;",
            ttl_str(&format!("IDS specification {n} ({version_list})"))
        );
        let _ = writeln!(ttl, "    ots:idsSpecification {} ;", ttl_long(&spec.xml));
        for t in &targets {
            let _ = writeln!(ttl, "    sh:targetClass {t} ;");
        }
        for (msg, q) in &constraints {
            let _ = writeln!(
                ttl,
                "    sh:sparql [ a sh:SPARQLConstraint ;\n        sh:message {} ;\n        sh:select {} ] ;",
                ttl_str(msg),
                ttl_long(q)
            );
            selects.push(q.clone());
        }
        shape_count += 1;
        if spec.card == Card::Required {
            let x = "?x";
            let c = vars.next("class");
            let class_test = match entity {
                Some(Facet::Entity { name, .. }) => class_cond(&c, name, versions),
                _ => MODEL_SCHEMAS
                    .iter()
                    .map(|s| format!("STRSTARTS(STR({c}), {})", sparql_str(&class_ns(*s))))
                    .collect::<Vec<_>>()
                    .join(" || "),
            };
            let mut cond = format!("{x} a {c} . FILTER({class_test})");
            let appl_x = applicability(x, &mut vars);
            if !appl_x.is_empty() {
                let _ = write!(cond, " FILTER({appl_x})");
            }
            let q = format!("SELECT $this WHERE {{ FILTER NOT EXISTS {{ {cond} }} }}");
            let _ = writeln!(ttl, "    ots:idsExistenceShape <{shape}-exists> ;");
            selects.push(q.clone());
            let fp = fingerprint(&plain_targets, &selects);
            let _ = writeln!(ttl, "    ots:idsFingerprint {} .\n", ttl_str(&fp));
            let _ = writeln!(ttl, "<{shape}-exists> a sh:NodeShape ;");
            let _ = writeln!(
                ttl,
                "    sh:name {} ;",
                ttl_str(&format!("{} — at least one applicable entity", spec.name))
            );
            let _ = writeln!(
                ttl,
                "    rdfs:comment \"Dataset-level: checks the whole model, not a written node.\" ;"
            );
            let _ = writeln!(ttl, "    sh:targetClass ifcids:Model ;");
            let _ = writeln!(
                ttl,
                "    sh:sparql [ a sh:SPARQLConstraint ;\n        sh:message {} ;\n        sh:select {} ] .\n",
                ttl_str(&format!(
                    "the required specification `{}` has no applicable entity in this model",
                    spec.name
                )),
                ttl_long(&q)
            );
            shape_count += 1;
        } else {
            let fp = fingerprint(&plain_targets, &selects);
            let _ = writeln!(ttl, "    ots:idsFingerprint {} .\n", ttl_str(&fp));
        }
        summaries.push(SpecSummary {
            name: spec.name.clone(),
            shape,
            target_classes: targets
                .iter()
                .map(|t| {
                    let t = t.trim_matches(['<', '>']);
                    t.rsplit_once('/')
                        .map(|(_, local)| local.replace('#', ":"))
                        .unwrap_or_else(|| t.to_string())
                })
                .collect(),
            requirements: spec.requirements.len(),
        });
    }
    warnings.sort();
    warnings.dedup();
    ImportedShapes {
        title,
        description: doc.description.clone(),
        turtle: ttl,
        shape_count,
        specifications: summaries,
        warnings,
    }
}

/// The importer's own sample document, so the exporter's tests can
/// round-trip against exactly what this importer produces.
#[cfg(test)]
pub(crate) fn tests_sample() -> &'static str {
    tests::SAMPLE
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) const SAMPLE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<ids:ids xmlns:ids="http://standards.buildingsmart.org/IDS" xmlns:xs="http://www.w3.org/2001/XMLSchema">
  <ids:info><ids:title>Wall fire ratings</ids:title><ids:description>External walls carry a fire rating.</ids:description></ids:info>
  <ids:specifications>
    <ids:specification name="External walls need a fire rating" ifcVersion="IFC4">
      <ids:applicability minOccurs="1" maxOccurs="unbounded">
        <ids:entity><ids:name><ids:simpleValue>IFCWALL</ids:simpleValue></ids:name></ids:entity>
        <ids:property dataType="IFCBOOLEAN"><ids:propertySet><ids:simpleValue>Pset_WallCommon</ids:simpleValue></ids:propertySet><ids:baseName><ids:simpleValue>IsExternal</ids:simpleValue></ids:baseName><ids:value><ids:simpleValue>true</ids:simpleValue></ids:value></ids:property>
      </ids:applicability>
      <ids:requirements>
        <ids:property cardinality="required" dataType="IFCLABEL"><ids:propertySet><ids:simpleValue>Pset_WallCommon</ids:simpleValue></ids:propertySet><ids:baseName><ids:simpleValue>FireRating</ids:simpleValue></ids:baseName><ids:value><xs:restriction base="xs:string"><xs:enumeration value="REI30"/><xs:enumeration value="REI60"/></xs:restriction></ids:value></ids:property>
        <ids:attribute cardinality="required"><ids:name><ids:simpleValue>Name</ids:simpleValue></ids:name></ids:attribute>
      </ids:requirements>
    </ids:specification>
    <ids:specification name="Windows sit in a storey" ifcVersion="IFC2X3 IFC4">
      <ids:applicability minOccurs="0" maxOccurs="unbounded"><ids:entity><ids:name><ids:simpleValue>IFCWINDOW</ids:simpleValue></ids:name></ids:entity></ids:applicability>
      <ids:requirements><ids:partOf cardinality="required" relation="IFCRELCONTAINEDINSPATIALSTRUCTURE"><ids:entity><ids:name><ids:simpleValue>IFCBUILDINGSTOREY</ids:simpleValue></ids:name></ids:entity></ids:partOf></ids:requirements>
    </ids:specification>
  </ids:specifications>
</ids:ids>"#;

    fn import(doc: &str) -> anyhow::Result<ImportedShapes> {
        IdsImporter.import(doc.as_bytes())
    }

    fn spec(body: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<ids xmlns="http://standards.buildingsmart.org/IDS" xmlns:xs="http://www.w3.org/2001/XMLSchema">
  <info><title>t</title></info>
  <specifications>{body}</specifications>
</ids>"#
        )
    }

    const WALL: &str = "<entity><name><simpleValue>IFCWALL</simpleValue></name></entity>";

    #[test]
    fn the_sample_converts_to_valid_shacl_sparql_shapes() {
        let out = import(SAMPLE).expect("converts");
        assert_eq!(out.title, "Wall fire ratings");
        assert_eq!(out.specifications.len(), 2);
        let t = &out.turtle;
        assert!(
            t.contains("sh:targetClass <https://opentriplestore.org/ns/ifc-ids/IFC4#IFCWALL>"),
            "{t}"
        );
        // "IFC2X3 IFC4": both schemas' window classes are targets.
        assert!(
            t.contains("<https://opentriplestore.org/ns/ifc-ids/IFC2X3#IFCWINDOW>"),
            "{t}"
        );
        assert!(
            t.contains("<https://opentriplestore.org/ns/ifc-ids/IFC4#IFCWINDOW>"),
            "{t}"
        );
        // The required first specification gets an existence shape, the optional second none.
        assert!(t.contains("<urn:ids:spec1-exists> a sh:NodeShape"), "{t}");
        assert!(!t.contains("<urn:ids:spec2-exists>"), "{t}");
        // The source is kept for export.
        assert!(t.contains("ots:idsSpecification"), "{t}");
        assert!(t.contains("ots:idsFingerprint"), "{t}");
        // It loads as shapes the validator accepts.
        let store = crate::store::TripleStore::in_memory().unwrap();
        store
            .load_str(t, oxigraph::io::RdfFormat::Turtle, Some("urn:s"))
            .unwrap_or_else(|e| panic!("valid Turtle ({e}):\n{t}"));
        crate::shacl::engine::load_shapes(&store, "urn:s").expect("the SPARQL constraints parse");
    }

    #[test]
    fn the_xsd_structure_is_enforced() {
        // Unknown element, missing title, bad ifcVersion, cardinality on an
        // applicability facet, two simple values.
        let bad = r#"<ids xmlns="http://standards.buildingsmart.org/IDS"><info/><specifications>
          <specification name="s" ifcVersion="IFC5"><applicability><entity><name><simpleValue>IFCWALL</simpleValue></name></entity>
          <attribute cardinality="required"><name><simpleValue>Name</simpleValue><simpleValue>X</simpleValue></name></attribute><bogus/></applicability></specification>
          </specifications></ids>"#;
        let err = import(bad).unwrap_err().to_string();
        for needle in [
            "needs <title>",
            "IFC5",
            "attribute `cardinality`",
            "exactly one",
            "<bogus>",
        ] {
            assert!(err.contains(needle), "`{needle}` in: {err}");
        }
        let err = import("<ids/>").unwrap_err().to_string();
        assert!(err.contains("namespace"), "{err}");
    }

    #[test]
    fn the_audit_refuses_specifications_no_model_could_satisfy() {
        let prop = |dt: &str, value: &str| {
            format!(
                r#"<property dataType="{dt}"><propertySet><simpleValue>P</simpleValue></propertySet><baseName><simpleValue>V</simpleValue></baseName>{value}</property>"#
            )
        };
        let req = |schema: &str, app: &str, reqs: &str| {
            format!(
                r#"<specification name="s" ifcVersion="{schema}"><applicability>{app}</applicability><requirements>{reqs}</requirements></specification>"#
            )
        };
        let refraction =
            "<entity><name><simpleValue>IFCSURFACESTYLEREFRACTION</simpleValue></name></entity>";
        let attr = |name: &str| {
            format!("<attribute><name><simpleValue>{name}</simpleValue></name></attribute>")
        };
        let cases = [
            // Entity names are upper case and must exist.
            (
                r#"<specification name="s" ifcVersion="IFC4"><applicability><entity><name><simpleValue>IfcWall</simpleValue></name></entity></applicability></specification>"#.to_string(),
                "does not exist",
            ),
            // An attribute IfcWall does not have.
            (req("IFC4", WALL, &attr("ActingRole")), "no entity can meet both"),
            // An inverse attribute.
            (req("IFC4", WALL, &attr("ContainedInStructure")), "not an explicit attribute"),
            // A float for an integer property.
            (
                req("IFC4", WALL, &prop("IFCINTEGER", "<value><simpleValue>42.0</simpleValue></value>")),
                "not a valid xs:integer",
            ),
            // A boolean in upper case.
            (
                req("IFC4", WALL, &prop("IFCBOOLEAN", "<value><simpleValue>FALSE</simpleValue></value>")),
                "not a valid xs:boolean",
            ),
            // IFCDURATION is not an IFC2X3 type.
            (req("IFC2X3", WALL, &prop("IFCDURATION", "")), "does not exist in IFC2X3"),
            // Requirements on a prohibited specification.
            (
                format!(
                    r#"<specification name="s" ifcVersion="IFC4"><applicability minOccurs="0" maxOccurs="0">{WALL}</applicability><requirements><attribute><name><simpleValue>Name</simpleValue></name></attribute></requirements></specification>"#
                ),
                "not allowed when the applicability is prohibited",
            ),
            // A predefined type IFC2X3's IfcInventory does not have.
            (
                req("IFC2X3", WALL, r#"<partOf relation="IFCRELASSIGNSTOGROUP"><entity><name><simpleValue>IFCINVENTORY</simpleValue></name><predefinedType><simpleValue>X</simpleValue></predefinedType></entity></partOf>"#),
                "no predefined type",
            ),
            // A pattern on a number.
            (
                req("IFC4", refraction, r#"<attribute><name><simpleValue>RefractionIndex</simpleValue></name><value><xs:restriction base="xs:string"><xs:pattern value=".*"/></xs:restriction></value></attribute>"#),
                "restriction base is `xs:string` but the value is a xs:double",
            ),
        ];
        for (body, needle) in cases {
            let err = import(&spec(&body)).unwrap_err().to_string();
            assert!(err.contains(needle), "`{needle}` in: {err}\nfor {body}");
        }
    }

    #[test]
    fn requirement_cardinality_maps_to_the_violation_condition() {
        let out = import(&spec(&format!(
            r#"<specification name="s" ifcVersion="IFC4"><applicability>{WALL}</applicability><requirements>
              <attribute cardinality="prohibited"><name><simpleValue>Description</simpleValue></name></attribute>
              <attribute cardinality="optional"><name><simpleValue>Name</simpleValue></name><value><simpleValue>X</simpleValue></value></attribute>
              <property dataType="IFCREAL"><propertySet><simpleValue>P</simpleValue></propertySet><baseName><simpleValue>V</simpleValue></baseName><value><simpleValue>1.</simpleValue></value></property>
            </requirements></specification>"#
        )))
        .expect("converts");
        let t = &out.turtle;
        // Prohibited: the facet must not hold, so the violation is the match itself.
        assert!(
            t.contains("attribute \\\"Description\\\" (prohibited)"),
            "{t}"
        );
        assert!(t.contains("SELECT $this WHERE { FILTER(((EXISTS { $this <https://opentriplestore.org/ns/ifc-ids/attr#Description>"), "{t}");
        // Optional: present and not matching.
        assert!(
            t.contains("<https://opentriplestore.org/ns/ifc-ids/attr#Name> ?a"),
            "{t}"
        );
        assert!(
            t.contains(") && !((EXISTS { $this <https://opentriplestore.org/ns/ifc-ids/attr#Name>"),
            "{t}"
        );
        // A double is compared within the IDS tolerance.
        assert!(t.contains(">= \\\"0.99999799999999"), "{t}");
    }

    #[test]
    fn a_specification_round_trips_through_its_xml() {
        let root = parse_xml(SAMPLE.as_bytes()).unwrap();
        let doc = read(&root).unwrap();
        // The recorded source parses back to the same model.
        let wrapped = format!(
            "<ids:ids xmlns:ids=\"{IDS_NS}\" xmlns:xs=\"{XS_NS}\">{}<ids:specifications>{}</ids:specifications></ids:ids>",
            doc.info_xml, doc.specs[0].xml
        );
        let again = read(&parse_xml(wrapped.as_bytes()).unwrap()).unwrap();
        assert_eq!(again.specs.len(), 1);
        assert_eq!(again.specs[0].xml, doc.specs[0].xml);
        assert_eq!(again.title, "Wall fire ratings");
    }

    /// A non-ASCII value lands inside a SPARQL string literal; the
    /// pre-binding scanner must not slice inside a multi-byte character.
    #[test]
    fn non_ascii_values_load_as_shapes() {
        let out = import(&spec(&format!(
            r#"<specification name="s" ifcVersion="IFC4"><applicability>{WALL}</applicability><requirements><attribute><name><simpleValue>Name</simpleValue></name><value><simpleValue>♫Don'tÄrgerhôtelЊет</simpleValue></value></attribute></requirements></specification>"#
        )))
        .expect("converts");
        let store = crate::store::TripleStore::in_memory().unwrap();
        store
            .load_str(&out.turtle, oxigraph::io::RdfFormat::Turtle, Some("urn:s"))
            .unwrap();
        crate::shacl::engine::load_shapes(&store, "urn:s").expect("loads");
        assert!(out.turtle.contains("♫Don'tÄrgerhôtelЊет"));
    }
}
