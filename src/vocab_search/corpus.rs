//! Term extraction for the vocabulary search index.
//!
//! Two corpora feed the index:
//!
//! * **LOV corpus** — the full `lov.nq.gz` N-Quads dump (one named graph per
//!   vocabulary).  Located via `VOCAB_CORPUS_PATH`, `{data_dir}/vocab/`, or
//!   the image-baked `assets/vocab/` copy; optionally downloaded once at boot
//!   (`VOCAB_CORPUS_URL`, sha256-verified) when absent.  Missing corpus is a
//!   supported degraded mode: term search then covers platform vocabularies
//!   only.
//! * **Platform vocabularies** — the latest published version graphs of every
//!   *public* model/vocabulary registry entry, read directly from the store
//!   (version graphs are not visible through `/sparql`; direct reads are the
//!   sanctioned path).
//!
//! Extraction mirrors the LOV indexer ("vocidex"): one document per term
//! defined in the vocabulary's namespace, typed class / property / datatype /
//! instance, with primary labels (rdfs:label, skos:prefLabel, dcterms/dc
//! title) and secondary text (comments, descriptions, altLabels,
//! definitions) kept per document.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use oxigraph::io::{RdfFormat, RdfParser};
use oxigraph::model::{GraphName, NamedNode, Quad, Term};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::data_models::models::DataModelRecord;
use crate::data_models::registry;
use crate::store::TripleStore;

use super::catalog::VocabCatalog;

// ─── Corpus location / download ──────────────────────────────────────────────

/// Pinned source for the optional first-boot corpus download: the Internet
/// Archive snapshot of the official LOV dump (the live host is unreliable —
/// the whole reason this service exists).
pub const DEFAULT_CORPUS_URL: &str =
    "https://web.archive.org/web/20251218081818id_/https://lov.linkeddata.es/lov.nq.gz";
pub const DEFAULT_CORPUS_SHA256: &str =
    "7b5522b4f86d642d7e48df289f3d3330898e9aa021cc4d4ef0ad38f0f039c233";

/// Candidate corpus locations, in order.
pub fn corpus_candidates(data_dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(p) = std::env::var("VOCAB_CORPUS_PATH") {
        if !p.is_empty() {
            out.push(PathBuf::from(p));
        }
    }
    out.push(data_dir.join("vocab").join("lov.nq.gz"));
    out.push(PathBuf::from("assets/vocab/lov.nq.gz"));
    out
}

/// Find an existing corpus file.
pub fn find_corpus(data_dir: &Path) -> Option<PathBuf> {
    corpus_candidates(data_dir)
        .into_iter()
        .find(|p| p.is_file())
}

/// Download the corpus to `{data_dir}/vocab/lov.nq.gz` if configured.
///
/// Returns the corpus path when available afterwards.  `VOCAB_CORPUS_URL=""`
/// disables the download; a custom URL skips checksum verification unless
/// `VOCAB_CORPUS_SHA256` is provided.
pub async fn ensure_corpus(data_dir: &Path) -> Option<PathBuf> {
    if let Some(existing) = find_corpus(data_dir) {
        return Some(existing);
    }
    let url = std::env::var("VOCAB_CORPUS_URL").unwrap_or_else(|_| DEFAULT_CORPUS_URL.to_string());
    if url.is_empty() {
        info!("LOV corpus not present and download disabled (VOCAB_CORPUS_URL=\"\")");
        return None;
    }
    let expected_sha = match std::env::var("VOCAB_CORPUS_SHA256") {
        Ok(s) if !s.is_empty() => Some(s),
        Ok(_) => None,
        Err(_) if url == DEFAULT_CORPUS_URL => Some(DEFAULT_CORPUS_SHA256.to_string()),
        Err(_) => None,
    };

    info!("Downloading LOV corpus from {} (one-off, ~18 MB)", url);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(600))
        .user_agent(concat!("open-triplestore/", env!("CARGO_PKG_VERSION")))
        .build()
        .ok()?;
    let bytes = match client
        .get(&url)
        .send()
        .await
        .and_then(|r| r.error_for_status())
    {
        Ok(resp) => match resp.bytes().await {
            Ok(b) => b,
            Err(e) => {
                warn!("LOV corpus download failed while reading body: {e}");
                return None;
            }
        },
        Err(e) => {
            warn!("LOV corpus download failed: {e} — vocabulary term search will cover platform vocabularies only");
            return None;
        }
    };

    if let Some(expected) = expected_sha {
        use sha2::{Digest, Sha256};
        let actual = hex::encode(Sha256::digest(&bytes));
        if actual != expected {
            warn!(
                "LOV corpus checksum mismatch (expected {expected}, got {actual}) — discarding download"
            );
            return None;
        }
    }

    let dest = data_dir.join("vocab").join("lov.nq.gz");
    if let Err(e) = std::fs::create_dir_all(dest.parent().unwrap_or(data_dir)) {
        warn!("Cannot create vocab dir: {e}");
        return None;
    }
    let tmp = dest.with_extension("gz.tmp");
    if let Err(e) = std::fs::write(&tmp, &bytes).and_then(|_| std::fs::rename(&tmp, &dest)) {
        warn!("Cannot persist LOV corpus: {e}");
        return None;
    }
    info!("LOV corpus stored at {:?}", dest);
    Some(dest)
}

// ─── Term documents ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TermType {
    Class,
    Property,
    Datatype,
    Instance,
}

impl TermType {
    pub fn as_str(self) -> &'static str {
        match self {
            TermType::Class => "class",
            TermType::Property => "property",
            TermType::Datatype => "datatype",
            TermType::Instance => "instance",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "class" => Some(TermType::Class),
            "property" => Some(TermType::Property),
            "datatype" => Some(TermType::Datatype),
            "instance" => Some(TermType::Instance),
            _ => None,
        }
    }
}

/// One searchable term.
#[derive(Debug, Clone)]
pub struct TermDoc {
    pub iri: String,
    pub local_name: String,
    /// `foaf:Person`-style prefixed name.
    pub prefixed: String,
    pub ttype: TermType,
    pub vocab_prefix: String,
    /// Primary labels (rdfs:label, skos:prefLabel, dcterms/dc:title).
    pub labels: Vec<String>,
    /// Secondary text (comments, descriptions, altLabels, definitions).
    pub secondary: Vec<String>,
    /// Parent vocabulary text (title + description + tags) — LOV's embedded
    /// `vocabulary.*` fields.
    pub vocab_text: String,
    pub tags: Vec<String>,
    /// LOD-corpus term metrics (occurrences, datasets) — 0 when unknown.
    pub occurrences: u64,
    pub reused_by_datasets: u64,
    /// Vocabulary-level fallback metrics.
    pub vocab_occurrences: u64,
    pub vocab_reused: u64,
    /// `lov` or `platform`.
    pub source: &'static str,
    /// Registry model id for platform terms (empty for LOV).
    pub model_id: String,
}

// Predicates
const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const PRIMARY_LABELS: &[&str] = &[
    "http://www.w3.org/2000/01/rdf-schema#label",
    "http://www.w3.org/2004/02/skos/core#prefLabel",
    "http://purl.org/dc/terms/title",
    "http://purl.org/dc/elements/1.1/title",
];
const SECONDARY_TEXT: &[&str] = &[
    "http://www.w3.org/2000/01/rdf-schema#comment",
    "http://purl.org/dc/terms/description",
    "http://purl.org/dc/elements/1.1/description",
    "http://www.w3.org/2004/02/skos/core#altLabel",
    "http://www.w3.org/2004/02/skos/core#definition",
];
const CLASS_TYPES: &[&str] = &[
    "http://www.w3.org/2002/07/owl#Class",
    "http://www.w3.org/2000/01/rdf-schema#Class",
];
const PROPERTY_TYPES: &[&str] = &[
    "http://www.w3.org/1999/02/22-rdf-syntax-ns#Property",
    "http://www.w3.org/2002/07/owl#ObjectProperty",
    "http://www.w3.org/2002/07/owl#DatatypeProperty",
    "http://www.w3.org/2002/07/owl#AnnotationProperty",
];
const DATATYPE_TYPE: &str = "http://www.w3.org/2000/01/rdf-schema#Datatype";
const SKOS_CONCEPT: &str = "http://www.w3.org/2004/02/skos/core#Concept";

/// Per-vocabulary cap on `instance` documents so instance-heavy vocabularies
/// don't dominate the index; classes/properties/datatypes — and SKOS concepts,
/// which ARE the terms of a SKOS vocabulary (IMBOR alone defines ~9k) — are
/// never capped.  Drops are reported via
/// [`ExtractionStats::instances_dropped`].
const INSTANCE_CAP_PER_VOCAB: usize = 2_000;

const MAX_TEXT_LEN: usize = 400;
const MAX_LITERALS_PER_BUCKET: usize = 8;

#[derive(Default)]
struct TermAcc {
    types: Vec<String>,
    labels: Vec<String>,
    secondary: Vec<String>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ExtractionStats {
    pub vocabularies: usize,
    pub terms: usize,
    pub instances_dropped: usize,
}

fn local_name(iri: &str) -> &str {
    let cut = iri.rfind(['#', '/']).map(|i| i + 1).unwrap_or(0);
    &iri[cut..]
}

fn push_text(bucket: &mut Vec<String>, value: &str) {
    if bucket.len() >= MAX_LITERALS_PER_BUCKET || value.trim().is_empty() {
        return;
    }
    let mut v = value.trim().to_string();
    if v.len() > MAX_TEXT_LEN {
        // Truncate on a char boundary.
        let mut end = MAX_TEXT_LEN;
        while !v.is_char_boundary(end) {
            end -= 1;
        }
        v.truncate(end);
    }
    if !bucket.contains(&v) {
        bucket.push(v);
    }
}

fn classify(types: &[String]) -> Option<TermType> {
    if types.iter().any(|t| PROPERTY_TYPES.contains(&t.as_str())) {
        return Some(TermType::Property);
    }
    if types.iter().any(|t| CLASS_TYPES.contains(&t.as_str())) {
        return Some(TermType::Class);
    }
    if types.iter().any(|t| t == DATATYPE_TYPE) {
        return Some(TermType::Datatype);
    }
    if types.is_empty() {
        None
    } else {
        Some(TermType::Instance)
    }
}

fn accumulate(acc: &mut TermAcc, predicate: &str, object: &Term) {
    match object {
        Term::NamedNode(nn) if predicate == RDF_TYPE => {
            if acc.types.len() < 16 {
                acc.types.push(nn.as_str().to_string());
            }
        }
        Term::Literal(lit) => {
            if PRIMARY_LABELS.contains(&predicate) {
                push_text(&mut acc.labels, lit.value());
            } else if SECONDARY_TEXT.contains(&predicate) {
                push_text(&mut acc.secondary, lit.value());
            }
        }
        _ => {}
    }
}

fn vocab_text_of(title: &str, description: &str, tags: &[String]) -> String {
    let mut s = String::with_capacity(title.len() + description.len() + 32);
    s.push_str(title);
    if !description.is_empty() {
        s.push('\n');
        s.push_str(&description[..description.len().min(MAX_TEXT_LEN)]);
    }
    for t in tags {
        s.push('\n');
        s.push_str(t);
    }
    s
}

/// Turn accumulated subjects of one vocabulary into term docs.
#[allow(clippy::too_many_arguments)]
fn finish_vocab(
    subjects: HashMap<String, TermAcc>,
    nsp: &str,
    uri: &str,
    prefix: &str,
    vocab_text: &str,
    tags: &[String],
    vocab_metrics: (u64, u64),
    term_metrics: &dyn Fn(&str) -> Option<[u64; 2]>,
    source: &'static str,
    model_id: &str,
    stats: &mut ExtractionStats,
    out: &mut Vec<TermDoc>,
) {
    let mut instances = 0usize;
    for (iri, acc) in subjects {
        // Only terms defined in the vocabulary's own namespace.
        if !(iri.starts_with(nsp) || iri.starts_with(uri)) || iri == nsp || iri == uri {
            continue;
        }
        let Some(ttype) = classify(&acc.types) else {
            continue;
        };
        let is_concept = acc.types.iter().any(|t| t == SKOS_CONCEPT);
        if ttype == TermType::Instance && !is_concept {
            instances += 1;
            if instances > INSTANCE_CAP_PER_VOCAB {
                stats.instances_dropped += 1;
                continue;
            }
        }
        let local = local_name(&iri).to_string();
        if local.is_empty() {
            continue;
        }
        let metrics = term_metrics(&iri).unwrap_or([0, 0]);
        out.push(TermDoc {
            prefixed: format!("{prefix}:{local}"),
            local_name: local,
            ttype,
            vocab_prefix: prefix.to_string(),
            labels: acc.labels,
            secondary: acc.secondary,
            vocab_text: vocab_text.to_string(),
            tags: tags.to_vec(),
            occurrences: metrics[0],
            reused_by_datasets: metrics[1],
            vocab_occurrences: vocab_metrics.0,
            vocab_reused: vocab_metrics.1,
            source,
            model_id: model_id.to_string(),
            iri,
        });
        stats.terms += 1;
    }
}

// ─── LOV corpus extraction ───────────────────────────────────────────────────

/// Stream-parse the LOV N-Quads dump and produce term docs for every
/// vocabulary the catalog knows.
pub fn extract_lov_terms(
    corpus_path: &Path,
    catalog: &VocabCatalog,
) -> anyhow::Result<(Vec<TermDoc>, ExtractionStats)> {
    let file = std::fs::File::open(corpus_path)?;
    let reader = std::io::BufReader::new(flate2::read::GzDecoder::new(file));
    extract_lov_terms_from_reader(reader, catalog)
}

/// Testable core of [`extract_lov_terms`].
pub fn extract_lov_terms_from_reader<R: Read>(
    reader: R,
    catalog: &VocabCatalog,
) -> anyhow::Result<(Vec<TermDoc>, ExtractionStats)> {
    // graph uri -> subject iri -> accumulator
    let mut graphs: HashMap<String, HashMap<String, TermAcc>> = HashMap::new();

    let parser = RdfParser::from_format(RdfFormat::NQuads).lenient();
    for quad in parser.for_reader(reader) {
        let quad = match quad {
            Ok(q) => q,
            Err(_) => continue, // lenient: skip malformed lines
        };
        let GraphName::NamedNode(g) = &quad.graph_name else {
            continue;
        };
        // Only vocabulary graphs the catalog knows (skips the metadata graph).
        let g_str = g.as_str();
        if catalog.lov_by_uri(g_str).is_none() {
            continue;
        }
        let Quad {
            subject,
            predicate,
            object,
            ..
        } = quad;
        let oxigraph::model::NamedOrBlankNode::NamedNode(s) = subject else {
            continue;
        };
        let acc = graphs
            .entry(g_str.to_string())
            .or_default()
            .entry(s.into_string())
            .or_default();
        accumulate(acc, predicate.as_str(), &object);
    }

    let mut stats = ExtractionStats::default();
    let mut out = Vec::new();
    for (graph_uri, subjects) in graphs {
        let Some(vocab) = catalog.lov_by_uri(&graph_uri) else {
            continue;
        };
        stats.vocabularies += 1;
        let title = vocab
            .titles
            .first()
            .map(|t| t.value.as_str())
            .unwrap_or(vocab.prefix.as_str());
        let description = vocab
            .descriptions
            .first()
            .map(|d| d.value.as_str())
            .unwrap_or("");
        let vt = vocab_text_of(title, description, &vocab.tags);
        finish_vocab(
            subjects,
            &vocab.nsp,
            &vocab.uri,
            &vocab.prefix,
            &vt,
            &vocab.tags,
            (
                vocab.metrics.occurrences_in_datasets,
                vocab.metrics.reused_by_datasets,
            ),
            &|iri| catalog.term_metrics(iri),
            "lov",
            "",
            &mut stats,
            &mut out,
        );
    }
    Ok((out, stats))
}

/// Extract the raw quads of one vocabulary graph from the corpus (for the
/// offline install flow).  Returns quads in the default graph so the caller
/// can load them into a registry version graph.
pub fn extract_vocab_quads(corpus_path: &Path, graph_uri: &str) -> anyhow::Result<Vec<Quad>> {
    let file = std::fs::File::open(corpus_path)?;
    let reader = std::io::BufReader::new(flate2::read::GzDecoder::new(file));
    let parser = RdfParser::from_format(RdfFormat::NQuads).lenient();
    let mut out = Vec::new();
    for quad in parser.for_reader(reader) {
        let Ok(quad) = quad else { continue };
        if let GraphName::NamedNode(g) = &quad.graph_name {
            if g.as_str() == graph_uri {
                out.push(Quad {
                    graph_name: GraphName::DefaultGraph,
                    ..quad
                });
            }
        }
    }
    Ok(out)
}

// ─── Platform extraction ─────────────────────────────────────────────────────

/// Term docs from the latest published version graphs of the given **public**
/// registry records (the caller pre-filters).
pub fn extract_platform_terms(
    store: &TripleStore,
    base_url: &str,
    records: &[DataModelRecord],
    catalog: &VocabCatalog,
) -> (Vec<TermDoc>, ExtractionStats) {
    let mut stats = ExtractionStats::default();
    let mut out = Vec::new();
    for record in records {
        if !record.is_public || record.namespace.is_empty() {
            continue;
        }
        let Some(latest) = record.latest_published.as_deref() else {
            continue;
        };
        let versions = registry::list_versions(store, base_url, &record.id);
        let Some(version) = versions.iter().find(|v| v.version == latest) else {
            continue;
        };
        let mut graph_iris = vec![version.graph_iri.clone()];
        graph_iris.extend(version.sub_graphs.iter().cloned());

        let mut subjects: HashMap<String, TermAcc> = HashMap::new();
        for graph_iri in &graph_iris {
            let Ok(graph) = NamedNode::new(graph_iri.clone()) else {
                continue;
            };
            let Ok(quads) = store.quads_for_graph(graph.as_ref().into()) else {
                continue;
            };
            for quad in quads {
                let oxigraph::model::NamedOrBlankNode::NamedNode(s) = quad.subject else {
                    continue;
                };
                let acc = subjects.entry(s.into_string()).or_default();
                accumulate(acc, quad.predicate.as_str(), &quad.object);
            }
        }
        if subjects.is_empty() {
            continue;
        }
        stats.vocabularies += 1;

        // Enrich with LOV vocabulary text/metrics when the namespace matches.
        let lov = catalog.info(&record.namespace, &super::catalog::public_only);
        let tags = lov.as_ref().map(|l| l.tags.clone()).unwrap_or_default();
        let vocab_metrics = lov
            .as_ref()
            .map(|l| {
                (
                    l.metrics.occurrences_in_datasets,
                    l.metrics.reused_by_datasets,
                )
            })
            .unwrap_or((0, 0));
        let vt = vocab_text_of(
            &record.title,
            record.description.as_deref().unwrap_or(""),
            &tags,
        );
        finish_vocab(
            subjects,
            &record.namespace,
            &record.namespace,
            &record.id,
            &vt,
            &tags,
            vocab_metrics,
            &|iri| catalog.term_metrics(iri),
            "platform",
            &record.id,
            &mut stats,
            &mut out,
        );
    }
    (out, stats)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_name_extraction() {
        assert_eq!(local_name("http://xmlns.com/foaf/0.1/name"), "name");
        assert_eq!(local_name("http://www.w3.org/2002/07/owl#Class"), "Class");
        assert_eq!(local_name("no-separators"), "no-separators");
    }

    #[test]
    fn classify_priorities() {
        let t = |s: &str| vec![s.to_string()];
        assert_eq!(
            classify(&t("http://www.w3.org/2002/07/owl#Class")),
            Some(TermType::Class)
        );
        assert_eq!(
            classify(&t("http://www.w3.org/2002/07/owl#ObjectProperty")),
            Some(TermType::Property)
        );
        assert_eq!(
            classify(&t("http://www.w3.org/2000/01/rdf-schema#Datatype")),
            Some(TermType::Datatype)
        );
        assert_eq!(
            classify(&t("http://example.org/SomeThing")),
            Some(TermType::Instance)
        );
        assert_eq!(classify(&[]), None);
        // Property beats class when both are declared.
        assert_eq!(
            classify(&[
                "http://www.w3.org/2002/07/owl#Class".to_string(),
                "http://www.w3.org/1999/02/22-rdf-syntax-ns#Property".to_string(),
            ]),
            Some(TermType::Property)
        );
    }

    #[test]
    fn lov_extraction_from_fixture() {
        let catalog = VocabCatalog::bundled();
        // Two FOAF terms in the real FOAF graph URI, one alien-namespace term.
        let nq = br#"<http://xmlns.com/foaf/0.1/Person> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://www.w3.org/2002/07/owl#Class> <http://xmlns.com/foaf/0.1/> .
<http://xmlns.com/foaf/0.1/Person> <http://www.w3.org/2000/01/rdf-schema#label> "Person" <http://xmlns.com/foaf/0.1/> .
<http://xmlns.com/foaf/0.1/name> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://www.w3.org/2002/07/owl#DatatypeProperty> <http://xmlns.com/foaf/0.1/> .
<http://example.org/alien> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://www.w3.org/2002/07/owl#Class> <http://xmlns.com/foaf/0.1/> .
"#;
        let (docs, stats) =
            extract_lov_terms_from_reader(std::io::Cursor::new(&nq[..]), &catalog).unwrap();
        assert_eq!(stats.vocabularies, 1);
        assert_eq!(docs.len(), 2, "alien-namespace subject must be excluded");
        let person = docs.iter().find(|d| d.local_name == "Person").unwrap();
        assert_eq!(person.ttype, TermType::Class);
        assert_eq!(person.prefixed, "foaf:Person");
        assert_eq!(person.vocab_prefix, "foaf");
        assert!(person.vocab_occurrences > 0, "vocab metrics attached");
        let name = docs.iter().find(|d| d.local_name == "name").unwrap();
        assert_eq!(name.ttype, TermType::Property);
    }
}
