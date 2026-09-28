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
//! role = "model"                   # instances | model (ontology) | vocabulary | shapes |
//!                                  # domain-values | linkset | provenance | catalog
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

use super::{
    Bundle, BundleDataModel, BundleDataset, BundleGraph, BundleLicense, Fmt, OrgSpec, QuadsPayload,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestDoc {
    id: String,
    #[serde(default)]
    opt_out_env: Option<String>,
    organisation: OrgDoc,
    #[serde(default)]
    datasets: Vec<DatasetDoc>,
    /// Reference models (RDFS/OWL classes, SKOS vocabularies, value lists)
    /// registered in the data-model registry, so datasets can declare
    /// `conforms_to` them.
    #[serde(default)]
    data_models: Vec<DataModelDoc>,
    /// `[prefixes]` table: prefix label → namespace IRI, seeded into the
    /// server's prefix registry (label/IRI validation happens at apply time).
    #[serde(default)]
    prefixes: std::collections::HashMap<String, String>,
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
    /// The reference model this dataset conforms to (`dct:conformsTo`):
    /// `{ model = "<data model id>", version = "<version>" }`. The version
    /// defaults to the model's latest published one.
    #[serde(default)]
    conforms_to: Option<ConformsToDoc>,
    /// Graph IRIs holding SHACL shapes (declared above, or pre-existing) to
    /// register in the SHACL Studio library and bind to this dataset, so
    /// validation applies them without any further set-up.
    #[serde(default)]
    shape_graphs: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConformsToDoc {
    model: String,
    #[serde(default)]
    version: Option<String>,
}

/// A reference model shipped by the bundle: one registry entry with one
/// published version whose first graph is the version's base graph and the
/// rest its sub-graphs.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DataModelDoc {
    id: String,
    title: String,
    namespace: String,
    #[serde(default)]
    description: Option<String>,
    /// `model` (classes) or `vocabulary` (properties / SKOS). Default `model`.
    #[serde(default)]
    kind: Option<String>,
    /// Version label; default `1.0.0`.
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    graphs: Vec<GraphDoc>,
    /// The licence of the model's content (`[data_models.license]`), when it
    /// is someone else's work. See [`LicenseDoc`].
    #[serde(default)]
    license: Option<LicenseDoc>,
    /// Whether the registry entry is public; default `true`. Set `false` for
    /// content the operator may use but not re-serve publicly (NEN 2660-2
    /// carries no licence to redistribute).
    #[serde(default)]
    public: Option<bool>,
}

/// `[data_models.license]`: the licence and attribution of a bundle model
/// whose content is a third party's work, recorded on the model's registry
/// entry and version like the licence record of a bundled vocabulary.
///
/// ```toml
/// [data_models.license]
/// no_derivatives = true              # the rights holder allows no altered copies
/// copyright = ["© Stichting CROW"]
/// source = "https://github.com/Stichting-CROW/imbor/releases/tag/2025"
/// licenses = [
///   { name = "CC BY 4.0", uri = "https://creativecommons.org/licenses/by/4.0/" },
/// ]
/// remarks = "…"                      # optional; also `notice`, `changes`, `notice_url`
/// ```
///
/// With `no_derivatives = true` the registry refuses every way of altering or
/// adding content in the entry (uploads, edits, drafts, branches, merges,
/// rebases, publishes, direct SPARQL / Graph Store writes to its graphs) and
/// serves a version only while every graph still holds exactly its file's
/// triples. Every graph of a licensed model must load from a `file`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LicenseDoc {
    #[serde(default)]
    licenses: Vec<LicenseRefDoc>,
    #[serde(default)]
    copyright: Vec<String>,
    source: String,
    #[serde(default)]
    notice: Option<String>,
    #[serde(default)]
    changes: Option<String>,
    #[serde(default)]
    remarks: Option<String>,
    /// Where the full attribution and licence texts are; defaults to `source`.
    #[serde(default)]
    notice_url: Option<String>,
    #[serde(default)]
    no_derivatives: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LicenseRefDoc {
    name: String,
    uri: String,
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

        let graphs = resolve_graphs(dir, ds.graphs, &format!("dataset '{}'", ds.slug))?;

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
            conforms_to: ds.conforms_to.map(|c| (c.model, c.version)),
            shape_graphs: ds.shape_graphs,
        });
    }

    let mut data_models = Vec::with_capacity(doc.data_models.len());

    for dm in doc.data_models {
        let kind = match dm
            .kind
            .as_deref()
            .map(str::trim)
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            None | Some("model") | Some("data-model") | Some("ontology") => {
                crate::kind_detector::RegistryKind::DataModel
            }

            Some("vocabulary") | Some("vocab") => crate::kind_detector::RegistryKind::Vocabulary,

            Some("shapes") => crate::kind_detector::RegistryKind::Shapes,

            Some(other) => bail!("data model '{}': unknown kind '{other}'", dm.id),
        };

        if dm.graphs.is_empty() {
            bail!("data model '{}' declares no graphs", dm.id);
        }

        let license = match dm.license {
            None => None,
            Some(l) => {
                if l.source.trim().is_empty() {
                    bail!("data model '{}': license.source must not be empty", dm.id);
                }
                for r in &l.licenses {
                    oxigraph::model::NamedNode::new(r.uri.as_str()).with_context(|| {
                        format!("data model '{}': licence URI '{}'", dm.id, r.uri)
                    })?;
                }
                let mut files = Vec::with_capacity(dm.graphs.len());
                for g in &dm.graphs {
                    match &g.file {
                        Some(f) => files.push(f.clone()),
                        None => bail!(
                            "data model '{}': graph <{}> has no file; every graph of a licensed \
                             model must load from a file, so the registry can check it",
                            dm.id,
                            g.iri
                        ),
                    }
                }
                Some(BundleLicense {
                    licenses: l.licenses.into_iter().map(|r| (r.name, r.uri)).collect(),
                    copyright: l.copyright,
                    notice_url: l.notice_url.unwrap_or_else(|| l.source.clone()),
                    source: l.source,
                    notice: l.notice,
                    changes: l.changes,
                    remarks: l.remarks,
                    no_derivatives: l.no_derivatives,
                    files,
                })
            }
        };

        let graphs = resolve_graphs(dir, dm.graphs, &format!("data model '{}'", dm.id))?;

        data_models.push(BundleDataModel {
            id: dm.id,

            title: dm.title,

            namespace: dm.namespace,

            description: dm.description,

            kind,

            version: dm.version.unwrap_or_else(|| "1.0.0".to_string()),

            graphs,

            license,

            public: dm.public.unwrap_or(true),
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
        prefixes: doc.prefixes,
        data_models,
    })
}

/// Resolve a payload path relative to the bundle directory, rejecting absolute
/// paths and `..` components so a manifest cannot read files outside its own
/// bundle (a seed dir may be operator-writable but the manifest untrusted).
/// Resolve `[[…graphs]]` entries: role strings and payload files.
fn resolve_graphs(
    dir: &Path,
    docs: Vec<GraphDoc>,
    owner: &str,
) -> anyhow::Result<Vec<BundleGraph>> {
    let mut graphs = Vec::with_capacity(docs.len());
    for g in docs {
        let role = match g.role.as_deref() {
            None => None,
            Some(r) => Some(
                GraphKind::from_str(r)
                    .with_context(|| format!("{owner}: graph <{}>: unknown role '{r}'", g.iri))?,
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
    Ok(graphs)
}

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

    fn write_bundle(dir: &Path, license: &str, graph_file: bool) {
        std::fs::write(
            dir.join("manifest.toml"),
            format!(
                r#"
id = "lic"

[organisation]
slug = "lic-org"
name = "Licence Org"

[[data_models]]
id = "lic-model"
title = "Licensed"
namespace = "https://example.org/lic#"
version = "1"
{license}

[[data_models.graphs]]
iri = "https://example.org/lic/def"
{file}
"#,
                file = if graph_file { "file = \"m.ttl\"" } else { "" }
            ),
        )
        .unwrap();
        std::fs::write(
            dir.join("m.ttl"),
            "<https://example.org/a> a <https://example.org/T> .",
        )
        .unwrap();
    }

    /// `[data_models.license]` parses into the bundle model; every graph of a
    /// licensed model must load from a file, and a licence URI must be an IRI.
    #[test]
    fn a_data_model_licence_is_parsed_and_checked() {
        let tmp = tempfile::tempdir().unwrap();
        let license = r#"
[data_models.license]
no_derivatives = true
copyright = ["© Stichting CROW"]
source = "https://github.com/Stichting-CROW/imbor/releases/tag/2025"
licenses = [{ name = "CC BY 4.0", uri = "https://creativecommons.org/licenses/by/4.0/" }]
"#;
        write_bundle(tmp.path(), license, true);
        let b = parse_bundle(tmp.path()).unwrap();
        let l = b.data_models[0].license.as_ref().unwrap();
        assert!(l.no_derivatives);
        assert_eq!(l.files, vec!["m.ttl".to_string()]);
        assert_eq!(l.notice_url, l.source);
        assert_eq!(
            l.licenses[0].1,
            "https://creativecommons.org/licenses/by/4.0/"
        );

        write_bundle(tmp.path(), license, false);
        assert!(parse_bundle(tmp.path()).is_err(), "a graph without a file");

        write_bundle(
            tmp.path(),
            &license.replace("https://creativecommons.org/licenses/by/4.0/", "not an iri"),
            true,
        );
        assert!(
            parse_bundle(tmp.path()).is_err(),
            "a licence URI that is no IRI"
        );

        write_bundle(tmp.path(), "", true);
        assert!(parse_bundle(tmp.path()).unwrap().data_models[0]
            .license
            .is_none());
    }

    /// The shipped nen2660-imbor bundle declares IMBOR Kern's licence: CROW's,
    /// with no altered copies.
    #[test]
    fn the_nen2660_imbor_manifest_declares_imbor_no_derivatives() {
        let raw = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("examples/seed-bundles/nen2660-imbor/manifest.toml"),
        )
        .unwrap();
        let doc: ManifestDoc = toml::from_str(&raw).unwrap();
        let otl = doc
            .data_models
            .iter()
            .find(|m| m.id == "imbor-otl")
            .unwrap();
        let l = otl
            .license
            .as_ref()
            .expect("imbor-otl declares its licence");
        assert!(l.no_derivatives);
        assert_eq!(l.copyright, vec!["© Stichting CROW".to_string()]);
        assert!(otl.graphs.iter().all(|g| g.file.is_some()));
        let nen = doc
            .data_models
            .iter()
            .find(|m| m.id == "nen2660-2")
            .unwrap();
        assert!(nen.license.is_none());
    }

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
