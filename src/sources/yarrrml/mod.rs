//! YARRRML — the authoring view over RML.
//!
//! YARRRML is a YAML shorthand for RML. It is translated on the way in and
//! **only** RML is stored, so the store has exactly one mapping representation
//! to version, diff, gate and execute. Round-tripping YAML is deliberately not
//! offered: a stored mapping is RDF, and pretending otherwise would invite two
//! sources of truth.
//!
//! The subset translated here is the one this engine can execute against a SQL
//! datasource. Anything outside it is an error naming the construct, rather
//! than a silent omission that surfaces later as missing triples.
//!
//! Supported: `prefixes`; named and inline `sources` (`access` naming a
//! registered datasource, with `table` or `query`); `mappings` with `s`/
//! `subject` and `po`/`predicateobjects`; the `[p, o]`, `[p, o, datatype]` and
//! `{p:, o:}` forms; `a` for `rdf:type`; `$(column)` references and templates;
//! constant IRIs; `~iri` and `~lang` suffixes; and joins through
//! `o: {mapping:, condition:}` with the `equal` function.
//!
//! One extension beyond the YARRRML spec, because the engine supports it and a
//! SQL source needs it: an object may be a code-list lookup, written
//! `o: {value: $(col), values: {code: ex:Term, …}, normalize: lower_trim,
//! unmapped: literal}`. It translates to the same RML-FNML function a
//! hand-written mapping would use.
//!
//! Not supported: function-valued terms beyond joins, graph maps (a run's graph
//! is the unit of promotion — see [`super::mappings`]), multiple sources per
//! mapping, and nested/quoted templates.
//!
//! The legacy `mapping.sql2rdf.yaml` format is a separate, one-time migration
//! and is not translated here.

pub mod node;
pub mod rml_out;
pub mod yaml;

use std::collections::BTreeMap;

use node::Node;
use rml_out::{
    datatype_iri, expand, template_to_rml, ObjectOut, PomOut, TermTypeOut, TriplesMapOut,
    UnmappedOut,
};

/// Why a YARRRML document could not be translated. Every message names the
/// mapping (and where useful the predicate) it came from, because a YAML file
/// with twenty mappings is otherwise a guessing game.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum YarrrmlError {
    #[error("the document is not valid YAML: {0}")]
    Yaml(String),
    #[error("the document is {0}, not a map — a YARRRML file's top level is a map")]
    NotAMap(&'static str),
    #[error("the document declares no 'mappings'")]
    NoMappings,
    #[error("mapping '{mapping}': {reason}")]
    Mapping { mapping: String, reason: String },
    #[error("{0}")]
    Document(String),
}

fn m_err(mapping: &str, reason: impl Into<String>) -> YarrrmlError {
    YarrrmlError::Mapping {
        mapping: mapping.to_string(),
        reason: reason.into(),
    }
}

/// Translate a YARRRML document into RML Turtle.
///
/// `default_source` is the datasource the mapping is being registered against;
/// a document that names no source of its own is bound to it, which is the
/// common case when authoring from the Sources workspace.
pub fn to_rml(yarrrml: &str, default_source: Option<&str>) -> Result<String, YarrrmlError> {
    let doc = yaml::parse(yarrrml).map_err(YarrrmlError::Yaml)?;
    translate(&doc, default_source)
}

/// The tree-level translation, separate from parsing so it can be tested
/// without writing YAML.
pub fn translate(doc: &Node, default_source: Option<&str>) -> Result<String, YarrrmlError> {
    let Node::Map(_) = doc else {
        return Err(YarrrmlError::NotAMap(doc.kind()));
    };

    let prefixes = read_prefixes(doc)?;
    let sources = read_sources(doc, &prefixes)?;
    let mappings = doc
        .get_any(&["mappings", "mapping"])
        .and_then(Node::as_map)
        .ok_or(YarrrmlError::NoMappings)?;
    if mappings.is_empty() {
        return Err(YarrrmlError::NoMappings);
    }

    let mut out = Vec::with_capacity(mappings.len());
    for (name, body) in mappings {
        out.push(translate_mapping(
            name,
            body,
            &prefixes,
            &sources,
            default_source,
        )?);
    }
    rml_out::render(&out).map_err(YarrrmlError::Document)
}

fn read_prefixes(doc: &Node) -> Result<BTreeMap<String, String>, YarrrmlError> {
    let mut out = BTreeMap::new();
    if let Some(Node::Map(m)) = doc.get("prefixes") {
        for (label, value) in m {
            let ns = value.as_str().ok_or_else(|| {
                YarrrmlError::Document(format!(
                    "prefix '{label}' maps to {}, but a namespace is a string",
                    value.kind()
                ))
            })?;
            out.insert(label.clone(), ns.trim().to_string());
        }
    }
    Ok(out)
}

/// One logical source: the datasource it reads and what it selects.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SourceDef {
    source: String,
    table: Option<String>,
    query: Option<String>,
}

/// Normalise whatever the document called the datasource into its IRI. A bare
/// id is accepted because that is what the Sources workspace shows.
fn source_iri(access: &str) -> String {
    let a = access.trim();
    if a.starts_with("urn:source:") {
        a.to_string()
    } else {
        format!("urn:source:{a}")
    }
}

fn read_source_body(
    body: &Node,
    prefixes: &BTreeMap<String, String>,
    default_source: Option<&str>,
) -> Result<SourceDef, String> {
    let _ = prefixes;
    let access = body
        .str_at(&["access", "source", "datasource"])
        .map(|a| source_iri(&a))
        .or_else(|| default_source.map(source_iri))
        .ok_or_else(|| {
            "the source names no 'access', and the mapping is not being registered against a \
             datasource that could supply one"
                .to_string()
        })?;
    let table = body.str_at(&["table", "tableName"]);
    let query = body.str_at(&["query", "sqlQuery"]);
    if table.is_none() && query.is_none() {
        return Err("a relational source needs 'table' or 'query'".to_string());
    }
    Ok(SourceDef {
        source: access,
        table,
        query,
    })
}

fn read_sources(
    doc: &Node,
    prefixes: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, SourceDef>, YarrrmlError> {
    let mut out = BTreeMap::new();
    if let Some(Node::Map(m)) = doc.get("sources") {
        for (name, body) in m {
            let def = read_source_body(body, prefixes, None)
                .map_err(|e| YarrrmlError::Document(format!("source '{name}': {e}")))?;
            out.insert(name.clone(), def);
        }
    }
    Ok(out)
}

fn translate_mapping(
    name: &str,
    body: &Node,
    prefixes: &BTreeMap<String, String>,
    sources: &BTreeMap<String, SourceDef>,
    default_source: Option<&str>,
) -> Result<TriplesMapOut, YarrrmlError> {
    if body.get_any(&["graph", "graphs", "g"]).is_some() {
        return Err(m_err(
            name,
            "declares a graph. A run's graph is the unit the write gate validates and the role \
             swap promotes, so a mapping may not choose its own",
        ));
    }

    let source = resolve_source(name, body, sources, prefixes, default_source)?;
    let subject_raw = body
        .str_at(&["s", "subject", "subjects"])
        .ok_or_else(|| m_err(name, "has no subject ('s')"))?;
    let (subject_value, subject_type) = strip_suffix(&subject_raw);
    let subject =
        template_to_rml(&expand_template(&subject_value, prefixes).map_err(|e| m_err(name, e))?);
    if subject_type == Some(TermTypeOut::Literal) {
        return Err(m_err(name, "a subject cannot be a literal"));
    }

    let mut classes = Vec::new();
    let mut poms = Vec::new();
    for entry in body
        .get_any(&["po", "predicateobjects", "predicateObjects"])
        .map(Node::as_seq)
        .unwrap_or_default()
    {
        translate_po(name, entry, prefixes, &mut classes, &mut poms)?;
    }

    Ok(TriplesMapOut {
        name: name.to_string(),
        source: source.source,
        table: source.table,
        query: source.query,
        subject,
        subject_term_type: subject_type.unwrap_or(TermTypeOut::Iri),
        classes,
        poms,
    })
}

fn resolve_source(
    name: &str,
    body: &Node,
    sources: &BTreeMap<String, SourceDef>,
    prefixes: &BTreeMap<String, String>,
    default_source: Option<&str>,
) -> Result<SourceDef, YarrrmlError> {
    let declared = body
        .get_any(&["sources", "source"])
        .map(Node::as_seq)
        .unwrap_or_default();
    match declared.len() {
        0 => {
            // No source of its own: bind to the datasource being registered
            // against, and read the table from the mapping body.
            read_source_body(body, prefixes, default_source).map_err(|e| m_err(name, e))
        }
        1 => {
            let entry = declared[0];
            if let Some(reference) = entry.as_str() {
                return sources.get(reference).cloned().ok_or_else(|| {
                    m_err(
                        name,
                        format!("names source '{reference}', which the document does not declare"),
                    )
                });
            }
            read_source_body(entry, prefixes, default_source).map_err(|e| m_err(name, e))
        }
        n => Err(m_err(
            name,
            format!(
                "declares {n} sources. A mapping reads exactly one datasource, so a run has one \
                 connection and one write gate — split it into one mapping per source"
            ),
        )),
    }
}

/// Split a YARRRML `~`-suffixed value: `ex:Thing~iri`, `"text"~lang`.
fn strip_suffix(value: &str) -> (String, Option<TermTypeOut>) {
    match value.rsplit_once('~') {
        Some((head, "iri")) => (head.to_string(), Some(TermTypeOut::Iri)),
        Some((head, "blanknode" | "blank")) => (head.to_string(), Some(TermTypeOut::BlankNode)),
        Some((head, "literal" | "lang")) => (head.to_string(), Some(TermTypeOut::Literal)),
        _ => (value.to_string(), None),
    }
}

/// Expand prefixed names inside a template's literal segments, leaving
/// `$(column)` placeholders alone. `ex:product_$(id)` becomes
/// `http://example.org/product_$(id)`.
fn expand_template(value: &str, prefixes: &BTreeMap<String, String>) -> Result<String, String> {
    let head = value.split("$(").next().unwrap_or("");
    if head.contains("://") || head.starts_with("urn:") || head.starts_with('<') {
        return Ok(value.trim_matches(['<', '>']).to_string());
    }
    let Some((prefix, _)) = head.split_once(':') else {
        return Ok(value.to_string());
    };
    let ns = prefixes.get(prefix).ok_or_else(|| {
        format!("prefix '{prefix}:' is used by '{value}' but is not declared in the prefixes block")
    })?;
    Ok(value.replacen(&format!("{prefix}:"), ns, 1))
}

/// Whether a value interpolates a column.
fn is_template(value: &str) -> bool {
    value.contains("$(")
}

/// Whether a value is exactly one column reference, `$(name)`.
fn sole_reference(value: &str) -> Option<String> {
    let inner = value.strip_prefix("$(")?.strip_suffix(')')?;
    (!inner.is_empty() && !inner.contains("$(")).then(|| inner.to_string())
}

fn translate_po(
    name: &str,
    entry: &Node,
    prefixes: &BTreeMap<String, String>,
    classes: &mut Vec<String>,
    poms: &mut Vec<PomOut>,
) -> Result<(), YarrrmlError> {
    // Shorthand: [predicate, object] or [predicate, object, datatype].
    let (predicate_raw, object_node, third) = match entry {
        Node::Seq(items) if items.len() >= 2 => (
            items[0]
                .as_str()
                .ok_or_else(|| m_err(name, "a predicate must be a string"))?
                .to_string(),
            items[1].clone(),
            items.get(2).and_then(Node::as_str).map(str::to_string),
        ),
        Node::Map(_) => {
            let p = entry
                .str_at(&["p", "predicate", "predicates"])
                .ok_or_else(|| m_err(name, "a predicate-object entry has no 'p'"))?;
            let o = entry
                .get_any(&["o", "object", "objects"])
                .cloned()
                .ok_or_else(|| m_err(name, format!("predicate '{p}' has no 'o'")))?;
            let dt = entry.str_at(&["datatype", "language"]);
            (p, o, dt)
        }
        other => {
            return Err(m_err(
                name,
                format!(
                "a predicate-object entry is {}, but it must be [p, o] or a map with 'p' and 'o'",
                other.kind()
            ),
            ))
        }
    };

    let predicate = expand(&predicate_raw, prefixes).map_err(|e| m_err(name, e))?;

    // `a ex:Product` / `rdf:type ex:Product` is a class, not a predicate-object
    // pair — RML expresses it on the subject map.
    if predicate == format!("{}type", "http://www.w3.org/1999/02/22-rdf-syntax-ns#") {
        let value = object_node
            .as_str()
            .ok_or_else(|| m_err(name, "rdf:type needs a class IRI"))?;
        let (value, _) = strip_suffix(value);
        let class = expand(&value, prefixes).map_err(|e| m_err(name, e))?;
        if !classes.contains(&class) {
            classes.push(class);
        }
        return Ok(());
    }

    let object = translate_object(name, &predicate_raw, &object_node, third, prefixes)?;
    poms.push(PomOut { predicate, object });
    Ok(())
}

fn translate_object(
    name: &str,
    predicate: &str,
    node: &Node,
    third: Option<String>,
    prefixes: &BTreeMap<String, String>,
) -> Result<ObjectOut, YarrrmlError> {
    let where_ = |reason: String| m_err(name, format!("predicate '{predicate}': {reason}"));

    // A join: `o: { mapping: other, condition: {...} }`.
    if let Some(parent) = node.str_at(&["mapping", "parentTriplesMap"]) {
        let joins = read_condition(node).map_err(where_)?;
        return Ok(ObjectOut::Parent {
            mapping: parent,
            joins,
        });
    }

    // A code list: `o: { value: $(col), values: {code: ex:Term, …} }`.
    if let Some(Node::Map(values)) = node.get_any(&["values", "value_map", "valueMap"]) {
        let raw = node
            .str_at(&["value", "o", "object"])
            .ok_or_else(|| where_("a code list needs 'value' naming the column".to_string()))?;
        let column = sole_reference(&raw).unwrap_or(raw);
        let normalize = node
            .str_at(&["normalize"])
            .unwrap_or_else(|| "none".to_string());
        let mut entries = Vec::with_capacity(values.len());
        for (code, target) in values {
            let target = target.as_str().ok_or_else(|| {
                where_(format!(
                    "code '{code}' maps to {}, not a term",
                    target.kind()
                ))
            })?;
            entries.push((code.clone(), expand(target, prefixes).map_err(where_)?));
        }
        let unmapped = match node
            .str_at(&["unmapped"])
            .unwrap_or_else(|| "literal".to_string())
            .as_str()
        {
            "literal" => UnmappedOut::Literal,
            "omit" | "skip" => UnmappedOut::Omit,
            "template" | "mint" => UnmappedOut::Template(
                node.str_at(&["unmappedTemplate", "unmapped_template"])
                    .map(|t| template_to_rml(&t))
                    .ok_or_else(|| {
                        where_(
                            "unmapped 'template' needs 'unmappedTemplate' with an absolute IRI"
                                .to_string(),
                        )
                    })?,
            ),
            other => {
                return Err(where_(format!(
                    "unknown 'unmapped' policy '{other}'; expected literal, omit or template"
                )))
            }
        };
        let datatype = match node.str_at(&["datatype"]) {
            Some(d) => Some(datatype_iri(&d, prefixes).map_err(where_)?),
            None => None,
        };
        return Ok(ObjectOut::Enumeration {
            column,
            normalize,
            entries,
            unmapped,
            datatype,
        });
    }

    // The expanded value form: `o: { value: $(col), datatype: xsd:decimal }`.
    let (raw, declared) = if let Some(v) = node.str_at(&["value", "o", "object"]) {
        (v, node.str_at(&["datatype", "type"]).or(third.clone()))
    } else {
        let v = node.as_str().ok_or_else(|| {
            where_(format!(
                "the object is {}, which is not a value",
                node.kind()
            ))
        })?;
        (v.to_string(), third.clone())
    };
    let language = node.str_at(&["language", "lang"]);

    let (value, forced) = strip_suffix(&raw);

    // `$(col)` on its own is a column; the datatype (or language) applies to it.
    if let Some(column) = sole_reference(&value) {
        if forced == Some(TermTypeOut::Iri) {
            return Ok(ObjectOut::Template {
                template: format!("{{{column}}}"),
                term_type: TermTypeOut::Iri,
            });
        }
        let datatype = match &declared {
            Some(d) if language.is_none() && !is_language_tag(d) => {
                Some(datatype_iri(d, prefixes).map_err(where_)?)
            }
            _ => None,
        };
        let language = language.or_else(|| declared.filter(|d| is_language_tag(d)));
        return Ok(ObjectOut::Column {
            column,
            datatype,
            language,
        });
    }

    // Anything else containing `$(` is a template.
    if is_template(&value) {
        let expanded = expand_template(&value, prefixes).map_err(where_)?;
        return Ok(ObjectOut::Template {
            template: template_to_rml(&expanded),
            // A template that expanded through a prefix is an IRI unless the
            // author said otherwise; a bare one is text.
            term_type: forced.unwrap_or(
                if expanded.contains("://") || expanded.starts_with("urn:") {
                    TermTypeOut::Iri
                } else {
                    TermTypeOut::Literal
                },
            ),
        });
    }

    // A constant. A prefixed name or absolute IRI is a term; anything else is
    // a plain literal.
    match forced {
        Some(TermTypeOut::Literal) | None if expand(&value, prefixes).is_err() => {
            Ok(ObjectOut::Constant {
                value,
                is_iri: false,
            })
        }
        Some(TermTypeOut::Literal) => Ok(ObjectOut::Constant {
            value,
            is_iri: false,
        }),
        _ => Ok(ObjectOut::Constant {
            value: expand(&value, prefixes).map_err(where_)?,
            is_iri: true,
        }),
    }
}

/// A two-letter-ish tag rather than a datatype name. YARRRML puts both in the
/// same slot, so they are told apart by shape.
fn is_language_tag(value: &str) -> bool {
    !value.contains(':')
        && value.len() <= 8
        && value.chars().all(|c| c.is_ascii_alphabetic() || c == '-')
        && value.contains('-')
        || (value.len() == 2 && value.chars().all(|c| c.is_ascii_alphabetic()))
}

/// Read `condition: {function: equal, parameters: [[str1, $(a)], [str2, $(b)]]}`
/// into child/parent column pairs.
fn read_condition(node: &Node) -> Result<Vec<(String, String)>, String> {
    let mut out = Vec::new();
    for condition in node
        .get_any(&["condition", "conditions"])
        .map(Node::as_seq)
        .unwrap_or_default()
    {
        let function = condition
            .str_at(&["function", "f"])
            .unwrap_or_else(|| "equal".to_string());
        if !function.ends_with("equal") {
            return Err(format!(
                "join function '{function}' is not supported; a join condition compares two \
                 columns with 'equal'"
            ));
        }
        let params = condition
            .get_any(&["parameters", "params"])
            .map(Node::as_seq)
            .unwrap_or_default();
        let mut child = None;
        let mut parent = None;
        for p in params {
            let pair = p.as_seq();
            let (key, value) = match pair.as_slice() {
                [k, v] => (
                    k.as_str().unwrap_or_default().to_string(),
                    v.as_str().unwrap_or_default().to_string(),
                ),
                _ => (
                    p.str_at(&["parameter"]).unwrap_or_default(),
                    p.str_at(&["value"]).unwrap_or_default(),
                ),
            };
            let column = sole_reference(&value).unwrap_or(value);
            match key.as_str() {
                "str1" | "child" => child = Some(column),
                "str2" | "parent" => parent = Some(column),
                other => return Err(format!("unknown join parameter '{other}'")),
            }
        }
        match (child, parent) {
            (Some(c), Some(p)) => out.push((c, p)),
            _ => return Err("a join condition needs both 'str1' and 'str2'".to_string()),
        }
    }
    if out.is_empty() {
        return Err(
            "a join needs a 'condition'; without one every parent row would match every child row"
                .to_string(),
        );
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rml::model::{ObjectMap, RmlMapping, TermMapKind};

    /// The translator's contract: whatever it emits, this engine parses.
    fn rml(yarrrml: &str) -> RmlMapping {
        let turtle = to_rml(yarrrml, Some("legacy"))
            .unwrap_or_else(|e| panic!("translation failed: {e}\n---\n{yarrrml}"));
        crate::rml::parse_rml(&turtle)
            .unwrap_or_else(|e| panic!("emitted RML does not parse: {e}\n---\n{turtle}"))
    }

    fn tm<'a>(m: &'a RmlMapping, name: &str) -> &'a crate::rml::model::TriplesMap {
        m.find(&rml_out::map_iri(name))
            .unwrap_or_else(|| panic!("no triples map '{name}'"))
    }

    fn object<'a>(m: &'a RmlMapping, map: &str, predicate: &str) -> &'a ObjectMap {
        &tm(m, map)
            .predicate_object_maps
            .iter()
            .find(|p| matches!(&p.predicate_map.kind, TermMapKind::Constant(c) if c == predicate))
            .unwrap_or_else(|| panic!("no predicate <{predicate}> on '{map}'"))
            .object
    }

    const BASIC: &str = r#"
prefixes:
  ex: http://example.org/products/ontology#
  prod: http://example.org/products/

mappings:
  product:
    sources:
      - access: urn:source:legacy
        query: SELECT product_id, name, price FROM products
    s: prod:product_$(product_id)
    po:
      - [a, ex:Product]
      - [ex:name, $(name)]
      - [ex:hasPrice, $(price), xsd:decimal]
"#;

    #[test]
    fn a_basic_mapping_translates_into_executable_rml() {
        let m = rml(BASIC);
        assert_eq!(m.datasources(), vec!["urn:source:legacy"]);
        let p = tm(&m, "product");
        assert_eq!(
            p.logical_source.query.as_deref(),
            Some("SELECT product_id, name, price FROM products")
        );
        assert!(
            matches!(&p.subject_map.term_map.kind,
                TermMapKind::Template(t) if t == "http://example.org/products/product_{product_id}"),
            "the prefix expanded and $(col) became {{col}}: {:?}",
            p.subject_map.term_map.kind
        );
        assert_eq!(
            p.subject_map.classes,
            vec!["http://example.org/products/ontology#Product"],
            "`a` becomes rr:class, not a predicate-object pair"
        );
        let ObjectMap::Term(t) = object(&m, "product", "http://example.org/products/ontology#name")
        else {
            panic!("expected a term object")
        };
        assert!(matches!(&t.kind, TermMapKind::Reference(c) if c == "name"));
        assert_eq!(t.datatype, None, "an undeclared datatype stays undeclared");

        let ObjectMap::Term(price) = object(
            &m,
            "product",
            "http://example.org/products/ontology#hasPrice",
        ) else {
            panic!()
        };
        assert_eq!(
            price.datatype.as_deref(),
            Some("http://www.w3.org/2001/XMLSchema#decimal"),
            "the third element of the shorthand is the datatype"
        );
    }

    #[test]
    fn a_named_source_is_resolved_and_a_missing_one_is_named() {
        let m = rml(r#"
prefixes: {ex: "http://example.org/"}
sources:
  db:
    access: legacy
    table: products
mappings:
  product:
    sources: [db]
    s: ex:p_$(id)
    po: [[ex:name, $(name)]]
"#);
        assert_eq!(
            tm(&m, "product").logical_source.table_name.as_deref(),
            Some("products")
        );
        assert_eq!(
            m.datasources(),
            vec!["urn:source:legacy"],
            "a bare id becomes the IRI"
        );

        let err = to_rml(
            "prefixes: {ex: \"http://x/\"}\nmappings:\n  p:\n    sources: [nope]\n    s: ex:a\n",
            Some("legacy"),
        )
        .unwrap_err();
        assert!(err.to_string().contains("nope"), "{err}");
    }

    #[test]
    fn a_mapping_with_no_source_binds_to_the_datasource_it_is_registered_against() {
        let m = rml(r#"
prefixes: {ex: "http://example.org/"}
mappings:
  product:
    table: products
    s: ex:p_$(id)
    po: [[ex:name, $(name)]]
"#);
        assert_eq!(m.datasources(), vec!["urn:source:legacy"]);
        assert_eq!(
            tm(&m, "product").logical_source.table_name.as_deref(),
            Some("products")
        );
    }

    #[test]
    fn a_join_becomes_a_referencing_object_map() {
        let m = rml(r#"
prefixes: {ex: "http://example.org/"}
mappings:
  product:
    table: products
    s: ex:p_$(product_id)
    po:
      - p: ex:suppliedBy
        o:
          mapping: supplier
          condition:
            function: equal
            parameters:
              - [str1, $(supplier_id)]
              - [str2, $(supplier_id)]
  supplier:
    table: suppliers
    s: ex:s_$(supplier_id)
    po: [[ex:name, $(name)]]
"#);
        let ObjectMap::Ref(r) = object(&m, "product", "http://example.org/suppliedBy") else {
            panic!("expected a referencing object map")
        };
        assert_eq!(r.parent_triples_map, rml_out::map_iri("supplier"));
        assert_eq!(r.joins.len(), 1);
        assert_eq!(r.joins[0].child, "supplier_id");
        assert_eq!(r.joins[0].parent, "supplier_id");
    }

    #[test]
    fn a_join_without_a_condition_is_refused() {
        let err = to_rml(
            r#"
prefixes: {ex: "http://example.org/"}
mappings:
  a:
    table: t
    s: ex:a_$(id)
    po:
      - p: ex:rel
        o: {mapping: b}
  b:
    table: u
    s: ex:b_$(id)
"#,
            Some("legacy"),
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("condition"),
            "a cross join must be refused, not silently emitted: {err}"
        );
    }

    #[test]
    fn a_code_list_becomes_the_value_map_function() {
        let m = rml(r#"
prefixes: {ex: "http://example.org/"}
mappings:
  product:
    table: products
    s: ex:p_$(id)
    po:
      - p: ex:hasStatus
        o:
          value: $(status)
          normalize: lower_trim
          values:
            active: ex:Active
            retired: ex:Retired
          unmapped: literal
"#);
        let ObjectMap::Function(f) = object(&m, "product", "http://example.org/hasStatus") else {
            panic!("expected a function object map")
        };
        assert_eq!(f.function, crate::rml::terms::FN_MAP_VALUE);
        assert_eq!(f.all(&format!("{}mapping", rml_out::FN)).len(), 2);
        assert_eq!(
            f.first(&format!("{}unmapped", rml_out::FN))
                .and_then(|a| match a {
                    crate::rml::model::FunctionArg::Constant(c) => Some(c.as_str()),
                    _ => None,
                }),
            Some("literal")
        );
    }

    #[test]
    fn an_iri_suffix_makes_a_template_object_an_iri() {
        let m = rml(r#"
prefixes: {ex: "http://example.org/"}
mappings:
  product:
    table: products
    s: ex:p_$(id)
    po:
      - [ex:category, "ex:cat_$(category)~iri"]
      - [ex:note, "just text"]
"#);
        let ObjectMap::Term(t) = object(&m, "product", "http://example.org/category") else {
            panic!()
        };
        assert_eq!(t.term_type, crate::rml::model::TermType::IRI);
        assert!(
            matches!(&t.kind, TermMapKind::Template(s) if s == "http://example.org/cat_{category}")
        );
        let ObjectMap::Term(note) = object(&m, "product", "http://example.org/note") else {
            panic!()
        };
        assert!(matches!(&note.kind, TermMapKind::Constant(c) if c == "just text"));
    }

    #[test]
    fn undeclared_prefixes_and_missing_pieces_are_reported_by_name() {
        for (doc, expected) in [
            (
                "mappings:\n  a:\n    table: t\n    s: nope:thing\n",
                "nope:",
            ),
            (
                "prefixes: {ex: \"http://x/\"}\nmappings:\n  a:\n    table: t\n",
                "no subject",
            ),
            (
                "prefixes: {ex: \"http://x/\"}\nmappings: {}\n",
                "no 'mappings'",
            ),
            ("prefixes: {ex: \"http://x/\"}\n", "no 'mappings'"),
            ("- a\n- b\n", "not a map"),
        ] {
            let err = to_rml(doc, Some("legacy")).unwrap_err().to_string();
            assert!(err.contains(expected), "expected {expected:?} in: {err}");
        }
    }

    #[test]
    fn a_mapping_may_not_choose_its_own_graph() {
        let err = to_rml(
            "prefixes: {ex: \"http://x/\"}\nmappings:\n  a:\n    table: t\n    s: ex:a_$(id)\n    g: ex:somewhere\n",
            Some("legacy"),
        )
        .unwrap_err();
        assert!(err.to_string().contains("graph"), "{err}");
    }

    #[test]
    fn two_sources_in_one_mapping_are_refused_with_the_reason() {
        let err = to_rml(
            r#"
prefixes: {ex: "http://x/"}
mappings:
  a:
    sources:
      - {access: one, table: t}
      - {access: two, table: u}
    s: ex:a_$(id)
"#,
            Some("legacy"),
        )
        .unwrap_err();
        assert!(err.to_string().contains("one datasource"), "{err}");
    }
}
