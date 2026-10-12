//! ISO 21597-1 ICDD — the first container profile.
//!
//! Layout (ISO 21597-1, Part 1): `Index.rdf` at the root (RDF/XML, the one
//! `ct:ContainerDescription`), `Payload documents/` (every file a
//! `ct:InternalDocument`'s `ct:filename` or a `ct:FolderDocument`'s
//! `ct:foldername` names, relative to that folder), `Payload triples/` (the
//! linksets the description `ct:containsLinkset`), and `Ontology resources/`
//! (the ISO `Container.rdf` and `Linkset.rdf` ontologies, plus any other
//! ontology the container uses).
//!
//! Reading is lenient — anything the index describes and the archive carries
//! is imported, and what is wrong is reported by [`super::icdd_validate`].
//! Writing produces an index that passes that validator. The ISO ontology
//! files are never part of this repository: an export embeds them, unchanged,
//! when the operator points [`ONTOLOGY_DIR_ENV`] at a directory holding them.

use std::collections::{BTreeSet, HashSet};

use oxigraph::io::{RdfFormat, RdfParser, RdfSerializer};
use oxigraph::model::{
    Literal, NamedNode, NamedNodeRef, NamedOrBlankNode, NamedOrBlankNodeRef, Term, Triple,
};
use oxigraph::store::Store;

use super::{
    format_for, ChecksumStatus, ContainerManifest, ContainerProfile, ContainerReport,
    DocumentEntry, DocumentKind, DocumentMeta, Entry, FolderFile, Party, PayloadKind, RdfPayload,
};

pub const CT: &str = "https://standards.iso.org/iso/21597/-1/ed-1/en/Container#";
pub const LS: &str = "https://standards.iso.org/iso/21597/-1/ed-1/en/Linkset#";
/// The ontology IRIs an index and a linkset import.
pub const CT_ONTOLOGY: &str = "https://standards.iso.org/iso/21597/-1/ed-1/en/Container";
pub const OTS: &str = "https://opentriplestore.org/ns#";
const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const XSD: &str = "http://www.w3.org/2001/XMLSchema#";
const OWL: &str = "http://www.w3.org/2002/07/owl#";

/// The Part 1 conformance indicator.
pub const PART1_INDICATOR: &str = "ICDD-Part1-Container";
/// Part 2's indicator, inferred from the Part 1 naming (the published value
/// is in the paid ISO 21597-2 text); recognised only to say Part 2 is not
/// interpreted.
pub const PART2_INDICATOR: &str = "ICDD-Part2-Container";

pub const INDEX: &str = "Index.rdf";
pub const DOCS_DIR: &str = "Payload documents/";
pub const TRIPLES_DIR: &str = "Payload triples/";
pub const ONTO_DIR: &str = "Ontology resources/";
/// The ontology files ISO publishes for Part 1 (and Part 2's extension). They
/// are recognised by name in `Ontology resources/` and never loaded as data.
pub const STANDARD_ONTOLOGIES: [&str; 3] = ["Container.rdf", "Linkset.rdf", "ExtendedLinkset.rdf"];
/// The two a Part 1 container carries.
pub const PART1_ONTOLOGIES: [&str; 2] = ["Container.rdf", "Linkset.rdf"];
/// Directory holding the operator-supplied ISO `Container.rdf` and `Linkset.rdf`.
pub const ONTOLOGY_DIR_ENV: &str = "OTS_ICDD_ONTOLOGY_DIR";

pub struct Icdd;

// ── Reading helpers (shared with the validator and the export) ──────────────

pub(crate) fn ct(local: &str) -> NamedNode {
    NamedNode::new_unchecked(format!("{CT}{local}"))
}

pub(crate) fn ots(local: &str) -> NamedNode {
    NamedNode::new_unchecked(format!("{OTS}{local}"))
}

fn rdf_type() -> NamedNode {
    NamedNode::new_unchecked(RDF_TYPE)
}

/// Root entries named `index.*` (any case) — candidates for the index.
pub(crate) fn index_candidates(entries: &[Entry]) -> Vec<&Entry> {
    entries
        .iter()
        .filter(|e| {
            !e.is_dir()
                && !e.name.contains('/')
                && e.name.to_ascii_lowercase().starts_with("index.")
        })
        .collect()
}

/// The index: `Index.rdf` when present, else (leniently) another root
/// `index.*` in a known RDF syntax.
pub(crate) fn index_entry(entries: &[Entry]) -> Option<&Entry> {
    let candidates = index_candidates(entries);
    candidates
        .iter()
        .find(|e| e.name == INDEX)
        .or_else(|| {
            candidates
                .iter()
                .find(|e| e.name.eq_ignore_ascii_case(INDEX))
        })
        .or_else(|| candidates.iter().find(|e| format_for(&e.name).is_some()))
        .copied()
}

/// Parse an RDF entry into a fresh in-memory store.
pub(crate) fn parse_entry(entry: &Entry) -> anyhow::Result<(Store, RdfFormat)> {
    let fmt = format_for(&entry.name).unwrap_or(RdfFormat::RdfXml);
    let store = Store::new()?;
    store.load_from_reader(RdfParser::from_format(fmt), entry.bytes.as_slice())?;
    Ok((store, fmt))
}

pub(crate) fn node_str(n: NamedOrBlankNodeRef<'_>) -> String {
    match n {
        NamedOrBlankNodeRef::NamedNode(n) => n.as_str().to_string(),
        NamedOrBlankNodeRef::BlankNode(b) => format!("_:{}", b.as_str()),
    }
}

fn term_node(t: &Term) -> Option<NamedOrBlankNode> {
    match t {
        Term::NamedNode(n) => Some(n.clone().into()),
        Term::BlankNode(b) => Some(b.clone().into()),
        _ => None,
    }
}

fn term_text(t: &Term) -> Option<String> {
    match t {
        Term::NamedNode(n) => Some(n.as_str().to_string()),
        Term::Literal(l) => Some(l.value().to_string()),
        Term::BlankNode(b) => Some(format!("_:{}", b.as_str())),
        #[allow(unreachable_patterns)]
        _ => None,
    }
}

pub(crate) fn objects(store: &Store, s: NamedOrBlankNodeRef<'_>, p: NamedNodeRef<'_>) -> Vec<Term> {
    store
        .quads_for_pattern(Some(s), Some(p), None, None)
        .filter_map(Result::ok)
        .map(|q| q.object)
        .collect()
}

pub(crate) fn first_text(store: &Store, s: NamedOrBlankNodeRef<'_>, local: &str) -> Option<String> {
    objects(store, s, ct(local).as_ref())
        .iter()
        .find_map(term_text)
}

pub(crate) fn types_of(store: &Store, s: NamedOrBlankNodeRef<'_>) -> Vec<String> {
    objects(store, s, rdf_type().as_ref())
        .iter()
        .filter_map(|t| match t {
            Term::NamedNode(n) => Some(n.as_str().to_string()),
            _ => None,
        })
        .collect()
}

/// Every `ct:ContainerDescription` in an index.
pub(crate) fn container_nodes(store: &Store) -> Vec<NamedOrBlankNode> {
    let class = ct("ContainerDescription");
    let mut out: Vec<NamedOrBlankNode> = store
        .quads_for_pattern(
            None,
            Some(rdf_type().as_ref()),
            Some(class.as_ref().into()),
            None,
        )
        .filter_map(Result::ok)
        .map(|q| q.subject)
        .collect();
    out.dedup();
    out
}

/// The document kind an index gives a node: its `rdf:type`, else what its
/// properties imply.
pub(crate) fn document_kind(
    types: &[String],
    has_filename: bool,
    has_url: bool,
    has_foldername: bool,
) -> Option<DocumentKind> {
    let has = |local: &str| types.iter().any(|t| t == &format!("{CT}{local}"));
    if has("ExternalDocument") {
        Some(DocumentKind::External)
    } else if has("FolderDocument") {
        Some(DocumentKind::Folder)
    } else if has("EncryptedDocument") {
        Some(DocumentKind::Encrypted)
    } else if has("SecuredDocument") {
        Some(DocumentKind::Secured)
    } else if has("InternalDocument") {
        Some(DocumentKind::Internal)
    } else if has_foldername {
        Some(DocumentKind::Folder)
    } else if has_url && !has_filename {
        Some(DocumentKind::External)
    } else if has_filename {
        Some(DocumentKind::Internal)
    } else {
        None
    }
}

pub(crate) fn party(store: &Store, t: &Term) -> Party {
    match term_node(t) {
        Some(n) => {
            let types = types_of(store, n.as_ref());
            let kind = if types.iter().any(|t| t == &format!("{CT}Person")) {
                "person"
            } else if types.iter().any(|t| t == &format!("{CT}Organisation")) {
                "organisation"
            } else if types.iter().any(|t| t == &format!("{CT}Party")) {
                "party"
            } else {
                "untyped"
            };
            Party {
                iri: match &n {
                    NamedOrBlankNode::NamedNode(nn) => Some(nn.as_str().to_string()),
                    _ => None,
                },
                kind: kind.to_string(),
                name: first_text(store, n.as_ref(), "name"),
            }
        }
        None => Party {
            iri: None,
            kind: "literal".into(),
            name: term_text(t),
        },
    }
}

/// One document as the index describes it.
#[derive(Debug, Clone)]
pub(crate) struct IndexDoc {
    pub iri: String,
    pub kind: Option<DocumentKind>,
    pub filename: Option<String>,
    pub foldername: Option<String>,
    pub url: Option<String>,
    pub description: Option<String>,
    pub belongs_to: Vec<String>,
    /// `ots:graphRole` — our own marker on an RDF data document an export wrote.
    pub graph_role: Option<String>,
    /// `ots:assetId` / `ots:downloadUrl` / `ots:assetFolder` — what an import
    /// made of the document (only in a catalogue graph).
    pub asset_id: Option<String>,
    pub download_url: Option<String>,
    pub asset_folder: Option<String>,
    pub meta: DocumentMeta,
}

pub(crate) fn index_documents(store: &Store, container: NamedOrBlankNodeRef<'_>) -> Vec<IndexDoc> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for t in objects(store, container, ct("containsDocument").as_ref()) {
        let Some(n) = term_node(&t) else { continue };
        if !seen.insert(n.clone()) {
            continue;
        }
        out.push(index_doc(store, n));
    }
    out
}

fn index_doc(store: &Store, n: NamedOrBlankNode) -> IndexDoc {
    let s = n.as_ref();
    let text = |local: &str| first_text(store, s, local);
    let ots_text = |local: &str| {
        objects(store, s, ots(local).as_ref())
            .iter()
            .find_map(term_text)
    };
    let types = types_of(store, s);
    let filename = text("filename");
    let url = text("url");
    let foldername = text("foldername");
    let kind = document_kind(
        &types,
        filename.is_some(),
        url.is_some(),
        foldername.is_some(),
    );
    let party_of = |local: &str| {
        objects(store, s, ct(local).as_ref())
            .first()
            .map(|t| party(store, t))
    };
    let meta = DocumentMeta {
        name: text("name"),
        version_id: text("versionID"),
        version_description: text("versionDescription"),
        prior_version: text("priorVersion"),
        alternatives: objects(store, s, ct("alternativeDocument").as_ref())
            .iter()
            .filter_map(term_text)
            .collect(),
        requested: text("requested").map(|v| v == "true" || v == "1"),
        created_by: party_of("createdBy"),
        creation_date: text("creationDate"),
        modified_by: party_of("modifiedBy"),
        modification_date: text("modificationDate"),
        user_defined_id: text("userDefinedID"),
        checksum: text("checksum"),
        checksum_algorithm: text("checksumAlgorithm"),
        encryption_algorithm: text("encryptionAlgorithm"),
        filetype: text("filetype"),
        format: text("format"),
    };
    IndexDoc {
        iri: node_str(s),
        kind,
        filename,
        foldername,
        url,
        description: text("description"),
        belongs_to: objects(store, s, ct("belongsToContainer").as_ref())
            .iter()
            .filter_map(term_text)
            .collect(),
        graph_role: ots_text("graphRole"),
        asset_id: ots_text("assetId"),
        download_url: ots_text("downloadUrl"),
        asset_folder: ots_text("assetFolder"),
        meta,
    }
}

/// A `ct:filename` / `ct:foldername` that stays inside its folder: relative,
/// no `.`/`..` segments, no drive or scheme. Returns the normalised path.
pub(crate) fn safe_rel_path(p: &str) -> Option<String> {
    let p = p.replace('\\', "/");
    if p.starts_with('/') || p.contains(':') || p.contains('\0') {
        return None;
    }
    let mut segs = Vec::new();
    for seg in p.split('/') {
        match seg {
            "" | "." => continue,
            ".." => return None,
            s => segs.push(s),
        }
    }
    if segs.is_empty() {
        return None;
    }
    Some(segs.join("/"))
}

/// The archive entry for `filename` under `dir`: the exact path first, then
/// (for older exports and hand-made archives) case-insensitively, the bare
/// name, and the name under `dir` without its sub-folders.
pub(crate) fn find<'a>(entries: &'a [Entry], dir: &str, filename: &str) -> Option<&'a Entry> {
    let fname = filename.replace('\\', "/");
    let fname = fname.trim_start_matches("./");
    let exact = format!("{dir}{fname}");
    if let Some(e) = entries.iter().find(|e| !e.is_dir() && e.name == exact) {
        return Some(e);
    }
    let wanted = [
        exact,
        fname.to_string(),
        format!("{}{}", dir, fname.rsplit('/').next().unwrap_or(fname)),
    ];
    entries
        .iter()
        .filter(|e| !e.is_dir())
        .find(|e| wanted.iter().any(|w| e.name.eq_ignore_ascii_case(w)))
}

/// Is `name` one of ISO's ontology files in `Ontology resources/`?
pub(crate) fn is_standard_ontology(name: &str) -> bool {
    name.strip_prefix(ONTO_DIR).is_some_and(|f| {
        STANDARD_ONTOLOGIES
            .iter()
            .any(|s| s.eq_ignore_ascii_case(f))
    })
}

// ── Operator-supplied ISO ontology files ────────────────────────────────────

/// `Container.rdf` and `Linkset.rdf` from the directory [`ONTOLOGY_DIR_ENV`]
/// names, byte for byte, for an export's `Ontology resources/`. ISO publishes
/// them at <https://standards.iso.org/iso/21597/-1/ed-1/en/>; this repository
/// does not carry them. Returns the files found and a note per file missing.
pub fn operator_ontologies() -> (Vec<Entry>, Vec<String>) {
    let dir = std::env::var(ONTOLOGY_DIR_ENV).unwrap_or_default();
    let dir = dir.trim();
    if dir.is_empty() {
        return (
            Vec::new(),
            vec![format!(
                "{ONTOLOGY_DIR_ENV} is not set; the export has no Container.rdf / Linkset.rdf and is not Part 1-conformant"
            )],
        );
    }
    let listing: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
        .map(|rd| rd.filter_map(|e| e.ok().map(|e| e.path())).collect())
        .unwrap_or_default();
    let mut files = Vec::new();
    let mut notes = Vec::new();
    for name in PART1_ONTOLOGIES {
        let path = listing.iter().find(|p| {
            p.file_name()
                .and_then(|f| f.to_str())
                .is_some_and(|f| f.eq_ignore_ascii_case(name))
        });
        let Some(path) = path else {
            notes.push(format!("{ONTOLOGY_DIR_ENV}={dir} has no {name}"));
            continue;
        };
        match std::fs::read(path) {
            Ok(bytes) => {
                // Shipped unchanged, but only if it is the RDF/XML it claims to be.
                let ok = Store::new().is_ok_and(|s| {
                    s.load_from_reader(RdfParser::from_format(RdfFormat::RdfXml), bytes.as_slice())
                        .is_ok()
                });
                if ok {
                    files.push(Entry {
                        name: name.to_string(),
                        bytes,
                    });
                } else {
                    notes.push(format!("{} is not RDF/XML; not embedded", path.display()));
                }
            }
            Err(e) => notes.push(format!("{}: {e}", path.display())),
        }
    }
    (files, notes)
}

// ── Checksums ───────────────────────────────────────────────────────────────

/// The digest of `bytes` under a `ct:checksumAlgorithm` name (`SHA-256`,
/// `sha256`, `SHA-1`, …), or `None` for an algorithm we do not compute.
pub(crate) fn digest(algorithm: &str, bytes: &[u8]) -> Option<Vec<u8>> {
    use sha2::Digest as _;
    let alg: String = algorithm
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_uppercase();
    Some(match alg.as_str() {
        "SHA256" | "SHA2256" => sha2::Sha256::digest(bytes).to_vec(),
        "SHA384" | "SHA2384" => sha2::Sha384::digest(bytes).to_vec(),
        "SHA512" | "SHA2512" => sha2::Sha512::digest(bytes).to_vec(),
        "SHA224" | "SHA2224" => sha2::Sha224::digest(bytes).to_vec(),
        "SHA1" => sha1::Sha1::digest(bytes).to_vec(),
        _ => return None,
    })
}

/// Check a document's bytes against its `ct:checksum` (hex or base64).
pub(crate) fn verify_checksum(
    algorithm: Option<&str>,
    expected: Option<&str>,
    bytes: &[u8],
) -> ChecksumStatus {
    use base64::Engine as _;
    let (Some(alg), Some(expected)) = (algorithm, expected) else {
        return ChecksumStatus::Missing;
    };
    let Some(actual) = digest(alg, bytes) else {
        return ChecksumStatus::UnsupportedAlgorithm;
    };
    let expected: String = expected.chars().filter(|c| !c.is_whitespace()).collect();
    let hex_ok = hex::decode(&expected).is_ok_and(|v| v == actual);
    let b64_ok = base64::engine::general_purpose::STANDARD
        .decode(&expected)
        .is_ok_and(|v| v == actual);
    if hex_ok || b64_ok {
        ChecksumStatus::Verified
    } else {
        ChecksumStatus::Mismatch
    }
}

fn content_type_for(meta: &DocumentMeta, filename: &str) -> String {
    if let Some(f) = meta.format.as_deref().filter(|f| f.contains('/')) {
        return f.to_string();
    }
    let ft = meta
        .filetype
        .clone()
        .unwrap_or_else(|| filename.rsplit('.').next().unwrap_or("").to_string())
        .to_ascii_lowercase();
    match ft.trim_start_matches('.') {
        "pdf" => "application/pdf",
        "ifc" => "application/x-step",
        "txt" => "text/plain",
        "csv" => "text/csv",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "json" => "application/json",
        "xml" => "application/xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        _ => "application/octet-stream",
    }
    .to_string()
}

// ── The profile ─────────────────────────────────────────────────────────────

impl ContainerProfile for Icdd {
    fn id(&self) -> &'static str {
        "icdd"
    }
    fn label(&self) -> &'static str {
        "ICDD (ISO 21597-1)"
    }
    fn detect(&self, entries: &[Entry]) -> bool {
        index_entry(entries).is_some()
    }

    fn validate(&self, entries: &[Entry]) -> ContainerReport {
        super::icdd_validate::validate(entries)
    }

    fn read(&self, entries: &[Entry]) -> anyhow::Result<ContainerManifest> {
        let index = index_entry(entries)
            .ok_or_else(|| anyhow::anyhow!("no Index.rdf at the archive root"))?;
        let (store, fmt) = parse_entry(index)?;
        let text = String::from_utf8_lossy(&index.bytes).into_owned();
        let mut warnings = Vec::new();

        let containers = container_nodes(&store);
        let c = match containers.as_slice() {
            [] => anyhow::bail!("the index has no ct:ContainerDescription"),
            [c] => c.clone(),
            many => anyhow::bail!(
                "the index has {} ct:ContainerDescription nodes; a container has exactly one",
                many.len()
            ),
        };
        let cs = c.as_ref();
        let iri = match &c {
            NamedOrBlankNode::NamedNode(n) => n.as_str().to_string(),
            _ => "urn:icdd:container".into(),
        };
        let conformance: Vec<String> = objects(&store, cs, ct("conformanceIndicator").as_ref())
            .iter()
            .filter_map(term_text)
            .collect();
        let party_of = |local: &str| {
            objects(&store, cs, ct(local).as_ref())
                .first()
                .map(|t| party(&store, t))
        };

        let mut documents = Vec::new();
        let mut referenced: HashSet<String> = HashSet::new();
        let mut payloads: Vec<RdfPayload> = Vec::new();
        for d in index_documents(&store, cs) {
            let Some(kind) = d.kind else {
                warnings.push(format!(
                    "document <{}> has no document type, ct:filename, ct:foldername or ct:url; skipped",
                    d.iri
                ));
                continue;
            };
            match kind {
                DocumentKind::External => documents.push(DocumentEntry {
                    iri: d.iri,
                    kind,
                    filename: d.filename.unwrap_or_default(),
                    content_type: String::new(),
                    description: d.description,
                    external_url: d.url,
                    bytes: None,
                    files: Vec::new(),
                    checksum_status: None,
                    meta: d.meta,
                }),
                DocumentKind::Folder => {
                    let Some(folder) = d.foldername.as_deref().and_then(safe_rel_path) else {
                        warnings.push(format!(
                            "folder document <{}> has no usable ct:foldername; skipped",
                            d.iri
                        ));
                        continue;
                    };
                    let prefix = format!("{DOCS_DIR}{folder}/");
                    let mut files = Vec::new();
                    let mut present = false;
                    for e in entries {
                        let Some(rel) = e.name.strip_prefix(&prefix) else {
                            continue;
                        };
                        present = true;
                        if e.is_dir() || rel.is_empty() {
                            continue;
                        }
                        let Some(rel) = safe_rel_path(rel) else {
                            continue;
                        };
                        referenced.insert(e.name.clone());
                        files.push(FolderFile {
                            path: rel,
                            bytes: e.bytes.clone(),
                        });
                    }
                    if !present {
                        warnings.push(format!(
                            "folder {folder} listed in the index is missing from the archive"
                        ));
                        continue;
                    }
                    documents.push(DocumentEntry {
                        iri: d.iri,
                        kind,
                        filename: folder,
                        content_type: String::new(),
                        description: d.description,
                        external_url: None,
                        bytes: None,
                        files,
                        checksum_status: None,
                        meta: d.meta,
                    });
                }
                DocumentKind::Internal | DocumentKind::Secured | DocumentKind::Encrypted => {
                    let Some(fname) = d.filename.clone() else {
                        warnings.push(format!("document <{}> has no ct:filename; skipped", d.iri));
                        continue;
                    };
                    if safe_rel_path(&fname).is_none() {
                        warnings.push(format!(
                            "document <{}>: ct:filename {fname:?} leaves 'Payload documents/'; skipped",
                            d.iri
                        ));
                        continue;
                    }
                    let Some(entry) = find(entries, DOCS_DIR, &fname) else {
                        warnings.push(format!(
                            "document {fname} listed in the index is missing from the archive"
                        ));
                        continue;
                    };
                    referenced.insert(entry.name.clone());
                    let doc_iri = (!d.iri.starts_with("_:")).then(|| d.iri.clone());
                    // An RDF data document our own export wrote (`ots:graphRole`),
                    // or an RDF file under Payload triples/ / Ontology resources/
                    // listed as a document (as exports before 0.7 did): data, not
                    // an asset.
                    let role_kind = match d.graph_role.as_deref() {
                        Some("model") | Some("ontology") => Some(PayloadKind::Ontology),
                        Some(_) => Some(PayloadKind::Triples),
                        None if entry.name.starts_with(ONTO_DIR) => Some(PayloadKind::Ontology),
                        None if entry.name.starts_with(TRIPLES_DIR) => Some(PayloadKind::Triples),
                        None => None,
                    };
                    if let (Some(pk), Some(pfmt)) = (role_kind, format_for(&entry.name)) {
                        if kind == DocumentKind::Internal {
                            payloads.push(RdfPayload {
                                iri: doc_iri,
                                filename: entry
                                    .name
                                    .rsplit('/')
                                    .next()
                                    .unwrap_or(&entry.name)
                                    .to_string(),
                                kind: pk,
                                format: pfmt,
                                text: String::from_utf8_lossy(&entry.bytes).into_owned(),
                            });
                            continue;
                        }
                    }
                    let checksum_status =
                        (kind == DocumentKind::Secured || d.meta.checksum.is_some()).then(|| {
                            verify_checksum(
                                d.meta.checksum_algorithm.as_deref(),
                                d.meta.checksum.as_deref(),
                                &entry.bytes,
                            )
                        });
                    let content_type = if kind == DocumentKind::Encrypted {
                        "application/octet-stream".to_string()
                    } else {
                        content_type_for(&d.meta, &fname)
                    };
                    documents.push(DocumentEntry {
                        iri: d.iri,
                        kind,
                        filename: safe_rel_path(&fname).unwrap_or(fname),
                        content_type,
                        description: d.description,
                        external_url: None,
                        bytes: Some(entry.bytes.clone()),
                        files: Vec::new(),
                        checksum_status,
                        meta: d.meta,
                    });
                }
            }
        }

        // Linksets, then every other RDF under Payload triples/ and Ontology resources/.
        for t in objects(&store, cs, ct("containsLinkset").as_ref()) {
            let Some(l) = term_node(&t) else { continue };
            let l_iri = node_str(l.as_ref());
            let Some(fname) = first_text(&store, l.as_ref(), "filename") else {
                warnings.push(format!("linkset <{l_iri}> has no ct:filename; skipped"));
                continue;
            };
            let Some(entry) = find(entries, TRIPLES_DIR, &fname) else {
                warnings.push(format!(
                    "linkset {fname} listed in the index is missing from the archive"
                ));
                continue;
            };
            let Some(lfmt) = format_for(&entry.name) else {
                warnings.push(format!("linkset {fname}: unknown RDF syntax"));
                continue;
            };
            if !referenced.insert(entry.name.clone()) {
                continue;
            }
            payloads.push(RdfPayload {
                iri: (!l_iri.starts_with("_:")).then_some(l_iri),
                filename: fname,
                kind: PayloadKind::Linkset,
                format: lfmt,
                text: String::from_utf8_lossy(&entry.bytes).into_owned(),
            });
        }
        let mut standard_ontologies = Vec::new();
        for e in entries {
            if e.is_dir() || referenced.contains(&e.name) || e.name == index.name {
                continue;
            }
            if is_standard_ontology(&e.name) {
                standard_ontologies.push(e.name.trim_start_matches(ONTO_DIR).to_string());
                continue;
            }
            let kind = if e.name.starts_with(TRIPLES_DIR) {
                PayloadKind::Triples
            } else if e.name.starts_with(ONTO_DIR) {
                PayloadKind::Ontology
            } else {
                continue;
            };
            let Some(efmt) = format_for(&e.name) else {
                continue;
            };
            payloads.push(RdfPayload {
                iri: None,
                filename: e
                    .name
                    .trim_start_matches(TRIPLES_DIR)
                    .trim_start_matches(ONTO_DIR)
                    .to_string(),
                kind,
                format: efmt,
                text: String::from_utf8_lossy(&e.bytes).into_owned(),
            });
        }

        Ok(ContainerManifest {
            iri,
            title: None,
            description: first_text(&store, cs, "description"),
            created_by: party_of("createdBy"),
            published_by: party_of("publishedBy"),
            creation_date: first_text(&store, cs, "creationDate"),
            version_id: first_text(&store, cs, "versionID"),
            version_description: first_text(&store, cs, "versionDescription"),
            conformance,
            documents,
            payloads,
            ontology_resources: Vec::new(),
            standard_ontologies,
            index_text: text,
            index_format: Some(fmt),
            warnings,
        })
    }

    fn write(&self, m: &ContainerManifest) -> anyhow::Result<Vec<Entry>> {
        let mut w = IndexWriter::new(&m.iri)?;
        let container: NamedOrBlankNode = w.container.clone().into();
        let mut files: Vec<Entry> = Vec::new();
        let mut used: HashSet<String> = HashSet::new();
        for (dir, name) in m
            .ontology_resources
            .iter()
            .map(|e| (ONTO_DIR, e.name.as_str()))
        {
            used.insert(format!("{dir}{name}").to_ascii_lowercase());
        }

        // Ontology header: the index imports the Container ontology.
        let onto = NamedNode::new(format!("{}/index", m.iri))?;
        w.add(
            onto.clone(),
            rdf_type(),
            NamedNode::new_unchecked(format!("{OWL}Ontology")),
        );
        w.add(
            onto,
            NamedNode::new_unchecked(format!("{OWL}imports")),
            NamedNode::new_unchecked(CT_ONTOLOGY),
        );

        // The container description.
        w.add(container.clone(), rdf_type(), ct("ContainerDescription"));
        w.add(
            container.clone(),
            ct("conformanceIndicator"),
            Literal::new_simple_literal(PART1_INDICATOR),
        );
        if let Some(d) = m.description.as_deref().or(m.title.as_deref()) {
            w.add(
                container.clone(),
                ct("description"),
                Literal::new_simple_literal(d),
            );
        }
        let created = m
            .creation_date
            .clone()
            .unwrap_or_else(|| chrono::Utc::now().to_rfc3339());
        w.add(
            container.clone(),
            ct("creationDate"),
            date_literal(&created),
        );
        for v in [
            ("versionID", &m.version_id),
            ("versionDescription", &m.version_description),
        ] {
            if let (local, Some(val)) = v {
                w.add(
                    container.clone(),
                    ct(local),
                    Literal::new_simple_literal(val),
                );
            }
        }
        if let Some(p) = &m.created_by {
            let n = w.party(p)?;
            w.add(container.clone(), ct("createdBy"), n);
        }
        // Exactly one publisher is required; the creator stands in when absent.
        if let Some(p) = m.published_by.as_ref().or(m.created_by.as_ref()) {
            let n = w.party(p)?;
            w.add(container.clone(), ct("publishedBy"), n);
        }

        // Documents.
        for d in &m.documents {
            let node: NamedOrBlankNode = NamedNode::new(d.iri.as_str())
                .map(Into::into)
                .unwrap_or_else(|_| w.mint("document").into());
            w.add(container.clone(), ct("containsDocument"), node.clone());
            let class = match d.kind {
                DocumentKind::Internal => "InternalDocument",
                DocumentKind::External => "ExternalDocument",
                DocumentKind::Folder => "FolderDocument",
                DocumentKind::Secured => "SecuredDocument",
                DocumentKind::Encrypted => "EncryptedDocument",
            };
            w.add(node.clone(), rdf_type(), ct(class));
            if let Some(desc) = &d.description {
                w.add(
                    node.clone(),
                    ct("description"),
                    Literal::new_simple_literal(desc),
                );
            }
            w.meta(&node, &d.meta)?;
            match d.kind {
                DocumentKind::External => {
                    let url = d.external_url.clone().unwrap_or_default();
                    let name = d.meta.name.clone().unwrap_or_else(|| url.clone());
                    w.add(node.clone(), ct("name"), Literal::new_simple_literal(name));
                    w.add(node.clone(), ct("url"), Literal::new_simple_literal(url));
                }
                DocumentKind::Folder => {
                    let folder = unique_path(&mut used, DOCS_DIR, &d.filename, true);
                    let name = d.meta.name.clone().unwrap_or_else(|| folder.clone());
                    w.add(node.clone(), ct("name"), Literal::new_simple_literal(name));
                    w.add(
                        node.clone(),
                        ct("foldername"),
                        Literal::new_simple_literal(folder.clone()),
                    );
                    w.add(node.clone(), ct("belongsToContainer"), container.clone());
                    for f in &d.files {
                        let path = format!("{DOCS_DIR}{folder}/{}", f.path);
                        used.insert(path.to_ascii_lowercase());
                        files.push(Entry {
                            name: path,
                            bytes: f.bytes.clone(),
                        });
                    }
                }
                DocumentKind::Internal | DocumentKind::Secured | DocumentKind::Encrypted => {
                    let rel = unique_path(&mut used, DOCS_DIR, &d.filename, false);
                    let base = rel.rsplit('/').next().unwrap_or(&rel).to_string();
                    let ext = base
                        .rsplit_once('.')
                        .map(|(_, e)| e.to_ascii_lowercase())
                        .unwrap_or_default();
                    let name = d.meta.name.clone().unwrap_or_else(|| base.clone());
                    w.add(node.clone(), ct("name"), Literal::new_simple_literal(name));
                    w.add(
                        node.clone(),
                        ct("filename"),
                        Literal::new_simple_literal(rel.clone()),
                    );
                    let filetype = d.meta.filetype.clone().unwrap_or(ext);
                    w.add(
                        node.clone(),
                        ct("filetype"),
                        Literal::new_simple_literal(filetype),
                    );
                    let format = d
                        .meta
                        .format
                        .clone()
                        .unwrap_or_else(|| d.content_type.clone());
                    if !format.is_empty() {
                        w.add(
                            node.clone(),
                            ct("format"),
                            Literal::new_simple_literal(format),
                        );
                    }
                    w.add(node.clone(), ct("belongsToContainer"), container.clone());
                    let bytes = d.bytes.clone().unwrap_or_default();
                    if d.kind == DocumentKind::Secured {
                        let alg = d
                            .meta
                            .checksum_algorithm
                            .clone()
                            .filter(|a| digest(a, b"").is_some())
                            .unwrap_or_else(|| "SHA-256".to_string());
                        let sum = hex::encode(digest(&alg, &bytes).unwrap_or_default());
                        w.add(
                            node.clone(),
                            ct("checksum"),
                            Literal::new_simple_literal(sum),
                        );
                        w.add(
                            node.clone(),
                            ct("checksumAlgorithm"),
                            Literal::new_simple_literal(alg),
                        );
                    }
                    if d.kind == DocumentKind::Encrypted {
                        if let Some(a) = &d.meta.encryption_algorithm {
                            w.add(
                                node.clone(),
                                ct("encryptionAlgorithm"),
                                Literal::new_simple_literal(a),
                            );
                        }
                    }
                    files.push(Entry {
                        name: format!("{DOCS_DIR}{rel}"),
                        bytes,
                    });
                }
            }
        }

        // RDF payloads: linksets under Payload triples/, data graphs as RDF
        // documents under Payload documents/, ontologies under Ontology resources/.
        for p in &m.payloads {
            let about = p
                .iri
                .clone()
                .and_then(|i| NamedNode::new(i).ok())
                .unwrap_or_else(|| w.mint("payload"));
            match p.kind {
                PayloadKind::Linkset => {
                    let rel = unique_path(&mut used, TRIPLES_DIR, &p.filename, false);
                    w.add(container.clone(), ct("containsLinkset"), about.clone());
                    w.add(about.clone(), rdf_type(), ct("Linkset"));
                    w.add(
                        about,
                        ct("filename"),
                        Literal::new_simple_literal(rel.clone()),
                    );
                    files.push(Entry {
                        name: format!("{TRIPLES_DIR}{rel}"),
                        bytes: p.text.clone().into_bytes(),
                    });
                }
                PayloadKind::Triples => {
                    let rel = unique_path(&mut used, DOCS_DIR, &p.filename, false);
                    let ext = rel
                        .rsplit_once('.')
                        .map(|(_, e)| e.to_string())
                        .unwrap_or_default();
                    w.add(container.clone(), ct("containsDocument"), about.clone());
                    w.add(about.clone(), rdf_type(), ct("InternalDocument"));
                    w.add(
                        about.clone(),
                        ct("name"),
                        Literal::new_simple_literal(rel.clone()),
                    );
                    w.add(
                        about.clone(),
                        ct("filename"),
                        Literal::new_simple_literal(rel.clone()),
                    );
                    w.add(
                        about.clone(),
                        ct("filetype"),
                        Literal::new_simple_literal(ext),
                    );
                    w.add(
                        about.clone(),
                        ct("format"),
                        Literal::new_simple_literal(p.format.media_type()),
                    );
                    w.add(about.clone(), ct("belongsToContainer"), container.clone());
                    w.add(
                        about,
                        ots("graphRole"),
                        Literal::new_simple_literal("instances"),
                    );
                    files.push(Entry {
                        name: format!("{DOCS_DIR}{rel}"),
                        bytes: p.text.clone().into_bytes(),
                    });
                }
                PayloadKind::Ontology => {
                    let rel = unique_path(&mut used, ONTO_DIR, &p.filename, false);
                    files.push(Entry {
                        name: format!("{ONTO_DIR}{rel}"),
                        bytes: p.text.clone().into_bytes(),
                    });
                }
            }
        }

        let mut entries = vec![
            Entry {
                name: INDEX.into(),
                bytes: w.finish()?,
            },
            Entry::dir(ONTO_DIR),
            Entry::dir(DOCS_DIR),
            Entry::dir(TRIPLES_DIR),
        ];
        for o in &m.ontology_resources {
            entries.push(Entry {
                name: format!("{ONTO_DIR}{}", o.name),
                bytes: o.bytes.clone(),
            });
        }
        entries.extend(files);
        Ok(entries)
    }
}

fn date_literal(v: &str) -> Literal {
    Literal::new_typed_literal(v, NamedNode::new_unchecked(format!("{XSD}dateTime")))
}

/// A path under `dir` no other entry uses (case-insensitively): `name`, else
/// `stem-2.ext`, `stem-3.ext`, … Records the result in `used`. A folder name
/// claims its whole subtree.
fn unique_path(used: &mut HashSet<String>, dir: &str, name: &str, folder: bool) -> String {
    let clean = safe_rel_path(name).unwrap_or_else(|| "document".to_string());
    let taken = |used: &HashSet<String>, cand: &str| {
        let full = format!("{dir}{cand}").to_ascii_lowercase();
        if folder {
            let pre = format!("{full}/");
            used.iter().any(|u| *u == full || u.starts_with(&pre))
        } else {
            used.contains(&full)
                || used
                    .iter()
                    .any(|u| u.ends_with('/') && full.starts_with(u.as_str()))
        }
    };
    let mut cand = clean.clone();
    let mut n = 2;
    while taken(used, &cand) {
        cand = match (folder, clean.rsplit_once('.')) {
            (false, Some((stem, ext))) if !stem.is_empty() && !ext.contains('/') => {
                format!("{stem}-{n}.{ext}")
            }
            _ => format!("{clean}-{n}"),
        };
        n += 1;
    }
    let full = format!("{dir}{cand}").to_ascii_lowercase();
    used.insert(if folder { format!("{full}/") } else { full });
    cand
}

/// Builds the index as triples, then writes RDF/XML.
struct IndexWriter {
    container: NamedNode,
    triples: Vec<Triple>,
    minted: usize,
    parties: Vec<(Party, NamedNode)>,
}

impl IndexWriter {
    fn new(iri: &str) -> anyhow::Result<Self> {
        Ok(Self {
            container: NamedNode::new(iri)?,
            triples: Vec::new(),
            minted: 0,
            parties: Vec::new(),
        })
    }

    fn add(&mut self, s: impl Into<NamedOrBlankNode>, p: NamedNode, o: impl Into<Term>) {
        self.triples.push(Triple::new(s, p, o));
    }

    fn mint(&mut self, what: &str) -> NamedNode {
        self.minted += 1;
        NamedNode::new_unchecked(format!(
            "{}/{what}/{}",
            self.container.as_str(),
            self.minted
        ))
    }

    /// A party as `ct:Person` or `ct:Organisation` with its one `ct:name`
    /// (`ct:Party` is abstract; an untyped party is written as an organisation).
    fn party(&mut self, p: &Party) -> anyhow::Result<NamedNode> {
        if let Some((_, n)) = self.parties.iter().find(|(q, _)| q == p) {
            return Ok(n.clone());
        }
        let node = match p.iri.as_deref().and_then(|i| NamedNode::new(i).ok()) {
            Some(n) => n,
            None => self.mint("party"),
        };
        let class = if p.kind == "person" {
            "Person"
        } else {
            "Organisation"
        };
        self.add(node.clone(), rdf_type(), ct(class));
        let name = p
            .name
            .clone()
            .or_else(|| p.iri.clone())
            .unwrap_or_else(|| "unknown".into());
        self.add(node.clone(), ct("name"), Literal::new_simple_literal(name));
        self.parties.push((p.clone(), node.clone()));
        Ok(node)
    }

    fn meta(&mut self, node: &NamedOrBlankNode, m: &DocumentMeta) -> anyhow::Result<()> {
        for (local, v) in [
            ("versionID", &m.version_id),
            ("versionDescription", &m.version_description),
            ("userDefinedID", &m.user_defined_id),
        ] {
            if let Some(v) = v {
                self.add(node.clone(), ct(local), Literal::new_simple_literal(v));
            }
        }
        for (local, v) in [
            ("creationDate", &m.creation_date),
            ("modificationDate", &m.modification_date),
        ] {
            if let Some(v) = v {
                self.add(node.clone(), ct(local), date_literal(v));
            }
        }
        if let Some(p) = m
            .prior_version
            .as_deref()
            .and_then(|i| NamedNode::new(i).ok())
        {
            self.add(node.clone(), ct("priorVersion"), p);
        }
        for a in &m.alternatives {
            if let Ok(a) = NamedNode::new(a.as_str()) {
                self.add(node.clone(), ct("alternativeDocument"), a);
            }
        }
        if let Some(r) = m.requested {
            self.add(
                node.clone(),
                ct("requested"),
                Literal::new_typed_literal(
                    if r { "true" } else { "false" },
                    NamedNode::new_unchecked(format!("{XSD}boolean")),
                ),
            );
        }
        for (local, p) in [("createdBy", &m.created_by), ("modifiedBy", &m.modified_by)] {
            if let Some(p) = p {
                let n = self.party(p)?;
                self.add(node.clone(), ct(local), n);
            }
        }
        Ok(())
    }

    fn finish(self) -> anyhow::Result<Vec<u8>> {
        let mut ser = RdfSerializer::from_format(RdfFormat::RdfXml);
        for (p, ns) in [
            ("ct", CT),
            ("ls", LS),
            ("owl", OWL),
            ("xsd", XSD),
            ("ots", OTS),
        ] {
            ser = ser.with_prefix(p, ns)?;
        }
        let mut buf = Vec::new();
        let mut out = ser.for_writer(&mut buf);
        // Group by subject so each description is written once.
        let mut subjects: Vec<NamedOrBlankNode> = Vec::new();
        let mut seen = BTreeSet::new();
        for t in &self.triples {
            if seen.insert(t.subject.to_string()) {
                subjects.push(t.subject.clone());
            }
        }
        let mut written = HashSet::new();
        for s in subjects {
            for t in self.triples.iter().filter(|t| t.subject == s) {
                if written.insert(t.to_string()) {
                    out.serialize_triple(t.as_ref())?;
                }
            }
        }
        out.finish()?;
        Ok(buf)
    }
}
