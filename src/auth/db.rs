use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tracing::info;

/// TTL for the accessible-graphs cache. Short enough that newly granted access
/// shows up quickly, long enough to absorb the typical burst of /browse calls.
const ACCESSIBLE_GRAPHS_TTL: Duration = Duration::from_secs(30);

type AccessibleGraphs = (HashSet<String>, HashSet<String>);

use super::models::*;

/// Helper to read a User from a row (columns per USER_COLS: id, username, email, password_hash, role, is_active, created_at, updated_at, is_public, avatar_key, can_publish, display_name, bio, website, phone, organization, email_verified, totp_enabled).
fn read_user(row: &rusqlite::Row) -> rusqlite::Result<User> {
    // Tolerate NULLs in any column so one malformed row never fails the whole query.
    let role_str: String = row.get::<_, Option<String>>(4)?.unwrap_or_default();
    Ok(User {
        id: row.get::<_, Option<String>>(0)?.unwrap_or_default(),
        username: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
        email: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
        password_hash: row.get::<_, Option<String>>(3)?.unwrap_or_default(),
        role: SystemRole::from_str(&role_str).unwrap_or(SystemRole::User),
        is_active: row.get::<_, i32>(5).unwrap_or(1) != 0,
        // Timestamps may be NULL for users created via some flows (e.g. OAuth);
        // tolerate that rather than failing the whole row mapping.
        created_at: row.get::<_, Option<String>>(6)?.unwrap_or_default(),
        updated_at: row.get::<_, Option<String>>(7)?.unwrap_or_default(),
        is_public: row.get::<_, i32>(8).unwrap_or(0) != 0,
        avatar_key: row.get(9)?,
        can_publish: row.get::<_, i32>(10).unwrap_or(0) != 0,
        display_name: row.get(11)?,
        bio: row.get(12)?,
        website: row.get(13)?,
        phone: row.get(14)?,
        organization: row.get(15)?,
        email_verified: row.get::<_, i32>(16).unwrap_or(0) != 0,
        totp_enabled: row.get::<_, i32>(17).unwrap_or(0) != 0,
    })
}

/// Read a per-resource grant row. Column order matches the SELECTs in the
/// `resource_access` query methods.
fn read_resource_grant(row: &rusqlite::Row) -> rusqlite::Result<ResourceGrant> {
    Ok(ResourceGrant {
        id: row.get(0)?,
        resource_type: row.get(1)?,
        resource_id: row.get(2)?,
        principal_type: row.get(3)?,
        principal_id: row.get(4)?,
        role: row.get(5)?,
        created_at: row.get(6)?,
        created_by: row.get(7)?,
    })
}

/// Return the stronger of two optional resource roles (`None` is weakest).
fn stronger(a: Option<ResourceRole>, b: Option<ResourceRole>) -> Option<ResourceRole> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.max(y)),
        (Some(x), None) | (None, Some(x)) => Some(x),
        (None, None) => None,
    }
}

/// Combine a membership-derived role with an explicit per-resource grant.
///
/// An explicit grant *replaces* the membership default, so it can both elevate
/// (e.g. give an editor admin on one dataset) and restrict (e.g. limit a member
/// to read-only). The one exception: an org/group **admin** keeps a manage
/// floor — a grant can never demote someone who administers the owning org or
/// group, so they can't be locked out of the resources they're responsible for.
fn combine_membership_and_grant(
    membership: Option<ResourceRole>,
    grant: Option<ResourceRole>,
) -> Option<ResourceRole> {
    let admin_floor = if membership == Some(ResourceRole::Admin) {
        Some(ResourceRole::Admin)
    } else {
        None
    };
    let base = match grant {
        Some(g) => Some(g),
        None => membership,
    };
    stronger(admin_floor, base)
}

/// Apply a resource's visibility gate to a raw org/group membership role.
///
/// * `public` / `members`: members get the full role implied by membership.
/// * `private`: only an admin (owner-level) membership keeps access; plain
///   members and viewers get nothing here and must be granted explicitly.
fn scope_membership_role(role: Option<Role>, visibility: Visibility) -> Option<ResourceRole> {
    let role = role?;
    match visibility {
        Visibility::Public | Visibility::Members => Some(ResourceRole::from_membership(role)),
        Visibility::Private => {
            if role == Role::Admin {
                Some(ResourceRole::Admin)
            } else {
                None
            }
        }
    }
}

const USER_COLS: &str = "id, username, email, password_hash, role, is_active, created_at, updated_at, COALESCE(is_public,0), avatar_key, COALESCE(can_publish,0), display_name, bio, website, phone, organization, COALESCE(email_verified,0), COALESCE(totp_enabled,0)";
/// Same columns but table-qualified with `u.` alias, for use in JOIN queries.
const USER_COLS_U: &str = "u.id, u.username, u.email, u.password_hash, u.role, u.is_active, u.created_at, u.updated_at, COALESCE(u.is_public,0), u.avatar_key, COALESCE(u.can_publish,0), u.display_name, u.bio, u.website, u.phone, u.organization, COALESCE(u.email_verified,0), COALESCE(u.totp_enabled,0)";
/// Number of columns in USER_COLS/USER_COLS_U — the first index AFTER the user
/// columns when a query appends extra columns (e.g. a membership role).
const USER_COLS_LEN: usize = 18;

/// Helper to read a Dataset from a row (24 columns, 0-indexed).
/// Column order: id(0), name(1), description(2), owner_type(3), owner_id(4), visibility(5),
///   shacl_on_write(6), shapes_graph_iri(7), conforms_to_model(8), conforms_to_version(9),
///   image_key(10), graph_role(11), created_at(12), updated_at(13),
///   license(14), themes(15), keywords(16), contact_name(17), contact_email(18),
///   contact_url(19), adms_status(20), version_notes(21), spatial(22), landing_page(23),
///   banner_key(24).
fn read_dataset_row(row: &rusqlite::Row) -> rusqlite::Result<Dataset> {
    let owner_type_str: String = row.get(3)?;
    let vis_str: String = row.get(5)?;
    let role_str: Option<String> = row.get(11)?;
    Ok(Dataset {
        id: row.get(0)?,
        name: row.get(1)?,
        description: row.get(2)?,
        owner_type: OwnerType::from_str(&owner_type_str).unwrap_or(OwnerType::User),
        owner_id: row.get(4)?,
        visibility: Visibility::from_str(&vis_str).unwrap_or(Visibility::Private),
        shacl_on_write: row.get::<_, i32>(6)? != 0,
        shapes_graph_iri: row.get(7)?,
        conforms_to_model: row.get(8)?,
        conforms_to_version: row.get(9)?,
        image_key: row.get(10)?,
        banner_key: row.get(24)?,
        graph_role: role_str.as_deref().and_then(GraphKind::from_str),
        created_at: row.get(12)?,
        updated_at: row.get(13)?,
        license: row.get(14)?,
        themes: row.get(15)?,
        keywords: row.get(16)?,
        contact_name: row.get(17)?,
        contact_email: row.get(18)?,
        contact_url: row.get(19)?,
        adms_status: row.get(20)?,
        version_notes: row.get(21)?,
        spatial: row.get(22)?,
        landing_page: row.get(23)?,
    })
}

const RUN_SUMMARY_COLS: &str = "id, dataset_id, run_timestamp, conforms, results_count, violation_count, warning_count, info_count, triggered_by";

/// Helper to read a ShaclRunSummary from a row (columns per RUN_SUMMARY_COLS).
fn read_run_summary(row: &rusqlite::Row) -> rusqlite::Result<ShaclRunSummary> {
    Ok(ShaclRunSummary {
        id: row.get(0)?,
        dataset_id: row.get(1)?,
        run_timestamp: row.get(2)?,
        conforms: row.get::<_, i32>(3)? != 0,
        results_count: row.get(4)?,
        violation_count: row.get(5)?,
        warning_count: row.get(6)?,
        info_count: row.get(7)?,
        triggered_by: row.get(8)?,
    })
}

/// Raw row tuple for a full validation run (RUN_SUMMARY_COLS + report_json + created_at).
type RunRow = (
    String,
    String,
    String,
    bool,
    i64,
    i64,
    i64,
    i64,
    String,
    Option<String>,
    String,
);

fn map_run_row(row: &rusqlite::Row) -> rusqlite::Result<RunRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get::<_, i32>(3)? != 0,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
        row.get(8)?,
        row.get(9)?,
        row.get(10)?,
    ))
}

fn parse_run_row(r: RunRow) -> anyhow::Result<ShaclValidationRun> {
    let (
        id,
        dataset_id,
        run_timestamp,
        conforms,
        results_count,
        violation_count,
        warning_count,
        info_count,
        report_json,
        triggered_by,
        created_at,
    ) = r;
    let report: crate::shacl::report::ValidationReport = serde_json::from_str(&report_json)?;
    Ok(ShaclValidationRun {
        id,
        dataset_id,
        run_timestamp,
        conforms,
        results_count,
        violation_count,
        warning_count,
        info_count,
        report,
        triggered_by,
        created_at,
    })
}

/// Helper to read an Organisation from a row (0-indexed).
/// Column order: id(0), name(1), slug(2), description(3), created_at(4), image_key(5),
///   homepage(6), identifier(7), contact_name(8), contact_email(9), contact_url(10), org_type(11),
///   parent_org_id(12), banner_key(13).
fn map_oidc_client_row(row: &rusqlite::Row) -> rusqlite::Result<OidcClient> {
    let redirect_uris_json: String = row.get(2)?;
    let secret_enc: Option<String> = row.get(4)?;
    Ok(OidcClient {
        client_id: row.get(0)?,
        name: row.get(1)?,
        redirect_uris: serde_json::from_str(&redirect_uris_json).unwrap_or_default(),
        public: row.get::<_, i64>(3)? != 0,
        has_secret: secret_enc.is_some(),
        secret_enc,
        created_at: row.get(5)?,
    })
}

fn map_org_row(row: &rusqlite::Row) -> rusqlite::Result<Organisation> {
    Ok(Organisation {
        id: row.get(0)?,
        name: row.get(1)?,
        slug: row.get(2)?,
        description: row.get(3)?,
        created_at: row.get(4)?,
        image_key: row.get(5)?,
        banner_key: row.get(13)?,
        homepage: row.get(6)?,
        identifier: row.get(7)?,
        contact_name: row.get(8)?,
        contact_email: row.get(9)?,
        contact_url: row.get(10)?,
        org_type: row.get(11)?,
        parent_org_id: row.get(12)?,
    })
}

/// SQLite-backed authentication and identity database.
pub struct AuthDb {
    pool: Pool<SqliteConnectionManager>,
    /// Short-lived cache for `get_accessible_graph_iris`, keyed by user_id
    /// (`None` = anonymous). /browse hits this path many times per second; the
    /// uncached path does two SELECTs + a HashSet join each call.
    #[allow(clippy::type_complexity)] // a cache tuple; a type alias would obscure it
    accessible_graphs_cache: Mutex<HashMap<Option<String>, (Instant, Arc<AccessibleGraphs>)>>,
}

impl AuthDb {
    /// Open or create the SQLite database at the given path and run migrations.
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        let manager = SqliteConnectionManager::file(path).with_init(|c| {
            // `busy_timeout` MUST be set first: it makes a connection wait for the
            // write lock instead of failing instantly with SQLITE_BUSY ("database is
            // locked"). r2d2 opens the whole pool eagerly, so the 8 connections race
            // to run `journal_mode=WAL` at boot — without the timeout already in
            // effect, the losers of that race error out. Ordering it ahead of WAL
            // also covers the boot seeders/audit writes that contend on the DB.
            c.execute_batch(
                "PRAGMA busy_timeout=5000; PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;",
            )
        });
        let pool = r2d2::Pool::builder()
            .max_size(8)
            .build(manager)
            .map_err(|e| anyhow::anyhow!("Pool build failed: {}", e))?;
        let db = Self {
            pool,
            accessible_graphs_cache: Mutex::new(HashMap::new()),
        };
        db.migrate()?;
        info!("Auth database ready at {}", path.display());
        Ok(db)
    }

    /// Create an in-memory database (for testing).
    /// Uses max_size=1 so that all requests share the same in-memory connection.
    pub fn in_memory() -> anyhow::Result<Self> {
        let manager = SqliteConnectionManager::memory()
            .with_init(|c| c.execute_batch("PRAGMA foreign_keys=ON;"));
        let pool = r2d2::Pool::builder()
            .max_size(1)
            .build(manager)
            .map_err(|e| anyhow::anyhow!("Pool build failed: {}", e))?;
        let db = Self {
            pool,
            accessible_graphs_cache: Mutex::new(HashMap::new()),
        };
        db.migrate()?;
        Ok(db)
    }

    /// Shared pool accessor — used by the audit logger so it can reuse the
    /// same SQLite connection pool without owning a separate copy.
    pub fn pool(&self) -> Pool<SqliteConnectionManager> {
        self.pool.clone()
    }

    fn migrate(&self) -> anyhow::Result<()> {
        let conn = self.pool.get()?;

        // Create all tables with the correct schema.
        // CREATE TABLE IF NOT EXISTS is idempotent — safe to run on every startup.
        conn.execute_batch("
            CREATE TABLE IF NOT EXISTS users (
                id TEXT PRIMARY KEY,
                username TEXT NOT NULL UNIQUE,
                email TEXT NOT NULL UNIQUE,
                email_verified INTEGER NOT NULL DEFAULT 0,
                password_hash TEXT NOT NULL,
                is_admin INTEGER NOT NULL DEFAULT 0,
                role TEXT NOT NULL DEFAULT 'user',
                is_active INTEGER NOT NULL DEFAULT 1,
                is_public INTEGER NOT NULL DEFAULT 0,
                avatar_key TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                -- TOTP two-factor login: AES-GCM-encrypted shared secret, the
                -- enabled flag, and the last successfully-used time step
                -- (replay guard — a TOTP code must never authenticate twice).
                totp_secret_enc TEXT,
                totp_enabled INTEGER NOT NULL DEFAULT 0,
                totp_last_step INTEGER NOT NULL DEFAULT 0
            );

            -- Single-use, expiring tokens mailed to users to prove mailbox
            -- control: email verification, password reset, email change.
            -- Only the SHA-256 hash of a token is stored.
            CREATE TABLE IF NOT EXISTS email_tokens (
                id TEXT PRIMARY KEY,
                user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
                kind TEXT NOT NULL CHECK(kind IN ('verify_email','reset_password','change_email')),
                token_hash TEXT NOT NULL UNIQUE,
                new_email TEXT,
                expires_at TEXT NOT NULL,
                created_at TEXT NOT NULL,
                used_at TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_email_tokens_user ON email_tokens(user_id, kind);

            -- Single-use 2FA recovery codes (hashed); replace-on-regenerate.
            CREATE TABLE IF NOT EXISTS totp_recovery_codes (
                id TEXT PRIMARY KEY,
                user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
                code_hash TEXT NOT NULL,
                created_at TEXT NOT NULL,
                used_at TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_totp_recovery_user ON totp_recovery_codes(user_id);

            -- WebAuthn/FIDO2 passkeys. public_key holds the full serialized
            -- webauthn-rs Passkey (COSE public key + verification policy);
            -- counter and transports are denormalised copies for listing and
            -- clone detection without parsing the JSON.
            CREATE TABLE IF NOT EXISTS webauthn_credentials (
                id TEXT PRIMARY KEY,
                user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
                credential_id TEXT NOT NULL UNIQUE,
                public_key TEXT NOT NULL,
                counter INTEGER NOT NULL DEFAULT 0,
                transports TEXT,
                name TEXT NOT NULL,
                created_at TEXT NOT NULL,
                last_used_at TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_webauthn_credentials_user ON webauthn_credentials(user_id);

            -- Per-account login throttle (independent of the per-IP rate limit) to
            -- stop distributed credential-stuffing against a single username.
            -- Keyed by the SUBMITTED username so guesses at non-existent accounts
            -- are throttled too (and don't become an enumeration oracle).
            CREATE TABLE IF NOT EXISTS login_attempts (
                username TEXT PRIMARY KEY,
                failed_count INTEGER NOT NULL DEFAULT 0,
                first_failed_at TEXT,
                locked_until TEXT
            );

            CREATE TABLE IF NOT EXISTS organisations (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                slug TEXT NOT NULL UNIQUE,
                description TEXT,
                image_key TEXT,
                banner_key TEXT,
                created_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS groups (
                id TEXT PRIMARY KEY,
                org_id TEXT NOT NULL REFERENCES organisations(id) ON DELETE CASCADE,
                name TEXT NOT NULL,
                parent_group_id TEXT REFERENCES groups(id) ON DELETE SET NULL,
                created_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_groups_org_id ON groups(org_id);
            CREATE INDEX IF NOT EXISTS idx_groups_parent ON groups(parent_group_id);

            CREATE TABLE IF NOT EXISTS org_memberships (
                user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
                org_id TEXT NOT NULL REFERENCES organisations(id) ON DELETE CASCADE,
                role TEXT NOT NULL DEFAULT 'member',
                PRIMARY KEY (user_id, org_id)
            );
            CREATE INDEX IF NOT EXISTS idx_org_memberships_org ON org_memberships(org_id);

            CREATE TABLE IF NOT EXISTS group_memberships (
                user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
                group_id TEXT NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
                role TEXT NOT NULL DEFAULT 'member',
                PRIMARY KEY (user_id, group_id)
            );
            CREATE INDEX IF NOT EXISTS idx_group_memberships_group ON group_memberships(group_id);

            CREATE TABLE IF NOT EXISTS validation_reports (
                id TEXT PRIMARY KEY,
                dataset_id TEXT NOT NULL REFERENCES datasets(id) ON DELETE CASCADE,
                version TEXT,
                conforms INTEGER NOT NULL,
                report_ttl TEXT NOT NULL,
                data_ref TEXT,
                shapes_ref TEXT,
                source TEXT NOT NULL DEFAULT 'platform',
                created_by TEXT,
                created_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_validation_reports_dataset ON validation_reports(dataset_id);

            CREATE TABLE IF NOT EXISTS share_links (
                id TEXT PRIMARY KEY,
                token_hash TEXT NOT NULL UNIQUE,
                dataset_id TEXT NOT NULL REFERENCES datasets(id) ON DELETE CASCADE,
                graph TEXT,
                permission TEXT NOT NULL DEFAULT 'read',
                label TEXT,
                created_by TEXT,
                expires_at TEXT,
                revoked INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_share_links_dataset ON share_links(dataset_id);

            CREATE TABLE IF NOT EXISTS datasets (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                description TEXT,
                owner_type TEXT NOT NULL CHECK(owner_type IN ('user','organisation','group')),
                owner_id TEXT NOT NULL,
                visibility TEXT NOT NULL DEFAULT 'private' CHECK(visibility IN ('public','members','private')),
                shacl_on_write INTEGER NOT NULL DEFAULT 0,
                shapes_graph_iri TEXT,
                conforms_to_model TEXT,
                conforms_to_version TEXT,
                image_key TEXT,
                banner_key TEXT,
                graph_role TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_datasets_owner ON datasets(owner_type, owner_id);

            CREATE TABLE IF NOT EXISTS dataset_private_access (
                dataset_id TEXT NOT NULL REFERENCES datasets(id) ON DELETE CASCADE,
                user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
                PRIMARY KEY (dataset_id, user_id)
            );

            -- ── Per-resource role grants ─────────────────────────────────────
            -- Grants an explicit role (viewer | editor | admin) on a single
            -- resource (dataset | model | vocabulary) to a principal (a user or
            -- a group). These grants are combined with — and override upward —
            -- the role a user would otherwise derive from org/group membership.
            -- resource_id / principal_id are intentionally un-foreign-keyed
            -- because they span multiple owning tables (and the RDF-backed
            -- model/vocabulary registries, which have no SQLite rows).
            CREATE TABLE IF NOT EXISTS resource_access (
                id TEXT PRIMARY KEY,
                resource_type TEXT NOT NULL CHECK(resource_type IN ('dataset','model','vocabulary')),
                resource_id TEXT NOT NULL,
                principal_type TEXT NOT NULL CHECK(principal_type IN ('user','group','organisation')),
                principal_id TEXT NOT NULL,
                role TEXT NOT NULL CHECK(role IN ('viewer','editor','admin')),
                created_at TEXT NOT NULL,
                created_by TEXT NOT NULL,
                UNIQUE(resource_type, resource_id, principal_type, principal_id)
            );
            CREATE INDEX IF NOT EXISTS idx_resource_access_resource ON resource_access(resource_type, resource_id);
            CREATE INDEX IF NOT EXISTS idx_resource_access_principal ON resource_access(principal_type, principal_id);

            CREATE TABLE IF NOT EXISTS dataset_graphs (
                dataset_id TEXT NOT NULL REFERENCES datasets(id) ON DELETE CASCADE,
                graph_iri TEXT NOT NULL,
                graph_role TEXT,
                private INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (dataset_id, graph_iri)
            );

            CREATE TABLE IF NOT EXISTS sparql_services (
                id TEXT PRIMARY KEY,
                dataset_id TEXT NOT NULL REFERENCES datasets(id) ON DELETE CASCADE,
                name TEXT NOT NULL,
                slug TEXT NOT NULL,
                description TEXT,
                is_active INTEGER NOT NULL DEFAULT 1,
                created_at TEXT NOT NULL,
                UNIQUE(dataset_id, slug)
            );
            CREATE INDEX IF NOT EXISTS idx_sparql_services_dataset ON sparql_services(dataset_id);

            CREATE TABLE IF NOT EXISTS service_graphs (
                service_id TEXT NOT NULL REFERENCES sparql_services(id) ON DELETE CASCADE,
                graph_iri TEXT NOT NULL,
                PRIMARY KEY (service_id, graph_iri)
            );

            CREATE TABLE IF NOT EXISTS assets (
                id TEXT PRIMARY KEY,
                dataset_id TEXT NOT NULL REFERENCES datasets(id) ON DELETE CASCADE,
                filename TEXT NOT NULL,
                content_type TEXT NOT NULL,
                s3_key TEXT NOT NULL UNIQUE,
                size_bytes INTEGER NOT NULL,
                uploaded_by TEXT NOT NULL REFERENCES users(id),
                created_at TEXT NOT NULL,
                public INTEGER NOT NULL DEFAULT 0
            );
            CREATE INDEX IF NOT EXISTS idx_assets_dataset ON assets(dataset_id);

            CREATE TABLE IF NOT EXISTS refresh_tokens (
                id TEXT PRIMARY KEY,
                user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
                token_hash TEXT NOT NULL UNIQUE,
                expires_at TEXT NOT NULL,
                created_at TEXT NOT NULL,
                revoked INTEGER NOT NULL DEFAULT 0
            );
            CREATE INDEX IF NOT EXISTS idx_refresh_tokens_user ON refresh_tokens(user_id);
            CREATE INDEX IF NOT EXISTS idx_refresh_tokens_hash ON refresh_tokens(token_hash);

            CREATE TABLE IF NOT EXISTS api_tokens (
                id TEXT PRIMARY KEY,
                user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
                name TEXT NOT NULL,
                token_hash TEXT NOT NULL UNIQUE,
                token_prefix TEXT NOT NULL,
                scopes TEXT NOT NULL DEFAULT 'read',
                expires_at TEXT,
                last_used_at TEXT,
                created_at TEXT NOT NULL,
                revoked INTEGER NOT NULL DEFAULT 0
            );
            CREATE INDEX IF NOT EXISTS idx_api_tokens_user ON api_tokens(user_id);
            CREATE INDEX IF NOT EXISTS idx_api_tokens_hash ON api_tokens(token_hash);

            -- ── Endpoint ACL ────────────────────────────────────────────────
            CREATE TABLE IF NOT EXISTS endpoint_acl (
                id TEXT PRIMARY KEY,
                principal_type TEXT NOT NULL CHECK(principal_type IN ('user','organisation','group','role')),
                principal_id TEXT NOT NULL,
                path_pattern TEXT NOT NULL,
                http_methods TEXT NOT NULL DEFAULT '*',
                effect TEXT NOT NULL CHECK(effect IN ('allow','deny')),
                priority INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL,
                created_by TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_endpoint_acl_principal ON endpoint_acl(principal_type, principal_id);

            -- ── Named-graph ACL ──────────────────────────────────────────────
            CREATE TABLE IF NOT EXISTS graph_acl (
                id TEXT PRIMARY KEY,
                graph_iri TEXT NOT NULL,
                principal_type TEXT NOT NULL CHECK(principal_type IN ('user','organisation','group','role','public')),
                principal_id TEXT NOT NULL,
                permission TEXT NOT NULL CHECK(permission IN ('read','write','admin')),
                created_at TEXT NOT NULL,
                created_by TEXT NOT NULL,
                UNIQUE(graph_iri, principal_type, principal_id, permission)
            );
            CREATE INDEX IF NOT EXISTS idx_graph_acl_graph ON graph_acl(graph_iri);

            -- ── Triple-level security labels ─────────────────────────────────
            CREATE TABLE IF NOT EXISTS triple_security_labels (
                id TEXT PRIMARY KEY,
                subject_iri TEXT NOT NULL,
                predicate_iri TEXT NOT NULL,
                object_value TEXT NOT NULL,
                graph_iri TEXT NOT NULL,
                label_graph_iri TEXT NOT NULL,
                created_at TEXT NOT NULL,
                UNIQUE(subject_iri, predicate_iri, object_value, graph_iri)
            );
            CREATE INDEX IF NOT EXISTS idx_triple_labels_graph ON triple_security_labels(graph_iri);
            CREATE INDEX IF NOT EXISTS idx_triple_labels_label ON triple_security_labels(label_graph_iri);

            -- ── OAuth/SSO providers ──────────────────────────────────────────
            CREATE TABLE IF NOT EXISTS oauth_providers (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                slug TEXT NOT NULL UNIQUE,
                provider_type TEXT NOT NULL CHECK(provider_type IN ('oidc','saml')),
                client_id TEXT,
                client_secret_enc TEXT,
                discovery_url TEXT,
                tenant_id TEXT,
                entity_id TEXT,
                sso_url TEXT,
                idp_certificate TEXT,
                scopes TEXT NOT NULL DEFAULT 'openid email profile',
                role_claim_map TEXT,
                auto_provision INTEGER NOT NULL DEFAULT 1,
                default_role TEXT NOT NULL DEFAULT 'user',
                is_active INTEGER NOT NULL DEFAULT 1,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            -- ── External identity ↔ local user mapping ────────────────────────
            CREATE TABLE IF NOT EXISTS oauth_identities (
                id TEXT PRIMARY KEY,
                user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
                provider_id TEXT NOT NULL REFERENCES oauth_providers(id) ON DELETE CASCADE,
                external_subject TEXT NOT NULL,
                external_email TEXT,
                last_login_at TEXT,
                created_at TEXT NOT NULL,
                UNIQUE(provider_id, external_subject)
            );
            CREATE INDEX IF NOT EXISTS idx_oauth_identities_user ON oauth_identities(user_id);

            -- ── Audit log (append-only) ──────────────────────────────────────
            CREATE TABLE IF NOT EXISTS audit_events (
                id TEXT PRIMARY KEY,
                timestamp TEXT NOT NULL,
                actor_id TEXT,
                actor_username TEXT,
                actor_role TEXT,
                event_type TEXT NOT NULL,
                resource_type TEXT,
                resource_id TEXT,
                action TEXT,
                outcome TEXT NOT NULL CHECK(outcome IN ('success','failure','denied')),
                ip_address TEXT,
                user_agent TEXT,
                details TEXT,
                request_id TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_audit_timestamp ON audit_events(timestamp);
            CREATE INDEX IF NOT EXISTS idx_audit_actor ON audit_events(actor_id);
            CREATE INDEX IF NOT EXISTS idx_audit_event_type ON audit_events(event_type);

            -- Enforce immutability at the database level. The application code
            -- only ever INSERTs into audit_events; these triggers turn any
            -- accidental UPDATE/DELETE (including via raw SQL) into an error.
            CREATE TRIGGER IF NOT EXISTS audit_events_no_update
                BEFORE UPDATE ON audit_events
                BEGIN SELECT RAISE(ABORT, 'audit_events is append-only'); END;
            CREATE TRIGGER IF NOT EXISTS audit_events_no_delete
                BEFORE DELETE ON audit_events
                BEGIN SELECT RAISE(ABORT, 'audit_events is append-only'); END;

            -- ── SHACL validation run history ─────────────────────────────────
            CREATE TABLE IF NOT EXISTS shacl_validation_runs (
                id TEXT PRIMARY KEY,
                dataset_id TEXT NOT NULL REFERENCES datasets(id) ON DELETE CASCADE,
                run_timestamp TEXT NOT NULL,
                conforms INTEGER NOT NULL DEFAULT 0,
                results_count INTEGER NOT NULL DEFAULT 0,
                violation_count INTEGER NOT NULL DEFAULT 0,
                warning_count INTEGER NOT NULL DEFAULT 0,
                info_count INTEGER NOT NULL DEFAULT 0,
                report_json TEXT NOT NULL,
                triggered_by TEXT,
                created_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_shacl_runs_dataset ON shacl_validation_runs(dataset_id);
            CREATE INDEX IF NOT EXISTS idx_shacl_runs_ts ON shacl_validation_runs(dataset_id, run_timestamp DESC);

            -- ── Saved / versioned SPARQL queries ─────────────────────────────
            -- A reusable SPARQL query owned by a dataset, organisation or group.
            -- `current_revision` points at the head row in saved_query_revisions
            -- (the query's own edit history). `parameters` is a JSON array of the
            -- typed variables the query exposes when run as an API; `test_parameters`
            -- is a JSON object of example bindings used for automatic version tests.
            CREATE TABLE IF NOT EXISTS saved_queries (
                id TEXT PRIMARY KEY,
                owner_type TEXT NOT NULL CHECK(owner_type IN ('dataset','organisation','group')),
                owner_id TEXT NOT NULL,
                name TEXT NOT NULL,
                slug TEXT NOT NULL,
                description TEXT,
                current_revision INTEGER NOT NULL DEFAULT 1,
                parameters TEXT NOT NULL DEFAULT '[]',
                test_parameters TEXT,
                visibility TEXT,
                is_active INTEGER NOT NULL DEFAULT 1,
                created_by TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                UNIQUE(owner_type, owner_id, slug)
            );
            CREATE INDEX IF NOT EXISTS idx_saved_queries_owner ON saved_queries(owner_type, owner_id);

            -- Append-only edit history of a saved query's SPARQL text. Each
            -- revision can carry a commit-style custom `name` and longer `note`.
            CREATE TABLE IF NOT EXISTS saved_query_revisions (
                query_id TEXT NOT NULL REFERENCES saved_queries(id) ON DELETE CASCADE,
                revision INTEGER NOT NULL,
                name TEXT,
                sparql TEXT NOT NULL,
                note TEXT,
                origin TEXT NOT NULL DEFAULT 'manual' CHECK(origin IN ('manual','llm_repair','import')),
                created_by TEXT NOT NULL,
                created_at TEXT NOT NULL,
                PRIMARY KEY (query_id, revision)
            );

            -- Result of running a saved-query revision against one dataset version.
            -- Never deleted — this is the query's reported test history. `status`
            -- is ok | changed (results differ from prev_version) | error (broken).
            CREATE TABLE IF NOT EXISTS saved_query_tests (
                id TEXT PRIMARY KEY,
                query_id TEXT NOT NULL REFERENCES saved_queries(id) ON DELETE CASCADE,
                revision INTEGER NOT NULL,
                dataset_id TEXT NOT NULL,
                dataset_version TEXT NOT NULL,
                prev_version TEXT,
                status TEXT NOT NULL CHECK(status IN ('ok','changed','error')),
                result_hash TEXT,
                result_rowcount INTEGER,
                error_message TEXT,
                acknowledged INTEGER NOT NULL DEFAULT 0,
                acknowledged_by TEXT,
                acknowledged_at TEXT,
                created_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_sqt_query ON saved_query_tests(query_id, created_at DESC);
            CREATE INDEX IF NOT EXISTS idx_sqt_query_version ON saved_query_tests(query_id, dataset_version);

            -- ── SHACL Studio ───────────────────────────────────────────────
            -- Reusable shape graphs (the Library), decoupled from datasets.
            -- List-valued fields (tags, target_classes) are JSON TEXT, the
            -- same convention saved_queries.parameters uses.
            CREATE TABLE IF NOT EXISTS shape_sets (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                description TEXT,
                owner_type TEXT NOT NULL CHECK(owner_type IN ('user','organisation','group')),
                owner_id TEXT NOT NULL,
                visibility TEXT NOT NULL DEFAULT 'private' CHECK(visibility IN ('public','members','private')),
                graph_iri TEXT NOT NULL,
                tags TEXT NOT NULL DEFAULT '[]',
                target_classes TEXT NOT NULL DEFAULT '[]',
                shape_count INTEGER NOT NULL DEFAULT 0,
                source TEXT NOT NULL DEFAULT 'manual',
                version INTEGER NOT NULL DEFAULT 1,
                status TEXT NOT NULL DEFAULT 'draft',
                created_by TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_shape_sets_owner ON shape_sets(owner_type, owner_id);
            CREATE INDEX IF NOT EXISTS idx_shape_sets_graph ON shape_sets(graph_iri);

            -- Append-only Turtle snapshots for rollback.
            CREATE TABLE IF NOT EXISTS shape_set_revisions (
                shape_set_id TEXT NOT NULL REFERENCES shape_sets(id) ON DELETE CASCADE,
                revision INTEGER NOT NULL,
                turtle TEXT NOT NULL,
                note TEXT,
                created_by TEXT,
                created_at TEXT NOT NULL,
                PRIMARY KEY (shape_set_id, revision)
            );

            -- Saved validation pipelines. Composed shape_set_ids + scope are
            -- JSON arrays. `gate_writes=1` makes the on-write hook reject
            -- non-conforming writes (HTTP 422 + report) for matching graphs.
            CREATE TABLE IF NOT EXISTS validation_pipelines (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                description TEXT,
                owner_type TEXT NOT NULL CHECK(owner_type IN ('user','organisation','group')),
                owner_id TEXT NOT NULL,
                visibility TEXT NOT NULL DEFAULT 'private' CHECK(visibility IN ('public','members','private')),
                dataset_ids TEXT NOT NULL DEFAULT '[]',
                graph_iris TEXT NOT NULL DEFAULT '[]',
                target_classes TEXT NOT NULL DEFAULT '[]',
                shape_set_ids TEXT NOT NULL DEFAULT '[]',
                targets TEXT NOT NULL DEFAULT '[]',
                severity_threshold TEXT NOT NULL DEFAULT 'violation',
                run_inference INTEGER NOT NULL DEFAULT 0,
                max_results INTEGER,
                trigger_on_write INTEGER NOT NULL DEFAULT 0,
                schedule_cron TEXT,
                gate_writes INTEGER NOT NULL DEFAULT 0,
                retention INTEGER NOT NULL DEFAULT 50,
                last_run_at TEXT,
                last_conforms INTEGER,
                created_by TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                -- Where SHACL-AF/function-derived triples are persisted:
                -- 'in_place' (default) | 'new_graph' | 'new_version'.
                inferred_target_kind TEXT NOT NULL DEFAULT 'in_place',
                inferred_target_graph TEXT,
                -- Where validation results (as sh:ValidationReport RDF) are persisted:
                -- 'none' (default) | 'in_place' | 'new_graph' | 'new_version'.
                results_target_kind TEXT NOT NULL DEFAULT 'none',
                results_target_graph TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_pipelines_owner ON validation_pipelines(owner_type, owner_id);
            CREATE INDEX IF NOT EXISTS idx_pipelines_gate ON validation_pipelines(gate_writes);
            CREATE INDEX IF NOT EXISTS idx_pipelines_schedule ON validation_pipelines(schedule_cron);

            -- Pipeline runs (full report as JSON; retention enforced per-pipeline at insert time).
            CREATE TABLE IF NOT EXISTS pipeline_runs (
                id TEXT PRIMARY KEY,
                pipeline_id TEXT NOT NULL REFERENCES validation_pipelines(id) ON DELETE CASCADE,
                triggered_by TEXT NOT NULL DEFAULT 'manual',
                actor TEXT,
                ran_at TEXT NOT NULL,
                conforms INTEGER NOT NULL DEFAULT 0,
                results_count INTEGER NOT NULL DEFAULT 0,
                violation_count INTEGER NOT NULL DEFAULT 0,
                warning_count INTEGER NOT NULL DEFAULT 0,
                info_count INTEGER NOT NULL DEFAULT 0,
                duration_ms INTEGER NOT NULL DEFAULT 0,
                report_json TEXT NOT NULL DEFAULT '',
                created_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_pipeline_runs_pipeline ON pipeline_runs(pipeline_id, ran_at DESC);

            -- ── Private dataset usage telemetry ──────────────────────────────
            -- Append-only record of who touched which dataset, when, and how
            -- ('view' | 'validate' | 'pipeline'). Used to rank a user's own
            -- 'recently used / use a lot' datasets in the validate overview.
            -- This is sensitive activity data: a user may read back only their
            -- OWN footprint; the cross-user aggregate is super_admin only.
            CREATE TABLE IF NOT EXISTS dataset_usage_events (
                id TEXT PRIMARY KEY,
                dataset_id TEXT NOT NULL,
                user_id TEXT,
                action TEXT NOT NULL,
                used_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_dataset_usage_user ON dataset_usage_events(user_id, used_at DESC);
            CREATE INDEX IF NOT EXISTS idx_dataset_usage_dataset ON dataset_usage_events(dataset_id, used_at DESC);

            -- In-app documentation pages. Built-in docs are seeded with
            -- source='builtin' and re-seeded on boot unless a user has edited
            -- them (source flips to 'user'). admin_only=1 docs are filtered out
            -- server-side for non-admins (returned as 404, never listed).
            CREATE TABLE IF NOT EXISTS docs (
                slug TEXT PRIMARY KEY,
                title TEXT NOT NULL,
                category TEXT,
                body_md TEXT NOT NULL,
                admin_only INTEGER NOT NULL DEFAULT 0,
                source TEXT NOT NULL DEFAULT 'user',
                sort_order INTEGER NOT NULL DEFAULT 100,
                updated_by TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_docs_category ON docs(category, sort_order);

            -- ── Spark chat history ────────────────────────────────────────────
            -- Per-user chat conversations with the Spark assistant. The client
            -- appends messages after each turn; the assistant's retrieval trail
            -- (queries JSON) rides along so a restored conversation renders its
            -- widgets and query disclosures exactly like the live turn did.
            CREATE TABLE IF NOT EXISTS chat_conversations (
                id TEXT PRIMARY KEY,
                user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
                title TEXT NOT NULL DEFAULT '',
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_chat_conversations_user ON chat_conversations(user_id, updated_at DESC);

            CREATE TABLE IF NOT EXISTS chat_messages (
                id TEXT PRIMARY KEY,
                conversation_id TEXT NOT NULL REFERENCES chat_conversations(id) ON DELETE CASCADE,
                seq INTEGER NOT NULL,
                role TEXT NOT NULL CHECK(role IN ('user','assistant')),
                content TEXT NOT NULL,
                queries TEXT,
                model TEXT,
                stopped INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_chat_messages_conv ON chat_messages(conversation_id, seq);

            -- Standing user preferences injected into the Spark system prompt
            -- (\"answer in Dutch\", \"I mostly work with the bridges dataset\", …).
            CREATE TABLE IF NOT EXISTS chat_user_memory (
                user_id TEXT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
                instructions TEXT NOT NULL DEFAULT '',
                enabled INTEGER NOT NULL DEFAULT 1,
                updated_at TEXT NOT NULL
            );

            -- ── LLM request telemetry (admin-visible) ─────────────────────────
            -- One row per LLM-backed request (chat turn, NL→SPARQL, SHACL assist):
            -- who, which endpoint/model, outcome (ok|error|blocked), latency,
            -- time-to-first-token for streamed turns, size metrics and the guard
            -- rule that fired, if any. Message *contents* are not stored — only
            -- an optional short preview of the question (LLM_LOG_PREVIEW_DISABLED
            -- turns that off too).
            CREATE TABLE IF NOT EXISTS llm_request_log (
                id TEXT PRIMARY KEY,
                timestamp TEXT NOT NULL,
                user_id TEXT,
                endpoint TEXT NOT NULL,
                model TEXT,
                status TEXT NOT NULL CHECK(status IN ('ok','error','blocked')),
                guard_flag TEXT,
                duration_ms INTEGER,
                ttft_ms INTEGER,
                prompt_chars INTEGER,
                answer_chars INTEGER,
                query_rounds INTEGER,
                question_preview TEXT,
                ip_address TEXT,
                error TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_llm_request_log_ts ON llm_request_log(timestamp DESC);
            CREATE INDEX IF NOT EXISTS idx_llm_request_log_user ON llm_request_log(user_id, timestamp DESC);

            -- Small runtime-changeable instance settings (admin toggles that must
            -- not require an env change + restart, e.g. guest self-registration).
            CREATE TABLE IF NOT EXISTS app_settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            -- ── OIDC provider (this store as the suite's identity provider) ──
            -- Registered relying-party clients (SPAs and services signing users
            -- in AGAINST this store; distinct from oauth_providers = upstream
            -- IdPs this store signs users in WITH).
            CREATE TABLE IF NOT EXISTS oauth_clients (
                client_id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                redirect_uris TEXT NOT NULL,          -- JSON array, exact-match allowlist
                public INTEGER NOT NULL DEFAULT 1,    -- 1 = PKCE-only SPA, 0 = confidential
                secret_enc TEXT,                      -- AES-GCM blob (confidential only)
                created_at TEXT NOT NULL
            );
            -- Single-use authorization codes (stored hashed; 10-minute lifetime).
            CREATE TABLE IF NOT EXISTS oauth_auth_codes (
                code_hash TEXT PRIMARY KEY,
                client_id TEXT NOT NULL,
                user_id TEXT NOT NULL,
                redirect_uri TEXT NOT NULL,
                scope TEXT NOT NULL,
                nonce TEXT,
                code_challenge TEXT NOT NULL,         -- PKCE S256 challenge
                expires_at TEXT NOT NULL,
                used INTEGER NOT NULL DEFAULT 0
            );
            -- Rotating refresh tokens for provider clients (hashed; single-use).
            CREATE TABLE IF NOT EXISTS oauth_client_refresh_tokens (
                token_hash TEXT PRIMARY KEY,
                client_id TEXT NOT NULL,
                user_id TEXT NOT NULL,
                scope TEXT NOT NULL,
                expires_at TEXT NOT NULL
            );
            -- Remembered per-user consent (client_id x scope), so the consent
            -- screen shows once per client unless the scope grows.
            CREATE TABLE IF NOT EXISTS oauth_consents (
                user_id TEXT NOT NULL,
                client_id TEXT NOT NULL,
                scope TEXT NOT NULL,
                granted_at TEXT NOT NULL,
                PRIMARY KEY (user_id, client_id)
            );
            -- The provider's signing keypair(s). The private key is AES-GCM
            -- encrypted with the HKDF-derived key (see auth::secret); the JWK
            -- column is the PUBLIC key as served by /oauth/jwks.
            CREATE TABLE IF NOT EXISTS oauth_signing_keys (
                kid TEXT PRIMARY KEY,
                alg TEXT NOT NULL,
                pkcs8_enc TEXT NOT NULL,
                public_jwk TEXT NOT NULL,
                created_at TEXT NOT NULL
            );
        ")?;

        // Additive column upgrades for databases created before the current schema.
        // ALTER TABLE ADD COLUMN is a no-op if the column already exists via IF NOT EXISTS
        // (supported in SQLite ≥ 3.37). For older SQLite, we try and ignore errors.
        let upgrades = [
            "ALTER TABLE users ADD COLUMN role TEXT NOT NULL DEFAULT 'user'",
            "ALTER TABLE users ADD COLUMN is_active INTEGER NOT NULL DEFAULT 1",
            // Why an account was deactivated ('guest_disabled' = the admin turned
            // guest self-registration off; such accounts auto-reactivate when it
            // returns and get a specific sign-in message meanwhile).
            "ALTER TABLE users ADD COLUMN deactivated_reason TEXT",
            "ALTER TABLE users ADD COLUMN is_public INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE sparql_services ADD COLUMN is_active INTEGER NOT NULL DEFAULT 1",
            "ALTER TABLE users ADD COLUMN avatar_key TEXT",
            "ALTER TABLE organisations ADD COLUMN image_key TEXT",
            "ALTER TABLE datasets ADD COLUMN image_key TEXT",
            // Wide banner/header images, distinct from the icon/cover image_key.
            "ALTER TABLE organisations ADD COLUMN banner_key TEXT",
            "ALTER TABLE datasets ADD COLUMN banner_key TEXT",
            "ALTER TABLE assets ADD COLUMN public INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE users ADD COLUMN can_publish INTEGER NOT NULL DEFAULT 0",
            // Migrate legacy 'publisher' role rows: grant can_publish and reset to 'user'
            "UPDATE users SET can_publish=1, role='user' WHERE role='publisher'",
            "ALTER TABLE datasets ADD COLUMN conforms_to_model TEXT",
            "ALTER TABLE datasets ADD COLUMN conforms_to_version TEXT",
            "ALTER TABLE datasets ADD COLUMN graph_role TEXT",
            "ALTER TABLE assets ADD COLUMN title TEXT",
            "ALTER TABLE assets ADD COLUMN description TEXT",
            "ALTER TABLE assets ADD COLUMN updated_at TEXT",
            // Dataset DCAT/ADMS/VoID metadata
            "ALTER TABLE datasets ADD COLUMN license TEXT",
            "ALTER TABLE datasets ADD COLUMN themes TEXT",
            "ALTER TABLE datasets ADD COLUMN keywords TEXT",
            "ALTER TABLE datasets ADD COLUMN contact_name TEXT",
            "ALTER TABLE datasets ADD COLUMN contact_email TEXT",
            "ALTER TABLE datasets ADD COLUMN contact_url TEXT",
            "ALTER TABLE datasets ADD COLUMN adms_status TEXT",
            "ALTER TABLE datasets ADD COLUMN version_notes TEXT",
            "ALTER TABLE datasets ADD COLUMN spatial TEXT",
            "ALTER TABLE datasets ADD COLUMN landing_page TEXT",
            // Organisation Linked Data / FOAF / vCard metadata fields
            "ALTER TABLE organisations ADD COLUMN homepage TEXT",
            "ALTER TABLE organisations ADD COLUMN identifier TEXT",
            "ALTER TABLE organisations ADD COLUMN contact_name TEXT",
            "ALTER TABLE organisations ADD COLUMN contact_email TEXT",
            "ALTER TABLE organisations ADD COLUMN contact_url TEXT",
            "ALTER TABLE organisations ADD COLUMN org_type TEXT NOT NULL DEFAULT 'FormalOrganization'",
            // Organisation hierarchy: parent organisation (org:subOrganizationOf).
            "ALTER TABLE organisations ADD COLUMN parent_org_id TEXT REFERENCES organisations(id)",
            // User FOAF/VCARD profile fields
            "ALTER TABLE users ADD COLUMN display_name TEXT",
            "ALTER TABLE users ADD COLUMN bio TEXT",
            "ALTER TABLE users ADD COLUMN website TEXT",
            "ALTER TABLE users ADD COLUMN phone TEXT",
            "ALTER TABLE users ADD COLUMN organization TEXT",
            "ALTER TABLE dataset_graphs ADD COLUMN graph_role TEXT",
            // Per-graph privacy: a private graph is hidden from dataset viewers and
            // the public — only principals who can write the owning dataset see it.
            "ALTER TABLE dataset_graphs ADD COLUMN private INTEGER NOT NULL DEFAULT 0",
            // Rename old role strings to the new canonical names.
            "UPDATE datasets SET graph_role = 'model' WHERE graph_role = 'tbox'",
            "UPDATE datasets SET graph_role = 'instances' WHERE graph_role = 'abox'",
            "UPDATE dataset_graphs SET graph_role = 'model' WHERE graph_role = 'tbox'",
            "UPDATE dataset_graphs SET graph_role = 'instances' WHERE graph_role = 'abox'",
            // Commit-style custom version name on each saved-query revision.
            "ALTER TABLE saved_query_revisions ADD COLUMN name TEXT",
            // SHACL Studio: shape-graph lifecycle status + generalised pipeline targets.
            "ALTER TABLE shape_sets ADD COLUMN status TEXT NOT NULL DEFAULT 'draft'",
            "ALTER TABLE validation_pipelines ADD COLUMN targets TEXT NOT NULL DEFAULT '[]'",
            // Derived-data write targets (inferred triples + validation results).
            "ALTER TABLE validation_pipelines ADD COLUMN inferred_target_kind TEXT NOT NULL DEFAULT 'in_place'",
            "ALTER TABLE validation_pipelines ADD COLUMN inferred_target_graph TEXT",
            "ALTER TABLE validation_pipelines ADD COLUMN results_target_kind TEXT NOT NULL DEFAULT 'none'",
            "ALTER TABLE validation_pipelines ADD COLUMN results_target_graph TEXT",
            // Refresh-token rotation families: every token minted from one login shares
            // a family id, so reuse-detection can revoke just that session instead of
            // every session the user has (which logged people out of their other
            // browsers/tabs). NULL on rows created before this column existed.
            "ALTER TABLE refresh_tokens ADD COLUMN family_id TEXT",
            // TOTP two-factor login columns.
            "ALTER TABLE users ADD COLUMN totp_secret_enc TEXT",
            "ALTER TABLE users ADD COLUMN totp_enabled INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE users ADD COLUMN totp_last_step INTEGER NOT NULL DEFAULT 0",
        ];
        for sql in &upgrades {
            let _ = conn.execute_batch(sql); // ignore "duplicate column" / already-run errors
        }

        // One-time upgrade: add users.email_verified, grandfathering every
        // EXISTING account as verified — they predate verification, and
        // retroactively unverifying them could lock people out when the
        // OTS_REQUIRE_VERIFIED_EMAIL gate is enabled. Detected via
        // pragma_table_info so the backfill runs exactly once (fresh databases
        // get the column from CREATE TABLE above and skip this path).
        let has_email_verified = conn
            .prepare("SELECT 1 FROM pragma_table_info('users') WHERE name='email_verified'")?
            .exists([])?;
        if !has_email_verified {
            conn.execute_batch(
                "ALTER TABLE users ADD COLUMN email_verified INTEGER NOT NULL DEFAULT 0;
                 UPDATE users SET email_verified = 1;",
            )?;
        }

        // One-time rename: datasets.conforms_to_ontology → conforms_to_model (the
        // field was renamed when "ontology" stopped being the umbrella term — it is
        // now "Model"). The additive upgrade above adds `conforms_to_model`; a DB
        // created by a prior build still carries data in the legacy
        // `conforms_to_ontology` column. Copy it across and drop the legacy column.
        // Guarded on the legacy column's presence so it runs at most once.
        let has_legacy_conforms = conn
            .query_row(
                "SELECT 1 FROM pragma_table_info('datasets') WHERE name='conforms_to_ontology'",
                [],
                |_| Ok(()),
            )
            .is_ok();
        if has_legacy_conforms {
            let _ = conn.execute_batch(
                "UPDATE datasets SET conforms_to_model = conforms_to_ontology \
                 WHERE conforms_to_model IS NULL AND conforms_to_ontology IS NOT NULL; \
                 ALTER TABLE datasets DROP COLUMN conforms_to_ontology;",
            );
        }

        // One-time rebuild: widen `resource_access.principal_type` to allow
        // 'organisation' (older DBs were created with CHECK IN ('user','group')).
        // SQLite cannot ALTER a CHECK, so we rebuild the table. Detect the old
        // definition by inspecting the stored CREATE SQL so this runs at most once.
        let resource_access_needs_widening = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type='table' AND name='resource_access'",
                [],
                |r| r.get::<_, String>(0),
            )
            .map(|sql: String| !sql.contains("'organisation'"))
            .unwrap_or(false);
        if resource_access_needs_widening {
            conn.execute_batch(
                "BEGIN;
                 CREATE TABLE resource_access_new (
                     id TEXT PRIMARY KEY,
                     resource_type TEXT NOT NULL CHECK(resource_type IN ('dataset','model','vocabulary')),
                     resource_id TEXT NOT NULL,
                     principal_type TEXT NOT NULL CHECK(principal_type IN ('user','group','organisation')),
                     principal_id TEXT NOT NULL,
                     role TEXT NOT NULL CHECK(role IN ('viewer','editor','admin')),
                     created_at TEXT NOT NULL,
                     created_by TEXT NOT NULL,
                     UNIQUE(resource_type, resource_id, principal_type, principal_id)
                 );
                 INSERT INTO resource_access_new SELECT * FROM resource_access;
                 DROP TABLE resource_access;
                 ALTER TABLE resource_access_new RENAME TO resource_access;
                 CREATE INDEX IF NOT EXISTS idx_resource_access_resource ON resource_access(resource_type, resource_id);
                 CREATE INDEX IF NOT EXISTS idx_resource_access_principal ON resource_access(principal_type, principal_id);
                 COMMIT;",
            )?;
        }

        Ok(())
    }

    // ─── User CRUD ────────────────────────────────────────────────────────────

    pub fn create_user(
        &self,
        id: &str,
        username: &str,
        email: &str,
        password_hash: &str,
        role: SystemRole,
    ) -> anyhow::Result<User> {
        let conn = self.pool.get()?;
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO users (id, username, email, password_hash, is_admin, role, is_active, created_at, updated_at) VALUES (?1,?2,?3,?4,?5,?6,1,?7,?8)",
            params![id, username, email, password_hash, role.is_admin() as i32, role.as_str(), now, now],
        )?;
        Ok(User {
            id: id.to_string(),
            username: username.to_string(),
            email: email.to_string(),
            password_hash: password_hash.to_string(),
            role,
            is_active: true,
            is_public: false,
            can_publish: false,
            avatar_key: None,
            created_at: now.clone(),
            updated_at: now,
            display_name: None,
            bio: None,
            website: None,
            phone: None,
            organization: None,
            email_verified: false,
            totp_enabled: false,
        })
    }

    pub fn get_user_by_id(&self, id: &str) -> anyhow::Result<Option<User>> {
        let conn = self.pool.get()?;
        conn.query_row(
            &format!("SELECT {} FROM users WHERE id = ?1", USER_COLS),
            params![id],
            read_user,
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn get_user_by_username(&self, username: &str) -> anyhow::Result<Option<User>> {
        let conn = self.pool.get()?;
        conn.query_row(
            &format!("SELECT {} FROM users WHERE username = ?1", USER_COLS),
            params![username],
            read_user,
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn get_user_by_email(&self, email: &str) -> anyhow::Result<Option<User>> {
        let conn = self.pool.get()?;
        conn.query_row(
            &format!("SELECT {} FROM users WHERE email = ?1", USER_COLS),
            params![email],
            read_user,
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn update_user(&self, id: &str, username: &str, email: &str) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE users SET username=?1, email=?2, updated_at=?3 WHERE id=?4",
            params![username, email, now, id],
        )?;
        Ok(())
    }

    pub fn update_password(&self, id: &str, password_hash: &str) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE users SET password_hash=?1, updated_at=?2 WHERE id=?3",
            params![password_hash, now, id],
        )?;
        Ok(())
    }

    /// Mark the user's email address as verified (or not).
    pub fn set_email_verified(&self, id: &str, verified: bool) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE users SET email_verified=?1, updated_at=?2 WHERE id=?3",
            params![verified as i32, now, id],
        )?;
        Ok(())
    }

    /// Switch the account to a new (just-confirmed) email address.
    pub fn update_user_email_verified(&self, id: &str, email: &str) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE users SET email=?1, email_verified=1, updated_at=?2 WHERE id=?3",
            params![email, now, id],
        )?;
        Ok(())
    }

    // ─── Email action tokens (verification / reset / change-email) ───────────

    pub fn create_email_token(
        &self,
        id: &str,
        user_id: &str,
        kind: &str,
        token_hash: &str,
        new_email: Option<&str>,
        expires_at: &str,
    ) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO email_tokens (id, user_id, kind, token_hash, new_email, expires_at, created_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7)",
            params![id, user_id, kind, token_hash, new_email, expires_at, now],
        )?;
        Ok(())
    }

    pub fn get_email_token_by_hash(&self, token_hash: &str) -> anyhow::Result<Option<EmailToken>> {
        let conn = self.pool.get()?;
        conn.query_row(
            "SELECT id, user_id, kind, token_hash, new_email, expires_at, created_at, used_at
             FROM email_tokens WHERE token_hash = ?1",
            params![token_hash],
            |row| {
                Ok(EmailToken {
                    id: row.get(0)?,
                    user_id: row.get(1)?,
                    kind: row.get(2)?,
                    token_hash: row.get(3)?,
                    new_email: row.get(4)?,
                    expires_at: row.get(5)?,
                    created_at: row.get(6)?,
                    used_at: row.get(7)?,
                })
            },
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn mark_email_token_used(&self, id: &str) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE email_tokens SET used_at=?1 WHERE id=?2 AND used_at IS NULL",
            params![now, id],
        )?;
        Ok(())
    }

    /// Void every outstanding token of `kind` for a user (issuing a fresh one
    /// supersedes the old, and a completed reset voids its siblings).
    pub fn invalidate_email_tokens(&self, user_id: &str, kind: &str) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE email_tokens SET used_at=?1 WHERE user_id=?2 AND kind=?3 AND used_at IS NULL",
            params![now, user_id, kind],
        )?;
        Ok(())
    }

    /// Creation time of the newest still-valid token of `kind` for a user —
    /// drives per-account resend throttles.
    pub fn latest_email_token_created_at(
        &self,
        user_id: &str,
        kind: &str,
    ) -> anyhow::Result<Option<String>> {
        let conn = self.pool.get()?;
        conn.query_row(
            "SELECT MAX(created_at) FROM email_tokens
             WHERE user_id=?1 AND kind=?2 AND used_at IS NULL",
            params![user_id, kind],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .map(|o| o.flatten())
        .map_err(Into::into)
    }

    /// The pending new address of an outstanding change-email token, if any.
    pub fn pending_email_change(&self, user_id: &str) -> anyhow::Result<Option<String>> {
        let conn = self.pool.get()?;
        let now = chrono::Utc::now().to_rfc3339();
        conn.query_row(
            "SELECT new_email FROM email_tokens
             WHERE user_id=?1 AND kind='change_email' AND used_at IS NULL AND expires_at > ?2
             ORDER BY created_at DESC LIMIT 1",
            params![user_id, now],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .map(|o| o.flatten())
        .map_err(Into::into)
    }

    // ─── TOTP two-factor login ────────────────────────────────────────────────

    /// Store (or clear) the encrypted TOTP secret. Setting a new secret resets
    /// the enabled flag and replay guard — enablement requires a correct code.
    pub fn set_totp_secret(&self, id: &str, secret_enc: Option<&str>) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE users SET totp_secret_enc=?1, totp_enabled=0, totp_last_step=0, updated_at=?2 WHERE id=?3",
            params![secret_enc, now, id],
        )?;
        Ok(())
    }

    pub fn get_totp_secret(&self, id: &str) -> anyhow::Result<Option<String>> {
        let conn = self.pool.get()?;
        conn.query_row(
            "SELECT totp_secret_enc FROM users WHERE id = ?1",
            params![id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .map(|o| o.flatten())
        .map_err(Into::into)
    }

    pub fn set_totp_enabled(&self, id: &str, enabled: bool) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE users SET totp_enabled=?1, updated_at=?2 WHERE id=?3",
            params![enabled as i32, now, id],
        )?;
        Ok(())
    }

    /// Replay guard: record the last successfully-used TOTP step. Returns true
    /// only if `step` is strictly newer than the stored one (compare-and-set,
    /// so two concurrent logins can't both consume the same code).
    pub fn try_advance_totp_step(&self, id: &str, step: u64) -> anyhow::Result<bool> {
        let conn = self.pool.get()?;
        let n = conn.execute(
            "UPDATE users SET totp_last_step=?1 WHERE id=?2 AND totp_last_step < ?1",
            params![step as i64, id],
        )?;
        Ok(n > 0)
    }

    pub fn get_totp_last_step(&self, id: &str) -> anyhow::Result<u64> {
        let conn = self.pool.get()?;
        let v: i64 = conn.query_row(
            "SELECT COALESCE(totp_last_step,0) FROM users WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )?;
        Ok(v.max(0) as u64)
    }

    /// Replace the user's recovery codes with a fresh set (stored hashed).
    pub fn replace_recovery_codes(
        &self,
        user_id: &str,
        code_hashes: &[String],
    ) -> anyhow::Result<()> {
        let mut conn = self.pool.get()?;
        let tx = conn.transaction()?;
        let now = chrono::Utc::now().to_rfc3339();
        tx.execute(
            "DELETE FROM totp_recovery_codes WHERE user_id=?1",
            params![user_id],
        )?;
        for hash in code_hashes {
            tx.execute(
                "INSERT INTO totp_recovery_codes (id, user_id, code_hash, created_at) VALUES (?1,?2,?3,?4)",
                params![uuid::Uuid::new_v4().to_string(), user_id, hash, now],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Atomically consume one unused recovery code. True when a code matched.
    pub fn consume_recovery_code(&self, user_id: &str, code_hash: &str) -> anyhow::Result<bool> {
        let conn = self.pool.get()?;
        let now = chrono::Utc::now().to_rfc3339();
        let n = conn.execute(
            "UPDATE totp_recovery_codes SET used_at=?1
             WHERE user_id=?2 AND code_hash=?3 AND used_at IS NULL",
            params![now, user_id, code_hash],
        )?;
        Ok(n > 0)
    }

    pub fn count_unused_recovery_codes(&self, user_id: &str) -> anyhow::Result<i64> {
        let conn = self.pool.get()?;
        conn.query_row(
            "SELECT COUNT(*) FROM totp_recovery_codes WHERE user_id=?1 AND used_at IS NULL",
            params![user_id],
            |row| row.get(0),
        )
        .map_err(Into::into)
    }

    pub fn clear_recovery_codes(&self, user_id: &str) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        conn.execute(
            "DELETE FROM totp_recovery_codes WHERE user_id=?1",
            params![user_id],
        )?;
        Ok(())
    }

    // ─── WebAuthn passkeys ────────────────────────────────────────────────────

    #[allow(clippy::too_many_arguments)]
    pub fn create_webauthn_credential(
        &self,
        id: &str,
        user_id: &str,
        credential_id: &str,
        public_key: &str,
        counter: i64,
        transports: Option<&str>,
        name: &str,
    ) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO webauthn_credentials
                 (id, user_id, credential_id, public_key, counter, transports, name, created_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            params![
                id,
                user_id,
                credential_id,
                public_key,
                counter,
                transports,
                name,
                now
            ],
        )?;
        Ok(())
    }

    fn row_to_webauthn_credential(row: &rusqlite::Row) -> rusqlite::Result<WebauthnCredential> {
        Ok(WebauthnCredential {
            id: row.get(0)?,
            user_id: row.get(1)?,
            credential_id: row.get(2)?,
            public_key: row.get(3)?,
            counter: row.get(4)?,
            transports: row.get(5)?,
            name: row.get(6)?,
            created_at: row.get(7)?,
            last_used_at: row.get(8)?,
        })
    }

    const WEBAUTHN_CREDENTIAL_COLS: &'static str =
        "id, user_id, credential_id, public_key, counter, transports, name, created_at, last_used_at";

    pub fn list_webauthn_credentials(
        &self,
        user_id: &str,
    ) -> anyhow::Result<Vec<WebauthnCredential>> {
        let conn = self.pool.get()?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {} FROM webauthn_credentials WHERE user_id=?1 ORDER BY created_at",
            Self::WEBAUTHN_CREDENTIAL_COLS
        ))?;
        let rows = stmt.query_map(params![user_id], Self::row_to_webauthn_credential)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Look up a credential by its (globally unique) WebAuthn credential ID.
    pub fn get_webauthn_credential_by_cred_id(
        &self,
        credential_id: &str,
    ) -> anyhow::Result<Option<WebauthnCredential>> {
        let conn = self.pool.get()?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {} FROM webauthn_credentials WHERE credential_id=?1",
            Self::WEBAUTHN_CREDENTIAL_COLS
        ))?;
        let mut rows = stmt.query_map(params![credential_id], Self::row_to_webauthn_credential)?;
        rows.next().transpose().map_err(Into::into)
    }

    /// Persist the post-authentication credential state: updated serialized
    /// passkey (counter/backup flags), denormalised counter, and last-used time.
    pub fn update_webauthn_credential_usage(
        &self,
        id: &str,
        public_key: &str,
        counter: i64,
    ) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE webauthn_credentials SET public_key=?1, counter=?2, last_used_at=?3 WHERE id=?4",
            params![public_key, counter, now, id],
        )?;
        Ok(())
    }

    /// Delete one of `user_id`'s passkeys. Returns false when the row did not
    /// exist or belongs to another user (scoping the DELETE prevents IDOR).
    pub fn delete_webauthn_credential(&self, id: &str, user_id: &str) -> anyhow::Result<bool> {
        let conn = self.pool.get()?;
        let n = conn.execute(
            "DELETE FROM webauthn_credentials WHERE id=?1 AND user_id=?2",
            params![id, user_id],
        )?;
        Ok(n > 0)
    }

    pub fn update_user_role(&self, id: &str, role: SystemRole) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE users SET role=?1, is_admin=?2, updated_at=?3 WHERE id=?4",
            params![role.as_str(), role.is_admin() as i32, now, id],
        )?;
        Ok(())
    }

    pub fn update_user_can_publish(&self, id: &str, can_publish: bool) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE users SET can_publish=?1, updated_at=?2 WHERE id=?3",
            params![can_publish as i32, now, id],
        )?;
        Ok(())
    }

    pub fn set_user_active(&self, id: &str, is_active: bool) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE users SET is_active=?1, updated_at=?2 WHERE id=?3",
            params![is_active as i32, now, id],
        )?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)] // flat profile fields; a struct adds churn
    pub fn update_user_profile(
        &self,
        id: &str,
        display_name: Option<&str>,
        bio: Option<&str>,
        website: Option<&str>,
        phone: Option<&str>,
        organization: Option<&str>,
        is_public: bool,
    ) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE users SET display_name=?1, bio=?2, website=?3, phone=?4, organization=?5, is_public=?6, updated_at=?7 WHERE id=?8",
            params![display_name, bio, website, phone, organization, is_public as i32, now, id],
        )?;
        Ok(())
    }

    pub fn update_user_public(&self, id: &str, is_public: bool) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE users SET is_public=?1, updated_at=?2 WHERE id=?3",
            params![is_public as i32, now, id],
        )?;
        Ok(())
    }

    pub fn list_public_users(&self) -> anyhow::Result<Vec<User>> {
        let conn = self.pool.get()?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {} FROM users WHERE is_public=1 AND is_active=1 ORDER BY username",
            USER_COLS
        ))?;
        let users = stmt
            .query_map([], read_user)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(users)
    }

    pub fn list_users(&self) -> anyhow::Result<Vec<User>> {
        let conn = self.pool.get()?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {} FROM users ORDER BY username",
            USER_COLS
        ))?;
        let users = stmt
            .query_map([], read_user)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(users)
    }

    pub fn count_users(&self) -> anyhow::Result<i64> {
        let conn = self.pool.get()?;
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM users", [], |row| row.get(0))?;
        Ok(count)
    }

    // ─── Membership summaries (for token introspection / resource servers) ────

    /// `(org_slug, org_name, membership_role)` for every organisation the user
    /// belongs to — the compact shape `GET /api/auth/me` exposes so resource
    /// servers (validation platform, form service, LLM gateway) can authorize
    /// on membership without extra round-trips.
    pub fn list_user_membership_summaries(
        &self,
        user_id: &str,
    ) -> anyhow::Result<Vec<(String, String, String)>> {
        let conn = self.pool.get()?;
        let mut stmt = conn.prepare(
            "SELECT o.slug, o.name, m.role FROM org_memberships m
             JOIN organisations o ON o.id = m.org_id
             WHERE m.user_id = ?1 ORDER BY o.slug",
        )?;
        let rows = stmt
            .query_map([user_id], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// `(org_slug, group_id, group_name)` for every group/team the user is in.
    /// Groups have no slug of their own — clients match on name or id.
    pub fn list_user_group_summaries(
        &self,
        user_id: &str,
    ) -> anyhow::Result<Vec<(String, String, String)>> {
        let conn = self.pool.get()?;
        let mut stmt = conn.prepare(
            "SELECT o.slug, g.id, g.name FROM group_memberships gm
             JOIN groups g ON g.id = gm.group_id
             JOIN organisations o ON o.id = g.org_id
             WHERE gm.user_id = ?1 ORDER BY o.slug, g.name",
        )?;
        let rows = stmt
            .query_map([user_id], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Active members of one organisation.
    pub fn count_org_members(&self, org_id: &str) -> anyhow::Result<i64> {
        let conn = self.pool.get()?;
        Ok(conn.query_row(
            "SELECT COUNT(*) FROM org_memberships WHERE org_id = ?1",
            [org_id],
            |r| r.get(0),
        )?)
    }

    /// Groups/teams of one organisation.
    pub fn count_org_groups(&self, org_id: &str) -> anyhow::Result<i64> {
        let conn = self.pool.get()?;
        Ok(conn.query_row(
            "SELECT COUNT(*) FROM groups WHERE org_id = ?1",
            [org_id],
            |r| r.get(0),
        )?)
    }

    /// Compact LLM request-log aggregates for the plugin capability / dashboard:
    /// `{ last_24h: { by_status, requests }, top_users_7d: [{user, requests}] }`.
    /// (The richer admin endpoint lives in `server::llm_guard`.)
    pub fn llm_request_aggregates(&self) -> anyhow::Result<serde_json::Value> {
        let conn = self.pool.get()?;
        let day_ago = (chrono::Utc::now() - chrono::Duration::hours(24)).to_rfc3339();
        let week_ago = (chrono::Utc::now() - chrono::Duration::days(7)).to_rfc3339();
        let mut by_status = serde_json::Map::new();
        let mut total: i64 = 0;
        {
            let mut stmt = conn.prepare(
                "SELECT status, COUNT(*) FROM llm_request_log WHERE timestamp >= ?1 GROUP BY status",
            )?;
            let rows = stmt
                .query_map([&day_ago], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
            for row in rows {
                let (s, n) = row?;
                total += n;
                by_status.insert(s, serde_json::Value::from(n));
            }
        }
        let mut top_users = Vec::new();
        {
            let mut stmt = conn.prepare(
                "SELECT COALESCE(u.username, l.user_id, 'anonymous'), COUNT(*)
                 FROM llm_request_log l LEFT JOIN users u ON u.id = l.user_id
                 WHERE l.timestamp >= ?1
                 GROUP BY COALESCE(u.username, l.user_id, 'anonymous')
                 ORDER BY COUNT(*) DESC LIMIT 10",
            )?;
            let rows = stmt.query_map([&week_ago], |r| {
                Ok(serde_json::json!({
                    "user": r.get::<_, String>(0)?,
                    "requests": r.get::<_, i64>(1)?,
                }))
            })?;
            for row in rows {
                top_users.push(row?);
            }
        }
        Ok(serde_json::json!({
            "last_24h": { "by_status": by_status, "requests": total },
            "top_users_7d": top_users,
        }))
    }

    // ─── OIDC provider: clients, codes, refresh tokens, consents, keys ────────

    /// Create or update a relying-party client. `redirect_uris` is stored as a
    /// JSON array (exact-match allowlist); `secret_enc` is the AES-GCM blob for
    /// confidential clients (None clears none — pass through what you have).
    pub fn upsert_oauth_client(
        &self,
        client_id: &str,
        name: &str,
        redirect_uris: &[String],
        public: bool,
        secret_enc: Option<&str>,
    ) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        conn.execute(
            "INSERT INTO oauth_clients (client_id, name, redirect_uris, public, secret_enc, created_at)
             VALUES (?1,?2,?3,?4,?5,?6)
             ON CONFLICT(client_id) DO UPDATE SET
               name = excluded.name,
               redirect_uris = excluded.redirect_uris,
               public = excluded.public,
               secret_enc = COALESCE(excluded.secret_enc, oauth_clients.secret_enc)",
            params![
                client_id,
                name,
                serde_json::to_string(redirect_uris)?,
                public as i64,
                secret_enc,
                chrono::Utc::now().to_rfc3339()
            ],
        )?;
        Ok(())
    }

    pub fn get_oauth_client(&self, client_id: &str) -> anyhow::Result<Option<OidcClient>> {
        let conn = self.pool.get()?;
        conn.query_row(
            "SELECT client_id, name, redirect_uris, public, secret_enc, created_at
             FROM oauth_clients WHERE client_id = ?1",
            [client_id],
            map_oidc_client_row,
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn list_oauth_clients(&self) -> anyhow::Result<Vec<OidcClient>> {
        let conn = self.pool.get()?;
        let mut stmt = conn.prepare(
            "SELECT client_id, name, redirect_uris, public, secret_enc, created_at
             FROM oauth_clients ORDER BY client_id",
        )?;
        let rows = stmt
            .query_map([], map_oidc_client_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn delete_oauth_client(&self, client_id: &str) -> anyhow::Result<bool> {
        let conn = self.pool.get()?;
        let n = conn.execute(
            "DELETE FROM oauth_clients WHERE client_id = ?1",
            [client_id],
        )?;
        conn.execute(
            "DELETE FROM oauth_client_refresh_tokens WHERE client_id = ?1",
            [client_id],
        )?;
        Ok(n > 0)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn insert_oauth_code(
        &self,
        code_hash: &str,
        client_id: &str,
        user_id: &str,
        redirect_uri: &str,
        scope: &str,
        nonce: Option<&str>,
        code_challenge: &str,
        expires_at: &str,
    ) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        conn.execute(
            "INSERT INTO oauth_auth_codes
               (code_hash, client_id, user_id, redirect_uri, scope, nonce, code_challenge, expires_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            params![code_hash, client_id, user_id, redirect_uri, scope, nonce, code_challenge, expires_at],
        )?;
        Ok(())
    }

    /// Atomically consume an authorization code: returns its row only the FIRST
    /// time, and only while unexpired. (Single-use is enforced by the UPDATE's
    /// used=0 guard, so two racing exchanges can't both win.)
    pub fn consume_oauth_code(&self, code_hash: &str) -> anyhow::Result<Option<OidcAuthCode>> {
        let conn = self.pool.get()?;
        let now = chrono::Utc::now().to_rfc3339();
        let n = conn.execute(
            "UPDATE oauth_auth_codes SET used = 1
             WHERE code_hash = ?1 AND used = 0 AND expires_at > ?2",
            params![code_hash, now],
        )?;
        if n == 0 {
            return Ok(None);
        }
        conn.query_row(
            "SELECT client_id, user_id, redirect_uri, scope, nonce, code_challenge
             FROM oauth_auth_codes WHERE code_hash = ?1",
            [code_hash],
            |r| {
                Ok(OidcAuthCode {
                    client_id: r.get(0)?,
                    user_id: r.get(1)?,
                    redirect_uri: r.get(2)?,
                    scope: r.get(3)?,
                    nonce: r.get(4)?,
                    code_challenge: r.get(5)?,
                })
            },
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn insert_client_refresh_token(
        &self,
        token_hash: &str,
        client_id: &str,
        user_id: &str,
        scope: &str,
        expires_at: &str,
    ) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        conn.execute(
            "INSERT INTO oauth_client_refresh_tokens (token_hash, client_id, user_id, scope, expires_at)
             VALUES (?1,?2,?3,?4,?5)",
            params![token_hash, client_id, user_id, scope, expires_at],
        )?;
        Ok(())
    }

    /// Atomically take (delete + return) a refresh token — rotation makes every
    /// refresh single-use; a replayed old token simply finds nothing.
    pub fn take_client_refresh_token(
        &self,
        token_hash: &str,
    ) -> anyhow::Result<Option<(String, String, String, String)>> {
        let conn = self.pool.get()?;
        let row = conn
            .query_row(
                "SELECT client_id, user_id, scope, expires_at
                 FROM oauth_client_refresh_tokens WHERE token_hash = ?1",
                [token_hash],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()?;
        if row.is_some() {
            conn.execute(
                "DELETE FROM oauth_client_refresh_tokens WHERE token_hash = ?1",
                [token_hash],
            )?;
        }
        Ok(row)
    }

    /// Has the user already consented to this client for (at least) `scope`?
    pub fn has_oauth_consent(&self, user_id: &str, client_id: &str, scope: &str) -> bool {
        let granted: Option<String> = (|| {
            let conn = self.pool.get().ok()?;
            conn.query_row(
                "SELECT scope FROM oauth_consents WHERE user_id = ?1 AND client_id = ?2",
                params![user_id, client_id],
                |r| r.get(0),
            )
            .optional()
            .ok()?
        })();
        match granted {
            Some(g) => {
                let have: std::collections::HashSet<&str> = g.split_whitespace().collect();
                scope.split_whitespace().all(|s| have.contains(s))
            }
            None => false,
        }
    }

    pub fn record_oauth_consent(
        &self,
        user_id: &str,
        client_id: &str,
        scope: &str,
    ) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        conn.execute(
            "INSERT INTO oauth_consents (user_id, client_id, scope, granted_at)
             VALUES (?1,?2,?3,?4)
             ON CONFLICT(user_id, client_id) DO UPDATE SET
               scope = excluded.scope, granted_at = excluded.granted_at",
            params![user_id, client_id, scope, chrono::Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn get_signing_key(&self) -> anyhow::Result<Option<(String, String, String, String)>> {
        let conn = self.pool.get()?;
        conn.query_row(
            "SELECT kid, alg, pkcs8_enc, public_jwk FROM oauth_signing_keys
             ORDER BY created_at DESC LIMIT 1",
            [],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                ))
            },
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn insert_signing_key(
        &self,
        kid: &str,
        alg: &str,
        pkcs8_enc: &str,
        public_jwk: &str,
    ) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        conn.execute(
            "INSERT INTO oauth_signing_keys (kid, alg, pkcs8_enc, public_jwk, created_at)
             VALUES (?1,?2,?3,?4,?5)",
            params![
                kid,
                alg,
                pkcs8_enc,
                public_jwk,
                chrono::Utc::now().to_rfc3339()
            ],
        )?;
        Ok(())
    }

    // ─── App settings (runtime-changeable admin toggles) ──────────────────────

    /// Read one instance setting; None when never set.
    pub fn get_app_setting(&self, key: &str) -> anyhow::Result<Option<String>> {
        let conn = self.pool.get()?;
        let v = conn
            .query_row(
                "SELECT value FROM app_settings WHERE key = ?1",
                [key],
                |r| r.get::<_, String>(0),
            )
            .optional()?;
        Ok(v)
    }

    /// Upsert one instance setting.
    pub fn set_app_setting(&self, key: &str, value: &str) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        conn.execute(
            "INSERT INTO app_settings (key, value, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
            rusqlite::params![key, value, chrono::Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    /// Convenience: a boolean setting with a default.
    pub fn app_setting_bool(&self, key: &str, default: bool) -> bool {
        match self.get_app_setting(key) {
            Ok(Some(v)) => matches!(v.as_str(), "true" | "1" | "yes" | "on"),
            _ => default,
        }
    }

    // ─── Guest lifecycle (guest self-registration toggle) ─────────────────────

    /// Why the account is deactivated, if it is (see `deactivated_reason`).
    pub fn deactivation_reason(&self, user_id: &str) -> anyhow::Result<Option<String>> {
        let conn = self.pool.get()?;
        let v = conn
            .query_row(
                "SELECT deactivated_reason FROM users WHERE id = ?1",
                [user_id],
                |r| r.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten();
        Ok(v)
    }

    /// Bulk-deactivate every active guest account, stamping the reason.
    /// Returns how many accounts were disabled.
    pub fn deactivate_guests(&self, reason: &str) -> anyhow::Result<usize> {
        let conn = self.pool.get()?;
        let n = conn.execute(
            "UPDATE users SET is_active = 0, deactivated_reason = ?1, updated_at = ?2
             WHERE role = 'guest' AND is_active = 1",
            rusqlite::params![reason, chrono::Utc::now().to_rfc3339()],
        )?;
        Ok(n)
    }

    /// Reactivate the guest accounts a previous toggle-off disabled (only those
    /// carrying the matching reason — a guest an admin deactivated individually
    /// stays deactivated). Returns how many accounts came back.
    pub fn reactivate_guests(&self, reason: &str) -> anyhow::Result<usize> {
        let conn = self.pool.get()?;
        let n = conn.execute(
            "UPDATE users SET is_active = 1, deactivated_reason = NULL, updated_at = ?2
             WHERE role = 'guest' AND is_active = 0 AND deactivated_reason = ?1",
            rusqlite::params![reason, chrono::Utc::now().to_rfc3339()],
        )?;
        Ok(n)
    }

    // ─── Per-account login throttle (brute-force / credential-stuffing) ───────

    /// Threshold of failures (within the sliding window) before an account locks.
    const LOGIN_LOCK_THRESHOLD: i64 = 8;
    /// Sliding window over which failures accumulate (seconds).
    const LOGIN_LOCK_WINDOW_SECS: i64 = 900; // 15 min
    /// How long the account stays locked once the threshold is crossed (seconds).
    const LOGIN_LOCK_DURATION_SECS: i64 = 900; // 15 min

    /// True if the account is currently locked out (a future `locked_until`).
    pub fn is_login_locked(&self, username: &str) -> anyhow::Result<bool> {
        let conn = self.pool.get()?;
        let locked_until: Option<String> = conn
            .query_row(
                "SELECT locked_until FROM login_attempts WHERE username = ?1",
                [username],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        Ok(match locked_until {
            Some(until) => chrono::DateTime::parse_from_rfc3339(&until)
                .map(|t| t.with_timezone(&chrono::Utc) > chrono::Utc::now())
                .unwrap_or(false),
            None => false,
        })
    }

    /// Record a failed login for `username`, locking the account once
    /// `LOGIN_LOCK_THRESHOLD` failures accumulate within `LOGIN_LOCK_WINDOW_SECS`.
    pub fn record_login_failure(&self, username: &str) -> anyhow::Result<()> {
        let mut conn = self.pool.get()?;
        let now = chrono::Utc::now();
        let now_s = now.to_rfc3339();
        // IMMEDIATE acquires the write lock up front, so the SELECT below cannot be
        // interleaved with another writer's increment for the same row — concurrent
        // failed attempts serialize and each sees a fresh `failed_count` rather than
        // racing on a stale read and under-counting (which would let an attacker stay
        // under the lock threshold).
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let row: Option<(i64, Option<String>)> = tx
            .query_row(
                "SELECT failed_count, first_failed_at FROM login_attempts WHERE username = ?1",
                [username],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let window_expired = row
            .as_ref()
            .and_then(|(_, f)| f.as_deref())
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|t| {
                (now - t.with_timezone(&chrono::Utc)).num_seconds() > Self::LOGIN_LOCK_WINDOW_SECS
            })
            .unwrap_or(true);
        let (new_count, new_first) = match (row, window_expired) {
            (Some((c, Some(f))), false) => (c + 1, f),
            _ => (1, now_s.clone()),
        };
        let locked_until = (new_count >= Self::LOGIN_LOCK_THRESHOLD).then(|| {
            (now + chrono::Duration::seconds(Self::LOGIN_LOCK_DURATION_SECS)).to_rfc3339()
        });
        tx.execute(
            "INSERT INTO login_attempts (username, failed_count, first_failed_at, locked_until) \
             VALUES (?1, ?2, ?3, ?4) \
             ON CONFLICT(username) DO UPDATE SET failed_count = ?2, first_failed_at = ?3, locked_until = ?4",
            params![username, new_count, new_first, locked_until],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Clear the throttle for `username` (called on a successful login).
    pub fn clear_login_attempts(&self, username: &str) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        conn.execute("DELETE FROM login_attempts WHERE username = ?1", [username])?;
        Ok(())
    }

    /// Count active users holding the `super_admin` role. Used to refuse any
    /// operation (demotion, deactivation, deletion) that would leave the system
    /// with zero super admins and therefore unadministrable / unrecoverable.
    pub fn count_active_super_admins(&self) -> anyhow::Result<i64> {
        let conn = self.pool.get()?;
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM users WHERE role = 'super_admin' AND is_active = 1",
            [],
            |row| row.get(0),
        )?;
        Ok(count)
    }

    /// Paginated user list with optional search.
    pub fn list_users_paginated(
        &self,
        page: i64,
        limit: i64,
        search: Option<&str>,
    ) -> anyhow::Result<(Vec<User>, i64)> {
        let conn = self.pool.get()?;
        let offset = (page - 1) * limit;

        #[allow(clippy::type_complexity)]
        let (where_clause, count_params, query_params): (
            String,
            Vec<Box<dyn rusqlite::types::ToSql>>,
            Vec<Box<dyn rusqlite::types::ToSql>>,
        ) = if let Some(search) = search {
            // Escape LIKE metacharacters so the term matches literally — without
            // this, a search of `%` or `_` is treated as a wildcard and scans
            // every user. The `\` escape character is declared via ESCAPE below.
            let escaped = search
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_");
            let pattern = format!("%{}%", escaped);
            (
                " WHERE username LIKE ?1 ESCAPE '\\' OR email LIKE ?1 ESCAPE '\\'".to_string(),
                vec![Box::new(pattern.clone())],
                vec![Box::new(pattern), Box::new(limit), Box::new(offset)],
            )
        } else {
            (
                String::new(),
                vec![],
                vec![Box::new(limit), Box::new(offset)],
            )
        };

        let total: i64 = if count_params.is_empty() {
            conn.query_row(
                &format!("SELECT COUNT(*) FROM users{}", where_clause),
                [],
                |row| row.get(0),
            )?
        } else {
            conn.query_row(
                &format!("SELECT COUNT(*) FROM users{}", where_clause),
                rusqlite::params_from_iter(&count_params),
                |row| row.get(0),
            )?
        };

        let param_offset = if search.is_some() { 2 } else { 1 };
        let sql = format!(
            "SELECT {} FROM users{} ORDER BY username LIMIT ?{} OFFSET ?{}",
            USER_COLS,
            where_clause,
            param_offset,
            param_offset + 1
        );

        let mut stmt = conn.prepare(&sql)?;
        let users = stmt
            .query_map(rusqlite::params_from_iter(&query_params), read_user)?
            .collect::<Result<Vec<_>, _>>()?;

        Ok((users, total))
    }

    pub fn delete_user(&self, id: &str) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        // assets.uploaded_by has no CASCADE — clear it before deleting the user
        // to avoid a FK constraint violation. Asset metadata is lost; S3 objects
        // become orphaned (acceptable for a permanent purge operation).
        conn.execute("DELETE FROM assets WHERE uploaded_by = ?1", params![id])?;
        conn.execute("DELETE FROM users WHERE id = ?1", params![id])?;
        Ok(())
    }

    // ─── Deactivation side-effects ────────────────────────────────────────────

    /// Make all personal (owner_type='user') datasets owned by this user private.
    pub fn make_user_datasets_private(&self, user_id: &str) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        conn.execute(
            "UPDATE datasets SET visibility='private' WHERE owner_type='user' AND owner_id=?1",
            params![user_id],
        )?;
        Ok(())
    }

    /// Return the IDs of all organisations this user is currently a member of.
    pub fn get_user_org_ids(&self, user_id: &str) -> anyhow::Result<Vec<String>> {
        let conn = self.pool.get()?;
        let mut stmt = conn.prepare("SELECT org_id FROM org_memberships WHERE user_id=?1")?;
        let ids = stmt
            .query_map(params![user_id], |row| row.get(0))?
            .collect::<Result<Vec<String>, _>>()?;
        Ok(ids)
    }

    /// Count how many *other* active users remain in an org after excluding one user.
    pub fn count_org_other_active_members(
        &self,
        org_id: &str,
        exclude_user_id: &str,
    ) -> anyhow::Result<usize> {
        let conn = self.pool.get()?;
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM org_memberships om \
             JOIN users u ON u.id = om.user_id \
             WHERE om.org_id=?1 AND om.user_id != ?2 AND u.is_active=1",
            params![org_id, exclude_user_id],
            |row| row.get(0),
        )?;
        Ok(count as usize)
    }

    /// Make all datasets owned by an organisation private.
    pub fn make_org_datasets_private(&self, org_id: &str) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        conn.execute(
            "UPDATE datasets SET visibility='private' WHERE owner_type='organisation' AND owner_id=?1",
            params![org_id],
        )?;
        Ok(())
    }

    /// Remove a user from all organisations and groups.
    pub fn remove_user_from_all_orgs_and_groups(&self, user_id: &str) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        conn.execute(
            "DELETE FROM group_memberships WHERE user_id=?1",
            params![user_id],
        )?;
        conn.execute(
            "DELETE FROM org_memberships WHERE user_id=?1",
            params![user_id],
        )?;
        Ok(())
    }

    // ─── Refresh Token CRUD ──────────────────────────────────────────────────

    pub fn create_refresh_token(
        &self,
        id: &str,
        user_id: &str,
        token_hash: &str,
        expires_at: &str,
        family_id: &str,
    ) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO refresh_tokens (id, user_id, token_hash, expires_at, created_at, family_id) VALUES (?1,?2,?3,?4,?5,?6)",
            params![id, user_id, token_hash, expires_at, now, family_id],
        )?;
        Ok(())
    }

    pub fn get_refresh_token_by_hash(
        &self,
        token_hash: &str,
    ) -> anyhow::Result<Option<RefreshToken>> {
        let conn = self.pool.get()?;
        conn.query_row(
            "SELECT id, user_id, token_hash, expires_at, created_at, revoked, family_id FROM refresh_tokens WHERE token_hash = ?1",
            params![token_hash],
            Self::map_refresh_token_row,
        )
        .optional()
        .map_err(Into::into)
    }

    /// The newest still-valid token in a rotation family, if any. Used by the
    /// refresh handler to absorb benign concurrent-refresh races (multiple
    /// tabs/the session-restore thundering herd replaying the just-rotated cookie):
    /// as long as the session still has a live head, the replay rotates from it
    /// instead of being flagged as token theft.
    pub fn get_active_family_head(&self, family_id: &str) -> anyhow::Result<Option<RefreshToken>> {
        let conn = self.pool.get()?;
        conn.query_row(
            "SELECT id, user_id, token_hash, expires_at, created_at, revoked, family_id \
             FROM refresh_tokens WHERE family_id = ?1 AND revoked = 0 \
             ORDER BY created_at DESC LIMIT 1",
            params![family_id],
            Self::map_refresh_token_row,
        )
        .optional()
        .map_err(Into::into)
    }

    fn map_refresh_token_row(row: &rusqlite::Row) -> rusqlite::Result<RefreshToken> {
        Ok(RefreshToken {
            id: row.get(0)?,
            user_id: row.get(1)?,
            token_hash: row.get(2)?,
            expires_at: row.get(3)?,
            created_at: row.get(4)?,
            revoked: row.get::<_, i32>(5)? != 0,
            family_id: row.get(6)?,
        })
    }

    pub fn revoke_refresh_token(&self, id: &str) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        conn.execute(
            "UPDATE refresh_tokens SET revoked = 1 WHERE id = ?1",
            params![id],
        )?;
        Ok(())
    }

    /// Revoke every token in one rotation family (i.e. one session). This is the
    /// blast radius for refresh-token reuse: a stolen/replayed chain kills only
    /// that session, leaving the user's other browsers/devices logged in.
    pub fn revoke_refresh_token_family(&self, family_id: &str) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        conn.execute(
            "UPDATE refresh_tokens SET revoked = 1 WHERE family_id = ?1",
            params![family_id],
        )?;
        Ok(())
    }

    pub fn revoke_all_user_refresh_tokens(&self, user_id: &str) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        conn.execute(
            "UPDATE refresh_tokens SET revoked = 1 WHERE user_id = ?1",
            params![user_id],
        )?;
        Ok(())
    }

    // ─── API Token CRUD ──────────────────────────────────────────────────────

    #[allow(clippy::too_many_arguments)]
    pub fn create_api_token(
        &self,
        id: &str,
        user_id: &str,
        name: &str,
        token_hash: &str,
        token_prefix: &str,
        scopes: &[ApiScope],
        expires_at: Option<&str>,
    ) -> anyhow::Result<ApiToken> {
        let conn = self.pool.get()?;
        let now = chrono::Utc::now().to_rfc3339();
        let scopes_str = ApiScope::scopes_to_string(scopes);
        conn.execute(
            "INSERT INTO api_tokens (id, user_id, name, token_hash, token_prefix, scopes, expires_at, created_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            params![id, user_id, name, token_hash, token_prefix, scopes_str, expires_at, now],
        )?;
        Ok(ApiToken {
            id: id.to_string(),
            user_id: user_id.to_string(),
            name: name.to_string(),
            token_hash: token_hash.to_string(),
            token_prefix: token_prefix.to_string(),
            scopes: scopes.to_vec(),
            expires_at: expires_at.map(String::from),
            last_used_at: None,
            created_at: now,
            revoked: false,
        })
    }

    pub fn list_api_tokens(&self, user_id: &str) -> anyhow::Result<Vec<ApiToken>> {
        let conn = self.pool.get()?;
        let mut stmt = conn.prepare(
            "SELECT id, user_id, name, token_hash, token_prefix, scopes, expires_at, last_used_at, created_at, revoked
             FROM api_tokens WHERE user_id = ?1 ORDER BY created_at DESC",
        )?;
        let tokens = stmt
            .query_map(params![user_id], |row| {
                let scopes_str: String = row.get(5)?;
                Ok(ApiToken {
                    id: row.get(0)?,
                    user_id: row.get(1)?,
                    name: row.get(2)?,
                    token_hash: row.get(3)?,
                    token_prefix: row.get(4)?,
                    scopes: ApiScope::parse_scopes(&scopes_str),
                    expires_at: row.get(6)?,
                    last_used_at: row.get(7)?,
                    created_at: row.get(8)?,
                    revoked: row.get::<_, i32>(9)? != 0,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(tokens)
    }

    pub fn get_api_token_by_hash(&self, token_hash: &str) -> anyhow::Result<Option<ApiToken>> {
        let conn = self.pool.get()?;
        conn.query_row(
            "SELECT id, user_id, name, token_hash, token_prefix, scopes, expires_at, last_used_at, created_at, revoked
             FROM api_tokens WHERE token_hash = ?1",
            params![token_hash],
            |row| {
                let scopes_str: String = row.get(5)?;
                Ok(ApiToken {
                    id: row.get(0)?,
                    user_id: row.get(1)?,
                    name: row.get(2)?,
                    token_hash: row.get(3)?,
                    token_prefix: row.get(4)?,
                    scopes: ApiScope::parse_scopes(&scopes_str),
                    expires_at: row.get(6)?,
                    last_used_at: row.get(7)?,
                    created_at: row.get(8)?,
                    revoked: row.get::<_, i32>(9)? != 0,
                })
            },
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn get_api_token_by_id(&self, id: &str) -> anyhow::Result<Option<ApiToken>> {
        let conn = self.pool.get()?;
        conn.query_row(
            "SELECT id, user_id, name, token_hash, token_prefix, scopes, expires_at, last_used_at, created_at, revoked
             FROM api_tokens WHERE id = ?1",
            params![id],
            |row| {
                let scopes_str: String = row.get(5)?;
                Ok(ApiToken {
                    id: row.get(0)?,
                    user_id: row.get(1)?,
                    name: row.get(2)?,
                    token_hash: row.get(3)?,
                    token_prefix: row.get(4)?,
                    scopes: ApiScope::parse_scopes(&scopes_str),
                    expires_at: row.get(6)?,
                    last_used_at: row.get(7)?,
                    created_at: row.get(8)?,
                    revoked: row.get::<_, i32>(9)? != 0,
                })
            },
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn revoke_api_token(&self, id: &str) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        conn.execute(
            "UPDATE api_tokens SET revoked = 1 WHERE id = ?1",
            params![id],
        )?;
        Ok(())
    }

    pub fn revoke_all_user_api_tokens(&self, user_id: &str) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        conn.execute(
            "UPDATE api_tokens SET revoked = 1 WHERE user_id = ?1",
            params![user_id],
        )?;
        Ok(())
    }

    pub fn update_api_token_last_used(&self, id: &str) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE api_tokens SET last_used_at = ?1 WHERE id = ?2",
            params![now, id],
        )?;
        Ok(())
    }

    // ─── Organisation CRUD ────────────────────────────────────────────────────

    pub fn create_organisation(
        &self,
        id: &str,
        name: &str,
        slug: &str,
        description: Option<&str>,
        parent_org_id: Option<&str>,
    ) -> anyhow::Result<Organisation> {
        let conn = self.pool.get()?;
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO organisations (id, name, slug, description, created_at, parent_org_id) VALUES (?1,?2,?3,?4,?5,?6)",
            params![id, name, slug, description, now, parent_org_id],
        )?;
        Ok(Organisation {
            id: id.to_string(),
            name: name.to_string(),
            slug: slug.to_string(),
            description: description.map(String::from),
            image_key: None,
            banner_key: None,
            created_at: now,
            homepage: None,
            identifier: None,
            contact_name: None,
            contact_email: None,
            contact_url: None,
            org_type: Some("FormalOrganization".to_string()),
            parent_org_id: parent_org_id.map(String::from),
        })
    }

    pub fn get_organisation(&self, id: &str) -> anyhow::Result<Option<Organisation>> {
        let conn = self.pool.get()?;
        conn.query_row(
            "SELECT id, name, slug, description, created_at, image_key, homepage, identifier, contact_name, contact_email, contact_url, org_type, parent_org_id, banner_key FROM organisations WHERE id = ?1",
            params![id],
            map_org_row,
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn get_organisation_by_slug(&self, slug: &str) -> anyhow::Result<Option<Organisation>> {
        let conn = self.pool.get()?;
        conn.query_row(
            "SELECT id, name, slug, description, created_at, image_key, homepage, identifier, contact_name, contact_email, contact_url, org_type, parent_org_id, banner_key FROM organisations WHERE slug = ?1",
            params![slug],
            map_org_row,
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn list_organisations(&self) -> anyhow::Result<Vec<Organisation>> {
        let conn = self.pool.get()?;
        let mut stmt = conn.prepare(
            "SELECT id, name, slug, description, created_at, image_key, homepage, identifier, contact_name, contact_email, contact_url, org_type, parent_org_id, banner_key FROM organisations ORDER BY name",
        )?;
        let orgs = stmt
            .query_map([], map_org_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(orgs)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn update_organisation(
        &self,
        id: &str,
        name: &str,
        description: Option<&str>,
        homepage: Option<&str>,
        identifier: Option<&str>,
        contact_name: Option<&str>,
        contact_email: Option<&str>,
        contact_url: Option<&str>,
        org_type: Option<&str>,
        parent_org_id: Option<&str>,
    ) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        conn.execute(
            "UPDATE organisations SET name=?1, description=?2, homepage=?3, identifier=?4, contact_name=?5, contact_email=?6, contact_url=?7, org_type=?8, parent_org_id=?9 WHERE id=?10",
            params![name, description, homepage, identifier, contact_name, contact_email, contact_url, org_type, parent_org_id, id],
        )?;
        Ok(())
    }

    /// List the direct child organisations of `parent_id` (`org:hasSubOrganization`).
    pub fn list_child_organisations(&self, parent_id: &str) -> anyhow::Result<Vec<Organisation>> {
        let conn = self.pool.get()?;
        let mut stmt = conn.prepare(
            "SELECT id, name, slug, description, created_at, image_key, homepage, identifier, contact_name, contact_email, contact_url, org_type, parent_org_id, banner_key FROM organisations WHERE parent_org_id = ?1 ORDER BY name",
        )?;
        let orgs = stmt
            .query_map(params![parent_id], map_org_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(orgs)
    }

    /// True if `candidate_ancestor` is `org_id` itself or one of its ancestors.
    /// Used to reject a parent assignment that would create a cycle.
    pub fn is_org_ancestor(&self, candidate_ancestor: &str, org_id: &str) -> anyhow::Result<bool> {
        let mut current = Some(org_id.to_string());
        let mut guard = 0;
        while let Some(cur) = current {
            if cur == candidate_ancestor {
                return Ok(true);
            }
            guard += 1;
            if guard > 256 {
                break; // defensive: stop if existing data already contains a cycle
            }
            current = self
                .get_organisation(&cur)?
                .and_then(|o| o.parent_org_id)
                .filter(|p| !p.is_empty());
        }
        Ok(false)
    }

    pub fn delete_organisation(&self, id: &str) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        conn.execute("DELETE FROM organisations WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn list_user_organisations(&self, user_id: &str) -> anyhow::Result<Vec<Organisation>> {
        let conn = self.pool.get()?;
        let mut stmt = conn.prepare(
            "SELECT o.id, o.name, o.slug, o.description, o.created_at, o.image_key, o.homepage, o.identifier, o.contact_name, o.contact_email, o.contact_url, o.org_type, o.parent_org_id, o.banner_key
             FROM organisations o
             JOIN org_memberships om ON o.id = om.org_id
             WHERE om.user_id = ?1
             ORDER BY o.name",
        )?;
        let orgs = stmt
            .query_map(params![user_id], map_org_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(orgs)
    }

    // ─── Org Memberships ──────────────────────────────────────────────────────

    pub fn add_org_member(&self, user_id: &str, org_id: &str, role: Role) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        conn.execute(
            "INSERT OR REPLACE INTO org_memberships (user_id, org_id, role) VALUES (?1,?2,?3)",
            params![user_id, org_id, role.as_str()],
        )?;
        Ok(())
    }

    pub fn remove_org_member(&self, user_id: &str, org_id: &str) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        conn.execute(
            "DELETE FROM org_memberships WHERE user_id=?1 AND org_id=?2",
            params![user_id, org_id],
        )?;
        Ok(())
    }

    pub fn list_org_members(&self, org_id: &str) -> anyhow::Result<Vec<(User, Role)>> {
        let conn = self.pool.get()?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {}, om.role
                 FROM users u
                 JOIN org_memberships om ON u.id = om.user_id
                 WHERE om.org_id = ?1
                 ORDER BY u.username",
            USER_COLS_U
        ))?;
        let members = stmt
            .query_map(params![org_id], |row| {
                let user = read_user(row)?;
                // Role is appended after the USER_COLS_U columns.
                let role_str: String = row
                    .get::<_, Option<String>>(USER_COLS_LEN)?
                    .unwrap_or_default();
                Ok((user, Role::from_str(&role_str).unwrap_or(Role::Member)))
            })?
            // Skip any row that fails to map rather than 500-ing the whole list.
            .filter_map(|r| r.ok())
            .collect::<Vec<_>>();
        Ok(members)
    }

    pub fn get_org_membership(&self, user_id: &str, org_id: &str) -> anyhow::Result<Option<Role>> {
        let conn = self.pool.get()?;
        let role: Option<String> = conn
            .query_row(
                "SELECT role FROM org_memberships WHERE user_id=?1 AND org_id=?2",
                params![user_id, org_id],
                |row| row.get(0),
            )
            .optional()?;
        Ok(role.and_then(|r| Role::from_str(&r)))
    }

    // ─── Validation reports (Phase 5) ───────────────────────────────────────

    pub fn insert_validation_report(&self, r: &ValidationReportRecord) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        conn.execute(
            "INSERT INTO validation_reports
             (id, dataset_id, version, conforms, report_ttl, data_ref, shapes_ref, source, created_by, created_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![
                r.id, r.dataset_id, r.version, r.conforms as i32, r.report_ttl,
                r.data_ref, r.shapes_ref, r.source, r.created_by, r.created_at
            ],
        )?;
        Ok(())
    }

    /// List a dataset's reports newest-first. `report_ttl` is omitted (empty) here.
    pub fn list_validation_reports(
        &self,
        dataset_id: &str,
    ) -> anyhow::Result<Vec<ValidationReportRecord>> {
        let conn = self.pool.get()?;
        let mut stmt = conn.prepare(
            "SELECT id, dataset_id, version, conforms, data_ref, shapes_ref, source, created_by, created_at
             FROM validation_reports WHERE dataset_id=?1 ORDER BY created_at DESC",
        )?;
        let rows = stmt
            .query_map(params![dataset_id], |row| {
                Ok(ValidationReportRecord {
                    id: row.get(0)?,
                    dataset_id: row.get(1)?,
                    version: row.get(2)?,
                    conforms: row.get::<_, i32>(3)? != 0,
                    report_ttl: String::new(),
                    data_ref: row.get(4)?,
                    shapes_ref: row.get(5)?,
                    source: row.get(6)?,
                    created_by: row.get(7)?,
                    created_at: row.get(8)?,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    }

    pub fn get_validation_report(
        &self,
        dataset_id: &str,
        id: &str,
    ) -> anyhow::Result<Option<ValidationReportRecord>> {
        let conn = self.pool.get()?;
        let rec = conn
            .query_row(
                "SELECT id, dataset_id, version, conforms, report_ttl, data_ref, shapes_ref, source, created_by, created_at
                 FROM validation_reports WHERE dataset_id=?1 AND id=?2",
                params![dataset_id, id],
                |row| {
                    Ok(ValidationReportRecord {
                        id: row.get(0)?,
                        dataset_id: row.get(1)?,
                        version: row.get(2)?,
                        conforms: row.get::<_, i32>(3)? != 0,
                        report_ttl: row.get(4)?,
                        data_ref: row.get(5)?,
                        shapes_ref: row.get(6)?,
                        source: row.get(7)?,
                        created_by: row.get(8)?,
                        created_at: row.get(9)?,
                    })
                },
            )
            .optional()?;
        Ok(rec)
    }

    // ─── Share links (Phase 6) ──────────────────────────────────────────────

    pub fn insert_share_link(&self, s: &ShareLink) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        conn.execute(
            "INSERT INTO share_links
             (id, token_hash, dataset_id, graph, permission, label, created_by, expires_at, revoked, created_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![
                s.id, s.token_hash, s.dataset_id, s.graph, s.permission,
                s.label, s.created_by, s.expires_at, s.revoked as i32, s.created_at
            ],
        )?;
        Ok(())
    }

    pub fn get_share_link_by_token_hash(
        &self,
        token_hash: &str,
    ) -> anyhow::Result<Option<ShareLink>> {
        let conn = self.pool.get()?;
        let rec = conn
            .query_row(
                "SELECT id, token_hash, dataset_id, graph, permission, label, created_by, expires_at, revoked, created_at
                 FROM share_links WHERE token_hash=?1",
                params![token_hash],
                |row| {
                    Ok(ShareLink {
                        id: row.get(0)?,
                        token_hash: row.get(1)?,
                        dataset_id: row.get(2)?,
                        graph: row.get(3)?,
                        permission: row.get(4)?,
                        label: row.get(5)?,
                        created_by: row.get(6)?,
                        expires_at: row.get(7)?,
                        revoked: row.get::<_, i32>(8)? != 0,
                        created_at: row.get(9)?,
                    })
                },
            )
            .optional()?;
        Ok(rec)
    }

    // ─── Group CRUD ───────────────────────────────────────────────────────────

    pub fn create_group(
        &self,
        id: &str,
        org_id: &str,
        name: &str,
        parent_group_id: Option<&str>,
    ) -> anyhow::Result<Group> {
        let conn = self.pool.get()?;
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO groups (id, org_id, name, parent_group_id, created_at) VALUES (?1,?2,?3,?4,?5)",
            params![id, org_id, name, parent_group_id, now],
        )?;
        Ok(Group {
            id: id.to_string(),
            org_id: org_id.to_string(),
            name: name.to_string(),
            parent_group_id: parent_group_id.map(String::from),
            created_at: now,
        })
    }

    pub fn get_group(&self, id: &str) -> anyhow::Result<Option<Group>> {
        let conn = self.pool.get()?;
        conn.query_row(
            "SELECT id, org_id, name, parent_group_id, created_at FROM groups WHERE id = ?1",
            params![id],
            |row| {
                Ok(Group {
                    id: row.get(0)?,
                    org_id: row.get(1)?,
                    name: row.get(2)?,
                    parent_group_id: row.get(3)?,
                    created_at: row.get(4)?,
                })
            },
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn list_org_groups(&self, org_id: &str) -> anyhow::Result<Vec<Group>> {
        let conn = self.pool.get()?;
        let mut stmt = conn.prepare(
            "SELECT id, org_id, name, parent_group_id, created_at FROM groups WHERE org_id=?1 ORDER BY name",
        )?;
        let groups = stmt
            .query_map(params![org_id], |row| {
                Ok(Group {
                    id: row.get(0)?,
                    org_id: row.get(1)?,
                    name: row.get(2)?,
                    parent_group_id: row.get(3)?,
                    created_at: row.get(4)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(groups)
    }

    pub fn update_group(
        &self,
        id: &str,
        name: &str,
        parent_group_id: Option<&str>,
    ) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        conn.execute(
            "UPDATE groups SET name=?1, parent_group_id=?2 WHERE id=?3",
            params![name, parent_group_id, id],
        )?;
        Ok(())
    }

    pub fn delete_group(&self, id: &str) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        conn.execute("DELETE FROM groups WHERE id = ?1", params![id])?;
        Ok(())
    }

    // ─── Group Memberships ────────────────────────────────────────────────────

    pub fn add_group_member(
        &self,
        user_id: &str,
        group_id: &str,
        role: Role,
    ) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        conn.execute(
            "INSERT OR REPLACE INTO group_memberships (user_id, group_id, role) VALUES (?1,?2,?3)",
            params![user_id, group_id, role.as_str()],
        )?;
        Ok(())
    }

    pub fn remove_group_member(&self, user_id: &str, group_id: &str) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        conn.execute(
            "DELETE FROM group_memberships WHERE user_id=?1 AND group_id=?2",
            params![user_id, group_id],
        )?;
        Ok(())
    }

    pub fn list_group_members(&self, group_id: &str) -> anyhow::Result<Vec<(User, Role)>> {
        let conn = self.pool.get()?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {}, gm.role
                 FROM users u
                 JOIN group_memberships gm ON u.id = gm.user_id
                 WHERE gm.group_id = ?1
                 ORDER BY u.username",
            USER_COLS_U
        ))?;
        let members = stmt
            .query_map(params![group_id], |row| {
                let user = read_user(row)?;
                // Role is appended after the USER_COLS_U columns.
                let role_str: String = row
                    .get::<_, Option<String>>(USER_COLS_LEN)?
                    .unwrap_or_default();
                Ok((user, Role::from_str(&role_str).unwrap_or(Role::Member)))
            })?
            .filter_map(|r| r.ok())
            .collect::<Vec<_>>();
        Ok(members)
    }

    pub fn get_group_membership(
        &self,
        user_id: &str,
        group_id: &str,
    ) -> anyhow::Result<Option<Role>> {
        let conn = self.pool.get()?;
        let role: Option<String> = conn
            .query_row(
                "SELECT role FROM group_memberships WHERE user_id=?1 AND group_id=?2",
                params![user_id, group_id],
                |row| row.get(0),
            )
            .optional()?;
        Ok(role.and_then(|r| Role::from_str(&r)))
    }

    pub fn list_user_groups(&self, user_id: &str) -> anyhow::Result<Vec<Group>> {
        let conn = self.pool.get()?;
        let mut stmt = conn.prepare(
            "SELECT g.id, g.org_id, g.name, g.parent_group_id, g.created_at
             FROM groups g
             JOIN group_memberships gm ON g.id = gm.group_id
             WHERE gm.user_id = ?1
             ORDER BY g.name",
        )?;
        let groups = stmt
            .query_map(params![user_id], |row| {
                Ok(Group {
                    id: row.get(0)?,
                    org_id: row.get(1)?,
                    name: row.get(2)?,
                    parent_group_id: row.get(3)?,
                    created_at: row.get(4)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(groups)
    }

    // ─── Dataset CRUD ─────────────────────────────────────────────────────────

    #[allow(clippy::too_many_arguments)] // flat dataset columns; a struct adds churn
    pub fn create_dataset(
        &self,
        id: &str,
        name: &str,
        description: Option<&str>,
        owner_type: OwnerType,
        owner_id: &str,
        visibility: Visibility,
        graph_role: Option<GraphKind>,
    ) -> anyhow::Result<Dataset> {
        let conn = self.pool.get()?;
        let now = chrono::Utc::now().to_rfc3339();
        let role_str = graph_role.map(|r| r.as_str().to_string());
        conn.execute(
            "INSERT INTO datasets (id, name, description, owner_type, owner_id, visibility, shacl_on_write, graph_role, created_at, updated_at) VALUES (?1,?2,?3,?4,?5,?6,0,?7,?8,?9)",
            params![id, name, description, owner_type.as_str(), owner_id, visibility.as_str(), role_str, now, now],
        )?;
        Ok(Dataset {
            id: id.to_string(),
            name: name.to_string(),
            description: description.map(String::from),
            owner_type,
            owner_id: owner_id.to_string(),
            visibility,
            shacl_on_write: false,
            shapes_graph_iri: None,
            conforms_to_model: None,
            conforms_to_version: None,
            image_key: None,
            banner_key: None,
            graph_role,
            created_at: now.clone(),
            updated_at: now,
            license: None,
            themes: None,
            keywords: None,
            contact_name: None,
            contact_email: None,
            contact_url: None,
            adms_status: None,
            version_notes: None,
            spatial: None,
            landing_page: None,
        })
    }

    pub fn get_dataset(&self, id: &str) -> anyhow::Result<Option<Dataset>> {
        let conn = self.pool.get()?;
        conn.query_row(
            "SELECT id, name, description, owner_type, owner_id, visibility, shacl_on_write, shapes_graph_iri, conforms_to_model, conforms_to_version, image_key, graph_role, created_at, updated_at, license, themes, keywords, contact_name, contact_email, contact_url, adms_status, version_notes, spatial, landing_page, banner_key FROM datasets WHERE id = ?1",
            params![id],
            read_dataset_row,
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn list_datasets(&self) -> anyhow::Result<Vec<Dataset>> {
        let conn = self.pool.get()?;
        let mut stmt = conn.prepare(
            "SELECT id, name, description, owner_type, owner_id, visibility, shacl_on_write, shapes_graph_iri, conforms_to_model, conforms_to_version, image_key, graph_role, created_at, updated_at, license, themes, keywords, contact_name, contact_email, contact_url, adms_status, version_notes, spatial, landing_page, banner_key FROM datasets ORDER BY name",
        )?;
        let datasets = stmt
            .query_map([], read_dataset_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(datasets)
    }

    pub fn list_datasets_by_org(&self, org_id: &str) -> anyhow::Result<Vec<Dataset>> {
        let conn = self.pool.get()?;
        let mut stmt = conn.prepare(
            "SELECT id, name, description, owner_type, owner_id, visibility, shacl_on_write, shapes_graph_iri, conforms_to_model, conforms_to_version, image_key, graph_role, created_at, updated_at, license, themes, keywords, contact_name, contact_email, contact_url, adms_status, version_notes, spatial, landing_page, banner_key FROM datasets WHERE owner_type='organisation' AND owner_id = ?1 ORDER BY name",
        )?;
        let datasets = stmt
            .query_map(params![org_id], read_dataset_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(datasets)
    }

    /// Find the dataset that owns a given graph IRI (via dataset_graphs table).
    pub fn find_dataset_by_graph_iri(&self, graph_iri: &str) -> anyhow::Result<Option<Dataset>> {
        let conn = self.pool.get()?;
        conn.query_row(
            "SELECT d.id, d.name, d.description, d.owner_type, d.owner_id, d.visibility, d.shacl_on_write, d.shapes_graph_iri, d.conforms_to_model, d.conforms_to_version, d.image_key, d.graph_role, d.created_at, d.updated_at, d.license, d.themes, d.keywords, d.contact_name, d.contact_email, d.contact_url, d.adms_status, d.version_notes, d.spatial, d.landing_page, d.banner_key
             FROM datasets d JOIN dataset_graphs dg ON d.id = dg.dataset_id
             WHERE dg.graph_iri = ?1 LIMIT 1",
            params![graph_iri],
            read_dataset_row,
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn update_dataset(
        &self,
        id: &str,
        name: &str,
        description: Option<&str>,
        visibility: Visibility,
    ) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE datasets SET name=?1, description=?2, visibility=?3, updated_at=?4 WHERE id=?5",
            params![name, description, visibility.as_str(), now, id],
        )?;
        Ok(())
    }

    pub fn update_dataset_conformance(
        &self,
        id: &str,
        conforms_to_model: Option<&str>,
        conforms_to_version: Option<&str>,
    ) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE datasets SET conforms_to_model=?1, conforms_to_version=?2, updated_at=?3 WHERE id=?4",
            params![conforms_to_model, conforms_to_version, now, id],
        )?;
        Ok(())
    }

    pub fn update_dataset_shacl(
        &self,
        id: &str,
        shacl_on_write: bool,
        shapes_graph_iri: Option<&str>,
    ) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE datasets SET shacl_on_write=?1, shapes_graph_iri=?2, updated_at=?3 WHERE id=?4",
            params![shacl_on_write as i32, shapes_graph_iri, now, id],
        )?;
        Ok(())
    }

    // ─── SHACL validation run history ──────────────────────────────────────────

    /// Persist a validation run and prune to the most recent 50 runs per dataset.
    #[allow(clippy::too_many_arguments)]
    pub fn insert_validation_run(
        &self,
        dataset_id: &str,
        conforms: bool,
        results_count: i64,
        violation_count: i64,
        warning_count: i64,
        info_count: i64,
        report_json: &str,
        triggered_by: Option<&str>,
    ) -> anyhow::Result<ShaclRunSummary> {
        let conn = self.pool.get()?;
        let id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO shacl_validation_runs (id, dataset_id, run_timestamp, conforms, results_count, violation_count, warning_count, info_count, report_json, triggered_by, created_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?3)",
            params![id, dataset_id, now, conforms as i32, results_count, violation_count, warning_count, info_count, report_json, triggered_by],
        )?;
        conn.execute(
            "DELETE FROM shacl_validation_runs WHERE dataset_id = ?1 AND id NOT IN (
                SELECT id FROM shacl_validation_runs WHERE dataset_id = ?1 ORDER BY run_timestamp DESC LIMIT 50
            )",
            params![dataset_id],
        )?;
        Ok(ShaclRunSummary {
            id,
            dataset_id: dataset_id.to_string(),
            run_timestamp: now,
            conforms,
            results_count,
            violation_count,
            warning_count,
            info_count,
            triggered_by: triggered_by.map(|s| s.to_string()),
        })
    }

    // ── Private dataset usage telemetry ──────────────────────────────────
    //
    // `action` is a short verb: "view" | "validate" | "pipeline". Recording is
    // best-effort — callers ignore the Result so telemetry never breaks a real
    // request. Reads are deliberately scoped: `dataset_usage_for_user` returns
    // only one user's own footprint (used by the validate overview ranking),
    // while `dataset_usage_all` returns the cross-user aggregate and must only
    // be exposed behind a super_admin check.

    /// Append a usage event. `user_id` is `None` for anonymous access.
    pub fn record_dataset_usage(
        &self,
        dataset_id: &str,
        user_id: Option<&str>,
        action: &str,
    ) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        conn.execute(
            "INSERT INTO dataset_usage_events (id, dataset_id, user_id, action, used_at) VALUES (?1,?2,?3,?4,?5)",
            params![
                uuid::Uuid::new_v4().to_string(),
                dataset_id,
                user_id,
                action,
                chrono::Utc::now().to_rfc3339()
            ],
        )?;
        Ok(())
    }

    /// One user's own usage, aggregated per dataset (count + most recent),
    /// ordered most-used first. This is the caller reading their own footprint.
    pub fn dataset_usage_for_user(&self, user_id: &str) -> anyhow::Result<Vec<DatasetUsageStat>> {
        let conn = self.pool.get()?;
        let mut stmt = conn.prepare(
            "SELECT dataset_id, COUNT(*) AS use_count, MAX(used_at) AS last_used
             FROM dataset_usage_events
             WHERE user_id = ?1
             GROUP BY dataset_id
             ORDER BY use_count DESC, last_used DESC",
        )?;
        let rows = stmt.query_map(params![user_id], |row| {
            Ok(DatasetUsageStat {
                dataset_id: row.get(0)?,
                user_id: None,
                use_count: row.get(1)?,
                last_used: row.get(2)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Cross-user aggregate, per (dataset, user). super_admin only — callers
    /// MUST gate on the SuperAdmin role before invoking this. `since` is an
    /// optional RFC3339 lower bound on `used_at`.
    pub fn dataset_usage_all(
        &self,
        since: Option<&str>,
        limit: i64,
    ) -> anyhow::Result<Vec<DatasetUsageStat>> {
        let conn = self.pool.get()?;
        let mut sql = String::from(
            "SELECT dataset_id, user_id, COUNT(*) AS use_count, MAX(used_at) AS last_used
             FROM dataset_usage_events WHERE 1=1",
        );
        let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        if let Some(s) = since {
            sql.push_str(" AND used_at >= ?");
            args.push(Box::new(s.to_string()));
        }
        sql.push_str(
            " GROUP BY dataset_id, user_id ORDER BY use_count DESC, last_used DESC LIMIT ?",
        );
        args.push(Box::new(limit));
        let mut stmt = conn.prepare(&sql)?;
        let params_ref: Vec<&dyn rusqlite::ToSql> = args.iter().map(|b| b.as_ref()).collect();
        let rows = stmt.query_map(params_ref.as_slice(), |row| {
            Ok(DatasetUsageStat {
                dataset_id: row.get(0)?,
                user_id: row.get(1)?,
                use_count: row.get(2)?,
                last_used: row.get(3)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Latest stored run (with full report) for a dataset, if any.
    pub fn get_latest_validation_run(
        &self,
        dataset_id: &str,
    ) -> anyhow::Result<Option<ShaclValidationRun>> {
        let conn = self.pool.get()?;
        let row = conn.query_row(
            "SELECT id, dataset_id, run_timestamp, conforms, results_count, violation_count, warning_count, info_count, report_json, triggered_by, created_at FROM shacl_validation_runs WHERE dataset_id = ?1 ORDER BY run_timestamp DESC LIMIT 1",
            params![dataset_id],
            map_run_row,
        ).optional()?;
        row.map(parse_run_row).transpose()
    }

    /// One stored run (with full report) by run id.
    pub fn get_validation_run(&self, run_id: &str) -> anyhow::Result<Option<ShaclValidationRun>> {
        let conn = self.pool.get()?;
        let row = conn.query_row(
            "SELECT id, dataset_id, run_timestamp, conforms, results_count, violation_count, warning_count, info_count, report_json, triggered_by, created_at FROM shacl_validation_runs WHERE id = ?1",
            params![run_id],
            map_run_row,
        ).optional()?;
        row.map(parse_run_row).transpose()
    }

    /// History (newest first) of run summaries for a dataset.
    pub fn list_validation_run_summaries(
        &self,
        dataset_id: &str,
        limit: i64,
    ) -> anyhow::Result<Vec<ShaclRunSummary>> {
        let conn = self.pool.get()?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {RUN_SUMMARY_COLS} FROM shacl_validation_runs WHERE dataset_id = ?1 ORDER BY run_timestamp DESC LIMIT ?2"
        ))?;
        let runs = stmt
            .query_map(params![dataset_id, limit], read_run_summary)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(runs)
    }

    /// Latest run summary per dataset, for the requested dataset ids.
    pub fn list_latest_run_summaries(
        &self,
        dataset_ids: &[String],
    ) -> anyhow::Result<Vec<ShaclRunSummary>> {
        if dataset_ids.is_empty() {
            return Ok(vec![]);
        }
        let conn = self.pool.get()?;
        let placeholders = dataset_ids
            .iter()
            .map(|_| "?")
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT {RUN_SUMMARY_COLS} FROM shacl_validation_runs r
             WHERE dataset_id IN ({placeholders})
               AND run_timestamp = (SELECT MAX(run_timestamp) FROM shacl_validation_runs WHERE dataset_id = r.dataset_id)"
        );
        let mut stmt = conn.prepare(&sql)?;
        let runs = stmt
            .query_map(
                rusqlite::params_from_iter(dataset_ids.iter()),
                read_run_summary,
            )?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(runs)
    }

    pub fn update_dataset_role(
        &self,
        id: &str,
        graph_role: Option<GraphKind>,
    ) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE datasets SET graph_role=?1, updated_at=?2 WHERE id=?3",
            params![graph_role.map(|r| r.as_str()), now, id],
        )?;
        Ok(())
    }

    /// Bulk-set every `dataset_graphs` row owned by `dataset_id` to the given
    /// graph role (or NULL). Used when "Convert to Model/Vocabulary/Shapes"
    /// retags a whole dataset — the per-graph role would otherwise stay stale
    /// and downstream views (model browser, vocab browser) keep using the old role.
    pub fn update_dataset_graphs_role(
        &self,
        dataset_id: &str,
        graph_role: Option<GraphKind>,
    ) -> anyhow::Result<usize> {
        let conn = self.pool.get()?;
        let count = conn.execute(
            "UPDATE dataset_graphs SET graph_role=?1 WHERE dataset_id=?2",
            params![graph_role.map(|r| r.as_str()), dataset_id],
        )?;
        Ok(count)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn update_dataset_metadata(
        &self,
        id: &str,
        license: Option<&str>,
        themes: Option<&str>,
        keywords: Option<&str>,
        contact_name: Option<&str>,
        contact_email: Option<&str>,
        contact_url: Option<&str>,
        adms_status: Option<&str>,
        version_notes: Option<&str>,
        spatial: Option<&str>,
        landing_page: Option<&str>,
    ) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE datasets SET license=?1, themes=?2, keywords=?3, contact_name=?4, contact_email=?5, contact_url=?6, adms_status=?7, version_notes=?8, spatial=?9, landing_page=?10, updated_at=?11 WHERE id=?12",
            params![license, themes, keywords, contact_name, contact_email, contact_url, adms_status, version_notes, spatial, landing_page, now, id],
        )?;
        Ok(())
    }

    pub fn delete_dataset(&self, id: &str) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        conn.execute("DELETE FROM datasets WHERE id = ?1", params![id])?;
        self.invalidate_accessible_graphs_cache();
        Ok(())
    }

    /// Return the IDs of all datasets owned by the given owner (user or org).
    pub fn list_dataset_ids_by_owner(&self, owner_id: &str) -> anyhow::Result<Vec<String>> {
        let conn = self.pool.get()?;
        let mut stmt = conn.prepare("SELECT id FROM datasets WHERE owner_id = ?1")?;
        let ids = stmt
            .query_map(params![owner_id], |row| row.get(0))?
            .collect::<Result<Vec<String>, _>>()?;
        Ok(ids)
    }

    // ─── Dataset Private Access ───────────────────────────────────────────────

    pub fn grant_dataset_access(&self, dataset_id: &str, user_id: &str) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        conn.execute(
            "INSERT OR IGNORE INTO dataset_private_access (dataset_id, user_id) VALUES (?1,?2)",
            params![dataset_id, user_id],
        )?;
        self.invalidate_accessible_graphs_cache();
        Ok(())
    }

    pub fn revoke_dataset_access(&self, dataset_id: &str, user_id: &str) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        conn.execute(
            "DELETE FROM dataset_private_access WHERE dataset_id=?1 AND user_id=?2",
            params![dataset_id, user_id],
        )?;
        self.invalidate_accessible_graphs_cache();
        Ok(())
    }

    pub fn has_dataset_access(&self, dataset_id: &str, user_id: &str) -> anyhow::Result<bool> {
        let conn = self.pool.get()?;
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM dataset_private_access WHERE dataset_id=?1 AND user_id=?2",
            params![dataset_id, user_id],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    pub fn list_dataset_access_users(&self, dataset_id: &str) -> anyhow::Result<Vec<User>> {
        let conn = self.pool.get()?;
        let mut stmt = conn.prepare(&format!(
            "SELECT u.{}
                 FROM users u
                 JOIN dataset_private_access dpa ON u.id = dpa.user_id
                 WHERE dpa.dataset_id = ?1
                 ORDER BY u.username",
            USER_COLS
        ))?;
        let users = stmt
            .query_map(params![dataset_id], read_user)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(users)
    }

    // ─── Per-resource role grants ───────────────────────────────────────────────

    /// Insert or update a per-resource grant (one role per principal per
    /// resource). Returns the resulting grant row.
    pub fn set_resource_grant(
        &self,
        resource_type: &str,
        resource_id: &str,
        principal_type: &str,
        principal_id: &str,
        role: ResourceRole,
        created_by: &str,
    ) -> anyhow::Result<ResourceGrant> {
        let id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        {
            let conn = self.pool.get()?;
            conn.execute(
                "INSERT INTO resource_access
                    (id, resource_type, resource_id, principal_type, principal_id, role, created_at, created_by)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8)
                 ON CONFLICT(resource_type, resource_id, principal_type, principal_id)
                 DO UPDATE SET role = excluded.role",
                params![id, resource_type, resource_id, principal_type, principal_id, role.as_str(), now, created_by],
            )?;
        }
        self.invalidate_accessible_graphs_cache();
        self.get_resource_grant(resource_type, resource_id, principal_type, principal_id)?
            .ok_or_else(|| anyhow::anyhow!("resource grant missing after upsert"))
    }

    pub fn get_resource_grant(
        &self,
        resource_type: &str,
        resource_id: &str,
        principal_type: &str,
        principal_id: &str,
    ) -> anyhow::Result<Option<ResourceGrant>> {
        let conn = self.pool.get()?;
        let grant = conn
            .query_row(
                "SELECT id, resource_type, resource_id, principal_type, principal_id, role, created_at, created_by
                 FROM resource_access
                 WHERE resource_type=?1 AND resource_id=?2 AND principal_type=?3 AND principal_id=?4",
                params![resource_type, resource_id, principal_type, principal_id],
                read_resource_grant,
            )
            .optional()?;
        Ok(grant)
    }

    pub fn revoke_resource_grant(
        &self,
        resource_type: &str,
        resource_id: &str,
        principal_type: &str,
        principal_id: &str,
    ) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        conn.execute(
            "DELETE FROM resource_access
             WHERE resource_type=?1 AND resource_id=?2 AND principal_type=?3 AND principal_id=?4",
            params![resource_type, resource_id, principal_type, principal_id],
        )?;
        self.invalidate_accessible_graphs_cache();
        Ok(())
    }

    /// List all explicit grants on a resource (ordered for stable display).
    pub fn list_resource_grants(
        &self,
        resource_type: &str,
        resource_id: &str,
    ) -> anyhow::Result<Vec<ResourceGrant>> {
        let conn = self.pool.get()?;
        let mut stmt = conn.prepare(
            "SELECT id, resource_type, resource_id, principal_type, principal_id, role, created_at, created_by
             FROM resource_access
             WHERE resource_type=?1 AND resource_id=?2
             ORDER BY principal_type, principal_id",
        )?;
        let grants = stmt
            .query_map(params![resource_type, resource_id], read_resource_grant)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(grants)
    }

    /// Remove every grant on a resource (call when the resource is deleted).
    pub fn delete_resource_grants(
        &self,
        resource_type: &str,
        resource_id: &str,
    ) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        conn.execute(
            "DELETE FROM resource_access WHERE resource_type=?1 AND resource_id=?2",
            params![resource_type, resource_id],
        )?;
        self.invalidate_accessible_graphs_cache();
        Ok(())
    }

    /// The strongest explicit grant a user holds on a resource — via a direct
    /// user grant, any group the user belongs to, or any organisation the user
    /// is a member of.
    pub fn granted_resource_role(
        &self,
        user_id: &str,
        resource_type: &str,
        resource_id: &str,
    ) -> anyhow::Result<Option<ResourceRole>> {
        let conn = self.pool.get()?;
        let mut stmt = conn.prepare(
            "SELECT role FROM resource_access
             WHERE resource_type=?1 AND resource_id=?2
               AND ( (principal_type='user' AND principal_id=?3)
                  OR (principal_type='group' AND principal_id IN (
                        SELECT group_id FROM group_memberships WHERE user_id=?3))
                  OR (principal_type='organisation' AND principal_id IN (
                        SELECT org_id FROM org_memberships WHERE user_id=?3)) )",
        )?;
        let roles = stmt
            .query_map(params![resource_type, resource_id, user_id], |r| {
                r.get::<_, String>(0)
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(roles.iter().filter_map(|s| ResourceRole::from_str(s)).max())
    }

    // ─── Effective per-resource role resolution ─────────────────────────────────

    /// Compute the effective role a user holds on a dataset, or `None` for no
    /// access. Combines: system admin, ownership, org/group membership (gated
    /// by visibility), explicit per-resource grants, the legacy private-access
    /// allow-list, and public readability — taking the strongest.
    pub fn effective_dataset_role(
        &self,
        user_id: Option<&str>,
        dataset: &Dataset,
    ) -> anyhow::Result<Option<ResourceRole>> {
        let user_id = match user_id {
            Some(id) => id,
            None => {
                return Ok(if dataset.visibility == Visibility::Public {
                    Some(ResourceRole::Viewer)
                } else {
                    None
                });
            }
        };

        if let Some(user) = self.get_user_by_id(user_id)? {
            if user.role.is_admin() {
                return Ok(Some(ResourceRole::Admin));
            }
        }

        // The owning user always manages their own dataset; grants cannot
        // restrict them.
        if dataset.owner_type == OwnerType::User && dataset.owner_id == user_id {
            return Ok(Some(ResourceRole::Admin));
        }

        let membership = self.membership_role_for_dataset(user_id, dataset)?;
        let grant = self.granted_resource_role(user_id, "dataset", &dataset.id)?;
        let mut best = combine_membership_and_grant(membership, grant);

        // Legacy private-access allow-list grants read.
        if best.is_none()
            && dataset.visibility == Visibility::Private
            && self.has_dataset_access(&dataset.id, user_id)?
        {
            best = Some(ResourceRole::Viewer);
        }

        // Public datasets are always at least readable.
        if best.is_none() && dataset.visibility == Visibility::Public {
            best = Some(ResourceRole::Viewer);
        }

        Ok(best)
    }

    /// Ownership/membership-derived role for a dataset, gated by visibility.
    fn membership_role_for_dataset(
        &self,
        user_id: &str,
        dataset: &Dataset,
    ) -> anyhow::Result<Option<ResourceRole>> {
        match dataset.owner_type {
            OwnerType::User => Ok(if dataset.owner_id == user_id {
                Some(ResourceRole::Admin)
            } else {
                None
            }),
            OwnerType::Organisation => {
                let role = self.get_org_membership(user_id, &dataset.owner_id)?;
                Ok(scope_membership_role(role, dataset.visibility))
            }
            OwnerType::Group => {
                let group_role = self.get_group_membership(user_id, &dataset.owner_id)?;
                let mut best = scope_membership_role(group_role, dataset.visibility);
                if let Some(group) = self.get_group(&dataset.owner_id)? {
                    let org_role = self.get_org_membership(user_id, &group.org_id)?;
                    best = stronger(best, scope_membership_role(org_role, dataset.visibility));
                }
                Ok(best)
            }
        }
    }

    /// Whether `user_id` may create/own a resource under the owner
    /// (`owner_type`, `owner_id`): the user themselves, or an organisation/group
    /// they belong to (directly, or via the group's parent org). Platform admins
    /// bypass this at the call site. Prevents forging `owner_id` to impersonate
    /// another user or attribute a resource to a foreign org/group.
    pub fn can_act_as_owner(
        &self,
        user_id: &str,
        owner_type: OwnerType,
        owner_id: &str,
    ) -> anyhow::Result<bool> {
        match owner_type {
            OwnerType::User => Ok(owner_id == user_id),
            OwnerType::Organisation => Ok(self.get_org_membership(user_id, owner_id)?.is_some()),
            OwnerType::Group => {
                if self.get_group_membership(user_id, owner_id)?.is_some() {
                    return Ok(true);
                }
                if let Some(group) = self.get_group(owner_id)? {
                    if self.get_org_membership(user_id, &group.org_id)?.is_some() {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
        }
    }

    /// Compute the effective role a user holds on an ontology (data-model or
    /// vocabulary), or `None` for no access. Org members get a role derived
    /// from their membership; explicit per-resource grants and public
    /// readability are layered on top.
    pub fn effective_ontology_role(
        &self,
        user_id: Option<&str>,
        resource_type: &str,
        resource_id: &str,
        is_public: bool,
        owner_type: Option<&str>,
        owner_id: Option<&str>,
    ) -> anyhow::Result<Option<ResourceRole>> {
        let user_id = match user_id {
            Some(id) => id,
            None => {
                return Ok(if is_public {
                    Some(ResourceRole::Viewer)
                } else {
                    None
                })
            }
        };

        if let Some(user) = self.get_user_by_id(user_id)? {
            if user.role.is_admin() {
                return Ok(Some(ResourceRole::Admin));
            }
        }

        // The owning user always manages their own ontology.
        if owner_type == Some("user") && owner_id == Some(user_id) {
            return Ok(Some(ResourceRole::Admin));
        }

        let membership = match owner_type {
            Some("organisation") => match owner_id {
                Some(oid) => self
                    .get_org_membership(user_id, oid)?
                    .map(ResourceRole::from_membership),
                None => None,
            },
            _ => None,
        };
        let grant = self.granted_resource_role(user_id, resource_type, resource_id)?;
        let mut best = combine_membership_and_grant(membership, grant);

        if best.is_none() && is_public {
            best = Some(ResourceRole::Viewer);
        }

        Ok(best)
    }

    // ─── Dataset Graphs ───────────────────────────────────────────────────────

    pub fn add_dataset_graph(&self, dataset_id: &str, graph_iri: &str) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        conn.execute(
            "INSERT OR IGNORE INTO dataset_graphs (dataset_id, graph_iri) VALUES (?1,?2)",
            params![dataset_id, graph_iri],
        )?;
        self.invalidate_accessible_graphs_cache();
        Ok(())
    }

    pub fn remove_dataset_graph(&self, dataset_id: &str, graph_iri: &str) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        conn.execute(
            "DELETE FROM dataset_graphs WHERE dataset_id=?1 AND graph_iri=?2",
            params![dataset_id, graph_iri],
        )?;
        self.invalidate_accessible_graphs_cache();
        Ok(())
    }

    pub fn list_dataset_graphs(&self, dataset_id: &str) -> anyhow::Result<Vec<String>> {
        let conn = self.pool.get()?;
        let mut stmt = conn.prepare(
            "SELECT graph_iri FROM dataset_graphs WHERE dataset_id=?1 ORDER BY graph_iri",
        )?;
        let graphs = stmt
            .query_map(params![dataset_id], |row| row.get(0))?
            .collect::<Result<Vec<String>, _>>()?;
        Ok(graphs)
    }

    /// Like [`list_dataset_graphs`] but returns full entries including the optional `graph_role`.
    pub fn list_dataset_graph_entries(
        &self,
        dataset_id: &str,
    ) -> anyhow::Result<Vec<DatasetGraphEntry>> {
        let conn = self.pool.get()?;
        let mut stmt = conn.prepare(
            "SELECT graph_iri, graph_role, private FROM dataset_graphs WHERE dataset_id=?1 ORDER BY graph_iri",
        )?;
        let entries = stmt
            .query_map(params![dataset_id], |row| {
                let iri: String = row.get(0)?;
                let role_str: Option<String> = row.get(1)?;
                let private: i64 = row.get(2)?;
                Ok(DatasetGraphEntry {
                    graph_iri: iri,
                    graph_role: role_str.as_deref().and_then(GraphKind::from_str),
                    private: private != 0,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(entries)
    }

    /// Return the distinct, non-null `graph_role` values for every dataset that
    /// has at least one role-tagged graph, keyed by `dataset_id`. Used to show
    /// the mix of roles a dataset contains on the datasets list. Roles are
    /// ordered deterministically (alphabetically) for stable rendering.
    pub fn all_dataset_roles(
        &self,
    ) -> anyhow::Result<std::collections::HashMap<String, Vec<GraphKind>>> {
        let conn = self.pool.get()?;
        let mut stmt = conn.prepare(
            "SELECT DISTINCT dataset_id, graph_role FROM dataset_graphs \
             WHERE graph_role IS NOT NULL ORDER BY dataset_id, graph_role",
        )?;
        let rows = stmt.query_map([], |row| {
            let ds_id: String = row.get(0)?;
            let role_str: String = row.get(1)?;
            Ok((ds_id, role_str))
        })?;
        let mut map: std::collections::HashMap<String, Vec<GraphKind>> =
            std::collections::HashMap::new();
        for r in rows {
            let (ds_id, role_str) = r?;
            if let Some(role) = GraphKind::from_str(&role_str) {
                let entry = map.entry(ds_id).or_default();
                if !entry.contains(&role) {
                    entry.push(role);
                }
            }
        }
        Ok(map)
    }

    /// Set or clear the `graph_role` for an already-registered graph.
    pub fn set_dataset_graph_role(
        &self,
        dataset_id: &str,
        graph_iri: &str,
        graph_role: Option<GraphKind>,
    ) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        let role_str = graph_role.map(|r| r.as_str().to_string());
        conn.execute(
            "UPDATE dataset_graphs SET graph_role=?1 WHERE dataset_id=?2 AND graph_iri=?3",
            params![role_str, dataset_id, graph_iri],
        )?;
        Ok(())
    }

    /// Mark a registered graph private or public. A private graph is excluded
    /// from the accessible-graph set of users who cannot write the owning
    /// dataset, so it disappears from listings and SPARQL scope for them.
    pub fn set_dataset_graph_private(
        &self,
        dataset_id: &str,
        graph_iri: &str,
        private: bool,
    ) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        conn.execute(
            "UPDATE dataset_graphs SET private=?1 WHERE dataset_id=?2 AND graph_iri=?3",
            params![private as i64, dataset_id, graph_iri],
        )?;
        self.invalidate_accessible_graphs_cache();
        Ok(())
    }

    /// Returns `true` when `graph_iri` is still registered to at least one dataset
    /// other than `exclude_dataset_id`.  Pass `""` for `exclude_dataset_id` when the
    /// calling dataset's own rows have already been removed from the table.
    pub fn graph_has_other_dataset_refs(
        &self,
        graph_iri: &str,
        exclude_dataset_id: &str,
    ) -> anyhow::Result<bool> {
        let conn = self.pool.get()?;
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM dataset_graphs WHERE graph_iri = ?1 AND dataset_id != ?2",
            params![graph_iri, exclude_dataset_id],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    /// Returns (accessible_graph_iris, all_registered_graph_iris).
    /// A graph is accessible if it belongs to a dataset the user can access.
    /// Graphs not registered to any dataset are treated as unmanaged/public.
    /// Callers should show a graph if it is in `accessible` OR not in `all_registered`.
    pub fn get_accessible_graph_iris(
        &self,
        user_id: Option<&str>,
    ) -> anyhow::Result<(
        std::collections::HashSet<String>,
        std::collections::HashSet<String>,
    )> {
        use std::collections::HashSet;

        // 1. Collect ALL (dataset_id, graph_iri, private) pairs from the registry table.
        let all_pairs: Vec<(String, String, bool)> = {
            let conn = self.pool.get()?;
            let mut stmt =
                conn.prepare("SELECT dataset_id, graph_iri, private FROM dataset_graphs")?;
            let rows = stmt
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)? != 0,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            rows
        };

        let all_registered: HashSet<String> = all_pairs.iter().map(|(_, g, _)| g.clone()).collect();

        // 2. Find which datasets this user can access, and of those, which they
        //    can write — private graphs are only visible to writers (owner /
        //    maintainer / admin), never to plain viewers or the public.
        let accessible_datasets = self.list_accessible_datasets(user_id)?;
        let accessible_ids: HashSet<&str> =
            accessible_datasets.iter().map(|d| d.id.as_str()).collect();
        let is_admin = match user_id {
            Some(uid) => self
                .get_user_by_id(uid)?
                .map(|u| u.role.is_admin())
                .unwrap_or(false),
            None => false,
        };
        let writable_ids: HashSet<&str> = if is_admin {
            accessible_ids.clone()
        } else {
            accessible_datasets
                .iter()
                .filter(|d| {
                    self.effective_dataset_role(user_id, d)
                        .ok()
                        .flatten()
                        .map(|r| r.can_write())
                        .unwrap_or(false)
                })
                .map(|d| d.id.as_str())
                .collect()
        };

        // 3. Keep graph IRIs whose owning dataset is accessible, dropping private
        //    graphs in datasets the user cannot write.
        let accessible: HashSet<String> = all_pairs
            .into_iter()
            .filter(|(ds_id, _, private)| {
                accessible_ids.contains(ds_id.as_str())
                    && (!*private || writable_ids.contains(ds_id.as_str()))
            })
            .map(|(_, g, _)| g)
            .collect();

        Ok((accessible, all_registered))
    }

    /// Cached wrapper around `get_accessible_graph_iris`. Safe in hot paths.
    /// Entries expire after `ACCESSIBLE_GRAPHS_TTL` or on
    /// `invalidate_accessible_graphs_cache()`.
    pub fn get_accessible_graph_iris_cached(
        &self,
        user_id: Option<&str>,
    ) -> anyhow::Result<Arc<AccessibleGraphs>> {
        let key = user_id.map(|s| s.to_string());
        if let Ok(guard) = self.accessible_graphs_cache.lock() {
            if let Some((ts, val)) = guard.get(&key) {
                if ts.elapsed() < ACCESSIBLE_GRAPHS_TTL {
                    return Ok(Arc::clone(val));
                }
            }
        }
        let fresh = Arc::new(self.get_accessible_graph_iris(user_id)?);
        if let Ok(mut guard) = self.accessible_graphs_cache.lock() {
            guard.insert(key, (Instant::now(), Arc::clone(&fresh)));
        }
        Ok(fresh)
    }

    /// Drop all cached accessible-graph sets. Call after dataset/ACL changes.
    pub fn invalidate_accessible_graphs_cache(&self) {
        if let Ok(mut guard) = self.accessible_graphs_cache.lock() {
            guard.clear();
        }
    }

    // ─── SPARQL Services ──────────────────────────────────────────────────────

    pub fn create_sparql_service(
        &self,
        id: &str,
        dataset_id: &str,
        name: &str,
        slug: &str,
        description: Option<&str>,
    ) -> anyhow::Result<SparqlService> {
        let conn = self.pool.get()?;
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO sparql_services (id, dataset_id, name, slug, description, created_at) VALUES (?1,?2,?3,?4,?5,?6)",
            params![id, dataset_id, name, slug, description, now],
        )?;
        Ok(SparqlService {
            id: id.to_string(),
            dataset_id: dataset_id.to_string(),
            name: name.to_string(),
            slug: slug.to_string(),
            sparql_endpoint: format!("/api/datasets/{}/services/{}/sparql", dataset_id, slug),
            description: description.map(String::from),
            is_active: true,
            created_at: now,
        })
    }

    pub fn get_sparql_service(&self, id: &str) -> anyhow::Result<Option<SparqlService>> {
        let conn = self.pool.get()?;
        conn.query_row(
            "SELECT id, dataset_id, name, slug, description, is_active, created_at FROM sparql_services WHERE id = ?1",
            params![id],
            |row| {
                Ok(SparqlService {
                    id: row.get(0)?,
                    dataset_id: row.get(1)?,
                    name: row.get(2)?,
                    slug: row.get(3)?,
                    sparql_endpoint: {
                        let dataset_id: String = row.get(1)?;
                        let slug: String = row.get(3)?;
                        format!("/api/datasets/{}/services/{}/sparql", dataset_id, slug)
                    },
                    description: row.get(4)?,
                    is_active: row.get::<_, i32>(5)? != 0,
                    created_at: row.get(6)?,
                })
            },
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn get_sparql_service_by_slug(
        &self,
        dataset_id: &str,
        slug: &str,
    ) -> anyhow::Result<Option<SparqlService>> {
        let conn = self.pool.get()?;
        conn.query_row(
            "SELECT id, dataset_id, name, slug, description, is_active, created_at FROM sparql_services WHERE dataset_id=?1 AND slug=?2",
            params![dataset_id, slug],
            |row| {
                Ok(SparqlService {
                    id: row.get(0)?,
                    dataset_id: row.get(1)?,
                    name: row.get(2)?,
                    slug: row.get(3)?,
                    sparql_endpoint: {
                        let dataset_id: String = row.get(1)?;
                        let slug: String = row.get(3)?;
                        format!("/api/datasets/{}/services/{}/sparql", dataset_id, slug)
                    },
                    description: row.get(4)?,
                    is_active: row.get::<_, i32>(5)? != 0,
                    created_at: row.get(6)?,
                })
            },
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn list_dataset_services(&self, dataset_id: &str) -> anyhow::Result<Vec<SparqlService>> {
        let conn = self.pool.get()?;
        let mut stmt = conn.prepare(
            "SELECT id, dataset_id, name, slug, description, is_active, created_at FROM sparql_services WHERE dataset_id=?1 ORDER BY name",
        )?;
        let services = stmt
            .query_map(params![dataset_id], |row| {
                Ok(SparqlService {
                    id: row.get(0)?,
                    dataset_id: row.get(1)?,
                    name: row.get(2)?,
                    slug: row.get(3)?,
                    sparql_endpoint: {
                        let dataset_id: String = row.get(1)?;
                        let slug: String = row.get(3)?;
                        format!("/api/datasets/{}/services/{}/sparql", dataset_id, slug)
                    },
                    description: row.get(4)?,
                    is_active: row.get::<_, i32>(5)? != 0,
                    created_at: row.get(6)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(services)
    }

    pub fn update_sparql_service(
        &self,
        id: &str,
        name: &str,
        description: Option<&str>,
        is_active: Option<bool>,
    ) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        let is_active_val = is_active.map(|v| v as i32);
        conn.execute(
            "UPDATE sparql_services SET name=?1, description=?2, is_active=COALESCE(?3, is_active) WHERE id=?4",
            params![name, description, is_active_val, id],
        )?;
        Ok(())
    }

    pub fn delete_sparql_service(&self, id: &str) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        conn.execute("DELETE FROM sparql_services WHERE id = ?1", params![id])?;
        Ok(())
    }

    // ─── Service Graphs ───────────────────────────────────────────────────────

    pub fn add_service_graph(&self, service_id: &str, graph_iri: &str) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        conn.execute(
            "INSERT OR IGNORE INTO service_graphs (service_id, graph_iri) VALUES (?1,?2)",
            params![service_id, graph_iri],
        )?;
        Ok(())
    }

    pub fn remove_service_graph(&self, service_id: &str, graph_iri: &str) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        conn.execute(
            "DELETE FROM service_graphs WHERE service_id=?1 AND graph_iri=?2",
            params![service_id, graph_iri],
        )?;
        Ok(())
    }

    pub fn list_service_graphs(&self, service_id: &str) -> anyhow::Result<Vec<String>> {
        let conn = self.pool.get()?;
        let mut stmt = conn.prepare(
            "SELECT graph_iri FROM service_graphs WHERE service_id=?1 ORDER BY graph_iri",
        )?;
        let graphs = stmt
            .query_map(params![service_id], |row| row.get(0))?
            .collect::<Result<Vec<String>, _>>()?;
        Ok(graphs)
    }

    // ─── Assets ───────────────────────────────────────────────────────────────

    #[allow(clippy::too_many_arguments)]
    pub fn create_asset(
        &self,
        id: &str,
        dataset_id: &str,
        filename: &str,
        content_type: &str,
        s3_key: &str,
        size_bytes: i64,
        uploaded_by: &str,
        public: bool,
    ) -> anyhow::Result<Asset> {
        let conn = self.pool.get()?;
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO assets (id, dataset_id, filename, content_type, s3_key, size_bytes, uploaded_by, created_at, public) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![id, dataset_id, filename, content_type, s3_key, size_bytes, uploaded_by, now, public as i64],
        )?;
        Ok(Asset {
            id: id.to_string(),
            dataset_id: dataset_id.to_string(),
            filename: filename.to_string(),
            content_type: content_type.to_string(),
            s3_key: s3_key.to_string(),
            size_bytes,
            uploaded_by: uploaded_by.to_string(),
            created_at: now,
            updated_at: None,
            title: None,
            description: None,
            public,
        })
    }

    fn row_to_asset(row: &rusqlite::Row<'_>) -> rusqlite::Result<Asset> {
        Ok(Asset {
            id: row.get(0)?,
            dataset_id: row.get(1)?,
            filename: row.get(2)?,
            content_type: row.get(3)?,
            s3_key: row.get(4)?,
            size_bytes: row.get(5)?,
            uploaded_by: row.get(6)?,
            created_at: row.get(7)?,
            public: row.get::<_, i64>(8)? != 0,
            title: row.get(9)?,
            description: row.get(10)?,
            updated_at: row.get(11)?,
        })
    }

    pub fn get_asset(&self, id: &str) -> anyhow::Result<Option<Asset>> {
        let conn = self.pool.get()?;
        conn.query_row(
            "SELECT id, dataset_id, filename, content_type, s3_key, size_bytes, uploaded_by, created_at, public, title, description, updated_at FROM assets WHERE id = ?1",
            params![id],
            Self::row_to_asset,
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn list_dataset_assets(&self, dataset_id: &str) -> anyhow::Result<Vec<Asset>> {
        let conn = self.pool.get()?;
        let mut stmt = conn.prepare(
            "SELECT id, dataset_id, filename, content_type, s3_key, size_bytes, uploaded_by, created_at, public, title, description, updated_at FROM assets WHERE dataset_id=?1 ORDER BY filename",
        )?;
        let assets = stmt
            .query_map(params![dataset_id], Self::row_to_asset)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(assets)
    }

    pub fn update_asset_public(&self, id: &str, public: bool) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        conn.execute(
            "UPDATE assets SET public = ?1 WHERE id = ?2",
            params![public as i64, id],
        )?;
        Ok(())
    }

    pub fn update_asset_metadata(
        &self,
        id: &str,
        title: Option<&str>,
        description: Option<&str>,
    ) -> anyhow::Result<Asset> {
        let conn = self.pool.get()?;
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE assets SET title=?1, description=?2, updated_at=?3 WHERE id=?4",
            params![title, description, now, id],
        )?;
        self.get_asset(id)?
            .ok_or_else(|| anyhow::anyhow!("Asset not found"))
    }

    pub fn delete_asset(&self, id: &str) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        conn.execute("DELETE FROM assets WHERE id = ?1", params![id])?;
        Ok(())
    }

    // ─── Access Control Helpers ───────────────────────────────────────────────

    /// Check if a user can read a dataset (any effective role grants read).
    pub fn can_access_dataset(
        &self,
        user_id: Option<&str>,
        dataset: &Dataset,
    ) -> anyhow::Result<bool> {
        Ok(self.effective_dataset_role(user_id, dataset)?.is_some())
    }

    /// Check if a user can modify a dataset's data (editor or admin role).
    pub fn can_write_dataset(&self, user_id: &str, dataset: &Dataset) -> anyhow::Result<bool> {
        Ok(self
            .effective_dataset_role(Some(user_id), dataset)?
            .map(|r| r.can_write())
            .unwrap_or(false))
    }

    /// Check if a user can manage a dataset — its settings, metadata and access
    /// grants (admin/owner-level role).
    pub fn can_manage_dataset(&self, user_id: &str, dataset: &Dataset) -> anyhow::Result<bool> {
        Ok(self
            .effective_dataset_role(Some(user_id), dataset)?
            .map(|r| r.can_manage())
            .unwrap_or(false))
    }

    /// Check if a user can access an ontology.
    ///
    /// Rules (mirrors `can_access_dataset`):
    /// - Public ontologies: always accessible.
    /// - Anonymous users: cannot access private ontologies.
    /// - Admins: always accessible.
    /// - Owner (user type): the owning user can access.
    /// - Owner (organisation type): any member of the owning org can access.
    pub fn can_access_ontology(
        &self,
        user_id: Option<&str>,
        is_public: bool,
        owner_type: Option<&str>,
        owner_id: Option<&str>,
    ) -> anyhow::Result<bool> {
        if is_public {
            return Ok(true);
        }
        let user_id = match user_id {
            Some(id) => id,
            None => return Ok(false),
        };
        // Admins can see everything
        if let Some(user) = self.get_user_by_id(user_id)? {
            if user.role.is_admin() {
                return Ok(true);
            }
        }
        // Owner-based access
        match owner_type {
            Some("user") => {
                if let Some(oid) = owner_id {
                    if oid == user_id {
                        return Ok(true);
                    }
                }
            }
            Some("organisation") => {
                if let Some(oid) = owner_id {
                    if self.get_org_membership(user_id, oid)?.is_some() {
                        return Ok(true);
                    }
                }
            }
            _ => {}
        }
        Ok(false)
    }

    /// Check if a user can WRITE (mutate metadata, upload versions, edit drafts,
    /// stage/publish/deprecate) an ontology — data-model or vocabulary.
    ///
    /// Rules:
    /// - Admins: always allowed.
    /// - Owner (user type): the owning user is allowed.
    /// - Owner (organisation type): only org *admins* may write; regular org
    ///   members and viewers may read but not mutate.
    /// - Otherwise: denied.
    ///
    /// `is_public` is intentionally ignored — public visibility never grants
    /// write access.
    pub fn can_write_ontology(
        &self,
        user_id: &str,
        owner_type: Option<&str>,
        owner_id: Option<&str>,
    ) -> anyhow::Result<bool> {
        if let Some(user) = self.get_user_by_id(user_id)? {
            if user.role.is_admin() {
                return Ok(true);
            }
        }
        match owner_type {
            Some("user") => Ok(owner_id.map(|oid| oid == user_id).unwrap_or(false)),
            Some("organisation") => {
                if let Some(oid) = owner_id {
                    // Only org admins may write org-owned ontologies; plain
                    // members and viewers are read-only.
                    Ok(matches!(
                        self.get_org_membership(user_id, oid)?,
                        Some(Role::Admin)
                    ))
                } else {
                    Ok(false)
                }
            }
            _ => Ok(false),
        }
    }

    /// Check if a user can MANAGE an ontology — its settings, metadata and
    /// access grants (admin/owner-level). Plain members and viewers cannot.
    pub fn can_manage_ontology(
        &self,
        user_id: &str,
        owner_type: Option<&str>,
        owner_id: Option<&str>,
    ) -> anyhow::Result<bool> {
        if let Some(user) = self.get_user_by_id(user_id)? {
            if user.role.is_admin() {
                return Ok(true);
            }
        }
        match owner_type {
            Some("user") => Ok(owner_id.map(|oid| oid == user_id).unwrap_or(false)),
            Some("organisation") => {
                if let Some(oid) = owner_id {
                    Ok(matches!(
                        self.get_org_membership(user_id, oid)?,
                        Some(Role::Admin)
                    ))
                } else {
                    Ok(false)
                }
            }
            _ => Ok(false),
        }
    }

    /// List datasets accessible by a user (public + membership-based + private access).
    /// List datasets a user can access. Resolves the same effective role as
    /// [`Self::effective_dataset_role`] / [`Self::can_access_dataset`], but
    /// prefetches every per-user authorization fact once (memberships, grants,
    /// private-access allow-list) and resolves each dataset in memory — avoiding
    /// the N round-trips of calling `can_access_dataset` per row.
    pub fn list_accessible_datasets(&self, user_id: Option<&str>) -> anyhow::Result<Vec<Dataset>> {
        use std::collections::{HashMap, HashSet};

        let all = self.list_datasets()?;

        // Anonymous users only ever see public datasets.
        let user_id = match user_id {
            Some(id) => id,
            None => {
                return Ok(all
                    .into_iter()
                    .filter(|d| d.visibility == Visibility::Public)
                    .collect());
            }
        };

        // System admins have an Admin effective role on every dataset.
        if let Some(user) = self.get_user_by_id(user_id)? {
            if user.role.is_admin() {
                return Ok(all);
            }
        }

        let conn = self.pool.get()?;

        // org_id -> membership role
        let mut org_roles: HashMap<String, Role> = HashMap::new();
        {
            let mut stmt =
                conn.prepare("SELECT org_id, role FROM org_memberships WHERE user_id=?1")?;
            let rows = stmt.query_map(params![user_id], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?;
            for row in rows {
                let (org_id, role) = row?;
                if let Some(r) = Role::from_str(&role) {
                    org_roles.insert(org_id, r);
                }
            }
        }

        // group_id -> membership role
        let mut group_roles: HashMap<String, Role> = HashMap::new();
        {
            let mut stmt =
                conn.prepare("SELECT group_id, role FROM group_memberships WHERE user_id=?1")?;
            let rows = stmt.query_map(params![user_id], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?;
            for row in rows {
                let (group_id, role) = row?;
                if let Some(r) = Role::from_str(&role) {
                    group_roles.insert(group_id, r);
                }
            }
        }

        // group_id -> owning org_id (group-owned datasets also honour parent-org membership)
        let mut group_orgs: HashMap<String, String> = HashMap::new();
        {
            let mut stmt = conn.prepare("SELECT id, org_id FROM groups")?;
            let rows =
                stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
            for row in rows {
                let (id, org_id) = row?;
                group_orgs.insert(id, org_id);
            }
        }

        // dataset_id -> strongest explicit grant (direct user grant or via a group)
        let mut dataset_grants: HashMap<String, ResourceRole> = HashMap::new();
        {
            let mut stmt = conn.prepare(
                "SELECT resource_id, role FROM resource_access
                 WHERE resource_type='dataset'
                   AND ( (principal_type='user' AND principal_id=?1)
                      OR (principal_type='group' AND principal_id IN (
                            SELECT group_id FROM group_memberships WHERE user_id=?1)) )",
            )?;
            let rows = stmt.query_map(params![user_id], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?;
            for row in rows {
                let (rid, role) = row?;
                if let Some(rr) = ResourceRole::from_str(&role) {
                    let slot = dataset_grants.entry(rid).or_insert(rr);
                    if rr > *slot {
                        *slot = rr;
                    }
                }
            }
        }

        // Legacy private-access allow-list (grants Viewer on private datasets).
        let mut private_access: HashSet<String> = HashSet::new();
        {
            let mut stmt =
                conn.prepare("SELECT dataset_id FROM dataset_private_access WHERE user_id=?1")?;
            let rows = stmt.query_map(params![user_id], |r| r.get::<_, String>(0))?;
            for row in rows {
                private_access.insert(row?);
            }
        }

        let accessible = all
            .into_iter()
            .filter(|ds| {
                // The owning user always manages their own dataset.
                if ds.owner_type == OwnerType::User && ds.owner_id == user_id {
                    return true;
                }

                // membership_role_for_dataset, resolved from prefetched maps.
                let membership = match ds.owner_type {
                    OwnerType::User => None,
                    OwnerType::Organisation => {
                        scope_membership_role(org_roles.get(&ds.owner_id).copied(), ds.visibility)
                    }
                    OwnerType::Group => {
                        let mut best = scope_membership_role(
                            group_roles.get(&ds.owner_id).copied(),
                            ds.visibility,
                        );
                        if let Some(org_id) = group_orgs.get(&ds.owner_id) {
                            best = stronger(
                                best,
                                scope_membership_role(
                                    org_roles.get(org_id).copied(),
                                    ds.visibility,
                                ),
                            );
                        }
                        best
                    }
                };

                let grant = dataset_grants.get(&ds.id).copied();
                let mut best = combine_membership_and_grant(membership, grant);

                if best.is_none()
                    && ds.visibility == Visibility::Private
                    && private_access.contains(&ds.id)
                {
                    best = Some(ResourceRole::Viewer);
                }
                if best.is_none() && ds.visibility == Visibility::Public {
                    best = Some(ResourceRole::Viewer);
                }

                best.is_some()
            })
            .collect();

        Ok(accessible)
    }

    pub fn update_user_avatar(
        &self,
        user_id: &str,
        avatar_key: Option<&str>,
    ) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE users SET avatar_key=?1, updated_at=?2 WHERE id=?3",
            params![avatar_key, now, user_id],
        )?;
        Ok(())
    }

    pub fn update_org_image(&self, org_id: &str, image_key: Option<&str>) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        conn.execute(
            "UPDATE organisations SET image_key=?1 WHERE id=?2",
            params![image_key, org_id],
        )?;
        Ok(())
    }

    pub fn update_dataset_image(
        &self,
        dataset_id: &str,
        image_key: Option<&str>,
    ) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE datasets SET image_key=?1, updated_at=?2 WHERE id=?3",
            params![image_key, now, dataset_id],
        )?;
        Ok(())
    }

    pub fn update_org_banner(&self, org_id: &str, banner_key: Option<&str>) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        conn.execute(
            "UPDATE organisations SET banner_key=?1 WHERE id=?2",
            params![banner_key, org_id],
        )?;
        Ok(())
    }

    pub fn update_dataset_banner(
        &self,
        dataset_id: &str,
        banner_key: Option<&str>,
    ) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE datasets SET banner_key=?1, updated_at=?2 WHERE id=?3",
            params![banner_key, now, dataset_id],
        )?;
        Ok(())
    }

    // ─── Endpoint ACL ─────────────────────────────────────────────────────────

    #[allow(clippy::too_many_arguments)]
    pub fn create_endpoint_acl_rule(
        &self,
        id: &str,
        principal_type: &str,
        principal_id: &str,
        path_pattern: &str,
        http_methods: &str,
        effect: &str,
        priority: i64,
        created_by: &str,
    ) -> anyhow::Result<EndpointAclRule> {
        let conn = self.pool.get()?;
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO endpoint_acl (id, principal_type, principal_id, path_pattern, http_methods, effect, priority, created_at, created_by)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![id, principal_type, principal_id, path_pattern, http_methods, effect, priority, now, created_by],
        )?;
        Ok(EndpointAclRule {
            id: id.to_string(),
            principal_type: principal_type.to_string(),
            principal_id: principal_id.to_string(),
            path_pattern: path_pattern.to_string(),
            http_methods: http_methods.to_string(),
            effect: effect.to_string(),
            priority,
            created_at: now,
            created_by: created_by.to_string(),
        })
    }

    pub fn list_endpoint_acl_rules(&self) -> anyhow::Result<Vec<EndpointAclRule>> {
        let conn = self.pool.get()?;
        let mut stmt = conn.prepare(
            "SELECT id, principal_type, principal_id, path_pattern, http_methods, effect, priority, created_at, created_by
             FROM endpoint_acl ORDER BY priority DESC, created_at ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(EndpointAclRule {
                id: row.get(0)?,
                principal_type: row.get(1)?,
                principal_id: row.get(2)?,
                path_pattern: row.get(3)?,
                http_methods: row.get(4)?,
                effect: row.get(5)?,
                priority: row.get(6)?,
                created_at: row.get(7)?,
                created_by: row.get(8)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn update_endpoint_acl_rule(
        &self,
        id: &str,
        path_pattern: &str,
        http_methods: &str,
        effect: &str,
        priority: i64,
    ) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        let affected = conn.execute(
            "UPDATE endpoint_acl SET path_pattern=?1, http_methods=?2, effect=?3, priority=?4 WHERE id=?5",
            params![path_pattern, http_methods, effect, priority, id],
        )?;
        if affected == 0 {
            anyhow::bail!("endpoint ACL rule not found");
        }
        Ok(())
    }

    pub fn delete_endpoint_acl_rule(&self, id: &str) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        let affected = conn.execute("DELETE FROM endpoint_acl WHERE id=?1", params![id])?;
        if affected == 0 {
            anyhow::bail!("endpoint ACL rule not found");
        }
        Ok(())
    }

    /// Fetch all endpoint ACL rules relevant to a given user (by user id, role, org memberships, group memberships).
    pub fn get_endpoint_acl_rules_for_user(
        &self,
        user_id: &str,
        role: &str,
    ) -> anyhow::Result<Vec<EndpointAclRule>> {
        let conn = self.pool.get()?;
        // Collect org and group ids for this user
        let org_ids: Vec<String> = {
            let mut stmt = conn.prepare("SELECT org_id FROM org_memberships WHERE user_id=?1")?;
            let rows: rusqlite::Result<Vec<String>> =
                stmt.query_map(params![user_id], |r| r.get(0))?.collect();
            rows?
        };
        let group_ids: Vec<String> = {
            let mut stmt =
                conn.prepare("SELECT group_id FROM group_memberships WHERE user_id=?1")?;
            let rows: rusqlite::Result<Vec<String>> =
                stmt.query_map(params![user_id], |r| r.get(0))?.collect();
            rows?
        };

        let mut all_rules = Vec::new();
        // Rules by user id
        {
            let mut stmt = conn.prepare(
                "SELECT id, principal_type, principal_id, path_pattern, http_methods, effect, priority, created_at, created_by
                 FROM endpoint_acl WHERE principal_type='user' AND principal_id=?1
                 ORDER BY priority DESC",
            )?;
            let rows = stmt.query_map(params![user_id], |row| {
                Ok(EndpointAclRule {
                    id: row.get(0)?,
                    principal_type: row.get(1)?,
                    principal_id: row.get(2)?,
                    path_pattern: row.get(3)?,
                    http_methods: row.get(4)?,
                    effect: row.get(5)?,
                    priority: row.get(6)?,
                    created_at: row.get(7)?,
                    created_by: row.get(8)?,
                })
            })?;
            all_rules.extend(rows.collect::<Result<Vec<_>, _>>()?);
        }
        // Rules by role
        {
            let mut stmt = conn.prepare(
                "SELECT id, principal_type, principal_id, path_pattern, http_methods, effect, priority, created_at, created_by
                 FROM endpoint_acl WHERE principal_type='role' AND principal_id=?1
                 ORDER BY priority DESC",
            )?;
            let rows = stmt.query_map(params![role], |row| {
                Ok(EndpointAclRule {
                    id: row.get(0)?,
                    principal_type: row.get(1)?,
                    principal_id: row.get(2)?,
                    path_pattern: row.get(3)?,
                    http_methods: row.get(4)?,
                    effect: row.get(5)?,
                    priority: row.get(6)?,
                    created_at: row.get(7)?,
                    created_by: row.get(8)?,
                })
            })?;
            all_rules.extend(rows.collect::<Result<Vec<_>, _>>()?);
        }
        // Rules by org membership
        for org_id in &org_ids {
            let mut stmt = conn.prepare(
                "SELECT id, principal_type, principal_id, path_pattern, http_methods, effect, priority, created_at, created_by
                 FROM endpoint_acl WHERE principal_type='organisation' AND principal_id=?1
                 ORDER BY priority DESC",
            )?;
            let rows = stmt.query_map(params![org_id], |row| {
                Ok(EndpointAclRule {
                    id: row.get(0)?,
                    principal_type: row.get(1)?,
                    principal_id: row.get(2)?,
                    path_pattern: row.get(3)?,
                    http_methods: row.get(4)?,
                    effect: row.get(5)?,
                    priority: row.get(6)?,
                    created_at: row.get(7)?,
                    created_by: row.get(8)?,
                })
            })?;
            all_rules.extend(rows.collect::<Result<Vec<_>, _>>()?);
        }
        // Rules by group membership
        for group_id in &group_ids {
            let mut stmt = conn.prepare(
                "SELECT id, principal_type, principal_id, path_pattern, http_methods, effect, priority, created_at, created_by
                 FROM endpoint_acl WHERE principal_type='group' AND principal_id=?1
                 ORDER BY priority DESC",
            )?;
            let rows = stmt.query_map(params![group_id], |row| {
                Ok(EndpointAclRule {
                    id: row.get(0)?,
                    principal_type: row.get(1)?,
                    principal_id: row.get(2)?,
                    path_pattern: row.get(3)?,
                    http_methods: row.get(4)?,
                    effect: row.get(5)?,
                    priority: row.get(6)?,
                    created_at: row.get(7)?,
                    created_by: row.get(8)?,
                })
            })?;
            all_rules.extend(rows.collect::<Result<Vec<_>, _>>()?);
        }

        all_rules.sort_by_key(|b| std::cmp::Reverse(b.priority));
        Ok(all_rules)
    }

    // ─── Graph ACL ────────────────────────────────────────────────────────────

    pub fn grant_graph_permission(
        &self,
        id: &str,
        graph_iri: &str,
        principal_type: &str,
        principal_id: &str,
        permission: &str,
        created_by: &str,
    ) -> anyhow::Result<GraphAclRule> {
        let conn = self.pool.get()?;
        let now = chrono::Utc::now().to_rfc3339();
        // Public grants have no meaningful principal_id; canonicalize it to '*' so
        // the stored row matches what check_graph_permission enforces. Without this a
        // public grant written with some other principal_id would be listed
        // (get_graph_acl_readable_iris ignores principal_id for public) yet 403 on
        // access (enforcement matched only '*'), making it "discoverable but dead".
        let principal_id = if principal_type == "public" {
            "*"
        } else {
            principal_id
        };
        conn.execute(
            "INSERT OR IGNORE INTO graph_acl (id, graph_iri, principal_type, principal_id, permission, created_at, created_by)
             VALUES (?1,?2,?3,?4,?5,?6,?7)",
            params![id, graph_iri, principal_type, principal_id, permission, now, created_by],
        )?;
        Ok(GraphAclRule {
            id: id.to_string(),
            graph_iri: graph_iri.to_string(),
            principal_type: principal_type.to_string(),
            principal_id: principal_id.to_string(),
            permission: permission.to_string(),
            created_at: now,
            created_by: created_by.to_string(),
        })
    }

    pub fn list_graph_acl_rules(
        &self,
        graph_iri: Option<&str>,
    ) -> anyhow::Result<Vec<GraphAclRule>> {
        let conn = self.pool.get()?;
        let read_row = |row: &rusqlite::Row| -> rusqlite::Result<GraphAclRule> {
            Ok(GraphAclRule {
                id: row.get(0)?,
                graph_iri: row.get(1)?,
                principal_type: row.get(2)?,
                principal_id: row.get(3)?,
                permission: row.get(4)?,
                created_at: row.get(5)?,
                created_by: row.get(6)?,
            })
        };
        let mut stmt = conn.prepare(
            "SELECT id, graph_iri, principal_type, principal_id, permission, created_at, created_by
             FROM graph_acl WHERE (?1 IS NULL OR graph_iri = ?1) ORDER BY graph_iri, created_at ASC",
        )?;
        let rows = stmt
            .query_map(params![graph_iri], read_row)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(Into::into);
        rows
    }

    pub fn revoke_graph_permission(&self, id: &str) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        let affected = conn.execute("DELETE FROM graph_acl WHERE id=?1", params![id])?;
        if affected == 0 {
            anyhow::bail!("graph ACL rule not found");
        }
        Ok(())
    }

    /// Returns graph IRIs readable by this user (via graph_acl, in addition to dataset-visibility).
    pub fn get_graph_acl_readable_iris(
        &self,
        user_id: &str,
        role: &str,
    ) -> anyhow::Result<Vec<String>> {
        let conn = self.pool.get()?;
        let org_ids: Vec<String> = {
            let mut stmt = conn.prepare("SELECT org_id FROM org_memberships WHERE user_id=?1")?;
            let rows: rusqlite::Result<Vec<String>> =
                stmt.query_map(params![user_id], |r| r.get(0))?.collect();
            rows?
        };
        let group_ids: Vec<String> = {
            let mut stmt =
                conn.prepare("SELECT group_id FROM group_memberships WHERE user_id=?1")?;
            let rows: rusqlite::Result<Vec<String>> =
                stmt.query_map(params![user_id], |r| r.get(0))?.collect();
            rows?
        };

        let mut iris = std::collections::HashSet::new();
        let perms_check = "('read','write','admin')";

        // public grants
        {
            let sql = format!(
                "SELECT DISTINCT graph_iri FROM graph_acl WHERE principal_type='public' AND permission IN {perms_check}"
            );
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
            for r in rows {
                iris.insert(r?);
            }
        }
        // role grants
        {
            let sql = format!(
                "SELECT DISTINCT graph_iri FROM graph_acl WHERE principal_type='role' AND principal_id=?1 AND permission IN {perms_check}"
            );
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map(params![role], |r| r.get::<_, String>(0))?;
            for r in rows {
                iris.insert(r?);
            }
        }
        // user grants
        {
            let sql = format!(
                "SELECT DISTINCT graph_iri FROM graph_acl WHERE principal_type='user' AND principal_id=?1 AND permission IN {perms_check}"
            );
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map(params![user_id], |r| r.get::<_, String>(0))?;
            for r in rows {
                iris.insert(r?);
            }
        }
        // org grants
        for org_id in &org_ids {
            let sql = format!(
                "SELECT DISTINCT graph_iri FROM graph_acl WHERE principal_type='organisation' AND principal_id=?1 AND permission IN {perms_check}"
            );
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map(params![org_id], |r| r.get::<_, String>(0))?;
            for r in rows {
                iris.insert(r?);
            }
        }
        // group grants
        for group_id in &group_ids {
            let sql = format!(
                "SELECT DISTINCT graph_iri FROM graph_acl WHERE principal_type='group' AND principal_id=?1 AND permission IN {perms_check}"
            );
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map(params![group_id], |r| r.get::<_, String>(0))?;
            for r in rows {
                iris.insert(r?);
            }
        }

        Ok(iris.into_iter().collect())
    }

    /// Check if a user has a specific permission on a named graph via graph_acl.
    pub fn check_graph_permission(
        &self,
        user_id: &str,
        role: &str,
        graph_iri: &str,
        required_permission: &str,
    ) -> anyhow::Result<bool> {
        // Permission hierarchy (manage ⊇ write ⊇ read) lives on AccessLevel:
        // a grant satisfies the requirement when it is at least as strong.
        let matching_perms = match AccessLevel::from_str(required_permission) {
            Some(level) => level.satisfying_spellings(),
            None => return Ok(false),
        };

        let conn = self.pool.get()?;
        let org_ids: Vec<String> = {
            let mut stmt = conn.prepare("SELECT org_id FROM org_memberships WHERE user_id=?1")?;
            let rows: rusqlite::Result<Vec<String>> =
                stmt.query_map(params![user_id], |r| r.get(0))?.collect();
            rows?
        };
        let group_ids: Vec<String> = {
            let mut stmt =
                conn.prepare("SELECT group_id FROM group_memberships WHERE user_id=?1")?;
            let rows: rusqlite::Result<Vec<String>> =
                stmt.query_map(params![user_id], |r| r.get(0))?.collect();
            rows?
        };

        let perms_list = matching_perms
            .iter()
            .map(|p| format!("'{p}'"))
            .collect::<Vec<_>>()
            .join(",");

        // Public grants are matched on principal_type alone — principal_id is not
        // meaningful for them. This mirrors get_graph_acl_readable_iris (which
        // ignores principal_id for public) so enforcement and discoverability agree,
        // and it tolerates legacy rows whose principal_id is not '*'.
        {
            let sql = format!(
                "SELECT 1 FROM graph_acl WHERE graph_iri=?1 AND principal_type='public' AND permission IN ({perms_list}) LIMIT 1"
            );
            let mut stmt = conn.prepare(&sql)?;
            if stmt.exists(params![graph_iri])? {
                return Ok(true);
            }
        }

        // Remaining principals genuinely key on principal_id: role, user, org, group.
        let principals: Vec<(String, String)> = {
            let mut v = vec![
                ("role".to_string(), role.to_string()),
                ("user".to_string(), user_id.to_string()),
            ];
            for oid in &org_ids {
                v.push(("organisation".to_string(), oid.clone()));
            }
            for gid in &group_ids {
                v.push(("group".to_string(), gid.clone()));
            }
            v
        };

        for (pt, pid) in principals {
            let sql = format!(
                "SELECT 1 FROM graph_acl WHERE graph_iri=?1 AND principal_type=?2 AND principal_id=?3 AND permission IN ({perms_list}) LIMIT 1"
            );
            let mut stmt = conn.prepare(&sql)?;
            let found: bool = stmt.exists(params![graph_iri, pt, pid])?;
            if found {
                return Ok(true);
            }
        }
        Ok(false)
    }

    // ─── Triple Security Labels ───────────────────────────────────────────────

    pub fn create_triple_security_label(
        &self,
        id: &str,
        subject_iri: &str,
        predicate_iri: &str,
        object_value: &str,
        graph_iri: &str,
        label_graph_iri: &str,
    ) -> anyhow::Result<TripleSecurityLabel> {
        let conn = self.pool.get()?;
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT OR REPLACE INTO triple_security_labels
             (id, subject_iri, predicate_iri, object_value, graph_iri, label_graph_iri, created_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7)",
            params![
                id,
                subject_iri,
                predicate_iri,
                object_value,
                graph_iri,
                label_graph_iri,
                now
            ],
        )?;
        Ok(TripleSecurityLabel {
            id: id.to_string(),
            subject_iri: subject_iri.to_string(),
            predicate_iri: predicate_iri.to_string(),
            object_value: object_value.to_string(),
            graph_iri: graph_iri.to_string(),
            label_graph_iri: label_graph_iri.to_string(),
            created_at: now,
        })
    }

    pub fn list_triple_security_labels(
        &self,
        graph_iri: Option<&str>,
    ) -> anyhow::Result<Vec<TripleSecurityLabel>> {
        let conn = self.pool.get()?;
        let read_row = |row: &rusqlite::Row| -> rusqlite::Result<TripleSecurityLabel> {
            Ok(TripleSecurityLabel {
                id: row.get(0)?,
                subject_iri: row.get(1)?,
                predicate_iri: row.get(2)?,
                object_value: row.get(3)?,
                graph_iri: row.get(4)?,
                label_graph_iri: row.get(5)?,
                created_at: row.get(6)?,
            })
        };
        let mut stmt = conn.prepare(
            "SELECT id, subject_iri, predicate_iri, object_value, graph_iri, label_graph_iri, created_at
             FROM triple_security_labels WHERE (?1 IS NULL OR graph_iri = ?1) ORDER BY graph_iri, created_at ASC",
        )?;
        let rows = stmt
            .query_map(params![graph_iri], read_row)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(Into::into);
        rows
    }

    pub fn delete_triple_security_label(&self, id: &str) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        let affected = conn.execute(
            "DELETE FROM triple_security_labels WHERE id=?1",
            params![id],
        )?;
        if affected == 0 {
            anyhow::bail!("triple security label not found");
        }
        Ok(())
    }

    /// Returns true if there are any triple security labels in the given graphs.
    /// Used to short-circuit the triple-filter when the table is empty.
    pub fn has_triple_security_labels(&self, graph_iris: &[&str]) -> anyhow::Result<bool> {
        if graph_iris.is_empty() {
            let conn = self.pool.get()?;
            let count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM triple_security_labels LIMIT 1",
                [],
                |r| r.get(0),
            )?;
            return Ok(count > 0);
        }
        let conn = self.pool.get()?;
        let placeholders = graph_iris
            .iter()
            .enumerate()
            .map(|(i, _)| format!("?{}", i + 1))
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT 1 FROM triple_security_labels WHERE graph_iri IN ({placeholders}) LIMIT 1"
        );
        let mut stmt = conn.prepare(&sql)?;
        let params_boxed: Vec<&dyn rusqlite::ToSql> = graph_iris
            .iter()
            .map(|s| s as &dyn rusqlite::ToSql)
            .collect();
        let exists = stmt.exists(params_boxed.as_slice())?;
        Ok(exists)
    }

    /// Batch-lookup triple security labels for a set of (s,p,o,g) quads.
    /// Returns labels for quads that have one; others are unrestricted.
    pub fn get_labels_for_quads(
        &self,
        quads: &[(String, String, String, String)],
    ) -> anyhow::Result<Vec<(usize, String)>> {
        // Returns (quad_index, label_graph_iri) for each labelled quad
        let conn = self.pool.get()?;
        let mut results = Vec::new();
        for (idx, (s, p, o, g)) in quads.iter().enumerate() {
            let label: Option<String> = conn
                .query_row(
                    "SELECT label_graph_iri FROM triple_security_labels
                 WHERE subject_iri=?1 AND predicate_iri=?2 AND object_value=?3 AND graph_iri=?4",
                    params![s, p, o, g],
                    |r| r.get(0),
                )
                .optional()?;
            if let Some(lbl) = label {
                results.push((idx, lbl));
            }
        }
        Ok(results)
    }

    // ─── OAuth Providers ──────────────────────────────────────────────────────

    pub fn create_oauth_provider(&self, p: &OauthProviderCreate) -> anyhow::Result<OauthProvider> {
        let conn = self.pool.get()?;
        let now = chrono::Utc::now().to_rfc3339();
        let id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO oauth_providers
             (id, name, slug, provider_type, client_id, client_secret_enc, discovery_url, tenant_id,
              entity_id, sso_url, idp_certificate, scopes, role_claim_map, auto_provision,
              default_role, is_active, created_at, updated_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?17)",
            params![
                id,
                p.name,
                p.slug,
                p.provider_type,
                p.client_id,
                p.client_secret_enc,
                p.discovery_url,
                p.tenant_id,
                p.entity_id,
                p.sso_url,
                p.idp_certificate,
                // `scopes` is NOT NULL; an omitted value (None) must fall back to the
                // column default rather than binding an explicit NULL (500).
                p.scopes_or_default(),
                p.role_claim_map,
                p.auto_provision as i32,
                p.default_role,
                p.is_active as i32,
                now
            ],
        )?;
        self.get_oauth_provider_by_id_conn(&conn, &id)?
            .ok_or_else(|| anyhow::anyhow!("provider not found after insert"))
    }

    fn get_oauth_provider_by_id_conn(
        &self,
        conn: &Connection,
        id: &str,
    ) -> anyhow::Result<Option<OauthProvider>> {
        conn.query_row(
            "SELECT id, name, slug, provider_type, client_id, client_secret_enc, discovery_url, tenant_id,
                    entity_id, sso_url, idp_certificate, scopes, role_claim_map, auto_provision,
                    default_role, is_active, created_at, updated_at
             FROM oauth_providers WHERE id=?1",
            params![id],
            Self::row_to_oauth_provider,
        ).optional().map_err(Into::into)
    }

    fn row_to_oauth_provider(row: &rusqlite::Row) -> rusqlite::Result<OauthProvider> {
        Ok(OauthProvider {
            id: row.get(0)?,
            name: row.get(1)?,
            slug: row.get(2)?,
            provider_type: row.get(3)?,
            client_id: row.get(4)?,
            client_secret_enc: row.get(5)?,
            discovery_url: row.get(6)?,
            tenant_id: row.get(7)?,
            entity_id: row.get(8)?,
            sso_url: row.get(9)?,
            idp_certificate: row.get(10)?,
            scopes: row
                .get::<_, Option<String>>(11)?
                .unwrap_or_else(|| "openid email profile".to_string()),
            role_claim_map: row.get(12)?,
            auto_provision: row.get::<_, i32>(13)? != 0,
            default_role: row
                .get::<_, Option<String>>(14)?
                .unwrap_or_else(|| "user".to_string()),
            is_active: row.get::<_, i32>(15)? != 0,
            created_at: row.get(16)?,
            updated_at: row.get(17)?,
        })
    }

    pub fn get_oauth_provider_by_id(&self, id: &str) -> anyhow::Result<Option<OauthProvider>> {
        let conn = self.pool.get()?;
        self.get_oauth_provider_by_id_conn(&conn, id)
    }

    pub fn get_oauth_provider_by_slug(&self, slug: &str) -> anyhow::Result<Option<OauthProvider>> {
        let conn = self.pool.get()?;
        conn.query_row(
            "SELECT id, name, slug, provider_type, client_id, client_secret_enc, discovery_url, tenant_id,
                    entity_id, sso_url, idp_certificate, scopes, role_claim_map, auto_provision,
                    default_role, is_active, created_at, updated_at
             FROM oauth_providers WHERE slug=?1",
            params![slug],
            Self::row_to_oauth_provider,
        ).optional().map_err(Into::into)
    }

    pub fn list_oauth_providers(&self, active_only: bool) -> anyhow::Result<Vec<OauthProvider>> {
        let conn = self.pool.get()?;
        let sql = if active_only {
            "SELECT id, name, slug, provider_type, client_id, client_secret_enc, discovery_url, tenant_id,
                    entity_id, sso_url, idp_certificate, scopes, role_claim_map, auto_provision,
                    default_role, is_active, created_at, updated_at
             FROM oauth_providers WHERE is_active=1 ORDER BY name ASC"
        } else {
            "SELECT id, name, slug, provider_type, client_id, client_secret_enc, discovery_url, tenant_id,
                    entity_id, sso_url, idp_certificate, scopes, role_claim_map, auto_provision,
                    default_role, is_active, created_at, updated_at
             FROM oauth_providers ORDER BY name ASC"
        };
        let mut stmt = conn.prepare(sql)?;
        let rows = stmt.query_map([], Self::row_to_oauth_provider)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn update_oauth_provider(&self, id: &str, p: &OauthProviderCreate) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE oauth_providers SET name=?1, slug=?2, provider_type=?3, client_id=?4,
             client_secret_enc=?5, discovery_url=?6, tenant_id=?7, entity_id=?8, sso_url=?9,
             idp_certificate=?10, scopes=?11, role_claim_map=?12, auto_provision=?13,
             default_role=?14, is_active=?15, updated_at=?16 WHERE id=?17",
            params![
                p.name,
                p.slug,
                p.provider_type,
                p.client_id,
                p.client_secret_enc,
                p.discovery_url,
                p.tenant_id,
                p.entity_id,
                p.sso_url,
                p.idp_certificate,
                // `scopes` is NOT NULL; an omitted value (None) must fall back to the
                // column default rather than binding an explicit NULL (500).
                p.scopes_or_default(),
                p.role_claim_map,
                p.auto_provision as i32,
                p.default_role,
                p.is_active as i32,
                now,
                id
            ],
        )?;
        Ok(())
    }

    pub fn delete_oauth_provider(&self, id: &str) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        conn.execute("DELETE FROM oauth_providers WHERE id=?1", params![id])?;
        Ok(())
    }

    // ─── OAuth Identities ─────────────────────────────────────────────────────

    pub fn upsert_oauth_identity(
        &self,
        id: &str,
        user_id: &str,
        provider_id: &str,
        external_subject: &str,
        external_email: Option<&str>,
    ) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO oauth_identities (id, user_id, provider_id, external_subject, external_email, last_login_at, created_at)
             VALUES (?1,?2,?3,?4,?5,?6,?6)
             ON CONFLICT(provider_id, external_subject) DO UPDATE SET
               last_login_at=excluded.last_login_at,
               external_email=excluded.external_email",
            params![id, user_id, provider_id, external_subject, external_email, now],
        )?;
        Ok(())
    }

    pub fn find_user_by_oauth_identity(
        &self,
        provider_id: &str,
        external_subject: &str,
    ) -> anyhow::Result<Option<User>> {
        let conn = self.pool.get()?;
        conn.query_row(
            &format!(
                "SELECT {USER_COLS_U} FROM users u
                 JOIN oauth_identities oi ON oi.user_id = u.id
                 WHERE oi.provider_id=?1 AND oi.external_subject=?2"
            ),
            params![provider_id, external_subject],
            read_user,
        )
        .optional()
        .map_err(Into::into)
    }

    /// List all OAuth/SAML identities linked to a user account.
    pub fn list_oauth_identities_for_user(
        &self,
        user_id: &str,
    ) -> anyhow::Result<Vec<crate::auth::models::OauthIdentity>> {
        let conn = self.pool.get()?;
        let mut stmt = conn.prepare(
            "SELECT id, user_id, provider_id, external_subject, external_email, last_login_at, created_at
             FROM oauth_identities WHERE user_id=?1 ORDER BY created_at"
        )?;
        let rows = stmt.query_map(params![user_id], |row| {
            Ok(crate::auth::models::OauthIdentity {
                id: row.get(0)?,
                user_id: row.get(1)?,
                provider_id: row.get(2)?,
                external_subject: row.get(3)?,
                external_email: row.get(4)?,
                last_login_at: row.get(5)?,
                created_at: row.get(6)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_and_get_user() {
        let db = AuthDb::in_memory().unwrap();
        let user = db
            .create_user(
                "u1",
                "alice",
                "alice@example.com",
                "hash123",
                SystemRole::User,
            )
            .unwrap();
        assert_eq!(user.username, "alice");
        assert_eq!(user.role, SystemRole::User);
        assert!(user.is_active);

        let found = db.get_user_by_username("alice").unwrap().unwrap();
        assert_eq!(found.id, "u1");
        assert_eq!(found.role, SystemRole::User);
    }

    #[test]
    fn test_user_roles() {
        let db = AuthDb::in_memory().unwrap();
        let user = db
            .create_user(
                "u1",
                "admin",
                "admin@example.com",
                "hash",
                SystemRole::Admin,
            )
            .unwrap();
        assert!(user.is_admin());
        assert_eq!(user.role, SystemRole::Admin);

        db.update_user_role("u1", SystemRole::SuperAdmin).unwrap();
        let updated = db.get_user_by_id("u1").unwrap().unwrap();
        assert_eq!(updated.role, SystemRole::SuperAdmin);
    }

    #[test]
    fn test_org_and_group_hierarchy() {
        let db = AuthDb::in_memory().unwrap();
        db.create_user("u1", "alice", "alice@example.com", "hash", SystemRole::User)
            .unwrap();
        let org = db
            .create_organisation("o1", "Acme Corp", "acme", Some("A company"), None)
            .unwrap();
        assert_eq!(org.slug, "acme");

        db.add_org_member("u1", "o1", Role::Admin).unwrap();
        let members = db.list_org_members("o1").unwrap();
        assert_eq!(members.len(), 1);
        assert_eq!(members[0].1, Role::Admin);

        db.create_group("g1", "o1", "Engineering", None).unwrap();
        db.create_group("g2", "o1", "Backend", Some("g1")).unwrap();

        let groups = db.list_org_groups("o1").unwrap();
        assert_eq!(groups.len(), 2);

        let backend = db.get_group("g2").unwrap().unwrap();
        assert_eq!(backend.parent_group_id.as_deref(), Some("g1"));
    }

    fn sample_report(conforms: bool, n: usize) -> crate::shacl::report::ValidationReport {
        use crate::shacl::report::{Severity, ValidationReport, ValidationResult};
        let results: Vec<ValidationResult> = (0..n)
            .map(|i| ValidationResult {
                severity: Severity::Violation,
                focus_node: format!("urn:node:{i}"),
                path: None,
                value: None,
                source_shape: "urn:shape".into(),
                source_constraint: "sh:minCount 1".into(),
                message: "missing".into(),
            })
            .collect();
        ValidationReport {
            conforms,
            results_count: results.len(),
            results,
        }
    }

    #[test]
    fn test_validation_run_pruning() {
        let db = AuthDb::in_memory().unwrap();
        db.create_user("u1", "alice", "alice@example.com", "h", SystemRole::User)
            .unwrap();
        db.create_dataset(
            "d1",
            "DS",
            None,
            OwnerType::User,
            "u1",
            Visibility::Private,
            None,
        )
        .unwrap();
        for _ in 0..55 {
            let json = serde_json::to_string(&sample_report(false, 1)).unwrap();
            db.insert_validation_run("d1", false, 1, 1, 0, 0, &json, Some("u1"))
                .unwrap();
        }
        let runs = db.list_validation_run_summaries("d1", 100).unwrap();
        assert_eq!(
            runs.len(),
            50,
            "history should be capped at 50 runs per dataset"
        );
        assert!(db.get_latest_validation_run("d1").unwrap().is_some());
    }

    #[test]
    fn test_latest_run_per_dataset() {
        let db = AuthDb::in_memory().unwrap();
        db.create_user("u1", "alice", "alice@example.com", "h", SystemRole::User)
            .unwrap();
        db.create_dataset(
            "d1",
            "DS1",
            None,
            OwnerType::User,
            "u1",
            Visibility::Private,
            None,
        )
        .unwrap();
        db.create_dataset(
            "d2",
            "DS2",
            None,
            OwnerType::User,
            "u1",
            Visibility::Private,
            None,
        )
        .unwrap();

        let r1 = serde_json::to_string(&sample_report(true, 0)).unwrap();
        db.insert_validation_run("d1", true, 0, 0, 0, 0, &r1, None)
            .unwrap();
        std::thread::sleep(Duration::from_millis(3)); // ensure a strictly later run_timestamp
        let r2 = serde_json::to_string(&sample_report(false, 3)).unwrap();
        db.insert_validation_run("d1", false, 3, 3, 0, 0, &r2, None)
            .unwrap();
        let r3 = serde_json::to_string(&sample_report(false, 1)).unwrap();
        db.insert_validation_run("d2", false, 1, 1, 0, 0, &r3, None)
            .unwrap();

        let latest = db
            .list_latest_run_summaries(&["d1".to_string(), "d2".to_string()])
            .unwrap();
        assert_eq!(latest.len(), 2, "one latest run per dataset");
        let d1 = latest.iter().find(|r| r.dataset_id == "d1").unwrap();
        assert_eq!(d1.results_count, 3, "should reflect the most recent d1 run");
        assert!(!d1.conforms);
        let d2 = latest.iter().find(|r| r.dataset_id == "d2").unwrap();
        assert_eq!(d2.results_count, 1);

        assert!(db.list_latest_run_summaries(&[]).unwrap().is_empty());
    }

    #[test]
    fn test_dataset_access_control() {
        let db = AuthDb::in_memory().unwrap();
        db.create_user("u1", "alice", "alice@example.com", "hash", SystemRole::User)
            .unwrap();
        db.create_user("u2", "bob", "bob@example.com", "hash", SystemRole::User)
            .unwrap();
        db.create_organisation("o1", "Acme", "acme", None, None)
            .unwrap();
        db.add_org_member("u1", "o1", Role::Member).unwrap();

        // Public dataset — anyone can access
        let public_ds = db
            .create_dataset(
                "d1",
                "Public Data",
                None,
                OwnerType::Organisation,
                "o1",
                Visibility::Public,
                None,
            )
            .unwrap();
        assert!(db.can_access_dataset(None, &public_ds).unwrap());
        assert!(db.can_access_dataset(Some("u2"), &public_ds).unwrap());

        // Members-only — only org members
        let members_ds = db
            .create_dataset(
                "d2",
                "Members Data",
                None,
                OwnerType::Organisation,
                "o1",
                Visibility::Members,
                None,
            )
            .unwrap();
        assert!(!db.can_access_dataset(None, &members_ds).unwrap());
        assert!(db.can_access_dataset(Some("u1"), &members_ds).unwrap());
        assert!(!db.can_access_dataset(Some("u2"), &members_ds).unwrap());

        // Private — only explicit access
        let private_ds = db
            .create_dataset(
                "d3",
                "Private Data",
                None,
                OwnerType::Organisation,
                "o1",
                Visibility::Private,
                None,
            )
            .unwrap();
        assert!(!db.can_access_dataset(Some("u1"), &private_ds).unwrap());
        db.grant_dataset_access("d3", "u1").unwrap();
        assert!(db.can_access_dataset(Some("u1"), &private_ds).unwrap());
    }

    // The batched `list_accessible_datasets` must return exactly the same set as
    // the per-dataset `effective_dataset_role` path across every access vector,
    // so the optimization can never widen or narrow visibility.
    #[test]
    fn test_list_accessible_datasets_matches_per_dataset() {
        let db = AuthDb::in_memory().unwrap();
        db.create_user("admin", "admin", "a@e.com", "h", SystemRole::Admin)
            .unwrap();
        db.create_user("owner", "owner", "o@e.com", "h", SystemRole::User)
            .unwrap();
        db.create_user("orgmem", "orgmem", "m@e.com", "h", SystemRole::User)
            .unwrap();
        db.create_user("grpmem", "grpmem", "g@e.com", "h", SystemRole::User)
            .unwrap();
        db.create_user("granted", "granted", "gr@e.com", "h", SystemRole::User)
            .unwrap();
        db.create_user("outsider", "outsider", "x@e.com", "h", SystemRole::User)
            .unwrap();

        db.create_organisation("o1", "Acme", "acme", None, None)
            .unwrap();
        db.add_org_member("orgmem", "o1", Role::Member).unwrap();
        db.create_group("g1", "o1", "Eng", None).unwrap();
        db.add_group_member("grpmem", "g1", Role::Member).unwrap();

        // A dataset for every owner-type / visibility combination plus grants.
        db.create_dataset(
            "d_user_pub",
            "x",
            None,
            OwnerType::User,
            "owner",
            Visibility::Public,
            None,
        )
        .unwrap();
        db.create_dataset(
            "d_user_priv",
            "x",
            None,
            OwnerType::User,
            "owner",
            Visibility::Private,
            None,
        )
        .unwrap();
        db.create_dataset(
            "d_org_pub",
            "x",
            None,
            OwnerType::Organisation,
            "o1",
            Visibility::Public,
            None,
        )
        .unwrap();
        db.create_dataset(
            "d_org_mem",
            "x",
            None,
            OwnerType::Organisation,
            "o1",
            Visibility::Members,
            None,
        )
        .unwrap();
        db.create_dataset(
            "d_org_priv",
            "x",
            None,
            OwnerType::Organisation,
            "o1",
            Visibility::Private,
            None,
        )
        .unwrap();
        db.create_dataset(
            "d_grp_mem",
            "x",
            None,
            OwnerType::Group,
            "g1",
            Visibility::Members,
            None,
        )
        .unwrap();
        db.create_dataset(
            "d_grp_priv",
            "x",
            None,
            OwnerType::Group,
            "g1",
            Visibility::Private,
            None,
        )
        .unwrap();

        // Explicit grant to a non-member, legacy private allow-list entry.
        db.set_resource_grant(
            "dataset",
            "d_org_priv",
            "user",
            "granted",
            ResourceRole::Editor,
            "owner",
        )
        .unwrap();
        db.grant_dataset_access("d_user_priv", "granted").unwrap();
        // Group-scoped grant on an otherwise-inaccessible private dataset.
        db.set_resource_grant(
            "dataset",
            "d_grp_priv",
            "group",
            "g1",
            ResourceRole::Viewer,
            "owner",
        )
        .unwrap();

        for user in [
            None,
            Some("admin"),
            Some("owner"),
            Some("orgmem"),
            Some("grpmem"),
            Some("granted"),
            Some("outsider"),
        ] {
            let mut batched: Vec<String> = db
                .list_accessible_datasets(user)
                .unwrap()
                .into_iter()
                .map(|d| d.id)
                .collect();
            let mut per_dataset: Vec<String> = db
                .list_datasets()
                .unwrap()
                .into_iter()
                .filter(|d| db.effective_dataset_role(user, d).unwrap().is_some())
                .map(|d| d.id)
                .collect();
            batched.sort();
            per_dataset.sort();
            assert_eq!(batched, per_dataset, "mismatch for user {user:?}");
        }
    }

    #[test]
    fn test_resource_role_grants() {
        let db = AuthDb::in_memory().unwrap();
        db.create_user("owner", "owner", "o@example.com", "h", SystemRole::User)
            .unwrap();
        db.create_user("ed", "editor", "e@example.com", "h", SystemRole::User)
            .unwrap();
        db.create_user("vw", "viewer", "v@example.com", "h", SystemRole::User)
            .unwrap();
        db.create_user("out", "outsider", "x@example.com", "h", SystemRole::User)
            .unwrap();
        db.create_organisation("o1", "Acme", "acme", None, None)
            .unwrap();
        db.add_org_member("owner", "o1", Role::Admin).unwrap();
        db.add_org_member("ed", "o1", Role::Member).unwrap();
        db.add_org_member("vw", "o1", Role::Viewer).unwrap();

        let ds = db
            .create_dataset(
                "d1",
                "Data",
                None,
                OwnerType::Organisation,
                "o1",
                Visibility::Members,
                None,
            )
            .unwrap();

        // Member can modify data; viewer cannot; org admin manages.
        assert!(db.can_write_dataset("ed", &ds).unwrap());
        assert!(!db.can_write_dataset("vw", &ds).unwrap());
        assert!(db.can_manage_dataset("owner", &ds).unwrap());
        assert!(!db.can_manage_dataset("ed", &ds).unwrap());

        // A grant elevates the viewer to editor on this dataset only.
        db.set_resource_grant("dataset", "d1", "user", "vw", ResourceRole::Editor, "owner")
            .unwrap();
        assert!(db.can_write_dataset("vw", &ds).unwrap());

        // A grant restricts the editor to read-only on this dataset only.
        db.set_resource_grant("dataset", "d1", "user", "ed", ResourceRole::Viewer, "owner")
            .unwrap();
        assert!(!db.can_write_dataset("ed", &ds).unwrap());
        assert!(db.can_access_dataset(Some("ed"), &ds).unwrap());

        // A grant cannot demote an org admin (manage floor).
        db.set_resource_grant(
            "dataset",
            "d1",
            "user",
            "owner",
            ResourceRole::Viewer,
            "owner",
        )
        .unwrap();
        assert!(db.can_manage_dataset("owner", &ds).unwrap());

        // An outsider gets access only via an explicit grant.
        assert!(!db.can_access_dataset(Some("out"), &ds).unwrap());
        db.set_resource_grant(
            "dataset",
            "d1",
            "user",
            "out",
            ResourceRole::Editor,
            "owner",
        )
        .unwrap();
        assert!(db.can_write_dataset("out", &ds).unwrap());

        // Revoking returns the outsider to no access.
        db.revoke_resource_grant("dataset", "d1", "user", "out")
            .unwrap();
        assert!(!db.can_access_dataset(Some("out"), &ds).unwrap());

        assert_eq!(db.list_resource_grants("dataset", "d1").unwrap().len(), 3);
    }

    #[test]
    fn test_resource_grant_to_organisation() {
        let db = AuthDb::in_memory().unwrap();
        db.create_user("owner", "owner", "o@example.com", "h", SystemRole::User)
            .unwrap();
        db.create_user("partner", "partner", "p@example.com", "h", SystemRole::User)
            .unwrap();
        db.create_user(
            "outsider",
            "outsider",
            "x@example.com",
            "h",
            SystemRole::User,
        )
        .unwrap();

        // The owning org and a separate partner org the grant targets.
        db.create_organisation("o_own", "Owner Co", "owner-co", None, None)
            .unwrap();
        db.add_org_member("owner", "o_own", Role::Admin).unwrap();
        db.create_organisation("o_partner", "Partner Co", "partner-co", None, None)
            .unwrap();
        db.add_org_member("partner", "o_partner", Role::Member)
            .unwrap();

        // A private dataset owned by o_own — invisible to non-members by default.
        let ds = db
            .create_dataset(
                "d1",
                "Data",
                None,
                OwnerType::Organisation,
                "o_own",
                Visibility::Private,
                None,
            )
            .unwrap();
        assert!(!db.can_access_dataset(Some("partner"), &ds).unwrap());

        // Grant the *partner organisation* editor: every partner member can now write.
        db.set_resource_grant(
            "dataset",
            "d1",
            "organisation",
            "o_partner",
            ResourceRole::Editor,
            "owner",
        )
        .unwrap();
        assert!(db.can_write_dataset("partner", &ds).unwrap());
        // A user in no granted org/group still has no access.
        assert!(!db.can_access_dataset(Some("outsider"), &ds).unwrap());

        // Revoking the org grant removes access for its members.
        db.revoke_resource_grant("dataset", "d1", "organisation", "o_partner")
            .unwrap();
        assert!(!db.can_access_dataset(Some("partner"), &ds).unwrap());
    }

    #[test]
    fn test_api_tokens() {
        let db = AuthDb::in_memory().unwrap();
        db.create_user("u1", "alice", "alice@example.com", "hash", SystemRole::User)
            .unwrap();

        let token = db
            .create_api_token(
                "t1",
                "u1",
                "CI Token",
                "hash_abc",
                "ots_abc12",
                &[ApiScope::Read, ApiScope::Write],
                None,
            )
            .unwrap();
        assert_eq!(token.name, "CI Token");
        assert_eq!(token.scopes, vec![ApiScope::Read, ApiScope::Write]);

        let found = db.get_api_token_by_hash("hash_abc").unwrap().unwrap();
        assert_eq!(found.id, "t1");

        let tokens = db.list_api_tokens("u1").unwrap();
        assert_eq!(tokens.len(), 1);

        db.revoke_api_token("t1").unwrap();
        let revoked = db.get_api_token_by_id("t1").unwrap().unwrap();
        assert!(revoked.revoked);
    }

    #[test]
    fn test_refresh_tokens() {
        let db = AuthDb::in_memory().unwrap();
        db.create_user("u1", "alice", "alice@example.com", "hash", SystemRole::User)
            .unwrap();

        db.create_refresh_token("rt1", "u1", "hash_rt1", "2099-01-01T00:00:00Z", "fam1")
            .unwrap();

        let found = db.get_refresh_token_by_hash("hash_rt1").unwrap().unwrap();
        assert_eq!(found.user_id, "u1");
        assert_eq!(found.family_id.as_deref(), Some("fam1"));
        assert!(!found.revoked);

        db.revoke_refresh_token("rt1").unwrap();
        let revoked = db.get_refresh_token_by_hash("hash_rt1").unwrap().unwrap();
        assert!(revoked.revoked);
    }

    #[test]
    fn refresh_token_family_revocation_is_session_scoped() {
        // Two logins for the same user create two families (sessions). Revoking one
        // family must NOT revoke the other — that is the whole point of the fix:
        // a rotation race in one browser can't log the user out everywhere.
        let db = AuthDb::in_memory().unwrap();
        db.create_user("u1", "alice", "alice@example.com", "hash", SystemRole::User)
            .unwrap();
        // Session A: two rotations in family "famA".
        db.create_refresh_token("a1", "u1", "h_a1", "2099-01-01T00:00:00Z", "famA")
            .unwrap();
        db.create_refresh_token("a2", "u1", "h_a2", "2099-01-01T00:00:00Z", "famA")
            .unwrap();
        // Session B: one token in family "famB".
        db.create_refresh_token("b1", "u1", "h_b1", "2099-01-01T00:00:00Z", "famB")
            .unwrap();

        // The live head of family A is its newest token.
        assert_eq!(db.get_active_family_head("famA").unwrap().unwrap().id, "a2");

        // Revoke family A only.
        db.revoke_refresh_token_family("famA").unwrap();
        assert!(
            db.get_refresh_token_by_hash("h_a1")
                .unwrap()
                .unwrap()
                .revoked
        );
        assert!(
            db.get_refresh_token_by_hash("h_a2")
                .unwrap()
                .unwrap()
                .revoked
        );
        // Session B survives.
        assert!(
            !db.get_refresh_token_by_hash("h_b1")
                .unwrap()
                .unwrap()
                .revoked
        );
        assert!(db.get_active_family_head("famA").unwrap().is_none());
        assert_eq!(db.get_active_family_head("famB").unwrap().unwrap().id, "b1");
    }

    // ─── [CB13] login-failure increments are atomic and never lost ────────────

    // Each failed attempt must increment by exactly one. If increments were lost
    // (the pre-fix read-modify-write race), `LOGIN_LOCK_THRESHOLD` failures would
    // not be enough to lock the account. Driving exactly the threshold and
    // asserting the account flips locked proves no increment was dropped.
    #[test]
    fn test_record_login_failure_increments_reach_threshold() {
        let db = AuthDb::in_memory().unwrap();
        let user = "attacker-target";

        // One short of the threshold: still unlocked.
        for _ in 0..(AuthDb::LOGIN_LOCK_THRESHOLD - 1) {
            db.record_login_failure(user).unwrap();
            assert!(
                !db.is_login_locked(user).unwrap(),
                "should not lock before the threshold is crossed"
            );
        }

        // The threshold-th failure locks the account.
        db.record_login_failure(user).unwrap();
        assert!(
            db.is_login_locked(user).unwrap(),
            "exactly LOGIN_LOCK_THRESHOLD failures must lock the account (no lost increments)"
        );

        // A successful login clears the throttle.
        db.clear_login_attempts(user).unwrap();
        assert!(!db.is_login_locked(user).unwrap());
    }

    // Concurrent failed attempts against the same account must not lose updates.
    // The in-memory pool shares a single connection, so r2d2 serializes access;
    // combined with the BEGIN IMMEDIATE transaction this guarantees each spawned
    // attempt observes a fresh count. We assert the total reaches the lock.
    #[test]
    fn test_record_login_failure_concurrent_not_lost() {
        use std::sync::Arc;
        let db = Arc::new(AuthDb::in_memory().unwrap());
        let user = "concurrent-target";

        let mut handles = Vec::new();
        for _ in 0..AuthDb::LOGIN_LOCK_THRESHOLD {
            let db = Arc::clone(&db);
            let user = user.to_string();
            handles.push(std::thread::spawn(move || {
                db.record_login_failure(&user).unwrap();
            }));
        }
        for h in handles {
            h.join().unwrap();
        }

        assert!(
            db.is_login_locked(user).unwrap(),
            "concurrent failures totalling the threshold must still lock the account"
        );
    }

    // ─── [S16] mutating ops on a missing id must error, not silently succeed ──

    #[test]
    fn test_mutations_on_missing_id_return_err() {
        let db = AuthDb::in_memory().unwrap();

        // update_endpoint_acl_rule
        assert!(
            db.update_endpoint_acl_rule("no-such-id", "/x", "*", "allow", 0)
                .is_err(),
            "updating a non-existent endpoint ACL rule must error"
        );
        // delete_endpoint_acl_rule
        assert!(
            db.delete_endpoint_acl_rule("no-such-id").is_err(),
            "deleting a non-existent endpoint ACL rule must error"
        );
        // revoke_graph_permission
        assert!(
            db.revoke_graph_permission("no-such-id").is_err(),
            "revoking a non-existent graph ACL rule must error"
        );
        // delete_triple_security_label
        assert!(
            db.delete_triple_security_label("no-such-id").is_err(),
            "deleting a non-existent triple security label must error"
        );

        // Sanity: each method succeeds on a row that actually exists.
        db.create_endpoint_acl_rule("e1", "role", "user", "/api/*", "*", "allow", 0, "admin")
            .unwrap();
        db.update_endpoint_acl_rule("e1", "/api/v2/*", "GET", "deny", 5)
            .unwrap();
        db.delete_endpoint_acl_rule("e1").unwrap();

        db.grant_graph_permission("g1", "urn:graph:1", "user", "u1", "read", "admin")
            .unwrap();
        db.revoke_graph_permission("g1").unwrap();

        db.create_triple_security_label(
            "t1",
            "urn:s",
            "urn:p",
            "o",
            "urn:graph:1",
            "urn:label:secret",
        )
        .unwrap();
        db.delete_triple_security_label("t1").unwrap();
    }

    // ─── [S11] public graph grants: write-normalization + enforcement parity ──

    #[test]
    fn test_public_graph_grant_normalized_and_enforced() {
        let db = AuthDb::in_memory().unwrap();
        let graph = "urn:public:graph";

        // A public grant created with a non-'*' principal_id is canonicalized.
        let rule = db
            .grant_graph_permission("p1", graph, "public", "ignored", "read", "admin")
            .unwrap();
        assert_eq!(
            rule.principal_id, "*",
            "public grants must store principal_id='*'"
        );

        // It is both discoverable (listing) and enforceable (access check).
        let readable = db.get_graph_acl_readable_iris("u1", "user").unwrap();
        assert!(
            readable.contains(&graph.to_string()),
            "public grant must be discoverable"
        );
        assert!(
            db.check_graph_permission("u1", "user", graph, "read")
                .unwrap(),
            "public grant must grant read to any user"
        );
        // Anonymous callers (acl.rs passes an empty user_id and the role 'public') also pass.
        assert!(
            db.check_graph_permission("", "public", graph, "read")
                .unwrap(),
            "public grant must grant read to anonymous callers"
        );

        // Enforcement ignores principal_id for public, so even a *legacy* row whose
        // principal_id is not '*' (written before normalization existed) is honored,
        // matching get_graph_acl_readable_iris. Insert such a row directly to
        // bypass grant_graph_permission's normalization and exercise the check path.
        let legacy = "urn:public:legacy";
        {
            let conn = db.pool.get().unwrap();
            conn.execute(
                "INSERT INTO graph_acl (id, graph_iri, principal_type, principal_id, permission, created_at, created_by)
                 VALUES (?1,?2,?3,?4,?5,?6,?7)",
                params![
                    "p2",
                    legacy,
                    "public",
                    "legacy-id",
                    "write",
                    chrono::Utc::now().to_rfc3339(),
                    "admin"
                ],
            )
            .unwrap();
        }
        assert!(
            db.check_graph_permission("u2", "user", legacy, "read")
                .unwrap(),
            "legacy public grant (principal_id != '*') must satisfy a read requirement"
        );
        assert!(
            db.check_graph_permission("u2", "user", legacy, "write")
                .unwrap(),
            "write-level public grant must satisfy a write requirement"
        );
        assert!(
            db.get_graph_acl_readable_iris("u2", "user")
                .unwrap()
                .contains(&legacy.to_string()),
            "legacy public grant must be discoverable (parity with enforcement)"
        );
    }
}
