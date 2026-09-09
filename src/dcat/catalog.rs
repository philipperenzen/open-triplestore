//! DCAT 3 catalogue generation — plain DCAT, DCAT-AP 3 or DCAT-AP-NL 3 —
//! from the dataset registry and the store.
//!
//! The catalogue is built as an RDF graph (a `Vec<Triple>`) and serialised
//! per request in whatever format was negotiated. Every user-supplied value
//! becomes a real RDF term: a title with a quote or a description with a `>`
//! cannot corrupt the document, and a malformed IRI (a theme, a licence, a
//! homepage) is dropped with a warning instead of being interpolated.
//!
//! What is emitted:
//! - the `dcat:Catalog` (title, description, publisher agent, language,
//!   licence, issued/modified, homepage, theme taxonomy, the SPARQL endpoint
//!   as `dcat:service`) and its datasets;
//! - a `dcat:Dataset` + `void:Dataset` per registered dataset the caller may
//!   see: Dublin Core, access rights, publisher/creator agents, PROV
//!   attribution, model and shapes conformance, per-graph `void:subset`s with
//!   roles, VoID counts, and distributions (SPARQL as a `dcat:DataService`,
//!   Graph Store, one download per graph, LDES when published, OGC API –
//!   Features / 3D Tiles / viewer feed when there is geometry);
//! - under an application profile, the properties DCAT-AP 3 and DCAT-AP-NL 3
//!   make mandatory: typed `foaf:Agent` publishers with names,
//!   `dct:identifier` + `adms:identifier`, `dct:language`, `dct:format`
//!   (EU file-type authority) and `dcat:mediaType` on every distribution,
//!   `adms:status` on the EU dataset-status authority, licences repeated on
//!   distributions (NL);
//! - the aggregate `void:Dataset` for the whole store with statistics that
//!   cover default and named graphs alike, cached until the next write.

use std::collections::HashSet;
use std::sync::Arc;

use oxigraph::io::{RdfFormat, RdfSerializer};
use oxigraph::model::{BlankNode, Literal, NamedNode, NamedOrBlankNode, Term, Triple};
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};

use crate::auth::db::AuthDb;
use crate::auth::models::{Dataset, Organisation, OwnerType, Visibility};
use crate::store::engine::TripleStore;

use super::vocabulary::*;

const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const SKOS: &str = "http://www.w3.org/2004/02/skos/core#";
const OTS: &str = "https://opentriplestore.org/ns#";
const EU_LANG: &str = "http://publications.europa.eu/resource/authority/language/";
const EU_FILETYPE: &str = "http://publications.europa.eu/resource/authority/file-type/";
const EU_STATUS: &str = "http://publications.europa.eu/resource/authority/dataset-status/";
const EU_ACCESS: &str = "http://publications.europa.eu/resource/authority/access-right/";
const EU_THEMES: &str = "http://publications.europa.eu/resource/authority/data-theme";
const IANA: &str = "https://www.iana.org/assignments/media-types/";
const SPARQL_PROTOCOL: &str = "https://www.w3.org/TR/sparql11-protocol/";
const GSP_PROTOCOL: &str = "https://www.w3.org/TR/sparql11-http-rdf-update/";
const LDES_SPEC: &str = "https://w3id.org/ldes/specification";

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
    pub fn from_env() -> Self {
        match std::env::var("DCAT_PROFILE")
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "dcat-ap" | "dcat_ap" | "dcatap" => Profile::DcatAp,
            "dcat-ap-nl" | "dcat_ap_nl" | "dcatapnl" => Profile::DcatApNl,
            _ => Profile::Dcat,
        }
    }
    pub fn is_ap(self) -> bool {
        !matches!(self, Profile::Dcat)
    }
}

/// Catalogue-level metadata, from the environment.
#[derive(Debug, Clone)]
pub struct CatalogOptions {
    pub base_url: String,
    pub profile: Profile,
    pub title: String,
    pub description: String,
    pub publisher_iri: String,
    pub publisher_name: String,
    pub publisher_identifier: Option<String>,
    /// ISO 639-3, upper case (`ENG`, `NLD`).
    pub language: String,
    pub license: Option<String>,
}

fn env_nonempty(k: &str) -> Option<String> {
    std::env::var(k)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

impl CatalogOptions {
    pub fn from_env(base_url: &str) -> Self {
        let base = base_url.trim_end_matches('/').to_string();
        let profile = Profile::from_env();
        let host = base
            .split("://")
            .nth(1)
            .unwrap_or(&base)
            .trim_end_matches('/')
            .to_string();
        Self {
            profile,
            title: env_nonempty("CATALOG_TITLE")
                .unwrap_or_else(|| "Open Triplestore Catalog".into()),
            description: env_nonempty("CATALOG_DESCRIPTION").unwrap_or_else(|| {
                format!("Datasets published by the Open Triplestore instance at {host}.")
            }),
            publisher_iri: env_nonempty("CATALOG_PUBLISHER_URI")
                .unwrap_or_else(|| format!("{base}/publisher")),
            publisher_name: env_nonempty("CATALOG_PUBLISHER_NAME")
                .unwrap_or_else(|| format!("Open Triplestore instance at {host}")),
            publisher_identifier: env_nonempty("CATALOG_PUBLISHER_IDENTIFIER"),
            language: env_nonempty("CATALOG_LANGUAGE")
                .map(|l| l.to_ascii_uppercase())
                .unwrap_or_else(|| {
                    if profile == Profile::DcatApNl {
                        "NLD".into()
                    } else {
                        "ENG".into()
                    }
                }),
            license: env_nonempty("CATALOG_LICENSE"),
            base_url: base,
        }
    }

    fn lang_tag(&self) -> &'static str {
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
    fn language_iri(&self) -> String {
        format!("{EU_LANG}{}", self.language)
    }
}

// ── graph builder ───────────────────────────────────────────────────────────

struct G {
    triples: Vec<Triple>,
    agents: HashSet<String>,
}

fn nn(s: &str) -> NamedNode {
    NamedNode::new_unchecked(s)
}

impl G {
    fn new() -> Self {
        Self {
            triples: Vec::with_capacity(256),
            agents: HashSet::new(),
        }
    }
    /// A user-supplied IRI, or none (with a warning) when it is not one.
    fn iri(&self, s: &str, what: &str) -> Option<NamedNode> {
        match NamedNode::new(s.trim()) {
            Ok(n) => Some(n),
            Err(e) => {
                tracing::warn!("dcat: dropping {what} `{s}`: not an IRI ({e})");
                None
            }
        }
    }
    fn add(&mut self, s: impl Into<NamedOrBlankNode>, p: &str, o: impl Into<Term>) {
        self.triples.push(Triple::new(s, nn(p), o));
    }
    fn typ(&mut self, s: impl Into<NamedOrBlankNode>, class: &str) {
        self.add(s, RDF_TYPE, nn(class));
    }
    fn lit(&mut self, s: impl Into<NamedOrBlankNode>, p: &str, text: &str) {
        self.add(s, p, Literal::new_simple_literal(text));
    }
    fn lang(&mut self, s: impl Into<NamedOrBlankNode>, p: &str, text: &str, lang: &str) {
        match Literal::new_language_tagged_literal(text, lang) {
            Ok(l) => self.add(s, p, l),
            Err(_) => self.lit(s, p, text),
        }
    }
    fn typed(&mut self, s: impl Into<NamedOrBlankNode>, p: &str, text: &str, dt: &str) {
        self.add(s, p, Literal::new_typed_literal(text, nn(dt)));
    }
    fn int(&mut self, s: impl Into<NamedOrBlankNode>, p: &str, n: usize) {
        self.add(s, p, Literal::from(n as i64));
    }
    /// Add `s p <o>` when `o` is a valid IRI.
    fn link(&mut self, s: impl Into<NamedOrBlankNode>, p: &str, o: &str, what: &str) -> bool {
        match self.iri(o, what) {
            Some(n) => {
                self.add(s, p, n);
                true
            }
            None => false,
        }
    }
}

fn dt(s: &str) -> &'static str {
    let _ = s;
    "http://www.w3.org/2001/XMLSchema#dateTime"
}

fn p(ns: &str, local: &str) -> String {
    format!("{ns}{local}")
}

fn enc(s: &str) -> String {
    utf8_percent_encode(s, NON_ALPHANUMERIC).to_string()
}

// ── public API ──────────────────────────────────────────────────────────────

/// The catalogue as triples: the whole instance, or one organisation's slice.
pub fn build_catalog(
    opts: &CatalogOptions,
    store: &TripleStore,
    auth_db: &Arc<AuthDb>,
    user_id: Option<&str>,
    scope: Option<&Organisation>,
) -> Vec<Triple> {
    let base = opts.base_url.as_str();
    let mut g = G::new();

    let datasets: Vec<Dataset> = match scope {
        Some(org) => auth_db.list_datasets_by_org(&org.id).unwrap_or_default(),
        None => auth_db.list_datasets().unwrap_or_default(),
    }
    .into_iter()
    .filter(|ds| auth_db.can_access_dataset(user_id, ds).unwrap_or(false))
    .collect();

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
    g.link(
        catalog.clone(),
        &p(DCT, "language"),
        &opts.language_iri(),
        "language",
    );
    if let Some(l) = &opts.license {
        g.link(catalog.clone(), &p(DCT, "license"), l, "catalogue licence");
    }
    g.link(
        catalog.clone(),
        &p(FOAF, "homepage"),
        &match scope {
            Some(org) => format!("{base}/{}/", org.slug),
            None => format!("{base}/"),
        },
        "homepage",
    );
    g.add(catalog.clone(), &p(DCAT, "themeTaxonomy"), nn(EU_THEMES));
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
    g.typed(catalog.clone(), &p(DCT, "issued"), &issued, dt(&issued));
    g.typed(
        catalog.clone(),
        &p(DCT, "modified"),
        &modified,
        dt(&modified),
    );
    let sparql = nn(&format!("{base}/sparql"));
    g.add(catalog.clone(), &p(DCAT, "service"), sparql.clone());
    if scope.is_none() {
        g.add(
            catalog.clone(),
            &p(DCAT, "dataset"),
            nn(&format!("{base}/dataset")),
        );
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
        aggregate_dataset(&mut g, opts, store, &publisher);
    }

    // ── per-dataset entries ──
    for ds in &datasets {
        dataset_entry(&mut g, opts, store, auth_db, ds);
    }

    // ── the SPARQL service ──
    g.typ(sparql.clone(), &p(SD, "Service"));
    g.typ(sparql.clone(), &p(DCAT, "DataService"));
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
    g.lang(sparql.clone(), &p(DCT, "title"), "SPARQL endpoint", "en");
    g.add(sparql.clone(), &p(DCAT, "endpointURL"), sparql.clone());
    g.add(
        sparql.clone(),
        &p(DCAT, "endpointDescription"),
        nn(&format!("{base}/")),
    );
    g.add(sparql.clone(), &p(DCT, "conformsTo"), nn(SPARQL_PROTOCOL));
    if opts.profile.is_ap() {
        g.add(sparql.clone(), &p(DCT, "publisher"), publisher);
    }

    g.triples
}

/// Serialise the catalogue in `format` (Turtle gets the usual prefixes).
pub fn serialize_catalog(triples: &[Triple], format: RdfFormat) -> Result<Vec<u8>, String> {
    let mut ser = RdfSerializer::from_format(format);
    if matches!(format, RdfFormat::Turtle | RdfFormat::TriG) {
        for (pfx, ns) in [
            ("dcat", DCAT),
            ("dct", DCT),
            ("void", VOID),
            ("foaf", FOAF),
            ("prov", PROV),
            ("org", ORG),
            ("adms", ADMS),
            ("schema", SCHEMA),
            ("vcard", VCARD),
            ("xsd", XSD),
            ("sd", SD),
            ("skos", SKOS),
            ("ots", OTS),
        ] {
            ser = ser.with_prefix(pfx, ns).map_err(|e| e.to_string())?;
        }
    }
    let mut buf = Vec::with_capacity(triples.len() * 96);
    let mut w = ser.for_writer(&mut buf);
    for t in triples {
        w.serialize_triple(t.as_ref()).map_err(|e| e.to_string())?;
    }
    w.finish().map_err(|e| e.to_string())?;
    Ok(buf)
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
    let triples = build_catalog(&opts, store, auth_db, user_id, org);
    serialize_catalog(&triples, format)
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

// ── agents ──────────────────────────────────────────────────────────────────

fn catalog_publisher(g: &mut G, opts: &CatalogOptions) -> NamedNode {
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
        g.link(
            iri.clone(),
            &p(FOAF, "homepage"),
            &format!("{}/", opts.base_url),
            "homepage",
        );
    }
    iri
}

fn org_agent(g: &mut G, opts: &CatalogOptions, org: &Organisation) -> NamedNode {
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
        g.link(
            iri.clone(),
            &p(FOAF, "homepage"),
            h,
            "organisation homepage",
        );
    }
    if let Some(id) = &org.identifier {
        g.lit(iri.clone(), &p(DCT, "identifier"), id);
    }
    if org.contact_name.is_some() || org.contact_email.is_some() || org.contact_url.is_some() {
        contact_point(
            g,
            iri.clone(),
            org.contact_name.as_deref(),
            org.contact_email.as_deref(),
            org.contact_url.as_deref(),
        );
    }
    iri
}

fn user_agent(g: &mut G, opts: &CatalogOptions, auth_db: &Arc<AuthDb>, user_id: &str) -> NamedNode {
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

fn contact_point(
    g: &mut G,
    subject: impl Into<NamedOrBlankNode>,
    name: Option<&str>,
    email: Option<&str>,
    url: Option<&str>,
) {
    let name = name.map(str::trim).filter(|s| !s.is_empty());
    let email = email.map(str::trim).filter(|s| !s.is_empty());
    let url = url.map(str::trim).filter(|s| !s.is_empty());
    if name.is_none() && email.is_none() && url.is_none() {
        return;
    }
    let cp = BlankNode::default();
    g.add(subject, &p(DCAT, "contactPoint"), cp.clone());
    g.typ(cp.clone(), &p(VCARD, "Kind"));
    g.typ(cp.clone(), &p(VCARD, "Organization"));
    if let Some(n) = name {
        g.lit(cp.clone(), &p(VCARD, "fn"), n);
    }
    if let Some(e) = email {
        g.link(
            cp.clone(),
            &p(VCARD, "hasEmail"),
            &format!("mailto:{e}"),
            "contact e-mail",
        );
    }
    if let Some(u) = url {
        g.link(cp.clone(), &p(VCARD, "hasURL"), u, "contact URL");
    }
}

// ── the aggregate dataset ───────────────────────────────────────────────────

fn aggregate_dataset(g: &mut G, opts: &CatalogOptions, store: &TripleStore, publisher: &NamedNode) {
    let base = opts.base_url.as_str();
    let root = nn(&format!("{base}/dataset"));
    let stats = store.void_stats();
    g.typ(root.clone(), &p(VOID, "Dataset"));
    g.typ(root.clone(), &p(DCAT, "Dataset"));
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
    if opts.profile.is_ap() {
        g.add(root.clone(), &p(DCT, "publisher"), publisher.clone());
        g.lit(
            root.clone(),
            &p(DCT, "identifier"),
            &format!("{base}/dataset"),
        );
        g.link(
            root.clone(),
            &p(DCT, "language"),
            &opts.language_iri(),
            "language",
        );
        g.add(
            root.clone(),
            &p(DCT, "accessRights"),
            nn(&format!("{EU_ACCESS}PUBLIC")),
        );
    }
    sparql_distribution(g, opts, root.clone(), opts.license.as_deref());
    g.add(
        root.clone(),
        &p(DCAT, "landingPage"),
        nn(&format!("{base}/")),
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
            g.link(d.clone(), &p(DCT, "license"), l, "distribution licence");
        }
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
    g.add(d.clone(), &p(DCT, "conformsTo"), nn(SPARQL_PROTOCOL));
    g.add(
        d.clone(),
        &p(DCAT, "mediaType"),
        nn(&format!("{IANA}application/sparql-results+json")),
    );
    if opts.profile.is_ap() {
        g.add(
            d.clone(),
            &p(DCT, "format"),
            nn(&format!("{EU_FILETYPE}SPARQLQ")),
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn turtle_distribution(
    g: &mut G,
    opts: &CatalogOptions,
    ds: NamedNode,
    title: &str,
    access_url: &str,
    download_url: Option<&str>,
    conforms_to: Option<&str>,
    license: Option<&str>,
) {
    let d = BlankNode::default();
    g.add(ds, &p(DCAT, "distribution"), d.clone());
    distribution_common(g, opts, &d, title, license);
    g.add(d.clone(), &p(DCAT, "accessURL"), nn(access_url));
    if let Some(u) = download_url {
        g.add(d.clone(), &p(DCAT, "downloadURL"), nn(u));
    }
    g.add(
        d.clone(),
        &p(DCAT, "mediaType"),
        nn(&format!("{IANA}text/turtle")),
    );
    if opts.profile.is_ap() {
        g.add(
            d.clone(),
            &p(DCT, "format"),
            nn(&format!("{EU_FILETYPE}RDF_TURTLE")),
        );
    }
    if let Some(c) = conforms_to {
        g.add(d.clone(), &p(DCT, "conformsTo"), nn(c));
    }
}

// ── per-dataset entry ───────────────────────────────────────────────────────

fn status_iri(g: &G, s: &str) -> Option<NamedNode> {
    let t = s.trim();
    if t.is_empty() {
        return None;
    }
    let key = t.to_ascii_uppercase().replace([' ', '-'], "_");
    match key.as_str() {
        "COMPLETED" | "DEPRECATED" | "UNDER_DEVELOPMENT" | "WITHDRAWN" | "DEVELOP"
        | "DISCONTINUED" => Some(nn(&format!("{EU_STATUS}{key}"))),
        _ => g.iri(t, "adms:status"),
    }
}

fn dataset_entry(
    g: &mut G,
    opts: &CatalogOptions,
    store: &TripleStore,
    auth_db: &Arc<AuthDb>,
    ds: &Dataset,
) {
    let base = opts.base_url.as_str();
    let ap = opts.profile.is_ap();
    let ds_iri = nn(&format!("{base}/dataset/{}", ds.id));
    let s = ds_iri.clone();

    g.typ(s.clone(), &p(DCAT, "Dataset"));
    g.typ(s.clone(), &p(VOID, "Dataset"));
    g.lang(s.clone(), &p(DCT, "title"), &ds.name, opts.lang_tag());
    let description = ds
        .description
        .as_deref()
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .map(str::to_string)
        .or_else(|| if ap { Some(ds.name.clone()) } else { None });
    if let Some(d) = description {
        g.lang(s.clone(), &p(DCT, "description"), &d, opts.lang_tag());
    }
    g.typed(
        s.clone(),
        &p(DCT, "issued"),
        &ds.created_at,
        dt(&ds.created_at),
    );
    g.typed(
        s.clone(),
        &p(DCT, "modified"),
        &ds.updated_at,
        dt(&ds.updated_at),
    );
    let access = match ds.visibility {
        Visibility::Public => "PUBLIC",
        Visibility::Members => "RESTRICTED",
        Visibility::Private => "NON_PUBLIC",
    };
    g.add(
        s.clone(),
        &p(DCT, "accessRights"),
        nn(&format!("{EU_ACCESS}{access}")),
    );
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
        g.link(
            s.clone(),
            &p(DCT, "language"),
            &opts.language_iri(),
            "language",
        );
    }

    // Publisher / creator agents.
    let agent = match ds.owner_type {
        OwnerType::Organisation => match auth_db.get_organisation(&ds.owner_id) {
            Ok(Some(org)) => org_agent(g, opts, &org),
            _ => nn(&format!("{base}/org/{}", ds.owner_id)),
        },
        OwnerType::User => user_agent(g, opts, auth_db, &ds.owner_id),
        OwnerType::Group => group_agent(g, opts, auth_db, &ds.owner_id),
    };
    match ds.owner_type {
        OwnerType::User => {
            g.add(s.clone(), &p(DCT, "creator"), agent.clone());
            if ap {
                g.add(s.clone(), &p(DCT, "publisher"), agent.clone());
            }
        }
        _ => g.add(s.clone(), &p(DCT, "publisher"), agent.clone()),
    }
    g.add(s.clone(), &p(PROV, "wasAttributedTo"), agent.clone());

    // Graphs, roles, counts, provenance.
    let entries = auth_db
        .list_dataset_graph_entries(&ds.id)
        .unwrap_or_default();
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
        g.add(
            s.clone(),
            &p(PROV, "wasGeneratedBy"),
            nn(&format!("{base}/commit/{}", c.commit_id)),
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

    // Conformance.
    if ds.shacl_on_write {
        if let Some(shapes) = &ds.shapes_graph_iri {
            g.link(s.clone(), &p(DCT, "conformsTo"), shapes, "shapes graph");
        }
    }
    if let Some(model) = &ds.conforms_to_model {
        let target = match &ds.conforms_to_version {
            Some(v) => format!("{base}/data-model/{model}/version/{v}"),
            None => format!("{base}/data-model/{model}"),
        };
        g.add(s.clone(), &p(DCT, "conformsTo"), nn(&target));
    }

    // DCAT metadata.
    let license = ds
        .license
        .as_deref()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .or_else(|| if ap { opts.license.clone() } else { None });
    if let Some(l) = &license {
        g.link(s.clone(), &p(DCT, "license"), l, "dataset licence");
    }
    if let Some(themes) = ds
        .themes
        .as_deref()
        .and_then(|j| serde_json::from_str::<Vec<String>>(j).ok())
    {
        for t in themes.iter().filter(|t| !t.trim().is_empty()) {
            g.link(s.clone(), &p(DCAT, "theme"), t, "dataset theme");
        }
    }
    if let Some(kws) = ds
        .keywords
        .as_deref()
        .and_then(|j| serde_json::from_str::<Vec<String>>(j).ok())
    {
        for k in kws.iter().filter(|k| !k.trim().is_empty()) {
            g.lang(s.clone(), &p(DCAT, "keyword"), k.trim(), opts.lang_tag());
        }
    }
    if let Some(st) = ds.adms_status.as_deref().and_then(|x| status_iri(g, x)) {
        g.add(s.clone(), &p(ADMS, "status"), st);
    }
    if let Some(n) = ds
        .version_notes
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
    {
        g.lang(s.clone(), &p(ADMS, "versionNotes"), n, opts.lang_tag());
    }
    if let Some(sp) = ds
        .spatial
        .as_deref()
        .map(str::trim)
        .filter(|x| !x.is_empty())
    {
        g.link(s.clone(), &p(DCT, "spatial"), sp, "dct:spatial");
    }
    contact_point(
        g,
        s.clone(),
        ds.contact_name.as_deref(),
        ds.contact_email.as_deref(),
        ds.contact_url.as_deref(),
    );

    // Versions (DCAT 3): the newest published version, and every version as a
    // dcat:hasVersion link.
    let versions = crate::dataset_versions::registry::list_versions(store, base, &ds.id);
    for v in &versions {
        g.add(
            s.clone(),
            &p(DCAT, "hasVersion"),
            nn(&format!(
                "{base}/dataset/{}/version/{}",
                ds.id,
                enc(&v.version)
            )),
        );
    }
    if let Some(v) = versions
        .iter()
        .filter(|v| {
            matches!(
                v.status,
                crate::dataset_versions::models::VersionStatus::Published
            )
        })
        .max_by(|a, b| a.created_at.cmp(&b.created_at))
    {
        g.lit(s.clone(), &p(DCAT, "version"), &v.version);
    }

    // Distributions.
    sparql_distribution(g, opts, s.clone(), license.as_deref());
    turtle_distribution(
        g,
        opts,
        s.clone(),
        "Graph Store HTTP Protocol",
        &format!("{base}/store"),
        None,
        Some(GSP_PROTOCOL),
        license.as_deref(),
    );
    for e in entries
        .iter()
        .filter(|e| !e.graph_iri.starts_with("urn:system:"))
    {
        if g.iri(&e.graph_iri, "graph IRI").is_none() {
            continue;
        }
        let url = format!("{base}/store?graph={}", enc(&e.graph_iri));
        turtle_distribution(
            g,
            opts,
            s.clone(),
            &format!("Graph {}", e.graph_iri),
            &url,
            Some(&url),
            None,
            license.as_deref(),
        );
    }
    if matches!(crate::ldes::store::stream(auth_db, &ds.id), Ok(Some(cfg)) if cfg.enabled) {
        turtle_distribution(
            g,
            opts,
            s.clone(),
            "Linked Data Event Stream",
            &crate::ldes::stream_iri(base, &ds.id),
            None,
            Some(LDES_SPEC),
            license.as_deref(),
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
    if geo.has_coordinates || geo.has_3d {
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
        g.add(
            d.clone(),
            &p(DCAT, "mediaType"),
            nn(&format!("{IANA}application/geo+json")),
        );
        if ap {
            g.add(
                d.clone(),
                &p(DCT, "format"),
                nn(&format!("{EU_FILETYPE}GEOJSON")),
            );
        }
        g.add(
            d.clone(),
            &p(DCT, "conformsTo"),
            nn("http://www.opengis.net/spec/ogcapi-features-1/1.0/conf/core"),
        );
        let svc = nn(&format!("{base}/api/ogc"));
        g.add(d.clone(), &p(DCAT, "accessService"), svc.clone());
        g.typ(svc.clone(), &p(DCAT, "DataService"));
        g.lang(svc.clone(), &p(DCT, "title"), "OGC API – Features", "en");
        g.add(svc.clone(), &p(DCAT, "endpointURL"), svc.clone());
        g.add(
            svc.clone(),
            &p(DCT, "conformsTo"),
            nn("http://www.opengis.net/spec/ogcapi-features-1/1.0/conf/core"),
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
            g.add(
                d.clone(),
                &p(DCAT, "mediaType"),
                nn(&format!("{IANA}application/json")),
            );
            if ap {
                g.add(
                    d.clone(),
                    &p(DCT, "format"),
                    nn(&format!("{EU_FILETYPE}JSON")),
                );
            }
            g.add(
                d.clone(),
                &p(DCT, "conformsTo"),
                nn("https://docs.ogc.org/cs/22-025r4/22-025r4.html"),
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
        g.add(
            d.clone(),
            &p(DCAT, "mediaType"),
            nn(&format!("{IANA}application/json")),
        );
        if ap {
            g.add(
                d.clone(),
                &p(DCT, "format"),
                nn(&format!("{EU_FILETYPE}JSON")),
            );
        }
    }

    let landing = ds
        .landing_page
        .as_deref()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("{base}/"));
    if !g.link(s.clone(), &p(DCAT, "landingPage"), &landing, "landing page") {
        g.add(s.clone(), &p(DCAT, "landingPage"), nn(&format!("{base}/")));
    }
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
