//! Linked-document containers: a packaged archive of payload documents, RDF
//! payloads, link graphs and an index describing them — imported into a
//! dataset (documents → the dataset's assets, RDF → role-typed graphs, the
//! index → a catalogue graph) and exported from one.
//!
//! The mechanism is profile-neutral: [`ContainerProfile`] reads an archive
//! into a [`ContainerManifest`] and writes one back. ISO 21597-1 **ICDD**
//! ([`icdd`]) is the first profile. Needs the `asset-archive` feature (ZIP).
//!
//! * `POST /api/datasets/:id/containers/import?profile=icdd` — body: the ZIP
//! * `GET  /api/datasets/:id/containers/export?profile=icdd` — a ZIP download

pub mod icdd;

#[cfg(feature = "asset-archive")]
use std::io::{Cursor, Read};

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use oxigraph::io::RdfFormat;
use serde::Deserialize;

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

#[derive(Debug, Clone)]
pub struct DocumentEntry {
    /// The document's IRI in the index.
    pub iri: String,
    pub filename: String,
    pub content_type: String,
    pub description: Option<String>,
    /// An external document: a URL instead of bytes.
    pub external_url: Option<String>,
    pub bytes: Option<Vec<u8>>,
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
#[derive(Debug, Clone)]
pub struct ContainerManifest {
    /// The container description's IRI.
    pub iri: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub created_by: Option<String>,
    pub published_by: Option<String>,
    pub conformance: Vec<String>,
    pub documents: Vec<DocumentEntry>,
    pub payloads: Vec<RdfPayload>,
    /// The index document itself, to keep as the catalogue graph.
    pub index_text: String,
    pub index_format: Option<RdfFormat>,
    pub warnings: Vec<String>,
}

/// One archive entry.
pub struct Entry {
    pub name: String,
    pub bytes: Vec<u8>,
}

pub trait ContainerProfile: Send + Sync {
    fn id(&self) -> &'static str;
    fn label(&self) -> &'static str;
    /// Does this archive look like one of ours?
    fn detect(&self, entries: &[Entry]) -> bool;
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
        f.read_to_end(&mut buf)?;
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

#[derive(Debug, Default, Deserialize)]
pub struct ProfileQuery {
    pub profile: Option<String>,
}

/// POST /api/datasets/:id/containers/import — body: the archive.
pub async fn import_container(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(dataset_id): Path<String>,
    Query(q): Query<ProfileQuery>,
    body: Bytes,
) -> Result<impl IntoResponse, ApiErr> {
    let ds = dataset(&state, Some(&user.user_id), &dataset_id)?;
    if !state
        .auth_db
        .can_write_dataset(&user.user_id, &ds)
        .map_err(e500)?
    {
        return Err((StatusCode::FORBIDDEN, "Write access required".to_string()));
    }
    if body.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "empty body; send the container archive".to_string(),
        ));
    }
    let entries = unzip(&body).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            format!("not a readable archive: {e}"),
        )
    })?;
    let prof: &dyn ContainerProfile = match q.profile.as_deref() {
        Some(p) => profile(p).ok_or_else(|| {
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
        })?,
        None => profiles()
            .iter()
            .copied()
            .find(|p| p.detect(&entries))
            .ok_or_else(|| {
                (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "no container profile recognises this archive (no index found)".to_string(),
                )
            })?,
    };
    let manifest = prof.read(&entries).map_err(|e| {
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("{}: {e}", prof.label()),
        )
    })?;

    let base = state.base_url.trim_end_matches('/').to_string();
    let cid = uuid::Uuid::new_v4().to_string();
    let container_ns = format!("{base}/dataset/{dataset_id}/container/{cid}");
    let folder = format!("containers/{cid}");
    let mut warnings = manifest.warnings.clone();
    let mut documents = Vec::new();
    let mut graphs = Vec::new();

    // 1. Payload documents → the dataset's assets.
    if manifest.documents.iter().any(|d| d.bytes.is_some()) && !state.object_store.is_configured() {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "the container carries documents but no object storage is configured (S3_* or a local store)".to_string(),
        ));
    }
    let _ = state.auth_db.create_asset_folder(&dataset_id, &folder);
    for d in &manifest.documents {
        let Some(bytes) = &d.bytes else {
            documents.push(serde_json::json!({ "iri": d.iri, "filename": d.filename, "external_url": d.external_url }));
            continue;
        };
        let asset_id = uuid::Uuid::new_v4().to_string();
        let file_name = d
            .filename
            .rsplit('/')
            .next()
            .unwrap_or(&d.filename)
            .to_string();
        let key = format!("datasets/{dataset_id}/{asset_id}/{file_name}");
        let ct = crate::assets::sniff_mime(bytes).unwrap_or_else(|| d.content_type.clone());
        state
            .object_store
            .upload(&key, Bytes::from(bytes.clone()), &ct)
            .await
            .map_err(e500)?;
        state
            .auth_db
            .create_asset(
                &asset_id,
                &dataset_id,
                &file_name,
                &ct,
                &key,
                bytes.len() as i64,
                &user.user_id,
                ds.visibility == crate::auth::models::Visibility::Public,
                &folder,
            )
            .map_err(e500)?;
        documents.push(serde_json::json!({
            "iri": d.iri, "filename": d.filename, "asset_id": asset_id, "content_type": ct, "size_bytes": bytes.len(),
            "url": format!("{base}/api/datasets/{dataset_id}/assets/{asset_id}"),
        }));
    }

    // 2. RDF payloads → role-typed graphs.
    let st = state.clone();
    let payloads = manifest.payloads.clone();
    let ns = container_ns.clone();
    let loaded = tokio::task::spawn_blocking(
        move || -> Vec<Result<(String, PayloadKind, String, usize), String>> {
            payloads
                .iter()
                .map(|p| {
                    let iri = match &p.iri {
                        Some(i) if oxigraph::model::NamedNode::new(i).is_ok() => i.clone(),
                        _ => format!(
                            "{ns}/{}/{}",
                            match p.kind {
                                PayloadKind::Linkset => "linkset",
                                PayloadKind::Triples => "triples",
                                PayloadKind::Ontology => "ontology",
                            },
                            sanitize_segment(&p.filename)
                        ),
                    };
                    st.store
                        .load_str(&p.text, p.format, Some(&iri))
                        .map_err(|e| format!("{}: {e}", p.filename))?;
                    let n = st.store.graph_count_cached(Some(&iri)).unwrap_or(0);
                    Ok((iri, p.kind, p.filename.clone(), n))
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
    let mut extra = format!(
        "<{}> <https://opentriplestore.org/ns#importedInto> <{base}/dataset/{dataset_id}> ; <https://opentriplestore.org/ns#containerProfile> \"{}\" ; <https://opentriplestore.org/ns#containerId> \"{cid}\" .\n",
        manifest.iri, prof.id()
    );
    for d in &documents {
        if let (Some(iri), Some(url)) = (d["iri"].as_str(), d["url"].as_str()) {
            if oxigraph::model::NamedNode::new(iri).is_ok() {
                extra.push_str(&format!(
                    "<{iri}> <https://opentriplestore.org/ns#downloadUrl> <{url}> .\n"
                ));
            }
        }
    }
    for g in &graphs {
        if let (Some(iri), Some(role)) = (g["iri"].as_str(), g["role"].as_str()) {
            extra.push_str(&format!(
                "<{iri}> <https://opentriplestore.org/ns#partOfContainer> <{}> ; <https://opentriplestore.org/ns#graphRole> \"{role}\" .\n",
                manifest.iri
            ));
        }
    }
    state
        .store
        .load_str(&extra, RdfFormat::Turtle, Some(&index_graph))
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
            "index_graph": index_graph,
            "documents": documents,
            "graphs": graphs,
            "warnings": warnings,
        })),
    ))
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
    let prof = profile(pid).ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            format!("unknown container profile `{pid}`"),
        )
    })?;
    let base = state.base_url.trim_end_matches('/').to_string();

    // Documents: every asset of the dataset.
    let mut documents = Vec::new();
    for a in state
        .auth_db
        .list_dataset_assets(&dataset_id)
        .map_err(e500)?
    {
        match state.object_store.download(&a.s3_key).await {
            Ok((bytes, ct)) => documents.push(DocumentEntry {
                iri: format!("{base}/api/datasets/{dataset_id}/assets/{}", a.id),
                filename: if a.folder.is_empty() {
                    a.filename.clone()
                } else {
                    format!("{}/{}", a.folder, a.filename)
                },
                content_type: if ct.is_empty() {
                    a.content_type.clone()
                } else {
                    ct
                },
                description: a.description.clone(),
                external_url: None,
                bytes: Some(bytes.to_vec()),
            }),
            Err(e) => tracing::warn!("container export: asset {} unreadable: {e}", a.id),
        }
    }
    // RDF payloads: every graph by role (catalogue graphs of earlier imports are skipped).
    let entries = state
        .auth_db
        .list_dataset_graph_entries(&dataset_id)
        .map_err(e500)?;
    let mut payloads = Vec::new();
    for e in entries
        .iter()
        .filter(|e| !e.graph_iri.starts_with("urn:system:") && !e.graph_iri.starts_with("urn:ots:"))
    {
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
        let text = state
            .store
            .dump(RdfFormat::Turtle, Some(&e.graph_iri))
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .map_err(e500)?;
        payloads.push(RdfPayload {
            iri: Some(e.graph_iri.clone()),
            filename: format!("{}.ttl", sanitize_segment(&e.graph_iri)),
            kind,
            format: RdfFormat::Turtle,
            text,
        });
    }
    let owner = crate::provenance::owner_iri(&base, &ds);
    let manifest = ContainerManifest {
        iri: format!("{base}/dataset/{dataset_id}/container"),
        title: Some(ds.name.clone()),
        description: ds.description.clone(),
        created_by: Some(owner.clone()),
        published_by: Some(owner),
        conformance: Vec::new(),
        documents,
        payloads,
        index_text: String::new(),
        index_format: None,
        warnings: Vec::new(),
    };
    let entries = prof.write(&manifest).map_err(e500)?;
    let bytes = zip_entries(&entries).map_err(e500)?;
    let mut resp = (StatusCode::OK, bytes).into_response();
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/zip"),
    );
    resp.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!("attachment; filename=\"{dataset_id}-{pid}.zip\""))
            .map_err(e500)?,
    );
    Ok(resp)
}
