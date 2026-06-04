//! Seed the bundled public "Open Triplestore" demo organisation.
//!
//! On first run this creates a public organisation that owns one dataset per
//! standards category (Core RDF & SPARQL, Reasoning, Spatial, Validation, Rules,
//! Linked Data & Catalog, Capabilities). Each dataset holds one named graph per
//! standard — loaded through a mix of RDF serializations (Turtle, N-Triples,
//! RDF/XML, JSON-LD, and a SPARQL-star update) to exercise the parsers — plus
//! saved queries that demonstrate every query-able standard. The data and query
//! definitions live in [`super::seed_data`].
//!
//! Protocol and auth standards (SPARQL Update, Graph Store HTTP, Service
//! Description, LDP, DCAT/VoID, SHACL-C, JWT, OAuth/OIDC) can't be shown with a
//! SPARQL query; they are advertised in the `capabilities` dataset and verified
//! by the e2e suite against the live endpoints.
//!
//! Idempotent: skips entirely once the `open-triplestore` organisation exists,
//! so it never duplicates on restart. Best-effort — a failure loading one graph
//! or creating one service is logged and skipped without aborting the rest. Opt
//! out with `SEED_STANDARDS_DEMO=false`.

use bytes::Bytes;
use uuid::Uuid;

use crate::auth::models::{OwnerType, Role, SystemRole, Visibility};
use crate::auth::{dataset_graph, org_graph};
use crate::server::AppState;

use super::metadata;
use super::models::QueryScope;
use super::seed_data::{self, Fmt, GraphSpec, DEMO_BASE, ORG_NAME, ORG_SLUG};
use super::store::SavedQueryStore;

const ORG_DESCRIPTION: &str =
    "Public reference deployment showcasing every standard Open Triplestore implements: \
     RDF 1.1/1.2, SPARQL 1.1/1.2, RDFS, OWL 2 (QL/EL/RL/DL), GeoSPARQL, SHACL, ShEx, SWRL, \
     LDP and DCAT — each with a browsable dataset and runnable saved queries.";

/// Branded artwork for the bundled demo organisation, applied at seed time so
/// the public org page ships with a logo + header instead of a blank card. The
/// mark is the Open Triplestore knowledge-graph motif (teal "O" ring + three
/// nodes); PNG, because the image-upload allow-list rejects SVG. Regenerate
/// with `scripts/gen_org_brand.py`. Bundled into the binary like the seed docs.
const ORG_LOGO_PNG: &[u8] = include_bytes!("../../docs/assets/org-logo.png");
const ORG_BANNER_PNG: &[u8] = include_bytes!("../../docs/assets/org-banner.png");

/// Entry point — best-effort, never panics out of startup.
pub fn seed_open_triplestore(state: &AppState) {
    let disabled = std::env::var("SEED_STANDARDS_DEMO")
        .map(|v| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "false" | "0" | "no" | "off"
            )
        })
        .unwrap_or(false);
    if disabled {
        return;
    }
    if let Err(e) = try_seed(state) {
        tracing::warn!("open-triplestore demo seed skipped: {e}");
    }
}

fn try_seed(state: &AppState) -> anyhow::Result<()> {
    // Idempotent: if the demo organisation already exists we don't reseed it,
    // but we do back-fill branding when an earlier (pre-branding) seed left it
    // without a logo or banner — without ever clobbering artwork an admin has
    // since uploaded.
    if let Some(existing) = state.auth_db.get_organisation_by_slug(ORG_SLUG)? {
        seed_org_branding(
            state,
            &existing.id,
            existing.image_key.is_none(),
            existing.banner_key.is_none(),
        );
        return Ok(());
    }

    // Owner = a super_admin (preferred) or any admin. On a brand-new install
    // before any admin exists, skip quietly — it will seed on the next start.
    let users = state.auth_db.list_users()?;
    let owner = users
        .iter()
        .find(|u| matches!(u.role, SystemRole::SuperAdmin) && u.is_active)
        .or_else(|| users.iter().find(|u| u.role.is_admin() && u.is_active));
    let owner = match owner {
        Some(u) => u,
        None => {
            tracing::info!("open-triplestore seed: no admin user yet; will retry on next start");
            return Ok(());
        }
    };

    // Create the public organisation and attach the seeding admin to it.
    let org_id = Uuid::new_v4().to_string();
    let org = state.auth_db.create_organisation(
        &org_id,
        ORG_NAME,
        ORG_SLUG,
        Some(ORG_DESCRIPTION),
        None,
    )?;
    let _ = state
        .auth_db
        .add_org_member(&owner.id, &org_id, Role::Admin);
    org_graph::write_org_metadata_graph(&state.store, &state.base_url, &org, &[]);

    // Give the public org page a branded logo + banner out of the box.
    seed_org_branding(state, &org_id, true, true);

    let sq = SavedQueryStore::new(state.auth_db.pool());
    let mut graph_count = 0usize;
    let mut service_count = 0usize;

    for ds in seed_data::datasets() {
        // Use the curated, URL-safe slug as the dataset id so the minted IRI
        // (`{base}/dataset/{id}`) is human-readable — e.g. `…/dataset/spatial`
        // rather than `…/dataset/<uuid>`. Slugs are unique across the demo set.
        let ds_id = ds.slug.to_string();
        if let Err(e) = state.auth_db.create_dataset(
            &ds_id,
            ds.name,
            Some(ds.description),
            OwnerType::Organisation,
            &org_id,
            Visibility::Public,
            None,
        ) {
            tracing::warn!("open-triplestore seed: dataset '{}' failed: {e}", ds.slug);
            continue;
        }

        for g in ds.graphs {
            let graph_iri = format!("{DEMO_BASE}/{}/{}", ds.slug, g.suffix);
            if let Err(e) = load_graph(state, &graph_iri, g) {
                tracing::warn!("open-triplestore seed: graph <{graph_iri}> load failed: {e}");
                continue;
            }
            let _ = state.auth_db.add_dataset_graph(&ds_id, &graph_iri);
            let _ = state
                .auth_db
                .set_dataset_graph_role(&ds_id, &graph_iri, Some(g.role));
            graph_count += 1;
        }

        // Project the dataset's DCAT/VoID metadata graph so it is discoverable
        // as linked data (mirrors what the bulk-import path does).
        if let Ok(Some(dsrec)) = state.auth_db.get_dataset(&ds_id) {
            let entries = state
                .auth_db
                .list_dataset_graph_entries(&ds_id)
                .unwrap_or_default();
            dataset_graph::write_dataset_metadata_graph(
                &state.store,
                &state.base_url,
                &dsrec,
                &entries,
            );
        }

        for req in seed_data::services_for(ds.slug) {
            match sq.create(QueryScope::Dataset, &ds_id, &req, &owner.id) {
                Ok(svc) => {
                    metadata::record_service(&state.store, &state.base_url, &svc);
                    metadata::record_revision(
                        &state.store,
                        &state.base_url,
                        &svc.id,
                        svc.current_revision,
                        req.version_name.as_deref(),
                        req.note.as_deref(),
                        svc.sparql.as_deref().unwrap_or(&req.sparql),
                        "manual",
                        &svc.created_by,
                        &svc.created_at,
                    );
                    service_count += 1;
                }
                Err(e) => {
                    tracing::warn!("open-triplestore seed: service '{}' failed: {e}", req.name)
                }
            }
        }
    }

    #[cfg(feature = "text-search")]
    state.mark_text_dirty();

    tracing::info!(
        "Seeded '{}' organisation ({}) with {} datasets, {} graphs, {} saved queries",
        ORG_NAME,
        org_id,
        seed_data::datasets().len(),
        graph_count,
        service_count
    );
    Ok(())
}

/// Upload the bundled logo/banner for the demo organisation and record their
/// object keys (the same keys the upload endpoints use, so the existing
/// `GET /api/organisations/:id/{image,banner}` handlers serve them). `do_logo`
/// / `do_banner` let the caller back-fill only what's missing. Best-effort:
/// it skips silently when object storage isn't configured and logs (without
/// aborting the seed) on any failure.
fn seed_org_branding(state: &AppState, org_id: &str, do_logo: bool, do_banner: bool) {
    if !state.object_store.is_configured() || (!do_logo && !do_banner) {
        return;
    }
    // The seed runs on a blocking thread (spawn_blocking), so drive the async
    // object-store uploads to completion on the current runtime handle.
    let rt = tokio::runtime::Handle::current();
    let assets: [(bool, &str, &'static [u8]); 2] = [
        (do_logo, "org-images", ORG_LOGO_PNG),
        (do_banner, "org-banners", ORG_BANNER_PNG),
    ];
    for (wanted, prefix, bytes) in assets {
        if !wanted {
            continue;
        }
        let key = format!("{prefix}/{org_id}.png");
        if let Err(e) =
            rt.block_on(state.object_store.upload(&key, Bytes::from_static(bytes), "image/png"))
        {
            tracing::warn!("open-triplestore seed: branding upload to '{key}' failed: {e}");
            continue;
        }
        let recorded = if prefix == "org-images" {
            state.auth_db.update_org_image(org_id, Some(&key))
        } else {
            state.auth_db.update_org_banner(org_id, Some(&key))
        };
        if let Err(e) = recorded {
            tracing::warn!("open-triplestore seed: recording branding key '{key}' failed: {e}");
        }
    }
}

/// Load one graph's bundled data into its named graph. Quoted-triple data is
/// loaded through a SPARQL-star `INSERT DATA`; everything else goes through the
/// Graph Store PUT path in its declared serialization.
fn load_graph(state: &AppState, graph_iri: &str, g: &GraphSpec) -> anyhow::Result<()> {
    match g.fmt {
        Fmt::SparqlStarUpdate => {
            state
                .store
                .update(&format!(
                    "INSERT DATA {{ GRAPH <{graph_iri}> {{ {} }} }}",
                    g.data
                ))
                .map_err(|e| anyhow::anyhow!("{e}"))?;
        }
        other => {
            let format = other
                .rdf_format()
                .expect("non-update formats always map to an RdfFormat");
            state
                .store
                .graph_store_put(Some(graph_iri), g.data, format)
                .map_err(|e| anyhow::anyhow!("{e}"))?;
        }
    }
    Ok(())
}
