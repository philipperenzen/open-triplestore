//! SHACL Studio — the consolidated workspace's backend foundation:
//!
//! * [`models`] — `ShapeGraph`, `ValidationPipeline`, `PipelineRun` entities.
//! * [`store`] — SQLite persistence (reuses the auth DB pool).
//! * [`access`] — visibility / manage checks shared by handlers.
//! * [`bindings`] — the RDF *validation layer*: shape↔target bindings +
//!   the dynamic `effective_shape_graphs_for_dataset` inheritance resolver.
//! * [`run`] — multi-shape-graph validation + report merging + facet analysis.
//! * [`exec`] — execute a pipeline against the live store and persist a run.
//! * [`introspect`] — model-context (classes/properties present) and the
//!   draft-from-data shape inducer.
//! * [`manifest`] — the form-manifest contract publisher (no in-app forms).
//! * [`gate`] — write-gating helper called by the on-write hook.
//! * [`cron`] — minimal 5-field cron matcher for the scheduler.
//! * [`scheduler`] — background task firing due pipelines.
//! * [`migrate`] — one-time legacy-shapes-graph → ShapeGraph import.
//! * [`seed`] — idempotent built-in SHACL-SHACL meta-shapes.
//! * [`handlers`] / [`routes`] — axum wiring.

pub mod access;
pub mod bindings;
pub mod catalog;
pub mod cron;
pub mod exec;
pub mod gate;
pub mod handlers;
pub mod introspect;
pub mod manifest;
pub mod migrate;
pub mod models;
pub mod report_rdf;
pub mod routes;
pub mod run;
pub mod scheduler;
pub mod seed;
pub mod seed_standards;
pub mod store;
