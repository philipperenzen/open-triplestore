//! Virtual sources: a SPARQL endpoint — an Ontop virtual knowledge graph
//! over a database, or any other endpoint — registered as a datasource
//! behind the same connector trait as a SQL database.
//!
//! The dialect is `sparql`. The endpoint is `host` + `port` + `database` (the
//! path, `/sparql` by default) with `tls` choosing the scheme; `username`
//! and the credential reference become HTTP Basic, resolved at the moment
//! of use like every other credential. The endpoint must be covered by
//! `OTS_REMOTE_ALLOWLIST`: every request goes through [`crate::remote`],
//! the same door SPARQL federation and LDES sync use, so a virtual source is
//! never a way around the allowlist.
//!
//! **The catalogue is the classes.** A virtual source has no tables, so the
//! connector presents each class as one — its instances the rows, `subject`
//! the primary key, the predicates its instances carry the columns (named by
//! predicate IRI). A mapping reads a class with `rr:tableName "<class IRI>"`
//! or runs its own `rml:query`, which for this dialect is a SPARQL SELECT
//! whose variables are the columns. Rows are typed from the bindings'
//! datatypes and carried as lexical forms.
//!
//! **A snapshot run** (`mode: snapshot`) materialises the endpoint's whole
//! graph as one `CONSTRUCT`, into a run graph like any other run — gated,
//! swapped in atomically, reviewable, promotable — so a virtual graph can be
//! served from the store with the same guarantees as a mapped one.
//!
//! **Federation.** `SERVICE <urn:source:id>` in a local query resolves to
//! the source's endpoint with its credential, so a virtual source is also
//! queryable live, without materialising anything.

use std::collections::{BTreeMap, HashMap};
use std::sync::{OnceLock, RwLock};

use ots_plugin_api::sources::{
    BatchSink, ColumnInfo, ConnectParams, Row, SecretString, SourceConnection, SourceConnector,
    SourceError, TableInfo, TableKind, Value, ValueKind,
};
use oxigraph::io::RdfFormat;
use serde_json::Value as Json;

use crate::remote::{self, Auth, RemoteError};
use crate::secrets::{self, Secret, SecretRef};
use crate::store::TripleStore;

use super::model::SqlSource;

/// The dialect token a datasource names.
pub const DIALECT: &str = "sparql";

/// The column that carries the instance IRI in a class-table.
pub const SUBJECT_COLUMN: &str = "subject";

/// Subjects the catalogue samples per class to learn its predicates.
const CATALOGUE_SAMPLE: usize = 100;
/// Classes listed at most.
const MAX_CLASSES: usize = 1_000;

const XSD: &str = "http://www.w3.org/2001/XMLSchema#";

// ───────────────────────────── The endpoint ─────────────────────────────

/// The endpoint URL a datasource describes.
pub fn endpoint_url(
    host: &str,
    port: Option<u16>,
    path: &str,
    tls: bool,
) -> Result<String, String> {
    let host = host.trim();
    if host.is_empty() {
        return Err("a sparql datasource needs a host".to_string());
    }
    if host.contains(['/', '@', '?', '#', ' ']) {
        return Err(format!("'{host}' is not a host name"));
    }
    let scheme = if tls { "https" } else { "http" };
    let mut path = path.trim().to_string();
    if path.is_empty() {
        path = "/sparql".to_string();
    }
    if !path.starts_with('/') {
        path.insert(0, '/');
    }
    Ok(match port {
        Some(p) => format!("{scheme}://{host}:{p}{path}"),
        None => format!("{scheme}://{host}{path}"),
    })
}

/// The endpoint a registered source points at.
pub fn endpoint_of(source: &SqlSource) -> Result<String, String> {
    endpoint_url(
        source.host.as_deref().unwrap_or(""),
        source.port,
        &source.database,
        source.tls,
    )
}

/// Whether a source is a SPARQL endpoint rather than a database.
pub fn is_virtual(source: &SqlSource) -> bool {
    source.dialect.eq_ignore_ascii_case(DIALECT)
}

// ───────────────────────── Federation resolution ─────────────────────────

/// What `SERVICE <urn:source:id>` needs: the endpoint, and the account.
#[derive(Debug, Clone)]
struct Target {
    endpoint: String,
    username: Option<String>,
    credential: Option<SecretRef>,
}

fn targets() -> &'static RwLock<HashMap<String, Target>> {
    static T: OnceLock<RwLock<HashMap<String, Target>>> = OnceLock::new();
    T.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Keep (or drop) a source's endpoint for `SERVICE` resolution. Called by
/// the registry on every write, and at boot for every source.
pub fn remember(source: &SqlSource) {
    let mut map = targets().write().unwrap_or_else(|e| e.into_inner());
    match (is_virtual(source), endpoint_of(source)) {
        (true, Ok(endpoint)) => {
            map.insert(
                source.id.clone(),
                Target {
                    endpoint,
                    username: source.username.clone(),
                    credential: source.credential.clone(),
                },
            );
        }
        _ => {
            map.remove(&source.id);
        }
    }
}

pub fn forget(source_id: &str) {
    targets()
        .write()
        .unwrap_or_else(|e| e.into_inner())
        .remove(source_id);
}

/// Every registered virtual source, at boot.
pub fn load_all(store: &TripleStore) {
    for source in super::registry::list_sources(store) {
        remember(&source);
    }
}

/// A resolved `SERVICE` target: the endpoint, and Basic credentials when the
/// source has an account — resolved now, dropped with the request.
pub struct Resolved {
    pub endpoint: String,
    pub username: Option<String>,
    pub secret: Option<Secret>,
}

/// `urn:source:<id>` → its endpoint, for a registered virtual source.
/// Anything else is not a source IRI and resolves to nothing.
pub fn resolve(service_iri: &str) -> Option<Result<Resolved, String>> {
    let id = service_iri.strip_prefix("urn:source:")?;
    let target = targets()
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .get(id)
        .cloned()?;
    let secret = match &target.credential {
        Some(r) => match secrets::resolve(r) {
            Ok(s) => Some(s),
            Err(e) => {
                return Some(Err(format!(
                    "the credential of datasource '{id}' could not be resolved: {e}"
                )))
            }
        },
        None => None,
    };
    Some(Ok(Resolved {
        endpoint: target.endpoint,
        username: target.username,
        secret,
    }))
}

// ───────────────────────────── The connector ─────────────────────────────

pub struct SparqlConnector;

impl SourceConnector for SparqlConnector {
    fn dialect(&self) -> &'static str {
        DIALECT
    }

    fn connect(&self, params: &ConnectParams) -> Result<Box<dyn SourceConnection>, SourceError> {
        if !params.read_only {
            return Err(SourceError::Config(
                "this store only opens datasources read-only".to_string(),
            ));
        }
        let endpoint = endpoint_url(
            params.host.as_deref().unwrap_or(""),
            params.port,
            &params.database,
            params.tls,
        )
        .map_err(SourceError::Config)?;
        if !remote::is_allowed(&endpoint) {
            return Err(SourceError::Config(format!(
                "the endpoint is not covered by {}; a virtual source is reached through the \
                 same allowlist as SPARQL federation",
                remote::ALLOWLIST_ENV
            )));
        }
        let conn = SparqlConnection {
            endpoint,
            username: params
                .username
                .clone()
                .map(|u| u.trim().to_string())
                .filter(|u| !u.is_empty()),
            password: params.password.clone(),
            classes: None,
        };
        // Reachable, and answering SPARQL: an ASK costs nothing on a virtual
        // knowledge graph and proves both.
        let answer = conn.text("ASK { ?s ?p ?o }", "application/sparql-results+json")?;
        let parsed: Json = serde_json::from_str(&answer).map_err(|e| {
            SourceError::Connect(format!("the endpoint did not answer SPARQL results: {e}"))
        })?;
        if parsed.get("boolean").is_none() {
            return Err(SourceError::Connect(
                "the endpoint did not answer the ASK with a boolean".to_string(),
            ));
        }
        Ok(Box::new(conn))
    }

    /// A "table" is a class IRI; quoted so the engine's `SELECT * FROM`
    /// carries it intact and [`class_of_sql`] finds it again.
    fn quote_identifier(&self, ident: &str) -> String {
        format!("\"{}\"", ident.replace('"', "\"\""))
    }
}

/// The class behind `SELECT * FROM "<class>"` — what the engine generates
/// for an `rr:tableName` logical source. Anything else is SPARQL as written.
pub fn class_of_sql(query: &str) -> Option<String> {
    let rest = query.trim().strip_prefix("SELECT * FROM \"")?;
    let rest = rest.strip_suffix('"')?;
    Some(rest.replace("\"\"", "\""))
}

struct SparqlConnection {
    endpoint: String,
    username: Option<String>,
    password: Option<SecretString>,
    /// The catalogue, once read: the class listing does not change under a
    /// connection.
    classes: Option<Vec<(String, u64)>>,
}

fn iri(value: &str) -> String {
    format!("<{}>", crate::store::escape_sparql_iri(value))
}

/// The generic type of a binding's datatype.
pub fn kind_of_datatype(datatype: &str) -> ValueKind {
    let local = datatype.strip_prefix(XSD).unwrap_or(datatype);
    match local {
        "integer" | "int" | "long" | "short" | "byte" | "nonNegativeInteger"
        | "positiveInteger" | "nonPositiveInteger" | "negativeInteger" | "unsignedLong"
        | "unsignedInt" | "unsignedShort" | "unsignedByte" => ValueKind::Integer,
        "decimal" => ValueKind::Decimal,
        "double" | "float" => ValueKind::Float,
        "boolean" => ValueKind::Boolean,
        "date" => ValueKind::Date,
        "time" => ValueKind::Time,
        "dateTime" | "dateTimeStamp" => ValueKind::DateTime,
        "hexBinary" | "base64Binary" => ValueKind::Binary,
        "string" | "normalizedString" | "token" | "language" | "anyURI" | "Name" | "NCName" => {
            ValueKind::Text
        }
        "http://www.w3.org/1999/02/22-rdf-syntax-ns#JSON" => ValueKind::Json,
        "http://www.w3.org/1999/02/22-rdf-syntax-ns#langString" => ValueKind::Text,
        _ => ValueKind::Other,
    }
}

/// One binding of a SPARQL JSON result as a cell: IRIs and blank nodes as
/// their identifier with kind `Other`, literals typed by their datatype.
fn cell(binding: &Json) -> Option<Value> {
    let value = binding.get("value")?.as_str()?;
    Some(match binding.get("type").and_then(Json::as_str) {
        Some("uri") => Value::new(value, ValueKind::Other),
        Some("bnode") => Value::new(format!("_:{value}"), ValueKind::Other),
        _ => match binding.get("datatype").and_then(Json::as_str) {
            Some(dt) => Value::new(value, kind_of_datatype(dt)),
            None => Value::text(value),
        },
    })
}

/// The short name of a datatype or "iri" / "bnode" / "string", for the
/// catalogue's native type.
fn native_of(binding: &Json) -> String {
    match binding.get("type").and_then(Json::as_str) {
        Some("uri") => "iri".to_string(),
        Some("bnode") => "bnode".to_string(),
        _ => match binding.get("datatype").and_then(Json::as_str) {
            Some(dt) => dt.rsplit(['#', '/']).next().unwrap_or(dt).to_string(),
            None if binding.get("xml:lang").is_some() => "langString".to_string(),
            None => "string".to_string(),
        },
    }
}

impl SparqlConnection {
    fn auth(&self) -> Auth<'_> {
        match (&self.username, &self.password) {
            (Some(u), Some(p)) => Auth::Basic(u, p.expose()),
            _ => Auth::None,
        }
    }

    fn text(&self, query: &str, accept: &str) -> Result<String, SourceError> {
        remote::post_sparql_blocking(&self.endpoint, query, accept, self.auth()).map_err(
            |e| match e {
                RemoteError::NotAllowed(_) => SourceError::Config(e.to_string()),
                RemoteError::Request { reason, .. } if reason.contains("timed out") => {
                    SourceError::Timeout
                }
                other => SourceError::Query(other.to_string()),
            },
        )
    }

    /// A SELECT's solutions, as `variable → cell` rows.
    fn select(&self, query: &str) -> Result<(Vec<String>, Vec<Row>), SourceError> {
        let body = self.text(query, "application/sparql-results+json")?;
        let parsed: Json = serde_json::from_str(&body).map_err(|e| {
            SourceError::Query(format!("the endpoint's results could not be parsed: {e}"))
        })?;
        let variables: Vec<String> = parsed["head"]["vars"]
            .as_array()
            .map(|v| {
                v.iter()
                    .filter_map(|x| x.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        let rows = parsed["results"]["bindings"]
            .as_array()
            .map(|bindings| {
                bindings
                    .iter()
                    .map(|b| {
                        let mut row = Row::new();
                        if let Some(obj) = b.as_object() {
                            for (var, binding) in obj {
                                if let Some(c) = cell(binding) {
                                    row.insert(var.clone(), c);
                                }
                            }
                        }
                        row
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok((variables, rows))
    }

    fn class_listing(&mut self) -> Result<Vec<(String, u64)>, SourceError> {
        if let Some(c) = &self.classes {
            return Ok(c.clone());
        }
        let (_, rows) = self.select(&format!(
            "SELECT ?c (COUNT(?s) AS ?n) WHERE {{ ?s a ?c }} GROUP BY ?c ORDER BY ?c LIMIT {MAX_CLASSES}"
        ))?;
        let classes: Vec<(String, u64)> = rows
            .iter()
            .filter_map(|r| {
                let c = r.get("c")?.lexical.clone();
                let n = r.get("n").and_then(|v| v.lexical.parse().ok()).unwrap_or(0);
                Some((c, n))
            })
            .collect();
        self.classes = Some(classes.clone());
        Ok(classes)
    }

    /// One class as a table: the predicates its sampled instances carry.
    fn class_table(
        &mut self,
        class: &str,
        instances: Option<u64>,
    ) -> Result<TableInfo, SourceError> {
        let (_, rows) = self.select(&format!(
            "SELECT ?p (SAMPLE(?o) AS ?v) (COUNT(DISTINCT ?s) AS ?n) WHERE {{ \
               {{ SELECT ?s WHERE {{ ?s a {c} }} LIMIT {CATALOGUE_SAMPLE} }} ?s ?p ?o }} \
             GROUP BY ?p ORDER BY ?p",
            c = iri(class)
        ))?;
        let sampled = rows
            .iter()
            .filter_map(|r| r.get("n").and_then(|v| v.lexical.parse::<u64>().ok()))
            .max()
            .unwrap_or(0);
        let mut columns = vec![ColumnInfo {
            name: SUBJECT_COLUMN.to_string(),
            native_type: "iri".to_string(),
            generic_type: ValueKind::Other,
            nullable: false,
            default: None,
            comment: Some("the instance".to_string()),
            primary_key: true,
        }];
        // The raw sample binding is what names the native type; the cell
        // typed it already.
        let body = self.text(
            &format!(
                "SELECT ?p (SAMPLE(?o) AS ?v) WHERE {{ \
                   {{ SELECT ?s WHERE {{ ?s a {c} }} LIMIT {CATALOGUE_SAMPLE} }} ?s ?p ?o }} \
                 GROUP BY ?p ORDER BY ?p",
                c = iri(class)
            ),
            "application/sparql-results+json",
        )?;
        let raw: Json = serde_json::from_str(&body).unwrap_or(Json::Null);
        let natives: HashMap<String, String> = raw["results"]["bindings"]
            .as_array()
            .map(|b| {
                b.iter()
                    .filter_map(|x| {
                        Some((x["p"]["value"].as_str()?.to_string(), native_of(&x["v"])))
                    })
                    .collect()
            })
            .unwrap_or_default();
        for r in &rows {
            let Some(p) = r.get("p").map(|v| v.lexical.clone()) else {
                continue;
            };
            let carriers: u64 = r.get("n").and_then(|v| v.lexical.parse().ok()).unwrap_or(0);
            let native = natives
                .get(&p)
                .cloned()
                .unwrap_or_else(|| "string".to_string());
            let generic = match native.as_str() {
                "iri" | "bnode" => ValueKind::Other,
                "string" | "langString" => ValueKind::Text,
                _ => r.get("v").map(|v| v.kind).unwrap_or(ValueKind::Other),
            };
            columns.push(ColumnInfo {
                name: p,
                native_type: native,
                generic_type: generic,
                nullable: carriers < sampled,
                default: None,
                comment: None,
                primary_key: false,
            });
        }
        Ok(TableInfo {
            schema: None,
            name: class.to_string(),
            kind: TableKind::View,
            columns,
            primary_key: vec![SUBJECT_COLUMN.to_string()],
            foreign_keys: Vec::new(),
            indexes: Vec::new(),
            row_estimate: instances,
            comment: None,
        })
    }

    /// The instances of a class as rows: `subject`, then one column per
    /// predicate (the first value when there are several).
    fn class_rows(&self, class: &str, limit: Option<usize>) -> Result<Vec<Row>, SourceError> {
        let query = match limit {
            Some(n) => format!(
                "SELECT ?subject ?p ?o WHERE {{ {{ SELECT ?subject WHERE {{ ?subject a {c} }} LIMIT {n} }} \
                 ?subject ?p ?o }} ORDER BY ?subject",
                c = iri(class)
            ),
            None => format!(
                "SELECT ?subject ?p ?o WHERE {{ ?subject a {c} ; ?p ?o }} ORDER BY ?subject",
                c = iri(class)
            ),
        };
        let (_, bindings) = self.select(&query)?;
        let mut rows: Vec<Row> = Vec::new();
        let mut order: Vec<String> = Vec::new();
        let mut by_subject: BTreeMap<String, Row> = BTreeMap::new();
        for b in bindings {
            let (Some(s), Some(p), Some(o)) = (b.get("subject"), b.get("p"), b.get("o")) else {
                continue;
            };
            let row = by_subject.entry(s.lexical.clone()).or_insert_with(|| {
                order.push(s.lexical.clone());
                Row::from([(SUBJECT_COLUMN.to_string(), s.clone())])
            });
            row.entry(p.lexical.clone()).or_insert_with(|| o.clone());
        }
        for s in order {
            if let Some(r) = by_subject.remove(&s) {
                rows.push(r);
            }
        }
        Ok(rows)
    }
}

impl SourceConnection for SparqlConnection {
    fn introspect(&mut self) -> Result<Vec<TableInfo>, SourceError> {
        let classes = self.class_listing()?;
        let mut tables = Vec::with_capacity(classes.len());
        for (class, n) in classes {
            tables.push(self.class_table(&class, Some(n))?);
        }
        Ok(tables)
    }

    fn table_names(&mut self) -> Result<Vec<String>, SourceError> {
        Ok(self.class_listing()?.into_iter().map(|(c, _)| c).collect())
    }

    fn sample(&mut self, table: &str, limit: usize) -> Result<Vec<Row>, SourceError> {
        self.class_rows(table, Some(limit.clamp(1, 10_000)))
    }

    fn stream(
        &mut self,
        query: &str,
        batch_size: usize,
        sink: BatchSink<'_>,
    ) -> Result<u64, SourceError> {
        let batch_size = batch_size.max(1);
        let rows = match class_of_sql(query) {
            Some(class) => self.class_rows(&class, None)?,
            None => self.select(query)?.1,
        };
        let mut delivered = 0u64;
        let mut batch: Vec<Row> = Vec::with_capacity(batch_size);
        for row in rows {
            batch.push(row);
            delivered += 1;
            if batch.len() >= batch_size {
                sink(std::mem::replace(
                    &mut batch,
                    Vec::with_capacity(batch_size),
                ))?;
            }
        }
        if !batch.is_empty() {
            sink(batch)?;
        }
        Ok(delivered)
    }

    fn max_watermark(&mut self, table: &str, column: &str) -> Result<Option<String>, SourceError> {
        let (_, rows) = self.select(&format!(
            "SELECT (MAX(?v) AS ?m) WHERE {{ ?s a {t} ; {p} ?v }}",
            t = iri(table),
            p = iri(column)
        ))?;
        Ok(rows
            .first()
            .and_then(|r| r.get("m"))
            .map(|v| v.lexical.clone()))
    }

    fn unique_keys(&mut self, _table: &str) -> Result<Vec<Vec<String>>, SourceError> {
        Ok(vec![vec![SUBJECT_COLUMN.to_string()]])
    }

    fn server_version(&mut self) -> Result<Option<String>, SourceError> {
        Ok(Some("SPARQL endpoint".to_string()))
    }
}

// ───────────────────────────── Snapshots ─────────────────────────────

/// Materialise the endpoint's whole graph into `graph`: one `CONSTRUCT`,
/// loaded in one pass so blank nodes keep their identity. Returns the
/// triple count.
pub fn snapshot(source: &SqlSource, store: &TripleStore, graph: &str) -> Result<u64, String> {
    let endpoint = endpoint_of(source)?;
    let secret = match &source.credential {
        Some(r) => Some(secrets::resolve(r).map_err(|e| e.to_string())?),
        None => None,
    };
    let auth = match (&source.username, &secret) {
        (Some(u), Some(s)) => Auth::Basic(u, s.expose()),
        _ => Auth::None,
    };
    let body = remote::post_sparql_blocking(
        &endpoint,
        "CONSTRUCT { ?s ?p ?o } WHERE { ?s ?p ?o }",
        "application/n-triples",
        auth,
    )
    .map_err(|e| e.to_string())?;
    store
        .load_str(&body, RdfFormat::NTriples, Some(graph))
        .map_err(|e| format!("loading the snapshot into <{graph}>: {e}"))?;
    Ok(store.count_graph(Some(graph)).unwrap_or(0) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_endpoint_is_host_port_path_and_scheme() {
        assert_eq!(
            endpoint_url("kg.example", None, "", false).unwrap(),
            "http://kg.example/sparql"
        );
        assert_eq!(
            endpoint_url("kg.example", Some(8080), "ontop/sparql", true).unwrap(),
            "https://kg.example:8080/ontop/sparql"
        );
        assert!(endpoint_url("", None, "", false).is_err());
        assert!(endpoint_url("kg.example/evil", None, "", false).is_err());
        assert!(endpoint_url("user@kg.example", None, "", false).is_err());
    }

    #[test]
    fn a_class_table_query_is_recognised_and_sparql_is_not() {
        assert_eq!(
            class_of_sql("SELECT * FROM \"http://example.org/Product\"").as_deref(),
            Some("http://example.org/Product")
        );
        assert_eq!(class_of_sql("SELECT ?s WHERE { ?s a ?c }"), None);
        assert_eq!(class_of_sql("SELECT * FROM (SELECT 1) x"), None);
    }

    #[test]
    fn bindings_become_typed_cells() {
        let lit = serde_json::json!({"type": "literal", "value": "2.5", "datatype": "http://www.w3.org/2001/XMLSchema#decimal"});
        let c = cell(&lit).unwrap();
        assert_eq!((c.lexical.as_str(), c.kind), ("2.5", ValueKind::Decimal));
        let uri = serde_json::json!({"type": "uri", "value": "http://x/a"});
        assert_eq!(cell(&uri).unwrap().kind, ValueKind::Other);
        let bnode = serde_json::json!({"type": "bnode", "value": "b0"});
        assert_eq!(cell(&bnode).unwrap().lexical, "_:b0");
        let lang = serde_json::json!({"type": "literal", "value": "hi", "xml:lang": "en"});
        assert_eq!(cell(&lang).unwrap().kind, ValueKind::Text);
        assert_eq!(native_of(&lang), "langString");
        assert_eq!(native_of(&lit), "decimal");
        assert_eq!(native_of(&uri), "iri");
        assert_eq!(
            kind_of_datatype("http://www.w3.org/2001/XMLSchema#dateTimeStamp"),
            ValueKind::DateTime
        );
        assert_eq!(
            kind_of_datatype("http://example.org/custom"),
            ValueKind::Other
        );
    }

    #[test]
    fn a_virtual_source_is_remembered_for_federation_and_forgotten_on_delete() {
        let source = SqlSource {
            id: "vkg".into(),
            dialect: "sparql".into(),
            host: Some("kg.example".into()),
            database: "/sparql".into(),
            username: Some("reader".into()),
            ..Default::default()
        };
        remember(&source);
        let r = resolve("urn:source:vkg").unwrap().unwrap();
        assert_eq!(r.endpoint, "http://kg.example/sparql");
        assert_eq!(r.username.as_deref(), Some("reader"));
        assert!(r.secret.is_none());
        assert!(resolve("urn:source:nope").is_none());
        assert!(resolve("http://kg.example/sparql").is_none());
        // A SQL source under the same id is not a federation target.
        remember(&SqlSource {
            dialect: "sqlite".into(),
            ..source.clone()
        });
        assert!(resolve("urn:source:vkg").is_none());
        remember(&source);
        forget("vkg");
        assert!(resolve("urn:source:vkg").is_none());
    }
}
