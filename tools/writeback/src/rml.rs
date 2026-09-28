//! An RML mapping read backwards: which table a triples map fills, how its
//! subject names the key, and which predicate carries which column.

use oxrdf::{NamedNode, NamedOrBlankNode, Term, Triple};

const RR: &str = "http://www.w3.org/ns/r2rml#";
const RML: &str = "http://semweb.mmlab.be/ns/rml#";
/// The RML-IO namespace, for mappings written against it.
const RML_IO: &str = "http://w3id.org/rml/";
const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";

/// How a triples map names its subject, read backwards to the key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubjectPattern {
    /// `rr:template "…{key}…"` with exactly one placeholder.
    Template { prefix: String, suffix: String },
    /// `rr:column "key"` with `rr:termType rr:IRI`: the IRI is the value.
    Column,
}

impl SubjectPattern {
    /// The key value an entity IRI encodes, if it fits the pattern.
    pub fn key_of(&self, iri: &str) -> Option<String> {
        match self {
            SubjectPattern::Template { prefix, suffix } => {
                let rest = iri.strip_prefix(prefix.as_str())?;
                let key = rest.strip_suffix(suffix.as_str())?;
                (!key.is_empty() && !key.contains('/')).then(|| key.to_string())
            }
            SubjectPattern::Column => Some(iri.to_string()),
        }
    }
}

/// One triples map, inverted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableRule {
    pub triples_map: String,
    pub table: String,
    pub key_column: String,
    pub subject: SubjectPattern,
    /// `rr:class` values: a member typed with one of these belongs here
    /// even before its IRI is checked.
    pub classes: Vec<String>,
    /// `(predicate, column)` for every column-valued object map.
    pub columns: Vec<(String, String)>,
}

impl TableRule {
    /// The key an entity maps to under this rule, or `None` when the rule
    /// does not recognise it: the IRI must fit the subject pattern, and when
    /// the rule declares classes the member must carry one of them.
    pub fn key_of(&self, entity: &str, types: &[String]) -> Option<String> {
        if !self.classes.is_empty() && !types.iter().any(|t| self.classes.contains(t)) {
            return None;
        }
        self.subject.key_of(entity)
    }
}

/// A small triple index: what a mapping says about each node.
struct Graph {
    triples: Vec<Triple>,
}

impl Graph {
    fn objects<'a>(
        &'a self,
        subject: &'a NamedOrBlankNode,
        predicate: &str,
    ) -> impl Iterator<Item = &'a Term> + 'a {
        let p = predicate.to_string();
        self.triples
            .iter()
            .filter(move |t| &t.subject == subject && t.predicate.as_str() == p)
            .map(|t| &t.object)
    }

    /// The first object under any of the predicates, tried in order —
    /// classic RML and RML-IO spell the same thing in two namespaces.
    fn first<'a>(
        &'a self,
        subject: &'a NamedOrBlankNode,
        predicates: &[String],
    ) -> Option<&'a Term> {
        predicates
            .iter()
            .find_map(|p| self.objects(subject, p).next())
    }

    fn all<'a>(&'a self, subject: &'a NamedOrBlankNode, predicates: &[String]) -> Vec<&'a Term> {
        predicates
            .iter()
            .flat_map(|p| self.objects(subject, p).collect::<Vec<_>>())
            .collect()
    }

    fn subjects_with(&self, predicate: &str, object: &str) -> Vec<NamedOrBlankNode> {
        let mut out: Vec<NamedOrBlankNode> = self
            .triples
            .iter()
            .filter(|t| t.predicate.as_str() == predicate && term_iri(&t.object) == Some(object))
            .map(|t| t.subject.clone())
            .collect();
        out.dedup();
        out
    }
}

fn term_iri(t: &Term) -> Option<&str> {
    match t {
        Term::NamedNode(n) => Some(n.as_str()),
        _ => None,
    }
}

fn term_text(t: &Term) -> Option<String> {
    match t {
        Term::Literal(l) => Some(l.value().to_string()),
        Term::NamedNode(n) => Some(n.as_str().to_string()),
        _ => None,
    }
}

fn node_of(t: &Term) -> Option<NamedOrBlankNode> {
    match t {
        Term::NamedNode(n) => Some(NamedOrBlankNode::NamedNode(n.clone())),
        Term::BlankNode(b) => Some(NamedOrBlankNode::BlankNode(b.clone())),
        _ => None,
    }
}

fn both(local: &str) -> Vec<String> {
    vec![format!("{RR}{local}"), format!("{RML_IO}{local}")]
}

fn rml_both(local: &str) -> Vec<String> {
    vec![format!("{RML}{local}"), format!("{RML_IO}{local}")]
}

/// Parse a Turtle mapping.
pub fn parse_turtle(turtle: &str) -> Result<Vec<Triple>, String> {
    let mut triples = Vec::new();
    for t in oxttl::TurtleParser::new().for_reader(turtle.as_bytes()) {
        triples.push(t.map_err(|e| format!("the mapping is not valid Turtle: {e}"))?);
    }
    Ok(triples)
}

/// A template's single placeholder, as the text before and after it.
fn split_template(template: &str) -> Option<(String, String, String)> {
    let open = template.find('{')?;
    let close = template[open..].find('}')? + open;
    let column = template[open + 1..close].to_string();
    let suffix = &template[close + 1..];
    if column.is_empty() || suffix.contains('{') {
        return None;
    }
    Some((template[..open].to_string(), column, suffix.to_string()))
}

/// Invert every triples map of a mapping. What cannot be inverted is
/// reported, not guessed.
pub fn invert(rml_turtle: &str) -> Result<(Vec<TableRule>, Vec<String>), String> {
    let graph = Graph {
        triples: parse_turtle(rml_turtle)?,
    };
    let mut rules = Vec::new();
    let mut warnings = Vec::new();

    let mut maps = graph.subjects_with(RDF_TYPE, &format!("{RR}TriplesMap"));
    for p in rml_both("logicalSource") {
        for t in graph.triples.iter().filter(|t| t.predicate.as_str() == p) {
            if !maps.contains(&t.subject) {
                maps.push(t.subject.clone());
            }
        }
    }

    for tm in maps {
        let name = match &tm {
            NamedOrBlankNode::NamedNode(n) => n.as_str().to_string(),
            NamedOrBlankNode::BlankNode(b) => format!("_:{}", b.as_str()),
        };
        let Some(source) = graph
            .first(&tm, &rml_both("logicalSource"))
            .and_then(node_of)
        else {
            warnings.push(format!("{name}: no logical source; skipped"));
            continue;
        };
        if graph.first(&source, &rml_both("query")).is_some() {
            warnings.push(format!(
                "{name}: reads a query, not a table, so its rows cannot be written back; skipped"
            ));
            continue;
        }
        let Some(table) = graph.first(&source, &both("tableName")).and_then(term_text) else {
            warnings.push(format!("{name}: no rr:tableName; skipped"));
            continue;
        };
        let Some(sm) = graph.first(&tm, &both("subjectMap")).and_then(node_of) else {
            warnings.push(format!("{name}: no subject map; skipped"));
            continue;
        };
        let (subject, key_column) =
            if let Some(template) = graph.first(&sm, &both("template")).and_then(term_text) {
                match split_template(&template) {
                    Some((prefix, column, suffix)) => {
                        (SubjectPattern::Template { prefix, suffix }, column)
                    }
                    None => {
                        warnings.push(format!(
                        "{name}: the subject template '{template}' has no single placeholder, so \
                         the key cannot be read back from an IRI; skipped"
                    ));
                        continue;
                    }
                }
            } else if let Some(column) = graph.first(&sm, &both("column")).and_then(term_text) {
                (SubjectPattern::Column, column)
            } else {
                warnings.push(format!(
                    "{name}: the subject is neither a template nor a column; skipped"
                ));
                continue;
            };
        let classes: Vec<String> = graph
            .all(&sm, &both("class"))
            .into_iter()
            .filter_map(term_iri)
            .map(str::to_string)
            .collect();

        let mut columns: Vec<(String, String)> = Vec::new();
        for pom in graph.all(&tm, &both("predicateObjectMap")) {
            let Some(pom) = node_of(pom) else { continue };
            let predicate = graph
                .first(&pom, &both("predicate"))
                .and_then(term_iri)
                .map(str::to_string)
                .or_else(|| {
                    graph
                        .first(&pom, &both("predicateMap"))
                        .and_then(node_of)
                        .and_then(|pm| {
                            graph
                                .first(&pm, &both("constant"))
                                .and_then(term_iri)
                                .map(str::to_string)
                        })
                });
            let Some(predicate) = predicate else {
                warnings.push(format!(
                    "{name}: a predicate-object map without a constant predicate; skipped"
                ));
                continue;
            };
            let Some(om) = graph.first(&pom, &both("objectMap")).and_then(node_of) else {
                if graph.first(&pom, &both("object")).is_some() {
                    warnings.push(format!(
                        "{name}: <{predicate}> is a constant object, not a column; skipped"
                    ));
                }
                continue;
            };
            match graph.first(&om, &both("column")).and_then(term_text) {
                Some(column) => columns.push((predicate, column)),
                None => warnings.push(format!(
                    "{name}: <{predicate}> is not a plain column (a template, constant, function or \
                     join); skipped"
                )),
            }
        }

        rules.push(TableRule {
            triples_map: name,
            table,
            key_column,
            subject,
            classes,
            columns,
        });
    }
    Ok((rules, warnings))
}

/// Helper for the CLI's report: the predicate of a `NamedNode`, shortened.
pub fn short(iri: &str) -> &str {
    iri.rsplit(['#', '/']).next().unwrap_or(iri)
}

#[allow(dead_code)]
fn _assert_named(_: NamedNode) {}

#[cfg(test)]
mod tests {
    use super::*;

    const MAPPING: &str = r#"
@prefix rr:  <http://www.w3.org/ns/r2rml#> .
@prefix rml: <http://semweb.mmlab.be/ns/rml#> .
@prefix ql:  <http://semweb.mmlab.be/ns/ql#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
@prefix ex:  <http://example.org/ontology#> .

ex:ProductsMap a rr:TriplesMap ;
  rml:logicalSource [ rml:source <urn:source:shop> ; rml:referenceFormulation ql:SQL2008 ; rr:tableName "products" ] ;
  rr:subjectMap [ rr:template "http://example.org/products/product_{product_id}" ; rr:class ex:Product ] ;
  rr:predicateObjectMap [ rr:predicate ex:name ; rr:objectMap [ rr:column "name" ] ] ;
  rr:predicateObjectMap [ rr:predicate ex:hasPrice ; rr:objectMap [ rr:column "price" ; rr:datatype xsd:decimal ] ] ;
  rr:predicateObjectMap [ rr:predicate ex:label ; rr:objectMap [ rr:template "{name} ({code})" ] ] ;
  rr:predicateObjectMap [ rr:predicate ex:category ; rr:objectMap [ rr:parentTriplesMap ex:CategoriesMap ; rr:joinCondition [ rr:child "cat" ; rr:parent "id" ] ] ] .

ex:CategoriesMap a rr:TriplesMap ;
  rml:logicalSource [ rml:source <urn:source:shop> ; rml:referenceFormulation ql:SQL2008 ; rml:query "SELECT id, label FROM categories" ] ;
  rr:subjectMap [ rr:template "http://example.org/categories/{id}" ] ;
  rr:predicateObjectMap [ rr:predicate ex:label ; rr:objectMap [ rr:column "label" ] ] .

ex:IriMap a rr:TriplesMap ;
  rml:logicalSource [ rml:source <urn:source:shop> ; rml:referenceFormulation ql:SQL2008 ; rr:tableName "things" ] ;
  rr:subjectMap [ rr:column "iri" ; rr:termType rr:IRI ] ;
  rr:predicateObjectMap [ rr:predicate ex:note ; rr:objectMap [ rr:column "note" ] ] .
"#;

    #[test]
    fn a_mapping_inverts_to_table_rules_and_reports_what_it_cannot() {
        let (rules, warnings) = invert(MAPPING).unwrap();
        assert_eq!(rules.len(), 2, "{rules:?}");
        let products = rules.iter().find(|r| r.table == "products").unwrap();
        assert_eq!(products.key_column, "product_id");
        assert_eq!(
            products.subject,
            SubjectPattern::Template {
                prefix: "http://example.org/products/product_".into(),
                suffix: String::new()
            }
        );
        assert_eq!(
            products.classes,
            vec!["http://example.org/ontology#Product"]
        );
        assert_eq!(
            products.columns,
            vec![
                (
                    "http://example.org/ontology#name".to_string(),
                    "name".to_string()
                ),
                (
                    "http://example.org/ontology#hasPrice".to_string(),
                    "price".to_string()
                ),
            ]
        );
        let things = rules.iter().find(|r| r.table == "things").unwrap();
        assert_eq!(things.subject, SubjectPattern::Column);
        assert_eq!(things.key_column, "iri");
        // The query source, the template object and the join are reported.
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("CategoriesMap") && w.contains("query")),
            "{warnings:?}"
        );
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("label") && w.contains("not a plain column")),
            "{warnings:?}"
        );
        assert!(
            warnings.iter().any(|w| w.contains("category")),
            "{warnings:?}"
        );
    }

    #[test]
    fn keys_are_read_back_from_iris() {
        let (rules, _) = invert(MAPPING).unwrap();
        let products = rules.iter().find(|r| r.table == "products").unwrap();
        let product = vec!["http://example.org/ontology#Product".to_string()];
        assert_eq!(
            products
                .key_of("http://example.org/products/product_42", &product)
                .as_deref(),
            Some("42")
        );
        assert_eq!(
            products.key_of("http://example.org/products/product_42", &[]),
            None,
            "the class is required"
        );
        assert_eq!(
            products.key_of("http://example.org/other/42", &product),
            None
        );
        assert_eq!(
            products.key_of("http://example.org/products/product_", &product),
            None
        );
        assert_eq!(
            products.key_of("http://example.org/products/product_4/2", &product),
            None
        );
        let things = rules.iter().find(|r| r.table == "things").unwrap();
        assert_eq!(
            things.key_of("http://x/anything", &[]).as_deref(),
            Some("http://x/anything")
        );
        assert_eq!(
            split_template("a{b}c"),
            Some(("a".into(), "b".into(), "c".into()))
        );
        assert_eq!(split_template("a{b}{c}"), None);
        assert_eq!(split_template("abc"), None);
        assert_eq!(short("http://example.org/ontology#name"), "name");
    }

    #[test]
    fn bad_turtle_is_an_error() {
        assert!(invert("this is not turtle").is_err());
        let (rules, warnings) = invert("").unwrap();
        assert!(rules.is_empty() && warnings.is_empty());
    }
}
