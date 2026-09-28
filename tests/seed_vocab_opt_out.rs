//! `SEED_STANDARD_VOCABS=false` stops seeding, not the licence duties on a
//! copy an earlier start seeded of a vocabulary whose licence allows no
//! altered copies: IMBOR is still checked, and repaired, on every start.
//!
//! One test per binary: it sets a process-wide environment variable.

mod common;

use common::*;
use open_triplestore::data_models::{content_digest, registry, seed_vocab, vocab_files};

#[test]
fn seeding_off_still_verifies_and_repairs_imbor() {
    let state = test_state();
    std::env::remove_var("SEED_STANDARD_VOCABS");
    seed_vocab::seed_standard_vocabularies(&state);
    let base = state.base_url.to_string();
    let graph = registry::version_record_iri(&base, "imbor", "2025");
    let removed = "<https://data.crow.nl/imbor/term/74e825e1-9b93-4dd5-8fab-52783fdb758b> \
                   <http://www.w3.org/2004/02/skos/core#prefLabel> \"Vaart\"@nl";
    state
        .store
        .update(&format!(
            "DELETE DATA {{ GRAPH <{graph}> {{ {removed} }} }}"
        ))
        .unwrap();

    std::env::set_var("SEED_STANDARD_VOCABS", "false");
    assert_eq!(seed_vocab::seed_standard_vocabularies(&state), 0);
    std::env::remove_var("SEED_STANDARD_VOCABS");

    let stored = content_digest::graph_triples(&state.store, &graph).unwrap();
    let file = content_digest::as_stored(&content_digest::quads_as_triples(
        &open_triplestore::data_models::upload::parse_rdf(
            vocab_files::IMBOR.ttl.as_bytes(),
            "text/turtle",
            "imbor.ttl",
        )
        .unwrap(),
    ))
    .unwrap();
    assert!(
        content_digest::same_triples(&stored, &file),
        "CROW's file again"
    );
    let a = registry::get_attribution(&state.store, &graph).unwrap();
    assert!(a.unchanged && a.no_derivatives);
    let kept = registry::get_version(&state.store, &base, "imbor", "2025-kept-1")
        .expect("the altered copy is kept aside");
    assert!(kept.created_by.is_none());
    assert_eq!(
        content_digest::graph_triples(&state.store, &kept.graph_iri)
            .unwrap()
            .len(),
        file.len() - 1
    );
}
