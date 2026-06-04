use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

// ─── System-level role (user account role) ───────────────────────────────────

/// System-wide role for a user account.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SystemRole {
    SuperAdmin,
    Admin,
    User,
}

impl SystemRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            SystemRole::SuperAdmin => "super_admin",
            SystemRole::Admin => "admin",
            SystemRole::User => "user",
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "super_admin" => Some(SystemRole::SuperAdmin),
            "admin" => Some(SystemRole::Admin),
            // "publisher" was a legacy role; migrated to can_publish flag on User
            "user" | "publisher" => Some(SystemRole::User),
            _ => None,
        }
    }

    /// Returns true if this role is admin-level or above.
    pub fn is_admin(&self) -> bool {
        matches!(self, SystemRole::SuperAdmin | SystemRole::Admin)
    }

    /// Returns the privilege level (higher = more privileged).
    pub fn level(&self) -> u8 {
        match self {
            SystemRole::User => 0,
            SystemRole::Admin => 1,
            SystemRole::SuperAdmin => 2,
        }
    }
}

// ─── IdP claim → role / capability mapping ────────────────────────────────────

/// Outcome of mapping external IdP claim/group values through a provider's
/// `role_claim_map`. Produced during SSO provisioning.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MappedClaims {
    /// Highest-privilege system role matched by a claim value, or `None` when
    /// no claim mapped to a role. Callers fall back to the provider default and
    /// must never *downgrade* an existing user just because claims were absent.
    pub role: Option<SystemRole>,
    /// True when a claim value mapped to the `"publisher"` grant, which is a
    /// capability (`can_publish`) rather than a role. Applied non-destructively:
    /// SSO only ever *sets* it, never clears it.
    pub grant_publish: bool,
}

/// Map external IdP claim/group values to a [`SystemRole`] and capability flags
/// using a `role_claim_map` — a JSON object of
/// `{ "claim_or_group_value": "super_admin" | "admin" | "user" | "publisher" }`.
///
/// The strongest matched role wins. The literal grant `"publisher"` sets
/// [`MappedClaims::grant_publish`] *without* contributing a role (publisher
/// became a capability, not a role). Returns an empty result when the map is
/// missing/empty or nothing matched.
pub fn map_claims_to_role(claim_values: &[String], role_claim_map: Option<&str>) -> MappedClaims {
    let map: std::collections::HashMap<String, String> = role_claim_map
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();
    let mut out = MappedClaims::default();
    if map.is_empty() {
        return out;
    }
    for value in claim_values {
        let Some(mapped) = map.get(value) else {
            continue;
        };
        if mapped == "publisher" {
            out.grant_publish = true;
            continue;
        }
        if let Some(role) = SystemRole::from_str(mapped) {
            out.role = Some(match out.role {
                Some(best) if best.level() >= role.level() => best,
                _ => role,
            });
        }
    }
    out
}

// ─── Canonical access-capability ladder ───────────────────────────────────────

/// The single capability ladder shared across the system. Per-resource grants,
/// API-token scopes, and named-graph ACLs all express the same idea —
/// *read → write → manage*. Ordered (`Read < Write < Manage`) so `max()` of
/// several grants yields the strongest.
///
/// [`ResourceRole`] (`viewer`/`editor`/`admin`) and [`ApiScope`]
/// (`read`/`write`/`admin`) are wire-format *presentations* of this ladder and
/// convert into it via [`From`]. Make capability decisions and comparisons on
/// `AccessLevel`; keep the presentation types only at the API/storage boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum AccessLevel {
    Read,
    Write,
    Manage,
}

impl AccessLevel {
    /// Parse any equivalent spelling used across the system:
    /// `read`/`viewer`, `write`/`editor`, `manage`/`admin`/`owner`.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "read" | "viewer" => Some(AccessLevel::Read),
            "write" | "editor" => Some(AccessLevel::Write),
            "manage" | "admin" | "owner" => Some(AccessLevel::Manage),
            _ => None,
        }
    }

    /// Always true — every level includes read.
    pub fn can_read(&self) -> bool {
        true
    }

    /// May modify content (write or manage).
    pub fn can_write(&self) -> bool {
        *self >= AccessLevel::Write
    }

    /// May manage settings, metadata and access grants (manage only).
    pub fn can_manage(&self) -> bool {
        *self >= AccessLevel::Manage
    }

    /// Every stored permission spelling (across resource grants, token scopes
    /// and graph ACLs) that satisfies a requirement of at least `self`. Used to
    /// build ACL `IN (…)` filters without hardcoding the hierarchy.
    pub fn satisfying_spellings(&self) -> &'static [&'static str] {
        match self {
            AccessLevel::Read => &[
                "read", "viewer", "write", "editor", "manage", "admin", "owner",
            ],
            AccessLevel::Write => &["write", "editor", "manage", "admin", "owner"],
            AccessLevel::Manage => &["manage", "admin", "owner"],
        }
    }
}

impl From<ResourceRole> for AccessLevel {
    fn from(r: ResourceRole) -> Self {
        match r {
            ResourceRole::Viewer => AccessLevel::Read,
            ResourceRole::Editor => AccessLevel::Write,
            ResourceRole::Admin => AccessLevel::Manage,
        }
    }
}

impl From<ApiScope> for AccessLevel {
    fn from(s: ApiScope) -> Self {
        match s {
            ApiScope::Read => AccessLevel::Read,
            ApiScope::Write => AccessLevel::Write,
            ApiScope::Admin => AccessLevel::Manage,
        }
    }
}

// ─── API token scopes ────────────────────────────────────────────────────────

/// Scope for an API token.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ApiScope {
    Read,
    Write,
    Admin,
}

impl ApiScope {
    pub fn as_str(&self) -> &'static str {
        match self {
            ApiScope::Read => "read",
            ApiScope::Write => "write",
            ApiScope::Admin => "admin",
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "read" => Some(ApiScope::Read),
            "write" => Some(ApiScope::Write),
            "admin" => Some(ApiScope::Admin),
            _ => None,
        }
    }

    /// Parse a comma-separated scopes string.
    pub fn parse_scopes(s: &str) -> Vec<Self> {
        s.split(',')
            .filter_map(|part| Self::from_str(part.trim()))
            .collect()
    }

    /// Convert scopes to a comma-separated string.
    pub fn scopes_to_string(scopes: &[Self]) -> String {
        scopes
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join(",")
    }
}

// ─── User ────────────────────────────────────────────────────────────────────

/// A user account.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct User {
    pub id: String,
    pub username: String,
    pub email: String,
    #[serde(skip_serializing)]
    pub password_hash: String,
    pub role: SystemRole,
    pub is_active: bool,
    pub is_public: bool,
    /// Addon permission: user may create/edit/upload/publish model and vocabulary versions.
    /// Admins and super-admins always have this implicitly.
    pub can_publish: bool,
    pub avatar_key: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    // FOAF / VCARD profile fields
    pub display_name: Option<String>,
    pub bio: Option<String>,
    pub website: Option<String>,
    pub phone: Option<String>,
    pub organization: Option<String>,
}

impl User {
    pub fn is_admin(&self) -> bool {
        self.role.is_admin()
    }

    /// Returns true if this user can create/edit/upload/publish model and vocabulary versions.
    pub fn is_publisher(&self) -> bool {
        self.role.is_admin() || self.can_publish
    }
}

// ─── API Token ───────────────────────────────────────────────────────────────

/// A long-lived API token for programmatic access.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ApiToken {
    pub id: String,
    pub user_id: String,
    pub name: String,
    #[serde(skip_serializing)]
    pub token_hash: String,
    pub token_prefix: String,
    pub scopes: Vec<ApiScope>,
    pub expires_at: Option<String>,
    pub last_used_at: Option<String>,
    pub created_at: String,
    pub revoked: bool,
}

// ─── Refresh Token ───────────────────────────────────────────────────────────

/// A refresh token for obtaining new access tokens.
#[derive(Debug, Clone)]
pub struct RefreshToken {
    pub id: String,
    pub user_id: String,
    pub token_hash: String,
    pub expires_at: String,
    pub created_at: String,
    pub revoked: bool,
}

// ─── Organisation ────────────────────────────────────────────────────────────

/// An organisation.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct Organisation {
    pub id: String,
    pub name: String,
    pub slug: String,
    pub description: Option<String>,
    pub image_key: Option<String>,
    /// Wide banner/header image object-store key. Distinct from `image_key` (the icon/logo).
    pub banner_key: Option<String>,
    pub created_at: String,
    /// `foaf:homepage` — the organisation's primary web page IRI/URL.
    pub homepage: Option<String>,
    /// `dct:identifier` — an official identifier (e.g., KVK, LEI, company registration).
    pub identifier: Option<String>,
    /// Contact person/team name → `vcard:fn`.
    pub contact_name: Option<String>,
    /// Contact e-mail address → `vcard:hasEmail`.
    pub contact_email: Option<String>,
    /// Contact web page → `vcard:hasURL`.
    pub contact_url: Option<String>,
    /// RDF type extension: `"Organization"` | `"FormalOrganization"` | `"OrganizationalUnit"`.
    /// Always combined with `foaf:Organization`; defaults to `"FormalOrganization"`.
    pub org_type: Option<String>,
    /// Parent organisation in the org hierarchy (`org:subOrganizationOf`).
    /// `None` for a top-level organisation.
    pub parent_org_id: Option<String>,
}

/// A group within an organisation (can be nested).
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct Group {
    pub id: String,
    pub org_id: String,
    pub name: String,
    pub parent_group_id: Option<String>,
    pub created_at: String,
}

/// Membership role (within an org or group).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Admin,
    Member,
    Viewer,
}

impl Role {
    pub fn as_str(&self) -> &'static str {
        match self {
            Role::Admin => "admin",
            Role::Member => "member",
            Role::Viewer => "viewer",
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "admin" => Some(Role::Admin),
            "member" => Some(Role::Member),
            "viewer" => Some(Role::Viewer),
            _ => None,
        }
    }
}

// ─── Per-resource permission level ─────────────────────────────────────────────

/// Effective permission level a principal holds on a single resource
/// (dataset, model, or vocabulary). Ordered: `Viewer < Editor < Admin`, so
/// `max()` of several grants yields the strongest one.
///
/// * `Viewer` — read only; cannot modify data.
/// * `Editor` — read + modify the resource's data/content.
/// * `Admin`  — everything an editor can do, plus manage the resource's
///   settings, metadata and access grants (owner-equivalent).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ResourceRole {
    Viewer,
    Editor,
    Admin,
}

impl ResourceRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            ResourceRole::Viewer => "viewer",
            ResourceRole::Editor => "editor",
            ResourceRole::Admin => "admin",
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "viewer" => Some(ResourceRole::Viewer),
            "editor" => Some(ResourceRole::Editor),
            "admin" | "owner" => Some(ResourceRole::Admin),
            _ => None,
        }
    }

    /// The default resource role implied by an org/group membership role.
    /// A plain member can edit data; a viewer is read-only; an org/group
    /// admin manages the resource.
    pub fn from_membership(role: Role) -> Self {
        match role {
            Role::Admin => ResourceRole::Admin,
            Role::Member => ResourceRole::Editor,
            Role::Viewer => ResourceRole::Viewer,
        }
    }

    /// May read the resource (always true — every level includes read).
    pub fn can_read(&self) -> bool {
        AccessLevel::from(*self).can_read()
    }

    /// May modify the resource's data/content (editor or admin).
    pub fn can_write(&self) -> bool {
        AccessLevel::from(*self).can_write()
    }

    /// May manage the resource's settings, metadata and access grants (admin).
    pub fn can_manage(&self) -> bool {
        AccessLevel::from(*self).can_manage()
    }
}

/// A single per-resource access grant row.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ResourceGrant {
    pub id: String,
    pub resource_type: String,
    pub resource_id: String,
    /// "user" | "group"
    pub principal_type: String,
    pub principal_id: String,
    /// "viewer" | "editor" | "admin"
    pub role: String,
    pub created_at: String,
    pub created_by: String,
}

// ─── Dataset ─────────────────────────────────────────────────────────────────

/// Dataset visibility.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Visibility {
    Public,
    Members,
    Private,
}

impl Visibility {
    pub fn as_str(&self) -> &'static str {
        match self {
            Visibility::Public => "public",
            Visibility::Members => "members",
            Visibility::Private => "private",
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "public" => Some(Visibility::Public),
            "members" => Some(Visibility::Members),
            "private" => Some(Visibility::Private),
            _ => None,
        }
    }
}

/// Owner type for a dataset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum OwnerType {
    User,
    Organisation,
    Group,
}

impl OwnerType {
    pub fn as_str(&self) -> &'static str {
        match self {
            OwnerType::User => "user",
            OwnerType::Organisation => "organisation",
            OwnerType::Group => "group",
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "user" => Some(OwnerType::User),
            "organisation" => Some(OwnerType::Organisation),
            "group" => Some(OwnerType::Group),
            _ => None,
        }
    }
}

/// The logical *kind* / box classification of a dataset or named graph.
///
/// This is a content classification, **not** an access-control role — it is
/// unrelated to [`SystemRole`], [`ResourceRole`], or [`AccessLevel`]. The
/// owning field and DB column remain named `graph_role` for backward compat.
///
/// * `Instances` — instance data / assertions (default for user-created datasets).
/// * `Model`     — OWL/RDFS terminological schema (classes and properties).
/// * `Vocabulary` — SKOS concept schemes and controlled vocabularies.
/// * `Shapes`    — SHACL shape graphs used to validate instance data.
/// * `Entailment` — materialised inference results (written by the reasoner).
/// * `System`    — internal system graphs (registry metadata, etc.).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum GraphKind {
    Instances,
    Model,
    Vocabulary,
    Shapes,
    Entailment,
    System,
}

impl GraphKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            GraphKind::Instances => "instances",
            GraphKind::Model => "model",
            GraphKind::Vocabulary => "vocabulary",
            GraphKind::Shapes => "shapes",
            GraphKind::Entailment => "entailment",
            GraphKind::System => "system",
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "instances" | "abox" => Some(GraphKind::Instances),
            "model" | "tbox" => Some(GraphKind::Model),
            "vocabulary" => Some(GraphKind::Vocabulary),
            "shapes" => Some(GraphKind::Shapes),
            "entailment" => Some(GraphKind::Entailment),
            "system" => Some(GraphKind::System),
            _ => None,
        }
    }
}

/// A named graph registered to a dataset, with its optional role.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct DatasetGraphEntry {
    pub graph_iri: String,
    pub graph_role: Option<GraphKind>,
    /// When true, the graph is hidden from dataset viewers and the public; only
    /// principals who can write the owning dataset can see or query it.
    pub private: bool,
}

/// A dataset with access control.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct Dataset {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub owner_type: OwnerType,
    pub owner_id: String,
    pub visibility: Visibility,
    pub shacl_on_write: bool,
    pub shapes_graph_iri: Option<String>,
    /// Model registry ID this dataset's instance data conforms to.
    pub conforms_to_ontology: Option<String>,
    /// Specific model version (semver) the dataset conforms to.
    pub conforms_to_version: Option<String>,
    pub image_key: Option<String>,
    /// Wide banner/header image object-store key. Distinct from `image_key` (the cover/icon).
    pub banner_key: Option<String>,
    /// Logical classification: instances, model, vocabulary, shapes, entailment, system.
    /// None means unclassified.
    pub graph_role: Option<GraphKind>,
    pub created_at: String,
    pub updated_at: String,
    // DCAT / ADMS / VoID metadata
    pub license: Option<String>,
    pub themes: Option<String>,
    pub keywords: Option<String>,
    pub contact_name: Option<String>,
    pub contact_email: Option<String>,
    pub contact_url: Option<String>,
    pub adms_status: Option<String>,
    pub version_notes: Option<String>,
    pub spatial: Option<String>,
    pub landing_page: Option<String>,
}

/// A persisted SHACL validation run, including the full report.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ShaclValidationRun {
    pub id: String,
    pub dataset_id: String,
    pub run_timestamp: String,
    pub conforms: bool,
    pub results_count: i64,
    pub violation_count: i64,
    pub warning_count: i64,
    pub info_count: i64,
    /// The full SHACL validation report (deserialized from stored JSON).
    pub report: crate::shacl::report::ValidationReport,
    pub triggered_by: Option<String>,
    pub created_at: String,
}

/// Lightweight summary of a validation run, without the full report payload.
/// Used for dataset-list status and history listings.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ShaclRunSummary {
    pub id: String,
    pub dataset_id: String,
    pub run_timestamp: String,
    pub conforms: bool,
    pub results_count: i64,
    pub violation_count: i64,
    pub warning_count: i64,
    pub info_count: i64,
    pub triggered_by: Option<String>,
}

/// Aggregated dataset usage telemetry: how many times and how recently a
/// dataset was touched. `user_id` is `None` in a single-user (own-footprint)
/// view and populated in the super_admin cross-user view.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct DatasetUsageStat {
    pub dataset_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
    pub use_count: i64,
    pub last_used: String,
}

/// A SPARQL service within a dataset, exposing selected graphs.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SparqlService {
    pub id: String,
    pub dataset_id: String,
    pub name: String,
    pub slug: String,
    pub sparql_endpoint: String,
    pub description: Option<String>,
    pub is_active: bool,
    pub created_at: String,
}

// ─── Endpoint ACL ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct EndpointAclRule {
    pub id: String,
    /// "user" | "organisation" | "group" | "role"
    pub principal_type: String,
    /// user_id, org_id, group_id, or role name (e.g. "admin", "user")
    pub principal_id: String,
    /// Glob-style path pattern, e.g. "/api/datasets/*/sparql"
    pub path_pattern: String,
    /// Comma-separated HTTP methods or "*"
    pub http_methods: String,
    /// "allow" | "deny"
    pub effect: String,
    /// Higher = evaluated first
    pub priority: i64,
    pub created_at: String,
    pub created_by: String,
}

// ─── Graph ACL ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct GraphAclRule {
    pub id: String,
    pub graph_iri: String,
    /// "user" | "organisation" | "group" | "role" | "public"
    pub principal_type: String,
    /// principal identifier, or "*" for wildcards
    pub principal_id: String,
    /// "read" | "write" | "admin"
    pub permission: String,
    pub created_at: String,
    pub created_by: String,
}

// ─── Triple Security Labels ───────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct TripleSecurityLabel {
    pub id: String,
    pub subject_iri: String,
    pub predicate_iri: String,
    /// Serialised RDF term (IRI as "<iri>" or literal as "\"value\"@lang" etc.)
    pub object_value: String,
    pub graph_iri: String,
    /// Named graph IRI whose graph_acl entries govern visibility of this triple
    pub label_graph_iri: String,
    pub created_at: String,
}

// ─── OAuth / SSO Provider ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct OauthProvider {
    pub id: String,
    pub name: String,
    pub slug: String,
    /// "oidc" | "saml"
    pub provider_type: String,
    pub client_id: Option<String>,
    /// AES-256-GCM encrypted client secret; None when reading (redacted in API responses)
    #[serde(skip_serializing)]
    pub client_secret_enc: Option<String>,
    /// OIDC discovery URL
    pub discovery_url: Option<String>,
    /// Azure AD tenant ID (None = multi-tenant "common")
    pub tenant_id: Option<String>,
    /// SAML entity ID
    pub entity_id: Option<String>,
    /// SAML SSO redirect URL
    pub sso_url: Option<String>,
    /// SAML IdP signing certificate (PEM)
    #[serde(skip_serializing)]
    pub idp_certificate: Option<String>,
    pub scopes: String,
    /// JSON map of IdP group/claim value → grant, e.g.
    /// `{ "team-admins": "admin", "team-pub": "publisher", "staff": "user" }`.
    /// Role values (`super_admin`/`admin`/`user`) set the account role; the
    /// special grant `"publisher"` instead sets the `can_publish` capability.
    pub role_claim_map: Option<String>,
    pub auto_provision: bool,
    pub default_role: String,
    pub is_active: bool,
    pub created_at: String,
    pub updated_at: String,
}

/// Input struct for creating/updating an OAuth provider (client secret in plaintext here).
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct OauthProviderCreate {
    pub name: String,
    pub slug: String,
    pub provider_type: String,
    pub client_id: Option<String>,
    /// Plaintext secret — will be encrypted before storage
    pub client_secret: Option<String>,
    /// Pre-encrypted secret — used internally when updating without changing the secret
    #[serde(skip)]
    pub client_secret_enc: Option<String>,
    pub discovery_url: Option<String>,
    pub tenant_id: Option<String>,
    pub entity_id: Option<String>,
    pub sso_url: Option<String>,
    pub idp_certificate: Option<String>,
    pub scopes: Option<String>,
    pub role_claim_map: Option<String>,
    pub auto_provision: bool,
    pub default_role: Option<String>,
    pub is_active: bool,
}

impl OauthProviderCreate {
    pub fn scopes_or_default(&self) -> String {
        self.scopes
            .clone()
            .unwrap_or_else(|| "openid email profile".to_string())
    }
    pub fn default_role_or_user(&self) -> String {
        self.default_role
            .clone()
            .unwrap_or_else(|| "user".to_string())
    }
}

// ─── Validation reports (Unified Accounts plan, Phase 5) ──────────────────────

/// A stored SHACL validation report with provenance. `report_ttl` is the
/// `sh:ValidationReport` as Turtle (omitted from list responses for size).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationReportRecord {
    pub id: String,
    pub dataset_id: String,
    /// Dataset version this report is associated with (when committed).
    pub version: Option<String>,
    pub conforms: bool,
    #[serde(default)]
    pub report_ttl: String,
    /// Provenance: where the validated data/shapes came from.
    pub data_ref: Option<String>,
    pub shapes_ref: Option<String>,
    /// "platform" (validate-and-commit) | "on-write" (continuous).
    pub source: String,
    pub created_by: Option<String>,
    pub created_at: String,
}

/// An OTS-minted, scoped, expiring share link (Phase 6) — replaces ad-hoc
/// client-side form "login codes" with a server-verified, revocable token.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShareLink {
    pub id: String,
    /// SHA-256 of the opaque token; never serialized.
    #[serde(skip_serializing)]
    pub token_hash: String,
    pub dataset_id: String,
    pub graph: Option<String>,
    /// "read" | "submit".
    pub permission: String,
    pub label: Option<String>,
    pub created_by: Option<String>,
    pub expires_at: Option<String>,
    pub revoked: bool,
    pub created_at: String,
}

// ─── OAuth Identity ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct OauthIdentity {
    pub id: String,
    pub user_id: String,
    pub provider_id: String,
    /// OIDC `sub` claim or SAML NameID
    pub external_subject: String,
    pub external_email: Option<String>,
    pub last_login_at: Option<String>,
    pub created_at: String,
}

/// An asset (non-RDF file) stored in S3.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct Asset {
    pub id: String,
    pub dataset_id: String,
    pub filename: String,
    pub content_type: String,
    pub s3_key: String,
    pub size_bytes: i64,
    pub uploaded_by: String,
    pub created_at: String,
    pub updated_at: Option<String>,
    /// User-supplied display title (falls back to filename when absent).
    pub title: Option<String>,
    /// User-supplied description for this distribution.
    pub description: Option<String>,
    /// Whether this asset is publicly accessible without authentication (only effective when dataset is also public).
    pub public: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vals(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn map_claims_empty_or_absent_map_yields_nothing() {
        assert_eq!(
            map_claims_to_role(&vals(&["any"]), None),
            MappedClaims::default()
        );
        assert_eq!(
            map_claims_to_role(&vals(&["any"]), Some("{}")),
            MappedClaims::default()
        );
        // Map present, but no claim value matched.
        let m = map_claims_to_role(&vals(&["nope"]), Some(r#"{"admins":"admin"}"#));
        assert_eq!(m, MappedClaims::default());
    }

    #[test]
    fn map_claims_picks_strongest_role() {
        let map = r#"{"a":"user","b":"admin","c":"super_admin"}"#;
        let m = map_claims_to_role(&vals(&["a", "b"]), Some(map));
        assert_eq!(m.role, Some(SystemRole::Admin));
        assert!(!m.grant_publish);
        let m = map_claims_to_role(&vals(&["b", "c"]), Some(map));
        assert_eq!(m.role, Some(SystemRole::SuperAdmin));
    }

    #[test]
    fn map_claims_publisher_is_capability_not_role() {
        // "publisher" sets grant_publish without contributing a role.
        let m = map_claims_to_role(&vals(&["pub"]), Some(r#"{"pub":"publisher"}"#));
        assert_eq!(m.role, None, "publisher must not set a role");
        assert!(m.grant_publish);

        // Combined with a real role: role wins, publish still granted.
        let map = r#"{"pub":"publisher","admins":"admin"}"#;
        let m = map_claims_to_role(&vals(&["pub", "admins"]), Some(map));
        assert_eq!(m.role, Some(SystemRole::Admin));
        assert!(m.grant_publish);
    }

    #[test]
    fn access_level_is_ordered_and_capability_consistent() {
        assert!(AccessLevel::Read < AccessLevel::Write);
        assert!(AccessLevel::Write < AccessLevel::Manage);
        // max() yields the strongest grant.
        assert_eq!(
            AccessLevel::Read.max(AccessLevel::Manage),
            AccessLevel::Manage
        );

        assert!(AccessLevel::Read.can_read() && !AccessLevel::Read.can_write());
        assert!(AccessLevel::Write.can_write() && !AccessLevel::Write.can_manage());
        assert!(AccessLevel::Manage.can_manage());
    }

    #[test]
    fn access_level_parses_all_spellings() {
        for s in ["read", "viewer"] {
            assert_eq!(AccessLevel::from_str(s), Some(AccessLevel::Read));
        }
        for s in ["write", "editor"] {
            assert_eq!(AccessLevel::from_str(s), Some(AccessLevel::Write));
        }
        for s in ["manage", "admin", "owner"] {
            assert_eq!(AccessLevel::from_str(s), Some(AccessLevel::Manage));
        }
        assert_eq!(AccessLevel::from_str("nope"), None);
    }

    #[test]
    fn access_level_satisfying_spellings_are_monotonic() {
        // A stronger requirement is satisfied by a subset of the spellings that
        // satisfy a weaker one (manage ⊆ write ⊆ read).
        let read: std::collections::HashSet<_> =
            AccessLevel::Read.satisfying_spellings().iter().collect();
        let write: std::collections::HashSet<_> =
            AccessLevel::Write.satisfying_spellings().iter().collect();
        let manage: std::collections::HashSet<_> =
            AccessLevel::Manage.satisfying_spellings().iter().collect();
        assert!(manage.is_subset(&write));
        assert!(write.is_subset(&read));
        // The canonical stored spellings are all recognised.
        for s in ["read", "write", "admin"] {
            assert!(read.contains(&s));
        }
    }

    #[test]
    fn presentation_types_convert_into_access_level() {
        assert_eq!(AccessLevel::from(ResourceRole::Viewer), AccessLevel::Read);
        assert_eq!(AccessLevel::from(ResourceRole::Editor), AccessLevel::Write);
        assert_eq!(AccessLevel::from(ResourceRole::Admin), AccessLevel::Manage);
        assert_eq!(AccessLevel::from(ApiScope::Read), AccessLevel::Read);
        assert_eq!(AccessLevel::from(ApiScope::Write), AccessLevel::Write);
        assert_eq!(AccessLevel::from(ApiScope::Admin), AccessLevel::Manage);

        // ResourceRole capability checks delegate to AccessLevel.
        assert!(ResourceRole::Editor.can_write() && !ResourceRole::Editor.can_manage());
        assert!(ResourceRole::Admin.can_manage());
    }
}
