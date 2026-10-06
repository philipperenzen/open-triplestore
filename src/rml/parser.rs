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
/// The RML-Core / RML-IO namespace. Its terms with an R2RML or legacy RML
/// counterpart are rewritten to it before parsing ([`super::vocab`]); the
/// parser reads the rest here.
const RML_CORE: &str = "http://w3id.org/rml/";
const CSVW: &str = "http://www.w3.org/ns/csvw#";
const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
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
    /// `rml:datatypeMap` (RML-Core): generates datatype IRIs.
    Datatype,
}

impl Position {
    fn name(self) -> &'static str {
        match self {
            Position::Subject => "subject map",
            Position::Predicate => "predicate map",
            Position::Object => "object map",
            Position::Graph => "graph map",
            Position::Datatype => "datatype map",
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
    // RML-Core terms become their R2RML / legacy RML counterparts, in a copy.
    let (normalised, vocabulary) = super::vocab::normalise(store, graph)?;
    let store = &normalised;

    // Find all TriplesMap IRIs
    let q = format!("SELECT ?tm WHERE {{ ?tm a <{RR}TriplesMap> }}");
    let mut tm_iris = query_col(store, &q, "tm");
    // Deterministic order: a mapping's triples maps run (and their parent
    // indexes build) in a stable sequence, so two runs of the same mapping
    // produce the same blank-node labelling and the same log.
    tm_iris.sort();
    tm_iris.dedup();

    let ctx = Ctx {
        store,
        graph: None,
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
        rml_core: vocabulary.rml_core,
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
        for r in tm.refs() {
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

/// Whether two logical sources are the same source read the same way —
/// "effectively equal" (RML-Core §joins): the same source, table or query,
/// iterator and reference formulation. A join-less reference resolves from
/// the child's own row, which is only meaningful when that row is also one of
/// the parent's.
fn same_logical_source(a: &LogicalSource, b: &LogicalSource) -> bool {
    a.source == b.source
        && a.query == b.query
        && a.table_name == b.table_name
        && a.iterator == b.iterator
        && a.reference_formulation == b.reference_formulation
        && a.nulls == b.nulls
        && a.access == b.access
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
        // Exactly one logical source: rml:logicalSource, or R2RML's
        // rr:logicalTable (R2RML §6). Taking the first of two silently mapped
        // whichever the store happened to list first.
        let mut sources = self.objects(tm_iri, &format!("{RML}logicalSource"));
        sources.extend(self.objects(tm_iri, &format!("{RR}logicalTable")));
        let ls_iri = match sources.len() {
            0 => return Err("Missing rml:logicalSource or rr:logicalTable".to_string()),
            1 => sources.remove(0),
            n => {
                return Err(format!(
                    "the triples map has {n} logical sources (rml:logicalSource / \
                     rr:logicalTable); it reads exactly one"
                ))
            }
        };
        let logical_source = self.logical_source(&ls_iri)?;

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

        let base_iri = match self.single(
            tm_iri,
            &[format!("{RML_CORE}baseIRI"), format!("{RML}baseIRI")],
            "triples map",
        )? {
            Some(Term::NamedNode(n)) => Some(n.into_string()),
            Some(_) => return Err("rml:baseIRI must be an IRI".to_string()),
            None => None,
        };

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

    fn logical_source(&self, ls_iri: &str) -> Result<LogicalSource, String> {
        const OWNER: &str = "logical source";
        let text = |t: Term| match t {
            Term::Literal(l) => l.value().to_string(),
            Term::NamedNode(n) => n.into_string(),
            other => other.to_string(),
        };

        // The reference formulation: an IRI (`ql:CSV`, `rml:JSONPath`, …), or
        // RML-IO's description of one — `[ a rml:XPathReferenceFormulation ;
        // rml:namespace [ … ] ]`.
        let mut namespaces: Vec<(String, String)> = Vec::new();
        let declared = match self.single(ls_iri, &[format!("{RML}referenceFormulation")], OWNER)? {
            None => None,
            Some(Term::NamedNode(n)) => Some(n.into_string()),
            Some(Term::BlankNode(b)) => {
                let node = format!("_:{}", b.as_str());
                for ns in self.objects(&node, &format!("{RML_CORE}namespace")) {
                    let one = |p: &str| -> Result<String, String> {
                        match self.single(&ns, &[format!("{RML_CORE}{p}")], "rml:namespace")? {
                            Some(t) => Ok(text(t)),
                            None => Err(format!("an rml:namespace is missing rml:{p}")),
                        }
                    };
                    namespaces.push((one("namespacePrefix")?, one("namespaceURL")?));
                }
                let types = self.objects(&node, RDF_TYPE);
                Some(
                    types
                        .into_iter()
                        .next()
                        .unwrap_or_else(|| format!("{RML_CORE}XPathReferenceFormulation")),
                )
            }
            Some(other) => {
                return Err(format!(
                    "rml:referenceFormulation {other} is neither an IRI nor a description"
                ))
            }
        };
        let rml_io_sql = declared.as_deref().and_then(|f| {
            f.strip_prefix(RML_CORE)
                .filter(|l| matches!(*l, "SQL2008Table" | "SQL2008Query"))
        });

        let iterator = self
            .single(ls_iri, &[format!("{RML}iterator")], OWNER)?
            .map(text);

        // `rr:tableName` / `rml:query` / `rr:sqlQuery` describe a relational
        // source: a table or an R2RML view, never both (R2RML §5). RML-IO says
        // the same with `rml:SQL2008Table` / `rml:SQL2008Query` and the table
        // or query as the iterator.
        let mut table_name = self
            .single(ls_iri, &[format!("{RR}tableName")], OWNER)?
            .map(text);
        let mut query = self
            .single(
                ls_iri,
                &[format!("{RML}query"), format!("{RR}sqlQuery")],
                OWNER,
            )?
            .map(text);
        let mut iterator = iterator;
        match rml_io_sql {
            Some("SQL2008Table") if table_name.is_none() => table_name = iterator.take(),
            Some("SQL2008Query") if query.is_none() => query = iterator.take(),
            _ => {}
        }
        if table_name.is_some() && query.is_some() {
            return Err(
                "the logical source has both rr:tableName and a query (rr:sqlQuery / rml:query); \
                 it reads a table or an R2RML view, not both (R2RML §5)"
                    .to_string(),
            );
        }

        // rml:source — a datasource IRI, a file part name / path, or an
        // RML-IO source description.
        let source_term = self.single(ls_iri, &[format!("{RML}source")], OWNER)?;
        let mut access = Access {
            namespaces,
            ..Access::default()
        };
        let mut nulls: Vec<String> = Vec::new();
        let source = match &source_term {
            Some(Term::NamedNode(n)) if n.as_str().starts_with(DATASOURCE_PREFIX) => {
                SourceRef::Datasource(n.as_str().to_string())
            }
            Some(Term::NamedNode(n)) => {
                let node = n.as_str().to_string();
                match self.source_description(&node, &mut access, &mut nulls)? {
                    Some(path) => SourceRef::File(path),
                    None => SourceRef::File(node),
                }
            }
            Some(Term::BlankNode(b)) => {
                let node = format!("_:{}", b.as_str());
                match self.source_description(&node, &mut access, &mut nulls)? {
                    Some(path) => SourceRef::File(path),
                    None => SourceRef::File(node),
                }
            }
            Some(Term::Literal(l)) => SourceRef::File(file_key(l.value())),
            #[cfg(feature = "rdf-12")]
            Some(Term::Triple(_)) => return Err("rml:source cannot be a triple term".to_string()),
            // R2RML's `rr:logicalTable` names no source: the datasource is the
            // one the run supplies. Only legal with a table or query.
            None if table_name.is_some() || query.is_some() => SourceRef::Datasource(String::new()),
            None => return Err("Missing rml:source".to_string()),
        };
        if let SourceRef::File(path) = &source {
            let lower = path.to_ascii_lowercase();
            access.json_lines = lower.ends_with(".jsonl")
                || lower.ends_with(".ndjson")
                || lower.ends_with(".jsonl.gz")
                || lower.ends_with(".ndjson.gz");
        }

        let mut formulation = declared
            .as_deref()
            .map(ReferenceFormulation::from_iri)
            .unwrap_or(ReferenceFormulation::Csv);
        // A datasource source IS relational, whatever (if anything) the document
        // declared: the reference formulation is advisory, the source is not.
        if matches!(source, SourceRef::Datasource(_)) {
            formulation = ReferenceFormulation::Sql;
        }
        if formulation == ReferenceFormulation::Sql && query.is_none() && table_name.is_none() {
            return Err(
                "a relational logical source needs rr:tableName or rml:query / rr:sqlQuery"
                    .to_string(),
            );
        }
        if formulation == ReferenceFormulation::Csv && iterator.is_some() {
            // RML-IO: a CSV source iterates by row and takes no iterator.
            iterator = None;
        }

        // `rml:null` (RML-IO): source values that count as NULL. Read on the
        // logical source, where this engine's sources are described, and on
        // the source description (done above).
        self.nulls_of(ls_iri, &mut nulls)?;

        Ok(LogicalSource {
            source,
            reference_formulation: formulation,
            iterator,
            query,
            table_name,
            nulls,
            access,
        })
    }

    /// `rml:null` values on `node`, added to `nulls`.
    fn nulls_of(&self, node: &str, nulls: &mut Vec<String>) -> Result<(), String> {
        for p in [format!("{RML_CORE}null"), format!("{RML}null")] {
            for t in self.terms(node, &p) {
                match t {
                    Term::Literal(l) => {
                        if !nulls.iter().any(|n| n == l.value()) {
                            nulls.push(l.value().to_string());
                        }
                    }
                    other => {
                        return Err(format!(
                            "rml:null {other} is not a string; it names a value that counts \
                             as NULL"
                        ))
                    }
                }
            }
        }
        Ok(())
    }

    /// Read an RML-IO source description (`rml:Source`, `rml:FilePath`,
    /// `rml:RelativePathSource`, `csvw:Table`): the file it names, its
    /// encoding, compression and NULL values, and a CSVW dialect. `Ok(None)`
    /// when `node` describes nothing — a plain IRI naming a file.
    fn source_description(
        &self,
        node: &str,
        access: &mut Access,
        nulls: &mut Vec<String>,
    ) -> Result<Option<String>, String> {
        const OWNER: &str = "source description";
        let literal = |t: Term| match t {
            Term::Literal(l) => l.value().to_string(),
            Term::NamedNode(n) => n.into_string(),
            other => other.to_string(),
        };
        let types: Vec<String> = self.objects(node, RDF_TYPE);
        for unsupported in ["http://www.wiwiss.fu-berlin.de/suhl/bizer/D2RQ/0.1#Database"] {
            if types.iter().any(|t| t == unsupported)
                || !self
                    .objects(
                        node,
                        "http://www.wiwiss.fu-berlin.de/suhl/bizer/D2RQ/0.1#jdbcDSN",
                    )
                    .is_empty()
            {
                return Err(
                    "a D2RQ database description names its connection in the mapping; register \
                     the database as a datasource and name it rml:source <urn:source:…> instead"
                        .to_string(),
                );
            }
        }
        if types
            .iter()
            .any(|t| t == "http://www.w3.org/ns/sparql-service-description#Service")
        {
            return Err(
                "a SPARQL endpoint source is mapped through a registered datasource, not from \
                 uploaded files"
                    .to_string(),
            );
        }

        let path = self
            .single(node, &[format!("{RML_CORE}path")], OWNER)?
            .map(literal);
        let url = self
            .single(node, &[format!("{CSVW}url")], OWNER)?
            .map(literal);
        let file = path.or(url).map(|p| file_key(&p));

        match self
            .single(node, &[format!("{RML_CORE}encoding")], OWNER)?
            .map(literal)
            .as_deref()
        {
            None => {}
            Some(e) => access.encoding = Some(encoding_label(e)?),
        }
        access.compression = match self
            .single(node, &[format!("{RML_CORE}compression")], OWNER)?
            .map(literal)
            .as_deref()
            .map(|c| c.strip_prefix(RML_CORE).unwrap_or(c))
        {
            None | Some("none") => Compression::None,
            Some("gzip") => Compression::Gzip,
            Some("zip") => Compression::Zip,
            Some("tarxz") => Compression::TarXz,
            Some("targz") => Compression::TarGz,
            Some(other) => {
                return Err(format!(
                    "rml:compression {other} is not rml:none, rml:gzip, rml:zip, rml:tarxz or \
                     rml:targz"
                ))
            }
        };
        self.nulls_of(node, nulls)?;

        // CSVW: the dialect, and NULL values on the table.
        let is_table = types.iter().any(|t| t == &format!("{CSVW}Table"))
            || !self.objects(node, &format!("{CSVW}dialect")).is_empty();
        if is_table {
            for t in self.terms(node, &format!("{CSVW}null")) {
                if let Term::Literal(l) = t {
                    access.dialect.nulls.push(l.value().to_string());
                }
            }
            if let Some(d) = self
                .single(node, &[format!("{CSVW}dialect")], OWNER)?
                .map(|t| match t {
                    Term::BlankNode(b) => format!("_:{}", b.as_str()),
                    Term::NamedNode(n) => n.into_string(),
                    other => other.to_string(),
                })
            {
                self.csv_dialect(&d, access)?;
            }
        }
        Ok(file)
    }

    /// A `csvw:Dialect`.
    fn csv_dialect(&self, node: &str, access: &mut Access) -> Result<(), String> {
        const OWNER: &str = "csvw:dialect";
        let get = |p: &str| -> Result<Option<String>, String> {
            Ok(self
                .single(node, &[format!("{CSVW}{p}")], OWNER)?
                .map(|t| match t {
                    Term::Literal(l) => l.value().to_string(),
                    other => other.to_string(),
                }))
        };
        let byte = |p: &str, v: String| -> Result<u8, String> {
            let v = if v == "\\t" { "\t".to_string() } else { v };
            match v.as_bytes() {
                [b] => Ok(*b),
                _ => Err(format!(
                    "csvw:{p} \"{v}\" must be a single ASCII character for this engine"
                )),
            }
        };
        let boolean = |p: &str, v: String| -> Result<bool, String> {
            match v.trim() {
                "true" | "1" => Ok(true),
                "false" | "0" => Ok(false),
                other => Err(format!("csvw:{p} \"{other}\" is not a boolean")),
            }
        };
        let count = |p: &str, v: String| -> Result<usize, String> {
            v.trim()
                .parse()
                .map_err(|_| format!("csvw:{p} \"{v}\" is not a non-negative integer"))
        };
        let d = &mut access.dialect;
        if let Some(v) = get("delimiter")? {
            d.delimiter = byte("delimiter", v)?;
        }
        if let Some(v) = get("quoteChar")? {
            d.quote = byte("quoteChar", v)?;
        }
        if let Some(v) = get("doubleQuote")? {
            d.double_quote = boolean("doubleQuote", v)?;
        }
        if let Some(v) = get("header")? {
            d.header = boolean("header", v)?;
        }
        if let Some(v) = get("headerRowCount")? {
            d.header_rows = count("headerRowCount", v)?;
            d.header = d.header_rows > 0;
        }
        if let Some(v) = get("skipRows")? {
            d.skip_rows = count("skipRows", v)?;
        }
        if let Some(v) = get("commentPrefix")? {
            d.comment = Some(byte("commentPrefix", v)?);
        }
        if let Some(v) = get("trim")? {
            d.trim = matches!(v.trim(), "true" | "1" | "start" | "end");
        }
        if let Some(v) = get("encoding")? {
            if access.encoding.is_none() {
                access.encoding = Some(encoding_label(&v)?);
            }
        }
        Ok(())
    }

    fn subject_map(&self, tm_iri: &str) -> Result<SubjectMap, String> {
        // Exactly one subject map: `rr:subjectMap` or the `rr:subject`
        // shortcut (R2RML §6).
        let sm_nodes = self.objects(tm_iri, &format!("{RR}subjectMap"));
        let shortcuts = self.terms(tm_iri, &format!("{RR}subject"));
        let n = sm_nodes.len() + shortcuts.len();
        if n > 1 {
            return Err(format!(
                "the triples map has {n} subject maps (rr:subjectMap / rr:subject); it has \
                 exactly one"
            ));
        }
        let sm_node = sm_nodes.into_iter().next();
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
            TermMap::new(
                TermMapKind::Constant(Term::Literal(oxigraph::model::Literal::new_simple_literal(
                    "",
                ))),
                TermType::IRI,
            )
        } else if let Some(ref sm) = sm_node {
            self.term_map(sm, Position::Subject)?
        } else {
            let val = shortcuts
                .into_iter()
                .next()
                .ok_or("Missing rr:subjectMap or rr:subject")?;
            constant(val, Position::Subject)?
        };

        // rr:class assertions. R2RML places rr:class on the subjectMap; also accept it on
        // the TriplesMap as a convenience. Its values must be IRIs (R2RML §6.2).
        let mut class_terms = Vec::new();
        if let Some(ref sm) = sm_node {
            class_terms.extend(self.terms(sm, &format!("{RR}class")));
        }
        class_terms.extend(self.terms(tm_iri, &format!("{RR}class")));
        let mut classes = Vec::new();
        for c in class_terms {
            match c {
                Term::NamedNode(n) => classes.push(n.into_string()),
                other => {
                    return Err(format!(
                        "rr:class {other} is not an IRI; a class must be an IRI (R2RML §6.2)"
                    ))
                }
            }
        }
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
        // Every predicate map and every `rr:predicate` shortcut: the map
        // generates each predicate with each object (R2RML §11.1).
        let mut predicate_maps = Vec::new();
        for pm in self.objects(pom_node, &format!("{RR}predicateMap")) {
            predicate_maps.push(self.term_map(&pm, Position::Predicate)?);
        }
        for pred in self.terms(pom_node, &format!("{RR}predicate")) {
            predicate_maps.push(constant(pred, Position::Predicate)?);
        }
        if predicate_maps.is_empty() {
            return Err("Missing rr:predicateMap or rr:predicate".to_string());
        }

        // Every object map — a referencing map, a function, or a plain term —
        // and every `rr:object` shortcut, a constant: an IRI stays an IRI, and
        // a literal keeps its datatype and language tag.
        let mut object_maps = Vec::new();
        for om in self.objects(pom_node, &format!("{RR}objectMap")) {
            object_maps.push(self.object_map(&om)?);
        }
        for obj in self.terms(pom_node, &format!("{RR}object")) {
            object_maps.push(ObjectMap::Term(constant(obj, Position::Object)?));
        }
        if object_maps.is_empty() {
            return Err("Missing rr:objectMap or rr:object".to_string());
        }

        let graph_maps = self.graph_maps(pom_node, "predicate-object map")?;

        Ok(PredicateObjectMap {
            predicate_maps,
            object_maps,
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
        let parents = self.objects(om, &format!("{RR}parentTriplesMap"));
        let functions = self.objects(om, &format!("{FNML}functionValue"));
        let kinds = self.term_map_kinds(om);
        if parents.len() > 1 {
            return Err(format!(
                "the object map has {} values of rr:parentTriplesMap; a referencing object map \
                 names one parent",
                parents.len()
            ));
        }
        if let Some(parent) = parents.into_iter().next() {
            // A referencing object map is not a term map (R2RML §8).
            if let Some(kind) = kinds
                .first()
                .or(functions.first().map(|_| &"fnml:functionValue"))
            {
                return Err(format!(
                    "the object map has both rr:parentTriplesMap and {kind}; a referencing \
                     object map generates its parent's subject and takes no term map of its own"
                ));
            }
            let mut joins = Vec::new();
            for jc in self.objects(om, &format!("{RR}joinCondition")) {
                // `rr:child "c"` / `rr:parent "p"`, or RML-Core's
                // `rml:childMap` / `rml:parentMap`, expression maps that may
                // also be a template or a constant.
                let side = |short: &str, map: &str| -> Result<JoinSide, String> {
                    let mut v = self.objects(&jc, &format!("{RR}{short}"));
                    let maps = self.objects(&jc, &format!("{RML_CORE}{map}"));
                    match (v.len(), maps.len()) {
                        (1, 0) => Ok(JoinSide::Reference(v.remove(0))),
                        (0, 1) => self.join_side(&maps[0], map),
                        (0, 0) => Err(format!(
                            "rr:joinCondition is missing rr:{short} (or rml:{map})"
                        )),
                        (a, b) => Err(format!(
                            "rr:joinCondition has {} values of rr:{short} / rml:{map}; it takes \
                             exactly one",
                            a + b
                        )),
                    }
                };
                let child = side("child", "childMap")?;
                let parent_side = side("parent", "parentMap")?;
                joins.push(JoinCondition {
                    child,
                    parent: parent_side,
                });
            }
            joins.sort_by_key(|a| (a.child.key(), a.parent.key()));
            return Ok(ObjectMap::Ref(RefObjectMap {
                parent_triples_map: parent,
                joins,
            }));
        }

        if functions.len() > 1 {
            return Err(format!(
                "the object map has {} values of fnml:functionValue; it takes at most one",
                functions.len()
            ));
        }
        if let Some(fv) = functions.into_iter().next() {
            if let Some(kind) = kinds.first() {
                return Err(format!(
                    "the object map has both fnml:functionValue and {kind}; the function \
                     computes the term"
                ));
            }
            return self.function(om, &fv).map(ObjectMap::Function);
        }

        self.term_map(om, Position::Object).map(ObjectMap::Term)
    }

    /// An `rml:childMap` / `rml:parentMap`: a reference, a template or a
    /// constant.
    fn join_side(&self, node: &str, what: &str) -> Result<JoinSide, String> {
        let kinds = self.term_map_kinds(node);
        match kinds.as_slice() {
            ["rr:constant"] => match self.terms(node, &format!("{RR}constant")).remove(0) {
                Term::Literal(l) => Ok(JoinSide::Constant(l.value().to_string())),
                Term::NamedNode(n) => Ok(JoinSide::Constant(n.into_string())),
                other => Err(format!("the constant of rml:{what} {other} is not a value")),
            },
            ["rr:template"] => {
                let t = self.objects(node, &format!("{RR}template")).remove(0);
                validate_template(&t).map_err(|e| format!("rml:{what}: {e}"))?;
                Ok(JoinSide::Template(t))
            }
            ["rml:reference"] => Ok(JoinSide::Reference(
                self.objects(node, &format!("{RML}reference")).remove(0),
            )),
            ["rr:column"] => Ok(JoinSide::Reference(
                self.objects(node, &format!("{RR}column")).remove(0),
            )),
            [] => Err(format!(
                "rml:{what} has no rml:constant, rml:reference or rml:template"
            )),
            _ => Err(format!(
                "rml:{what} is exactly one of a constant, a reference or a template"
            )),
        }
    }

    /// An expression map that generates language tags (`rml:languageMap`):
    /// a constant folds into the term map's language, anything else is kept.
    fn language_map(&self, node: &str, owner: &str) -> Result<LanguageOrMap, String> {
        let kinds = self.term_map_kinds(node);
        match kinds.as_slice() {
            ["rr:constant"] => match self.terms(node, &format!("{RR}constant")).remove(0) {
                Term::Literal(l) => Ok(LanguageOrMap::Tag(checked_language(l.value(), owner)?)),
                other => Err(format!(
                    "the rml:languageMap of the {owner} has the constant {other}; a language \
                     tag is a string"
                )),
            },
            [_] => {
                let kind = match kinds[0] {
                    "rr:template" => {
                        let t = self.objects(node, &format!("{RR}template")).remove(0);
                        validate_template(&t)
                            .map_err(|e| format!("the rml:languageMap of the {owner}: {e}"))?;
                        TermMapKind::Template(t)
                    }
                    "rml:reference" => TermMapKind::Reference(
                        self.objects(node, &format!("{RML}reference")).remove(0),
                    ),
                    _ => {
                        TermMapKind::Reference(self.objects(node, &format!("{RR}column")).remove(0))
                    }
                };
                Ok(LanguageOrMap::Map(TermMap::new(kind, TermType::Literal)))
            }
            [] => Err(format!(
                "the rml:languageMap of the {owner} has no rml:constant, rml:reference or \
                 rml:template"
            )),
            _ => Err(format!(
                "the rml:languageMap of the {owner} is exactly one of a constant, a reference \
                 or a template"
            )),
        }
    }

    /// Which of the four term-map kinds `node` declares, one entry per value.
    fn term_map_kinds(&self, node: &str) -> Vec<&'static str> {
        let mut out = Vec::new();
        for (p, name) in [
            (format!("{RR}constant"), "rr:constant"),
            (format!("{RR}template"), "rr:template"),
            (format!("{RML}reference"), "rml:reference"),
            (format!("{RR}column"), "rr:column"),
        ] {
            out.extend(std::iter::repeat_n(name, self.terms(node, &p).len()));
        }
        out
    }

    /// The value of `predicates` on `node`, refusing more than one in all:
    /// each is single-valued on the construct it describes.
    fn single(
        &self,
        node: &str,
        predicates: &[String],
        owner: &str,
    ) -> Result<Option<Term>, String> {
        let mut values: Vec<Term> = Vec::new();
        for p in predicates {
            values.extend(self.terms(node, p));
        }
        if values.len() > 1 {
            let names: Vec<String> = predicates.iter().map(|p| curie(p)).collect();
            return Err(format!(
                "the {owner} has {} values of {}; it takes at most one",
                values.len(),
                names.join(" / ")
            ));
        }
        Ok(values.pop())
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
                    TermMapKind::Fresh => continue,
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
        let owner = position.name();
        let declared_type = match self.single(node, &[format!("{RR}termType")], owner)? {
            Some(Term::NamedNode(n)) => Some(term_type_from_iri(n.as_str()).ok_or_else(|| {
                format!(
                    "rr:termType <{}> on the {owner} is not rr:IRI, rr:BlankNode, rr:Literal, \
                     rml:URI, rml:UnsafeIRI or rml:UnsafeURI",
                    n.as_str()
                )
            })?),
            Some(other) => {
                return Err(format!(
                    "rr:termType {other} on the {owner} is not an IRI (R2RML §7.4)"
                ))
            }
            None => None,
        };
        // A term map is exactly one of a constant, a column (`rr:column` or
        // `rml:reference`) or a template (R2RML §7) — or, in RML-Core, a
        // blank-node term map with no expression at all.
        let kinds = self.term_map_kinds(node);
        match kinds.as_slice() {
            [] if declared_type == Some(TermType::BlankNode)
                && matches!(position, Position::Subject | Position::Object) =>
            {
                return Ok(TermMap::new(TermMapKind::Fresh, TermType::BlankNode));
            }
            [] => {
                return Err(format!(
                    "the {owner} has no rr:constant, rr:template, rr:column or rml:reference"
                ))
            }
            [_] => {}
            [a, b, ..] if a == b => {
                return Err(format!(
                    "the {owner} has {} values of {a}; it takes exactly one",
                    kinds.len()
                ))
            }
            [a, b, ..] => {
                return Err(format!(
                    "the {owner} has both {a} and {b}; a term map is exactly one of a constant, \
                     a column or a template (R2RML §7)"
                ))
            }
        }
        if kinds[0] == "rr:constant" {
            let c = self.terms(node, &format!("{RR}constant")).remove(0);
            return constant(c, position);
        }
        let kind = match kinds[0] {
            "rr:template" => {
                let t = self.objects(node, &format!("{RR}template")).remove(0);
                validate_template(&t).map_err(|e| format!("the {owner}: {e}"))?;
                TermMapKind::Template(t)
            }
            "rml:reference" => {
                TermMapKind::Reference(self.objects(node, &format!("{RML}reference")).remove(0))
            }
            _ => TermMapKind::Reference(self.objects(node, &format!("{RR}column")).remove(0)),
        };

        // A datatype: `rr:datatype` (or RML-Core's shortcut, the same), or an
        // `rml:datatypeMap`, whose constant is the same as the shortcut.
        let mut datatype = match self.single(node, &[format!("{RR}datatype")], owner)? {
            Some(Term::NamedNode(n)) => Some(n.into_string()),
            Some(other) => {
                return Err(format!(
                    "rr:datatype {other} on the {owner} is not an IRI (R2RML §7.6)"
                ))
            }
            None => None,
        };
        let mut datatype_map = None;
        if let Some(dm) = self
            .single(node, &[format!("{RML_CORE}datatypeMap")], owner)?
            .map(|t| node_id(&t))
        {
            if datatype.is_some() {
                return Err(format!(
                    "the {owner} has both rr:datatype and rml:datatypeMap; it takes one datatype \
                     map"
                ));
            }
            let map = self
                .term_map(&dm, Position::Datatype)
                .map_err(|e| format!("rml:datatypeMap of the {owner}: {e}"))?;
            match &map.kind {
                TermMapKind::Constant(Term::NamedNode(n)) => {
                    datatype = Some(n.as_str().to_string())
                }
                _ => datatype_map = Some(Box::new(map)),
            }
        }

        // A language: `rr:language` (or the shortcut), or an `rml:languageMap`.
        let mut language = match self.single(node, &[format!("{RR}language")], owner)? {
            Some(Term::Literal(l)) => Some(checked_language(l.value(), owner)?),
            Some(other) => {
                return Err(format!(
                    "rr:language {other} on the {owner} must be a string (R2RML §7.5)"
                ))
            }
            None => None,
        };
        let mut language_map = None;
        if let Some(lm) = self
            .single(node, &[format!("{RML}languageMap")], owner)?
            .map(|t| node_id(&t))
        {
            if language.is_some() {
                return Err(format!(
                    "the {owner} has both rr:language and rml:languageMap; it takes one language \
                     map"
                ));
            }
            match self.language_map(&lm, owner)? {
                LanguageOrMap::Tag(t) => language = Some(t),
                LanguageOrMap::Map(m) => language_map = Some(Box::new(m)),
            }
        }
        let has_datatype = datatype.is_some() || datatype_map.is_some();
        let has_language = language.is_some() || language_map.is_some();
        if has_datatype && has_language {
            return Err(format!(
                "the {owner} has both rr:language and rr:datatype; a literal carries a language \
                 tag or a datatype, not both (R2RML §7.6)"
            ));
        }

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
                        matches!(kind, TermMapKind::Reference(_)) || has_language || has_datatype
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
        let term_type = declared_type.unwrap_or(default_type);
        let iris = [
            TermType::IRI,
            TermType::URI,
            TermType::UnsafeIRI,
            TermType::UnsafeURI,
        ];
        let allowed: Vec<TermType> = match position {
            Position::Subject => iris
                .iter()
                .cloned()
                .chain(std::iter::once(TermType::BlankNode))
                .collect(),
            Position::Predicate | Position::Graph | Position::Datatype => iris.to_vec(),
            Position::Object => iris
                .iter()
                .cloned()
                .chain([TermType::BlankNode, TermType::Literal])
                .collect(),
        };
        if !allowed.contains(&term_type) {
            let names: Vec<&str> = allowed.iter().map(term_type_name).collect();
            return Err(format!(
                "the {owner} generates {}, which R2RML §7.4 does not allow there; it may \
                 generate {}",
                term_type_name(&term_type),
                names.join(" or ")
            ));
        }
        if term_type != TermType::Literal && (has_language || has_datatype) {
            return Err(format!(
                "the {owner} declares {} but generates {}; only a literal takes a language tag \
                 or a datatype (R2RML §7.5, §7.6)",
                if has_language {
                    "rr:language"
                } else {
                    "rr:datatype"
                },
                term_type_name(&term_type)
            ));
        }

        Ok(TermMap {
            kind,
            term_type,
            datatype,
            language,
            language_map,
            datatype_map,
        })
    }
}

/// Whether `tag` is a valid BCP 47 language tag (R2RML §7.5): well formed,
/// by the parser the store itself uses, and with a primary language subtag
/// that can be one. BCP 47 reserves four-letter primary subtags and leaves
/// five to eight letters for registration, and none is registered, so
/// `english` is well formed but names no language; the singletons `i` and `x`
/// start grandfathered and private-use tags.
pub(crate) fn valid_language_tag(tag: &str) -> bool {
    if oxigraph::model::Literal::new_language_tagged_literal("", tag).is_err() {
        return false;
    }
    let primary = tag.split('-').next().unwrap_or("");
    matches!(primary.len(), 2 | 3)
        || primary.eq_ignore_ascii_case("i")
        || primary.eq_ignore_ascii_case("x")
}

/// A language map's constant, or the map itself.
enum LanguageOrMap {
    Tag(String),
    Map(TermMap),
}

/// A BCP 47 language tag, validated by the parser the store itself uses.
fn checked_language(tag: &str, owner: &str) -> Result<String, String> {
    if !valid_language_tag(tag) {
        return Err(format!(
            "rr:language \"{tag}\" on the {owner} is not a valid BCP 47 language tag \
             (R2RML §7.5)"
        ));
    }
    Ok(tag.to_string())
}

/// A node as the store's lookups name it: an IRI, or `_:label`.
fn node_id(t: &Term) -> String {
    match t {
        Term::NamedNode(n) => n.as_str().to_string(),
        Term::BlankNode(b) => format!("_:{}", b.as_str()),
        other => other.to_string(),
    }
}

/// The part name a file source is looked up by: its path without a leading
/// `./` or `file://`. A remote URL stays as it is; the executor refuses it
/// unless the run supplied it as an upload part of that name.
fn file_key(path: &str) -> String {
    let p = path.trim();
    let p = p.strip_prefix("file://").unwrap_or(p);
    p.strip_prefix("./").unwrap_or(p).to_string()
}

/// An `rml:encoding` IRI or a `csvw:encoding` name as an encoding label.
fn encoding_label(value: &str) -> Result<String, String> {
    let local = value.strip_prefix(RML_CORE).unwrap_or(value);
    match encoding_rs::Encoding::for_label(local.as_bytes()) {
        Some(e) => Ok(e.name().to_string()),
        None => Err(format!(
            "the encoding {value} is not one this engine reads (rml:UTF-8, rml:UTF-16 or a \
             WHATWG encoding label)"
        )),
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
    Ok(TermMap::new(TermMapKind::Constant(value), term_type))
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
    fn fix(m: &mut TermMap) {
        match &mut m.kind {
            TermMapKind::Reference(c) => *c = column_name(c),
            TermMapKind::Template(t) => *t = map_template_columns(t, &column_name),
            TermMapKind::Constant(_) | TermMapKind::Fresh => {}
        }
        if let Some(lm) = &mut m.language_map {
            fix(lm);
        }
        if let Some(dm) = &mut m.datatype_map {
            fix(dm);
        }
    }
    let fix_side = |side: &mut JoinSide| match side {
        JoinSide::Reference(c) => *c = column_name(c),
        JoinSide::Template(t) => *t = map_template_columns(t, &column_name),
        JoinSide::Constant(_) => {}
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
        pom.predicate_maps.iter_mut().for_each(fix);
        pom.graph_maps.iter_mut().for_each(fix);
        for object in &mut pom.object_maps {
            match object {
                ObjectMap::Term(t) => fix(t),
                ObjectMap::Function(f) => fix_function(f),
                ObjectMap::Ref(r) => {
                    for j in &mut r.joins {
                        fix_side(&mut j.child);
                        fix_side(&mut j.parent);
                    }
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

/// `rr:IRI`, `rr:BlankNode` or `rr:Literal`, in the R2RML or the RML-Core
/// namespace, and RML-Core's `rml:URI`, `rml:UnsafeIRI` and `rml:UnsafeURI`.
/// Anything else is not a term type.
fn term_type_from_iri(iri: &str) -> Option<TermType> {
    if let Some(local) = iri.strip_prefix(RML_CORE) {
        match local {
            "URI" => return Some(TermType::URI),
            "UnsafeIRI" => return Some(TermType::UnsafeIRI),
            "UnsafeURI" => return Some(TermType::UnsafeURI),
            _ => {}
        }
    }
    let local = iri
        .strip_prefix(RR)
        .or_else(|| iri.strip_prefix(RML_CORE))?;
    match local {
        "IRI" => Some(TermType::IRI),
        "BlankNode" => Some(TermType::BlankNode),
        "Literal" => Some(TermType::Literal),
        _ => None,
    }
}

fn term_type_name(t: &TermType) -> &'static str {
    match t {
        TermType::IRI => "rr:IRI",
        TermType::BlankNode => "rr:BlankNode",
        TermType::Literal => "rr:Literal",
        TermType::URI => "rml:URI",
        TermType::UnsafeIRI => "rml:UnsafeIRI",
        TermType::UnsafeURI => "rml:UnsafeURI",
    }
}

/// A predicate IRI in the prefixed form error messages use.
fn curie(iri: &str) -> String {
    for (ns, prefix) in [
        (RR, "rr:"),
        (RML, "rml:"),
        (RML_CORE, "rml:"),
        (FNML, "fnml:"),
    ] {
        if let Some(local) = iri.strip_prefix(ns) {
            return format!("{prefix}{local}");
        }
    }
    format!("<{iri}>")
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
        let ObjectMap::Ref(r) = &child.predicate_object_maps[0].object_maps[0] else {
            panic!("expected a referencing object map");
        };
        assert_eq!(r.parent_triples_map, "http://example.org/Parent");
        assert_eq!(r.joins, vec![JoinCondition::columns("pid", "id")]);
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
        let ObjectMap::Function(f) = &m.triples_maps[0].predicate_object_maps[0].object_maps[0]
        else {
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
            .find_map(|p| match &p.object_maps[0] {
                ObjectMap::Term(t) => Some(t),
                _ => None,
            })
            .unwrap();
        assert!(matches!(&n.kind, TermMapKind::Reference(r) if r == "Name"));
        let r = c
            .predicate_object_maps
            .iter()
            .find_map(|p| match &p.object_maps[0] {
                ObjectMap::Ref(r) => Some(r),
                _ => None,
            })
            .unwrap();
        assert_eq!(r.joins, vec![JoinCondition::columns("PID", "ID")]);
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
                .map(|p| match (&p.predicate_maps[0].kind, &p.object_maps[0]) {
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
    fn a_predicate_object_map_keeps_every_predicate_and_object_map() {
        let m = parse_rml(&format!(
            "{PFX}
             ex:M a rr:TriplesMap ;
               rml:logicalSource [ rml:source \"d.csv\" ; rml:referenceFormulation ql:CSV ] ;
               rr:subjectMap [ rr:template \"http://x/{{id}}\" ] ;
               rr:predicateObjectMap [
                 rr:predicate ex:a, ex:b ; rr:predicateMap [ rr:template \"http://x/p/{{k}}\" ] ;
                 rr:object ex:O, \"lit\" ; rr:objectMap [ rml:reference \"v\" ] ] ."
        ))
        .unwrap();
        let pom = &m.triples_maps[0].predicate_object_maps[0];
        assert_eq!(pom.predicate_maps.len(), 3);
        assert_eq!(pom.object_maps.len(), 3);
    }

    #[test]
    fn a_document_with_no_triples_map_is_an_error() {
        let err = parse_rml(&format!("{PFX}ex:X a ex:NotAMapping .")).unwrap_err();
        assert!(err.contains("no rr:TriplesMap"), "{err}");
    }
}
