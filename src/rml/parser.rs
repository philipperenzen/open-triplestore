//! RML mapping document parser.
//!
//! Reads an RML mapping stored as triples in the triple store (or loaded
//! from a Turtle string into an in-memory store) and builds an `RmlMapping`.

use std::collections::BTreeMap;

use oxigraph::model::Term;

use super::model::*;
use crate::store::engine::TripleStore;

const RR: &str = "http://www.w3.org/ns/r2rml#";
const RML: &str = "http://semweb.mmlab.be/ns/rml#";
/// The RML-Core / RML-IO namespace. Only `rml:baseIRI` is read from it so far.
const RML_CORE: &str = "http://w3id.org/rml/";
const FNML: &str = "http://semweb.mmlab.be/ns/fnml#";
/// FnO moved host; both spellings of `fno:executes` are accepted.
const FNO_EXECUTES: [&str; 2] = [
    "https://w3id.org/function/ontology#executes",
    "http://w3id.org/function/ontology#executes",
];

/// The IRI a graph map generates to name the default graph (R2RML §9).
pub const DEFAULT_GRAPH: &str = "http://www.w3.org/ns/r2rml#defaultGraph";

/// The IRI prefix a `rml:source` carries when it names a registered
/// datasource rather than a file.
pub const DATASOURCE_PREFIX: &str = "urn:source:";

/// Where a term map sits, which decides the term types it may produce and its
/// default (R2RML §7.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Position {
    Subject,
    Predicate,
    Object,
    Graph,
}

impl Position {
    fn name(self) -> &'static str {
        match self {
            Position::Subject => "subject map",
            Position::Predicate => "predicate map",
            Position::Object => "object map",
            Position::Graph => "graph map",
        }
    }
}

/// Parse RML mappings from Turtle text into an `RmlMapping`.
pub fn parse_rml(turtle: &str) -> Result<RmlMapping, String> {
    let store =
        TripleStore::in_memory().map_err(|e| format!("Failed to create temp store: {e}"))?;
    store
        .load_str(turtle, oxigraph::io::RdfFormat::Turtle, None)
        .map_err(|e| format!("Failed to parse RML Turtle: {e}"))?;
    parse_from_store(&store, None)
}

/// Parse RML mappings from a named graph in an existing store, under the
/// current term-generation rules.
pub fn parse_from_store(store: &TripleStore, graph: Option<&str>) -> Result<RmlMapping, String> {
    parse_from_store_as(store, graph, Semantics::default())
}

/// [`parse_from_store`] under the rules a frozen mapping version was written
/// against. The rules decide a default here — what an object map's template
/// generates when it names no `rr:termType` — so they are applied at parse.
pub fn parse_from_store_as(
    store: &TripleStore,
    graph: Option<&str>,
    semantics: Semantics,
) -> Result<RmlMapping, String> {
    let graph_clause = match graph {
        Some(g) => format!("GRAPH <{}> {{", crate::store::escape_sparql_iri(g)),
        None => String::new(),
    };
    let graph_close = if graph.is_some() { "}" } else { "" };

    // Find all TriplesMap IRIs
    let q = format!("SELECT ?tm WHERE {{ {graph_clause} ?tm a <{RR}TriplesMap> {graph_close} }}");
    let mut tm_iris = query_col(store, &q, "tm");
    // Deterministic order: a mapping's triples maps run (and their parent
    // indexes build) in a stable sequence, so two runs of the same mapping
    // produce the same blank-node labelling and the same log.
    tm_iris.sort();
    tm_iris.dedup();

    let ctx = Ctx {
        store,
        graph,
        semantics,
    };
    let mut triples_maps = Vec::new();
    for tm_iri in &tm_iris {
        match ctx.triples_map(tm_iri) {
            Ok(tm) => triples_maps.push(tm),
            Err(e) => return Err(format!("Error in TriplesMap <{tm_iri}>: {e}")),
        }
    }
    if triples_maps.is_empty() {
        return Err("the mapping declares no rr:TriplesMap".to_string());
    }

    let mapping = RmlMapping {
        triples_maps,
        semantics,
        base_iri: None,
    };
    validate_references(&mapping)?;
    Ok(mapping)
}

/// Every `rr:parentTriplesMap` must name a triples map that exists, and a
/// join-less reference is only legal when both sides read the same logical
/// source (R2RML §10.3). Catching it here turns a silent "no triples" into a
/// mapping error the author sees at upload.
fn validate_references(mapping: &RmlMapping) -> Result<(), String> {
    for tm in &mapping.triples_maps {
        for pom in &tm.predicate_object_maps {
            let ObjectMap::Ref(r) = &pom.object else {
                continue;
            };
            let parent = mapping.find(&r.parent_triples_map).ok_or_else(|| {
                format!(
                    "TriplesMap <{}> references parent <{}>, which the mapping does not define",
                    tm.iri, r.parent_triples_map
                )
            })?;
            if r.joins.is_empty()
                && !same_logical_source(&tm.logical_source, &parent.logical_source)
            {
                return Err(format!(
                    "TriplesMap <{}> references parent <{}> with no rr:joinCondition, but they \
                     read different logical sources",
                    tm.iri, r.parent_triples_map
                ));
            }
        }
    }
    Ok(())
}

fn same_logical_source(a: &LogicalSource, b: &LogicalSource) -> bool {
    a.source == b.source && a.query == b.query && a.table_name == b.table_name
}

/// The store, graph and rules a document is parsed under.
struct Ctx<'a> {
    store: &'a TripleStore,
    graph: Option<&'a str>,
    semantics: Semantics,
}

impl Ctx<'_> {
    fn objects(&self, subject: &str, predicate: &str) -> Vec<String> {
        get_objects(self.store, subject, predicate, self.graph)
    }

    fn terms(&self, subject: &str, predicate: &str) -> Vec<Term> {
        get_terms(self.store, subject, predicate, self.graph)
    }

    fn triples_map(&self, tm_iri: &str) -> Result<TriplesMap, String> {
        // Logical source (rml:logicalSource, or R2RML's rr:logicalTable)
        let ls_iri = self
            .objects(tm_iri, &format!("{RML}logicalSource"))
            .into_iter()
            .next()
            .or_else(|| {
                self.objects(tm_iri, &format!("{RR}logicalTable"))
                    .into_iter()
                    .next()
            })
            .ok_or("Missing rml:logicalSource")?;
        let logical_source = parse_logical_source(self.store, &ls_iri, self.graph)?;

        let mut subject_map = self.subject_map(tm_iri)?;
        // R2RML puts graph maps on the subject map. One on the triples map
        // itself is how this engine used to read them; it still means "every
        // triple of this map", which is what a subject graph map means.
        subject_map
            .graph_maps
            .extend(self.graph_maps(tm_iri, "triples map")?);

        let mut predicate_object_maps = Vec::new();
        for pom_node in &self.objects(tm_iri, &format!("{RR}predicateObjectMap")) {
            predicate_object_maps.push(self.pom(pom_node)?);
        }

        let base_iri = [format!("{RML_CORE}baseIRI"), format!("{RML}baseIRI")]
            .iter()
            .find_map(|p| match self.terms(tm_iri, p).into_iter().next() {
                Some(Term::NamedNode(n)) => Some(Ok(n.into_string())),
                Some(_) => Some(Err("rml:baseIRI must be an IRI".to_string())),
                None => None,
            })
            .transpose()?;

        let mut tm = TriplesMap {
            iri: tm_iri.to_string(),
            logical_source,
            subject_map,
            predicate_object_maps,
            base_iri,
        };
        if tm.logical_source.reference_formulation == ReferenceFormulation::Sql {
            normalise_sql_columns(&mut tm);
        }
        Ok(tm)
    }

    fn subject_map(&self, tm_iri: &str) -> Result<SubjectMap, String> {
        let sm_node = self
            .objects(tm_iri, &format!("{RR}subjectMap"))
            .into_iter()
            .next();
        // A function-valued subject (`fnml:functionValue` on the subject map):
        // the term map is an empty placeholder and the function does the work.
        let function = match &sm_node {
            Some(sm) => self
                .objects(sm, &format!("{FNML}functionValue"))
                .into_iter()
                .next()
                .map(|fv| self.function(sm, &fv))
                .transpose()?,
            None => None,
        };
        let term_map = if function.is_some() {
            TermMap {
                kind: TermMapKind::Constant(Term::Literal(
                    oxigraph::model::Literal::new_simple_literal(""),
                )),
                term_type: TermType::IRI,
                datatype: None,
                language: None,
            }
        } else if let Some(ref sm) = sm_node {
            self.term_map(sm, Position::Subject)?
        } else {
            let val = self
                .terms(tm_iri, &format!("{RR}subject"))
                .into_iter()
                .next()
                .ok_or("Missing rr:subjectMap or rr:subject")?;
            constant(val, Position::Subject)?
        };

        // rr:class assertions. R2RML places rr:class on the subjectMap; also accept it on
        // the TriplesMap as a convenience.
        let mut classes = Vec::new();
        if let Some(ref sm) = sm_node {
            classes.extend(self.objects(sm, &format!("{RR}class")));
        }
        classes.extend(self.objects(tm_iri, &format!("{RR}class")));
        classes.sort();
        classes.dedup();

        let graph_maps = match &sm_node {
            Some(sm) => self.graph_maps(sm, "subject map")?,
            None => Vec::new(),
        };

        Ok(SubjectMap {
            term_map,
            classes,
            function,
            graph_maps,
        })
    }

    fn pom(&self, pom_node: &str) -> Result<PredicateObjectMap, String> {
        // Predicate map
        let pm_nodes = self.objects(pom_node, &format!("{RR}predicateMap"));
        let predicate_map = if let Some(pm) = pm_nodes.into_iter().next() {
            self.term_map(&pm, Position::Predicate)?
        } else {
            let pred = self
                .terms(pom_node, &format!("{RR}predicate"))
                .into_iter()
                .next()
                .ok_or("Missing rr:predicateMap or rr:predicate")?;
            constant(pred, Position::Predicate)?
        };

        // Object map: a referencing map, a function, or a plain term.
        let om_nodes = self.objects(pom_node, &format!("{RR}objectMap"));
        let object = if let Some(om) = om_nodes.into_iter().next() {
            self.object_map(&om)?
        } else {
            // `rr:object` is a constant: an IRI stays an IRI, and a literal
            // keeps its datatype and language tag.
            let obj = self
                .terms(pom_node, &format!("{RR}object"))
                .into_iter()
                .next()
                .ok_or("Missing rr:objectMap or rr:object")?;
            ObjectMap::Term(constant(obj, Position::Object)?)
        };

        let graph_maps = self.graph_maps(pom_node, "predicate-object map")?;

        Ok(PredicateObjectMap {
            predicate_map,
            object,
            graph_maps,
        })
    }

    /// Every `rr:graphMap` and `rr:graph` on `node`. A malformed graph map is
    /// a mapping error: dropping it would write the triples somewhere else.
    fn graph_maps(&self, node: &str, owner: &str) -> Result<Vec<TermMap>, String> {
        let mut out = Vec::new();
        for gm in self.objects(node, &format!("{RR}graphMap")) {
            out.push(
                self.term_map(&gm, Position::Graph)
                    .map_err(|e| format!("rr:graphMap of the {owner}: {e}"))?,
            );
        }
        for g in self.terms(node, &format!("{RR}graph")) {
            out.push(constant(g, Position::Graph).map_err(|e| format!("rr:graph: {e}"))?);
        }
        Ok(out)
    }

    fn object_map(&self, om: &str) -> Result<ObjectMap, String> {
        if let Some(parent) = self
            .objects(om, &format!("{RR}parentTriplesMap"))
            .into_iter()
            .next()
        {
            let mut joins = Vec::new();
            for jc in self.objects(om, &format!("{RR}joinCondition")) {
                let child = self
                    .objects(&jc, &format!("{RR}child"))
                    .into_iter()
                    .next()
                    .ok_or("rr:joinCondition is missing rr:child")?;
                let parent_col = self
                    .objects(&jc, &format!("{RR}parent"))
                    .into_iter()
                    .next()
                    .ok_or("rr:joinCondition is missing rr:parent")?;
                joins.push(JoinCondition {
                    child,
                    parent: parent_col,
                });
            }
            joins.sort_by(|a, b| (&a.child, &a.parent).cmp(&(&b.child, &b.parent)));
            return Ok(ObjectMap::Ref(RefObjectMap {
                parent_triples_map: parent,
                joins,
            }));
        }

        if let Some(fv) = self
            .objects(om, &format!("{FNML}functionValue"))
            .into_iter()
            .next()
        {
            return self.function(om, &fv).map(ObjectMap::Function);
        }

        self.term_map(om, Position::Object).map(ObjectMap::Term)
    }

    /// The function call under `fnml:functionValue` node `fv`, hung off the term
    /// map `om` (an object map or a subject map).
    fn function(&self, om: &str, fv: &str) -> Result<FunctionMap, String> {
        let mut params: BTreeMap<String, Vec<FunctionArg>> = BTreeMap::new();
        let mut function: Option<String> = None;

        for pom in self.objects(fv, &format!("{RR}predicateObjectMap")) {
            let predicate = self
                .objects(&pom, &format!("{RR}predicate"))
                .into_iter()
                .next()
                .or_else(|| {
                    self.objects(&pom, &format!("{RR}predicateMap"))
                        .into_iter()
                        .next()
                        .and_then(|pm| {
                            self.objects(&pm, &format!("{RR}constant"))
                                .into_iter()
                                .next()
                        })
                })
                .ok_or("a function parameter is missing rr:predicate")?;

            // `rr:object` is a constant; `rr:objectMap [ rr:column … ]` reads the row.
            let mut args: Vec<FunctionArg> = self
                .objects(&pom, &format!("{RR}object"))
                .into_iter()
                .map(FunctionArg::Constant)
                .collect();
            for om_node in self.objects(&pom, &format!("{RR}objectMap")) {
                let tm = self.term_map(&om_node, Position::Object)?;
                args.push(match tm.kind {
                    TermMapKind::Reference(c) => FunctionArg::Reference(c),
                    TermMapKind::Constant(c) => FunctionArg::Constant(term_value(&c)),
                    TermMapKind::Template(t) => FunctionArg::Constant(t),
                });
            }
            if FNO_EXECUTES.contains(&predicate.as_str()) {
                function = args
                    .iter()
                    .find_map(|a| match a {
                        FunctionArg::Constant(c) => Some(c.clone()),
                        FunctionArg::Reference(_) => None,
                    })
                    .or(function);
                continue;
            }
            params.entry(predicate).or_default().extend(args);
        }

        let function = function.ok_or("fnml:functionValue is missing fno:executes")?;
        let datatype = self
            .objects(om, &format!("{RR}datatype"))
            .into_iter()
            .next();

        Ok(FunctionMap {
            function,
            params,
            datatype,
        })
    }

    fn term_map(&self, node: &str, position: Position) -> Result<TermMap, String> {
        // Determine TermMapKind
        let kind = if let Some(c) = self
            .terms(node, &format!("{RR}constant"))
            .into_iter()
            .next()
        {
            return constant(c, position);
        } else if let Some(t) = self
            .objects(node, &format!("{RR}template"))
            .into_iter()
            .next()
        {
            TermMapKind::Template(t)
        } else if let Some(r) = self
            .objects(node, &format!("{RML}reference"))
            .into_iter()
            .next()
        {
            TermMapKind::Reference(r)
        } else if let Some(c) = self
            .objects(node, &format!("{RR}column"))
            .into_iter()
            .next()
        {
            TermMapKind::Reference(c)
        } else {
            return Err(format!(
                "TermMap <{node}> has no constant, template, or reference"
            ));
        };

        let datatype = self
            .objects(node, &format!("{RR}datatype"))
            .into_iter()
            .next();
        let language = self
            .objects(node, &format!("{RR}language"))
            .into_iter()
            .next();

        // R2RML §7.4: an explicit rr:termType wins; otherwise an object map
        // is a literal when it reads a column or declares a language or a
        // datatype, and everything else is an IRI. Before the fix this engine
        // made every object map a literal by default, which a legacy version
        // keeps.
        let default_type = match position {
            Position::Object => {
                let literal = match self.semantics {
                    Semantics::Legacy => true,
                    Semantics::R2rml => {
                        matches!(kind, TermMapKind::Reference(_))
                            || language.is_some()
                            || datatype.is_some()
                    }
                };
                if literal {
                    TermType::Literal
                } else {
                    TermType::IRI
                }
            }
            _ => TermType::IRI,
        };
        let term_type = self
            .objects(node, &format!("{RR}termType"))
            .into_iter()
            .next()
            .map(|iri| term_type_from_iri(&iri, default_type.clone()))
            .unwrap_or(default_type);

        Ok(TermMap {
            kind,
            term_type,
            datatype,
            language,
        })
    }
}

/// A constant-valued term map. Its term type is the constant's own kind
/// (R2RML §7.4), so a literal keeps its datatype and language tag and an IRI
/// is an IRI wherever it appears — `rr:termType` has no effect on it. Only an
/// object may be a literal, and a blank node is never a constant.
fn constant(value: Term, position: Position) -> Result<TermMap, String> {
    let term_type = match &value {
        Term::NamedNode(_) => TermType::IRI,
        Term::Literal(_) if position == Position::Object => TermType::Literal,
        Term::Literal(l) => {
            return Err(format!(
                "the constant of a {} must be an IRI, not the literal \"{}\"",
                position.name(),
                l.value()
            ))
        }
        _ => {
            return Err(format!(
                "the constant of a {} must be an IRI{}",
                position.name(),
                if position == Position::Object {
                    " or a literal"
                } else {
                    ""
                }
            ))
        }
    };
    Ok(TermMap {
        kind: TermMapKind::Constant(value),
        term_type,
        datatype: None,
        language: None,
    })
}

/// A constant's lexical value: the IRI itself, or a literal's text.
fn term_value(t: &Term) -> String {
    match t {
        Term::NamedNode(n) => n.as_str().to_string(),
        Term::Literal(l) => l.value().to_string(),
        Term::BlankNode(b) => format!("_:{}", b.as_str()),
        #[cfg(feature = "rdf-12")]
        Term::Triple(_) => String::new(),
    }
}

/// Column names in a relational triples map as the result set reports them:
/// `rr:column "\"ID\""` and a template's `{"ID"}` read column `ID`. R2RML
/// takes column names from SQL, where the quotes are spelling, not name.
fn normalise_sql_columns(tm: &mut TriplesMap) {
    use super::sqlident::column_name;
    let fix = |m: &mut TermMap| match &mut m.kind {
        TermMapKind::Reference(c) => *c = column_name(c),
        TermMapKind::Template(t) => *t = map_template_columns(t, &column_name),
        TermMapKind::Constant(_) => {}
    };
    let fix_function = |f: &mut FunctionMap| {
        for args in f.params.values_mut() {
            for a in args {
                if let FunctionArg::Reference(c) = a {
                    *c = column_name(c);
                }
            }
        }
    };
    fix(&mut tm.subject_map.term_map);
    tm.subject_map.graph_maps.iter_mut().for_each(fix);
    if let Some(f) = &mut tm.subject_map.function {
        fix_function(f);
    }
    for pom in &mut tm.predicate_object_maps {
        fix(&mut pom.predicate_map);
        pom.graph_maps.iter_mut().for_each(fix);
        match &mut pom.object {
            ObjectMap::Term(t) => fix(t),
            ObjectMap::Function(f) => fix_function(f),
            ObjectMap::Ref(r) => {
                for j in &mut r.joins {
                    j.child = column_name(&j.child);
                    j.parent = column_name(&j.parent);
                }
            }
        }
    }
}

/// Rewrite each `{column}` placeholder of a template through `f`, keeping
/// escapes intact.
fn map_template_columns(template: &str, f: &dyn Fn(&str) -> String) -> String {
    let mut out = String::with_capacity(template.len());
    let mut chars = template.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                out.push('\\');
                if let Some(next) = chars.next() {
                    out.push(next);
                }
            }
            '{' => {
                let mut name = String::new();
                let mut closed = false;
                while let Some(inner) = chars.next() {
                    match inner {
                        '\\' => {
                            if let Some(next) = chars.next() {
                                name.push(next);
                            }
                        }
                        '}' => {
                            closed = true;
                            break;
                        }
                        other => name.push(other),
                    }
                }
                out.push('{');
                for ch in f(&name).chars() {
                    if matches!(ch, '{' | '}' | '\\') {
                        out.push('\\');
                    }
                    out.push(ch);
                }
                if closed {
                    out.push('}');
                }
            }
            other => out.push(other),
        }
    }
    out
}

fn parse_logical_source(
    store: &TripleStore,
    ls_iri: &str,
    graph: Option<&str>,
) -> Result<LogicalSource, String> {
    let get = |pred: &str| get_objects(store, ls_iri, pred, graph);

    // `rr:tableName` / `rml:query` / `rr:sqlQuery` describe a relational source.
    let table_name = get(&format!("{RR}tableName")).into_iter().next();
    let query = get(&format!("{RML}query"))
        .into_iter()
        .next()
        .or_else(|| get(&format!("{RR}sqlQuery")).into_iter().next());

    // rml:source — a datasource IRI, a file path/URL, or inline data.
    let source_terms = get_objects_typed(store, ls_iri, &format!("{RML}source"), graph);
    let source = match source_terms.into_iter().next() {
        Some((value, true)) if value.starts_with(DATASOURCE_PREFIX) => SourceRef::Datasource(value),
        Some((value, _)) => SourceRef::File(value),
        // R2RML's `rr:logicalTable` names no source: the datasource is the
        // one the run supplies. Only legal with a table or query.
        None if table_name.is_some() || query.is_some() => SourceRef::Datasource(String::new()),
        None => return Err("Missing rml:source".to_string()),
    };

    let mut formulation = get(&format!("{RML}referenceFormulation"))
        .into_iter()
        .next()
        .map(|iri| ReferenceFormulation::from_iri(&iri))
        .unwrap_or(ReferenceFormulation::Csv);
    // A datasource source IS relational, whatever (if anything) the document
    // declared: the reference formulation is advisory, the source is not.
    if matches!(source, SourceRef::Datasource(_)) {
        formulation = ReferenceFormulation::Sql;
    }
    if formulation == ReferenceFormulation::Sql && query.is_none() && table_name.is_none() {
        return Err(
            "a relational logical source needs rr:tableName or rml:query / rr:sqlQuery".to_string(),
        );
    }

    let iterator = get(&format!("{RML}iterator")).into_iter().next();

    Ok(LogicalSource {
        source,
        reference_formulation: formulation,
        iterator,
        query,
        table_name,
    })
}

fn term_type_from_iri(iri: &str, default: TermType) -> TermType {
    match iri {
        i if i.ends_with("IRI") || i.ends_with("URI") => TermType::IRI,
        i if i.ends_with("BlankNode") => TermType::BlankNode,
        i if i.ends_with("Literal") => TermType::Literal,
        _ => default,
    }
}

/// Get all object values for (subject, predicate) in the given graph context.
///
/// Resolves through the raw quad index (`objects_for_subject_in_graph`) rather
/// than a SPARQL query, because `subject` may be a *stored* blank node — inline
/// term maps written with the standard idiom (`rr:subjectMap [ … ]`,
/// `rr:predicateObjectMap [ … ]`, `rr:objectMap [ … ]`). SPARQL surface syntax
/// cannot name a specific stored blank node (`_:x` in a query is a fresh
/// existential), so the old query form matched EVERY blank node carrying the
/// predicate and cross-contaminated inline mappings. This mirrors the fix the
/// SHACL shape loader already applies.
fn get_objects(
    store: &TripleStore,
    subject: &str,
    predicate: &str,
    graph: Option<&str>,
) -> Vec<String> {
    get_objects_typed(store, subject, predicate, graph)
        .into_iter()
        .map(|(value, _)| value)
        .collect()
}

/// The objects of (subject, predicate) as RDF terms, for the places where the
/// kind of term is the meaning: `rr:constant` and its shortcuts.
fn get_terms(
    store: &TripleStore,
    subject: &str,
    predicate: &str,
    graph: Option<&str>,
) -> Vec<Term> {
    store.objects_for_subject_in_graph(subject, predicate, graph)
}

/// Like [`get_objects`], but keeps whether each object was an IRI (`true`) or a
/// literal — the difference between `rml:source <urn:source:x>` (a registered
/// datasource) and `rml:source "x.csv"` (a file part name).
fn get_objects_typed(
    store: &TripleStore,
    subject: &str,
    predicate: &str,
    graph: Option<&str>,
) -> Vec<(String, bool)> {
    store
        .objects_for_subject_in_graph(subject, predicate, graph)
        .into_iter()
        .filter_map(|t| match t {
            Term::NamedNode(n) => Some((n.as_str().to_string(), true)),
            Term::BlankNode(b) => Some((format!("_:{}", b.as_str()), false)),
            Term::Literal(l) => Some((l.value().to_string(), false)),
            #[cfg(feature = "rdf-12")]
            Term::Triple(_) => None,
        })
        .collect()
}

/// Run a SELECT query and return a named column's values as strings.
fn query_col(store: &TripleStore, sparql: &str, col: &str) -> Vec<String> {
    use oxigraph::sparql::QueryResults;

    match store.query(sparql) {
        Ok(QueryResults::Solutions(mut solutions)) => {
            let mut results = Vec::new();
            while let Some(Ok(sol)) = solutions.next() {
                if let Some(term) = sol.get(col) {
                    let s = match term {
                        Term::NamedNode(n) => n.as_str().to_string(),
                        Term::BlankNode(b) => format!("_:{}", b.as_str()),
                        Term::Literal(l) => l.value().to_string(),
                        #[cfg(feature = "rdf-12")]
                        Term::Triple(_) => continue,
                    };
                    results.push(s);
                }
            }
            results
        }
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PFX: &str = "@prefix rr: <http://www.w3.org/ns/r2rml#> .\n\
@prefix rml: <http://semweb.mmlab.be/ns/rml#> .\n\
@prefix ql: <http://semweb.mmlab.be/ns/ql#> .\n\
@prefix fnml: <http://semweb.mmlab.be/ns/fnml#> .\n\
@prefix fno: <https://w3id.org/function/ontology#> .\n\
@prefix fn: <https://w3id.org/open-triplestore/fn#> .\n\
@prefix ex: <http://example.org/> .\n";

    #[test]
    fn a_datasource_logical_source_is_relational() {
        let m = parse_rml(&format!(
            "{PFX}
             ex:M a rr:TriplesMap ;
               rml:logicalSource [ rml:source <urn:source:legacy> ; rr:tableName \"products\" ] ;
               rr:subjectMap [ rr:template \"http://x/{{id}}\" ] ;
               rr:predicateObjectMap [ rr:predicate ex:p ; rr:objectMap [ rr:column \"name\" ] ] ."
        ))
        .expect("parses");
        let tm = &m.triples_maps[0];
        assert_eq!(
            tm.logical_source.source,
            SourceRef::Datasource("urn:source:legacy".into())
        );
        assert_eq!(
            tm.logical_source.reference_formulation,
            ReferenceFormulation::Sql
        );
        assert_eq!(tm.logical_source.table_name.as_deref(), Some("products"));
        assert!(m.has_sql_source());
        assert_eq!(m.datasources(), vec!["urn:source:legacy"]);
    }

    #[test]
    fn a_file_logical_source_still_parses_as_before() {
        let m = parse_rml(&format!(
            "{PFX}
             ex:M a rr:TriplesMap ;
               rml:logicalSource [ rml:source \"people.csv\" ; rml:referenceFormulation ql:CSV ] ;
               rr:subjectMap [ rr:template \"http://x/{{id}}\" ] ;
               rr:predicateObjectMap [ rr:predicate ex:p ; rr:objectMap [ rml:reference \"name\" ] ] ."
        ))
        .unwrap();
        let ls = &m.triples_maps[0].logical_source;
        assert_eq!(ls.source, SourceRef::File("people.csv".into()));
        assert_eq!(ls.reference_formulation, ReferenceFormulation::Csv);
        assert!(!m.has_sql_source());
    }

    #[test]
    fn a_relational_source_without_a_table_or_query_is_an_error() {
        let err = parse_rml(&format!(
            "{PFX}
             ex:M a rr:TriplesMap ;
               rml:logicalSource [ rml:source <urn:source:legacy> ] ;
               rr:subjectMap [ rr:template \"http://x/{{id}}\" ] ."
        ))
        .unwrap_err();
        assert!(err.contains("rr:tableName"), "{err}");
    }

    #[test]
    fn referencing_object_maps_carry_their_join_conditions() {
        let m = parse_rml(&format!(
            "{PFX}
             ex:Child a rr:TriplesMap ;
               rml:logicalSource [ rml:source <urn:source:s> ; rr:tableName \"child\" ] ;
               rr:subjectMap [ rr:template \"http://x/c{{cid}}\" ] ;
               rr:predicateObjectMap [ rr:predicate ex:parent ; rr:objectMap [
                  rr:parentTriplesMap ex:Parent ;
                  rr:joinCondition [ rr:child \"pid\" ; rr:parent \"id\" ] ] ] .
             ex:Parent a rr:TriplesMap ;
               rml:logicalSource [ rml:source <urn:source:s> ; rr:tableName \"parent\" ] ;
               rr:subjectMap [ rr:template \"http://x/p{{id}}\" ] ."
        ))
        .expect("parses");
        let child = m.find("http://example.org/Child").unwrap();
        let ObjectMap::Ref(r) = &child.predicate_object_maps[0].object else {
            panic!("expected a referencing object map");
        };
        assert_eq!(r.parent_triples_map, "http://example.org/Parent");
        assert_eq!(
            r.joins,
            vec![JoinCondition {
                child: "pid".into(),
                parent: "id".into()
            }]
        );
    }

    #[test]
    fn a_reference_to_a_missing_parent_is_a_mapping_error() {
        let err = parse_rml(&format!(
            "{PFX}
             ex:Child a rr:TriplesMap ;
               rml:logicalSource [ rml:source <urn:source:s> ; rr:tableName \"child\" ] ;
               rr:subjectMap [ rr:template \"http://x/c{{cid}}\" ] ;
               rr:predicateObjectMap [ rr:predicate ex:parent ; rr:objectMap [
                  rr:parentTriplesMap ex:Nope ;
                  rr:joinCondition [ rr:child \"pid\" ; rr:parent \"id\" ] ] ] ."
        ))
        .unwrap_err();
        assert!(err.contains("does not define"), "{err}");
    }

    #[test]
    fn a_joinless_reference_across_different_sources_is_refused() {
        let err = parse_rml(&format!(
            "{PFX}
             ex:Child a rr:TriplesMap ;
               rml:logicalSource [ rml:source <urn:source:s> ; rr:tableName \"child\" ] ;
               rr:subjectMap [ rr:template \"http://x/c{{cid}}\" ] ;
               rr:predicateObjectMap [ rr:predicate ex:parent ;
                  rr:objectMap [ rr:parentTriplesMap ex:Parent ] ] .
             ex:Parent a rr:TriplesMap ;
               rml:logicalSource [ rml:source <urn:source:s> ; rr:tableName \"parent\" ] ;
               rr:subjectMap [ rr:template \"http://x/p{{id}}\" ] ."
        ))
        .unwrap_err();
        assert!(err.contains("rr:joinCondition"), "{err}");
    }

    #[test]
    fn a_function_object_map_collects_its_parameters() {
        let m = parse_rml(&format!(
            "{PFX}
             ex:M a rr:TriplesMap ;
               rml:logicalSource [ rml:source <urn:source:s> ; rr:tableName \"t\" ] ;
               rr:subjectMap [ rr:template \"http://x/{{id}}\" ] ;
               rr:predicateObjectMap [ rr:predicate ex:status ; rr:objectMap [
                 fnml:functionValue [
                   rr:predicateObjectMap [ rr:predicate fno:executes ; rr:object fn:mapValue ] ;
                   rr:predicateObjectMap [ rr:predicate fn:value ; rr:objectMap [ rr:column \"status\" ] ] ;
                   rr:predicateObjectMap [ rr:predicate fn:normalize ; rr:object \"lower_trim\" ] ;
                   rr:predicateObjectMap [ rr:predicate fn:mapping ; rr:object \"a=http://x/A\" ] ;
                   rr:predicateObjectMap [ rr:predicate fn:mapping ; rr:object \"b=http://x/B\" ] ;
                   rr:predicateObjectMap [ rr:predicate fn:unmapped ; rr:object \"literal\" ] ] ] ] ."
        ))
        .expect("parses");
        let ObjectMap::Function(f) = &m.triples_maps[0].predicate_object_maps[0].object else {
            panic!("expected a function object map");
        };
        assert_eq!(f.function, "https://w3id.org/open-triplestore/fn#mapValue");
        assert_eq!(
            f.first("https://w3id.org/open-triplestore/fn#value"),
            Some(&FunctionArg::Reference("status".into()))
        );
        assert_eq!(
            f.all("https://w3id.org/open-triplestore/fn#mapping").len(),
            2
        );
        assert_eq!(
            f.first("https://w3id.org/open-triplestore/fn#unmapped"),
            Some(&FunctionArg::Constant("literal".into()))
        );
    }

    #[test]
    fn a_function_without_fno_executes_is_an_error() {
        let err = parse_rml(&format!(
            "{PFX}
             ex:M a rr:TriplesMap ;
               rml:logicalSource [ rml:source <urn:source:s> ; rr:tableName \"t\" ] ;
               rr:subjectMap [ rr:template \"http://x/{{id}}\" ] ;
               rr:predicateObjectMap [ rr:predicate ex:s ; rr:objectMap [
                 fnml:functionValue [
                   rr:predicateObjectMap [ rr:predicate fn:value ; rr:object \"x\" ] ] ] ] ."
        ))
        .unwrap_err();
        assert!(err.contains("fno:executes"), "{err}");
    }

    #[test]
    fn a_literal_constant_is_refused_where_only_an_iri_may_stand() {
        for (placement, construct) in [
            ("rr:subject \"s\" ;", "subject map"),
            (
                "rr:subjectMap [ rr:template \"http://x/{id}\" ] ;
                 rr:predicateObjectMap [ rr:predicate \"p\" ; rr:object \"o\" ] ;",
                "predicate map",
            ),
            (
                "rr:subjectMap [ rr:template \"http://x/{id}\" ; rr:graph \"g\" ] ;",
                "graph map",
            ),
        ] {
            let err = parse_rml(&format!(
                "{PFX}
                 ex:M a rr:TriplesMap ;
                   rml:logicalSource [ rml:source \"d.csv\" ; rml:referenceFormulation ql:CSV ] ;
                   {placement} ."
            ))
            .unwrap_err();
            assert!(err.contains(construct), "{construct}: {err}");
        }
    }

    #[test]
    fn graph_maps_are_read_from_every_placement() {
        let m = parse_rml(&format!(
            "{PFX}
             ex:M a rr:TriplesMap ;
               rml:logicalSource [ rml:source \"d.csv\" ; rml:referenceFormulation ql:CSV ] ;
               rr:graphMap [ rr:constant ex:OnTheMap ] ;
               rr:subjectMap [ rr:template \"http://x/{{id}}\" ; rr:graph ex:G1 ;
                               rr:graphMap [ rr:template \"http://x/g/{{t}}\" ] ] ;
               rr:predicateObjectMap [ rr:predicate ex:p ; rr:object ex:o ;
                                       rr:graph ex:G2, ex:G3 ] ."
        ))
        .unwrap();
        let tm = &m.triples_maps[0];
        assert_eq!(
            tm.subject_map.graph_maps.len(),
            3,
            "rr:graphMap, rr:graph, and the triples map's own"
        );
        assert_eq!(tm.predicate_object_maps[0].graph_maps.len(), 2);
        assert!(m.has_graph_maps());
    }

    #[test]
    fn sql_column_names_lose_their_delimiters() {
        let m = parse_rml(&format!(
            "{PFX}
             ex:C a rr:TriplesMap ;
               rml:logicalSource [ rml:source <urn:source:s> ; rr:tableName \"\\\"Child\\\"\" ] ;
               rr:subjectMap [ rr:template \"http://x/{{\\\"ID\\\"}}/{{plain}}\" ] ;
               rr:predicateObjectMap [ rr:predicate ex:n ; rr:objectMap [ rr:column \"`Name`\" ] ] ;
               rr:predicateObjectMap [ rr:predicate ex:p ; rr:objectMap [
                  rr:parentTriplesMap ex:P ;
                  rr:joinCondition [ rr:child \"[PID]\" ; rr:parent \"\\\"ID\\\"\" ] ] ] .
             ex:P a rr:TriplesMap ;
               rml:logicalSource [ rml:source <urn:source:s> ; rr:tableName \"parent\" ] ;
               rr:subjectMap [ rr:template \"http://x/p{{ID}}\" ] ."
        ))
        .unwrap();
        let c = m.find("http://example.org/C").unwrap();
        assert!(
            matches!(&c.subject_map.term_map.kind, TermMapKind::Template(t) if t == "http://x/{ID}/{plain}")
        );
        assert_eq!(
            c.logical_source.table_name.as_deref(),
            Some("\"Child\""),
            "the table keeps its spelling; the dialect re-quotes it"
        );
        let n = c
            .predicate_object_maps
            .iter()
            .find_map(|p| match &p.object {
                ObjectMap::Term(t) => Some(t),
                _ => None,
            })
            .unwrap();
        assert!(matches!(&n.kind, TermMapKind::Reference(r) if r == "Name"));
        let r = c
            .predicate_object_maps
            .iter()
            .find_map(|p| match &p.object {
                ObjectMap::Ref(r) => Some(r),
                _ => None,
            })
            .unwrap();
        assert_eq!(
            r.joins,
            vec![JoinCondition {
                child: "PID".into(),
                parent: "ID".into()
            }]
        );
    }

    #[test]
    fn template_object_defaults_follow_the_semantics() {
        let doc = format!(
            "{PFX}
             ex:M a rr:TriplesMap ;
               rml:logicalSource [ rml:source \"d.csv\" ; rml:referenceFormulation ql:CSV ] ;
               rr:subjectMap [ rr:template \"http://x/{{id}}\" ] ;
               rr:predicateObjectMap [ rr:predicate ex:t ; rr:objectMap [ rr:template \"http://x/{{a}}\" ] ] ;
               rr:predicateObjectMap [ rr:predicate ex:d ; rr:objectMap [ rr:template \"{{a}}\" ; rr:datatype ex:dt ] ] ;
               rr:predicateObjectMap [ rr:predicate ex:c ; rr:objectMap [ rml:reference \"a\" ] ] ;
               rr:predicateObjectMap [ rr:predicate ex:i ; rr:objectMap [ rml:reference \"a\" ; rr:termType rr:IRI ] ] ."
        );
        let store = TripleStore::in_memory().unwrap();
        store
            .load_str(&doc, oxigraph::io::RdfFormat::Turtle, None)
            .unwrap();
        // Predicate-object maps come back in store order; sort by predicate.
        let types = |s: Semantics| -> Vec<TermType> {
            let m = parse_from_store_as(&store, None, s).unwrap();
            let mut by_predicate: Vec<(String, TermType)> = m.triples_maps[0]
                .predicate_object_maps
                .iter()
                .map(|p| match (&p.predicate_map.kind, &p.object) {
                    (TermMapKind::Constant(pred), ObjectMap::Term(t)) => {
                        (pred.to_string(), t.term_type.clone())
                    }
                    _ => panic!(),
                })
                .collect();
            by_predicate.sort_by(|a, b| a.0.cmp(&b.0));
            by_predicate.into_iter().map(|(_, t)| t).collect()
        };
        use TermType::*;
        // R2RML §7.4: a template object is an IRI unless it declares a
        // datatype or language; a column is a literal; explicit wins.
        // In predicate order: ex:c (column), ex:d (datatype), ex:i (explicit
        // IRI), ex:t (template).
        assert_eq!(types(Semantics::R2rml), vec![Literal, Literal, IRI, IRI]);
        assert_eq!(
            types(Semantics::Legacy),
            vec![Literal, Literal, IRI, Literal]
        );
    }

    #[test]
    fn a_document_with_no_triples_map_is_an_error() {
        let err = parse_rml(&format!("{PFX}ex:X a ex:NotAMapping .")).unwrap_err();
        assert!(err.contains("no rr:TriplesMap"), "{err}");
    }
}
