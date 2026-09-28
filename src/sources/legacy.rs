//! The legacy `mapping.sql2rdf.yaml` format, converted to RML once.
//!
//! An earlier generation of this process kept its mappings in a bespoke YAML
//! bundle: one `entities` list, each entity a table with a subject template,
//! a class, a list of properties and optional `nested` maps over the same row.
//! This module reads that format and renders the same [`TriplesMapOut`]
//! description YARRRML translates into, so the vocabulary decisions — how a
//! join is written, how an enumeration becomes a function — are made once.
//!
//! What each construct becomes:
//!
//! | legacy                                | RML                                                    |
//! |---------------------------------------|--------------------------------------------------------|
//! | `subject_iri` / `rdf_type`            | `rr:subjectMap [ rr:template … ; rr:class … ]`         |
//! | `{column}` / `{column_slug}`          | `{column}` in a template; a slug placeholder makes the |
//! |                                       | term an `otsfn:mintIri` function, since `rr:template`  |
//! |                                       | cannot slug                                            |
//! | property with `datatype`              | `rr:objectMap [ rr:column … ; rr:datatype … ]`         |
//! | `object: {kind: lookup, …}`           | a template object map, plus a second triples map on   |
//! |                                       | the same logical source that types and labels the     |
//! |                                       | minted node                                            |
//! | `object: {kind: reference, …}`        | `rr:parentTriplesMap` with a join on the entity whose  |
//! |                                       | subject the template names, else a plain template     |
//! | `object: {kind: enumeration, …}`      | the `otsfn:mapValue` function                          |
//! | `nested`                              | a second triples map on the same logical source,      |
//! |                                       | linked from the parent by a template object map       |
//!
//! **Empty cells.** The legacy transformer emitted nothing for an empty cell.
//! So does this store's engine — a term map over an empty value yields no
//! term — so a converted mapping reproduces the legacy output here as it
//! stands, with `rr:tableName` logical sources that keep join pushdown and
//! watermark runs available. R2RML proper says an empty cell is an empty
//! literal, and a mapping that has to behave the same under another
//! processor can be converted with `emptyAsNull=true`: the logical source
//! then becomes a query reading each text-valued column through
//! `NULLIF(col, '')`, which is opaque to the catalogue, so joins are indexed
//! rather than pushed down and watermark runs are not available. Either way
//! the choice is reported back as a warning.

use std::collections::{BTreeMap, BTreeSet};

use axum::extract::State;
use axum::http::StatusCode;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::server::AppState;

use super::model::source_iri;
use super::yarrrml::node::Node;
use super::yarrrml::rml_out::{
    self, expand, ObjectOut, PomOut, TermTypeOut, TriplesMapOut, UnmappedOut,
};
use super::{mappings, registry};

const XSD_STRING: &str = "http://www.w3.org/2001/XMLSchema#string";

/// Why a legacy document could not be converted.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LegacyError {
    #[error("the document is not valid YAML: {0}")]
    Yaml(String),
    #[error("{0}")]
    Document(String),
    #[error("entity '{entity}': {reason}")]
    Entity { entity: String, reason: String },
}

fn e_err(entity: &str, reason: impl Into<String>) -> LegacyError {
    LegacyError::Entity {
        entity: entity.to_string(),
        reason: reason.into(),
    }
}

/// What a conversion produced.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Converted {
    pub rml: String,
    pub triples_maps: usize,
    pub warnings: Vec<String>,
}

/// Prefixes every legacy bundle could use without declaring them.
fn well_known_prefixes() -> BTreeMap<String, String> {
    [
        ("rdf", "http://www.w3.org/1999/02/22-rdf-syntax-ns#"),
        ("rdfs", "http://www.w3.org/2000/01/rdf-schema#"),
        ("xsd", "http://www.w3.org/2001/XMLSchema#"),
        ("owl", "http://www.w3.org/2002/07/owl#"),
        ("skos", "http://www.w3.org/2004/02/skos/core#"),
        ("dct", "http://purl.org/dc/terms/"),
        ("dcterms", "http://purl.org/dc/terms/"),
        ("schema", "https://schema.org/"),
        ("foaf", "http://xmlns.com/foaf/0.1/"),
        ("prov", "http://www.w3.org/ns/prov#"),
        ("geo", "http://www.opengis.net/ont/geosparql#"),
    ]
    .into_iter()
    .map(|(l, n)| (l.to_string(), n.to_string()))
    .collect()
}

/// Convert a legacy document for `source_id`.
pub fn convert(yaml: &str, source_id: &str, empty_as_null: bool) -> Result<Converted, LegacyError> {
    let doc = super::yarrrml::yaml::parse(yaml).map_err(LegacyError::Yaml)?;
    let Node::Map(_) = doc else {
        return Err(LegacyError::Document(format!(
            "the document is {}, not a map — a mapping bundle's top level is a map",
            doc.kind()
        )));
    };

    let mut prefixes = well_known_prefixes();
    if let Some(Node::Map(m)) = doc.get("prefixes") {
        for (label, value) in m {
            let ns = value.as_str().ok_or_else(|| {
                LegacyError::Document(format!(
                    "prefix '{label}' maps to {}, but a namespace is a string",
                    value.kind()
                ))
            })?;
            prefixes.insert(label.clone(), ns.trim().to_string());
        }
    }

    let entities: Vec<&Node> = doc
        .get("entities")
        .map(Node::as_seq)
        .ok_or_else(|| LegacyError::Document("the document declares no 'entities'".to_string()))?;
    if entities.is_empty() {
        return Err(LegacyError::Document(
            "the document declares no 'entities'".to_string(),
        ));
    }

    let parsed: Vec<Entity> = entities
        .iter()
        .enumerate()
        .map(|(i, e)| Entity::read(e, i, &prefixes))
        .collect::<Result<_, _>>()?;

    let mut names: BTreeSet<String> = BTreeSet::new();
    let mut maps: Vec<TriplesMapOut> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let source = source_iri(source_id);
    for entity in &parsed {
        let name = unique_name(&mut names, &entity.table);
        entity.render(
            &name,
            &parsed,
            &source,
            empty_as_null,
            &mut names,
            &mut maps,
        )?;
    }
    warnings.push(if empty_as_null {
        "logical sources are queries reading text columns through NULLIF(col, ''), so an empty \
         cell produces no triple under any RML processor; a query is opaque to the catalogue, \
         so joins are indexed rather than pushed down and watermark runs are not available — \
         convert with emptyAsNull=false to keep rr:tableName"
            .to_string()
    } else {
        "logical sources are rr:tableName; this store's engine emits no term for an empty cell, \
         as the legacy transformer did, but a standard RML processor would emit an empty \
         literal — convert with emptyAsNull=true for a mapping that behaves the same everywhere"
            .to_string()
    });

    let rml = rml_out::render(&maps).map_err(LegacyError::Document)?;
    // What was rendered must be what the engine reads back.
    crate::rml::parse_rml(&rml)
        .map_err(|e| LegacyError::Document(format!("the converted RML does not parse: {e}")))?;
    Ok(Converted {
        triples_maps: maps.len(),
        rml,
        warnings,
    })
}

fn unique_name(names: &mut BTreeSet<String>, base: &str) -> String {
    let mut name = base.to_string();
    let mut n = 2;
    while !names.insert(name.clone()) {
        name = format!("{base}_{n}");
        n += 1;
    }
    name
}

/// One `entities` item.
struct Entity {
    label: String,
    table: String,
    primary_key: Option<String>,
    subject: String,
    class: Option<String>,
    properties: Vec<Property>,
    nested: Vec<Nested>,
}

struct Nested {
    predicate: String,
    subject: String,
    class: Option<String>,
    properties: Vec<Property>,
}

struct Property {
    column: String,
    predicate: String,
    object: PropObject,
}

enum PropObject {
    Literal {
        datatype: Option<String>,
    },
    Lookup {
        template: String,
        class: Option<String>,
        label_predicate: Option<String>,
        label_from: Option<String>,
    },
    Reference {
        template: String,
    },
    Enumeration {
        normalize: String,
        entries: Vec<(String, String)>,
        unmapped: UnmappedOut,
    },
}

impl Entity {
    fn read(
        node: &Node,
        index: usize,
        prefixes: &BTreeMap<String, String>,
    ) -> Result<Self, LegacyError> {
        let label = node
            .str_at(&["source_table", "table"])
            .unwrap_or_else(|| format!("entity #{}", index + 1));
        let table = node
            .str_at(&["source_table", "table"])
            .ok_or_else(|| e_err(&label, "needs a 'source_table'"))?;
        let subject = node
            .str_at(&["subject_iri", "subject"])
            .ok_or_else(|| e_err(&label, "needs a 'subject_iri' template"))?;
        let subject = expand_legacy_template(&subject, prefixes).map_err(|r| e_err(&label, r))?;
        let class = node
            .str_at(&["rdf_type", "class"])
            .map(|c| expand(&c, prefixes))
            .transpose()
            .map_err(|r| e_err(&label, r))?;
        let properties = read_properties(node, &label, prefixes)?;
        let mut nested = Vec::new();
        for n in node.get("nested").map(Node::as_seq).unwrap_or_default() {
            let predicate = n
                .str_at(&["predicate"])
                .ok_or_else(|| e_err(&label, "a nested map needs a 'predicate'"))?;
            let predicate = expand(&predicate, prefixes).map_err(|r| e_err(&label, r))?;
            let subject = n
                .str_at(&["subject_iri", "subject"])
                .ok_or_else(|| e_err(&label, "a nested map needs a 'subject_iri' template"))?;
            let subject =
                expand_legacy_template(&subject, prefixes).map_err(|r| e_err(&label, r))?;
            let class = n
                .str_at(&["rdf_type", "class"])
                .map(|c| expand(&c, prefixes))
                .transpose()
                .map_err(|r| e_err(&label, r))?;
            nested.push(Nested {
                predicate,
                subject,
                class,
                properties: read_properties(n, &label, prefixes)?,
            });
        }
        Ok(Entity {
            label,
            table,
            primary_key: node.str_at(&["primary_key"]),
            subject,
            class,
            properties,
            nested,
        })
    }

    /// The entity whose subject template a reference template names, once
    /// `{value}` stands for that entity's primary key.
    fn referenced<'a>(&self, template: &str, all: &'a [Entity]) -> Option<&'a Entity> {
        all.iter().find(|e| {
            e.primary_key
                .as_deref()
                .is_some_and(|pk| template.replace("{value}", &format!("{{{pk}}}")) == e.subject)
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn render(
        &self,
        name: &str,
        all: &[Entity],
        source: &str,
        empty_as_null: bool,
        names: &mut BTreeSet<String>,
        maps: &mut Vec<TriplesMapOut>,
    ) -> Result<(), LegacyError> {
        let (table, query) = self.logical_source(empty_as_null);
        let mut poms = Vec::new();
        let mut extra: Vec<TriplesMapOut> = Vec::new();

        for p in &self.properties {
            let object = match &p.object {
                PropObject::Literal { datatype } => ObjectOut::Column {
                    column: p.column.clone(),
                    datatype: datatype.clone(),
                    language: None,
                },
                PropObject::Lookup {
                    template,
                    class,
                    label_predicate,
                    label_from,
                } => {
                    let template = value_placeholders(template, &p.column);
                    // The node the lookup mints is typed and labelled by its
                    // own triples map over the same rows.
                    let mut lookup_poms = Vec::new();
                    if let (Some(pred), Some(from)) = (label_predicate, label_from) {
                        let column = if from == "value" {
                            p.column.clone()
                        } else {
                            from.clone()
                        };
                        lookup_poms.push(PomOut {
                            predicate: pred.clone(),
                            object: ObjectOut::Column {
                                column,
                                datatype: None,
                                language: None,
                            },
                        });
                    }
                    if class.is_some() || !lookup_poms.is_empty() {
                        extra.push(TriplesMapOut {
                            name: unique_name(names, &format!("{name}_{}", p.column)),
                            source: source.to_string(),
                            table: table.clone(),
                            query: query.clone(),
                            subject: template.clone(),
                            subject_term_type: TermTypeOut::Iri,
                            subject_mint: has_slug(&template),
                            classes: class.iter().cloned().collect(),
                            poms: lookup_poms,
                        });
                    }
                    iri_object(template)
                }
                PropObject::Reference { template } => match self.referenced(template, all) {
                    Some(parent) => ObjectOut::Parent {
                        mapping: parent.table.clone(),
                        joins: vec![(
                            p.column.clone(),
                            parent.primary_key.clone().expect("matched on the key"),
                        )],
                    },
                    None => iri_object(value_placeholders(template, &p.column)),
                },
                PropObject::Enumeration {
                    normalize,
                    entries,
                    unmapped,
                } => ObjectOut::Enumeration {
                    column: p.column.clone(),
                    normalize: normalize.clone(),
                    entries: entries.clone(),
                    unmapped: unmapped.clone(),
                    datatype: None,
                },
            };
            poms.push(PomOut {
                predicate: p.predicate.clone(),
                object,
            });
        }

        for n in &self.nested {
            let nested_name = unique_name(names, &format!("{name}_{}", local_name(&n.predicate)));
            poms.push(PomOut {
                predicate: n.predicate.clone(),
                object: iri_object(n.subject.clone()),
            });
            let mut nested_poms = Vec::new();
            for p in &n.properties {
                let object = match &p.object {
                    PropObject::Literal { datatype } => ObjectOut::Column {
                        column: p.column.clone(),
                        datatype: datatype.clone(),
                        language: None,
                    },
                    _ => {
                        return Err(e_err(
                            &self.label,
                            format!(
                                "nested map '{}': property '{}' is not a plain column; nested maps carry literals only",
                                n.predicate, p.column
                            ),
                        ))
                    }
                };
                nested_poms.push(PomOut {
                    predicate: p.predicate.clone(),
                    object,
                });
            }
            extra.push(TriplesMapOut {
                name: nested_name,
                source: source.to_string(),
                table: table.clone(),
                query: query.clone(),
                subject: n.subject.clone(),
                subject_term_type: TermTypeOut::Iri,
                subject_mint: has_slug(&n.subject),
                classes: n.class.iter().cloned().collect(),
                poms: nested_poms,
            });
        }

        maps.push(TriplesMapOut {
            name: name.to_string(),
            source: source.to_string(),
            table,
            query,
            subject: self.subject.clone(),
            subject_term_type: TermTypeOut::Iri,
            subject_mint: has_slug(&self.subject),
            classes: self.class.iter().cloned().collect(),
            poms,
        });
        maps.extend(extra);
        Ok(())
    }

    /// `rr:tableName`, or the NULLIF query over every column the entity reads.
    fn logical_source(&self, empty_as_null: bool) -> (Option<String>, Option<String>) {
        if !empty_as_null {
            return (Some(self.table.clone()), None);
        }
        // Columns read through a template are keys and codes; only a column
        // that becomes a text literal, a lookup value or an enumeration input
        // is read through NULLIF, so a numeric column never meets a string
        // comparison the dialect would refuse.
        let mut columns: BTreeSet<String> = BTreeSet::new();
        let mut nullable: BTreeSet<String> = BTreeSet::new();
        let mut visit = |props: &[Property]| {
            for p in props {
                columns.insert(p.column.clone());
                let text = match &p.object {
                    PropObject::Literal { datatype } => {
                        datatype.as_deref().is_none_or(|d| d == XSD_STRING)
                    }
                    PropObject::Lookup { .. } | PropObject::Enumeration { .. } => true,
                    PropObject::Reference { .. } => false,
                };
                if text {
                    nullable.insert(p.column.clone());
                }
            }
        };
        visit(&self.properties);
        for n in &self.nested {
            visit(&n.properties);
        }
        for template in std::iter::once(&self.subject).chain(self.nested.iter().map(|n| &n.subject))
        {
            for c in template_columns_base(template) {
                columns.insert(c);
            }
        }
        for p in &self.properties {
            if let PropObject::Lookup { template, .. } | PropObject::Reference { template } =
                &p.object
            {
                for c in template_columns_base(&value_placeholders(template, &p.column)) {
                    columns.insert(c);
                }
            }
        }
        let projection: Vec<String> = columns
            .iter()
            .map(|c| {
                let q = quote(c);
                if nullable.contains(c) {
                    format!("NULLIF({q}, '') AS {q}")
                } else {
                    q
                }
            })
            .collect();
        (
            None,
            Some(format!(
                "SELECT {} FROM {}",
                projection.join(", "),
                quote(&self.table)
            )),
        )
    }
}

fn read_properties(
    node: &Node,
    entity: &str,
    prefixes: &BTreeMap<String, String>,
) -> Result<Vec<Property>, LegacyError> {
    let mut out = Vec::new();
    for p in node.get("properties").map(Node::as_seq).unwrap_or_default() {
        let column = p
            .str_at(&["column"])
            .ok_or_else(|| e_err(entity, "a property needs a 'column'"))?;
        let predicate = p
            .str_at(&["predicate"])
            .ok_or_else(|| e_err(entity, format!("property '{column}' needs a 'predicate'")))?;
        let predicate = expand(&predicate, prefixes).map_err(|r| e_err(entity, r))?;
        let object = match p.get("object") {
            None => PropObject::Literal {
                datatype: p
                    .str_at(&["datatype"])
                    .map(|d| expand(&d, prefixes))
                    .transpose()
                    .map_err(|r| e_err(entity, r))?,
            },
            Some(o) => {
                let kind = o.str_at(&["kind"]).ok_or_else(|| {
                    e_err(
                        entity,
                        format!("property '{column}': object needs a 'kind'"),
                    )
                })?;
                match kind.as_str() {
                    "lookup" => PropObject::Lookup {
                        template: legacy_object_template(o, entity, &column, prefixes)?,
                        class: o
                            .str_at(&["rdf_type", "class"])
                            .map(|c| expand(&c, prefixes))
                            .transpose()
                            .map_err(|r| e_err(entity, r))?,
                        label_predicate: o
                            .str_at(&["label_predicate"])
                            .map(|c| expand(&c, prefixes))
                            .transpose()
                            .map_err(|r| e_err(entity, r))?,
                        label_from: o.str_at(&["label_from"]),
                    },
                    "reference" => PropObject::Reference {
                        template: legacy_object_template(o, entity, &column, prefixes)?,
                    },
                    "enumeration" => {
                        let mut entries = Vec::new();
                        if let Some(Node::Map(m)) = o.get_any(&["value_map", "values"]) {
                            for (value, target) in m {
                                let t = target.as_str().ok_or_else(|| {
                                    e_err(entity, format!("property '{column}': value_map entry '{value}' is not an IRI"))
                                })?;
                                entries.push((
                                    value.clone(),
                                    expand(t, prefixes).map_err(|r| e_err(entity, r))?,
                                ));
                            }
                        }
                        let unmapped = match o.str_at(&["unmapped"]).as_deref() {
                            None | Some("literal") => UnmappedOut::Literal,
                            Some("omit") | Some("skip") => UnmappedOut::Omit,
                            Some("template") | Some("mint") => {
                                let t = o.str_at(&["unmapped_template", "template"]).ok_or_else(|| {
                                    e_err(entity, format!("property '{column}': unmapped 'template' needs an 'unmapped_template'"))
                                })?;
                                UnmappedOut::Template(
                                    expand_legacy_template(&t, prefixes).map_err(|r| e_err(entity, r))?,
                                )
                            }
                            Some(other) => {
                                return Err(e_err(
                                    entity,
                                    format!("property '{column}': unknown unmapped policy '{other}'; expected literal, omit or template"),
                                ))
                            }
                        };
                        PropObject::Enumeration {
                            normalize: o.str_at(&["normalize"]).unwrap_or_else(|| "none".to_string()),
                            entries,
                            unmapped,
                        }
                    }
                    other => {
                        return Err(e_err(
                            entity,
                            format!("property '{column}': unknown object kind '{other}'; expected lookup, reference or enumeration"),
                        ))
                    }
                }
            }
        };
        out.push(Property {
            column,
            predicate,
            object,
        });
    }
    Ok(out)
}

fn legacy_object_template(
    o: &Node,
    entity: &str,
    column: &str,
    prefixes: &BTreeMap<String, String>,
) -> Result<String, LegacyError> {
    let t = o.str_at(&["iri", "template"]).ok_or_else(|| {
        e_err(
            entity,
            format!("property '{column}': object needs an 'iri' template"),
        )
    })?;
    expand_legacy_template(&t, prefixes).map_err(|r| e_err(entity, r))
}

/// Expand the prefix of a legacy template: `prod:product_{product_id}` becomes
/// `http://example.org/products/product_{product_id}`. A literal brace that is
/// not a placeholder is not a thing the legacy format had, so none is escaped.
fn expand_legacy_template(
    value: &str,
    prefixes: &BTreeMap<String, String>,
) -> Result<String, String> {
    let v = value.trim().trim_matches(['<', '>']);
    let head = v.split('{').next().unwrap_or("");
    if head.contains("://") || head.starts_with("urn:") {
        return Ok(v.to_string());
    }
    let Some((prefix, _)) = head.split_once(':') else {
        return Err(format!(
            "'{v}' is neither an absolute IRI template nor a prefixed one"
        ));
    };
    let ns = prefixes.get(prefix).ok_or_else(|| {
        format!("prefix '{prefix}:' is used by '{v}' but is not declared in the prefixes block")
    })?;
    Ok(v.replacen(&format!("{prefix}:"), ns, 1))
}

/// `{value}` and `{value_slug}` name the property's own column.
fn value_placeholders(template: &str, column: &str) -> String {
    template
        .replace("{value_slug}", &format!("{{{column}_slug}}"))
        .replace("{value}", &format!("{{{column}}}"))
}

fn has_slug(template: &str) -> bool {
    crate::rml::model::template_columns(template)
        .iter()
        .any(|c| c.ends_with("_slug"))
}

/// A template as an object: minted when it slugs, a plain template otherwise.
fn iri_object(template: String) -> ObjectOut {
    if has_slug(&template) {
        ObjectOut::MintIri { template }
    } else {
        ObjectOut::Template {
            template,
            term_type: TermTypeOut::Iri,
        }
    }
}

/// The columns a template reads, with `_slug` stripped.
fn template_columns_base(template: &str) -> Vec<String> {
    crate::rml::model::template_columns(template)
        .into_iter()
        .map(|c| c.strip_suffix("_slug").map(str::to_string).unwrap_or(c))
        .collect()
}

fn local_name(iri: &str) -> String {
    let local = iri.rsplit(['#', '/']).next().unwrap_or(iri);
    let s: String = local
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    if s.is_empty() {
        "nested".to_string()
    } else {
        s
    }
}

/// Standard SQL quoting. The converter runs without a connection, so it
/// cannot ask a dialect; every dialect this store supports reads the
/// double-quoted form.
fn quote(ident: &str) -> String {
    format!("\"{}\"", ident.replace('"', "\"\""))
}

// ─────────────────────────────── HTTP ───────────────────────────────

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConvertRequest {
    /// `sql2rdf` — the only legacy format there is.
    pub format: String,
    /// The datasource the converted mapping reads.
    pub source: String,
    /// The YAML document.
    pub document: String,
    /// Read text columns through `NULLIF(col, '')` so an empty cell produces
    /// no triple under any RML processor. Default false: this store's engine
    /// already emits nothing for an empty cell, and `rr:tableName` keeps join
    /// pushdown and watermark runs available.
    #[serde(default)]
    pub empty_as_null: bool,
}

/// `POST /api/mappings/convert` — a legacy bundle as RML, registered nowhere.
pub async fn convert_mapping(
    State(state): State<AppState>,
    Json(body): Json<ConvertRequest>,
) -> Result<Json<Converted>, (StatusCode, String)> {
    if body.format.trim() != "sql2rdf" {
        return Err((
            StatusCode::BAD_REQUEST,
            format!(
                "unknown legacy format '{}'; this converter reads 'sql2rdf'",
                body.format
            ),
        ));
    }
    let source_id = body.source.trim().trim_start_matches("urn:source:");
    let source = registry::get_source(&state.store, source_id).ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            format!("datasource '{source_id}' not found"),
        )
    })?;
    let converted = convert(&body.document, &source.id, body.empty_as_null)
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    mappings::validate_rml(&converted.rml, &source.id).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("the converted RML failed validation: {e}"),
        )
    })?;
    Ok(Json(converted))
}

pub fn routes() -> Router<AppState> {
    Router::new().route(
        "/api/mappings/convert",
        axum::routing::post(convert_mapping),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rml::model::ObjectMap;
    use crate::rml::terms::{expand_slug_template, slug, Row};
    use crate::store::TripleStore;
    use ots_plugin_api::sources::SourceConnector;
    use oxigraph::io::RdfFormat;

    /// The appendix document, with the prefixes it leaves undeclared added.
    const BUNDLE: &str = r#"
prefixes:
  ex: "http://example.org/products/ontology#"
  prod: "http://example.org/products/"
  cat: "http://example.org/categories/"
  sup: "http://example.org/suppliers/"
  addr: "http://example.org/addresses/"
entities:
  - source_table: products
    primary_key: product_id
    subject_iri: "prod:product_{product_id}"
    rdf_type: "ex:Product"
    properties:
      - { column: price, predicate: "ex:hasPrice", datatype: "xsd:decimal" }
      - column: category
        predicate: "ex:inCategory"
        object: { kind: lookup, iri: "cat:{value_slug}", rdf_type: "ex:Category",
                  label_predicate: "rdfs:label", label_from: value }
      - column: supplier_id
        predicate: "ex:suppliedBy"
        object: { kind: reference, iri: "sup:supplier_{value}" }
      - column: status
        predicate: "ex:hasStatus"
        object: { kind: enumeration, normalize: lower_trim,
                  value_map: { active: "ex:Active" }, unmapped: literal }
    nested:
      - predicate: "ex:hasAddress"
        subject_iri: "addr:address_{supplier_id}"
        rdf_type: "ex:Address"
        properties: [ { column: city, predicate: "ex:city", datatype: "xsd:string" } ]
  - source_table: suppliers
    primary_key: supplier_id
    subject_iri: "sup:supplier_{supplier_id}"
    rdf_type: "ex:Supplier"
    properties:
      - { column: name, predicate: "ex:name" }
"#;

    fn db() -> (
        tempfile::TempDir,
        Box<dyn ots_plugin_api::sources::SourceConnection>,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("legacy.db");
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE suppliers (supplier_id INTEGER PRIMARY KEY, name TEXT);
             CREATE TABLE products (
                 product_id INTEGER PRIMARY KEY, price REAL, category TEXT,
                 supplier_id INTEGER, status TEXT, city TEXT);
             INSERT INTO suppliers VALUES (10, 'Acme');
             INSERT INTO products VALUES
               (1, 0.25, 'Fasteners & Bolts', 10, 'active', 'Utrecht'),
               (2, NULL, '', 10, ' Retired ', '');",
        )
        .unwrap();
        let params = ots_plugin_api::sources::ConnectParams {
            dialect: "sqlite".into(),
            host: None,
            port: None,
            database: path.to_string_lossy().into_owned(),
            username: None,
            password: None,
            read_only: true,
            statement_timeout_ms: 5_000,
            tls: false,
            options: Default::default(),
        };
        (
            dir,
            crate::sources::sqlite::SqliteConnector
                .connect(&params)
                .unwrap(),
        )
    }

    /// The triples the legacy transformer produces for the fixture rows, as
    /// the appendix defines them: one line per triple, sorted, canonical
    /// N-Triples. Derived by hand from the format's stated semantics —
    /// `{value_slug}` slugs, `lower_trim` normalises before the value map is
    /// consulted, an unmapped value stays the original literal, a NULL or
    /// empty cell produces nothing.
    const EXPECTED: &str = "\
<http://example.org/addresses/address_10> <http://example.org/products/ontology#city> \"Utrecht\" .
<http://example.org/addresses/address_10> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://example.org/products/ontology#Address> .
<http://example.org/categories/fasteners-bolts> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://example.org/products/ontology#Category> .
<http://example.org/categories/fasteners-bolts> <http://www.w3.org/2000/01/rdf-schema#label> \"Fasteners & Bolts\" .
<http://example.org/products/product_1> <http://example.org/products/ontology#hasAddress> <http://example.org/addresses/address_10> .
<http://example.org/products/product_1> <http://example.org/products/ontology#hasPrice> \"0.25\"^^<http://www.w3.org/2001/XMLSchema#decimal> .
<http://example.org/products/product_1> <http://example.org/products/ontology#hasStatus> <http://example.org/products/ontology#Active> .
<http://example.org/products/product_1> <http://example.org/products/ontology#inCategory> <http://example.org/categories/fasteners-bolts> .
<http://example.org/products/product_1> <http://example.org/products/ontology#suppliedBy> <http://example.org/suppliers/supplier_10> .
<http://example.org/products/product_1> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://example.org/products/ontology#Product> .
<http://example.org/products/product_2> <http://example.org/products/ontology#hasAddress> <http://example.org/addresses/address_10> .
<http://example.org/products/product_2> <http://example.org/products/ontology#hasStatus> \" Retired \" .
<http://example.org/products/product_2> <http://example.org/products/ontology#suppliedBy> <http://example.org/suppliers/supplier_10> .
<http://example.org/products/product_2> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://example.org/products/ontology#Product> .
<http://example.org/suppliers/supplier_10> <http://example.org/products/ontology#name> \"Acme\" .
<http://example.org/suppliers/supplier_10> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://example.org/products/ontology#Supplier> .
";

    /// Canonical N-Triples of a graph: sorted lines, blank nodes excluded (the
    /// fixture mints none).
    fn canonical(store: &TripleStore, graph: &str) -> String {
        let bytes = store.dump(RdfFormat::NTriples, Some(graph)).unwrap();
        let mut lines: Vec<&str> = std::str::from_utf8(&bytes)
            .unwrap()
            .lines()
            .filter(|l| !l.trim().is_empty())
            .collect();
        lines.sort_unstable();
        let mut out = lines.join("\n");
        out.push('\n');
        out
    }

    #[test]
    fn the_converted_mapping_reproduces_the_legacy_transformers_triples_byte_for_byte() {
        let converted = convert(BUNDLE, "legacy", true).expect("converts");
        assert_eq!(
            converted.triples_maps, 4,
            "products, its lookup, its nested map, suppliers"
        );
        let mapping = crate::rml::parse_rml(&converted.rml).unwrap();
        let (_dir, mut conn) = db();
        let store = TripleStore::in_memory().unwrap();
        let quote = |t: &str| format!("\"{}\"", t.replace('"', "\"\""));
        crate::rml::execute_relational(
            &mapping,
            conn.as_mut(),
            &quote,
            &store,
            "urn:run:legacy",
            100,
            "legacy-run",
        )
        .unwrap_or_else(|e| panic!("{e}\n{}", converted.rml));
        assert_eq!(
            canonical(&store, "urn:run:legacy"),
            EXPECTED,
            "\n{}",
            converted.rml
        );
    }

    #[test]
    fn the_table_form_reproduces_the_same_triples_in_this_engine() {
        // The default: rr:tableName, no query. This engine emits no term for
        // an empty cell, so the output is the legacy transformer's here too —
        // the NULLIF form exists for other processors, and says so.
        let converted = convert(BUNDLE, "legacy", false).expect("converts");
        assert!(
            converted.rml.contains("rr:tableName \"products\""),
            "{}",
            converted.rml
        );
        assert!(!converted.rml.contains("NULLIF"), "{}", converted.rml);
        assert!(converted
            .warnings
            .iter()
            .any(|w| w.contains("empty literal")));
        let mapping = crate::rml::parse_rml(&converted.rml).unwrap();
        let (_dir, mut conn) = db();
        let store = TripleStore::in_memory().unwrap();
        let quote = |t: &str| format!("\"{}\"", t.replace('"', "\"\""));
        crate::rml::execute_relational(
            &mapping,
            conn.as_mut(),
            &quote,
            &store,
            "urn:run:t",
            100,
            "r",
        )
        .unwrap();
        assert_eq!(
            canonical(&store, "urn:run:t"),
            EXPECTED,
            "\n{}",
            converted.rml
        );
        // …and with a table, the key join can be pushed down.
        let products = mapping
            .triples_maps
            .iter()
            .find(|tm| tm.iri.ends_with("#products"))
            .unwrap();
        assert_eq!(
            products.logical_source.table_name.as_deref(),
            Some("products")
        );
    }

    #[test]
    fn each_construct_becomes_the_documented_rml() {
        let converted = convert(BUNDLE, "legacy", true).unwrap();
        let rml = &converted.rml;
        // The reference resolved to the suppliers entity by its key.
        assert!(rml.contains("rr:parentTriplesMap"), "{rml}");
        assert!(
            rml.contains(
                "rr:joinCondition [ rr:child \"supplier_id\" ; rr:parent \"supplier_id\" ]"
            ),
            "{rml}"
        );
        // The lookup slugs, so it is minted.
        assert!(rml.contains("otsfn:mintIri"), "{rml}");
        assert!(rml.contains("{category_slug}"), "{rml}");
        // The enumeration is the value-map function.
        assert!(
            rml.contains("otsfn:mapValue") && rml.contains("lower_trim"),
            "{rml}"
        );
        let mapping = crate::rml::parse_rml(rml).unwrap();
        // Only text-valued columns read through NULLIF; the key and the price
        // do not. Checked on the parsed query, past Turtle's own escaping.
        let query = mapping
            .triples_maps
            .iter()
            .find(|tm| tm.iri.ends_with("#products"))
            .and_then(|tm| tm.logical_source.query.clone())
            .expect("a query source");
        assert!(
            query.contains("NULLIF(\"category\", '') AS \"category\""),
            "{query}"
        );
        assert!(
            query.contains("NULLIF(\"city\", '') AS \"city\""),
            "{query}"
        );
        assert!(
            query.contains("NULLIF(\"status\", '') AS \"status\""),
            "{query}"
        );
        assert!(!query.contains("NULLIF(\"price\""), "{query}");
        assert!(!query.contains("NULLIF(\"product_id\""), "{query}");
        assert!(!query.contains("NULLIF(\"supplier_id\""), "{query}");
        let lookup = mapping
            .triples_maps
            .iter()
            .find(|tm| tm.iri.ends_with("products_category"))
            .expect("a lookup map");
        assert!(lookup.subject_map.function.is_some(), "a minted subject");
        let products = mapping
            .triples_maps
            .iter()
            .find(|tm| tm.iri.ends_with("#products"))
            .unwrap();
        assert!(products
            .predicate_object_maps
            .iter()
            .any(|p| matches!(p.object, ObjectMap::Function(_))));
    }

    #[test]
    fn a_reference_with_no_matching_entity_falls_back_to_a_template() {
        let doc = r#"
prefixes: { ex: "http://x/", sup: "http://x/sup/" }
entities:
  - source_table: products
    primary_key: product_id
    subject_iri: "ex:p_{product_id}"
    properties:
      - column: supplier_id
        predicate: "ex:suppliedBy"
        object: { kind: reference, iri: "sup:supplier_{value}" }
"#;
        let converted = convert(doc, "s", true).unwrap();
        assert!(
            !converted.rml.contains("rr:parentTriplesMap"),
            "{}",
            converted.rml
        );
        assert!(
            converted
                .rml
                .contains("rr:template \"http://x/sup/supplier_{supplier_id}\""),
            "{}",
            converted.rml
        );
    }

    #[test]
    fn mistakes_are_reported_by_entity_and_property() {
        let err = convert("entities: [{subject_iri: 'ex:a'}]", "s", true).unwrap_err();
        assert!(err.to_string().contains("source_table"), "{err}");
        let err = convert(
            "prefixes: {ex: 'http://x/'}\nentities: [{source_table: t, subject_iri: 'ex:a_{id}', properties: [{column: c, predicate: 'ex:p', object: {kind: nope}}]}]",
            "s",
            true,
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("'nope'") && err.to_string().contains("entity 't'"),
            "{err}"
        );
        let err = convert(
            "entities: [{source_table: t, subject_iri: 'undeclared:a_{id}'}]",
            "s",
            true,
        )
        .unwrap_err();
        assert!(err.to_string().contains("undeclared"), "{err}");
        assert!(matches!(
            convert("- not a map", "s", true),
            Err(LegacyError::Document(_))
        ));
        assert!(matches!(
            convert("prefixes: {}", "s", true),
            Err(LegacyError::Document(_))
        ));
    }

    #[test]
    fn slugs_and_slug_templates_behave_as_documented() {
        assert_eq!(slug("Fasteners & Bolts"), "fasteners-bolts");
        assert_eq!(slug("  Rolling Stock (2024) "), "rolling-stock-2024");
        assert_eq!(slug("---"), "");
        assert_eq!(slug("ÀB"), "b", "ASCII only");
        let row: Row = Row::from([
            ("category".to_string(), "Fasteners & Bolts".to_string()),
            ("id".to_string(), "a b".to_string()),
        ]);
        assert_eq!(
            expand_slug_template("http://x/{id}/{category_slug}", &row).as_deref(),
            Some("http://x/a%20b/fasteners-bolts")
        );
        assert_eq!(expand_slug_template("http://x/{missing_slug}", &row), None);
        assert_eq!(expand_slug_template("http://x/{missing}", &row), None);
        let blank: Row = Row::from([("category".to_string(), "***".to_string())]);
        assert_eq!(
            expand_slug_template("http://x/{category_slug}", &blank),
            None,
            "a value that slugs to nothing mints nothing"
        );
    }
}
