//! RML data model types.
//!
//! Represents parsed RML mapping documents as Rust structs.
//! See: <https://rml.io/specs/rml/> and R2RML <https://www.w3.org/TR/r2rml/>.

use std::collections::BTreeMap;

use oxigraph::model::Term;

/// A complete RML mapping document containing one or more TriplesMap entries.
#[derive(Debug, Clone)]
pub struct RmlMapping {
    pub triples_maps: Vec<TriplesMap>,
    /// Which term-generation rules the mapping runs under. A frozen mapping
    /// version keeps the rules it was frozen with; see [`Semantics`].
    pub semantics: Semantics,
    /// The base IRI the processor resolves relative IRIs against (R2RML
    /// §11.2), supplied by the run. A triples map's own `rml:baseIRI` wins.
    pub base_iri: Option<String>,
    /// Whether the document is written in the RML-Core / RML-IO vocabulary
    /// (`http://w3id.org/rml/`). Its JSON values carry their natural RDF
    /// datatypes (RML-IO registry: a JSON number is an `xsd:integer` or an
    /// `xsd:double`), where the legacy RML vocabulary read every value as a
    /// string; and a JSONPath reference that selects an array or an object
    /// is an error there, where the legacy vocabulary read an array's
    /// elements and an object's JSON text.
    pub rml_core: bool,
}

/// The term-generation rules a mapping runs under.
///
/// Fixing this engine's R2RML deviations changed the IRIs and blank nodes an
/// existing mapping produces, and an IRI is an identity: a run that renamed
/// every entity would look like a delete and a re-add of all of them. So a
/// frozen mapping version records the rules it was written against, and a
/// version frozen before the fix runs under [`Semantics::Legacy`]:
///
/// | rule                                   | `Legacy`                         | `R2rml` (R2RML §7.3, §7.4, §11) |
/// |----------------------------------------|----------------------------------|---------------------------------|
/// | template object map, no `rr:termType`  | literal                          | IRI                             |
/// | template value encoding                | everything but `[A-Za-z0-9]`, in every template | outside RFC 3987 `iunreserved`, IRI templates only |
/// | blank node                             | one per row and value            | one per value and graph         |
///
/// Everything else — constant term types, graph maps, base IRI, SQL
/// identifiers — follows the specification under both: the old behaviour
/// there produced wrong terms or failing SQL rather than different names.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Semantics {
    /// What this engine did before the R2RML term-semantics fix.
    Legacy,
    /// R2RML's own rules. What every newly written mapping gets.
    #[default]
    R2rml,
}

impl Semantics {
    pub fn as_str(self) -> &'static str {
        match self {
            Semantics::Legacy => "legacy",
            Semantics::R2rml => "r2rml",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim() {
            "legacy" => Some(Semantics::Legacy),
            "r2rml" => Some(Semantics::R2rml),
            _ => None,
        }
    }
}

impl RmlMapping {
    pub fn find(&self, iri: &str) -> Option<&TriplesMap> {
        self.triples_maps.iter().find(|tm| tm.iri == iri)
    }

    /// Every datasource IRI (`urn:source:<id>`) the mapping reads from.
    pub fn datasources(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .triples_maps
            .iter()
            .filter_map(|tm| match &tm.logical_source.source {
                SourceRef::Datasource(iri) => Some(iri.clone()),
                _ => None,
            })
            .collect();
        out.sort();
        out.dedup();
        out
    }

    pub fn has_sql_source(&self) -> bool {
        self.triples_maps
            .iter()
            .any(|tm| matches!(tm.logical_source.source, SourceRef::Datasource(_)))
    }

    /// The base IRI `tm`'s relative IRIs resolve against: its own
    /// `rml:baseIRI`, else the run's.
    pub fn base_for<'a>(&'a self, tm: &'a TriplesMap) -> Option<&'a str> {
        tm.base_iri.as_deref().or(self.base_iri.as_deref())
    }

    /// Whether any subject map or predicate-object map declares a graph map.
    pub fn has_graph_maps(&self) -> bool {
        self.triples_maps.iter().any(|tm| {
            !tm.subject_map.graph_maps.is_empty()
                || tm
                    .predicate_object_maps
                    .iter()
                    .any(|pom| !pom.graph_maps.is_empty())
        })
    }
}

/// An rml:TriplesMap — the unit of mapping from a logical source to RDF triples.
#[derive(Debug, Clone)]
pub struct TriplesMap {
    pub iri: String,
    pub logical_source: LogicalSource,
    pub subject_map: SubjectMap,
    pub predicate_object_maps: Vec<PredicateObjectMap>,
    /// `rml:baseIRI` on the triples map (RML-Core), overriding the run's.
    pub base_iri: Option<String>,
}

/// rml:LogicalSource — describes the source data (file, inline, database).
#[derive(Debug, Clone)]
pub struct LogicalSource {
    pub source: SourceRef,
    pub reference_formulation: ReferenceFormulation,
    pub iterator: Option<String>,
    /// `rml:query` / `rr:sqlQuery` — the statement that selects the rows.
    pub query: Option<String>,
    /// `rr:tableName` — a whole table or view as the logical source.
    pub table_name: Option<String>,
    /// `rml:null` (RML-IO): source values that count as NULL, besides the
    /// source's own NULL (a SQL NULL, a JSON `null`). CSV and XML have none,
    /// so there nothing is NULL unless listed here. Honoured under
    /// [`Semantics::R2rml`]; a legacy version treats every empty value as
    /// NULL instead.
    pub nulls: Vec<String>,
    /// How a file source's bytes are read: encoding, compression, CSV
    /// dialect, XML namespaces (RML-IO `rml:Source`, CSVW).
    pub access: Access,
}

/// How the bytes of a file source become records (RML-IO §Source and the
/// RML-IO registry's CSVW, JSONPath and XPath sections).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Access {
    /// `rml:encoding` (`rml:UTF-8`, `rml:UTF-16`) or `csvw:encoding`, as an
    /// encoding label. `None` is UTF-8, with a byte-order mark honoured.
    pub encoding: Option<String>,
    /// `rml:compression`.
    pub compression: Compression,
    /// `csvw:dialect` on a `csvw:Table` source.
    pub dialect: CsvDialect,
    /// XPath namespace prefixes (`rml:namespace` on an XPath reference
    /// formulation): prefix → namespace IRI.
    pub namespaces: Vec<(String, String)>,
    /// One JSON value per line (JSON Lines): every line is a document the
    /// iterator runs over.
    pub json_lines: bool,
}

/// `rml:compression` (RML-IO): how a file source is compressed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Compression {
    #[default]
    None,
    Gzip,
    /// A zip archive; the source is its one file (or the one the path names).
    Zip,
    /// A tar archive compressed with xz.
    TarXz,
    /// A tar archive compressed with gzip.
    TarGz,
}

/// The CSVW dialect description a `csvw:Table` source may carry
/// (<https://www.w3.org/TR/tabular-metadata/#dialect-descriptions>). Every
/// field is the CSVW default when the mapping does not say otherwise.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CsvDialect {
    /// `csvw:delimiter`, default `,`.
    pub delimiter: u8,
    /// `csvw:quoteChar`, default `"`.
    pub quote: u8,
    /// `csvw:doubleQuote`: whether a quote inside a quoted field is written
    /// twice (default) rather than escaped with a backslash.
    pub double_quote: bool,
    /// `csvw:header`: whether the first row names the columns (default).
    pub header: bool,
    /// `csvw:headerRowCount`: header rows; the last one names the columns.
    pub header_rows: usize,
    /// `csvw:skipRows`: rows before the header that are not data.
    pub skip_rows: usize,
    /// `csvw:commentPrefix`: a row starting with it is a comment.
    pub comment: Option<u8>,
    /// `csvw:trim`: strip whitespace around each value (default off for
    /// RML, which reads cells as they are).
    pub trim: bool,
    /// `csvw:null`: values that count as NULL, as `rml:null` does.
    pub nulls: Vec<String>,
}

impl Default for CsvDialect {
    fn default() -> Self {
        Self {
            delimiter: b',',
            quote: b'"',
            double_quote: true,
            header: true,
            header_rows: 1,
            skip_rows: 0,
            comment: None,
            trim: false,
            nulls: Vec::new(),
        }
    }
}

impl LogicalSource {
    /// The SQL this logical source selects, with the table name quoted by the
    /// dialect's own rules — part by part when it is schema-qualified, and
    /// without the delimiters the mapping wrote it with. `None` when the
    /// source is not relational.
    pub fn sql(&self, quote: &dyn Fn(&str) -> String) -> Option<String> {
        if let Some(q) = &self.query {
            // A view written as a statement may end in `;`, which a query
            // wrapped as a subquery (a join, a cursor, a column check) cannot.
            return Some(q.trim().trim_end_matches(';').trim_end().to_string());
        }
        self.table_name
            .as_ref()
            .map(|t| format!("SELECT * FROM {}", super::sqlident::table_sql(t, quote)))
    }

    /// The table a catalogue lookup (unique keys) can answer for: a named
    /// table, read whole, and not qualified by a schema.
    pub fn catalogue_table(&self) -> Option<String> {
        if self.query.is_some() {
            return None;
        }
        super::sqlident::catalogue_table(self.table_name.as_deref()?)
    }
}

/// The actual data source reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceRef {
    /// File path or URL string
    File(String),
    /// A registered datasource, by its `urn:source:<id>` IRI.
    Datasource(String),
}

/// rml:referenceFormulation — how to interpret references in the source.
#[derive(Debug, Clone, PartialEq)]
pub enum ReferenceFormulation {
    Csv,
    JsonPath,
    XPath,
    /// A relational source: references are column names.
    Sql,
    /// Unknown / custom formulation IRI
    Other(String),
}

impl ReferenceFormulation {
    pub fn from_iri(iri: &str) -> Self {
        match iri {
            "http://semweb.mmlab.be/ns/ql#CSV"
            | "http://w3id.org/rml/CSV"
            | "http://w3id.org/rml/CSVW"
            | "http://w3id.org/rml/CSVWReferenceFormulation" => Self::Csv,
            "http://semweb.mmlab.be/ns/ql#JSONPath" | "http://w3id.org/rml/JSONPath" => {
                Self::JsonPath
            }
            "http://semweb.mmlab.be/ns/ql#XPath"
            | "http://w3id.org/rml/XPath"
            | "http://w3id.org/rml/XPathReferenceFormulation" => Self::XPath,
            // R2RML has no reference formulation; RML implementations spell a
            // relational source `ql:SQL2008`, and the modern RML-IO vocabulary
            // `rml:SQL2008`. Accept either, plus a bare local name.
            other
                if other.ends_with("#SQL2008")
                    || other.ends_with("/SQL2008")
                    || other.ends_with("/SQL2008Table")
                    || other.ends_with("/SQL2008Query")
                    || other.ends_with("#SQL")
                    || other == "SQL2008" =>
            {
                Self::Sql
            }
            other => Self::Other(other.to_string()),
        }
    }
}

/// rr:SubjectMap — maps source rows to RDF subjects.
#[derive(Debug, Clone)]
pub struct SubjectMap {
    pub term_map: TermMap,
    /// rr:class — rdf:type assertions added to every generated subject
    pub classes: Vec<String>,
    /// `fnml:functionValue` on the subject map: the subject is computed by a
    /// declared function rather than by `term_map`, which then holds an
    /// empty placeholder. A function-valued subject is never pushed down as a
    /// join parent — the planner cannot project its columns — and resolves
    /// through the index instead.
    pub function: Option<FunctionMap>,
    /// `rr:graphMap` / `rr:graph`: the graphs every triple of this subject
    /// goes to, `rr:class` triples included (R2RML §9, §11.1).
    pub graph_maps: Vec<TermMap>,
}

/// rr:PredicateObjectMap — maps source rows to predicate-object pairs.
///
/// A map generates one triple for every predicate map × every object map
/// (R2RML §6.3, §11.1); `rr:predicate` and `rr:object` shortcuts count as
/// maps of their own.
#[derive(Debug, Clone)]
pub struct PredicateObjectMap {
    /// Never empty.
    pub predicate_maps: Vec<TermMap>,
    /// Never empty.
    pub object_maps: Vec<ObjectMap>,
    /// Graphs this map's triples go to *as well as* the subject's: R2RML
    /// takes the union, it does not override.
    pub graph_maps: Vec<TermMap>,
}

impl PredicateObjectMap {
    /// The referencing object maps among this map's objects.
    pub fn refs(&self) -> impl Iterator<Item = &RefObjectMap> {
        self.object_maps.iter().filter_map(|o| match o {
            ObjectMap::Ref(r) => Some(r),
            _ => None,
        })
    }
}

impl TriplesMap {
    /// Every referencing object map of every predicate-object map.
    pub fn refs(&self) -> impl Iterator<Item = &RefObjectMap> {
        self.predicate_object_maps.iter().flat_map(|p| p.refs())
    }
}

/// How a predicate-object map produces its object.
#[derive(Debug, Clone)]
pub enum ObjectMap {
    /// A term built from this row alone.
    Term(TermMap),
    /// `rr:parentTriplesMap` with join conditions: the object is the subject
    /// another triples map generates for the joined row.
    Ref(RefObjectMap),
    /// `fnml:functionValue`: the object is computed by a declared function.
    Function(FunctionMap),
}

/// A referencing object map.
#[derive(Debug, Clone)]
pub struct RefObjectMap {
    /// IRI of the parent `rr:TriplesMap`.
    pub parent_triples_map: String,
    /// `rr:joinCondition` pairs. With none, R2RML permits the reference only
    /// when both triples maps read the same logical source, and each row
    /// joins to itself: the object is the parent's subject for the child's
    /// own row (R2RML §8).
    pub joins: Vec<JoinCondition>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct JoinCondition {
    /// Evaluated on the child (this triples map's) logical iteration.
    pub child: JoinSide,
    /// Evaluated on the parent triples map's logical iteration.
    pub parent: JoinSide,
}

impl JoinCondition {
    /// The common case: `rr:child "c" ; rr:parent "p"` — two column or
    /// reference names.
    #[cfg(test)]
    pub fn columns(child: impl Into<String>, parent: impl Into<String>) -> Self {
        Self {
            child: JoinSide::Reference(child.into()),
            parent: JoinSide::Reference(parent.into()),
        }
    }
}

/// One side of a join condition: `rr:child` / `rr:parent` name a reference,
/// and RML-Core's `rml:childMap` / `rml:parentMap` may also be a template or
/// a constant.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum JoinSide {
    Reference(String),
    Template(String),
    Constant(String),
}

impl JoinSide {
    /// The column this side reads when it is a plain reference — the only
    /// form a relational join can push down or fetch by key.
    pub fn column(&self) -> Option<&str> {
        match self {
            JoinSide::Reference(c) => Some(c),
            _ => None,
        }
    }

    /// Every reference this side reads.
    pub fn referenced_columns(&self) -> Vec<String> {
        match self {
            JoinSide::Reference(c) => vec![c.clone()],
            JoinSide::Template(t) => template_columns(t),
            JoinSide::Constant(_) => Vec::new(),
        }
    }

    /// A stable text form, for keys and messages.
    pub fn key(&self) -> String {
        match self {
            JoinSide::Reference(c) => c.clone(),
            JoinSide::Template(t) => format!("template:{t}"),
            JoinSide::Constant(c) => format!("constant:{c}"),
        }
    }
}

/// An FNML function call producing a term.
#[derive(Debug, Clone)]
pub struct FunctionMap {
    /// `fno:executes` — the function IRI.
    pub function: String,
    /// Parameters, keyed by their predicate IRI. A parameter is either a
    /// constant or a reference to a column in the current row.
    pub params: BTreeMap<String, Vec<FunctionArg>>,
    /// `rr:datatype` declared on the object map, applied when the function
    /// falls back to emitting a literal.
    pub datatype: Option<String>,
}

impl FunctionMap {
    pub fn first(&self, predicate: &str) -> Option<&FunctionArg> {
        self.params.get(predicate).and_then(|v| v.first())
    }
    pub fn all(&self, predicate: &str) -> &[FunctionArg] {
        self.params.get(predicate).map(Vec::as_slice).unwrap_or(&[])
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FunctionArg {
    Constant(String),
    Reference(String),
}

/// Check a string template's braces (R2RML §7.3, RML-Core §template): a
/// `{` opens a reference that a `}` closes, and a brace that is not one of
/// those — inside a reference, or outside one — is escaped with a backslash.
/// A template that breaks this names something other than what it seems to,
/// so it is a mapping error rather than a template read as best it can be.
pub fn validate_template(template: &str) -> Result<(), String> {
    let mut chars = template.chars();
    let mut open = false;
    let mut empty = true;
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                chars.next();
                empty = false;
            }
            '{' if open => {
                return Err(format!(
                    "the template \"{template}\" opens a reference inside a reference; escape \
                     a literal brace as \\{{"
                ))
            }
            '{' => {
                open = true;
                empty = true;
            }
            '}' if open => {
                if empty {
                    return Err(format!(
                        "the template \"{template}\" has an empty reference {{}}"
                    ));
                }
                open = false;
            }
            '}' => {
                return Err(format!(
                    "the template \"{template}\" closes a reference it never opened; escape a \
                     literal brace as \\}}"
                ))
            }
            _ => empty = false,
        }
    }
    if open {
        return Err(format!(
            "the template \"{template}\" opens a reference it never closes"
        ));
    }
    Ok(())
}

/// The `{column}` placeholders in a template, in order, de-duplicated.
pub fn template_columns(template: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            // An escaped brace is literal text, not a placeholder.
            chars.next();
            continue;
        }
        if c == '{' {
            let mut col = String::new();
            while let Some(inner) = chars.next() {
                match inner {
                    // RML-Core: a brace or backslash inside a reference is
                    // escaped with a backslash too.
                    '\\' => {
                        if let Some(next) = chars.next() {
                            col.push(next);
                        }
                    }
                    '}' => break,
                    other => col.push(other),
                }
            }
            if !col.is_empty() && !out.contains(&col) {
                out.push(col);
            }
        }
    }
    out
}

/// A term map: constant, template, or column/reference.
#[derive(Debug, Clone)]
pub struct TermMap {
    pub kind: TermMapKind,
    pub term_type: TermType,
    /// Optional datatype IRI for literals
    pub datatype: Option<String>,
    /// Optional language tag for literals
    pub language: Option<String>,
    /// `rml:languageMap` that is not a constant (RML-Core): the language
    /// tags come from the logical iteration. A constant one is `language`.
    pub language_map: Option<Box<TermMap>>,
    /// `rml:datatypeMap` that is not a constant (RML-Core): the datatype
    /// IRIs come from the logical iteration. A constant one is `datatype`.
    pub datatype_map: Option<Box<TermMap>>,
}

impl TermMap {
    /// A term map with no language or datatype.
    pub fn new(kind: TermMapKind, term_type: TermType) -> Self {
        Self {
            kind,
            term_type,
            datatype: None,
            language: None,
            language_map: None,
            datatype_map: None,
        }
    }

    /// The columns this term map reads. A join planner projects exactly these
    /// from the parent side, rather than dragging the whole parent row along.
    pub fn referenced_columns(&self) -> Vec<String> {
        let mut out = match &self.kind {
            TermMapKind::Reference(c) => vec![c.clone()],
            TermMapKind::Template(t) => template_columns(t),
            TermMapKind::Constant(_) | TermMapKind::Fresh => Vec::new(),
        };
        for m in self.language_map.iter().chain(self.datatype_map.iter()) {
            for c in m.referenced_columns() {
                if !out.contains(&c) {
                    out.push(c);
                }
            }
        }
        out
    }
}

/// How the term value is produced.
#[derive(Debug, Clone)]
pub enum TermMapKind {
    /// `rr:constant` — a fixed IRI or literal, exactly as the mapping wrote
    /// it: its datatype and language tag are part of it, and its kind is the
    /// term type (R2RML §7.4: `rr:termType` has no effect on a constant).
    Constant(Term),
    /// `rr:template` — e.g. `"http://example.org/{column}"`
    Template(String),
    /// `rml:reference` or `rr:column` — a direct column/JSONPath/XPath reference
    Reference(String),
    /// A blank-node term map with no expression (RML-Core): a fresh blank
    /// node for every logical iteration.
    Fresh,
}

/// `rr:termType` — the RDF term type to produce.
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::upper_case_acronyms)]
pub enum TermType {
    IRI,
    BlankNode,
    Literal,
    /// RML-Core `rml:URI`: an RFC 3986 URI; template values are URI-safe
    /// (outside `unreserved`, percent-encoded).
    URI,
    /// RML-Core `rml:UnsafeIRI`: an IRI built from template values as they
    /// are, without percent-encoding.
    UnsafeIRI,
    /// RML-Core `rml:UnsafeURI`: a URI built from template values as they are.
    UnsafeURI,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sql_reference_formulations_are_recognised_in_every_spelling() {
        for iri in [
            "http://semweb.mmlab.be/ns/ql#SQL2008",
            "http://w3id.org/rml/SQL2008",
            "http://semweb.mmlab.be/ns/ql#SQL",
        ] {
            assert_eq!(
                ReferenceFormulation::from_iri(iri),
                ReferenceFormulation::Sql,
                "{iri}"
            );
        }
        assert_eq!(
            ReferenceFormulation::from_iri("http://semweb.mmlab.be/ns/ql#CSV"),
            ReferenceFormulation::Csv
        );
        assert!(matches!(
            ReferenceFormulation::from_iri("http://example.org/Custom"),
            ReferenceFormulation::Other(_)
        ));
    }

    #[test]
    fn a_template_with_stray_braces_is_refused() {
        for ok in [
            "http://x/{a}/{b}",
            r"literal \{brace\} {c}",
            r"http://x/{$['\{Name\}']}",
            "no references",
        ] {
            validate_template(ok).unwrap_or_else(|e| panic!("{ok}: {e}"));
        }
        for bad in [
            "http://x/{{Name}}",
            r"http://x/{\\{Name\\}}",
            "http://x/{a",
            "http://x/a}",
            "http://x/{}",
        ] {
            assert!(validate_template(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn template_columns_skips_escapes_and_dedupes() {
        assert_eq!(template_columns("http://x/{a}/{b}/{a}"), vec!["a", "b"]);
        assert_eq!(template_columns("no placeholders"), Vec::<String>::new());
        assert_eq!(template_columns(r"literal \{brace\} {c}"), vec!["c"]);
    }

    #[test]
    fn referenced_columns_covers_every_term_map_kind() {
        let tm = |kind| TermMap::new(kind, TermType::IRI);
        assert_eq!(
            tm(TermMapKind::Template("http://x/{a}/{b}".into())).referenced_columns(),
            vec!["a", "b"]
        );
        assert_eq!(
            tm(TermMapKind::Reference("c".into())).referenced_columns(),
            vec!["c"]
        );
        assert!(tm(TermMapKind::Constant(
            oxigraph::model::NamedNode::new_unchecked("http://x/c").into()
        ))
        .referenced_columns()
        .is_empty());
    }

    #[test]
    fn a_logical_source_renders_table_or_query_sql() {
        let quote = |t: &str| format!("\"{t}\"");
        let table = LogicalSource {
            source: SourceRef::Datasource("urn:source:s".into()),
            reference_formulation: ReferenceFormulation::Sql,
            iterator: None,
            query: None,
            table_name: Some("products".into()),
            nulls: Vec::new(),
            access: Access::default(),
        };
        assert_eq!(table.sql(&quote).unwrap(), "SELECT * FROM \"products\"");
        let query = LogicalSource {
            query: Some("SELECT 1".into()),
            table_name: Some("ignored".into()),
            ..table.clone()
        };
        assert_eq!(
            query.sql(&quote).unwrap(),
            "SELECT 1",
            "an explicit query wins"
        );
        let terminated = LogicalSource {
            query: Some("\n  SELECT 1 ;\n  ".into()),
            ..table.clone()
        };
        assert_eq!(
            terminated.sql(&quote).unwrap(),
            "SELECT 1",
            "a statement terminator is not part of the view"
        );
        let file = LogicalSource {
            source: SourceRef::File("x.csv".into()),
            reference_formulation: ReferenceFormulation::Csv,
            iterator: None,
            query: None,
            table_name: None,
            nulls: Vec::new(),
            access: Access::default(),
        };
        assert_eq!(file.sql(&quote), None);
    }
}
