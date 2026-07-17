//! `manifest.toml` parsing for on-disk seed bundles.
//!
//! A bundle directory looks like:
//!
//! ```text
//! my-bundle/
//! ├── manifest.toml
//! ├── model.ttl          # per-graph payload (Turtle/N-Triples/RDF-XML/JSON-LD)
//! └── instances.trig     # multi-graph payload (TriG/N-Quads)
//! ```
//!
//! and the manifest:
//!
//! ```toml
//! id = "my-bundle"
//! # optional — defaults to SEED_BUNDLE_MY_BUNDLE; set it to false/0/no/off to skip
//! opt_out_env = "SEED_MY_BUNDLE"
//!
//! [organisation]
//! slug = "my-org"
//! name = "My Organisation"
//! description = "Owns the seeded datasets."
//!
//! [[datasets]]
//! slug = "my-dataset"
//! name = "My Dataset"
//! description = "…"
//! visibility = "public"            # public | members | private
//!
//! [[datasets.graphs]]
//! iri = "https://example.org/my/model"
//! role = "model"                   # instances | model | vocabulary | shapes | …
//! file = "model.ttl"               # omit for graphs fed by a quads payload
//!
//! [[datasets.quads]]
//! file = "instances.trig"          # graphs are declared inside the file
//!
//! [[datasets.saved_queries]]
//! name = "All statements"
//! slug = "all-statements"
//! description = "…"
//! sparql = "SELECT ?s ?p ?o WHERE { ?s ?p ?o } LIMIT 100"
//! ```

use std::borrow::Cow;
use std::path::{Component, Path, PathBuf};

use anyhow::{bail, Context};
use oxigraph::io::RdfFormat;
use serde::Deserialize;

use crate::auth::models::{GraphKind, Visibility};
use crate::saved_queries::models::CreateSavedQueryRequest;

use super::{Bundle, BundleDataset, BundleGraph, Fmt, OrgSpec, QuadsPayload};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestDoc {
    id: String,
    #[serde(default)]
    opt_out_env: Option<String>,
    organisation: OrgDoc,
    #[serde(default)]
    datasets: Vec<DatasetDoc>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OrgDoc {
    slug: String,
    name: String,
    #[serde(default)]
    description: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DatasetDoc {
    slug: String,
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    visibility: Option<String>,
    #[serde(default)]
    graphs: Vec<GraphDoc>,
    #[serde(default)]
    quads: Vec<QuadsDoc>,
    #[serde(default)]
    saved_queries: Vec<SavedQueryDoc>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GraphDoc {
    iri: String,
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    file: Option<String>,
    /// Explicit format override; normally inferred from the file extension.
    #[serde(default)]
    format: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct QuadsDoc {
    file: String,
    #[serde(default)]
    format: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedQueryDoc {
    name: String,
    #[serde(default)]
    slug: Option<String>,
    #[serde(default)]
    description: Option<String>,
    sparql: String,
}

/// Parse `dir/manifest.toml` and read its payload files into a [`Bundle`].
pub fn parse_bundle(dir: &Path) -> anyhow::Result<Bundle> {
    let manifest_path = dir.join("manifest.toml");
    let raw = std::fs::read_to_string(&manifest_path)
        .with_context(|| format!("reading {manifest_path:?}"))?;
    let doc: ManifestDoc =
        toml::from_str(&raw).with_context(|| format!("parsing {manifest_path:?}"))?;

    if doc.id.trim().is_empty() {
        bail!("manifest `id` must not be empty");
    }

    let mut datasets = Vec::with_capacity(doc.datasets.len());
    for ds in doc.datasets {
        if ds.slug.trim().is_empty() {
            bail!("dataset slug must not be empty");
        }
        let visibility = match ds.visibility.as_deref() {
            None => Visibility::Public,
            Some(v) => Visibility::from_str(v)
                .with_context(|| format!("dataset '{}': unknown visibility '{v}'", ds.slug))?,
        };

        let mut graphs = Vec::with_capacity(ds.graphs.len());
        for g in ds.graphs {
            let role = match g.role.as_deref() {
                None => None,
                Some(r) => Some(
                    GraphKind::from_str(r)
                        .with_context(|| format!("graph <{}>: unknown role '{r}'", g.iri))?,
                ),
            };
            let data = match g.file.as_deref() {
                None => None,
                Some(file) => {
                    let path = resolve_payload(dir, file)?;
                    let fmt = graph_fmt(&path, g.format.as_deref())?;
                    let data = std::fs::read_to_string(&path)
                        .with_context(|| format!("reading payload {path:?}"))?;
                    Some((Cow::Owned(data), fmt))
                }
            };
            graphs.push(BundleGraph {
                iri: g.iri,
                role,
                data,
            });
        }

        let mut quads = Vec::with_capacity(ds.quads.len());
        for q in ds.quads {
            let path = resolve_payload(dir, &q.file)?;
            let format = quads_format(&path, q.format.as_deref())?;
            let data = std::fs::read_to_string(&path)
                .with_context(|| format!("reading payload {path:?}"))?;
            quads.push(QuadsPayload {
                label: q.file,
                data,
                format,
            });
        }

        let saved_queries = ds
            .saved_queries
            .into_iter()
            .map(|s| CreateSavedQueryRequest {
                name: s.name,
                slug: s.slug,
                description: s.description,
                sparql: s.sparql,
                parameters: Vec::new(),
                test_parameters: Some(serde_json::json!({})),
                visibility: None,
                version_name: None,
                note: None,
            })
            .collect();

        datasets.push(BundleDataset {
            slug: ds.slug,
            name: ds.name,
            description: ds.description,
            visibility,
            graphs,
            quads,
            saved_queries,
        });
    }

    Ok(Bundle {
        id: doc.id,
        opt_out_env: doc.opt_out_env,
        org: OrgSpec {
            slug: doc.organisation.slug,
            name: doc.organisation.name,
            description: doc.organisation.description,
        },
        datasets,
    })
}

/// Resolve a payload path relative to the bundle directory, rejecting absolute
/// paths and `..` components so a manifest cannot read files outside its own
/// bundle (a seed dir may be operator-writable but the manifest untrusted).
fn resolve_payload(dir: &Path, file: &str) -> anyhow::Result<PathBuf> {
    let rel = Path::new(file);
    if rel.is_absolute() || rel.components().any(|c| !matches!(c, Component::Normal(_))) {
        bail!("payload path '{file}' must be a plain relative path inside the bundle");
    }
    Ok(dir.join(rel))
}

/// Per-graph payload format, from an explicit override or the file extension.
fn graph_fmt(path: &Path, explicit: Option<&str>) -> anyhow::Result<Fmt> {
    let key = match explicit {
        Some(f) => f.to_ascii_lowercase(),
        None => path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase(),
    };
    match key.as_str() {
        "ttl" | "turtle" => Ok(Fmt::Turtle),
        "nt" | "ntriples" => Ok(Fmt::NTriples),
        "rdf" | "xml" | "owl" | "rdfxml" => Ok(Fmt::RdfXml),
        "jsonld" | "json" => Ok(Fmt::JsonLd),
        other => bail!(
            "payload {path:?}: unsupported graph format '{other}' \
             (expected turtle/ntriples/rdfxml/jsonld; use [[datasets.quads]] for trig/nquads)"
        ),
    }
}

/// Multi-graph payload format (graphs declared in-file).
fn quads_format(path: &Path, explicit: Option<&str>) -> anyhow::Result<RdfFormat> {
    let key = match explicit {
        Some(f) => f.to_ascii_lowercase(),
        None => path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase(),
    };
    match key.as_str() {
        "trig" => Ok(RdfFormat::TriG),
        "nq" | "nquads" => Ok(RdfFormat::NQuads),
        other => {
            bail!("payload {path:?}: unsupported quads format '{other}' (expected trig/nquads)")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_path_traversal_in_payload_paths() {
        let dir = Path::new("/tmp/bundle");
        assert!(resolve_payload(dir, "model.ttl").is_ok());
        assert!(resolve_payload(dir, "sub/model.ttl").is_ok());
        assert!(resolve_payload(dir, "../outside.ttl").is_err());
        assert!(resolve_payload(dir, "/etc/passwd").is_err());
    }

    #[test]
    fn format_detection_from_extension_and_override() {
        let p = Path::new("x.ttl");
        assert_eq!(graph_fmt(p, None).unwrap(), Fmt::Turtle);
        assert_eq!(graph_fmt(p, Some("jsonld")).unwrap(), Fmt::JsonLd);
        assert!(graph_fmt(Path::new("x.trig"), None).is_err());
        assert!(matches!(
            quads_format(Path::new("x.trig"), None).unwrap(),
            RdfFormat::TriG
        ));
        assert!(quads_format(Path::new("x.ttl"), None).is_err());
    }
}
