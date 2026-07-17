//! Authentication, authorization, and identity.
//!
//! Brings together the SQLite-backed user/identity store ([`db`]), password
//! hashing ([`password`], Argon2id), session and API-token issuance ([`jwt`]),
//! OAuth 2.0 / OIDC and SAML single sign-on ([`oauth`], [`oauth_handlers`],
//! [`saml`]), request middleware ([`middleware`]), endpoint and graph-level
//! access control ([`acl`], [`acl_handlers`]), and the append-only [`audit`]
//! trail. HTTP handlers for the `/api/auth/*` surface live in [`handlers`].

pub mod acl;
pub mod acl_handlers;
pub mod audit;
pub mod authz;
pub mod dataset_audit;
pub mod dataset_graph;
pub mod db;
pub mod handlers;
pub mod jwt;
pub mod middleware;
pub mod models;
pub mod oauth;
pub mod oauth_handlers;
pub mod oidc_rs;
pub mod org_graph;
pub mod passkey;
pub mod password;
pub mod saml;
pub mod secret;
pub mod totp;
pub mod user_graph;
pub mod validate;
