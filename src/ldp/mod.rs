//! Linked Data Platform (LDP) HTTP layer — W3C LDP 1.0 full implementation.
//!
//! Implements all W3C LDP 1.0 resource types and HTTP methods.
//!
//! # Resource types
//! - **`ldp:BasicContainer`** — ordered set of `ldp:contains` members.
//! - **`ldp:DirectContainer`** — adds `ldp:membershipResource` /
//!   `ldp:hasMemberRelation` / `ldp:insertedContentRelation`; membership
//!   triples written to the designated resource on POST.
//! - **`ldp:IndirectContainer`** — same as Direct, but the membership triple
//!   object is read from the new member's body via `ldp:insertedContentRelation`.
//! - **`ldp:NonRDFSource`** — arbitrary binary resources stored as
//!   base64-encoded triples and returned with their original `Content-Type`.
//! - **`ldp:RDFSource`** — plain RDF resource (non-container).
//!
//! # Routes (mounted under `/ldp/*path`)
//! ```text
//! GET     /ldp/*path  — fetch resource (content negotiation, Link headers, ETag, Prefer)
//! HEAD    /ldp/*path  — headers only
//! POST    /ldp/*path  — create member (Slug, 201 + Location, Direct/Indirect membership)
//! PUT     /ldp/*path  — replace resource (If-Match ETag, Turtle / JSON-LD / binary)
//! PATCH   /ldp/*path  — apply SPARQL Update (application/sparql-update, If-Match ETag)
//! DELETE  /ldp/*path  — remove resource + ldp:contains + membership triple (204)
//! OPTIONS /ldp/*path  — Allow + Accept-Post + Accept-Patch + Link
//! ```
//!
//! # Headers
//! - **Prefer**: `return=minimal` omits `ldp:contains` triples from GET body.
//! - **Preference-Applied**: echoed back in every GET response.
//! - **Constrained-By**: every response carries
//!   `Link: <{base}/ldp/constraints>; rel="http://www.w3.org/ns/ldp#constrainedBy"`.
//! - **Slug**: creation hint for POST; falls back to a UUID.
//! - **ETag** / **If-Match**: optimistic concurrency for PUT and PATCH.
//!
//! Enabled with the `ldp` Cargo feature.

pub mod container;
pub mod handler;
pub mod routes;

pub use routes::ldp_routes;
