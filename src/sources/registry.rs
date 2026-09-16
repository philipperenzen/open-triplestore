//! Storage for datasources, mappings and runs — RDF in `urn:system:sources`.
//!
//! The store *is* the registry. There is no sidecar database and no on-disk
//! mapping folder, so "the runtime does not read the registry" cannot happen:
//! a run resolves its source and its mapping version out of this graph, and
//! every write is recorded in the commit log like any other.
//!
//! A credential is stored as a `ds:SecretRef` node carrying the reference
//! string. Nothing here ever holds a resolved secret.

use std::collections::{BTreeMap, HashMap};

use oxigraph::model::Term;
use oxigraph::sparql::QueryResults;

use crate::secrets::SecretRef;
use crate::store::{escape_sparql_iri, escape_sparql_literal, TripleStore};

use super::model::*;

const PROV: &str = "http://www.w3.org/ns/prov#";
const DCT: &str = "http://purl.org/dc/terms/";
const DCAT: &str = "http://www.w3.org/ns/dcat#";
const XSD: &str = "http://www.w3.org/2001/XMLSchema#";

fn prefixes() -> String {
    format!(
        "PREFIX ds: <{DS}>\nPREFIX prov: <{PROV}>\nPREFIX dct: <{DCT}>\n\
         PREFIX dcat: <{DCAT}>\nPREFIX xsd: <{XSD}>\n"
    )
}

pub fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// A SELECT, as rows of `variable → lexical value` (IRIs unwrapped).
fn select(store: &TripleStore, sparql: &str) -> Vec<HashMap<String, String>> {
    let Ok(QueryResults::Solutions(solutions)) = store.query(sparql) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for solution in solutions {
        let Ok(solution) = solution else { continue };
        let mut row = HashMap::new();
        for (var, term) in solution.iter() {
            let value = match term {
                Term::NamedNode(n) => n.as_str().to_string(),
                Term::BlankNode(b) => format!("_:{}", b.as_str()),
                Term::Literal(l) => l.value().to_string(),
                #[cfg(feature = "rdf-12")]
                Term::Triple(_) => continue,
            };
            row.insert(var.as_str().to_string(), value);
        }
        out.push(row);
    }
    out
}

fn lit(value: &str) -> String {
    format!("\"{}\"", escape_sparql_literal(value))
}
fn iri(value: &str) -> String {
    format!("<{}>", escape_sparql_iri(value))
}
fn opt_lit(pred: &str, value: &Option<String>) -> String {
    match value {
        Some(v) if !v.is_empty() => format!("    {pred} {} ;\n", lit(v)),
        _ => String::new(),
    }
}

/// Remove a subject and any blank nodes hanging directly off it. One
/// statement, so a rewrite never leaves a half-deleted record behind.
///
/// Emits no prologue: SPARQL Update allows only one at the head of a request,
/// so a caller that chains this with an INSERT declares the prefixes once.
/// This statement needs none — it names no prefixed term.
fn delete_subject_sparql(subject: &str) -> String {
    let s = iri(subject);
    format!(
        "DELETE {{ GRAPH <{SOURCES_GRAPH}> {{ {s} ?p ?o . ?o ?bp ?bo }} }}\n\
         WHERE {{ GRAPH <{SOURCES_GRAPH}> {{ {s} ?p ?o . OPTIONAL {{ FILTER(isBlank(?o)) ?o ?bp ?bo }} }} }}"
    )
}

// ───────────────────────────── Datasources ─────────────────────────────

/// Insert or replace a datasource record. Delete-then-insert in one update,
/// so a concurrent reader sees either the old record or the new one.
pub fn put_source(store: &TripleStore, source: &SqlSource) -> Result<(), String> {
    let s = iri(&source.iri());
    let mut body = String::new();
    body.push_str(&format!("    a ds:SqlSource, dcat:DataService ;\n"));
    body.push_str(&format!("    ds:id {} ;\n", lit(&source.id)));
    body.push_str(&format!("    dct:title {} ;\n", lit(&source.name)));
    body.push_str(&format!("    ds:dialect {} ;\n", lit(&source.dialect)));
    body.push_str(&format!("    ds:database {} ;\n", lit(&source.database)));
    body.push_str(&opt_lit("ds:host", &source.host));
    if let Some(p) = source.port {
        body.push_str(&format!("    ds:port \"{p}\"^^xsd:integer ;\n"));
    }
    body.push_str(&opt_lit("ds:username", &source.username));
    if let Some(r) = &source.credential {
        body.push_str(&format!(
            "    ds:credential [ a ds:SecretRef ; ds:ref {} ] ;\n",
            lit(&r.to_string())
        ));
    }
    body.push_str(&format!(
        "    ds:readOnly \"{}\"^^xsd:boolean ;\n",
        source.read_only
    ));
    body.push_str(&format!(
        "    ds:statementTimeoutMs \"{}\"^^xsd:integer ;\n",
        source.statement_timeout_ms
    ));
    body.push_str(&opt_lit("ds:watermarkColumn", &source.watermark_column));
    body.push_str(&format!(
        "    ds:allowModelAssist \"{}\"^^xsd:boolean ;\n",
        source.allow_model_assist
    ));
    body.push_str(&format!("    ds:tls \"{}\"^^xsd:boolean ;\n", source.tls));
    for (k, v) in &source.options {
        body.push_str(&format!(
            "    ds:option [ ds:key {} ; ds:value {} ] ;\n",
            lit(k),
            lit(v)
        ));
    }
    body.push_str(&opt_lit("ds:dataset", &source.dataset));
    if let Some(o) = &source.owner {
        body.push_str(&format!("    ds:owner {} ;\n", iri(o)));
    }
    if let Some(a) = &source.created_by {
        body.push_str(&format!("    prov:wasAttributedTo {} ;\n", iri(a)));
    }
    if let Some(p) = &source.production {
        body.push_str(&format!("    ds:productionGraph {} ;\n", iri(&p.graph)));
        body.push_str(&format!("    ds:productionRun {} ;\n", lit(&p.run)));
    }
    if let Some(p) = &source.previous {
        body.push_str(&format!("    ds:previousGraph {} ;\n", iri(&p.graph)));
        body.push_str(&format!("    ds:previousRun {} ;\n", lit(&p.run)));
    }
    body.push_str(&format!("    dct:created {} ;\n", lit(&source.created_at)));
    body.push_str(&format!("    dct:modified {} .\n", lit(&source.updated_at)));

    let sparql = format!(
        "{pfx}{del};\nINSERT DATA {{ GRAPH <{SOURCES_GRAPH}> {{\n  {s}\n{body}}} }}",
        pfx = prefixes(),
        del = delete_subject_sparql(&source.iri()),
    );
    store.update(&sparql).map_err(|e| e.to_string())
}

fn source_select(filter: &str) -> String {
    format!(
        "{}SELECT ?id ?name ?dialect ?database ?host ?port ?username ?cred ?ro ?timeout \
         ?watermark ?assist ?tls ?dataset ?owner ?actor ?created ?modified ?pgraph ?prun ?qgraph ?qrun \
         WHERE {{ GRAPH <{SOURCES_GRAPH}> {{\n\
           ?s a ds:SqlSource ; ds:id ?id ; ds:dialect ?dialect ; ds:database ?database .\n\
           {filter}\n\
           OPTIONAL {{ ?s dct:title ?name }}\n\
           OPTIONAL {{ ?s ds:host ?host }}\n\
           OPTIONAL {{ ?s ds:port ?port }}\n\
           OPTIONAL {{ ?s ds:username ?username }}\n\
           OPTIONAL {{ ?s ds:credential/ds:ref ?cred }}\n\
           OPTIONAL {{ ?s ds:readOnly ?ro }}\n\
           OPTIONAL {{ ?s ds:statementTimeoutMs ?timeout }}\n\
           OPTIONAL {{ ?s ds:watermarkColumn ?watermark }}\n\
           OPTIONAL {{ ?s ds:allowModelAssist ?assist }}\n\
           OPTIONAL {{ ?s ds:tls ?tls }}\n\
           OPTIONAL {{ ?s ds:dataset ?dataset }}\n\
           OPTIONAL {{ ?s ds:owner ?owner }}\n\
           OPTIONAL {{ ?s prov:wasAttributedTo ?actor }}\n\
           OPTIONAL {{ ?s dct:created ?created }}\n\
           OPTIONAL {{ ?s dct:modified ?modified }}\n\
           OPTIONAL {{ ?s ds:productionGraph ?pgraph }}\n\
           OPTIONAL {{ ?s ds:productionRun ?prun }}\n\
           OPTIONAL {{ ?s ds:previousGraph ?qgraph }}\n\
           OPTIONAL {{ ?s ds:previousRun ?qrun }}\n\
         }} }} ORDER BY ?id",
        prefixes()
    )
}

fn row_to_source(store: &TripleStore, row: &HashMap<String, String>) -> SqlSource {
    let get = |k: &str| row.get(k).cloned();
    let id = get("id").unwrap_or_default();
    let options = read_options(store, &source_iri(&id));
    SqlSource {
        name: get("name").unwrap_or_else(|| id.clone()),
        dialect: get("dialect").unwrap_or_default(),
        database: get("database").unwrap_or_default(),
        host: get("host"),
        port: get("port").and_then(|p| p.parse().ok()),
        username: get("username"),
        // A stored reference that no longer parses is surfaced as "no
        // credential" rather than panicking; registration validated it, so
        // this only happens if the graph was edited by hand.
        credential: get("cred").and_then(|c| SecretRef::parse(&c).ok()),
        read_only: get("ro").map(|v| v == "true").unwrap_or(true),
        statement_timeout_ms: get("timeout").and_then(|v| v.parse().ok()).unwrap_or(30_000),
        watermark_column: get("watermark"),
        allow_model_assist: get("assist").map(|v| v == "true").unwrap_or(false),
        tls: get("tls").map(|v| v == "true").unwrap_or(false),
        options,
        dataset: get("dataset"),
        owner: get("owner"),
        created_by: get("actor"),
        created_at: get("created").unwrap_or_default(),
        updated_at: get("modified").unwrap_or_default(),
        production: match (get("pgraph"), get("prun")) {
            (Some(graph), Some(run)) => Some(RunPointer { graph, run }),
            _ => None,
        },
        previous: match (get("qgraph"), get("qrun")) {
            (Some(graph), Some(run)) => Some(RunPointer { graph, run }),
            _ => None,
        },
        id,
    }
}

fn read_options(store: &TripleStore, subject: &str) -> BTreeMap<String, String> {
    let q = format!(
        "{}SELECT ?k ?v WHERE {{ GRAPH <{SOURCES_GRAPH}> {{ {} ds:option ?o . ?o ds:key ?k ; ds:value ?v }} }}",
        prefixes(),
        iri(subject)
    );
    select(store, &q)
        .into_iter()
        .filter_map(|r| Some((r.get("k")?.clone(), r.get("v")?.clone())))
        .collect()
}

pub fn get_source(store: &TripleStore, id: &str) -> Option<SqlSource> {
    if !valid_id(id) {
        return None;
    }
    let filter = format!("FILTER(?s = {})", iri(&source_iri(id)));
    select(store, &source_select(&filter))
        .first()
        .map(|r| row_to_source(store, r))
}

pub fn list_sources(store: &TripleStore) -> Vec<SqlSource> {
    select(store, &source_select(""))
        .iter()
        .map(|r| row_to_source(store, r))
        .collect()
}

pub fn delete_source(store: &TripleStore, id: &str) -> Result<(), String> {
    store
        .update(&delete_subject_sparql(&source_iri(id)))
        .map_err(|e| e.to_string())
}

/// Re-point a source at `production`, keeping `previous` as the graph a
/// rollback returns to. One update: a reader never sees a source with no
/// production graph between two runs.
pub fn set_production(
    store: &TripleStore,
    source_id: &str,
    production: Option<&RunPointer>,
    previous: Option<&RunPointer>,
) -> Result<(), String> {
    let s = iri(&source_iri(source_id));
    let mut inserts = format!("    {s} dct:modified {} .\n", lit(&now()));
    if let Some(p) = production {
        inserts.push_str(&format!(
            "    {s} ds:productionGraph {} ; ds:productionRun {} .\n",
            iri(&p.graph),
            lit(&p.run)
        ));
    }
    if let Some(p) = previous {
        inserts.push_str(&format!(
            "    {s} ds:previousGraph {} ; ds:previousRun {} .\n",
            iri(&p.graph),
            lit(&p.run)
        ));
    }
    let sparql = format!(
        "{pfx}DELETE {{ GRAPH <{SOURCES_GRAPH}> {{\n\
         {s} ds:productionGraph ?pg ; ds:productionRun ?pr .\n\
         {s} ds:previousGraph ?qg ; ds:previousRun ?qr .\n\
         {s} dct:modified ?m .\n }} }}\n\
         INSERT {{ GRAPH <{SOURCES_GRAPH}> {{\n{inserts} }} }}\n\
         WHERE {{ GRAPH <{SOURCES_GRAPH}> {{\n\
           OPTIONAL {{ {s} ds:productionGraph ?pg }} OPTIONAL {{ {s} ds:productionRun ?pr }}\n\
           OPTIONAL {{ {s} ds:previousGraph ?qg }} OPTIONAL {{ {s} ds:previousRun ?qr }}\n\
           OPTIONAL {{ {s} dct:modified ?m }}\n }} }}",
        pfx = prefixes()
    );
    store.update(&sparql).map_err(|e| e.to_string())
}

// ────────────────────────────── Mappings ──────────────────────────────

pub fn put_mapping(store: &TripleStore, m: &MappingRecord) -> Result<(), String> {
    let s = iri(&m.iri());
    let mut body = String::new();
    body.push_str("    a ds:Mapping ;\n");
    body.push_str(&format!("    ds:id {} ;\n", lit(&m.id)));
    body.push_str(&format!("    dct:title {} ;\n", lit(&m.title)));
    body.push_str(&format!("    ds:source {} ;\n", iri(&source_iri(&m.source_id))));
    body.push_str(&format!(
        "    ds:currentVersion \"{}\"^^xsd:integer ;\n",
        m.version
    ));
    body.push_str(&format!("    ds:state {} ;\n", lit(m.state.as_str())));
    if let Some(g) = &m.shapes_graph {
        body.push_str(&format!("    dct:conformsTo {} ;\n", iri(g)));
    }
    body.push_str(&opt_lit("ds:model", &m.model));
    body.push_str(&opt_lit("ds:modelVersion", &m.model_version));
    if let Some(a) = &m.created_by {
        body.push_str(&format!("    prov:wasAttributedTo {} ;\n", iri(a)));
    }
    for v in 1..=m.version {
        body.push_str(&format!(
            "    ds:hasVersion {} ;\n",
            iri(&mapping_version_iri(&m.id, v))
        ));
    }
    body.push_str(&format!("    dct:created {} ;\n", lit(&m.created_at)));
    body.push_str(&format!("    dct:modified {} .\n", lit(&m.updated_at)));

    // Each frozen version is its own entity, and its IRI is the graph holding
    // that version's RML — so `prov:used <…:version:1>` on a run points at
    // exactly the triples that ran.
    let mut versions = String::new();
    for v in 1..=m.version {
        let vi = iri(&mapping_version_iri(&m.id, v));
        versions.push_str(&format!(
            "  {vi} a ds:MappingVersion, prov:Entity ; ds:mapping {s} ; ds:version \"{v}\"^^xsd:integer .\n"
        ));
    }

    let sparql = format!(
        "{pfx}{del};\nINSERT DATA {{ GRAPH <{SOURCES_GRAPH}> {{\n  {s}\n{body}{versions}}} }}",
        pfx = prefixes(),
        del = delete_subject_sparql(&m.iri()),
    );
    store.update(&sparql).map_err(|e| e.to_string())
}

fn mapping_select(filter: &str) -> String {
    format!(
        "{}SELECT ?id ?title ?source ?version ?state ?shapes ?model ?modelVersion ?created ?modified \
         WHERE {{ GRAPH <{SOURCES_GRAPH}> {{\n\
           ?s a ds:Mapping ; ds:id ?id ; ds:source ?source ; ds:currentVersion ?version .\n\
           {filter}\n\
           OPTIONAL {{ ?s dct:title ?title }}\n\
           OPTIONAL {{ ?s ds:state ?state }}\n\
           OPTIONAL {{ ?s dct:conformsTo ?shapes }}\n\
           OPTIONAL {{ ?s ds:model ?model }}\n\
           OPTIONAL {{ ?s ds:modelVersion ?modelVersion }}\n\
           OPTIONAL {{ ?s dct:created ?created }}\n\
           OPTIONAL {{ ?s dct:modified ?modified }}\n\
         }} }} ORDER BY ?id",
        prefixes()
    )
}

fn row_to_mapping(row: &HashMap<String, String>) -> MappingRecord {
    let get = |k: &str| row.get(k).cloned();
    let id = get("id").unwrap_or_default();
    MappingRecord {
        title: get("title").unwrap_or_else(|| id.clone()),
        source_id: get("source")
            .and_then(|s| s.strip_prefix("urn:source:").map(str::to_string))
            .unwrap_or_default(),
        version: get("version").and_then(|v| v.parse().ok()).unwrap_or(1),
        state: get("state")
            .and_then(|s| MappingState::parse(&s))
            .unwrap_or(MappingState::Draft),
        shapes_graph: get("shapes"),
        model: get("model"),
        model_version: get("modelVersion"),
        created_by: None,
        created_at: get("created").unwrap_or_default(),
        updated_at: get("modified").unwrap_or_default(),
        id,
    }
}

pub fn get_mapping(store: &TripleStore, id: &str) -> Option<MappingRecord> {
    if !valid_id(id) {
        return None;
    }
    let filter = format!("FILTER(?s = {})", iri(&mapping_iri(id)));
    select(store, &mapping_select(&filter))
        .first()
        .map(row_to_mapping)
}

pub fn list_mappings(store: &TripleStore, source_id: Option<&str>) -> Vec<MappingRecord> {
    let filter = match source_id {
        Some(s) => format!("FILTER(?source = {})", iri(&source_iri(s))),
        None => String::new(),
    };
    select(store, &mapping_select(&filter))
        .iter()
        .map(row_to_mapping)
        .collect()
}

/// Remove a mapping, its version entities and every version graph.
pub fn delete_mapping(store: &TripleStore, m: &MappingRecord) -> Result<(), String> {
    let graphs: Vec<String> = (1..=m.version)
        .map(|v| mapping_version_iri(&m.id, v))
        .collect();
    let refs: Vec<&str> = graphs.iter().map(String::as_str).collect();
    store.bulk_delete_graphs(&refs).map_err(|e| e.to_string())?;
    let mut sparql = delete_subject_sparql(&m.iri());
    for g in &graphs {
        sparql.push_str(&format!(";\n{}", delete_subject_sparql(g)));
    }
    store.update(&sparql).map_err(|e| e.to_string())
}

// ─────────────────────────────── Runs ───────────────────────────────

/// Record a run: PROV activity plus the operational counters.
pub fn put_run(store: &TripleStore, r: &RunRecord) -> Result<(), String> {
    let activity = iri(&run_activity_iri(&r.id));
    let graph = iri(&r.graph);
    let source = iri(&source_iri(&r.source_id));
    let version = iri(&mapping_version_iri(&r.mapping_id, r.mapping_version));
    let status = r.status.map(|s| s.as_str()).unwrap_or("failed");

    let mut body = format!(
        "    a prov:Activity, ds:Run ;\n\
         \x20   ds:id {} ;\n\
         \x20   ds:source {source} ;\n\
         \x20   ds:mapping {} ;\n\
         \x20   prov:used {source}, {version} ;\n\
         \x20   prov:generated {graph} ;\n\
         \x20   ds:status {} ;\n\
         \x20   ds:mode {} ;\n\
         \x20   ds:rowsExtracted \"{}\"^^xsd:integer ;\n\
         \x20   ds:triplesProduced \"{}\"^^xsd:integer ;\n\
         \x20   ds:durationMs \"{}\"^^xsd:integer ;\n\
         \x20   prov:startedAtTime \"{}\"^^xsd:dateTime ;\n\
         \x20   prov:endedAtTime \"{}\"^^xsd:dateTime ;\n",
        lit(&r.id),
        iri(&mapping_iri(&r.mapping_id)),
        lit(status),
        lit(&r.mode),
        r.rows_extracted,
        r.triples_produced,
        r.duration_ms,
        escape_sparql_literal(&r.started_at),
        escape_sparql_literal(&r.ended_at),
    );
    if let Some(mv) = &r.model_version {
        body.push_str(&format!("    ds:modelVersion {} ;\n", lit(mv)));
    }
    if let Some(p) = &r.previous_graph {
        body.push_str(&format!("    ds:previousGraph {} ;\n", iri(p)));
    }
    if let Some(a) = &r.actor {
        body.push_str(&format!("    prov:wasAssociatedWith {} ;\n", iri(a)));
    }
    if let Some(c) = r.conforms {
        body.push_str(&format!("    ds:conforms \"{c}\"^^xsd:boolean ;\n"));
        body.push_str(&format!(
            "    ds:violations \"{}\"^^xsd:integer ;\n",
            r.violations
        ));
    }
    if let Some(e) = &r.error {
        body.push_str(&format!("    ds:error {} ;\n", lit(e)));
    }
    body.push_str(&format!("    dct:created {} .\n", lit(&r.started_at)));

    // The graph the run produced is the PROV entity, so provenance can be
    // followed from the data back to the mapping version and the source.
    let entity = format!(
        "  {graph} a prov:Entity ; prov:wasGeneratedBy {activity} ; \
         prov:wasDerivedFrom {version} ; ds:run {} .\n",
        lit(&r.id)
    );

    let sparql = format!(
        "{pfx}{del};\nINSERT DATA {{ GRAPH <{SOURCES_GRAPH}> {{\n  {activity}\n{body}{entity}}} }}",
        pfx = prefixes(),
        del = delete_subject_sparql(&run_activity_iri(&r.id)),
    );
    store.update(&sparql).map_err(|e| e.to_string())
}

fn run_select(filter: &str) -> String {
    format!(
        "{}SELECT ?id ?source ?mapping ?version ?modelVersion ?status ?mode ?rows ?triples ?duration \
         ?started ?ended ?actor ?conforms ?violations ?error ?previous \
         WHERE {{ GRAPH <{SOURCES_GRAPH}> {{\n\
           ?a a ds:Run ; ds:id ?id ; ds:source ?source ; ds:mapping ?mapping ;\n\
              ds:status ?status ; ds:mode ?mode ; prov:startedAtTime ?started .\n\
           ?a prov:used ?v . ?v a ds:MappingVersion ; ds:version ?version .\n\
           {filter}\n\
           OPTIONAL {{ ?a ds:modelVersion ?modelVersion }}\n\
           OPTIONAL {{ ?a ds:rowsExtracted ?rows }}\n\
           OPTIONAL {{ ?a ds:triplesProduced ?triples }}\n\
           OPTIONAL {{ ?a ds:durationMs ?duration }}\n\
           OPTIONAL {{ ?a prov:endedAtTime ?ended }}\n\
           OPTIONAL {{ ?a prov:wasAssociatedWith ?actor }}\n\
           OPTIONAL {{ ?a ds:conforms ?conforms }}\n\
           OPTIONAL {{ ?a ds:violations ?violations }}\n\
           OPTIONAL {{ ?a ds:error ?error }}\n\
           OPTIONAL {{ ?a ds:previousGraph ?previous }}\n\
         }} }} ORDER BY DESC(?started)",
        prefixes()
    )
}

fn row_to_run(row: &HashMap<String, String>) -> RunRecord {
    let get = |k: &str| row.get(k).cloned();
    let id = get("id").unwrap_or_default();
    RunRecord {
        source_id: get("source")
            .and_then(|s| s.strip_prefix("urn:source:").map(str::to_string))
            .unwrap_or_default(),
        mapping_id: get("mapping")
            .and_then(|s| s.strip_prefix("urn:mapping:").map(str::to_string))
            .unwrap_or_default(),
        mapping_version: get("version").and_then(|v| v.parse().ok()).unwrap_or(1),
        model_version: get("modelVersion"),
        mode: get("mode").unwrap_or_else(|| "full".into()),
        status: get("status").and_then(|s| RunStatus::parse(&s)),
        graph: run_graph_iri(&id),
        previous_graph: get("previous"),
        rows_extracted: get("rows").and_then(|v| v.parse().ok()).unwrap_or(0),
        triples_produced: get("triples").and_then(|v| v.parse().ok()).unwrap_or(0),
        duration_ms: get("duration").and_then(|v| v.parse().ok()).unwrap_or(0),
        started_at: get("started").unwrap_or_default(),
        ended_at: get("ended").unwrap_or_default(),
        actor: get("actor"),
        conforms: get("conforms").map(|v| v == "true"),
        violations: get("violations").and_then(|v| v.parse().ok()).unwrap_or(0),
        error: get("error"),
        id,
    }
}

pub fn get_run(store: &TripleStore, id: &str) -> Option<RunRecord> {
    if !valid_id(id) {
        return None;
    }
    let filter = format!("FILTER(?a = {})", iri(&run_activity_iri(id)));
    select(store, &run_select(&filter)).first().map(row_to_run)
}

pub fn list_runs(store: &TripleStore, source_id: Option<&str>) -> Vec<RunRecord> {
    let filter = match source_id {
        Some(s) => format!("FILTER(?source = {})", iri(&source_iri(s))),
        None => String::new(),
    };
    select(store, &run_select(&filter))
        .iter()
        .map(row_to_run)
        .collect()
}

/// Delete a run's record and its graph.
pub fn delete_run(store: &TripleStore, r: &RunRecord) -> Result<(), String> {
    store
        .bulk_delete_graphs(&[r.graph.as_str()])
        .map_err(|e| e.to_string())?;
    let sparql = format!(
        "{};\n{}",
        delete_subject_sparql(&run_activity_iri(&r.id)),
        delete_subject_sparql(&r.graph)
    );
    store.update(&sparql).map_err(|e| e.to_string())
}

/// Record a rollback as its own activity, so re-pointing is auditable and
/// distinguishable from a run.
pub fn record_rollback(
    store: &TripleStore,
    source_id: &str,
    from: &RunPointer,
    to: &RunPointer,
    actor: Option<&str>,
) -> Result<(), String> {
    let id = uuid::Uuid::new_v4().to_string();
    let activity = iri(&format!("urn:rollback:{id}"));
    let mut body = format!(
        "    a prov:Activity, ds:Rollback ;\n\
         \x20   ds:id {} ;\n\
         \x20   ds:source {} ;\n\
         \x20   ds:fromGraph {} ;\n\
         \x20   ds:fromRun {} ;\n\
         \x20   ds:toGraph {} ;\n\
         \x20   ds:toRun {} ;\n\
         \x20   prov:used {} ;\n\
         \x20   prov:startedAtTime \"{now}\"^^xsd:dateTime ;\n\
         \x20   prov:endedAtTime \"{now}\"^^xsd:dateTime ;\n",
        lit(&id),
        iri(&source_iri(source_id)),
        iri(&from.graph),
        lit(&from.run),
        iri(&to.graph),
        lit(&to.run),
        iri(&to.graph),
        now = escape_sparql_literal(&now()),
    );
    if let Some(a) = actor {
        body.push_str(&format!("    prov:wasAssociatedWith {} ;\n", iri(a)));
    }
    body.push_str(&format!("    dct:created {} .\n", lit(&now())));
    let sparql = format!(
        "{pfx}INSERT DATA {{ GRAPH <{SOURCES_GRAPH}> {{\n  {activity}\n{body}}} }}",
        pfx = prefixes()
    );
    store.update(&sparql).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_source(id: &str) -> SqlSource {
        SqlSource {
            id: id.to_string(),
            name: format!("Source {id}"),
            dialect: "sqlite".into(),
            database: "/data/x.db".into(),
            credential: Some(SecretRef::parse("env:OTS_REG_TEST").unwrap()),
            read_only: true,
            statement_timeout_ms: 30_000,
            watermark_column: Some("updated_at".into()),
            allow_model_assist: false,
            options: BTreeMap::from([("sslmode".to_string(), "require".to_string())]),
            dataset: Some("ds-1".into()),
            created_by: Some("http://x/users/adm".into()),
            created_at: now(),
            updated_at: now(),
            ..Default::default()
        }
    }

    #[test]
    fn a_source_round_trips_through_rdf() {
        let store = TripleStore::in_memory().unwrap();
        let s = sample_source("legacy");
        put_source(&store, &s).unwrap();
        let back = get_source(&store, "legacy").expect("stored");
        assert_eq!(back.id, "legacy");
        assert_eq!(back.dialect, "sqlite");
        assert_eq!(back.database, "/data/x.db");
        assert_eq!(back.credential.unwrap().to_string(), "env:OTS_REG_TEST");
        assert_eq!(back.statement_timeout_ms, 30_000);
        assert_eq!(back.watermark_column.as_deref(), Some("updated_at"));
        assert!(back.read_only && !back.allow_model_assist);
        assert_eq!(back.options.get("sslmode").map(String::as_str), Some("require"));
        assert_eq!(back.dataset.as_deref(), Some("ds-1"));
        assert_eq!(list_sources(&store).len(), 1);
    }

    #[test]
    fn rewriting_a_source_leaves_no_stale_triples() {
        let store = TripleStore::in_memory().unwrap();
        let mut s = sample_source("legacy");
        put_source(&store, &s).unwrap();
        s.watermark_column = None;
        s.options.clear();
        s.credential = Some(SecretRef::parse("file:/run/secrets/db").unwrap());
        put_source(&store, &s).unwrap();
        let back = get_source(&store, "legacy").unwrap();
        assert_eq!(back.watermark_column, None, "the old watermark is gone");
        assert!(back.options.is_empty(), "the old option blank node is gone");
        assert_eq!(back.credential.unwrap().to_string(), "file:/run/secrets/db");
        // Exactly one source subject, and no orphaned blank nodes.
        assert_eq!(list_sources(&store).len(), 1);
        let leftovers = select(
            &store,
            &format!(
                "{}SELECT ?o WHERE {{ GRAPH <{SOURCES_GRAPH}> {{ ?o ds:key ?k }} }}",
                prefixes()
            ),
        );
        assert!(leftovers.is_empty(), "orphaned option nodes: {leftovers:?}");
    }

    #[test]
    fn production_swap_replaces_both_pointers_in_one_update() {
        let store = TripleStore::in_memory().unwrap();
        put_source(&store, &sample_source("s")).unwrap();
        let p1 = RunPointer { graph: "urn:run:a".into(), run: "a".into() };
        let p2 = RunPointer { graph: "urn:run:b".into(), run: "b".into() };
        set_production(&store, "s", Some(&p1), None).unwrap();
        assert_eq!(get_source(&store, "s").unwrap().production, Some(p1.clone()));
        set_production(&store, "s", Some(&p2), Some(&p1)).unwrap();
        let back = get_source(&store, "s").unwrap();
        assert_eq!(back.production, Some(p2));
        assert_eq!(back.previous, Some(p1));
        // Exactly one production pointer survives the swap.
        let rows = select(
            &store,
            &format!(
                "{}SELECT ?g WHERE {{ GRAPH <{SOURCES_GRAPH}> {{ <urn:source:s> ds:productionGraph ?g }} }}",
                prefixes()
            ),
        );
        assert_eq!(rows.len(), 1, "{rows:?}");
    }

    #[test]
    fn deleting_a_source_removes_it_entirely() {
        let store = TripleStore::in_memory().unwrap();
        put_source(&store, &sample_source("gone")).unwrap();
        delete_source(&store, "gone").unwrap();
        assert!(get_source(&store, "gone").is_none());
        assert!(list_sources(&store).is_empty());
    }

    fn sample_mapping(id: &str, version: u32) -> MappingRecord {
        MappingRecord {
            id: id.into(),
            title: "Products".into(),
            source_id: "legacy".into(),
            version,
            state: MappingState::Approved,
            shapes_graph: Some("urn:shapes:products".into()),
            model: Some("product-model".into()),
            model_version: Some("1.2.0".into()),
            created_by: None,
            created_at: now(),
            updated_at: now(),
        }
    }

    #[test]
    fn a_mapping_round_trips_with_one_entity_per_frozen_version() {
        let store = TripleStore::in_memory().unwrap();
        put_mapping(&store, &sample_mapping("m", 3)).unwrap();
        let back = get_mapping(&store, "m").expect("stored");
        assert_eq!(back.version, 3);
        assert_eq!(back.source_id, "legacy");
        assert_eq!(back.state, MappingState::Approved);
        assert_eq!(back.shapes_graph.as_deref(), Some("urn:shapes:products"));
        assert_eq!(back.model_version.as_deref(), Some("1.2.0"));
        let versions = select(
            &store,
            &format!(
                "{}SELECT ?v WHERE {{ GRAPH <{SOURCES_GRAPH}> {{ ?v a ds:MappingVersion }} }}",
                prefixes()
            ),
        );
        assert_eq!(versions.len(), 3, "one entity per version: {versions:?}");
        assert_eq!(list_mappings(&store, Some("legacy")).len(), 1);
        assert!(list_mappings(&store, Some("other")).is_empty());
    }

    fn sample_run(id: &str, status: RunStatus) -> RunRecord {
        RunRecord {
            id: id.into(),
            source_id: "legacy".into(),
            mapping_id: "m".into(),
            mapping_version: 2,
            mode: "full".into(),
            status: Some(status),
            graph: run_graph_iri(id),
            previous_graph: Some("urn:run:old".into()),
            rows_extracted: 120,
            triples_produced: 480,
            duration_ms: 42,
            started_at: now(),
            ended_at: now(),
            actor: Some("http://x/users/adm".into()),
            conforms: Some(status == RunStatus::Succeeded),
            violations: if status == RunStatus::Succeeded { 0 } else { 7 },
            ..Default::default()
        }
    }

    #[test]
    fn a_run_round_trips_and_lists_newest_first() {
        let store = TripleStore::in_memory().unwrap();
        put_mapping(&store, &sample_mapping("m", 2)).unwrap();
        put_run(&store, &sample_run("r1", RunStatus::Succeeded)).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
        put_run(&store, &sample_run("r2", RunStatus::Rejected)).unwrap();

        let back = get_run(&store, "r1").expect("stored");
        assert_eq!(back.mapping_version, 2);
        assert_eq!(back.rows_extracted, 120);
        assert_eq!(back.triples_produced, 480);
        assert_eq!(back.status, Some(RunStatus::Succeeded));
        assert_eq!(back.conforms, Some(true));
        assert_eq!(back.graph, "urn:run:r1");
        assert_eq!(back.previous_graph.as_deref(), Some("urn:run:old"));

        let runs = list_runs(&store, Some("legacy"));
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].id, "r2", "newest first");
        let rejected = get_run(&store, "r2").unwrap();
        assert_eq!(rejected.status, Some(RunStatus::Rejected));
        assert_eq!(rejected.violations, 7);
        assert!(list_runs(&store, Some("nope")).is_empty());

        // The PROV shape a consumer follows: one activity, what it used, what
        // it generated, and its interval.
        let prov = select(
            &store,
            &format!(
                "{}SELECT ?used ?gen WHERE {{ GRAPH <{SOURCES_GRAPH}> {{ \
                 <urn:run:r1:activity> a prov:Activity ; prov:used ?used ; prov:generated ?gen ; \
                 prov:startedAtTime ?st ; prov:endedAtTime ?et . }} }}",
                prefixes()
            ),
        );
        assert!(!prov.is_empty(), "no PROV bindings for the run activity");
        let used: Vec<&str> = prov.iter().map(|r| r["used"].as_str()).collect();
        assert!(used.contains(&"urn:source:legacy"), "{prov:?}");
        assert!(used.contains(&"urn:mapping:m:version:2"), "{prov:?}");
        assert_eq!(prov[0]["gen"], "urn:run:r1");
    }

    #[test]
    fn a_rollback_is_recorded_as_its_own_activity() {
        let store = TripleStore::in_memory().unwrap();
        let from = RunPointer { graph: "urn:run:b".into(), run: "b".into() };
        let to = RunPointer { graph: "urn:run:a".into(), run: "a".into() };
        record_rollback(&store, "legacy", &from, &to, Some("http://x/users/adm")).unwrap();
        let rows = select(
            &store,
            &format!(
                "{}SELECT ?from ?to WHERE {{ GRAPH <{SOURCES_GRAPH}> {{ \
                 ?a a ds:Rollback ; ds:source <urn:source:legacy> ; ds:fromGraph ?from ; ds:toGraph ?to }} }}",
                prefixes()
            ),
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["from"], "urn:run:b");
        assert_eq!(rows[0]["to"], "urn:run:a");
        // A rollback is NOT a run.
        assert!(list_runs(&store, Some("legacy")).is_empty());
    }

    #[test]
    fn values_that_would_break_out_of_sparql_are_escaped() {
        let store = TripleStore::in_memory().unwrap();
        let mut s = sample_source("tricky");
        s.name = "quote \" brace } newline \n end".into();
        s.database = "/data/a\"b.db".into();
        put_source(&store, &s).unwrap();
        let back = get_source(&store, "tricky").expect("stored despite hostile text");
        assert_eq!(back.name, s.name);
        assert_eq!(back.database, s.database);
    }
}
