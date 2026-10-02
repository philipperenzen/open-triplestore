//! Runner for the buildingSMART IDS 1.0 test corpus
//! (`Documentation/ImplementersDocumentation/TestCases` of
//! https://github.com/buildingSMART/IDS): 334 IDS + IFC pairs in nine
//! folders, each named `pass-`, `fail-` or `invalid-` after the outcome every
//! IDS implementation must reach on it.
//!
//! The corpus is **fetched at test time, never vendored**. It is licensed
//! CC BY-ND 4.0 (© buildingSMART International Ltd.), and its IFC files came
//! from IfcOpenShell's ifctester work, so the repository keeps only
//! `tests/fixtures/buildingsmart-ids/MANIFEST.sha256` — the path and SHA-256 of
//! every file at the pinned commit below — plus its PROVENANCE.md. The runner
//! downloads each file from that commit (or reuses an earlier download in
//! `target/buildingsmart-ids/<commit>/`, or `$OTS_IDS_CORPUS_DIR`), checks its
//! SHA-256 against the manifest, and refuses to run on any mismatch. Offline it
//! skips with a message, unless `OTS_TEST_IDS_CORPUS_REQUIRED=1` (set in CI),
//! which turns a failed download into a failure.
//!
//! Each case runs the path a user takes: the IFC file through the built-in IFC
//! lift, the IDS through the IDS importer (`POST /api/shacl/import/ids`), and
//! the resulting shapes through the SHACL validator over the lifted graph. The
//! outcome is satisfied when
//!
//! - `pass-`: the importer accepts the IDS and the lifted model conforms;
//! - `fail-`: the importer rejects the IDS, or the model does not conform;
//! - `invalid-`: the same as `fail-` (an invalid IDS "could not be satisfied,
//!   regardless of IFC contents"; rejecting it at import counts).
//!
//! These are development and regression results on the corpus, not a
//! buildingSMART certification, and no score is published from them (see
//! docs/conformance/ids.md).
//!
//! Gap policy (two-way ratchet, as in `w3c_shacl_conformance.rs`): every case
//! NOT in `KNOWN_FAILURES` must be satisfied and every listed case must still
//! be unsatisfied, so silent regressions and silent fixes both turn the suite
//! red. Run with `OTS_IDS_PRINT_FAILURES=1` to print the current list.

use open_triplestore::ifc::{convert, ConvertOptions};
use open_triplestore::shacl::validate;
use open_triplestore::spec_import::importer;
use open_triplestore::store::TripleStore;
use oxigraph::io::RdfFormat;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const CORPUS_REPO: &str = "buildingSMART/IDS";
const CORPUS_COMMIT: &str = "870f9c4e6e8f414e737b4d84ca1aa9b46fc6c8f3";
const CORPUS_DIR: &str = "Documentation/ImplementersDocumentation/TestCases";
const MANIFEST: &str = include_str!("fixtures/buildingsmart-ids/MANIFEST.sha256");

const SHAPES_GRAPH: &str = "urn:ids-corpus:shapes";
const DATA_GRAPH: &str = "http://example.org/ids-corpus/model/";

/// Cases that are currently unsatisfied, with the gap they sit behind. Keys
/// are `<folder>/<file stem>`. Keep sorted. Removing an entry requires the
/// case to be satisfied (the ratchet asserts both directions).
///
/// Baseline (develop @ 570a7a3, before any IDS work): 183 of 334 satisfied,
/// most of them vacuously — the building-topology lift leaves uncontained
/// elements out, so nothing is targeted and every specification conforms.
/// With typed values, negated prohibited facets and the existence check: 147,
/// and no longer vacuous — a required specification now fails when the
/// building-topology layer has none of the model's applicable elements.
const KNOWN_FAILURES: &[(&str, &str)] = &[
    ("attribute/pass-a_required_facet_checks_all_parameters_as_normal", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("attribute/pass-an_optional_attribute_passes_if_null", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("attribute/pass-an_optional_attribute_passes_if_specified", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("attribute/pass-attributes_referencing_an_object_should_pass", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("attribute/pass-attributes_should_check_strings_case_sensitively_1_2", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("attribute/pass-attributes_with_a_boolean_false_should_pass", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("attribute/pass-attributes_with_a_boolean_true_should_pass", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("attribute/pass-attributes_with_a_select_referencing_a_primitive_should_pass", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("attribute/pass-attributes_with_a_select_referencing_an_object_should_pass", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("attribute/pass-attributes_with_a_string_value_should_pass", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("attribute/pass-attributes_with_a_zero_duration_should_pass", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("attribute/pass-attributes_with_a_zero_number_have_meaning_and_should_pass", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("attribute/pass-booleans_must_be_specified_as_lowercase_strings_3_3", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("attribute/pass-dates_are_treated_as_strings_2_2", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("attribute/pass-durations_are_treated_as_strings_1_2", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("attribute/pass-globalids_are_treated_as_strings_and_not_expanded", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("attribute/pass-integers_follow_the_same_rules_as_numbers", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("attribute/pass-name_restrictions_will_match_any_result_1_3", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("attribute/pass-name_restrictions_will_match_any_result_2_3", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("attribute/pass-name_restrictions_will_match_any_result_3_3", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("attribute/pass-non_ascii_characters_are_treated_without_encoding", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("attribute/pass-numeric_values_are_checked_using_type_casting_1_4", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("attribute/pass-numeric_values_are_checked_using_type_casting_2_4", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("attribute/pass-numeric_values_are_checked_using_type_casting_3_4", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("attribute/pass-only_specifically_formatted_numbers_are_allowed_3_4", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("attribute/pass-only_specifically_formatted_numbers_are_allowed_4_4", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("attribute/pass-strict_numeric_checking_may_be_done_with_a_bounds_restriction", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("attribute/pass-typecast_checking_may_also_occur_within_enumeration_restrictions", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("attribute/pass-value_restrictions_may_be_used_1_3", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("attribute/pass-value_restrictions_may_be_used_2_3", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("classification/pass-a_classification_facet_with_no_data_matches_any_classification_2_2", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("classification/pass-a_required_facet_checks_all_parameters_as_normal", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("classification/pass-an_optional_classification_value_passes_if_null", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("classification/pass-an_optional_classification_value_passes_if_specified", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("classification/pass-both_system_and_value_must_match__all__not_any__if_specified_1_2", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("classification/pass-non_rooted_resources_that_have_external_classification_references_should_also_pass", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("classification/pass-occurrences_override_the_type_classification_per_system_1_3", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("classification/pass-occurrences_override_the_type_classification_per_system_3_3", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("classification/pass-restrictions_can_be_used_for_systems_2_2", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("classification/pass-restrictions_can_be_used_for_values_1_3", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("classification/pass-restrictions_can_be_used_for_values_2_3", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("classification/pass-systems_should_match_exactly_1_5", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("classification/pass-systems_should_match_exactly_3_5", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("classification/pass-systems_should_match_exactly_4_5", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("classification/pass-systems_should_match_exactly_5_5", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("classification/pass-values_match_subreferences_if_full_classifications_are_used__e_g__ef_25_10_should_match_ef_25_10_25__ef_25_10_30__etc_", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("classification/pass-values_should_match_exactly_if_lightweight_classifications_are_used", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("entity/pass-a_matching_entity_should_pass", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("entity/pass-a_matching_predefined_type_should_pass", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("entity/pass-a_predefined_type_may_specify_a_user_defined_element_type", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("entity/pass-a_predefined_type_may_specify_a_user_defined_object_type", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("entity/pass-a_predefined_type_may_specify_a_user_defined_process_type", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("entity/pass-an_matching_entity_should_pass_regardless_of_predefined_type", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("entity/pass-entities_can_be_specified_as_a_xsd_regex_pattern_2_2", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("entity/pass-entities_can_be_specified_as_an_enumeration_1_3", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("entity/pass-entities_can_be_specified_as_an_enumeration_2_3", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("entity/pass-in_ifc2x3_a_user_defined_airterminal_predefined_type_resolves_via_the_type_mapping_table_1_2", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("entity/pass-in_ifc2x3_an_airterminal_can_be_checked_by_name_via_the_type_mapping_table_1_2", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("entity/pass-in_ifc2x3_an_airterminal_predefined_type_resolves_via_the_type_mapping_table_1_2", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("entity/pass-in_ifc2x3_there_must_be_an_airterminal_per_the_type_mapping_table_1_2", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("entity/pass-inherited_predefined_types_should_pass", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("entity/pass-overridden_predefined_types_should_pass", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("entity/pass-restrictions_can_be_specified_for_the_predefined_type_1_3", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("entity/pass-restrictions_can_be_specified_for_the_predefined_type_2_3", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("entity/pass-userdefined_predefined_types_may_be_specified", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("ids/fail-a_minimal_ids_can_check_a_minimal_ifc_1_2", "an optional specification over an uncontained wall: nothing is targeted, so nothing fails"),
    ("ids/fail-a_specification_passes_only_if_all_requirements_pass_1_2", "an optional specification over an uncontained wall: nothing is targeted, so nothing fails"),
    ("ids/fail-prohibited_specifications_fails_if_the_applicability_matches", "the prohibited wall is uncontained, so the building-topology layer does not carry it"),
    ("ids/pass-required_specifications_need_at_least_one_applicable_entity_1_2", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("material/pass-a_layer_set_name_will_pass_a_value_check", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("material/pass-a_material_category_may_pass_the_value_check", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("material/pass-a_material_name_may_pass_the_value_check", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("material/pass-a_required_facet_checks_all_parameters_as_normal", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("material/pass-an_optional_material_passes_if_null", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("material/pass-an_optional_material_passes_if_specified", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("material/pass-any_constituent_category_in_a_constituent_set_will_pass_a_value_check", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("material/pass-any_constituent_name_in_a_constituent_set_will_pass_a_value_check", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("material/pass-any_layer_category_in_a_layer_set_will_pass_a_value_check", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("material/pass-any_layer_name_in_a_layer_set_will_pass_a_value_check", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("material/pass-any_material_category_in_a_constituent_set_will_pass_a_value_check", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("material/pass-any_material_category_in_a_layer_set_will_pass_a_value_check", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("material/pass-any_material_category_in_a_list_will_pass_a_value_check", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("material/pass-any_material_category_in_a_profile_set_will_pass_a_value_check", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("material/pass-any_material_name_in_a_constituent_set_will_pass_a_value_check", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("material/pass-any_material_name_in_a_layer_set_will_pass_a_value_check", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("material/pass-any_material_name_in_a_list_will_pass_a_value_check", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("material/pass-any_material_name_in_a_profile_set_will_pass_a_value_check", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("material/pass-any_profile_category_in_a_profile_set_will_pass_a_value_check", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("material/pass-any_profile_name_in_a_profile_set_will_pass_a_value_check", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("material/pass-elements_with_any_material_will_pass_an_empty_material_facet", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("material/pass-occurrences_can_inherit_materials_from_their_types", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("material/pass-occurrences_can_override_materials_from_their_types", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("partof/fail-a_prohibited_facet_returns_the_opposite_of_a_required_facet", "aggregation of elements into elements is not visible as a containing class here"),
    ("partof/fail-the_container_must_be_related_using_specified_relation_2_2", "the building-topology layer writes aggregation as bot:containsElement too, so containment and aggregation are not told apart"),
    ("partof/pass-a_group_entity_must_match_exactly_2_2", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("partof/pass-a_group_predefined_type_must_match_exactly_2_2", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("partof/pass-a_grouped_element_passes_a_group_relationship", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("partof/pass-a_required_facet_checks_all_parameters_as_normal", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("partof/pass-an_aggregate_entity_may_pass_any_ancestral_whole_passes", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("partof/pass-an_aggregate_may_specify_the_entity_of_the_whole_1_2", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("partof/pass-an_aggregate_may_specify_the_predefined_type_of_the_whole_1_2", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("partof/pass-any_contained_element_passes_a_containment_relationship_2_2", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("partof/pass-any_nested_part_passes_a_nest_relationship", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("partof/pass-nesting_may_be_indirect", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("partof/pass-the_aggregated_part_passes_an_aggregate_relationship", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("partof/pass-the_container_entity_must_match_exactly_2_2", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("partof/pass-the_container_predefined_type_must_match_exactly_2_2", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("partof/pass-the_containment_can_be_indirect_1_2", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("partof/pass-the_nest_entity_must_match_exactly_2_2", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("partof/pass-the_nest_predefined_type_must_match_exactly_2_2", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("property/fail-properties_can_be_associated_to_relevant_object_types", "type objects are not in the building-topology layer"),
    ("property/pass-a_name_check_will_match_any_property_with_any_string_value", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("property/pass-a_name_check_will_match_any_quantity_with_any_value", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("property/pass-a_number_specified_as_a_string_is_treated_as_a_string", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("property/pass-a_property_set_to_false_is_still_considered_a_value_and_will_pass_a_name_check", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("property/pass-a_property_set_to_true_will_pass_a_name_check", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("property/pass-a_required_facet_checks_all_parameters_as_normal", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("property/pass-a_zero_duration_will_pass", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("property/pass-all_matching_properties_must_satisfy_requirements_1_3", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("property/pass-all_matching_properties_must_satisfy_requirements_2_3", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("property/pass-all_matching_property_sets_must_satisfy_requirements_1_3", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("property/pass-all_matching_property_sets_must_satisfy_requirements_3_3", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("property/pass-an_optional_facet_always_passes_regardless_of_outcome_1_2", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("property/pass-an_optional_facet_always_passes_regardless_of_outcome_2_2", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("property/pass-any_matching_value_in_a_bounded_property_will_pass_1_4", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("property/pass-any_matching_value_in_a_bounded_property_will_pass_2_4", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("property/pass-any_matching_value_in_a_bounded_property_will_pass_3_4", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("property/pass-any_matching_value_in_a_list_property_will_pass_1_3", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("property/pass-any_matching_value_in_a_list_property_will_pass_2_3", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("property/pass-any_matching_value_in_a_table_property_will_pass_1_3", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("property/pass-any_matching_value_in_a_table_property_will_pass_2_3", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("property/pass-any_matching_value_in_an_enumerated_property_will_pass_1_3", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("property/pass-any_matching_value_in_an_enumerated_property_will_pass_2_3", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("property/pass-booleans_must_be_specified_as_lowercase_strings_2_3", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("property/pass-dates_are_treated_as_strings_1_2", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("property/pass-durations_are_treated_as_strings_2_2", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("property/pass-if_multiple_properties_are_matched__all_values_must_satisfy_requirements_1_2", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("property/pass-integer_values_are_checked_using_type_casting_1_4", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("property/pass-material_properties_are_supported_under_ifc2x3_via_extendedmaterialproperties", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("property/pass-material_properties_are_supported_under_ifc4_via_ifcmaterialproperties", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("property/pass-measures_are_used_to_specify_an_ifc_data_type_2_2", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("property/pass-non_ascii_characters_are_treated_without_encoding", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("property/pass-only_specifically_formatted_numbers_are_allowed_3_4", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("property/pass-only_specifically_formatted_numbers_are_allowed_4_4", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("property/pass-predefined_properties_are_supported_but_discouraged_1_2", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("property/pass-project_properties_are_supported_under_ifc2x3_via_ifcobject", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("property/pass-project_properties_are_supported_under_ifc4_via_ifccontext", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("property/pass-properties_can_be_inherited_from_the_type_1_2", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("property/pass-properties_can_be_inherited_from_the_type_2_2", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("property/pass-properties_can_be_overriden_by_an_occurrence_1_2", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("property/pass-real_values_are_checked_using_type_casting_1_3", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("property/pass-real_values_are_checked_using_type_casting_2_3", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("property/pass-real_values_are_checked_using_type_casting_3_3", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("property/pass-specifying_a_value_performs_a_case_sensitive_match_1_2", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("property/pass-unit_conversions_shall_take_place_to_ids_nominated_standard_units_2_2", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("restriction/invalid-patterns_always_fail_on_any_number", "no IDS audit: a pattern on a number is accepted"),
    ("restriction/pass-a_bound_can_be_exclusive_2_3", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("restriction/pass-a_bound_can_be_inclusive_1_4", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("restriction/pass-a_bound_can_be_inclusive_2_4", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("restriction/pass-a_bound_can_be_inclusive_3_4", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("restriction/pass-an_enumeration_matches_case_sensitively_1_3", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("restriction/pass-an_enumeration_matches_case_sensitively_2_3", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("restriction/pass-length_checks_can_be_used_2_2", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("restriction/pass-max_and_min_length_checks_can_be_used_2_3", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("restriction/pass-max_and_min_length_checks_can_be_used_3_3", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("restriction/pass-regex_patterns_can_be_used_1_3", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("restriction/pass-regex_patterns_can_be_used_2_3", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("restriction/pass-regex_patterns_work_in_OR_1_3", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("restriction/pass-regex_patterns_work_in_OR_2_3", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("tolerance/pass-comparison_tolerance_for_floating_point_negative_high_number_lower_bound", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("tolerance/pass-comparison_tolerance_for_floating_point_negative_high_number_upper_bound", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("tolerance/pass-comparison_tolerance_for_floating_point_negative_low_number_lower_bound", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("tolerance/pass-comparison_tolerance_for_floating_point_negative_low_number_upper_bound", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("tolerance/pass-comparison_tolerance_for_floating_point_negative_one_lower_bound", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("tolerance/pass-comparison_tolerance_for_floating_point_negative_one_upper_bound", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("tolerance/pass-comparison_tolerance_for_floating_point_one_lower_bound", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("tolerance/pass-comparison_tolerance_for_floating_point_one_upper_bound", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("tolerance/pass-comparison_tolerance_for_floating_point_positive_high_number_lower_bound", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("tolerance/pass-comparison_tolerance_for_floating_point_positive_high_number_upper_bound", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("tolerance/pass-comparison_tolerance_for_floating_point_positive_low_number_lower_bound", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("tolerance/pass-comparison_tolerance_for_floating_point_positive_low_number_upper_bound", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("tolerance/pass-comparison_tolerance_for_floating_point_range_greater_than_zero_exclusive", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("tolerance/pass-comparison_tolerance_for_floating_point_range_greater_than_zero_inclusive", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("tolerance/pass-comparison_tolerance_for_floating_point_range_lower_than_zero_exclusive", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("tolerance/pass-comparison_tolerance_for_floating_point_range_lower_than_zero_inclusive", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("tolerance/pass-comparison_tolerance_for_floating_point_zero_lower_bound", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
    ("tolerance/pass-comparison_tolerance_for_floating_point_zero_upper_bound", "the required specification's existence check fails: the building-topology lift does not carry this model's uncontained elements, attributes or type objects"),
];

#[derive(Debug, Clone, Copy, PartialEq)]
enum Expect {
    Pass,
    Fail,
    Invalid,
}

struct Case {
    key: String,
    expect: Expect,
    ids: PathBuf,
    ifc: PathBuf,
}

fn manifest() -> Vec<(String, String)> {
    MANIFEST
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let (hash, path) = l.split_once("  ").expect("`<sha256>  <path>` lines");
            (hash.to_string(), path.to_string())
        })
        .collect()
}

fn cache_dir() -> PathBuf {
    match std::env::var_os("OTS_IDS_CORPUS_DIR") {
        Some(d) => PathBuf::from(d),
        None => Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("buildingsmart-ids")
            .join(CORPUS_COMMIT),
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// Make every manifest file present and verified under `dir`. `Err` means the
/// corpus could not be fetched (offline, or the host refused); a file whose
/// bytes do not match the manifest is a panic, never a skip.
fn ensure_corpus(dir: &Path) -> Result<(), String> {
    let entries = manifest();
    let mut missing = Vec::new();
    for (hash, rel) in &entries {
        let path = dir.join(rel);
        match std::fs::read(&path) {
            Ok(bytes) if sha256_hex(&bytes) == *hash => {}
            Ok(_) => {
                // A stale or corrupted cache entry: fetch it again.
                let _ = std::fs::remove_file(&path);
                missing.push((hash.clone(), rel.clone()));
            }
            Err(_) => missing.push((hash.clone(), rel.clone())),
        }
    }
    if missing.is_empty() {
        return Ok(());
    }
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("runtime: {e}"))?;
    let dir = dir.to_path_buf();
    rt.block_on(async move {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .map_err(|e| format!("http client: {e}"))?;
        let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(16));
        let mut tasks = tokio::task::JoinSet::new();
        for (hash, rel) in missing {
            let client = client.clone();
            let sem = sem.clone();
            let dir = dir.clone();
            tasks.spawn(async move {
                let _permit = sem.acquire_owned().await.map_err(|e| e.to_string())?;
                let url = format!(
                    "https://raw.githubusercontent.com/{CORPUS_REPO}/{CORPUS_COMMIT}/{CORPUS_DIR}/{rel}"
                );
                let resp = client
                    .get(&url)
                    .send()
                    .await
                    .map_err(|e| format!("{url}: {e}"))?;
                if !resp.status().is_success() {
                    return Err(format!("{url}: HTTP {}", resp.status()));
                }
                let bytes = resp.bytes().await.map_err(|e| format!("{url}: {e}"))?;
                let got = sha256_hex(&bytes);
                assert_eq!(
                    got, hash,
                    "{rel}: the downloaded bytes do not match the pinned SHA-256 \
                     (tests/fixtures/buildingsmart-ids/MANIFEST.sha256) — refusing to run on them"
                );
                let path = dir.join(&rel);
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                }
                std::fs::write(&path, &bytes).map_err(|e| e.to_string())?;
                Ok::<(), String>(())
            });
        }
        while let Some(r) = tasks.join_next().await {
            match r {
                Ok(Ok(())) => {}
                Ok(Err(e)) => return Err(e),
                Err(e) if e.is_panic() => std::panic::resume_unwind(e.into_panic()),
                Err(e) => return Err(e.to_string()),
            }
        }
        Ok(())
    })
}

fn cases(dir: &Path) -> Vec<Case> {
    let mut stems: BTreeMap<String, (bool, bool)> = BTreeMap::new();
    for (_, rel) in manifest() {
        if let Some(stem) = rel.strip_suffix(".ids") {
            stems.entry(stem.to_string()).or_default().0 = true;
        } else if let Some(stem) = rel.strip_suffix(".ifc") {
            stems.entry(stem.to_string()).or_default().1 = true;
        }
    }
    stems
        .into_iter()
        .map(|(stem, (has_ids, has_ifc))| {
            assert!(
                has_ids && has_ifc,
                "{stem}: the corpus pairs every IDS with an IFC"
            );
            let name = stem.rsplit('/').next().unwrap_or(&stem);
            let expect = if name.starts_with("pass-") {
                Expect::Pass
            } else if name.starts_with("fail-") {
                Expect::Fail
            } else if name.starts_with("invalid-") {
                Expect::Invalid
            } else {
                panic!("{stem}: unknown outcome prefix");
            };
            Case {
                ids: dir.join(format!("{stem}.ids")),
                ifc: dir.join(format!("{stem}.ifc")),
                key: stem,
                expect,
            }
        })
        .collect()
}

/// What the platform made of one case: `Rejected` (the importer refused the
/// IDS) or the validator's verdict on the lifted model.
enum Verdict {
    Rejected(String),
    Conforms,
    Violates(usize),
}

impl std::fmt::Display for Verdict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Verdict::Rejected(why) => write!(f, "rejected at import: {why}"),
            Verdict::Conforms => write!(f, "conforms"),
            Verdict::Violates(n) => write!(f, "{n} violation(s)"),
        }
    }
}

fn run_case(case: &Case) -> Result<Verdict, String> {
    let ids = std::fs::read(&case.ids).map_err(|e| format!("read ids: {e}"))?;
    let ifc = std::fs::read_to_string(&case.ifc).map_err(|e| format!("read ifc: {e}"))?;
    let imported = match importer("ids").expect("ids importer").import(&ids) {
        Ok(i) => i,
        Err(e) => return Ok(Verdict::Rejected(e.to_string())),
    };
    let store = TripleStore::in_memory().map_err(|e| format!("store: {e}"))?;
    let mut data = String::new();
    let mut owl = String::new();
    convert(
        &ifc,
        &ConvertOptions {
            inst_base: DATA_GRAPH.to_string(),
            ..Default::default()
        },
        &mut |c| data.push_str(c),
        &mut |c| owl.push_str(c),
    )
    .map_err(|e| format!("ifc lift: {e}"))?;
    store
        .load_str(&data, RdfFormat::NTriples, Some(DATA_GRAPH))
        .map_err(|e| format!("load data: {e}"))?;
    store
        .load_str(&imported.turtle, RdfFormat::Turtle, Some(SHAPES_GRAPH))
        .map_err(|e| format!("load shapes: {e}\n{}", imported.turtle))?;
    let report = validate(&store, SHAPES_GRAPH, &[DATA_GRAPH.to_string()])
        .map_err(|e| format!("validate: {e}"))?;
    Ok(if report.conforms {
        Verdict::Conforms
    } else {
        Verdict::Violates(report.results_count)
    })
}

fn satisfied(expect: Expect, verdict: &Verdict) -> bool {
    match (expect, verdict) {
        (Expect::Pass, Verdict::Conforms) => true,
        (Expect::Pass, _) => false,
        (Expect::Fail | Expect::Invalid, Verdict::Conforms) => false,
        (Expect::Fail | Expect::Invalid, _) => true,
    }
}

#[test]
fn buildingsmart_ids_corpus() {
    let dir = cache_dir();
    if let Err(e) = ensure_corpus(&dir) {
        assert!(
            std::env::var_os("OTS_TEST_IDS_CORPUS_REQUIRED").is_none(),
            "OTS_TEST_IDS_CORPUS_REQUIRED is set but the buildingSMART IDS corpus could not be fetched: {e}"
        );
        eprintln!(
            "SKIP: the buildingSMART IDS corpus could not be fetched ({e}); it is downloaded from \
             github.com/{CORPUS_REPO} at {CORPUS_COMMIT} on first run"
        );
        return;
    }

    let all = cases(&dir);
    assert_eq!(all.len(), 334, "the pinned corpus has 334 cases");
    let mut unexpected_failures = Vec::new();
    let mut unexpected_passes = Vec::new();
    let mut still_failing = Vec::new();
    let mut seen_known = 0usize;
    let mut satisfied_count = 0usize;
    for case in &all {
        let known = KNOWN_FAILURES.iter().find(|(k, _)| *k == case.key);
        if known.is_some() {
            seen_known += 1;
        }
        let (ok, detail) = match run_case(case) {
            Ok(v) => (satisfied(case.expect, &v), v.to_string()),
            Err(e) => (false, format!("error: {e}")),
        };
        if ok {
            satisfied_count += 1;
            if let Some((k, why)) = known {
                unexpected_passes.push(format!("{k} (listed as: {why})"));
            }
        } else {
            still_failing.push((case.key.clone(), detail.clone()));
            if known.is_none() {
                unexpected_failures.push(format!("{}: {detail}", case.key));
            }
        }
    }
    println!(
        "buildingSMART IDS corpus @ {}: {satisfied_count} satisfied, {} known-unsatisfied, {} cases",
        &CORPUS_COMMIT[..7],
        KNOWN_FAILURES.len(),
        all.len()
    );
    if std::env::var_os("OTS_IDS_PRINT_FAILURES").is_some() {
        for (k, d) in &still_failing {
            let d: String = d.chars().take(160).collect();
            println!(
                "    (\"{k}\", \"{}\"),",
                d.replace('\\', "\\\\").replace('"', "\\\"")
            );
        }
    }
    assert_eq!(
        seen_known,
        KNOWN_FAILURES.len(),
        "every KNOWN_FAILURES key must name a corpus case (stale entries?)"
    );
    assert!(
        unexpected_failures.is_empty(),
        "IDS corpus cases unsatisfied that are not in KNOWN_FAILURES:\n  {}",
        unexpected_failures.join("\n  ")
    );
    assert!(
        unexpected_passes.is_empty(),
        "KNOWN_FAILURES entries are now satisfied — remove them to ratchet forward:\n  {}",
        unexpected_passes.join("\n  ")
    );
}

/// The manifest pins the whole corpus: every case's IDS and IFC, nothing
/// else, with well-formed SHA-256 digests. Runs offline.
#[test]
fn the_manifest_pins_every_corpus_file() {
    let entries = manifest();
    assert_eq!(entries.len(), 668, "334 IDS + IFC pairs");
    for (hash, rel) in &entries {
        assert_eq!(hash.len(), 64, "{rel}");
        assert!(hash.bytes().all(|b| b.is_ascii_hexdigit()), "{rel}");
        assert!(
            rel.ends_with(".ids") || rel.ends_with(".ifc"),
            "{rel}: only test-case files are pinned"
        );
        assert!(!rel.contains(".."), "{rel}");
    }
    let mut sorted = entries.iter().map(|(_, r)| r.clone()).collect::<Vec<_>>();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), entries.len(), "no duplicate paths");
}
