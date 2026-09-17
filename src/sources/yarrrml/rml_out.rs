//! Emitting RML Turtle.
//!
//! Both authoring formats — YARRRML and the legacy `mapping.sql2rdf.yaml` —
//! translate into this small description and are rendered from it, so IRI
//! resolution, Turtle escaping and the R2RML/RML vocabulary are decided once.
//!
//! Prefixes are resolved at translation time and every IRI is written out in
//! full. A mapping is stored as RDF, where prefixes do not survive anyway, and
//! a CURIE that silently resolved against the wrong namespace is a defect that
//! only shows up in the data much later.

use std::collections::BTreeMap;

use crate::store::escape_sparql_literal;

pub const RR: &str = "http://www.w3.org/ns/r2rml#";
pub const RML: &str = "http://semweb.mmlab.be/ns/rml#";
pub const FNML: &str = "http://semweb.mmlab.be/ns/fnml#";
pub const FNO: &str = "https://w3id.org/function/ontology#";
pub const FN: &str = "https://w3id.org/open-triplestore/fn#";
const XSD: &str = "http://www.w3.org/2001/XMLSchema#";
const RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";

/// What a term map produces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TermTypeOut {
    Iri,
    BlankNode,
    Literal,
}

/// How one predicate's object is built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObjectOut {
    /// A column, optionally with a declared datatype or language.
    Column {
        column: String,
        datatype: Option<String>,
        language: Option<String>,
    },
    /// A template over columns.
    Template {
        template: String,
        term_type: TermTypeOut,
    },
    /// A fixed term.
    Constant { value: String, is_iri: bool },
    /// The subject another triples map generates, joined on column pairs.
    Parent {
        mapping: String,
        /// `(child column, parent column)`.
        joins: Vec<(String, String)>,
    },
    /// A value map over an enumeration column.
    Enumeration {
        column: String,
        normalize: String,
        /// `(source value, target IRI)`.
        entries: Vec<(String, String)>,
        unmapped: UnmappedOut,
        datatype: Option<String>,
    },
}

/// What to do with an enumeration value the map does not cover.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnmappedOut {
    /// Keep the raw value as a literal, so a shape reports it.
    Literal,
    Omit,
    /// Mint under an absolute IRI template.
    Template(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PomOut {
    pub predicate: String,
    pub object: ObjectOut,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TriplesMapOut {
    /// Local name; becomes the triples map's IRI under [`MAP_NS`].
    pub name: String,
    /// The datasource IRI (`urn:source:<id>`) this map reads.
    pub source: String,
    pub table: Option<String>,
    pub query: Option<String>,
    pub subject: String,
    pub subject_term_type: TermTypeOut,
    pub classes: Vec<String>,
    pub poms: Vec<PomOut>,
}

/// Namespace the generated triples maps are named under. They are internal
/// document structure, not published terms, so they live under this store's
/// own namespace rather than the author's.
pub const MAP_NS: &str = "https://w3id.org/open-triplestore/mapping#";

pub fn map_iri(name: &str) -> String {
    format!("{MAP_NS}{}", slug(name))
}

/// A local name safe inside an IRI. YARRRML mapping keys are free text.
fn slug(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if s.is_empty() {
        "map".to_string()
    } else {
        s
    }
}

fn lit(s: &str) -> String {
    format!("\"{}\"", escape_sparql_literal(s))
}

/// An IRI reference, refusing anything that would not survive as one.
fn iri(value: &str) -> Result<String, String> {
    oxigraph::model::NamedNode::new(value)
        .map(|n| n.to_string())
        .map_err(|_| format!("'{value}' is not a valid IRI"))
}

/// Expand `prefix:local` against `prefixes`. A value that is already absolute
/// passes through; an undeclared prefix is an error rather than a guess.
pub fn expand(value: &str, prefixes: &BTreeMap<String, String>) -> Result<String, String> {
    let v = value.trim();
    if v.is_empty() {
        return Err("an empty IRI".to_string());
    }
    if v.starts_with('<') && v.ends_with('>') {
        return Ok(v[1..v.len() - 1].to_string());
    }
    if v == "a" {
        return Ok(format!("{RDF}type"));
    }
    if let Some((prefix, local)) = v.split_once(':') {
        // `http://…` and `urn:…` are already absolute; only a declared prefix
        // is expanded, so a typo cannot silently mint an IRI in a namespace
        // nobody owns.
        if local.starts_with("//") || prefix == "urn" || prefix == "mailto" {
            return Ok(v.to_string());
        }
        return match prefixes.get(prefix) {
            Some(ns) => Ok(format!("{ns}{local}")),
            None => Err(format!(
                "prefix '{prefix}:' is used by '{v}' but is not declared in the prefixes block"
            )),
        };
    }
    Err(format!(
        "'{v}' is neither an absolute IRI nor a prefixed name"
    ))
}

/// Translate a YARRRML template into an RML one: `$(column)` becomes
/// `{column}`, and a literal brace is escaped so the RML expander does not read
/// it as a placeholder.
pub fn template_to_rml(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            // `\$` is YARRRML's escape for a literal dollar.
            '\\' if chars.peek() == Some(&'$') => {
                chars.next();
                out.push('$');
            }
            '$' if chars.peek() == Some(&'(') => {
                chars.next();
                let mut col = String::new();
                for inner in chars.by_ref() {
                    if inner == ')' {
                        break;
                    }
                    col.push(inner);
                }
                out.push('{');
                out.push_str(&col);
                out.push('}');
            }
            // A brace that is not ours has to survive the RML expander.
            '{' => out.push_str("\\{"),
            '}' => out.push_str("\\}"),
            _ => out.push(c),
        }
    }
    out
}

/// Render triples maps as an RML document.
pub fn render(maps: &[TriplesMapOut]) -> Result<String, String> {
    let mut out = String::new();
    out.push_str(&format!("@prefix rr:   <{RR}> .\n"));
    out.push_str(&format!("@prefix rml:  <{RML}> .\n"));
    out.push_str(&format!("@prefix fnml: <{FNML}> .\n"));
    out.push_str(&format!("@prefix fno:  <{FNO}> .\n"));
    out.push_str(&format!("@prefix fn:   <{FN}> .\n\n"));

    for m in maps {
        if m.table.is_none() && m.query.is_none() {
            return Err(format!(
                "mapping '{}' names no table and no query, so there is nothing to read",
                m.name
            ));
        }
        out.push_str(&format!("{} a rr:TriplesMap ;\n", iri(&map_iri(&m.name))?));
        out.push_str("  rml:logicalSource [\n");
        out.push_str(&format!("    rml:source {} ;\n", iri(&m.source)?));
        if let Some(q) = &m.query {
            out.push_str(&format!("    rml:query {}\n", lit(q)));
        } else if let Some(t) = &m.table {
            out.push_str(&format!("    rr:tableName {}\n", lit(t)));
        }
        out.push_str("  ] ;\n");

        out.push_str("  rr:subjectMap [\n");
        match m.subject_term_type {
            TermTypeOut::BlankNode => {
                out.push_str(&format!("    rr:template {} ;\n", lit(&m.subject)));
                out.push_str("    rr:termType rr:BlankNode ;\n");
            }
            _ => out.push_str(&format!("    rr:template {} ;\n", lit(&m.subject))),
        }
        for c in &m.classes {
            out.push_str(&format!("    rr:class {} ;\n", iri(&expand_done(c)?)?));
        }
        // Trim the trailing separator the loop above always leaves.
        trim_separator(&mut out);
        out.push_str("\n  ] ;\n");

        for pom in &m.poms {
            out.push_str("  rr:predicateObjectMap [\n");
            out.push_str(&format!("    rr:predicate {} ;\n", iri(&pom.predicate)?));
            render_object(&mut out, &pom.object)?;
            out.push_str("  ] ;\n");
        }
        trim_separator(&mut out);
        out.push_str(" .\n\n");
    }
    Ok(out)
}

/// A class IRI has already been expanded by the translator; this only guards
/// against an empty one slipping through.
fn expand_done(value: &str) -> Result<String, String> {
    if value.trim().is_empty() {
        Err("an empty class IRI".to_string())
    } else {
        Ok(value.to_string())
    }
}

/// Drop the `;` the emitters unconditionally append, so the last clause in a
/// block is terminated correctly.
fn trim_separator(out: &mut String) {
    while out.ends_with('\n') || out.ends_with(' ') {
        out.pop();
    }
    if out.ends_with(';') {
        out.pop();
    }
}

fn render_object(out: &mut String, object: &ObjectOut) -> Result<(), String> {
    match object {
        ObjectOut::Column {
            column,
            datatype,
            language,
        } => {
            out.push_str(&format!("    rr:objectMap [ rr:column {}", lit(column)));
            if let Some(d) = datatype {
                out.push_str(&format!(" ; rr:datatype {}", iri(d)?));
            } else if let Some(l) = language {
                out.push_str(&format!(" ; rr:language {}", lit(l)));
            }
            out.push_str(" ]\n");
        }
        ObjectOut::Template {
            template,
            term_type,
        } => {
            out.push_str(&format!("    rr:objectMap [ rr:template {}", lit(template)));
            match term_type {
                TermTypeOut::Literal => out.push_str(" ; rr:termType rr:Literal"),
                TermTypeOut::BlankNode => out.push_str(" ; rr:termType rr:BlankNode"),
                TermTypeOut::Iri => out.push_str(" ; rr:termType rr:IRI"),
            }
            out.push_str(" ]\n");
        }
        ObjectOut::Constant { value, is_iri } => {
            if *is_iri {
                out.push_str(&format!("    rr:object {}\n", iri(value)?));
            } else {
                out.push_str(&format!("    rr:object {}\n", lit(value)));
            }
        }
        ObjectOut::Parent { mapping, joins } => {
            out.push_str(&format!(
                "    rr:objectMap [ rr:parentTriplesMap {}",
                iri(&map_iri(mapping))?
            ));
            for (child, parent) in joins {
                out.push_str(&format!(
                    " ;\n      rr:joinCondition [ rr:child {} ; rr:parent {} ]",
                    lit(child),
                    lit(parent)
                ));
            }
            out.push_str(" ]\n");
        }
        ObjectOut::Enumeration {
            column,
            normalize,
            entries,
            unmapped,
            datatype,
        } => {
            out.push_str("    rr:objectMap [\n");
            if let Some(d) = datatype {
                out.push_str(&format!("      rr:datatype {} ;\n", iri(d)?));
            }
            out.push_str("      fnml:functionValue [\n");
            out.push_str("        rr:predicateObjectMap [ rr:predicate fno:executes ; rr:object fn:mapValue ] ;\n");
            out.push_str(&format!(
                "        rr:predicateObjectMap [ rr:predicate fn:value ; rr:objectMap [ rr:column {} ] ] ;\n",
                lit(column)
            ));
            out.push_str(&format!(
                "        rr:predicateObjectMap [ rr:predicate fn:normalize ; rr:object {} ] ;\n",
                lit(normalize)
            ));
            for (value, target) in entries {
                out.push_str(&format!(
                    "        rr:predicateObjectMap [ rr:predicate fn:mapping ; rr:object {} ] ;\n",
                    lit(&format!("{value}={target}"))
                ));
            }
            let policy = match unmapped {
                UnmappedOut::Literal => "literal",
                UnmappedOut::Omit => "omit",
                UnmappedOut::Template(_) => "template",
            };
            out.push_str(&format!(
                "        rr:predicateObjectMap [ rr:predicate fn:unmapped ; rr:object {} ]",
                lit(policy)
            ));
            if let UnmappedOut::Template(t) = unmapped {
                out.push_str(&format!(
                    " ;\n        rr:predicateObjectMap [ rr:predicate fn:unmappedTemplate ; rr:object {} ]",
                    lit(t)
                ));
            }
            out.push_str("\n      ]\n    ]\n");
        }
    }
    Ok(())
}

/// The XSD datatype a short name refers to (`integer`, `xsd:decimal`, or an
/// absolute IRI).
pub fn datatype_iri(value: &str, prefixes: &BTreeMap<String, String>) -> Result<String, String> {
    let v = value.trim();
    if v.contains("://") || v.starts_with("urn:") {
        return Ok(v.to_string());
    }
    if let Some((p, local)) = v.split_once(':') {
        if let Some(ns) = prefixes.get(p) {
            return Ok(format!("{ns}{local}"));
        }
        if p == "xsd" {
            return Ok(format!("{XSD}{local}"));
        }
        return Err(format!("prefix '{p}:' in datatype '{v}' is not declared"));
    }
    Ok(format!("{XSD}{v}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prefixes() -> BTreeMap<String, String> {
        BTreeMap::from([
            ("ex".to_string(), "http://example.org/".to_string()),
            ("xsd".to_string(), XSD.to_string()),
        ])
    }

    #[test]
    fn templates_convert_placeholders_and_protect_literal_braces() {
        assert_eq!(template_to_rml("http://x/$(id)"), "http://x/{id}");
        assert_eq!(template_to_rml("http://x/$(a)/$(b)"), "http://x/{a}/{b}");
        assert_eq!(template_to_rml("no placeholders"), "no placeholders");
        assert_eq!(
            template_to_rml("http://x/{literal}/$(id)"),
            "http://x/\\{literal\\}/{id}",
            "a brace the author wrote must not become a placeholder"
        );
        assert_eq!(
            template_to_rml(r"cost \$(5)"),
            "cost $(5)",
            "an escaped dollar is literal"
        );
    }

    #[test]
    fn expansion_refuses_an_undeclared_prefix_rather_than_guessing() {
        let p = prefixes();
        assert_eq!(
            expand("ex:Product", &p).unwrap(),
            "http://example.org/Product"
        );
        assert_eq!(expand("http://x/y", &p).unwrap(), "http://x/y");
        assert_eq!(
            expand("urn:source:legacy", &p).unwrap(),
            "urn:source:legacy"
        );
        assert_eq!(expand("<http://x/y>", &p).unwrap(), "http://x/y");
        assert_eq!(expand("a", &p).unwrap(), format!("{RDF}type"));
        let err = expand("nope:Thing", &p).unwrap_err();
        assert!(err.contains("not declared"), "{err}");
        assert!(expand("bare", &p).is_err());
        assert!(expand("  ", &p).is_err());
    }

    #[test]
    fn datatypes_accept_a_short_name_a_curie_or_an_iri() {
        let p = prefixes();
        assert_eq!(
            datatype_iri("integer", &p).unwrap(),
            format!("{XSD}integer")
        );
        assert_eq!(
            datatype_iri("xsd:decimal", &p).unwrap(),
            format!("{XSD}decimal")
        );
        assert_eq!(
            datatype_iri("ex:Custom", &p).unwrap(),
            "http://example.org/Custom"
        );
        assert_eq!(datatype_iri("http://x/dt", &p).unwrap(), "http://x/dt");
        assert!(datatype_iri("nope:dt", &p).is_err());
    }

    #[test]
    fn mapping_names_become_safe_iris() {
        assert_eq!(map_iri("person"), format!("{MAP_NS}person"));
        assert_eq!(map_iri("a b/c"), format!("{MAP_NS}a_b_c"));
        assert_eq!(map_iri(""), format!("{MAP_NS}map"));
    }

    fn sample() -> TriplesMapOut {
        TriplesMapOut {
            name: "product".into(),
            source: "urn:source:legacy".into(),
            table: Some("products".into()),
            query: None,
            subject: "http://example.org/p{product_id}".into(),
            subject_term_type: TermTypeOut::Iri,
            classes: vec!["http://example.org/Product".into()],
            poms: vec![
                PomOut {
                    predicate: "http://example.org/name".into(),
                    object: ObjectOut::Column {
                        column: "name".into(),
                        datatype: None,
                        language: None,
                    },
                },
                PomOut {
                    predicate: "http://example.org/price".into(),
                    object: ObjectOut::Column {
                        column: "price".into(),
                        datatype: Some(format!("{XSD}decimal")),
                        language: None,
                    },
                },
            ],
        }
    }

    /// The emitter's whole contract: whatever it writes, this engine parses.
    fn parses(turtle: &str) -> crate::rml::model::RmlMapping {
        crate::rml::parse_rml(turtle)
            .unwrap_or_else(|e| panic!("emitted RML does not parse: {e}\n---\n{turtle}"))
    }

    #[test]
    fn a_rendered_mapping_parses_back_into_the_engine() {
        let turtle = render(&[sample()]).unwrap();
        let m = parses(&turtle);
        assert_eq!(m.triples_maps.len(), 1);
        let tm = &m.triples_maps[0];
        assert_eq!(tm.logical_source.table_name.as_deref(), Some("products"));
        assert_eq!(m.datasources(), vec!["urn:source:legacy"]);
        assert_eq!(tm.subject_map.classes, vec!["http://example.org/Product"]);
        assert_eq!(tm.predicate_object_maps.len(), 2);
    }

    #[test]
    fn a_join_renders_as_a_referencing_object_map() {
        let mut child = sample();
        child.poms.push(PomOut {
            predicate: "http://example.org/suppliedBy".into(),
            object: ObjectOut::Parent {
                mapping: "supplier".into(),
                joins: vec![("supplier_id".into(), "supplier_id".into())],
            },
        });
        let parent = TriplesMapOut {
            name: "supplier".into(),
            table: Some("suppliers".into()),
            subject: "http://example.org/s{supplier_id}".into(),
            classes: vec![],
            poms: vec![],
            ..sample()
        };
        let turtle = render(&[child, parent]).unwrap();
        let m = parses(&turtle);
        let child = m.find(&map_iri("product")).expect("child map");
        let pom = child
            .predicate_object_maps
            .iter()
            .find(|p| matches!(p.object, crate::rml::model::ObjectMap::Ref(_)))
            .expect("a referencing object map");
        let crate::rml::model::ObjectMap::Ref(r) = &pom.object else {
            unreachable!()
        };
        assert_eq!(r.parent_triples_map, map_iri("supplier"));
        assert_eq!(r.joins.len(), 1);
    }

    #[test]
    fn an_enumeration_renders_as_the_value_map_function() {
        let mut m = sample();
        m.poms.push(PomOut {
            predicate: "http://example.org/status".into(),
            object: ObjectOut::Enumeration {
                column: "status".into(),
                normalize: "lower_trim".into(),
                entries: vec![("active".into(), "http://example.org/Active".into())],
                unmapped: UnmappedOut::Literal,
                datatype: None,
            },
        });
        let turtle = render(&[m]).unwrap();
        let parsed = parses(&turtle);
        let tm = &parsed.triples_maps[0];
        let f = tm
            .predicate_object_maps
            .iter()
            .find_map(|p| match &p.object {
                crate::rml::model::ObjectMap::Function(f) => Some(f),
                _ => None,
            })
            .expect("a function object map");
        assert_eq!(f.function, crate::rml::terms::FN_MAP_VALUE);
        assert_eq!(f.all(&format!("{FN}mapping")).len(), 1);
    }

    #[test]
    fn a_mapping_with_no_table_or_query_is_refused() {
        let m = TriplesMapOut {
            table: None,
            query: None,
            ..sample()
        };
        let err = render(&[m]).unwrap_err();
        assert!(err.contains("nothing to read"), "{err}");
    }

    #[test]
    fn hostile_values_cannot_break_the_emitted_document() {
        let mut m = sample();
        m.subject = "http://x/{a\"b}".into();
        m.poms[0] = PomOut {
            predicate: "http://example.org/n".into(),
            object: ObjectOut::Column {
                column: "col\"with\nnewline".into(),
                datatype: None,
                language: None,
            },
        };
        let turtle = render(&[m]).unwrap();
        // It still parses: the quote and newline were escaped rather than
        // terminating the literal.
        parses(&turtle);
        assert!(!turtle.contains("col\"with\nnewline"));
    }

    #[test]
    fn an_invalid_iri_is_refused_rather_than_emitted() {
        let mut m = sample();
        m.poms[0].predicate = "not an iri".into();
        assert!(render(&[m]).unwrap_err().contains("not a valid IRI"));
    }
}
