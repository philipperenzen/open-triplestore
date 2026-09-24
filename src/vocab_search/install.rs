//! Offline vocabulary install — copy a vocabulary from the local LOV corpus
//! into the model/vocabulary registry as a first-class entry.
//!
//! Mirrors the standard-vocabulary seeder (`seed_vocab::seed_one`): registry
//! header + one published version whose named graph holds the vocabulary's
//! triples, kind auto-detected.  No network is involved — the triples come
//! from the local `lov.nq.gz`.  The version notes record the vocabulary's
//! own licence (LOV's CC BY 4.0 does not cover it) and the notice that
//! licence requires on copies.
//!
//! Visibility follows that licence.  The Docker image's corpus holds only
//! redistributable vocabularies, but an operator may mount the full dump
//! (`VOCAB_CORPUS_PATH`), which also holds NonCommercial, copyleft and
//! unlicensed ones, and ones withheld because LOV's copy cannot be shipped
//! under their licence.  A public registry entry would re-serve such a
//! vocabulary to anyone, so a vocabulary that is not redistributable is
//! installed **private** (owned by the installing admin); an admin can make
//! it public after checking its terms.
//!
//! Each installed version also gets a licence record in the registry
//! ([`ContentAttribution`], as the bundled vocabularies have): its licences
//! with their URIs, the required notice, where the copy comes from and
//! whether the store holds it unchanged.  Downloads carry it in their `Link`
//! headers, and a vocabulary whose licence allows only unaltered copies
//! (`no_derivatives`, CC BY-ND or the OGC Document Notice) is refused as the
//! source of a draft, branch or any other edit.
//!
//! Installs made by releases before these rules ([`migrate_legacy_installs`])
//! are checked at every boot: made private when this project may not serve
//! them publicly, and given a licence record that does not claim more than is
//! known about them.  Their graphs and notes are never changed.

use std::collections::BTreeSet;

use serde::Serialize;

use crate::data_models::models::{
    ContentAttribution, DataModelRecord, DataModelVersion, LicenseRef, VersionStatus,
};
use crate::data_models::{content_digest, registry, upload};
use crate::kind_detector;
use crate::store::TripleStore;

use super::catalog::{CatalogSource, LicenseSource, LicenseStatus, LovVocab, VocabCatalog};
use super::corpus;

#[derive(Debug, Serialize)]
pub struct InstallOutcome {
    pub model_id: String,
    pub version: String,
    pub triples: usize,
    pub kind: String,
    /// The vocabulary's licences: those its own graph declares or, where it
    /// names none, those its publisher states elsewhere (`license_source`).
    pub license: Vec<String>,
    pub license_status: Option<LicenseStatus>,
    pub license_source: Option<LicenseSource>,
    pub license_source_url: Option<String>,
    /// The statement the licence requires on copies.
    pub license_notice: Option<String>,
    /// Whether this project may redistribute the vocabulary.
    pub redistributable: bool,
    /// Every licence offered allows only unaltered copies: the installed
    /// version cannot be copied into a draft or branch, or edited.
    pub no_derivatives: bool,
    /// Why an openly licensed vocabulary is still not redistributed.
    pub redistribution_withheld: Option<String>,
    /// Whether the new registry entry is public.  False when the licence
    /// does not allow redistribution: the entry is then private to its
    /// owner and the admins.
    pub is_public: bool,
    /// The licence and visibility sentence also written to the version notes.
    pub note: String,
}

#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error("Unknown vocabulary {0:?}")]
    UnknownVocab(String),
    #[error("Vocabulary {0:?} is not present in this instance's LOV corpus")]
    NotInCorpus(String),
    #[error(
        "Vocabulary {0:?} is not in this instance's LOV corpus: the Docker image ships only \
         vocabularies whose licence lets this project redistribute them. Point \
         VOCAB_CORPUS_PATH at the full LOV dump to install it"
    )]
    NotRedistributed(String),
    #[error("The LOV corpus file is not available on this instance")]
    CorpusUnavailable,
    #[error("A registry entry for {0:?} already exists")]
    AlreadyInstalled(String),
    #[error("{0}")]
    Internal(String),
}

/// Derive an IRI-safe, slug-like version label from LOV version metadata.
fn version_label(name: Option<&str>, issued: Option<&str>) -> String {
    let candidate = name
        .map(|n| n.trim().trim_start_matches('v').to_string())
        .filter(|n| {
            !n.is_empty()
                && n.chars()
                    .all(|c| c.is_ascii_alphanumeric() || ".-_".contains(c))
        })
        .or_else(|| issued.map(str::to_string))
        .unwrap_or_else(|| "lov".to_string());
    candidate
}

/// The version-note sentences on the vocabulary's own licence and the
/// notice it requires on copies.
fn license_note(vocab: &LovVocab) -> String {
    let mut note = licence_sentence(vocab);
    if let Some(notice) = vocab.license_notice.as_deref() {
        note.push_str(&format!(" Required notice: \"{notice}\""));
    }
    note
}

fn licence_sentence(vocab: &LovVocab) -> String {
    let declared = || vocab.license_declared.join("; ");
    if vocab.license_source == Some(LicenseSource::PublisherTerms) {
        // The graph names none; the publisher states its terms elsewhere.
        let mut note = format!(
            "Licence stated by the publisher outside the vocabulary: {}{}.",
            vocab.license.join(", "),
            vocab
                .license_source_url
                .as_deref()
                .map(|u| format!(" ({u})"))
                .unwrap_or_default()
        );
        if vocab.license_status == Some(LicenseStatus::Restricted) {
            note.push_str(" It restricts redistribution.");
        }
        return note;
    }
    match vocab.license_status {
        Some(LicenseStatus::Open) => format!(
            "Licence declared by the vocabulary: {}.",
            vocab.license.join(", ")
        ),
        Some(LicenseStatus::Restricted) => format!(
            "Licence declared by the vocabulary: {} (restricts redistribution; check its terms \
             before republishing).",
            vocab.license.join(", ")
        ),
        Some(LicenseStatus::Unrecognised) => format!(
            "Licence statement in the vocabulary, not recognised: {}.",
            declared()
        ),
        Some(LicenseStatus::CopyrightOnly) => format!(
            "The vocabulary states only a copyright notice, no licence: {}.",
            declared()
        ),
        Some(LicenseStatus::Undeclared) => "The vocabulary declares no licence.".to_string(),
        None => "Licence not recorded.".to_string(),
    }
}

/// The version-note sentence on the entry's visibility.
fn visibility_note(vocab: &LovVocab, is_public: bool) -> String {
    if is_public {
        "Installed as a public vocabulary: its licence allows redistribution.".to_string()
    } else if let Some(why) = vocab.redistribution_withheld.as_deref() {
        format!(
            "Installed as a private vocabulary because this project does not redistribute it: \
             {why} Only its owner and admins can see it."
        )
    } else {
        "Installed as a private vocabulary because its licence does not allow \
         redistribution: only its owner and admins can see it. Check the \
         vocabulary's terms before making it public."
            .to_string()
    }
}

// ─── Licence record ──────────────────────────────────────────────────────────

/// The served page with a LOV vocabulary's licence, notice and origin
/// (`GET /api/vocab/notice`), relative to the server's base URL.
pub fn notice_path(prefix: &str) -> String {
    format!("/api/vocab/notice?vocab={prefix}")
}

/// Where an installed copy comes from: LOV's dump and the vocabulary's graph
/// in it.  Recorded as the licence record's `file`, which the registry quotes
/// as "vocab/<file>" (the corpus is `assets/vocab/lov.nq.gz` in the image and
/// `{data_dir}/vocab/lov.nq.gz` when downloaded).
fn corpus_origin(vocab: &LovVocab) -> String {
    format!("lov.nq.gz (graph <{}>)", vocab.uri)
}

/// What the stored copy is, for a version whose graph was checked against
/// LOV's copy in this instance's corpus (`unchanged`) or could not be.
/// `canonical_forms`: the store holds some of the copy's typed literals in
/// its canonical form (see [`content_digest::as_stored`]).
fn stored_copy_text(vocab: &LovVocab, unchanged: bool, canonical_forms: bool) -> String {
    if unchanged {
        let forms = if canonical_forms {
            " The store writes some of its typed literals in its canonical form, with the same \
             values (for example \"1\"^^xsd:nonNegativeInteger as \"1\"^^xsd:integer, or a \
             +00:00 time zone as Z)."
        } else {
            ""
        };
        format!(
            "The store holds the triples of the graph <{}> of this instance's LOV corpus \
             (lov.nq.gz), unchanged: loaded verbatim and checked against it.{forms} API \
             downloads are serialized anew from them.",
            vocab.uri
        )
    } else {
        format!(
            "Installed from the graph <{}> of a LOV corpus. The stored graph could not be shown \
             to hold exactly LOV's copy in this instance's corpus, so it may differ from it.",
            vocab.uri
        )
    }
}

/// The sentence on characters LOV mis-decoded in its copy, if any.
fn misdecoded_sentence(vocab: &LovVocab) -> String {
    if vocab.lov_misdecoded > 0 {
        format!(
            " LOV's copy has mis-decoded characters in {} literal(s) (see the notice).",
            vocab.lov_misdecoded
        )
    } else {
        String::new()
    }
}

/// A licence record for a copy of `vocab`: its licences, notice and source
/// from the catalogue, and what is known about the copy.
fn attribution(
    vocab: &LovVocab,
    changes: String,
    stored_copy: String,
    unchanged: bool,
    remarks: String,
) -> ContentAttribution {
    ContentAttribution {
        file: corpus_origin(vocab),
        licenses: vocab
            .license_refs()
            .into_iter()
            .map(|(name, uri)| LicenseRef { name, uri })
            .collect(),
        copyright: Vec::new(),
        notice: vocab.license_notice.clone(),
        status: None,
        source_url: vocab.uri.clone(),
        specification_url: None,
        changes: Some(changes),
        stored_copy,
        unchanged,
        remarks: Some(remarks),
        no_derivatives: vocab.no_derivatives,
        header: None,
        notice_url: notice_path(&vocab.prefix),
    }
}

/// The registry's licence record for a copy of `vocab` this release
/// installed.  `unchanged`: the stored graph was checked to hold LOV's copy;
/// `canonical_forms`: the store holds some of its typed literals in canonical
/// form.
pub fn lov_attribution(
    vocab: &LovVocab,
    source: &CatalogSource,
    unchanged: bool,
    canonical_forms: bool,
) -> ContentAttribution {
    let changes = format!(
        "None by Open Triplestore: LOV's N-Quads serialization of the vocabulary, loaded \
         verbatim from this instance's LOV corpus. This release's catalogue describes the LOV \
         dump {} (snapshot {}).{}",
        source.url,
        source.snapshot_date,
        misdecoded_sentence(vocab)
    );
    attribution(
        vocab,
        changes,
        stored_copy_text(vocab, unchanged, canonical_forms),
        unchanged,
        licence_sentence(vocab),
    )
}

/// The typed literals among the objects of `triples`, as N-Triples terms.
/// Comparing these before and after [`content_digest::as_stored`] tells
/// whether the store rewrites any of them.
fn typed_literals(triples: &[oxigraph::model::Triple]) -> BTreeSet<String> {
    use oxigraph::model::{vocab::xsd, Term};
    triples
        .iter()
        .filter_map(|t| match &t.object {
            Term::Literal(l) if l.language().is_none() && l.datatype() != xsd::STRING => {
                Some(l.to_string())
            }
            _ => None,
        })
        .collect()
}

/// Whether the graph `graph_iri` holds exactly the triples of `quads`
/// (blank nodes compared up to renaming).
fn holds_exactly(
    store: &TripleStore,
    graph_iri: &str,
    expected: &[oxigraph::model::Triple],
) -> bool {
    match content_digest::graph_triples(store, graph_iri) {
        Ok(stored) => content_digest::same_triples(&stored, expected),
        Err(e) => {
            tracing::warn!("vocab install: cannot read {graph_iri}: {e}");
            false
        }
    }
}

/// Install `prefix` from the corpus at `corpus_path` into the registry.
pub fn install_lov_vocab(
    store: &TripleStore,
    base_url: &str,
    catalog: &VocabCatalog,
    corpus_path: Option<&std::path::Path>,
    prefix: &str,
    installed_by: Option<&str>,
) -> Result<InstallOutcome, InstallError> {
    // Installs run behind the admin gate: visibility checks don't apply.
    let see_all = |_: &super::catalog::PlatformVocab| true;
    let vocab = catalog
        .lov_by_prefix(prefix)
        .or_else(|| {
            catalog.info(prefix, &see_all).and_then(|e| {
                // Allow lookup by URI/namespace too.
                let p = e.prefix.clone();
                catalog.lov_by_prefix(&p)
            })
        })
        .ok_or_else(|| InstallError::UnknownVocab(prefix.to_string()))?
        .clone();
    if vocab.graph_quads == 0 {
        return Err(InstallError::NotInCorpus(prefix.to_string()));
    }
    let corpus_path = corpus_path.ok_or(InstallError::CorpusUnavailable)?;

    // Registry id: the LOV prefix, lowercased (registry ids are slugs).
    let model_id = vocab.prefix.to_lowercase();
    if registry::data_model_exists(store, base_url, &model_id) {
        return Err(InstallError::AlreadyInstalled(model_id));
    }
    // A different id already claiming the namespace also counts as installed.
    if let Some(existing) = catalog.info(&vocab.nsp, &see_all) {
        if existing.source == "platform" {
            return Err(InstallError::AlreadyInstalled(
                existing.model_id.unwrap_or(model_id),
            ));
        }
    }

    let quads = corpus::extract_vocab_quads(corpus_path, &vocab.uri)
        .map_err(|e| InstallError::Internal(format!("corpus read failed: {e}")))?;
    if quads.is_empty() {
        // The image's corpus leaves out what we may not redistribute.
        return Err(if vocab.redistributable {
            InstallError::NotInCorpus(prefix.to_string())
        } else {
            InstallError::NotRedistributed(prefix.to_string())
        });
    }
    let triples = quads.len();

    let latest = vocab.versions.first();
    let version = version_label(
        latest.and_then(|v| v.name.as_deref()),
        latest.and_then(|v| v.issued.as_deref()),
    );

    let title = vocab
        .titles
        .first()
        .map(|t| t.value.clone())
        .unwrap_or_else(|| vocab.prefix.clone());
    let description = vocab.descriptions.first().map(|d| d.value.clone());
    let now = chrono::Utc::now().to_rfc3339();

    // Only a vocabulary we may redistribute becomes a public reference
    // vocabulary; anything else stays private to the installing admin (its
    // user id, from the registry's `{base}/users/{id}` creator IRI) and the
    // other admins.
    let is_public = vocab.redistributable;
    let owner_id = installed_by.and_then(|iri| iri.strip_prefix(&format!("{base_url}/users/")));
    let (owner_type, owner_id) = match (is_public, owner_id) {
        (false, Some(id)) if !id.is_empty() => (Some("user"), Some(id)),
        _ => (None, None),
    };
    let note = format!(
        "{} {}",
        license_note(&vocab),
        visibility_note(&vocab, is_public)
    );

    registry::insert_data_model(
        store,
        base_url,
        &model_id,
        &title,
        &vocab.nsp,
        description.as_deref(),
        is_public,
        owner_type,
        owner_id,
        installed_by,
        &now,
    )
    .map_err(|e| InstallError::Internal(format!("registry insert failed: {e}")))?;

    // Any failure past this point rolls the registry entry (and loaded
    // graphs) back — a half-installed entry would otherwise block reinstall
    // forever via the data_model_exists guard above.
    let rollback = |graphs: &[String], reason: String| -> InstallError {
        let refs: Vec<&str> = graphs.iter().map(String::as_str).collect();
        if let Err(e) = store.bulk_delete_graphs(&refs) {
            tracing::warn!("install rollback: graph cleanup failed for {model_id}: {e}");
        }
        if let Err(e) = registry::delete_data_model(store, base_url, &model_id) {
            tracing::warn!("install rollback: registry cleanup failed for {model_id}: {e}");
        }
        InstallError::Internal(reason)
    };

    let kind = kind_detector::detect(&quads).primary;
    // What the store holds once the quads are loaded: the same triples, with
    // typed literals in its canonical form ("1"^^xsd:nonNegativeInteger as
    // xsd:integer, +00:00 as Z).  The copy check compares with that, or every
    // OWL vocabulary with a cardinality restriction would read as altered.
    let raw = content_digest::quads_as_triples(&quads);
    let expected = match content_digest::as_stored(&raw) {
        Ok(t) => t,
        Err(e) => return Err(rollback(&[], format!("copy check failed: {e}"))),
    };
    let canonical_forms = typed_literals(&raw) != typed_literals(&expected);
    drop(raw);

    // Verbatim: a vocabulary's licence may allow only unaltered copies (CC
    // BY-ND, the W3C Document License), so nothing is added to its graph.
    let result =
        match upload::load_parsed_verbatim(store, base_url, &model_id, &version, quads, true) {
            Ok(r) => r,
            Err(e) => return Err(rollback(&[], format!("graph load failed: {e}"))),
        };

    let graph_iri = format!(
        "{}/data-model/{}/version/{}",
        base_url, model_id, result.version
    );
    let mut loaded_graphs = vec![graph_iri.clone()];
    loaded_graphs.extend(result.sub_graphs.iter().cloned());
    let record = DataModelVersion {
        data_model_id: model_id.clone(),
        version: result.version.clone(),
        status: VersionStatus::Published,
        graph_iri,
        sub_graphs: result.sub_graphs,
        created_at: latest
            .and_then(|v| v.issued.as_deref())
            .map(|d| format!("{d}T00:00:00Z"))
            .unwrap_or_else(|| now.clone()),
        created_by: installed_by.map(str::to_string),
        derived_from: None,
        notes: Some(format!(
            "Installed from the LOV corpus (snapshot {}). {}",
            catalog.source().snapshot_date,
            note
        )),
        branch: None,
        sub_graph_status: Vec::new(),
    };
    if let Err(e) = registry::insert_version(store, base_url, &record) {
        return Err(rollback(
            &loaded_graphs,
            format!("version insert failed: {e}"),
        ));
    }

    // The licence record: what the registry's download headers, the model
    // pages and the no-derivatives guard read.  "Unchanged" only once the
    // stored graph is checked to hold LOV's copy exactly.
    let unchanged = holds_exactly(store, &record.graph_iri, &expected);
    let attribution = lov_attribution(&vocab, catalog.source(), unchanged, canonical_forms);
    let record_iri = registry::version_record_iri(base_url, &model_id, &result.version);
    if let Err(e) = registry::set_attribution(store, &record_iri, Some(&attribution)) {
        return Err(rollback(
            &loaded_graphs,
            format!("licence record failed: {e}"),
        ));
    }
    // The digest of the checked copy, as the seeder records it: a later write
    // that names no graph is re-checked against it (write_guard), so a copy
    // called unchanged stops being called so the moment it is altered.
    if unchanged {
        let graphs = content_digest::version_graphs(&record.graph_iri, &record.sub_graphs);
        match content_digest::graphs_digest(store, &graphs) {
            Ok(digest) => {
                if let Err(e) = registry::set_seed_check(
                    store,
                    &record_iri,
                    &catalog.source().sha256,
                    Some(&digest),
                ) {
                    return Err(rollback(&loaded_graphs, format!("copy check failed: {e}")));
                }
            }
            Err(e) => return Err(rollback(&loaded_graphs, format!("copy check failed: {e}"))),
        }
    }

    if let Some(k) = kind {
        if let Err(e) = registry::set_data_model_kind(store, base_url, &model_id, k) {
            return Err(rollback(&loaded_graphs, format!("kind update failed: {e}")));
        }
    }
    if let Err(e) = registry::update_latest_published(store, base_url, &model_id, &result.version) {
        return Err(rollback(
            &loaded_graphs,
            format!("latest update failed: {e}"),
        ));
    }

    Ok(InstallOutcome {
        model_id,
        version: result.version,
        triples,
        kind: kind
            .map(|k| format!("{k:?}").to_lowercase())
            .unwrap_or_else(|| "unknown".to_string()),
        license: vocab.license.clone(),
        license_status: vocab.license_status,
        license_source: vocab.license_source,
        license_source_url: vocab.license_source_url.clone(),
        license_notice: vocab.license_notice.clone(),
        redistributable: vocab.redistributable,
        no_derivatives: vocab.no_derivatives,
        redistribution_withheld: vocab.redistribution_withheld.clone(),
        is_public,
        note,
    })
}

// ─── Installs by earlier releases ────────────────────────────────────────────

/// The version note of an install an earlier release made, verbatim.  Every
/// earlier release shipped the LOV catalogue of snapshot 2025-12-18 and wrote
/// exactly this text; nothing else is taken for one of its installs.
const LEGACY_NOTE: &str = "Installed from the bundled LOV corpus (snapshot 2025-12-18, CC BY 4.0).";
/// The LOV snapshot that note names.
const LEGACY_SNAPSHOT: &str = "2025-12-18";
/// How that note begins.  Only used to find notes that start like it, so that
/// an edited one is reported rather than acted on.
const LEGACY_NOTE_START: &str = "Installed from the bundled LOV corpus (";

/// Registry predicate (in the registry's own `urn:system:vocab/` namespace)
/// the migration sets, to `"applied"`, on an earlier install's version record
/// once the entry is private.  From then on the entry's visibility is the
/// admins' to decide: a later boot does not make it private again.
const VISIBILITY_MARK: &str = "urn:system:vocab/lovLegacyVisibility";

/// How the `changes` of the licence record the migration writes begin: how
/// it knows the record as its own, to bring up to date.
const LEGACY_CHANGES_START: &str = "Installed by an earlier release of Open Triplestore";

/// What the migration changed for one earlier install.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyInstall {
    pub model_id: String,
    pub version: String,
    /// The entry was public and is now private.
    pub made_private: bool,
    /// A licence record was written for the version, or the migration's own
    /// earlier record brought up to date.
    pub recorded_licence: bool,
}

/// Why this project may not serve an earlier install of `vocab` publicly,
/// if it may not: the vocabulary is not redistributable, or its licence
/// allows only unaltered copies and the install's graph is not shown to be
/// one (the earlier release may have added a triple to it, and nothing has
/// checked it since).
fn legacy_private_reason(vocab: &LovVocab) -> Option<String> {
    if !vocab.redistributable {
        return Some(match vocab.redistribution_withheld.as_deref() {
            Some(why) => format!(
                "this project does not redistribute it ({})",
                why.trim_end_matches('.')
            ),
            None => "its licence does not allow redistribution".to_string(),
        });
    }
    vocab.no_derivatives.then(|| {
        "its licence allows only unaltered copies, and the stored graph is not shown to be one \
         (the earlier release may have added an owl:versionInfo triple to it)"
            .to_string()
    })
}

/// The licence record of a version an earlier release installed: the
/// vocabulary's own licences, notice and source as this release's catalogue
/// records them, and a copy that is not claimed to be LOV's, unchanged.  A
/// function of the vocabulary and the version label alone, so a later boot
/// can tell whether the stored record is still this.
fn legacy_attribution(vocab: &LovVocab, version: &str) -> ContentAttribution {
    let changes = format!(
        "{LEGACY_CHANGES_START} from LOV's N-Quads serialization of the vocabulary (LOV corpus \
         snapshot {LEGACY_SNAPSHOT}). That release added an owl:versionInfo \"{version}\" triple \
         to the graph if the vocabulary stated no version of its own. The graph has not been \
         checked against LOV's copy since, and may also have been edited.{}",
        misdecoded_sentence(vocab)
    );
    let stored_copy = format!(
        "Installed from the graph <{}> of a LOV corpus by an earlier release, which may have \
         added an owl:versionInfo \"{version}\" triple to it. The stored graph is not checked \
         against LOV's copy, so it may differ from it: it may have been modified.",
        vocab.uri
    );
    let mut remarks = licence_sentence(vocab);
    if !vocab.redistributable {
        remarks.push_str(&match vocab.redistribution_withheld.as_deref() {
            Some(why) => format!(" This project does not redistribute it: {why}"),
            None => " Its licence does not allow this project to redistribute it.".to_string(),
        });
    }
    attribution(vocab, changes, stored_copy, false, remarks)
}

/// One version record whose note starts like the earlier installer's, as
/// the registry holds it.
struct NoteRow {
    record_iri: String,
    model_iri: Option<String>,
    version: Option<String>,
    notes: String,
    attribution: Option<String>,
    mark: Option<String>,
}

/// Every version record whose note starts like the earlier installer's, with
/// what a boot needs to see that nothing is left to do: one query.
fn legacy_note_rows(store: &TripleStore) -> Result<Vec<NoteRow>, String> {
    use oxigraph::model::Term;
    use oxigraph::sparql::QueryResults;
    let q = format!(
        r#"
        SELECT ?v ?m ?semver ?n ?a ?mark WHERE {{ GRAPH <{graph}> {{
          ?v <http://www.w3.org/ns/adms#versionNotes> ?n .
          FILTER(STRSTARTS(STR(?n), "{start}"))
          OPTIONAL {{ ?v <urn:system:vocab/dataModel> ?m }}
          OPTIONAL {{ ?v <http://www.w3.org/2002/07/owl#versionInfo> ?semver }}
          OPTIONAL {{ ?v <urn:system:vocab/attribution> ?a }}
          OPTIONAL {{ ?v <{VISIBILITY_MARK}> ?mark }}
        }} }}
        "#,
        graph = registry::REGISTRY_GRAPH,
        start = crate::store::escape_sparql_literal(LEGACY_NOTE_START),
    );
    let text = |t: Option<&Term>| -> Option<String> {
        match t? {
            Term::NamedNode(n) => Some(n.as_str().to_string()),
            Term::Literal(l) => Some(l.value().to_string()),
            _ => None,
        }
    };
    let mut rows: Vec<NoteRow> = Vec::new();
    match store.query(&q) {
        Ok(QueryResults::Solutions(solutions)) => {
            for row in solutions {
                let row = row.map_err(|e| e.to_string())?;
                let (Some(record_iri), Some(notes)) = (text(row.get("v")), text(row.get("n")))
                else {
                    continue;
                };
                if rows.iter().any(|r| r.record_iri == record_iri) {
                    continue;
                }
                rows.push(NoteRow {
                    record_iri,
                    model_iri: text(row.get("m")),
                    version: text(row.get("semver")),
                    notes,
                    attribution: text(row.get("a")),
                    mark: text(row.get("mark")),
                });
            }
            Ok(rows)
        }
        Ok(_) => Err("unexpected result form".to_string()),
        Err(e) => Err(e.to_string()),
    }
}

/// Bring the LOV installs of earlier releases in line with the licence
/// rules.  Those releases installed every vocabulary public, loaded it with
/// an `owl:versionInfo` triple added when it stated none, and noted LOV's
/// CC BY 4.0 as its licence.
///
/// A version counts as such an install only when everything the earlier
/// installer did holds (see [`legacy_install`]): its exact note, an entry
/// with no owner whose id and namespace are a LOV vocabulary's, the version
/// graph where the installer loaded it, a creator who is (as far as this
/// instance knows) an admin.  Anything else is left alone and logged.  For
/// each install:
///
/// * the entry is made private when the vocabulary is not redistributable or
///   its licence allows only unaltered copies — once: the registry marks the
///   version ([`VISIBILITY_MARK`]), and an admin may make the entry public
///   again after checking its terms;
/// * the version gets a licence record naming the vocabulary's own licence,
///   with `unchanged: false`, saying that the earlier release may have added
///   an `owl:versionInfo` triple, unless it has a record the migration did
///   not write.
///
/// Never changes a graph or a note, needs no corpus, and runs at every boot:
/// once nothing is left to do it costs one query.  Does nothing on a store
/// that is read-only here (a replication follower, a Raft member that does
/// not lead).  Blocking; run it before the registry-derived state (term
/// index, catalog overlay) is built.
///
/// `creator_is_non_admin(user_id)`: the user exists on this instance and is
/// not an admin.  Only admins could install, so such a version is not taken
/// for an install.
pub fn migrate_legacy_installs(
    store: &TripleStore,
    base_url: &str,
    catalog: &VocabCatalog,
    creator_is_non_admin: &dyn Fn(&str) -> bool,
) -> Vec<LegacyInstall> {
    let mut out = Vec::new();
    if store.replication().read_only() {
        return out;
    }
    let rows = match legacy_note_rows(store) {
        Ok(rows) => rows,
        Err(e) => {
            tracing::warn!("vocab-search: cannot look for LOV installs of earlier releases: {e}");
            return out;
        }
    };
    for row in rows {
        match migrate_one(store, base_url, catalog, creator_is_non_admin, &row) {
            Ok(Some(done)) => out.push(done),
            Ok(None) => {}
            Err(e) => tracing::warn!(
                "vocab-search: could not bring the earlier LOV install <{}> in line (retried at \
                 the next boot): {e}",
                row.record_iri
            ),
        }
    }
    out
}

/// A registry entry and version that are, beyond reasonable doubt, an install
/// an earlier release made, and the LOV vocabulary installed; or why not.
fn legacy_install(
    store: &TripleStore,
    base_url: &str,
    catalog: &VocabCatalog,
    creator_is_non_admin: &dyn Fn(&str) -> bool,
    row: &NoteRow,
    id: &str,
) -> Result<(DataModelRecord, DataModelVersion, LovVocab), &'static str> {
    if row.notes != LEGACY_NOTE {
        return Err("its note is not exactly the one the earlier installer wrote");
    }
    let vocab = catalog
        .lov_by_model_id(id)
        .ok_or("its id is not the prefix of a vocabulary in the LOV catalogue")?;
    let record = registry::get_data_model(store, base_url, id).ok_or("it has no registry entry")?;
    if record.namespace != vocab.nsp {
        return Err("its namespace is not the one LOV gives that prefix");
    }
    // The installer made entries without an owner; every entry a user
    // creates has one, and only admins may write an entry that has none.
    if record.owner_type.is_some() || record.owner_id.is_some() {
        return Err("its entry has an owner, and the earlier installer made none");
    }
    let label = row
        .version
        .as_deref()
        .ok_or("its record has no version label")?;
    if row.record_iri != registry::version_record_iri(base_url, id, label) {
        return Err("its version record is not where the installer put it");
    }
    let version = registry::get_version(store, base_url, id, label)
        .ok_or("its version record is incomplete")?;
    if version.graph_iri != row.record_iri
        || !version.sub_graphs.iter().all(|g| *g == version.graph_iri)
    {
        return Err("its graph is not the one the installer loaded");
    }
    if version.status != VersionStatus::Published
        || version.derived_from.is_some()
        || version.branch.is_some()
    {
        return Err("it is not a published version as the installer made it");
    }
    if version.created_by != record.created_by {
        return Err("its entry and version name different creators; the installer named one");
    }
    if let Some(creator) = version.created_by.as_deref() {
        let user = creator
            .strip_prefix(&format!("{base_url}/users/"))
            .filter(|u| !u.is_empty() && !u.contains('/'))
            .ok_or("its creator is not a user of this instance")?;
        if creator_is_non_admin(user) {
            return Err("its creator is not an admin, and only admins could install");
        }
    }
    Ok((record, version, vocab.clone()))
}

fn migrate_one(
    store: &TripleStore,
    base_url: &str,
    catalog: &VocabCatalog,
    creator_is_non_admin: &dyn Fn(&str) -> bool,
    row: &NoteRow,
) -> anyhow::Result<Option<LegacyInstall>> {
    let id = row
        .model_iri
        .as_deref()
        .and_then(|m| m.rsplit('/').next())
        .unwrap_or_default();

    // The steady state, decided from the query row alone: the record is the
    // one this build writes and the visibility rule was applied.
    if row.notes == LEGACY_NOTE {
        if let (Some(v), Some(label)) = (catalog.lov_by_model_id(id), row.version.as_deref()) {
            let stored = row
                .attribution
                .as_deref()
                .and_then(|j| serde_json::from_str::<ContentAttribution>(j).ok());
            if stored.as_ref() == Some(&legacy_attribution(v, label))
                && (legacy_private_reason(v).is_none() || row.mark.is_some())
            {
                return Ok(None);
            }
        }
    }

    let (record, version, vocab) =
        match legacy_install(store, base_url, catalog, creator_is_non_admin, row, id) {
            Ok(found) => found,
            Err(why) => {
                // Only worth an admin's attention when the entry is public
                // and holds a vocabulary this project may not serve so.
                let matters = catalog
                    .lov_by_model_id(id)
                    .is_some_and(|v| legacy_private_reason(v).is_some())
                    && registry::get_data_model(store, base_url, id).is_some_and(|r| r.is_public);
                if matters {
                    tracing::warn!(
                        "vocab-search: <{}> reads like a LOV install of an earlier release, but \
                         {why}; left as it is. If it holds LOV's copy of '{id}', an admin should \
                         check the vocabulary's terms: this project may not serve it publicly",
                        row.record_iri
                    );
                } else {
                    tracing::debug!(
                        "vocab-search: <{}> is not taken for a LOV install of an earlier release: \
                         {why}",
                        row.record_iri
                    );
                }
                return Ok(None);
            }
        };
    let ver_iri = row.record_iri.as_str();

    // 1. Visibility, once.  The mark goes in after the change, so a boot cut
    //    short in between finds the entry private and only marks it.
    let mut made_private = false;
    if let Some(why) = legacy_private_reason(&vocab) {
        if row.mark.is_none() {
            if record.is_public {
                registry::update_data_model(
                    store,
                    base_url,
                    &record.id,
                    None,
                    None,
                    None,
                    Some(false),
                    None,
                    None,
                )?;
                made_private = true;
                tracing::warn!(
                    "vocab-search: made the earlier LOV install '{}' private, because {why}. An \
                     admin can make it public again after checking its terms",
                    record.id
                );
            }
            set_visibility_mark(store, ver_iri)?;
        }
    }

    // 2. The licence record, unless the version has one the migration did not
    //    write.
    let wanted = legacy_attribution(&vocab, &version.version);
    let recorded_licence = match registry::get_attribution(store, ver_iri) {
        None => true,
        Some(a) if a == wanted => false,
        Some(a)
            if a.file == wanted.file
                && a.changes
                    .as_deref()
                    .is_some_and(|c| c.starts_with(LEGACY_CHANGES_START)) =>
        {
            true
        }
        Some(_) => {
            tracing::warn!(
                "vocab-search: the earlier LOV install '{}' version {} has a licence record the \
                 migration did not write; left as it is",
                record.id,
                version.version
            );
            false
        }
    };
    if recorded_licence {
        registry::set_attribution(store, ver_iri, Some(&wanted))?;
    }

    Ok((made_private || recorded_licence).then(|| LegacyInstall {
        model_id: record.id.clone(),
        version: version.version.clone(),
        made_private,
        recorded_licence,
    }))
}

/// Mark `ver_iri` as having had the visibility rule applied (see
/// [`VISIBILITY_MARK`]).
fn set_visibility_mark(
    store: &TripleStore,
    ver_iri: &str,
) -> Result<(), crate::store::engine::StoreError> {
    use crate::store::engine::StoreError;
    oxigraph::model::NamedNode::new(ver_iri)
        .map_err(|e| StoreError::Parse(format!("invalid registry IRI {ver_iri}: {e}")))?;
    store.update(&format!(
        "INSERT DATA {{ GRAPH <{}> {{ <{ver_iri}> <{VISIBILITY_MARK}> \"applied\" }} }}",
        registry::REGISTRY_GRAPH,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Write `nq` as a gzipped N-Quads corpus under a fresh temp directory.
    fn corpus(name: &str, nq: &str) -> std::path::PathBuf {
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("ots-install-test-{name}"));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("lov.nq.gz");
        let mut enc = flate2::write::GzEncoder::new(
            std::fs::File::create(&path).unwrap(),
            Default::default(),
        );
        enc.write_all(nq.as_bytes()).unwrap();
        enc.finish().unwrap();
        path
    }

    /// Two quads in the named graph `graph`.
    fn two_quads(graph: &str) -> String {
        format!(
            "<{graph}#Thing> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> \
             <http://www.w3.org/2002/07/owl#Class> <{graph}> .\n\
             <{graph}#Thing> <http://www.w3.org/2000/01/rdf-schema#label> \"Thing\"@en <{graph}> .\n"
        )
    }

    #[test]
    fn install_from_corpus_fixture() {
        let catalog = VocabCatalog::bundled();
        let store = TripleStore::in_memory().unwrap();
        let base = "http://localhost:7878";

        // Minimal corpus: two quads in the real GoodRelations graph.
        let path = corpus("gr", &two_quads("http://purl.org/goodrelations/v1"));

        // Creator passed as an IRI, matching the registry convention.
        let outcome = install_lov_vocab(
            &store,
            base,
            &catalog,
            Some(&path),
            "gr",
            Some("http://localhost:7878/users/admin-1"),
        )
        .expect("install succeeds");
        assert_eq!(outcome.model_id, "gr");
        assert_eq!(outcome.triples, 2);
        // GoodRelations' own graph declares CC BY 3.0 — that, not LOV's
        // CC BY 4.0, is what the install records.
        assert_eq!(outcome.license, vec!["CC BY 3.0".to_string()]);
        assert_eq!(outcome.license_status, Some(LicenseStatus::Open));
        assert_eq!(outcome.license_source, Some(LicenseSource::Graph));
        // Redistributable, so a public reference vocabulary as before.
        assert!(outcome.redistributable);
        assert!(outcome.is_public);
        let record = registry::get_data_model(&store, base, "gr").expect("registered");
        assert!(record.is_public);
        assert!(record.owner_id.is_none());
        let versions = registry::list_versions(&store, base, "gr");
        assert_eq!(versions.len(), 1);
        let notes = versions[0].notes.as_deref().unwrap_or_default();
        assert!(
            notes.contains("Licence declared by the vocabulary: CC BY 3.0."),
            "{notes}"
        );
        assert!(
            notes.contains("Installed as a public vocabulary"),
            "{notes}"
        );
        assert!(!notes.contains("CC BY 4.0"), "{notes}");

        // Re-install is a conflict, not a duplicate.
        let again = install_lov_vocab(&store, base, &catalog, Some(&path), "gr", None);
        assert!(matches!(again, Err(InstallError::AlreadyInstalled(_))));
    }

    #[test]
    fn unredistributable_vocab_from_the_full_dump_installs_private() {
        let catalog = VocabCatalog::bundled();
        let store = TripleStore::in_memory().unwrap();
        let base = "http://localhost:7878";
        // The full dump holds vocabularies the image leaves out, such as
        // REACT, whose own graph declares CC BY-NC 4.0.
        let react = catalog.lov_by_prefix("react").expect("react").clone();
        assert!(!react.redistributable);
        let path = corpus("react", &two_quads(&react.uri));

        let outcome = install_lov_vocab(
            &store,
            base,
            &catalog,
            Some(&path),
            "react",
            Some("http://localhost:7878/users/admin-1"),
        )
        .expect("install succeeds");
        assert!(!outcome.redistributable);
        assert!(!outcome.is_public);
        assert!(outcome.note.contains("private"), "{}", outcome.note);
        assert!(outcome.note.contains("CC BY-NC 4.0"), "{}", outcome.note);

        // Private, owned by the installing admin.
        let record = registry::get_data_model(&store, base, &outcome.model_id).expect("record");
        assert!(!record.is_public);
        assert_eq!(record.owner_type.as_deref(), Some("user"));
        assert_eq!(record.owner_id.as_deref(), Some("admin-1"));
        let versions = registry::list_versions(&store, base, &outcome.model_id);
        let notes = versions[0].notes.as_deref().unwrap_or_default();
        assert!(
            notes.contains("Installed as a private vocabulary because its licence does not allow redistribution"),
            "{notes}"
        );

        // No licence at all is not redistributable either (no installer
        // given: private, admins only).
        let void = catalog.lov_by_prefix("void").expect("void").clone();
        assert_eq!(void.license_status, Some(LicenseStatus::Undeclared));
        let path = corpus("void", &two_quads(&void.uri));
        let outcome = install_lov_vocab(&store, base, &catalog, Some(&path), "void", None)
            .expect("install succeeds");
        assert!(!outcome.is_public);
        let record = registry::get_data_model(&store, base, &outcome.model_id).expect("record");
        assert!(!record.is_public);
        assert!(record.owner_id.is_none());
    }

    #[test]
    fn publisher_terms_vocab_installs_public_with_its_terms() {
        let catalog = VocabCatalog::bundled();
        let store = TripleStore::in_memory().unwrap();
        let base = "http://localhost:7878";
        // FOAF's graph names no licence; its specification states CC BY 1.0.
        let foaf = catalog.lov_by_prefix("foaf").expect("foaf").clone();
        let path = corpus("foaf", &two_quads(&foaf.uri));
        let outcome = install_lov_vocab(&store, base, &catalog, Some(&path), "foaf", None)
            .expect("install succeeds");
        assert!(outcome.is_public);
        assert_eq!(outcome.license_source, Some(LicenseSource::PublisherTerms));
        assert_eq!(
            outcome.license_source_url.as_deref(),
            Some("http://xmlns.com/foaf/spec/")
        );
        let notes = registry::list_versions(&store, base, "foaf")[0]
            .notes
            .clone()
            .unwrap_or_default();
        assert!(
            notes.contains("Licence stated by the publisher outside the vocabulary: CC BY 1.0"),
            "{notes}"
        );
    }

    #[test]
    fn missing_graph_of_unredistributable_vocab_explains_why() {
        let catalog = VocabCatalog::bundled();
        let store = TripleStore::in_memory().unwrap();
        // A filtered corpus without VoID, which declares no licence anywhere
        // we could find.
        let void = catalog.lov_by_prefix("void").expect("void in catalog");
        assert!(!void.redistributable);
        let path = corpus("filtered", &two_quads("http://purl.org/goodrelations/v1"));

        let err = install_lov_vocab(
            &store,
            "http://localhost:7878",
            &catalog,
            Some(&path),
            "void",
            None,
        )
        .unwrap_err();
        assert!(matches!(err, InstallError::NotRedistributed(_)), "{err}");
        assert!(err.to_string().contains("VOCAB_CORPUS_PATH"));
    }

    #[test]
    fn license_note_names_the_vocabulary_licence() {
        let catalog = VocabCatalog::bundled();
        let note = |p: &str| license_note(catalog.lov_by_prefix(p).expect(p));
        assert_eq!(note("void"), "The vocabulary declares no licence.");
        assert!(note("react").contains("CC BY-NC 4.0"), "{}", note("react"));
        assert!(note("doap").contains("Edd Dumbill"), "{}", note("doap"));
        let foaf = note("foaf");
        assert!(
            foaf.contains("CC BY 1.0 (http://xmlns.com/foaf/spec/)"),
            "{foaf}"
        );
        let esco = note("esco");
        assert!(
            esco.contains(
                "Required notice: \"This service uses the ESCO classification of the European Commission.\""
            ),
            "{esco}"
        );
        let geop = note("geop");
        assert!(geop.contains("restricts redistribution"), "{geop}");
    }

    #[test]
    fn license_note_carries_the_required_notice_whatever_its_source() {
        let catalog = VocabCatalog::bundled();
        let note = |p: &str| license_note(catalog.lov_by_prefix(p).expect(p));
        // Declared in the graph (MIT), with the rights holder's line.
        let poso = note("poso");
        assert!(
            poso.starts_with("Licence declared by the vocabulary: MIT."),
            "{poso}"
        );
        assert!(
            poso.contains("Required notice: \"Copyright (c) 2021-2025 Maxim Van de Wynckel"),
            "{poso}"
        );
        // W3C's site default, with the notice filled in for this document.
        let rdfs = note("rdfs");
        assert!(
            rdfs.contains("Copyright © 2014 World Wide Web Consortium."),
            "{rdfs}"
        );
        assert!(!rdfs.contains("[$"), "{rdfs}");
    }

    #[test]
    fn withheld_vocab_installs_private_and_says_why() {
        let catalog = VocabCatalog::bundled();
        let store = TripleStore::in_memory().unwrap();
        let base = "http://localhost:7878";
        // drammar is CC BY-ND, but LOV's copy is not a faithful one.
        let drama = catalog.lov_by_prefix("drama").expect("drama").clone();
        assert_eq!(drama.license_status, Some(LicenseStatus::Open));
        let path = corpus("drama", &two_quads(&drama.uri));
        let outcome = install_lov_vocab(&store, base, &catalog, Some(&path), "drama", None)
            .expect("install succeeds");
        assert!(!outcome.is_public);
        assert!(outcome.redistribution_withheld.is_some());
        assert!(
            outcome
                .note
                .contains("because this project does not redistribute it"),
            "{}",
            outcome.note
        );
    }

    #[test]
    fn install_records_the_licence_and_checks_the_copy() {
        let catalog = VocabCatalog::bundled();
        let store = TripleStore::in_memory().unwrap();
        let base = "http://localhost:7878";
        let gr = catalog.lov_by_prefix("gr").expect("gr").clone();
        let path = corpus("record-gr", &two_quads(&gr.uri));
        let outcome = install_lov_vocab(&store, base, &catalog, Some(&path), "gr", None)
            .expect("install succeeds");
        let a = registry::get_attribution(
            &store,
            &registry::version_record_iri(base, "gr", &outcome.version),
        )
        .expect("licence record");
        assert_eq!(
            a.licenses,
            vec![LicenseRef {
                name: "CC BY 3.0".into(),
                uri: "https://creativecommons.org/licenses/by/3.0/".into()
            }]
        );
        assert!(a.unchanged, "loaded verbatim and checked");
        assert!(!a.no_derivatives);
        assert_eq!(a.source_url, gr.uri);
        assert_eq!(a.notice_url, "/api/vocab/notice?vocab=gr");
        assert!(a.stored_copy.contains("unchanged"), "{}", a.stored_copy);
        assert!(!outcome.no_derivatives);

        // CC BY-ND: the record says no altered copies, which the registry's
        // guards read.
        let pna = catalog.lov_by_prefix("pna").expect("pna").clone();
        assert!(pna.no_derivatives && pna.redistributable);
        let path = corpus("record-pna", &two_quads(&pna.uri));
        let outcome = install_lov_vocab(&store, base, &catalog, Some(&path), "pna", None)
            .expect("install succeeds");
        assert!(outcome.no_derivatives && outcome.is_public);
        let a = registry::get_attribution(
            &store,
            &registry::version_record_iri(base, "pna", &outcome.version),
        )
        .expect("licence record");
        assert!(a.no_derivatives && a.unchanged);
        assert_eq!(a.licenses[0].name, "CC BY-ND 3.0");
        assert_eq!(
            a.licenses[0].uri,
            "https://creativecommons.org/licenses/by-nd/3.0/"
        );
    }

    #[test]
    fn notice_page_names_licences_notice_and_origin() {
        let catalog = VocabCatalog::bundled();
        let text = catalog.notice_text(catalog.lov_by_prefix("locn").expect("locn"));
        assert!(
            text.contains(
                "Licence: ISA Open Metadata Licence 1.1, \
                 https://interoperable-europe.ec.europa.eu/licence/isa-open-metadata-licence-v11"
            ),
            "{text}"
        );
        assert!(text.contains("No Warranty: EACH WORK"), "{text}");
        assert!(text.contains("Linked Open Vocabularies (LOV)"), "{text}");
        let react = catalog.notice_text(catalog.lov_by_prefix("react").expect("react"));
        assert!(react.contains("does not redistribute it"), "{react}");
    }

    #[test]
    fn install_checks_the_copy_as_the_store_holds_it() {
        let catalog = VocabCatalog::bundled();
        let store = TripleStore::in_memory().unwrap();
        let base = "http://localhost:7878";
        // An OWL cardinality and a +00:00 dateTime: the store keeps both in
        // its canonical form, with the same values.
        let pna = catalog.lov_by_prefix("pna").expect("pna").clone();
        let nq = format!(
            "{}_:r <http://www.w3.org/2002/07/owl#cardinality> \
             \"1\"^^<http://www.w3.org/2001/XMLSchema#nonNegativeInteger> <{u}> .\n\
             <{u}> <http://purl.org/dc/terms/modified> \
             \"2014-08-28T15:00:00+00:00\"^^<http://www.w3.org/2001/XMLSchema#dateTime> <{u}> .\n",
            two_quads(&pna.uri),
            u = pna.uri
        );
        let path = corpus("typed-pna", &nq);
        let outcome = install_lov_vocab(&store, base, &catalog, Some(&path), "pna", None)
            .expect("install succeeds");
        assert!(outcome.is_public && outcome.no_derivatives);
        let a = registry::get_attribution(
            &store,
            &registry::version_record_iri(base, "pna", &outcome.version),
        )
        .expect("licence record");
        assert!(a.no_derivatives && a.unchanged, "{}", a.stored_copy);
        assert!(
            a.stored_copy.contains("canonical form"),
            "{}",
            a.stored_copy
        );

        // Without typed literals the record does not mention it.
        let gr = catalog.lov_by_prefix("gr").expect("gr").clone();
        let path = corpus("typed-gr", &two_quads(&gr.uri));
        let outcome = install_lov_vocab(&store, base, &catalog, Some(&path), "gr", None)
            .expect("install succeeds");
        let a = registry::get_attribution(
            &store,
            &registry::version_record_iri(base, "gr", &outcome.version),
        )
        .expect("licence record");
        assert!(a.unchanged);
        assert!(!a.stored_copy.contains("canonical"), "{}", a.stored_copy);
    }

    const BASE: &str = "http://localhost:7878";
    const INSTALLER: &str = "http://localhost:7878/users/admin-7";

    /// No user is known to this instance as a non-admin.
    fn no_non_admins(_: &str) -> bool {
        false
    }

    /// How an entry the earlier installer might have made differs from one
    /// it did make.
    struct Earlier<'a> {
        id: &'a str,
        nsp: &'a str,
        owner: Option<&'a str>,
        creator: Option<&'a str>,
        note: &'a str,
        graph: Option<&'a str>,
    }

    impl<'a> Earlier<'a> {
        /// Exactly what the earlier installer made for `vocab` (whose
        /// prefix is lowercase, so it is also the registry id).
        fn of(vocab: &'a LovVocab) -> Self {
            assert_eq!(vocab.prefix, vocab.prefix.to_lowercase());
            Earlier {
                id: &vocab.prefix,
                nsp: &vocab.nsp,
                owner: None,
                creator: Some(INSTALLER),
                note: LEGACY_NOTE,
                graph: None,
            }
        }

        /// Register it: public, the graph loaded as the earlier loader did
        /// (an `owl:versionInfo` triple added), version 2.0.
        fn register(&self, store: &TripleStore, quads: &str) {
            let version = "2.0";
            registry::insert_data_model(
                store,
                BASE,
                self.id,
                "Earlier",
                self.nsp,
                None,
                true,
                self.owner.map(|_| "user"),
                self.owner,
                self.creator,
                "2024-01-01T00:00:00Z",
            )
            .unwrap();
            let parsed = upload::parse_quads(quads.as_bytes(), oxigraph::io::RdfFormat::NQuads)
                .expect("fixture parses");
            let loaded =
                upload::load_parsed(store, BASE, self.id, Some(version), parsed, true).unwrap();
            let conventional = registry::version_record_iri(BASE, self.id, version);
            let graph = self.graph.map(str::to_string).unwrap_or(conventional);
            registry::insert_version(
                store,
                BASE,
                &DataModelVersion {
                    data_model_id: self.id.to_string(),
                    version: version.to_string(),
                    status: VersionStatus::Published,
                    graph_iri: graph.clone(),
                    sub_graphs: if self.graph.is_some() {
                        vec![graph]
                    } else {
                        loaded.sub_graphs
                    },
                    created_at: "2024-01-01T00:00:00Z".into(),
                    created_by: self.creator.map(str::to_string),
                    derived_from: None,
                    notes: Some(self.note.to_string()),
                    branch: None,
                    sub_graph_status: Vec::new(),
                },
            )
            .unwrap();
            registry::update_latest_published(store, BASE, self.id, version).unwrap();
        }
    }

    fn triple_count(store: &TripleStore, graph: &str) -> usize {
        content_digest::graph_triples(store, graph).unwrap().len()
    }

    fn note_of(store: &TripleStore, id: &str) -> String {
        registry::get_version(store, BASE, id, "2.0")
            .unwrap()
            .notes
            .unwrap_or_default()
    }

    fn record_of(store: &TripleStore, id: &str) -> Option<ContentAttribution> {
        registry::get_attribution(store, &registry::version_record_iri(BASE, id, "2.0"))
    }

    #[test]
    fn earlier_installs_are_made_private_and_labelled_and_nothing_else_changes() {
        let catalog = VocabCatalog::bundled();
        let store = TripleStore::in_memory().unwrap();
        let react = catalog.lov_by_prefix("react").expect("react").clone();
        let gr = catalog.lov_by_prefix("gr").expect("gr").clone();
        let pna = catalog.lov_by_prefix("pna").expect("pna").clone();
        assert!(!react.redistributable && gr.redistributable && !gr.no_derivatives);
        assert!(pna.redistributable && pna.no_derivatives);
        for v in [&react, &gr, &pna] {
            Earlier::of(v).register(&store, &two_quads(&v.uri));
            let graph = registry::version_record_iri(BASE, &v.prefix, "2.0");
            assert_eq!(triple_count(&store, &graph), 3, "the added triple");
        }

        // No corpus is needed, and none is read.
        let done = migrate_legacy_installs(&store, BASE, &catalog, &no_non_admins);
        assert_eq!(done.len(), 3, "{done:?}");
        let by = |id: &str| done.iter().find(|d| d.model_id == id).expect(id).clone();

        // NonCommercial: private, with no owner invented (admins only).
        let r = by("react");
        assert!(r.made_private && r.recorded_licence);
        let rec = registry::get_data_model(&store, BASE, "react").unwrap();
        assert!(!rec.is_public);
        assert!(rec.owner_type.is_none() && rec.owner_id.is_none());
        let a = record_of(&store, "react").expect("licence record");
        assert!(!a.unchanged && !a.no_derivatives);
        assert_eq!(a.licenses[0].name, "CC BY-NC 4.0");
        let changes = a.changes.clone().unwrap();
        assert!(!changes.contains("None by Open Triplestore"), "{changes}");
        assert!(changes.contains("owl:versionInfo \"2.0\""), "{changes}");
        assert!(changes.contains("snapshot 2025-12-18"), "{changes}");
        assert!(
            a.stored_copy.contains("may have been modified"),
            "{}",
            a.stored_copy
        );

        // CC BY 3.0: stays public, and its record names its own licence.
        let g = by("gr");
        assert!(!g.made_private && g.recorded_licence);
        assert!(
            registry::get_data_model(&store, BASE, "gr")
                .unwrap()
                .is_public
        );
        let a = record_of(&store, "gr").expect("licence record");
        assert!(!a.unchanged);
        assert_eq!(
            a.licenses[0].uri,
            "https://creativecommons.org/licenses/by/3.0/"
        );

        // CC BY-ND: not shown to be an unaltered copy, so private, and the
        // registry's no-derivatives guards now apply to the entry.
        let p = by("pna");
        assert!(p.made_private && p.recorded_licence);
        let a = record_of(&store, "pna").expect("licence record");
        assert!(a.no_derivatives && !a.unchanged);
        assert!(registry::no_derivatives_attribution(&store, BASE, "pna").is_some());

        // Graphs and notes are as they were: nothing removed, nothing rewritten.
        for id in ["react", "gr", "pna"] {
            let graph = registry::version_record_iri(BASE, id, "2.0");
            assert_eq!(triple_count(&store, &graph), 3, "{id}");
            assert_eq!(note_of(&store, id), LEGACY_NOTE, "{id}");
        }

        // Every later boot: nothing to do.
        assert!(migrate_legacy_installs(&store, BASE, &catalog, &no_non_admins).is_empty());

        // An admin who checked the terms makes one public again: it stays so.
        registry::update_data_model(
            &store,
            BASE,
            "react",
            None,
            None,
            None,
            Some(true),
            None,
            None,
        )
        .unwrap();
        assert!(migrate_legacy_installs(&store, BASE, &catalog, &no_non_admins).is_empty());
        assert!(
            registry::get_data_model(&store, BASE, "react")
                .unwrap()
                .is_public
        );
    }

    #[test]
    fn only_what_the_earlier_installer_made_is_taken_for_an_install() {
        let catalog = VocabCatalog::bundled();
        let store = TripleStore::in_memory().unwrap();
        let v = |p: &str| catalog.lov_by_prefix(p).expect(p).clone();
        let (gml, sf, pnc, pne, pni, pns, react, pnt) = (
            v("gml"),
            v("sf"),
            v("pnc"),
            v("pne"),
            v("pni"),
            v("pns"),
            v("react"),
            v("pnt"),
        );
        let edited = format!("{LEGACY_NOTE} Checked by X.");
        // Each differs from what the installer made in one way, named by the
        // reason it is not taken for an install.
        let cases = [
            // A user's own model, with the earlier installer's note copied.
            (
                Earlier {
                    owner: Some("u1"),
                    creator: Some("http://localhost:7878/users/u1"),
                    ..Earlier::of(&gml)
                },
                "has an owner",
            ),
            // Not a LOV prefix, although the namespace is LOV's.
            (
                Earlier {
                    id: "sf-ext",
                    ..Earlier::of(&sf)
                },
                "not the prefix of a vocabulary",
            ),
            // A LOV prefix with another namespace.
            (
                Earlier {
                    nsp: "http://example.org/pnc#",
                    ..Earlier::of(&pnc)
                },
                "namespace",
            ),
            // The note was edited: left to an admin.
            (
                Earlier {
                    note: &edited,
                    ..Earlier::of(&pne)
                },
                "note is not exactly",
            ),
            // The version graph is not where the installer loaded it.
            (
                Earlier {
                    graph: Some("http://example.org/elsewhere"),
                    ..Earlier::of(&pni)
                },
                "graph is not the one",
            ),
            // The creator is a user of this instance who is not an admin.
            (
                Earlier {
                    creator: Some("http://localhost:7878/users/user-3"),
                    ..Earlier::of(&pns)
                },
                "not an admin",
            ),
            // The creator is not a user of this instance.
            (
                Earlier {
                    creator: Some("http://elsewhere.example/users/admin-7"),
                    ..Earlier::of(&react)
                },
                "not a user of this instance",
            ),
        ];
        for (c, _) in &cases {
            c.register(&store, &two_quads(&format!("http://example.org/{}", c.id)));
        }
        // And one made exactly as the installer made it.
        Earlier::of(&pnt).register(&store, &two_quads(&pnt.uri));
        let non_admin = |u: &str| u == "user-3";

        let rows = legacy_note_rows(&store).unwrap();
        assert_eq!(rows.len(), cases.len() + 1);
        for (c, reason) in &cases {
            let row = rows
                .iter()
                .find(|r| {
                    r.model_iri
                        .as_deref()
                        .is_some_and(|m| m.ends_with(&format!("/{}", c.id)))
                })
                .expect(c.id);
            let why = legacy_install(&store, BASE, &catalog, &non_admin, row, c.id)
                .err()
                .unwrap_or_else(|| panic!("{} taken for an install", c.id));
            assert!(why.contains(reason), "{}: {why}", c.id);
        }

        let done = migrate_legacy_installs(&store, BASE, &catalog, &non_admin);
        assert_eq!(done.len(), 1, "{done:?}");
        assert_eq!(done[0].model_id, "pnt");
        assert!(done[0].made_private && done[0].recorded_licence);
        for (c, _) in &cases {
            let rec = registry::get_data_model(&store, BASE, c.id).unwrap();
            assert!(rec.is_public, "{} stays public", c.id);
            assert!(record_of(&store, c.id).is_none(), "{} gets no record", c.id);
            assert!(
                registry::no_derivatives_attribution(&store, BASE, c.id).is_none(),
                "{} is not locked",
                c.id
            );
            assert_eq!(note_of(&store, c.id), c.note, "{} keeps its note", c.id);
        }
        assert_eq!(
            registry::get_data_model(&store, BASE, "gml")
                .unwrap()
                .owner_id
                .as_deref(),
            Some("u1")
        );
    }

    #[test]
    fn a_boot_cut_short_is_finished_by_the_next_and_loses_nothing() {
        let catalog = VocabCatalog::bundled();
        let store = TripleStore::in_memory().unwrap();
        let react = catalog.lov_by_prefix("react").expect("react").clone();
        let gr = catalog.lov_by_prefix("gr").expect("gr").clone();
        Earlier::of(&react).register(&store, &two_quads(&react.uri));
        Earlier::of(&gr).register(&store, &two_quads(&gr.uri));

        // Cut short after making 'react' private, before its mark and record.
        registry::update_data_model(
            &store,
            BASE,
            "react",
            None,
            None,
            None,
            Some(false),
            None,
            None,
        )
        .unwrap();
        let done = migrate_legacy_installs(&store, BASE, &catalog, &no_non_admins);
        let r = done.iter().find(|d| d.model_id == "react").expect("react");
        assert!(!r.made_private && r.recorded_licence);
        assert!(record_of(&store, "react").is_some());

        // Cut short after the mark, before the record, and an admin has made
        // it public since: the record is written, the admin's choice kept.
        registry::set_attribution(
            &store,
            &registry::version_record_iri(BASE, "react", "2.0"),
            None,
        )
        .unwrap();
        registry::update_data_model(
            &store,
            BASE,
            "react",
            None,
            None,
            None,
            Some(true),
            None,
            None,
        )
        .unwrap();
        let done = migrate_legacy_installs(&store, BASE, &catalog, &no_non_admins);
        assert_eq!(done.len(), 1, "{done:?}");
        assert!(!done[0].made_private && done[0].recorded_licence);
        assert!(
            registry::get_data_model(&store, BASE, "react")
                .unwrap()
                .is_public
        );

        for id in ["react", "gr"] {
            let graph = registry::version_record_iri(BASE, id, "2.0");
            assert_eq!(triple_count(&store, &graph), 3, "{id}");
            assert_eq!(note_of(&store, id), LEGACY_NOTE, "{id}");
        }
    }

    #[test]
    fn the_migrations_own_record_is_kept_current_and_another_is_kept() {
        let catalog = VocabCatalog::bundled();
        let store = TripleStore::in_memory().unwrap();
        let gr = catalog.lov_by_prefix("gr").expect("gr").clone();
        Earlier::of(&gr).register(&store, &two_quads(&gr.uri));
        migrate_legacy_installs(&store, BASE, &catalog, &no_non_admins);
        let ver_iri = registry::version_record_iri(BASE, "gr", "2.0");

        // An earlier build's wording of the migration's own record.
        let mut older = record_of(&store, "gr").unwrap();
        older.remarks = Some("Older wording.".into());
        registry::set_attribution(&store, &ver_iri, Some(&older)).unwrap();
        let done = migrate_legacy_installs(&store, BASE, &catalog, &no_non_admins);
        assert!(done.len() == 1 && done[0].recorded_licence);
        assert_eq!(
            record_of(&store, "gr"),
            Some(legacy_attribution(&gr, "2.0"))
        );

        // A record the migration did not write stays as it is.
        let mut other = older.clone();
        other.file = "other.ttl".into();
        other.changes = Some("Someone else's.".into());
        registry::set_attribution(&store, &ver_iri, Some(&other)).unwrap();
        assert!(migrate_legacy_installs(&store, BASE, &catalog, &no_non_admins).is_empty());
        assert_eq!(record_of(&store, "gr"), Some(other));
    }

    #[test]
    fn a_read_only_store_is_left_alone() {
        use crate::store::replication::{Mode, ReplicationConfig, Scope};
        let catalog = VocabCatalog::bundled();
        let store = TripleStore::in_memory().unwrap();
        let react = catalog.lov_by_prefix("react").expect("react").clone();
        Earlier::of(&react).register(&store, &two_quads(&react.uri));
        let store = store.with_replication(ReplicationConfig::follower(
            "http://leader.example",
            Mode::Warm,
            Scope::All,
        ));
        assert!(store.replication().read_only());
        assert!(migrate_legacy_installs(&store, BASE, &catalog, &no_non_admins).is_empty());
        assert!(
            registry::get_data_model(&store, BASE, "react")
                .unwrap()
                .is_public
        );
        assert!(record_of(&store, "react").is_none());
    }

    #[test]
    fn legacy_note_is_the_one_every_earlier_release_wrote() {
        // The catalogue every earlier release shipped was of that snapshot,
        // and their installer wrote it into this sentence.
        assert_eq!(
            LEGACY_NOTE,
            format!("{LEGACY_NOTE_START}snapshot {LEGACY_SNAPSHOT}, CC BY 4.0).")
        );
    }

    #[test]
    fn version_label_sanitizes() {
        assert_eq!(version_label(Some("v2.3"), None), "2.3");
        assert_eq!(version_label(Some("v2015-07-22"), None), "2015-07-22");
        // "W3C Recommendation" contains spaces → falls back to the issued date.
        assert_eq!(
            version_label(Some("W3C Recommendation"), Some("2016-11-12")),
            "2016-11-12"
        );
        assert_eq!(version_label(None, None), "lov");
    }
}
