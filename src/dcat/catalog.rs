//! DCAT 3 catalogue generation — plain DCAT, DCAT-AP 3 or DCAT-AP-NL 3 —
//! from the dataset registry and the store.
//!
//! The catalogue is built as an RDF graph (a `Vec<Triple>`, through
//! [`super::graph::G`]) and serialised per request in whatever format was
//! negotiated. Every user-supplied value becomes a real RDF term: a title with
//! a quote or a description with a `>` cannot corrupt the document, and a
//! malformed IRI (a theme, a licence, a homepage) is dropped with a warning
//! instead of being interpolated.
//!
//! What is emitted:
//! - the `dcat:Catalog` (title, description, publisher agent, contact point,
//!   language, licence, issued/modified, homepage, theme taxonomy, its data
//!   services) and its datasets;
//! - a `dcat:Dataset` + `void:Dataset` per registered dataset the caller may
//!   see: Dublin Core, access rights, publisher and creator agents, contact
//!   point, temporal coverage, update frequency, PROV attribution, model and
//!   shapes conformance, per-graph `void:subset`s with roles, VoID counts, and
//!   distributions (SPARQL, Graph Store, one download per graph, LDES when
//!   published, OGC API – Features / 3D Tiles / viewer feed when there is
//!   geometry);
//! - its released versions (published or deprecated) as DCAT 3 §11 versions:
//!   each a `dcat:Dataset` with `dcat:isVersionOf`, `dcat:version`,
//!   `dcat:previousVersion`, `dct:issued` and `adms:versionNotes`, linked from
//!   the dataset by `dcat:hasVersion`, the newest by `dcat:hasCurrentVersion`;
//! - the SPARQL endpoint (and the OGC API when a dataset has geometry) as a
//!   `dcat:DataService` with `dcat:servesDataset`, its endpoint description,
//!   publisher, contact point, access rights and licence;
//! - DCAT range typing on every object (a licence is a `dct:LicenseDocument`,
//!   a theme a labelled `skos:Concept`, …; see [`super::graph`]);
//! - under an application profile, what DCAT-AP 3 and DCAT-AP-NL 3 make
//!   mandatory or recommend: `dct:identifier` + `adms:identifier`,
//!   `dct:language`, `dct:format` (EU file-type authority) on every
//!   distribution, licences repeated on distributions, and a
//!   `dcat:CatalogRecord` per dataset;
//! - VoID per dataset, over the dataset's graphs the caller may read:
//!   statistics, class and property partitions, vocabularies, example
//!   resources, features, data dumps, and its linkset-role graphs as
//!   `void:Linkset`s with their link predicates and targets;
//! - the aggregate `void:Dataset` for the whole store with statistics over the
//!   graphs the caller may read, cached until the next write — counts only,
//!   never partitions.
//!
//! `dcat:DatasetSeries` is not emitted: the product has no series concept (a
//! dataset's versions are versions, DCAT 3 §11, not members of a series).
//!
//! Nothing is invented to satisfy a profile. What a profile requires that the
//! registry does not hold — a theme, a contact point, a licence — is left out
//! and reported in [`CatalogReport::warnings`] (logged once per message).

use std::collections::HashSet;
use std::sync::Arc;

use oxigraph::io::RdfFormat;
use oxigraph::model::{BlankNode, NamedNode, NamedOrBlankNode, Triple};
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};

use crate::auth::db::AuthDb;
use crate::auth::models::{Dataset, Organisation, OwnerType, Visibility};
use crate::dataset_versions::models::{DatasetVersion, VersionStatus};
use crate::store::engine::TripleStore;

use super::authority;
use super::graph::{nn, p, Range, G, OTS, SKOS};
use super::vocabulary::*;

const EU_LANG: &str = "http://publications.europa.eu/resource/authority/language/";
const EU_FILETYPE: &str = "http://publications.europa.eu/resource/authority/file-type/";
const IANA: &str = "https://www.iana.org/assignments/media-types/";
const SPARQL_PROTOCOL: &str = "https://www.w3.org/TR/sparql11-protocol/";
const GSP_PROTOCOL: &str = "https://www.w3.org/TR/sparql11-http-rdf-update/";
const LDES_SPEC: &str = "https://w3id.org/ldes/specification";
const OGC_FEATURES: &str = "http://www.opengis.net/spec/ogcapi-features-1/1.0/conf/core";
const TILES3D: &str = "https://docs.ogc.org/cs/22-025r4/22-025r4.html";
/// The DCAT-AP 3.0.1 release, as its SHACL shapes name it.
pub const DCAT_AP_301: &str = "https://semiceu.github.io/DCAT-AP/releases/3.0.1";
/// The DCAT-AP-NL 3 specification.
pub const DCAT_AP_NL_3: &str = "https://geonovum.github.io/DCAT-AP-NL30/";

// ── profile & options ───────────────────────────────────────────────────────

/// The application profile the catalogue follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    /// DCAT 3 with VoID statistics.
    Dcat,
    /// DCAT-AP 3.
    DcatAp,
    /// DCAT-AP-NL 3 (on top of DCAT-AP).
    DcatApNl,
}

impl Profile {
    /// Parse a `DCAT_PROFILE` value; empty means `dcat`.
    pub fn parse(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "" | "dcat" | "dcat3" | "dcat-3" => Ok(Profile::Dcat),
            "dcat-ap" | "dcat_ap" | "dcatap" => Ok(Profile::DcatAp),
            "dcat-ap-nl" | "dcat_ap_nl" | "dcatapnl" => Ok(Profile::DcatApNl),
            other => Err(format!(
                "DCAT_PROFILE `{other}` is not a catalogue profile: use dcat, dcat-ap or dcat-ap-nl"
            )),
        }
    }
    /// The profile `DCAT_PROFILE` names. An invalid value is refused at
    /// startup ([`check_env`]); should one reach here anyway, it is `dcat`.
    pub fn from_env() -> Self {
        Self::parse(&std::env::var("DCAT_PROFILE").unwrap_or_default()).unwrap_or(Profile::Dcat)
    }
    pub fn is_ap(self) -> bool {
        !matches!(self, Profile::Dcat)
    }
    fn is_nl(self) -> bool {
        matches!(self, Profile::DcatApNl)
    }
    /// The application profile a catalogue record declares (`dct:conformsTo`).
    fn standard(self) -> Option<&'static str> {
        match self {
            Profile::Dcat => None,
            Profile::DcatAp => Some(DCAT_AP_301),
            Profile::DcatApNl => Some(DCAT_AP_NL_3),
        }
    }
    fn name(self) -> &'static str {
        match self {
            Profile::Dcat => "DCAT 3",
            Profile::DcatAp => "DCAT-AP 3",
            Profile::DcatApNl => "DCAT-AP-NL 3",
        }
    }
}

/// Validate the catalogue settings at startup, reporting every problem at
/// once: an unknown `DCAT_PROFILE`, a `CATALOG_PUBLISHER_TYPE` outside the
/// ADMS publisher types, a `CATALOG_LANGUAGE` that is not an ISO 639-3 code,
/// and IRIs that are not IRIs.
pub fn check_env() -> Result<(), String> {
    let mut errors = Vec::new();
    if let Err(e) = Profile::parse(&std::env::var("DCAT_PROFILE").unwrap_or_default()) {
        errors.push(e);
    }
    if let Some(t) = env_nonempty("CATALOG_PUBLISHER_TYPE") {
        if authority::publisher_type_iri(&t)
            .and_then(|i| NamedNode::new(i).ok())
            .is_none()
        {
            errors.push(format!(
                "CATALOG_PUBLISHER_TYPE `{t}` is not an ADMS publisher type (e.g. LocalAuthority, \
                 NationalAuthority, Company) or an IRI"
            ));
        }
    }
    if let Some(l) = env_nonempty("CATALOG_LANGUAGE") {
        if l.len() != 3 || !l.chars().all(|c| c.is_ascii_alphabetic()) {
            errors.push(format!(
                "CATALOG_LANGUAGE `{l}` is not an ISO 639-3 code (ENG, NLD, …)"
            ));
        }
    }
    for k in ["CATALOG_PUBLISHER_URI", "CATALOG_LICENSE"] {
        if let Some(v) = env_nonempty(k) {
            if NamedNode::new(v.as_str()).is_err() {
                errors.push(format!("{k} `{v}` is not an absolute IRI"));
            }
        }
    }
    for k in ["OTS_VOID_PARTITION_LIMIT", "OTS_VOID_PARTITION_MAX_TRIPLES"] {
        if let Some(v) = env_nonempty(k) {
            if v.parse::<usize>().is_err() {
                errors.push(format!("{k} `{v}` is not a whole number"));
            }
        }
    }
    if let Some(e) = env_nonempty("CATALOG_CONTACT_EMAIL") {
        if !e.contains('@') || NamedNode::new(format!("mailto:{e}")).is_err() {
            errors.push(format!(
                "CATALOG_CONTACT_EMAIL `{e}` is not an e-mail address"
            ));
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

/// Catalogue-level metadata, from the environment ([`CatalogOptions::from_env`])
/// or set explicitly ([`CatalogOptions::new`] plus field updates).
#[derive(Debug, Clone)]
pub struct CatalogOptions {
    pub base_url: String,
    pub profile: Profile,
    pub title: String,
    pub description: String,
    pub publisher_iri: String,
    pub publisher_name: String,
    pub publisher_identifier: Option<String>,
    /// ADMS publisher type of the catalogue's publisher (an IRI).
    pub publisher_type: Option<String>,
    /// ISO 639-3, upper case (`ENG`, `NLD`).
    pub language: String,
    pub license: Option<String>,
    /// The catalogue's and its data services' contact point (`vcard:fn`).
    pub contact_name: Option<String>,
    /// The catalogue's and its data services' contact e-mail (`vcard:hasEmail`).
    pub contact_email: Option<String>,
}

fn env_nonempty(k: &str) -> Option<String> {
    std::env::var(k)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn host_of(base: &str) -> String {
    base.split("://")
        .nth(1)
        .unwrap_or(base)
        .trim_end_matches('/')
        .to_string()
}

impl CatalogOptions {
    /// The defaults for `base_url` under `profile`, reading no environment.
    pub fn new(base_url: &str, profile: Profile) -> Self {
        let base = base_url.trim_end_matches('/').to_string();
        let host = host_of(&base);
        Self {
            profile,
            title: "Open Triplestore Catalog".into(),
            description: format!("Datasets published by the Open Triplestore instance at {host}."),
            publisher_iri: format!("{base}/publisher"),
            publisher_name: format!("Open Triplestore instance at {host}"),
            publisher_identifier: None,
            publisher_type: None,
            language: if profile == Profile::DcatApNl {
                "NLD".into()
            } else {
                "ENG".into()
            },
            license: None,
            contact_name: None,
            contact_email: None,
            base_url: base,
        }
    }

    /// The options `DCAT_PROFILE` and the `CATALOG_*` settings describe.
    pub fn from_env(base_url: &str) -> Self {
        let mut o = Self::new(base_url, Profile::from_env());
        if let Some(v) = env_nonempty("CATALOG_TITLE") {
            o.title = v;
        }
        if let Some(v) = env_nonempty("CATALOG_DESCRIPTION") {
            o.description = v;
        }
        if let Some(v) = env_nonempty("CATALOG_PUBLISHER_URI") {
            o.publisher_iri = v;
        }
        if let Some(v) = env_nonempty("CATALOG_PUBLISHER_NAME") {
            o.publisher_name = v;
        }
        o.publisher_identifier = env_nonempty("CATALOG_PUBLISHER_IDENTIFIER");
        o.publisher_type =
            env_nonempty("CATALOG_PUBLISHER_TYPE").and_then(|t| authority::publisher_type_iri(&t));
        if let Some(v) = env_nonempty("CATALOG_LANGUAGE") {
            o.language = v.to_ascii_uppercase();
        }
        o.license = env_nonempty("CATALOG_LICENSE");
        o.contact_name = env_nonempty("CATALOG_CONTACT_NAME");
        o.contact_email = env_nonempty("CATALOG_CONTACT_EMAIL");
        o
    }

    pub(crate) fn lang_tag(&self) -> &'static str {
        match self.language.as_str() {
            "NLD" => "nl",
            "DEU" => "de",
            "FRA" => "fr",
            "SPA" => "es",
            "ITA" => "it",
            "POR" => "pt",
            "DAN" => "da",
            "SWE" => "sv",
            "NOR" => "no",
            "FIN" => "fi",
            "POL" => "pl",
            _ => "en",
        }
    }
    pub(crate) fn language_iri(&self) -> String {
        format!("{EU_LANG}{}", self.language)
    }
    fn contact(&self) -> Option<Contact> {
        Contact::new(
            self.contact_name.as_deref(),
            self.contact_email.as_deref(),
            None,
        )
    }
}

/// A contact point's fields; at least one is set.
#[derive(Debug, Clone)]
struct Contact {
    name: Option<String>,
    email: Option<String>,
    url: Option<String>,
}

impl Contact {
    fn new(name: Option<&str>, email: Option<&str>, url: Option<&str>) -> Option<Self> {
        let f = |v: Option<&str>| {
            v.map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        };
        let c = Contact {
            name: f(name),
            email: f(email),
            url: f(url),
        };
        (c.name.is_some() || c.email.is_some() || c.url.is_some()).then_some(c)
    }
}

fn enc(s: &str) -> String {
    utf8_percent_encode(s, NON_ALPHANUMERIC).to_string()
}

// ── public API ──────────────────────────────────────────────────────────────

/// A built catalogue and the profile warnings met while building it.
#[derive(Debug, Clone)]
pub struct CatalogReport {
    pub triples: Vec<Triple>,
    /// What the profile asks for that the registry does not hold (a theme, a
    /// contact point, a licence, a concept label), one message each.
    pub warnings: Vec<String>,
}

/// The catalogue as triples: the whole instance, or one organisation's slice.
#[allow(dead_code)] // library and test entry point; the server uses the report
pub fn build_catalog(
    opts: &CatalogOptions,
    store: &TripleStore,
    auth_db: &Arc<AuthDb>,
    user_id: Option<&str>,
    scope: Option<&Organisation>,
) -> Vec<Triple> {
    build_catalog_report(opts, store, auth_db, user_id, scope).triples
}

/// What a data service serves: the datasets and their themes.
#[derive(Default)]
struct Served {
    datasets: Vec<NamedNode>,
    themes: Vec<String>,
}

impl Served {
    fn add(&mut self, ds: NamedNode, themes: &[String]) {
        self.datasets.push(ds);
        for t in themes {
            if !self.themes.contains(t) {
                self.themes.push(t.clone());
            }
        }
    }
}

/// The catalogue and its profile warnings.
pub fn build_catalog_report(
    opts: &CatalogOptions,
    store: &TripleStore,
    auth_db: &Arc<AuthDb>,
    user_id: Option<&str>,
    scope: Option<&Organisation>,
) -> CatalogReport {
    let base = opts.base_url.as_str();
    let mut g = G::new(opts.lang_tag());

    let datasets: Vec<Dataset> = match scope {
        Some(org) => auth_db.list_datasets_by_org(&org.id).unwrap_or_default(),
        None => auth_db.list_datasets().unwrap_or_default(),
    }
    .into_iter()
    .filter(|ds| auth_db.can_access_dataset(user_id, ds).unwrap_or(false))
    .collect();
    let readable = readable_graphs(auth_db, user_id);

    // ── the catalogue ──
    let catalog = nn(&match scope {
        Some(org) => format!("{base}/{}/catalog", org.slug),
        None => format!("{base}/catalog"),
    });
    g.typ(catalog.clone(), &p(DCAT, "Catalog"));
    let title = match scope {
        Some(org) => format!("{} Catalog", org.name),
        None => opts.title.clone(),
    };
    g.lang(catalog.clone(), &p(DCT, "title"), &title, opts.lang_tag());
    let description = match scope {
        Some(org) => org
            .description
            .clone()
            .unwrap_or_else(|| format!("Datasets published by {}.", org.name)),
        None => opts.description.clone(),
    };
    g.lang(
        catalog.clone(),
        &p(DCT, "description"),
        &description,
        opts.lang_tag(),
    );
    let publisher = match scope {
        Some(org) => org_agent(&mut g, opts, org),
        None => catalog_publisher(&mut g, opts),
    };
    g.add(catalog.clone(), &p(DCT, "publisher"), publisher.clone());
    g.ranged(
        catalog.clone(),
        &p(DCT, "language"),
        &opts.language_iri(),
        Range::LinguisticSystem,
        "language",
    );
    if let Some(l) = &opts.license {
        g.ranged(
            catalog.clone(),
            &p(DCT, "license"),
            l,
            Range::LicenseDocument,
            "catalogue licence",
        );
    }
    g.ranged(
        catalog.clone(),
        &p(FOAF, "homepage"),
        &match scope {
            Some(org) => format!("{base}/{}/", org.slug),
            None => format!("{base}/"),
        },
        Range::Document,
        "homepage",
    );
    let themes = nn(authority::EU_THEME_SCHEME);
    g.add(catalog.clone(), &p(DCAT, "themeTaxonomy"), themes.clone());
    g.typ(themes.clone(), &p(SKOS, "ConceptScheme"));
    g.lang(themes, &p(DCT, "title"), "Data theme", "en");
    let catalog_contact = match scope {
        Some(org) => Contact::new(
            org.contact_name.as_deref(),
            org.contact_email.as_deref(),
            org.contact_url.as_deref(),
        )
        .or_else(|| opts.contact()),
        None => opts.contact(),
    };
    match &catalog_contact {
        Some(c) => contact_point(&mut g, catalog.clone(), c),
        None if opts.profile.is_nl() => g.warn(
            "the catalogue has no contact point (DCAT-AP-NL requires one): set \
             CATALOG_CONTACT_NAME and CATALOG_CONTACT_EMAIL"
                .into(),
        ),
        None => {}
    }
    let now = chrono::Utc::now().to_rfc3339();
    let issued = datasets
        .iter()
        .map(|d| d.created_at.as_str())
        .min()
        .unwrap_or(now.as_str())
        .to_string();
    let modified = datasets
        .iter()
        .map(|d| d.updated_at.as_str())
        .max()
        .unwrap_or(now.as_str())
        .to_string();
    g.when(catalog.clone(), &p(DCT, "issued"), &issued);
    g.when(catalog.clone(), &p(DCT, "modified"), &modified);
    let sparql = nn(&format!("{base}/sparql"));
    g.add(catalog.clone(), &p(DCAT, "service"), sparql.clone());
    let mut served = Served::default();
    if scope.is_none() {
        let root = nn(&format!("{base}/dataset"));
        g.add(catalog.clone(), &p(DCAT, "dataset"), root.clone());
        served.add(root, &[]);
    }
    for ds in &datasets {
        g.add(
            catalog.clone(),
            &p(DCAT, "dataset"),
            nn(&format!("{base}/dataset/{}", ds.id)),
        );
    }

    // ── the aggregate dataset (whole store) ──
    if scope.is_none() {
        // Everything the instance serves covers its datasets' themes.
        let mut themes: Vec<String> = Vec::new();
        for t in datasets.iter().flat_map(|d| json_list(d.themes.as_deref())) {
            if !themes.contains(&t) {
                themes.push(t);
            }
        }
        aggregate_dataset(&mut g, opts, store, readable.as_ref(), &publisher, &themes);
    }

    // ── per-dataset entries ──
    let mut geo_served = Served::default();
    for ds in &datasets {
        let entry = dataset_entry(&mut g, opts, store, auth_db, readable.as_ref(), ds);
        let iri = nn(&format!("{base}/dataset/{}", ds.id));
        served.add(iri.clone(), &entry.themes);
        if entry.geo {
            geo_served.add(iri.clone(), &entry.themes);
        }
        if opts.profile.is_ap() {
            catalog_record(&mut g, opts, &catalog, &iri, ds);
        }
    }

    // ── the data services ──
    let service_contact = opts.contact().or(catalog_contact);
    sparql_service(
        &mut g,
        opts,
        &sparql,
        &publisher,
        service_contact.as_ref(),
        &served,
    );
    if !geo_served.datasets.is_empty() {
        let ogc = nn(&format!("{base}/api/ogc"));
        g.add(catalog.clone(), &p(DCAT, "service"), ogc.clone());
        ogc_service(
            &mut g,
            opts,
            &ogc,
            &publisher,
            service_contact.as_ref(),
            &geo_served,
        );
    }

    CatalogReport {
        triples: g.triples,
        warnings: g.warnings,
    }
}

/// Serialise the catalogue in `format` (Turtle gets the usual prefixes).
pub fn serialize_catalog(triples: &[Triple], format: RdfFormat) -> Result<Vec<u8>, String> {
    super::graph::serialize(triples, format)
}

/// Log each profile warning once per process, so a catalogue requested every
/// minute does not repeat itself.
fn log_warnings(profile: Profile, warnings: &[String]) {
    use std::sync::{Mutex, OnceLock};
    static SEEN: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    let seen = SEEN.get_or_init(|| Mutex::new(HashSet::new()));
    let Ok(mut seen) = seen.lock() else {
        return;
    };
    for w in warnings {
        if seen.len() < 10_000 && seen.insert(w.clone()) {
            tracing::warn!("dcat ({}): {w}", profile.name());
        }
    }
}

/// The catalogue (whole instance, or `org`'s slice) in `format`.
pub fn generate_catalog_bytes(
    base_url: &str,
    store: &TripleStore,
    auth_db: &Arc<AuthDb>,
    user_id: Option<&str>,
    org: Option<&Organisation>,
    format: RdfFormat,
) -> Result<Vec<u8>, String> {
    let opts = CatalogOptions::from_env(base_url);
    let report = build_catalog_report(&opts, store, auth_db, user_id, org);
    log_warnings(opts.profile, &report.warnings);
    serialize_catalog(&report.triples, format)
}

/// The whole-instance catalogue as Turtle.
#[allow(dead_code)]
pub fn generate_dcat_catalog(
    base_url: &str,
    store: &TripleStore,
    auth_db: &Arc<AuthDb>,
    user_id: Option<&str>,
) -> String {
    generate_catalog_bytes(base_url, store, auth_db, user_id, None, RdfFormat::Turtle)
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_else(|e| format!("# catalogue serialisation failed: {e}\n"))
}

/// One organisation's catalogue as Turtle.
#[allow(dead_code)]
pub fn generate_org_dcat_catalog(
    org: &Organisation,
    base_url: &str,
    store: &TripleStore,
    auth_db: &Arc<AuthDb>,
    user_id: Option<&str>,
) -> String {
    generate_catalog_bytes(
        base_url,
        store,
        auth_db,
        user_id,
        Some(org),
        RdfFormat::Turtle,
    )
    .map(|b| String::from_utf8_lossy(&b).into_owned())
    .unwrap_or_else(|e| format!("# catalogue serialisation failed: {e}\n"))
}

/// A dataset's temporal coverage and update frequency, checked and
/// normalised for storage.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Coverage {
    pub temporal_start: Option<String>,
    pub temporal_end: Option<String>,
    /// An IRI (a code of the EU frequency table becomes its IRI).
    pub accrual_periodicity: Option<String>,
}

/// Check the coverage fields of a dataset update: dates are `YYYY-MM-DD` or
/// RFC 3339, the end is not before the start, and the frequency is a code of
/// the EU frequency table or an absolute IRI.
pub fn check_coverage(
    start: Option<&str>,
    end: Option<&str>,
    periodicity: Option<&str>,
) -> Result<Coverage, String> {
    let date = |field: &str, v: Option<&str>| -> Result<Option<String>, String> {
        match v.map(str::trim).filter(|s| !s.is_empty()) {
            None => Ok(None),
            Some(s) => super::graph::temporal_literal(s)
                .map(|(v, _)| Some(v))
                .ok_or_else(|| {
                    format!("{field} must be a date (YYYY-MM-DD) or an RFC 3339 date-time")
                }),
        }
    };
    let start = date("temporal_start", start)?;
    let end = date("temporal_end", end)?;
    if let (Some(s), Some(e)) = (&start, &end) {
        // Compare on the date part: both forms start with YYYY-MM-DD.
        if e.get(..10) < s.get(..10) {
            return Err("temporal_end must not be before temporal_start".into());
        }
    }
    let periodicity = match periodicity.map(str::trim).filter(|s| !s.is_empty()) {
        None => None,
        Some(v) => Some(authority::frequency_iri(v).ok_or_else(|| {
            format!(
                "accrual_periodicity `{v}` is not a code of the EU frequency table \
                 (ANNUAL, MONTHLY, …) or an IRI"
            )
        })?),
    };
    if let Some(i) = &periodicity {
        NamedNode::new(i.as_str())
            .map_err(|_| format!("accrual_periodicity `{i}` is not an absolute IRI"))?;
    }
    Ok(Coverage {
        temporal_start: start,
        temporal_end: end,
        accrual_periodicity: periodicity,
    })
}

// ── agents ──────────────────────────────────────────────────────────────────

pub(crate) fn catalog_publisher(g: &mut G, opts: &CatalogOptions) -> NamedNode {
    let iri = g
        .iri(&opts.publisher_iri, "CATALOG_PUBLISHER_URI")
        .unwrap_or_else(|| nn(&format!("{}/publisher", opts.base_url)));
    if g.agents.insert(iri.as_str().to_string()) {
        g.typ(iri.clone(), &p(FOAF, "Agent"));
        g.typ(iri.clone(), &p(FOAF, "Organization"));
        g.lit(iri.clone(), &p(FOAF, "name"), &opts.publisher_name);
        if let Some(id) = &opts.publisher_identifier {
            g.lit(iri.clone(), &p(DCT, "identifier"), id);
        }
        if let Some(t) = &opts.publisher_type {
            g.concept(iri.clone(), &p(DCT, "type"), t, "CATALOG_PUBLISHER_TYPE");
        }
        g.ranged(
            iri.clone(),
            &p(FOAF, "homepage"),
            &format!("{}/", opts.base_url),
            Range::Document,
            "homepage",
        );
    }
    iri
}

pub(crate) fn org_agent(g: &mut G, opts: &CatalogOptions, org: &Organisation) -> NamedNode {
    let iri = nn(&format!("{}/org/{}", opts.base_url, org.id));
    if !g.agents.insert(iri.as_str().to_string()) {
        return iri;
    }
    g.typ(iri.clone(), &p(FOAF, "Agent"));
    g.typ(iri.clone(), &p(FOAF, "Organization"));
    match org.org_type.as_deref().unwrap_or("FormalOrganization") {
        "OrganizationalUnit" => g.typ(iri.clone(), &p(ORG, "OrganizationalUnit")),
        "Organization" => {}
        _ => g.typ(iri.clone(), &p(ORG, "FormalOrganization")),
    }
    g.lit(iri.clone(), &p(FOAF, "name"), &org.name);
    if let Some(d) = &org.description {
        g.lang(iri.clone(), &p(DCT, "description"), d, opts.lang_tag());
    }
    if let Some(h) = &org.homepage {
        g.ranged(
            iri.clone(),
            &p(FOAF, "homepage"),
            h,
            Range::Document,
            "organisation homepage",
        );
    }
    if let Some(id) = &org.identifier {
        g.lit(iri.clone(), &p(DCT, "identifier"), id);
    }
    if let Some(c) = Contact::new(
        org.contact_name.as_deref(),
        org.contact_email.as_deref(),
        org.contact_url.as_deref(),
    ) {
        contact_point(g, iri.clone(), &c);
    }
    iri
}

pub(crate) fn user_agent(
    g: &mut G,
    opts: &CatalogOptions,
    auth_db: &Arc<AuthDb>,
    user_id: &str,
) -> NamedNode {
    let iri = nn(&format!("{}/user/{}", opts.base_url, user_id));
    if g.agents.insert(iri.as_str().to_string()) {
        g.typ(iri.clone(), &p(FOAF, "Agent"));
        g.typ(iri.clone(), &p(FOAF, "Person"));
        let name = auth_db
            .get_user_by_id(user_id)
            .ok()
            .flatten()
            .map(|u| u.username)
            .unwrap_or_else(|| user_id.to_string());
        g.lit(iri.clone(), &p(FOAF, "name"), &name);
        g.concept(
            iri.clone(),
            &p(DCT, "type"),
            authority::PRIVATE_INDIVIDUAL,
            "publisher type",
        );
    }
    iri
}

fn group_agent(
    g: &mut G,
    opts: &CatalogOptions,
    auth_db: &Arc<AuthDb>,
    group_id: &str,
) -> NamedNode {
    let iri = nn(&format!("{}/group/{}", opts.base_url, group_id));
    if g.agents.insert(iri.as_str().to_string()) {
        g.typ(iri.clone(), &p(FOAF, "Agent"));
        g.typ(iri.clone(), &p(FOAF, "Group"));
        let name = auth_db
            .get_group(group_id)
            .ok()
            .flatten()
            .map(|gr| gr.name)
            .unwrap_or_else(|| group_id.to_string());
        g.lit(iri.clone(), &p(FOAF, "name"), &name);
    }
    iri
}

fn contact_point(g: &mut G, subject: impl Into<NamedOrBlankNode>, c: &Contact) {
    let cp = BlankNode::default();
    g.add(subject, &p(DCAT, "contactPoint"), cp.clone());
    g.typ(cp.clone(), &p(VCARD, "Kind"));
    g.typ(cp.clone(), &p(VCARD, "Organization"));
    if let Some(n) = &c.name {
        g.lit(cp.clone(), &p(VCARD, "fn"), n);
    }
    if let Some(e) = &c.email {
        g.link(
            cp.clone(),
            &p(VCARD, "hasEmail"),
            &format!("mailto:{e}"),
            "contact e-mail",
        );
    }
    if let Some(u) = &c.url {
        g.link(cp.clone(), &p(VCARD, "hasURL"), u, "contact URL");
    }
}

// ── the aggregate dataset ───────────────────────────────────────────────────

/// The graphs the caller may read, by the `/sparql` rule
/// ([`crate::auth::acl::readable_graph_iris`]): `None` for an administrator,
/// who reads every graph. The catalogue's statistics and graph listings stay
/// inside this set, so private and system graphs never show in a total or a
/// listing the caller could not query (as the service description at `/`
/// already does). A failed lookup reads nothing.
fn readable_graphs(auth_db: &Arc<AuthDb>, user_id: Option<&str>) -> Option<HashSet<String>> {
    let user = user_id.and_then(|id| auth_db.get_user_by_id(id).ok().flatten());
    if user.as_ref().is_some_and(|u| u.role.is_admin()) {
        return None;
    }
    let principal = user.as_ref().map(|u| (u.id.as_str(), u.role.as_str()));
    Some(crate::auth::acl::readable_graph_iris(auth_db, principal).unwrap_or_default())
}

fn aggregate_dataset(
    g: &mut G,
    opts: &CatalogOptions,
    store: &TripleStore,
    readable: Option<&HashSet<String>>,
    publisher: &NamedNode,
    themes: &[String],
) {
    let base = opts.base_url.as_str();
    let root = nn(&format!("{base}/dataset"));
    let stats = match readable {
        None => store.void_stats(),
        Some(graphs) => store.void_stats_over(graphs),
    };
    g.typ(root.clone(), &p(VOID, "Dataset"));
    g.typ(root.clone(), &p(DCAT, "Dataset"));
    if opts.profile.is_ap() {
        g.typ(root.clone(), &p(DCAT, "Resource"));
    }
    g.lang(root.clone(), &p(DCT, "title"), &opts.title, opts.lang_tag());
    g.lang(
        root.clone(),
        &p(DCT, "description"),
        "Everything this instance serves, as one VoID dataset.",
        "en",
    );
    g.add(
        root.clone(),
        &p(VOID, "sparqlEndpoint"),
        nn(&format!("{base}/sparql")),
    );
    g.lit(
        root.clone(),
        &p(VOID, "uriSpace"),
        &format!("{base}/resource/"),
    );
    g.int(root.clone(), &p(VOID, "triples"), stats.triples);
    g.int(
        root.clone(),
        &p(VOID, "distinctSubjects"),
        stats.distinct_subjects,
    );
    g.int(
        root.clone(),
        &p(VOID, "distinctObjects"),
        stats.distinct_objects,
    );
    g.int(
        root.clone(),
        &p(VOID, "properties"),
        stats.distinct_predicates,
    );
    g.int(root.clone(), &p(VOID, "documents"), stats.named_graphs);
    g.add(root.clone(), &p(DCT, "publisher"), publisher.clone());
    g.add(root.clone(), &p(DCT, "creator"), publisher.clone());
    access_rights(g, root.clone(), "PUBLIC");
    if opts.profile.is_ap() {
        g.lit(
            root.clone(),
            &p(DCT, "identifier"),
            &format!("{base}/dataset"),
        );
        g.ranged(
            root.clone(),
            &p(DCT, "language"),
            &opts.language_iri(),
            Range::LinguisticSystem,
            "language",
        );
        if let Some(l) = &opts.license {
            g.ranged(
                root.clone(),
                &p(DCT, "license"),
                l,
                Range::LicenseDocument,
                "catalogue licence",
            );
        }
    }
    match opts.contact() {
        Some(c) => contact_point(g, root.clone(), &c),
        None if opts.profile.is_nl() => g.warn(
            "the aggregate dataset has no contact point (DCAT-AP-NL requires one): set \
             CATALOG_CONTACT_NAME and CATALOG_CONTACT_EMAIL"
                .into(),
        ),
        None => {}
    }
    if opts.license.is_none() && opts.profile.is_nl() {
        g.warn(
            "the aggregate dataset's distribution has no licence (DCAT-AP-NL requires one): \
             set CATALOG_LICENSE"
                .into(),
        );
    }
    for t in themes {
        g.concept(root.clone(), &p(DCAT, "theme"), t, "dataset theme");
    }
    if themes.is_empty() && opts.profile.is_nl() {
        g.warn(
            "the aggregate dataset has no dcat:theme (DCAT-AP-NL requires one): none of the \
             datasets declares a theme"
                .into(),
        );
    }
    sparql_distribution(g, opts, root.clone(), opts.license.as_deref());
    g.ranged(
        root.clone(),
        &p(DCAT, "landingPage"),
        &format!("{base}/"),
        Range::Document,
        "landing page",
    );
}

fn access_rights(g: &mut G, s: impl Into<NamedOrBlankNode>, code: &str) {
    g.ranged(
        s,
        &p(DCT, "accessRights"),
        &format!("{}{code}", authority::EU_ACCESS),
        Range::RightsStatement,
        "access rights",
    );
}

// ── data services ───────────────────────────────────────────────────────────

/// What both data services carry: title, description, identifier, publisher,
/// contact point, access rights, licence, language, the datasets they serve
/// and those datasets' themes.
#[allow(clippy::too_many_arguments)]
fn service_common(
    g: &mut G,
    opts: &CatalogOptions,
    svc: &NamedNode,
    title: &str,
    description: &str,
    publisher: &NamedNode,
    contact: Option<&Contact>,
    served: &Served,
) {
    g.typ(svc.clone(), &p(DCAT, "DataService"));
    g.lang(svc.clone(), &p(DCT, "title"), title, "en");
    g.lang(svc.clone(), &p(DCT, "description"), description, "en");
    g.add(svc.clone(), &p(DCT, "publisher"), publisher.clone());
    g.ranged(
        svc.clone(),
        &p(DCAT, "endpointURL"),
        svc.as_str(),
        Range::Resource,
        "endpoint URL",
    );
    // The endpoint serves the public datasets anonymously and more to a
    // signed-in caller; the service itself is open to everyone.
    access_rights(g, svc.clone(), "PUBLIC");
    for d in &served.datasets {
        g.add(svc.clone(), &p(DCAT, "servesDataset"), d.clone());
    }
    for t in &served.themes {
        g.concept(svc.clone(), &p(DCAT, "theme"), t, "service theme");
    }
    match contact {
        Some(c) => contact_point(g, svc.clone(), c),
        None if opts.profile.is_nl() => g.warn(format!(
            "the data service <{}> has no contact point (DCAT-AP-NL requires one): set \
             CATALOG_CONTACT_NAME and CATALOG_CONTACT_EMAIL",
            svc.as_str()
        )),
        None => {}
    }
    if opts.profile.is_ap() {
        g.lit(svc.clone(), &p(DCT, "identifier"), svc.as_str());
        g.ranged(
            svc.clone(),
            &p(DCT, "language"),
            &opts.language_iri(),
            Range::LinguisticSystem,
            "language",
        );
        match &opts.license {
            Some(l) => {
                g.ranged(
                    svc.clone(),
                    &p(DCT, "license"),
                    l,
                    Range::LicenseDocument,
                    "catalogue licence",
                );
            }
            None if opts.profile.is_nl() => g.warn(format!(
                "the data service <{}> has no licence (DCAT-AP-NL requires one): set \
                 CATALOG_LICENSE",
                svc.as_str()
            )),
            None => {}
        }
        if served.themes.is_empty() && opts.profile.is_nl() {
            g.warn(format!(
                "the data service <{}> has no dcat:theme (DCAT-AP-NL requires one): none of \
                 the datasets it serves declares a theme",
                svc.as_str()
            ));
        }
    }
}

fn sparql_service(
    g: &mut G,
    opts: &CatalogOptions,
    sparql: &NamedNode,
    publisher: &NamedNode,
    contact: Option<&Contact>,
    served: &Served,
) {
    service_common(
        g,
        opts,
        sparql,
        "SPARQL endpoint",
        "SPARQL 1.1 query and update over the datasets of this catalogue.",
        publisher,
        contact,
        served,
    );
    g.typ(sparql.clone(), &p(SD, "Service"));
    g.add(sparql.clone(), &p(SD, "endpoint"), sparql.clone());
    g.add(
        sparql.clone(),
        &p(SD, "supportedLanguage"),
        nn(&p(SD, "SPARQL11Query")),
    );
    g.add(
        sparql.clone(),
        &p(SD, "supportedLanguage"),
        nn(&p(SD, "SPARQL11Update")),
    );
    // The SPARQL 1.1 Service Description, which `GET /sparql` without a query
    // returns (SPARQL 1.1 Service Description §2).
    g.ranged(
        sparql.clone(),
        &p(DCAT, "endpointDescription"),
        sparql.as_str(),
        Range::Resource,
        "endpoint description",
    );
    g.ranged(
        sparql.clone(),
        &p(DCT, "conformsTo"),
        SPARQL_PROTOCOL,
        Range::Standard,
        "standard",
    );
}

fn ogc_service(
    g: &mut G,
    opts: &CatalogOptions,
    svc: &NamedNode,
    publisher: &NamedNode,
    contact: Option<&Contact>,
    served: &Served,
) {
    service_common(
        g,
        opts,
        svc,
        "OGC API – Features",
        "The features of the datasets with geometry, as GeoJSON over OGC API – Features.",
        publisher,
        contact,
        served,
    );
    // The OGC API landing page links the API definition and conformance.
    g.ranged(
        svc.clone(),
        &p(DCAT, "endpointDescription"),
        svc.as_str(),
        Range::Resource,
        "endpoint description",
    );
    g.ranged(
        svc.clone(),
        &p(DCT, "conformsTo"),
        OGC_FEATURES,
        Range::Standard,
        "standard",
    );
}

// ── distributions ───────────────────────────────────────────────────────────

fn distribution_common(
    g: &mut G,
    opts: &CatalogOptions,
    d: &BlankNode,
    title: &str,
    license: Option<&str>,
) {
    g.typ(d.clone(), &p(DCAT, "Distribution"));
    g.lang(d.clone(), &p(DCT, "title"), title, "en");
    if opts.profile.is_ap() {
        if let Some(l) = license {
            g.ranged(
                d.clone(),
                &p(DCT, "license"),
                l,
                Range::LicenseDocument,
                "distribution licence",
            );
        }
    }
}

/// `dcat:mediaType` (IANA) and, under a profile, `dct:format` (EU file type).
pub(crate) fn media(g: &mut G, opts: &CatalogOptions, d: &BlankNode, mime: &str, filetype: &str) {
    g.ranged(
        d.clone(),
        &p(DCAT, "mediaType"),
        &format!("{IANA}{mime}"),
        Range::MediaType,
        "media type",
    );
    if opts.profile.is_ap() {
        g.ranged(
            d.clone(),
            &p(DCT, "format"),
            &format!("{EU_FILETYPE}{filetype}"),
            Range::MediaTypeOrExtent,
            "file type",
        );
    }
}

fn sparql_distribution(g: &mut G, opts: &CatalogOptions, ds: NamedNode, license: Option<&str>) {
    let base = opts.base_url.as_str();
    let d = BlankNode::default();
    g.add(ds, &p(DCAT, "distribution"), d.clone());
    distribution_common(g, opts, &d, "SPARQL endpoint", license);
    g.add(
        d.clone(),
        &p(DCAT, "accessURL"),
        nn(&format!("{base}/sparql")),
    );
    g.add(
        d.clone(),
        &p(DCAT, "accessService"),
        nn(&format!("{base}/sparql")),
    );
    g.ranged(
        d.clone(),
        &p(DCT, "conformsTo"),
        SPARQL_PROTOCOL,
        Range::Standard,
        "standard",
    );
    media(g, opts, &d, "application/sparql-results+json", "SPARQLQ");
}

#[allow(clippy::too_many_arguments)]
fn rdf_distribution(
    g: &mut G,
    opts: &CatalogOptions,
    ds: NamedNode,
    title: &str,
    access_url: &str,
    download_url: Option<&str>,
    conforms_to: Option<&str>,
    license: Option<&str>,
    trig: bool,
) {
    let d = BlankNode::default();
    g.add(ds, &p(DCAT, "distribution"), d.clone());
    distribution_common(g, opts, &d, title, license);
    g.add(d.clone(), &p(DCAT, "accessURL"), nn(access_url));
    if let Some(u) = download_url {
        g.add(d.clone(), &p(DCAT, "downloadURL"), nn(u));
    }
    if trig {
        media(g, opts, &d, "application/trig", "RDF_TRIG");
    } else {
        media(g, opts, &d, "text/turtle", "RDF_TURTLE");
    }
    if let Some(c) = conforms_to {
        g.ranged(
            d.clone(),
            &p(DCT, "conformsTo"),
            c,
            Range::Standard,
            "standard",
        );
    }
}

// ── per-dataset entry ───────────────────────────────────────────────────────

/// What a dataset and each of its versions share: who publishes it, what
/// covers it and under which terms.
struct Shared {
    access: &'static str,
    agent: NamedNode,
    license: Option<String>,
    themes: Vec<String>,
    keywords: Vec<String>,
    contact: Option<Contact>,
    spatial: Option<String>,
    temporal: (Option<String>, Option<String>),
    periodicity: Option<String>,
}

fn json_list(v: Option<&str>) -> Vec<String> {
    v.and_then(|j| serde_json::from_str::<Vec<String>>(j).ok())
        .unwrap_or_default()
        .into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn nonempty(v: Option<&str>) -> Option<String> {
    v.map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn describe_shared(g: &mut G, opts: &CatalogOptions, s: &NamedNode, sh: &Shared) {
    let ap = opts.profile.is_ap();
    access_rights(g, s.clone(), sh.access);
    g.add(s.clone(), &p(DCT, "publisher"), sh.agent.clone());
    // DCAT-AP-NL asks for a creator; the owner both created and publishes it.
    g.add(s.clone(), &p(DCT, "creator"), sh.agent.clone());
    g.add(s.clone(), &p(PROV, "wasAttributedTo"), sh.agent.clone());
    if ap {
        g.ranged(
            s.clone(),
            &p(DCT, "language"),
            &opts.language_iri(),
            Range::LinguisticSystem,
            "language",
        );
    }
    if let Some(l) = &sh.license {
        g.ranged(
            s.clone(),
            &p(DCT, "license"),
            l,
            Range::LicenseDocument,
            "dataset licence",
        );
    }
    for t in &sh.themes {
        g.concept(s.clone(), &p(DCAT, "theme"), t, "dataset theme");
    }
    for k in &sh.keywords {
        g.lang(s.clone(), &p(DCAT, "keyword"), k, opts.lang_tag());
    }
    if let Some(c) = &sh.contact {
        contact_point(g, s.clone(), c);
    }
    if let Some(sp) = &sh.spatial {
        g.ranged(
            s.clone(),
            &p(DCT, "spatial"),
            sp,
            Range::Location,
            "dct:spatial",
        );
    }
    if sh.temporal.0.is_some() || sh.temporal.1.is_some() {
        let t = BlankNode::default();
        g.add(s.clone(), &p(DCT, "temporal"), t.clone());
        g.typ(t.clone(), &p(DCT, "PeriodOfTime"));
        if let Some(v) = &sh.temporal.0 {
            g.when(t.clone(), &p(DCAT, "startDate"), v);
        }
        if let Some(v) = &sh.temporal.1 {
            g.when(t.clone(), &p(DCAT, "endDate"), v);
        }
    }
    if let Some(f) = &sh.periodicity {
        g.ranged(
            s.clone(),
            &p(DCT, "accrualPeriodicity"),
            f,
            Range::Frequency,
            "accrual periodicity",
        );
    }
}

/// What the catalogue needs to know about a dataset entry once written.
struct EntryInfo {
    themes: Vec<String>,
    geo: bool,
}

fn dataset_entry(
    g: &mut G,
    opts: &CatalogOptions,
    store: &TripleStore,
    auth_db: &Arc<AuthDb>,
    readable: Option<&HashSet<String>>,
    ds: &Dataset,
) -> EntryInfo {
    let base = opts.base_url.as_str();
    let ap = opts.profile.is_ap();
    let nl = opts.profile.is_nl();
    let s = nn(&format!("{base}/dataset/{}", ds.id));

    g.typ(s.clone(), &p(DCAT, "Dataset"));
    g.typ(s.clone(), &p(VOID, "Dataset"));
    if ap {
        g.typ(s.clone(), &p(DCAT, "Resource"));
    }
    g.lang(s.clone(), &p(DCT, "title"), &ds.name, opts.lang_tag());
    let description = nonempty(ds.description.as_deref()).or_else(|| ap.then(|| ds.name.clone()));
    if let Some(d) = description {
        g.lang(s.clone(), &p(DCT, "description"), &d, opts.lang_tag());
    }
    g.when(s.clone(), &p(DCT, "issued"), &ds.created_at);
    g.when(s.clone(), &p(DCT, "modified"), &ds.updated_at);
    if ap {
        g.lit(s.clone(), &p(DCT, "identifier"), &ds.id);
        let id = BlankNode::default();
        g.add(s.clone(), &p(ADMS, "identifier"), id.clone());
        g.typ(id.clone(), &p(ADMS, "Identifier"));
        g.lit(
            id.clone(),
            &p(SKOS, "notation"),
            &format!("{base}/dataset/{}", ds.id),
        );
    }

    // Publisher / creator agent: the owner.
    let (agent, owner_org) = match ds.owner_type {
        OwnerType::Organisation => match auth_db.get_organisation(&ds.owner_id) {
            Ok(Some(org)) => (org_agent(g, opts, &org), Some(org)),
            _ => (nn(&format!("{base}/org/{}", ds.owner_id)), None),
        },
        OwnerType::User => (user_agent(g, opts, auth_db, &ds.owner_id), None),
        OwnerType::Group => (group_agent(g, opts, auth_db, &ds.owner_id), None),
    };

    let license =
        nonempty(ds.license.as_deref()).or_else(|| ap.then(|| opts.license.clone()).flatten());
    let themes = json_list(ds.themes.as_deref());
    // A dataset without its own contact point is answered for by its owning
    // organisation, else by the catalogue's contact (the profiles require one).
    let contact = Contact::new(
        ds.contact_name.as_deref(),
        ds.contact_email.as_deref(),
        ds.contact_url.as_deref(),
    )
    .or_else(|| {
        if !ap {
            return None;
        }
        owner_org
            .as_ref()
            .and_then(|o| {
                Contact::new(
                    o.contact_name.as_deref(),
                    o.contact_email.as_deref(),
                    o.contact_url.as_deref(),
                )
            })
            .or_else(|| opts.contact())
    });
    if nl {
        if themes.is_empty() {
            g.warn(format!(
                "dataset `{}` has no dcat:theme (DCAT-AP-NL requires one): set its themes",
                ds.id
            ));
        }
        if contact.is_none() {
            g.warn(format!(
                "dataset `{}` has no contact point (DCAT-AP-NL requires one): set its contact, \
                 its organisation's, or CATALOG_CONTACT_NAME / CATALOG_CONTACT_EMAIL",
                ds.id
            ));
        }
        if license.is_none() {
            g.warn(format!(
                "dataset `{}` has no licence, so neither have its distributions (DCAT-AP-NL \
                 requires one): set its licence or CATALOG_LICENSE",
                ds.id
            ));
        }
    }
    let shared = Shared {
        access: match ds.visibility {
            Visibility::Public => "PUBLIC",
            Visibility::Members => "RESTRICTED",
            Visibility::Private => "NON_PUBLIC",
        },
        agent,
        license: license.clone(),
        themes: themes.clone(),
        keywords: json_list(ds.keywords.as_deref()),
        contact,
        spatial: nonempty(ds.spatial.as_deref()),
        temporal: (
            nonempty(ds.temporal_start.as_deref()),
            nonempty(ds.temporal_end.as_deref()),
        ),
        periodicity: nonempty(ds.accrual_periodicity.as_deref()),
    };
    describe_shared(g, opts, &s, &shared);

    // Graphs, roles, counts, provenance — of the graphs the caller may read:
    // a private graph is its dataset's writers' only.
    let mut entries = auth_db
        .list_dataset_graph_entries(&ds.id)
        .unwrap_or_default();
    entries.retain(|e| !e.private || readable.is_none_or(|r| r.contains(&e.graph_iri)));
    let graphs: Vec<String> = entries.iter().map(|e| e.graph_iri.clone()).collect();
    let latest = crate::commit_log::list_commits(
        store,
        &crate::commit_log::CommitScope::Graphs(graphs.clone()),
        &crate::commit_log::CommitQuery {
            limit: Some(1),
            ..Default::default()
        },
    );
    if let Some(c) = latest.first() {
        let commit = format!("{base}/commit/{}", c.commit_id);
        g.ranged(
            s.clone(),
            &p(PROV, "wasGeneratedBy"),
            &commit,
            Range::Activity,
            "commit",
        );
    }
    let mut total = 0usize;
    for e in &entries {
        total += store.graph_count_cached(Some(&e.graph_iri)).unwrap_or(0);
        if e.graph_iri.starts_with("urn:system:") {
            continue;
        }
        if let Some(gi) = g.iri(&e.graph_iri, "graph IRI") {
            g.add(s.clone(), &p(VOID, "subset"), gi.clone());
            if let Some(role) = e.graph_role {
                g.add(
                    gi.clone(),
                    &p(OTS, "graphRole"),
                    nn(crate::auth::dataset_graph::graph_role_iri(role)),
                );
            }
        }
    }
    g.int(s.clone(), &p(VOID, "triples"), total);
    void_description(g, opts, store, &s, &entries, total);

    // Conformance.
    if ds.shacl_on_write {
        if let Some(shapes) = &ds.shapes_graph_iri {
            g.ranged(
                s.clone(),
                &p(DCT, "conformsTo"),
                shapes,
                Range::Standard,
                "shapes graph",
            );
        }
    }
    if let Some(target) = model_target(
        base,
        ds.conforms_to_model.as_deref(),
        ds.conforms_to_version.as_deref(),
    ) {
        g.ranged(
            s.clone(),
            &p(DCT, "conformsTo"),
            &target,
            Range::Standard,
            "data model",
        );
    }

    if let Some(st) = ds.adms_status.as_deref().and_then(authority::status_iri) {
        g.concept(s.clone(), &p(ADMS, "status"), &st, "adms:status");
    }
    if let Some(n) = nonempty(ds.version_notes.as_deref()) {
        g.lang(s.clone(), &p(ADMS, "versionNotes"), &n, opts.lang_tag());
    }

    // Versions (DCAT 3 §11).
    versions(g, opts, store, ds, &s, &shared);

    // Distributions.
    sparql_distribution(g, opts, s.clone(), license.as_deref());
    rdf_distribution(
        g,
        opts,
        s.clone(),
        "Graph Store HTTP Protocol",
        &format!("{base}/store"),
        None,
        Some(GSP_PROTOCOL),
        license.as_deref(),
        false,
    );
    for e in entries
        .iter()
        .filter(|e| !e.graph_iri.starts_with("urn:system:"))
    {
        if g.iri(&e.graph_iri, "graph IRI").is_none() {
            continue;
        }
        let url = format!("{base}/store?graph={}", enc(&e.graph_iri));
        rdf_distribution(
            g,
            opts,
            s.clone(),
            &format!("Graph {}", e.graph_iri),
            &url,
            Some(&url),
            None,
            license.as_deref(),
            false,
        );
    }
    if matches!(crate::ldes::store::stream(auth_db, &ds.id), Ok(Some(cfg)) if cfg.enabled) {
        rdf_distribution(
            g,
            opts,
            s.clone(),
            "Linked Data Event Stream",
            &crate::ldes::stream_iri(base, &ds.id),
            None,
            Some(LDES_SPEC),
            license.as_deref(),
            false,
        );
    }

    // Geospatial access paths — only when the dataset actually carries geometry.
    // The verbose `…/ifcowl` lift graphs and the 3D-Tiles feed graphs are
    // excluded from the probe, as the viewer-feed and geo-stats handlers do.
    let data_graphs: Vec<String> = entries
        .iter()
        .filter(|e| !e.graph_iri.starts_with("urn:system:"))
        .filter(|e| !e.graph_iri.ends_with("/ifcowl"))
        .filter(|e| !crate::geo::viewer_feed::is_tiles3d_graph(&e.graph_iri))
        .map(|e| e.graph_iri.clone())
        .collect();
    let geo = crate::geo::viewer_feed::dataset_geo_stats(store, &data_graphs);
    let has_geo = geo.has_coordinates || geo.has_3d;
    if has_geo {
        let d = BlankNode::default();
        g.add(s.clone(), &p(DCAT, "distribution"), d.clone());
        distribution_common(
            g,
            opts,
            &d,
            "OGC API – Features (GeoJSON)",
            license.as_deref(),
        );
        g.add(
            d.clone(),
            &p(DCAT, "accessURL"),
            nn(&format!("{base}/api/ogc/collections/{}/items", ds.id)),
        );
        media(g, opts, &d, "application/geo+json", "GEOJSON");
        g.ranged(
            d.clone(),
            &p(DCT, "conformsTo"),
            OGC_FEATURES,
            Range::Standard,
            "standard",
        );
        g.add(
            d.clone(),
            &p(DCAT, "accessService"),
            nn(&format!("{base}/api/ogc")),
        );
        if geo.has_3d {
            let d = BlankNode::default();
            g.add(s.clone(), &p(DCAT, "distribution"), d.clone());
            distribution_common(g, opts, &d, "OGC 3D Tiles 1.1", license.as_deref());
            g.add(
                d.clone(),
                &p(DCAT, "accessURL"),
                nn(&format!(
                    "{base}/api/datasets/{}/3dtiles/tileset.json",
                    ds.id
                )),
            );
            media(g, opts, &d, "application/json", "JSON");
            g.ranged(
                d.clone(),
                &p(DCT, "conformsTo"),
                TILES3D,
                Range::Standard,
                "standard",
            );
        }
        let d = BlankNode::default();
        g.add(s.clone(), &p(DCAT, "distribution"), d.clone());
        distribution_common(g, opts, &d, "Viewer feed (JSON)", license.as_deref());
        g.add(
            d.clone(),
            &p(DCAT, "accessURL"),
            nn(&format!("{base}/api/datasets/{}/viewer-feed", ds.id)),
        );
        media(g, opts, &d, "application/json", "JSON");
    }

    let landing = nonempty(ds.landing_page.as_deref()).unwrap_or_else(|| format!("{base}/"));
    if !g.ranged(
        s.clone(),
        &p(DCAT, "landingPage"),
        &landing,
        Range::Document,
        "landing page",
    ) {
        g.ranged(
            s.clone(),
            &p(DCAT, "landingPage"),
            &format!("{base}/"),
            Range::Document,
            "landing page",
        );
    }

    EntryInfo {
        themes,
        geo: has_geo,
    }
}

/// The formats the Graph Store and the downloads serve (`void:feature`).
const FORMATS_NS: &str = "http://www.w3.org/ns/formats/";
const VOID_FEATURES: &[&str] = &[
    "Turtle",
    "N-Triples",
    "RDF_XML",
    "JSON-LD",
    "TriG",
    "N-Quads",
];

/// Partitions listed per kind unless `OTS_VOID_PARTITION_LIMIT` says otherwise.
const DEFAULT_PARTITION_LIMIT: usize = 100;
/// Datasets larger than this (`OTS_VOID_PARTITION_MAX_TRIPLES`) get counts but
/// no partitions: those take a grouping scan of the data.
const DEFAULT_PARTITION_MAX_TRIPLES: usize = 5_000_000;

fn env_usize(k: &str, default: usize) -> usize {
    env_nonempty(k)
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// A dataset's VoID description beyond `void:triples`: statistics, class and
/// property partitions, vocabularies, example resources, features, data dumps,
/// its SPARQL endpoint and its linksets — all over `entries`, the dataset's
/// graphs the caller may read. Nothing here is computed over a store-wide
/// aggregate: a partition names classes and predicates, and an aggregate
/// would name those of graphs the caller may not read.
fn void_description(
    g: &mut G,
    opts: &CatalogOptions,
    store: &TripleStore,
    s: &NamedNode,
    entries: &[crate::auth::models::DatasetGraphEntry],
    total: usize,
) {
    let base = opts.base_url.as_str();
    let graphs: HashSet<String> = entries.iter().map(|e| e.graph_iri.clone()).collect();
    g.add(
        s.clone(),
        &p(VOID, "sparqlEndpoint"),
        nn(&format!("{base}/sparql")),
    );
    for f in VOID_FEATURES {
        g.add(
            s.clone(),
            &p(VOID, "feature"),
            nn(&format!("{FORMATS_NS}{f}")),
        );
    }
    for e in entries
        .iter()
        .filter(|e| !e.graph_iri.starts_with("urn:system:"))
    {
        if g.iri(&e.graph_iri, "graph IRI").is_some() {
            g.add(
                s.clone(),
                &p(VOID, "dataDump"),
                nn(&format!("{base}/store?graph={}", enc(&e.graph_iri))),
            );
        }
    }
    if graphs.is_empty() {
        return;
    }
    let stats = store.void_stats_over(&graphs);
    g.int(
        s.clone(),
        &p(VOID, "distinctSubjects"),
        stats.distinct_subjects,
    );
    g.int(
        s.clone(),
        &p(VOID, "distinctObjects"),
        stats.distinct_objects,
    );
    g.int(s.clone(), &p(VOID, "properties"), stats.distinct_predicates);
    g.int(s.clone(), &p(VOID, "documents"), stats.named_graphs);

    if total
        <= env_usize(
            "OTS_VOID_PARTITION_MAX_TRIPLES",
            DEFAULT_PARTITION_MAX_TRIPLES,
        )
    {
        let limit = env_usize("OTS_VOID_PARTITION_LIMIT", DEFAULT_PARTITION_LIMIT);
        let parts = store.void_partitions_over(&graphs, limit);
        g.int(s.clone(), &p(VOID, "classes"), parts.class_count);
        let mut vocabularies: Vec<String> = Vec::new();
        let mut vocab = |iri: &str| {
            let ns = namespace_of(iri);
            if !ns.is_empty() && !vocabularies.iter().any(|v| v == ns) {
                vocabularies.push(ns.to_string());
            }
        };
        for (class, n) in &parts.classes {
            let cp = BlankNode::default();
            g.add(s.clone(), &p(VOID, "classPartition"), cp.clone());
            g.add(cp.clone(), &p(VOID, "class"), nn(class));
            g.int(cp, &p(VOID, "entities"), *n);
            vocab(class);
        }
        for (property, n) in &parts.properties {
            let pp = BlankNode::default();
            g.add(s.clone(), &p(VOID, "propertyPartition"), pp.clone());
            g.add(pp.clone(), &p(VOID, "property"), nn(property));
            g.int(pp, &p(VOID, "triples"), *n);
            vocab(property);
        }
        for v in vocabularies.iter().take(limit) {
            g.link(s.clone(), &p(VOID, "vocabulary"), v, "vocabulary");
        }
        for e in &parts.examples {
            g.add(s.clone(), &p(VOID, "exampleResource"), nn(e));
        }
    }

    // Linksets: the dataset's graphs whose role is `linkset`.
    for e in entries
        .iter()
        .filter(|e| e.graph_role == Some(crate::auth::models::GraphKind::Linkset))
    {
        let Some(ls) = g.iri(&e.graph_iri, "linkset graph") else {
            continue;
        };
        let links = store.void_linkset(
            &e.graph_iri,
            env_usize("OTS_VOID_PARTITION_LIMIT", DEFAULT_PARTITION_LIMIT),
        );
        g.typ(ls.clone(), &p(VOID, "Linkset"));
        g.add(ls.clone(), &p(VOID, "subjectsTarget"), s.clone());
        if let Some(space) = &links.object_space {
            let target = BlankNode::default();
            g.add(ls.clone(), &p(VOID, "objectsTarget"), target.clone());
            g.typ(target.clone(), &p(VOID, "Dataset"));
            g.lit(target, &p(VOID, "uriSpace"), space);
        }
        for (pred, _) in &links.predicates {
            g.add(ls.clone(), &p(VOID, "linkPredicate"), nn(pred));
        }
        g.int(
            ls,
            &p(VOID, "triples"),
            store.graph_count_cached(Some(&e.graph_iri)).unwrap_or(0),
        );
    }
}

/// An IRI's namespace: up to and including its last `#` or `/`.
fn namespace_of(iri: &str) -> &str {
    match iri.rfind(['#', '/']) {
        Some(i) if i + 1 < iri.len() => &iri[..=i],
        _ => "",
    }
}

/// The model (version) a dataset or a version conforms to.
fn model_target(base: &str, model: Option<&str>, version: Option<&str>) -> Option<String> {
    let model = model.map(str::trim).filter(|m| !m.is_empty())?;
    Some(match version.map(str::trim).filter(|v| !v.is_empty()) {
        Some(v) => format!("{base}/data-model/{model}/version/{v}"),
        None => format!("{base}/data-model/{model}"),
    })
}

/// The dataset's released versions (published or deprecated) as DCAT 3 §11
/// versions. A draft or staged version is work in progress, not a release,
/// and is not catalogued.
fn versions(
    g: &mut G,
    opts: &CatalogOptions,
    store: &TripleStore,
    ds: &Dataset,
    live: &NamedNode,
    shared: &Shared,
) {
    let base = opts.base_url.as_str();
    let mut released: Vec<DatasetVersion> =
        crate::dataset_versions::registry::list_versions(store, base, &ds.id)
            .into_iter()
            .filter(|v| {
                matches!(
                    v.status,
                    VersionStatus::Published | VersionStatus::Deprecated
                )
            })
            .collect();
    released.sort_by(|a, b| a.created_at.cmp(&b.created_at));
    let iri_of =
        |v: &DatasetVersion| -> Option<NamedNode> { NamedNode::new(v.graph_iri.as_str()).ok() };

    for (i, v) in released.iter().enumerate() {
        let Some(vi) = iri_of(v) else {
            continue;
        };
        g.add(live.clone(), &p(DCAT, "hasVersion"), vi.clone());
        g.typ(vi.clone(), &p(DCAT, "Dataset"));
        if opts.profile.is_ap() {
            g.typ(vi.clone(), &p(DCAT, "Resource"));
        }
        g.add(vi.clone(), &p(DCAT, "isVersionOf"), live.clone());
        g.lit(vi.clone(), &p(DCAT, "version"), &v.version);
        g.lang(
            vi.clone(),
            &p(DCT, "title"),
            &format!("{} {}", ds.name, v.version),
            opts.lang_tag(),
        );
        let notes = nonempty(v.notes.as_deref());
        g.lang(
            vi.clone(),
            &p(DCT, "description"),
            notes
                .as_deref()
                .unwrap_or(&format!("Version {} of {}.", v.version, ds.name)),
            opts.lang_tag(),
        );
        if let Some(n) = &notes {
            g.lang(vi.clone(), &p(ADMS, "versionNotes"), n, opts.lang_tag());
        }
        g.when(vi.clone(), &p(DCT, "issued"), &v.created_at);
        let status = match v.status {
            VersionStatus::Deprecated => "DEPRECATED",
            _ => "COMPLETED",
        };
        g.concept(
            vi.clone(),
            &p(ADMS, "status"),
            &format!("{}{status}", authority::EU_STATUS),
            "adms:status",
        );
        if opts.profile.is_ap() {
            g.lit(vi.clone(), &p(DCT, "identifier"), vi.as_str());
        }
        if let Some(target) = model_target(
            base,
            v.conforms_to_model.as_deref(),
            v.conforms_to_version.as_deref(),
        ) {
            g.ranged(
                vi.clone(),
                &p(DCT, "conformsTo"),
                &target,
                Range::Standard,
                "data model",
            );
        }
        describe_shared(g, opts, &vi, shared);
        // The previous version: the one it was derived from when that is a
        // release, else the release before it on the same branch.
        let prev = v
            .derived_from
            .as_deref()
            .and_then(|d| released[..i].iter().rev().find(|o| o.version == d))
            .or_else(|| released[..i].iter().rev().find(|o| o.branch == v.branch));
        if let Some(pv) = prev.and_then(iri_of) {
            g.add(vi.clone(), &p(DCAT, "previousVersion"), pv);
        }
        let url = format!(
            "{base}/api/datasets/{}/versions/{}/data",
            ds.id,
            // Validated on insert to IRI-safe characters; the route takes it as is.
            v.version
        );
        rdf_distribution(
            g,
            opts,
            vi.clone(),
            &format!("{} {} (TriG)", ds.name, v.version),
            &url,
            Some(&url),
            None,
            shared.license.as_deref(),
            true,
        );
    }
    // The current version: the newest published release on the main line,
    // else the newest published one on any branch.
    let published = |main: bool| {
        released
            .iter()
            .rev()
            .find(|v| matches!(v.status, VersionStatus::Published) && (!main || v.branch.is_none()))
    };
    if let Some(cur) = published(true)
        .or_else(|| published(false))
        .and_then(iri_of)
    {
        g.add(live.clone(), &p(DCAT, "hasCurrentVersion"), cur);
    }
}

/// The dataset's `dcat:CatalogRecord` (DCAT-AP 3 §4.1, optional; DCAT-AP-NL
/// asks the record for a language).
fn catalog_record(
    g: &mut G,
    opts: &CatalogOptions,
    catalog: &NamedNode,
    ds_iri: &NamedNode,
    ds: &Dataset,
) {
    let r = nn(&format!("{}/catalog/record/{}", opts.base_url, ds.id));
    g.add(catalog.clone(), &p(DCAT, "record"), r.clone());
    g.typ(r.clone(), &p(DCAT, "CatalogRecord"));
    g.add(r.clone(), &p(FOAF, "primaryTopic"), ds_iri.clone());
    g.when(r.clone(), &p(DCT, "issued"), &ds.created_at);
    g.when(r.clone(), &p(DCT, "modified"), &ds.updated_at);
    if let Some(profile) = opts.profile.standard() {
        g.ranged(
            r.clone(),
            &p(DCT, "conformsTo"),
            profile,
            Range::Standard,
            "application profile",
        );
    }
    g.ranged(
        r,
        &p(DCT, "language"),
        &opts.language_iri(),
        Range::LinguisticSystem,
        "language",
    );
}
#[cfg(test)]
mod tests {
    use super::*;
    use oxigraph::sparql::QueryResults;

    fn parse(ttl: &str) -> TripleStore {
        let s = TripleStore::in_memory().unwrap();
        s.load_str(ttl, RdfFormat::Turtle, None)
            .unwrap_or_else(|e| panic!("catalogue is not valid Turtle: {e}\n{ttl}"));
        s
    }
    fn ask(s: &TripleStore, q: &str) -> bool {
        matches!(s.query(q), Ok(QueryResults::Boolean(true)))
    }

    fn dataset_with_conformance(
        db: &Arc<AuthDb>,
        onto: Option<&str>,
        ver: Option<&str>,
    ) -> Dataset {
        db.create_dataset(
            "ds-1",
            "Library Catalogue 2025",
            None,
            OwnerType::User,
            "u1",
            Visibility::Public,
            None,
        )
        .unwrap();
        db.update_dataset_conformance("ds-1", onto, ver).unwrap();
        db.get_dataset("ds-1").unwrap().unwrap()
    }

    /// The conformance link must dereference at the model registry's
    /// `/data-model/` path — never the legacy `/ontology/`.
    #[test]
    fn conforms_to_uses_data_model_path_with_version() {
        let db = Arc::new(AuthDb::in_memory().unwrap());
        let store = TripleStore::in_memory().unwrap();
        dataset_with_conformance(&db, Some("library-catalogue-model"), Some("2.1.0"));
        let ttl = generate_dcat_catalog("http://example.org", &store, &db, None);
        let s = parse(&ttl);
        assert!(
            ask(&s, "ASK { <http://example.org/dataset/ds-1> <http://purl.org/dc/terms/conformsTo> <http://example.org/data-model/library-catalogue-model/version/2.1.0> }"),
            "{ttl}"
        );
        assert!(!ttl.contains("/ontology/library-catalogue-model"), "{ttl}");
    }

    #[test]
    fn conforms_to_uses_data_model_path_without_version() {
        let db = Arc::new(AuthDb::in_memory().unwrap());
        let store = TripleStore::in_memory().unwrap();
        dataset_with_conformance(&db, Some("library-catalogue-model"), None);
        let ttl = generate_dcat_catalog("http://example.org", &store, &db, None);
        let s = parse(&ttl);
        assert!(
            ask(&s, "ASK { <http://example.org/dataset/ds-1> <http://purl.org/dc/terms/conformsTo> <http://example.org/data-model/library-catalogue-model> }"),
            "{ttl}"
        );
    }

    /// `void:triples` sums every registered graph — including the verbose
    /// `…/ifcowl` lift graph — from the O(1) graph index.
    #[test]
    fn void_triples_counts_all_graphs_including_ifcowl() {
        let db = Arc::new(AuthDb::in_memory().unwrap());
        let store = TripleStore::in_memory().unwrap();
        db.create_dataset(
            "ds-graphs",
            "Graphs Dataset",
            None,
            OwnerType::User,
            "u1",
            Visibility::Public,
            None,
        )
        .unwrap();
        store
            .update("INSERT DATA { GRAPH <http://example.org/g/data> { <http://example.org/s1> <http://example.org/p> <http://example.org/o1> . <http://example.org/s2> <http://example.org/p> <http://example.org/o2> . } }")
            .unwrap();
        store
            .update("INSERT DATA { GRAPH <http://example.org/g/data/ifcowl> { <http://example.org/i1> <http://example.org/p> <http://example.org/o1> . <http://example.org/i2> <http://example.org/p> <http://example.org/o2> . <http://example.org/i3> <http://example.org/p> <http://example.org/o3> . } }")
            .unwrap();
        db.add_dataset_graph("ds-graphs", "http://example.org/g/data")
            .unwrap();
        db.add_dataset_graph("ds-graphs", "http://example.org/g/data/ifcowl")
            .unwrap();
        let ttl = generate_dcat_catalog("http://example.org", &store, &db, None);
        let s = parse(&ttl);
        assert!(
            ask(&s, "ASK { <http://example.org/dataset/ds-graphs> <http://rdfs.org/ns/void#triples> 5 ; <http://rdfs.org/ns/void#subset> <http://example.org/g/data/ifcowl> }"),
            "{ttl}"
        );
        // The aggregate statistics see the named graphs too (they used to run
        // over the default graph only, reporting 0 distinct subjects).
        assert!(
            ask(&s, "ASK { <http://example.org/dataset> <http://rdfs.org/ns/void#distinctSubjects> ?n . FILTER(?n >= 5) }"),
            "{ttl}"
        );
    }

    /// User-supplied values are terms, never syntax: a quote in the title, a
    /// `>` in the description and a malformed theme cannot break the document.
    #[test]
    fn hostile_metadata_cannot_corrupt_the_catalogue() {
        let db = Arc::new(AuthDb::in_memory().unwrap());
        let store = TripleStore::in_memory().unwrap();
        db.create_dataset(
            "ds-h",
            "Say \"hi\" .\n<urn:x> <urn:y> <urn:z> .",
            Some("a > b ; dcat:theme <urn:evil>"),
            OwnerType::User,
            "u1",
            Visibility::Public,
            None,
        )
        .unwrap();
        db.update_dataset_metadata(
            "ds-h",
            None,
            Some("[\"not an iri\", \"http://publications.europa.eu/resource/authority/data-theme/ENVI\"]"),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        let ttl = generate_dcat_catalog("http://example.org", &store, &db, None);
        let s = parse(&ttl);
        assert!(
            !ask(&s, "ASK { <urn:x> <urn:y> <urn:z> }"),
            "title text is not parsed as triples:\n{ttl}"
        );
        assert!(
            !ask(
                &s,
                "ASK { ?d <http://www.w3.org/ns/dcat#theme> <urn:evil> }"
            ),
            "{ttl}"
        );
    }
}
