//! Linked-document containers: a packaged archive of payload documents, RDF
//! payloads, link graphs and an index describing them — imported into a
//! dataset (documents → the dataset's assets, RDF → role-typed graphs, the
//! index → a catalogue graph) and exported from one.
//!
//! The mechanism is profile-neutral: [`ContainerProfile`] validates an
//! archive, reads it into a [`ContainerManifest`] and writes one back. ISO
//! 21597-1 **ICDD** ([`icdd`], validated by [`icdd_validate`]) is the first
//! profile. Needs the `asset-archive` feature (ZIP).
//!
//! * `POST /api/datasets/:id/containers/import?profile=icdd[&strict=true]` — body: the ZIP
//! * `GET  /api/datasets/:id/containers/export?profile=icdd` — an `.icdd` (ZIP) download
//! * `POST /api/containers/validate?profile=icdd` — the validation report only

pub mod icdd;
pub mod icdd_validate;

#[cfg(feature = "asset-archive")]
use std::io::{Cursor, Read};

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use oxigraph::io::RdfFormat;
use serde::{Deserialize, Serialize};

use crate::auth::middleware::AuthenticatedUser;
use crate::auth::models::{Dataset, GraphKind};
use crate::server::AppState;

/// Hard caps against archive bombs.
#[cfg(feature = "asset-archive")]
const MAX_ENTRY_BYTES: u64 = 512 * 1024 * 1024;
#[cfg(feature = "asset-archive")]
const MAX_TOTAL_BYTES: u64 = 2 * 1024 * 1024 * 1024;
#[cfg(feature = "asset-archive")]
const MAX_ENTRIES: usize = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PayloadKind {
    /// A link graph between documents / elements (role `linkset`).
    Linkset,
    /// Instance data (role `instances`).
    Triples,
    /// An ontology or shapes file (role `model`).
    Ontology,
}

impl PayloadKind {
    pub fn role(self) -> GraphKind {
        match self {
            PayloadKind::Linkset => GraphKind::Linkset,
            PayloadKind::Triples => GraphKind::Instances,
            PayloadKind::Ontology => GraphKind::Model,
        }
    }
}

/// What kind of document an index entry is (ISO 21597-1 document classes).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum DocumentKind {
    /// A file under `Payload documents/`.
    #[default]
    Internal,
    /// A URL; nothing is fetched.
    External,
    /// A folder under `Payload documents/`, imported as an asset sub-folder.
    Folder,
    /// An internal document with a checksum, verified on import.
    Secured,
    /// An internal document whose bytes are encrypted; kept opaque.
    Encrypted,
}

/// The outcome of checking a secured document's `ct:checksum`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ChecksumStatus {
    Verified,
    Mismatch,
    UnsupportedAlgorithm,
    Missing,
}

/// A party (`ct:Person` / `ct:Organisation`) named by an index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Party {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub iri: Option<String>,
    /// `person`, `organisation`, `party` (the abstract class), `untyped` or `literal`.
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// The index's metadata on a document, kept from import to export.
#[derive(Debug, Clone, Default, Serialize)]
pub struct DocumentMeta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version_description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prior_version: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub alternatives: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requested: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_by: Option<Party>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub creation_date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified_by: Option<Party>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modification_date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_defined_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checksum: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checksum_algorithm: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encryption_algorithm: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filetype: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
}

/// One file inside a folder document, by its path relative to the folder.
#[derive(Debug, Clone)]
pub struct FolderFile {
    pub path: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, Default)]
pub struct DocumentEntry {
    /// The document's IRI in the index.
    pub iri: String,
    pub kind: DocumentKind,
    /// Internal documents: the path relative to `Payload documents/`;
    /// folder documents: the folder name.
    pub filename: String,
    pub content_type: String,
    pub description: Option<String>,
    /// An external document: a URL instead of bytes.
    pub external_url: Option<String>,
    pub bytes: Option<Vec<u8>>,
    /// A folder document's files.
    pub files: Vec<FolderFile>,
    pub checksum_status: Option<ChecksumStatus>,
    pub meta: DocumentMeta,
}

#[derive(Debug, Clone)]
pub struct RdfPayload {
    /// The IRI the index gives this payload (a linkset's IRI), if any.
    pub iri: Option<String>,
    pub filename: String,
    pub kind: PayloadKind,
    pub format: RdfFormat,
    pub text: String,
}

/// A container, independent of the archive layout that carried it.
#[derive(Debug, Clone, Default)]
pub struct ContainerManifest {
    /// The container description's IRI.
    pub iri: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub created_by: Option<Party>,
    pub published_by: Option<Party>,
    pub creation_date: Option<String>,
    pub version_id: Option<String>,
    pub version_description: Option<String>,
    pub conformance: Vec<String>,
    pub documents: Vec<DocumentEntry>,
    pub payloads: Vec<RdfPayload>,
    /// Files written to the profile's ontology folder unchanged (export).
    pub ontology_resources: Vec<Entry>,
    /// Standard ontology files the archive carried and the import skipped.
    pub standard_ontologies: Vec<String>,
    /// The index document itself, to keep as the catalogue graph.
    pub index_text: String,
    pub index_format: Option<RdfFormat>,
    pub warnings: Vec<String>,
}

/// One archive entry. A name ending in `/` is a folder (no bytes).
#[derive(Debug, Clone)]
pub struct Entry {
    pub name: String,
    pub bytes: Vec<u8>,
}

impl Entry {
    pub fn dir(name: &str) -> Self {
        Entry {
            name: if name.ends_with('/') {
                name.to_string()
            } else {
                format!("{name}/")
            },
            bytes: Vec::new(),
        }
    }
    pub fn is_dir(&self) -> bool {
        self.name.ends_with('/')
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FindingSeverity {
    Violation,
    Warning,
    Info,
}

/// One finding of a container validation.
#[derive(Debug, Clone, Serialize)]
pub struct Finding {
    pub severity: FindingSeverity,
    /// A stable code: `shape:<ShapeName>` for a shape result, else the
    /// structural check (`document-missing`, `checksum-mismatch`, …).
    pub code: String,
    pub message: String,
    /// The index node the finding is about.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focus: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// The archive entry the finding is about.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shape: Option<String>,
}

impl Finding {
    pub fn new(severity: FindingSeverity, code: &str, message: impl Into<String>) -> Self {
        Finding {
            severity,
            code: code.to_string(),
            message: message.into(),
            focus: None,
            path: None,
            file: None,
            shape: None,
        }
    }
    pub fn focus(mut self, f: &str) -> Self {
        self.focus = Some(f.to_string());
        self
    }
    pub fn file(mut self, f: &str) -> Self {
        self.file = Some(f.to_string());
        self
    }
}

/// A profile's validation report on an archive. `conforms` means no
/// violation; warnings (a missing ontology file, say) are listed beside it.
#[derive(Debug, Clone, Serialize)]
pub struct ContainerReport {
    pub profile: &'static str,
    pub conforms: bool,
    pub violations: usize,
    pub warnings: usize,
    pub findings: Vec<Finding>,
}

impl ContainerReport {
    pub fn new(profile: &'static str, findings: Vec<Finding>) -> Self {
        let violations = findings
            .iter()
            .filter(|f| f.severity == FindingSeverity::Violation)
            .count();
        let warnings = findings
            .iter()
            .filter(|f| f.severity == FindingSeverity::Warning)
            .count();
        ContainerReport {
            profile,
            conforms: violations == 0,
            violations,
            warnings,
            findings,
        }
    }
}

pub trait ContainerProfile: Send + Sync {
    fn id(&self) -> &'static str;
    fn label(&self) -> &'static str;
    /// Does this archive look like one of ours?
    fn detect(&self, entries: &[Entry]) -> bool;
    /// Check an archive against the profile's rules.
    fn validate(&self, entries: &[Entry]) -> ContainerReport;
    fn read(&self, entries: &[Entry]) -> anyhow::Result<ContainerManifest>;
    /// The archive entries for a manifest (the caller zips them).
    fn write(&self, manifest: &ContainerManifest) -> anyhow::Result<Vec<Entry>>;
}

pub fn profiles() -> &'static [&'static dyn ContainerProfile] {
    static ICDD: icdd::Icdd = icdd::Icdd;
    static ALL: [&dyn ContainerProfile; 1] = [&ICDD];
    &ALL
}

pub fn profile(id: &str) -> Option<&'static dyn ContainerProfile> {
    profiles().iter().copied().find(|p| p.id() == id)
}

/// RDF format by file extension.
pub fn format_for(filename: &str) -> Option<RdfFormat> {
    let ext = filename.rsplit('.').next()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "ttl" => RdfFormat::Turtle,
        "nt" => RdfFormat::NTriples,
        "nq" => RdfFormat::NQuads,
        "trig" => RdfFormat::TriG,
        "rdf" | "owl" | "xml" => RdfFormat::RdfXml,
        "jsonld" | "json" => RdfFormat::from_media_type("application/ld+json")?,
        _ => return None,
    })
}

/// Unpack an archive (bomb-guarded).
#[cfg(feature = "asset-archive")]
pub fn unzip(bytes: &[u8]) -> anyhow::Result<Vec<Entry>> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))?;
    if archive.len() > MAX_ENTRIES {
        anyhow::bail!(
            "archive has {} entries (limit {MAX_ENTRIES})",
            archive.len()
        );
    }
    let mut total: u64 = 0;
    let mut out = Vec::with_capacity(archive.len());
    for i in 0..archive.len() {
        let mut f = archive.by_index(i)?;
        if f.is_dir() {
            // Kept (as a `/`-terminated name without bytes): a container's
            // layout may consist of empty folders.
            let name = f
                .name()
                .replace('\\', "/")
                .trim_start_matches("./")
                .to_string();
            out.push(Entry::dir(&name));
            continue;
        }
        if f.size() > MAX_ENTRY_BYTES {
            anyhow::bail!(
                "entry {} is {} bytes (limit {MAX_ENTRY_BYTES})",
                f.name(),
                f.size()
            );
        }
        total += f.size();
        if total > MAX_TOTAL_BYTES {
            anyhow::bail!("archive unpacks to more than {MAX_TOTAL_BYTES} bytes");
        }
        let mut buf = Vec::with_capacity(f.size() as usize);
        // The declared size is the archive's claim; never read past the cap.
        (&mut f).take(MAX_ENTRY_BYTES + 1).read_to_end(&mut buf)?;
        if buf.len() as u64 > MAX_ENTRY_BYTES {
            anyhow::bail!("entry {} is larger than {MAX_ENTRY_BYTES} bytes", f.name());
        }
        // Normalise separators; drop any leading "./".
        let name = f
            .name()
            .replace('\\', "/")
            .trim_start_matches("./")
            .to_string();
        out.push(Entry { name, bytes: buf });
    }
    Ok(out)
}

#[cfg(not(feature = "asset-archive"))]
pub fn unzip(_bytes: &[u8]) -> anyhow::Result<Vec<Entry>> {
    anyhow::bail!("container archives need the `asset-archive` build feature")
}

#[cfg(not(feature = "asset-archive"))]
pub fn zip_entries(_entries: &[Entry]) -> anyhow::Result<Vec<u8>> {
    anyhow::bail!("container archives need the `asset-archive` build feature")
}

/// Pack entries into a ZIP.
#[cfg(feature = "asset-archive")]
pub fn zip_entries(entries: &[Entry]) -> anyhow::Result<Vec<u8>> {
    use std::io::Write as _;
    let mut buf = Vec::new();
    {
        let mut w = zip::ZipWriter::new(Cursor::new(&mut buf));
        let opts: zip::write::FileOptions<'_, ()> =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for e in entries {
            if e.is_dir() {
                w.add_directory(e.name.trim_end_matches('/'), opts)?;
                continue;
            }
            w.start_file(&e.name, opts)?;
            w.write_all(&e.bytes)?;
        }
        w.finish()?;
    }
    Ok(buf)
}

fn sanitize_segment(s: &str) -> String {
    let base = s.rsplit('/').next().unwrap_or(s);
    let stem = base.rsplit_once('.').map(|(a, _)| a).unwrap_or(base);
    let cleaned: String = stem
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "payload".to_string()
    } else {
        cleaned
    }
}

// ── HTTP ────────────────────────────────────────────────────────────────────

type ApiErr = (StatusCode, String);

fn e500<E: std::fmt::Display>(e: E) -> ApiErr {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

fn dataset(state: &AppState, uid: Option<&str>, id: &str) -> Result<Dataset, ApiErr> {
    let ds = state
        .auth_db
        .get_dataset(id)
        .map_err(e500)?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Dataset not found".to_string()))?;
    if !state.auth_db.can_access_dataset(uid, &ds).map_err(e500)? {
        return Err((StatusCode::NOT_FOUND, "Dataset not found".to_string()));
    }
    Ok(ds)
}

/// May an imported payload be loaded into `iri` for `dataset_id`? Only a graph
/// in the dataset's own reserved namespace that no other dataset has registered.
/// Fails closed on a registry error.
fn payload_graph_allowed(state: &AppState, dataset_id: &str, iri: &str) -> bool {
    crate::auth::dataset_graph::dataset_owns_graph(&state.base_url, dataset_id, iri)
        && !state
            .auth_db
            .graph_has_other_dataset_refs(iri, dataset_id)
            .unwrap_or(true)
}

#[derive(Debug, Default, Deserialize)]
pub struct ProfileQuery {
    pub profile: Option<String>,
    /// Import only: refuse a container the profile's validator finds a
    /// violation in (422 with the report), instead of importing what it can.
    #[serde(default)]
    pub strict: Option<bool>,
}

fn unknown_profile(p: &str) -> ApiErr {
    (
        StatusCode::NOT_FOUND,
        format!(
            "unknown container profile `{p}`; known: {}",
            profiles()
                .iter()
                .map(|p| p.id())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    )
}

fn read_archive(body: &Bytes) -> Result<Vec<Entry>, ApiErr> {
    if body.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "empty body; send the container archive".to_string(),
        ));
    }
    unzip(body).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            format!("not a readable archive: {e}"),
        )
    })
}

/// The profile named by `?profile=`, else the one that recognises the
/// archive, else (`fallback`) the first profile.
fn resolve_profile(
    requested: Option<&str>,
    entries: &[Entry],
    fallback: bool,
) -> Result<&'static dyn ContainerProfile, ApiErr> {
    if let Some(p) = requested {
        return profile(p).ok_or_else(|| unknown_profile(p));
    }
    if let Some(p) = profiles().iter().copied().find(|p| p.detect(entries)) {
        return Ok(p);
    }
    if fallback {
        return Ok(profiles()[0]);
    }
    Err((
        StatusCode::UNPROCESSABLE_ENTITY,
        "no container profile recognises this archive (no index found)".to_string(),
    ))
}

async fn validate_blocking(
    prof: &'static dyn ContainerProfile,
    entries: std::sync::Arc<Vec<Entry>>,
) -> Result<ContainerReport, ApiErr> {
    tokio::task::spawn_blocking(move || prof.validate(&entries))
        .await
        .map_err(e500)
}

/// POST /api/containers/validate — validate an archive, store nothing.
pub async fn validate_container(
    Query(q): Query<ProfileQuery>,
    body: Bytes,
) -> Result<Json<ContainerReport>, ApiErr> {
    let entries = std::sync::Arc::new(read_archive(&body)?);
    let prof = resolve_profile(q.profile.as_deref(), &entries, true)?;
    Ok(Json(validate_blocking(prof, entries).await?))
}

/// One stored asset: its id, content type and URL.
struct StoredAsset {
    id: String,
    content_type: String,
    url: String,
}

#[allow(clippy::too_many_arguments)]
async fn store_asset(
    state: &AppState,
    ds: &Dataset,
    user_id: &str,
    folder: &str,
    file_name: &str,
    bytes: &[u8],
    declared_type: &str,
    title: Option<&str>,
    description: Option<&str>,
) -> Result<StoredAsset, ApiErr> {
    let base = state.base_url.trim_end_matches('/');
    let asset_id = uuid::Uuid::new_v4().to_string();
    let key = format!("datasets/{}/{asset_id}/{file_name}", ds.id);
    let ct = crate::assets::sniff_mime(bytes).unwrap_or_else(|| declared_type.to_string());
    state
        .object_store
        .upload(&key, Bytes::from(bytes.to_vec()), &ct)
        .await
        .map_err(e500)?;
    state
        .auth_db
        .create_asset(
            &asset_id,
            &ds.id,
            file_name,
            &ct,
            &key,
            bytes.len() as i64,
            user_id,
            ds.visibility == crate::auth::models::Visibility::Public,
            folder,
        )
        .map_err(e500)?;
    let title = title.filter(|t| *t != file_name);
    if title.is_some() || description.is_some() {
        let _ = state
            .auth_db
            .update_asset_metadata(&asset_id, title, description);
    }
    Ok(StoredAsset {
        url: format!("{base}/api/datasets/{}/assets/{asset_id}", ds.id),
        id: asset_id,
        content_type: ct,
    })
}

/// `dir/name` → (`dir`, `name`); a bare name has an empty dir.
fn split_path(p: &str) -> (&str, &str) {
    p.rsplit_once('/').unwrap_or(("", p))
}

/// The asset folder for a document path under the container's folder.
fn asset_folder(container_folder: &str, sub: &str, warnings: &mut Vec<String>) -> String {
    if sub.is_empty() {
        return container_folder.to_string();
    }
    match crate::assets::sanitize_folder_path(&format!("{container_folder}/{sub}")) {
        Ok(f) => f,
        Err(e) => {
            warnings.push(format!(
                "folder {sub:?} cannot be an asset folder ({e}); its files go to {container_folder}"
            ));
            container_folder.to_string()
        }
    }
}

fn doc_json(d: &DocumentEntry) -> serde_json::Value {
    let mut j = serde_json::json!({
        "iri": d.iri,
        "kind": d.kind,
        "filename": d.filename,
    });
    if let serde_json::Value::Object(meta) = serde_json::to_value(&d.meta).unwrap_or_default() {
        j.as_object_mut().unwrap().extend(meta);
    }
    if let Some(desc) = &d.description {
        j["description"] = desc.clone().into();
    }
    if let Some(s) = d.checksum_status {
        j["checksum_status"] = serde_json::to_value(s).unwrap_or_default();
    }
    if d.kind == DocumentKind::Encrypted {
        j["encrypted"] = true.into();
    }
    j
}

/// POST /api/datasets/:id/containers/import — body: the archive.
pub async fn import_container(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    Path(dataset_id): Path<String>,
    Query(q): Query<ProfileQuery>,
    body: Bytes,
) -> Result<Response, ApiErr> {
    use oxigraph::model::{Literal, NamedNode, Triple};
    let Some(Extension(user)) = user else {
        return Err((
            StatusCode::UNAUTHORIZED,
            "Authentication required".to_string(),
        ));
    };
    let ds = dataset(&state, Some(&user.user_id), &dataset_id)?;
    if !state
        .auth_db
        .can_write_dataset(&user.user_id, &ds)
        .map_err(e500)?
    {
        return Err((StatusCode::FORBIDDEN, "Write access required".to_string()));
    }
    let entries = std::sync::Arc::new(read_archive(&body)?);
    let prof = resolve_profile(q.profile.as_deref(), &entries, false)?;
    let report = validate_blocking(prof, entries.clone()).await?;
    if q.strict.unwrap_or(false) && !report.conforms {
        return Ok((
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({
                "error": format!(
                    "{}: the container has {} violation(s); nothing was imported (strict=true)",
                    prof.label(),
                    report.violations
                ),
                "validation": report,
            })),
        )
            .into_response());
    }
    let manifest = {
        let e = entries.clone();
        tokio::task::spawn_blocking(move || prof.read(&e))
            .await
            .map_err(e500)?
            .map_err(|e| {
                (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    format!("{}: {e}", prof.label()),
                )
            })?
    };
    drop(entries);

    let base = state.base_url.trim_end_matches('/').to_string();
    let cid = uuid::Uuid::new_v4().to_string();
    let container_ns = format!("{base}/dataset/{dataset_id}/container/{cid}");
    let folder = format!("containers/{cid}");
    let mut warnings = manifest.warnings.clone();
    let mut documents = Vec::new();
    let mut graphs = Vec::new();
    // Catalogue additions: what the import made of each document.
    let mut extra: Vec<Triple> = Vec::new();
    let ots = |l: &str| NamedNode::new_unchecked(format!("https://opentriplestore.org/ns#{l}"));

    // 1. Payload documents → the dataset's assets.
    let carries_files = manifest
        .documents
        .iter()
        .any(|d| d.bytes.is_some() || !d.files.is_empty());
    if carries_files && !state.object_store.is_configured() {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "the container carries documents but no object storage is configured (S3_* or a local store)".to_string(),
        ));
    }
    let _ = state.auth_db.create_asset_folder(&dataset_id, &folder);
    for d in &manifest.documents {
        let mut j = doc_json(d);
        let doc_node = NamedNode::new(d.iri.as_str()).ok();
        match d.kind {
            DocumentKind::External => {
                j["external_url"] = d.external_url.clone().into();
            }
            DocumentKind::Folder => {
                let sub = asset_folder(&folder, &d.filename, &mut warnings);
                let _ = state.auth_db.create_asset_folder(&dataset_id, &sub);
                let mut files = Vec::new();
                for f in &d.files {
                    let (dir, name) = split_path(&f.path);
                    let target = asset_folder(&sub, dir, &mut warnings);
                    let _ = state.auth_db.create_asset_folder(&dataset_id, &target);
                    let a = store_asset(
                        &state,
                        &ds,
                        &user.user_id,
                        &target,
                        name,
                        &f.bytes,
                        "application/octet-stream",
                        None,
                        None,
                    )
                    .await?;
                    files.push(serde_json::json!({
                        "path": f.path, "asset_id": a.id, "content_type": a.content_type,
                        "size_bytes": f.bytes.len(), "url": a.url,
                    }));
                }
                if let Some(n) = &doc_node {
                    extra.push(Triple::new(
                        n.clone(),
                        ots("assetFolder"),
                        Literal::new_simple_literal(&sub),
                    ));
                }
                j["asset_folder"] = sub.into();
                j["files"] = files.into();
            }
            DocumentKind::Internal | DocumentKind::Secured | DocumentKind::Encrypted => {
                let Some(bytes) = &d.bytes else {
                    documents.push(j);
                    continue;
                };
                let (dir, name) = split_path(&d.filename);
                let target = asset_folder(&folder, dir, &mut warnings);
                let _ = state.auth_db.create_asset_folder(&dataset_id, &target);
                let a = store_asset(
                    &state,
                    &ds,
                    &user.user_id,
                    &target,
                    name,
                    bytes,
                    &d.content_type,
                    d.meta.name.as_deref(),
                    d.description.as_deref(),
                )
                .await?;
                if let Some(n) = &doc_node {
                    extra.push(Triple::new(
                        n.clone(),
                        ots("downloadUrl"),
                        NamedNode::new_unchecked(&a.url),
                    ));
                    extra.push(Triple::new(
                        n.clone(),
                        ots("assetId"),
                        Literal::new_simple_literal(&a.id),
                    ));
                    if let Some(s) = d.checksum_status {
                        let s = serde_json::to_value(s)
                            .ok()
                            .and_then(|v| v.as_str().map(str::to_string))
                            .unwrap_or_default();
                        extra.push(Triple::new(
                            n.clone(),
                            ots("checksumStatus"),
                            Literal::new_simple_literal(s),
                        ));
                    }
                }
                j["asset_id"] = a.id.into();
                j["content_type"] = a.content_type.into();
                j["size_bytes"] = bytes.len().into();
                j["url"] = a.url.into();
                j["asset_folder"] = target.into();
            }
        }
        documents.push(j);
    }

    // 2. RDF payloads → role-typed graphs. The graph IRI an index gives a
    //    payload comes from an untrusted file: it is honoured only when it lies
    //    inside this dataset's own namespace and no other dataset has claimed
    //    it (admins included — the caller did not choose the IRI, the archive
    //    did). Anything else is re-homed under the container's namespace; the
    //    original IRI is kept as `ots:sourceIri` in the index graph.
    let mut sources: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut targets: Vec<String> = Vec::with_capacity(manifest.payloads.len());
    let mut minted_seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for p in &manifest.payloads {
        let mut minted = format!(
            "{container_ns}/{}/{}",
            match p.kind {
                PayloadKind::Linkset => "linkset",
                PayloadKind::Triples => "triples",
                PayloadKind::Ontology => "ontology",
            },
            sanitize_segment(&p.filename)
        );
        // Two payloads with one stem (links.rdf, links.ttl) get two graphs.
        let stem = minted.clone();
        let mut n = 2;
        while !minted_seen.insert(minted.clone()) {
            minted = format!("{stem}-{n}");
            n += 1;
        }
        let claimed = p
            .iri
            .as_deref()
            .filter(|i| oxigraph::model::NamedNode::new(*i).is_ok());
        let iri = match claimed {
            Some(i) if payload_graph_allowed(&state, &dataset_id, i) => i.to_string(),
            Some(i) => {
                warnings.push(format!(
                    "payload {}: graph <{i}> is outside dataset '{dataset_id}' or claimed by another dataset; stored as <{minted}>",
                    p.filename
                ));
                sources.insert(minted.clone(), i.to_string());
                minted
            }
            None => minted,
        };
        targets.push(iri);
    }
    let st = state.clone();
    let payloads: Vec<(RdfPayload, String)> =
        manifest.payloads.iter().cloned().zip(targets).collect();
    let loaded = tokio::task::spawn_blocking(
        move || -> Vec<Result<(String, PayloadKind, String, usize), String>> {
            payloads
                .iter()
                .map(|(p, iri)| {
                    st.store
                        .load_str(&p.text, p.format, Some(iri))
                        .map_err(|e| format!("{}: {e}", p.filename))?;
                    let n = st.store.graph_count_cached(Some(iri)).unwrap_or(0);
                    Ok((iri.clone(), p.kind, p.filename.clone(), n))
                })
                .collect()
        },
    )
    .await
    .map_err(e500)?;
    let mut graph_iris = Vec::new();
    for r in loaded {
        match r {
            Ok((iri, kind, file, n)) => {
                state
                    .auth_db
                    .add_dataset_graph(&dataset_id, &iri)
                    .map_err(e500)?;
                let _ = state
                    .auth_db
                    .set_dataset_graph_role(&dataset_id, &iri, Some(kind.role()));
                graphs.push(serde_json::json!({ "iri": iri, "role": kind.role().as_str(), "file": file, "triples": n }));
                graph_iris.push(iri);
            }
            Err(e) => warnings.push(format!("payload not loaded: {e}")),
        }
    }

    // 3. The index → the container's catalogue graph, with links to what was made of it.
    let index_graph = format!("{container_ns}/index");
    if let Some(fmt) = manifest.index_format {
        let st = state.clone();
        let text = manifest.index_text.clone();
        let g = index_graph.clone();
        tokio::task::spawn_blocking(move || st.store.load_str(&text, fmt, Some(&g)))
            .await
            .map_err(e500)?
            .map_err(|e| (StatusCode::UNPROCESSABLE_ENTITY, format!("index: {e}")))?;
    }
    if let Ok(c) = NamedNode::new(manifest.iri.as_str()) {
        extra.push(Triple::new(
            c.clone(),
            ots("importedInto"),
            NamedNode::new_unchecked(format!("{base}/dataset/{dataset_id}")),
        ));
        extra.push(Triple::new(
            c.clone(),
            ots("containerProfile"),
            Literal::new_simple_literal(prof.id()),
        ));
        extra.push(Triple::new(
            c.clone(),
            ots("containerId"),
            Literal::new_simple_literal(&cid),
        ));
        for g in &graphs {
            if let (Some(iri), Some(role)) = (g["iri"].as_str(), g["role"].as_str()) {
                let gn = NamedNode::new_unchecked(iri);
                extra.push(Triple::new(gn.clone(), ots("partOfContainer"), c.clone()));
                extra.push(Triple::new(
                    gn.clone(),
                    ots("graphRole"),
                    Literal::new_simple_literal(role),
                ));
                if let Some(src) = sources
                    .get(iri)
                    .and_then(|s| NamedNode::new(s.as_str()).ok())
                {
                    extra.push(Triple::new(gn, ots("sourceIri"), src));
                }
            }
        }
    }
    let extra_nt: String = extra.iter().map(|t| format!("{t} .\n")).collect();
    state
        .store
        .load_str(&extra_nt, RdfFormat::NTriples, Some(&index_graph))
        .map_err(e500)?;
    state
        .auth_db
        .add_dataset_graph(&dataset_id, &index_graph)
        .map_err(e500)?;
    let _ =
        state
            .auth_db
            .set_dataset_graph_role(&dataset_id, &index_graph, Some(GraphKind::Catalog));
    graph_iris.push(index_graph.clone());

    // 4. Bookkeeping like any other write.
    for g in &graph_iris {
        crate::server::routes::sync_text_index_after_graph_write(&state, Some(g.clone())).await;
    }
    {
        let st = state.clone();
        let gs = graph_iris.clone();
        let id = dataset_id.clone();
        let _ = tokio::task::spawn_blocking(move || {
            crate::ldes::capture::publish_all(&st, &id, &gs);
            crate::entailment::after_write(&st, &gs);
        })
        .await;
    }
    let total: usize = graphs
        .iter()
        .filter_map(|g| g["triples"].as_u64())
        .sum::<u64>() as usize;
    crate::commit_log::record(
        &state.store,
        &state.base_url,
        crate::commit_log::CommitKind::Import,
        format!(
            "{} container import: {} documents, {} graphs",
            prof.label(),
            documents.len(),
            graphs.len()
        ),
        Some(&user.user_id),
        Some(format!("{base}/dataset/{dataset_id}")),
        graph_iris.clone(),
        total,
        0,
        None,
    );
    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "container_id": cid,
            "container": manifest.iri,
            "profile": prof.id(),
            "title": manifest.title,
            "description": manifest.description,
            "conformance": manifest.conformance,
            "created_by": manifest.created_by,
            "published_by": manifest.published_by,
            "creation_date": manifest.creation_date,
            "version_id": manifest.version_id,
            "version_description": manifest.version_description,
            "index_graph": index_graph,
            "documents": documents,
            "graphs": graphs,
            "standard_ontologies": manifest.standard_ontologies,
            "warnings": warnings,
            "validation": report,
        })),
    )
        .into_response())
}

/// What earlier imports recorded about their documents, read back from the
/// dataset's catalogue graphs the caller may read.
#[derive(Default)]
struct Recovered {
    by_asset: std::collections::HashMap<String, icdd::IndexDoc>,
    folders: Vec<icdd::IndexDoc>,
    external: Vec<icdd::IndexDoc>,
}

fn recover_index_records(
    state: &AppState,
    graphs: &[&crate::auth::models::DatasetGraphEntry],
) -> Recovered {
    let mut out = Recovered::default();
    let mut seen = std::collections::HashSet::new();
    for e in graphs {
        let Ok(nt) = state.store.dump(RdfFormat::NTriples, Some(&e.graph_iri)) else {
            continue;
        };
        let Ok(tmp) = oxigraph::store::Store::new() else {
            continue;
        };
        if tmp
            .load_from_reader(
                oxigraph::io::RdfParser::from_format(RdfFormat::NTriples),
                nt.as_slice(),
            )
            .is_err()
        {
            continue;
        }
        for c in icdd::container_nodes(&tmp) {
            for d in icdd::index_documents(&tmp, c.as_ref()) {
                if !seen.insert(d.iri.clone()) {
                    continue;
                }
                let asset = d.asset_id.clone().or_else(|| {
                    d.download_url
                        .as_deref()
                        .and_then(|u| u.rsplit_once("/assets/"))
                        .map(|(_, id)| id.to_string())
                });
                match d.kind {
                    Some(DocumentKind::External) => out.external.push(d),
                    Some(DocumentKind::Folder) if d.asset_folder.is_some() => out.folders.push(d),
                    _ => {
                        if let Some(a) = asset {
                            out.by_asset.insert(a, d);
                        }
                    }
                }
            }
        }
    }
    out
}

/// GET /api/datasets/:id/containers/export?profile=icdd — the dataset as a container.
pub async fn export_container(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    Path(dataset_id): Path<String>,
    Query(q): Query<ProfileQuery>,
) -> Result<Response, ApiErr> {
    let uid = user.as_ref().map(|Extension(u)| u.user_id.as_str());
    let ds = dataset(&state, uid, &dataset_id)?;
    let pid = q.profile.as_deref().unwrap_or("icdd");
    let prof = profile(pid).ok_or_else(|| unknown_profile(pid))?;
    let base = state.base_url.trim_end_matches('/').to_string();
    let container_iri = format!("{base}/dataset/{dataset_id}/container");

    // A graph marked private is only visible to principals who can write the
    // dataset — a viewer's (or anonymous) export must not carry it, nor the
    // document metadata of a private catalogue graph.
    let can_write = match uid {
        Some(u) => state.auth_db.can_write_dataset(u, &ds).map_err(e500)?,
        None => false,
    };
    let graph_entries = state
        .auth_db
        .list_dataset_graph_entries(&dataset_id)
        .map_err(e500)?;
    let readable: Vec<&crate::auth::models::DatasetGraphEntry> = graph_entries
        .iter()
        .filter(|e| {
            (can_write || !e.private)
                && !e.graph_iri.starts_with("urn:system:")
                && !e.graph_iri.starts_with("urn:ots:")
        })
        .collect();
    let catalogues: Vec<&crate::auth::models::DatasetGraphEntry> = readable
        .iter()
        .copied()
        .filter(|e| e.graph_role == Some(GraphKind::Catalog))
        .collect();
    let recovered = {
        let st = state.clone();
        let cats: Vec<crate::auth::models::DatasetGraphEntry> =
            catalogues.iter().map(|e| (*e).clone()).collect();
        tokio::task::spawn_blocking(move || {
            let refs: Vec<&crate::auth::models::DatasetGraphEntry> = cats.iter().collect();
            recover_index_records(&st, &refs)
        })
        .await
        .map_err(e500)?
    };

    // Documents: the dataset's assets the caller may read. `Asset.public` gates
    // anonymity, not membership — a logged-out caller (only reachable here on a
    // public dataset) gets the public files; any authenticated dataset-reader
    // gets them all. This mirrors `list_assets` / `serve_asset`; without it a
    // hidden (non-public) asset's bytes would be zipped into an anonymous export.
    let anonymous = uid.is_none();
    let mut documents = Vec::new();
    let mut folder_files: Vec<Vec<FolderFile>> = vec![Vec::new(); recovered.folders.len()];
    for a in state
        .auth_db
        .list_dataset_assets(&dataset_id)
        .map_err(e500)?
        .into_iter()
        .filter(|a| !anonymous || a.public)
    {
        let (bytes, ct) = match state.object_store.download(&a.s3_key).await {
            Ok(x) => x,
            Err(e) => {
                tracing::warn!("container export: asset {} unreadable: {e}", a.id);
                continue;
            }
        };
        // Inside a folder document an earlier import made?
        if let Some((i, af)) = recovered.folders.iter().enumerate().find_map(|(i, f)| {
            let af = f.asset_folder.as_deref()?;
            (a.folder == af || a.folder.starts_with(&format!("{af}/"))).then_some((i, af))
        }) {
            let rel_dir = a.folder[af.len()..].trim_start_matches('/');
            folder_files[i].push(FolderFile {
                path: if rel_dir.is_empty() {
                    a.filename.clone()
                } else {
                    format!("{rel_dir}/{}", a.filename)
                },
                bytes: bytes.to_vec(),
            });
            continue;
        }
        let content_type = if ct.is_empty() {
            a.content_type.clone()
        } else {
            ct
        };
        let path = if a.folder.is_empty() {
            a.filename.clone()
        } else {
            format!("{}/{}", a.folder, a.filename)
        };
        let doc = match recovered.by_asset.get(&a.id) {
            // Imported from a container: its index metadata goes back out.
            Some(rec) => {
                let mut meta = rec.meta.clone();
                if a.title.is_some() {
                    meta.name = a.title.clone();
                }
                DocumentEntry {
                    iri: if rec.iri.starts_with("_:") {
                        format!("{base}/api/datasets/{dataset_id}/assets/{}", a.id)
                    } else {
                        rec.iri.clone()
                    },
                    kind: match rec.kind {
                        Some(k @ (DocumentKind::Secured | DocumentKind::Encrypted)) => k,
                        _ => DocumentKind::Internal,
                    },
                    filename: rec.filename.clone().unwrap_or(path),
                    content_type,
                    description: a.description.clone().or_else(|| rec.description.clone()),
                    bytes: Some(bytes.to_vec()),
                    meta,
                    ..Default::default()
                }
            }
            None => DocumentEntry {
                iri: format!("{base}/api/datasets/{dataset_id}/assets/{}", a.id),
                kind: DocumentKind::Internal,
                filename: path,
                content_type,
                description: a.description.clone(),
                bytes: Some(bytes.to_vec()),
                meta: DocumentMeta {
                    name: Some(a.title.clone().unwrap_or_else(|| a.filename.clone())),
                    ..Default::default()
                },
                ..Default::default()
            },
        };
        documents.push(doc);
    }
    for (rec, files) in recovered.folders.iter().zip(folder_files) {
        if files.is_empty() {
            continue;
        }
        documents.push(DocumentEntry {
            iri: rec.iri.clone(),
            kind: DocumentKind::Folder,
            filename: rec.foldername.clone().unwrap_or_else(|| "folder".into()),
            description: rec.description.clone(),
            files,
            meta: rec.meta.clone(),
            ..Default::default()
        });
    }
    for rec in &recovered.external {
        documents.push(DocumentEntry {
            iri: rec.iri.clone(),
            kind: DocumentKind::External,
            filename: String::new(),
            description: rec.description.clone(),
            external_url: rec.url.clone(),
            meta: rec.meta.clone(),
            ..Default::default()
        });
    }

    // RDF payloads: every readable graph by role, in RDF/XML where the graph
    // can be written in it (catalogue graphs of earlier imports are skipped —
    // their document metadata is in the index above).
    let mut payloads = Vec::new();
    for e in &readable {
        let kind = match e.graph_role {
            Some(GraphKind::Linkset) => PayloadKind::Linkset,
            Some(
                GraphKind::Model
                | GraphKind::Vocabulary
                | GraphKind::Shapes
                | GraphKind::DomainValues,
            ) => PayloadKind::Ontology,
            Some(
                GraphKind::Catalog
                | GraphKind::Provenance
                | GraphKind::Entailment
                | GraphKind::System,
            ) => continue,
            _ => PayloadKind::Triples,
        };
        let (bytes, format, ext) = match state.store.dump(RdfFormat::RdfXml, Some(&e.graph_iri)) {
            Ok(b) => (b, RdfFormat::RdfXml, "rdf"),
            Err(err) => {
                tracing::info!(
                    "container export: <{}> has no RDF/XML form ({err}); written as Turtle",
                    e.graph_iri
                );
                (
                    state
                        .store
                        .dump(RdfFormat::Turtle, Some(&e.graph_iri))
                        .map_err(e500)?,
                    RdfFormat::Turtle,
                    "ttl",
                )
            }
        };
        payloads.push(RdfPayload {
            iri: Some(e.graph_iri.clone()),
            filename: format!("{}.{ext}", sanitize_segment(&e.graph_iri)),
            kind,
            format,
            text: String::from_utf8_lossy(&bytes).into_owned(),
        });
    }

    let owner = owner_party(&state, &base, &ds);
    let ontology_resources = if prof.id() == "icdd" {
        let (files, notes) = icdd::operator_ontologies();
        for n in notes {
            tracing::warn!("container export: {n}");
        }
        files
    } else {
        Vec::new()
    };
    let manifest = ContainerManifest {
        iri: container_iri,
        title: Some(ds.name.clone()),
        description: ds.description.clone(),
        created_by: Some(owner.clone()),
        published_by: Some(owner),
        documents,
        payloads,
        ontology_resources,
        ..Default::default()
    };
    let entries = std::sync::Arc::new(prof.write(&manifest).map_err(e500)?);
    let report = validate_blocking(prof, entries.clone()).await?;
    let bytes = zip_entries(&entries).map_err(e500)?;
    let mut resp = (StatusCode::OK, bytes).into_response();
    let h = resp.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/zip"),
    );
    let ext = if prof.id() == "icdd" { "icdd" } else { "zip" };
    h.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!("attachment; filename=\"{dataset_id}.{ext}\""))
            .map_err(e500)?,
    );
    // The export's own validation, in brief (POST it to /api/containers/validate
    // for the full report). Without the operator's ISO ontology files it carries
    // the `ontology-resource-missing` warning: not Part 1-conformant.
    h.insert(
        "x-container-conforms",
        HeaderValue::from_static(if report.conforms { "true" } else { "false" }),
    );
    h.insert(
        "x-container-validation",
        HeaderValue::from_str(&format!(
            "violations={}; warnings={}",
            report.violations, report.warnings
        ))
        .map_err(e500)?,
    );
    let mut codes: Vec<&str> = report
        .findings
        .iter()
        .filter(|f| f.severity != FindingSeverity::Info)
        .map(|f| f.code.as_str())
        .collect();
    codes.sort_unstable();
    codes.dedup();
    if !codes.is_empty() {
        if let Ok(v) = HeaderValue::from_str(&codes.join(", ")) {
            h.insert("x-container-findings", v);
        }
    }
    Ok(resp)
}

/// The dataset's owner as an ICDD party: a person for a user, an
/// organisation for an organisation or group, named by its public name.
fn owner_party(state: &AppState, base: &str, ds: &Dataset) -> Party {
    use crate::auth::models::OwnerType;
    let iri = crate::provenance::owner_iri(base, ds);
    let (kind, name) = match ds.owner_type {
        OwnerType::User => (
            "person",
            state
                .auth_db
                .get_user_by_id(&ds.owner_id)
                .ok()
                .flatten()
                .map(|u| u.username),
        ),
        OwnerType::Organisation => (
            "organisation",
            state
                .auth_db
                .get_organisation(&ds.owner_id)
                .ok()
                .flatten()
                .map(|o| o.name),
        ),
        OwnerType::Group => (
            "organisation",
            state
                .auth_db
                .get_group(&ds.owner_id)
                .ok()
                .flatten()
                .map(|g| g.name),
        ),
    };
    Party {
        iri: Some(iri),
        kind: kind.to_string(),
        name: Some(name.unwrap_or_else(|| ds.owner_id.clone())),
    }
}
