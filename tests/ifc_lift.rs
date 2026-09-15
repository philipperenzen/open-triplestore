//! The IFC → linked-data lift over three hand-authored STEP fixtures
//! (tests/fixtures/ifc): the flat BOT / props: contract that the viewer
//! feed, the IDS importer and the Studio shapes read, pinned so nothing
//! later can quietly drop a triple — and, beside it, what the lift adds: the
//! IFC 4.3 facility spine, quantity sets and typed property values with
//! QUDT units, classifications as SKOS, materials, the NEN 2660-2 relation
//! family and the map conversion. The last test checks that every term the
//! emitter mints under its own namespace is declared by the `ifc-lift`
//! ontology bundle, and that the bundle seeds under the base URL.
//!
//! No IFC 4.3 model exists in the repository or the boot seed, so 4.3
//! behaviour is verified against these fixtures only.

mod common;

use std::path::Path;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use open_triplestore::ifc::{convert, ConvertOptions, IfcStats};
use open_triplestore::seed_bundles::load_seed_dir;
use open_triplestore::shacl::validate;
use open_triplestore::store::TripleStore;
use oxigraph::io::RdfFormat;
use oxigraph::sparql::QueryResults;
use tower::ServiceExt as _;

const BASE: &str = "http://ex.test/m/";
const LIFT: &str = "http://ex.test/ns/ifc-lift#";
const BOT: &str = "https://w3id.org/bot#";
const PROPS: &str = "https://w3id.org/props#";
const NEN: &str = "https://w3id.org/nen2660/def#";
const QUDT: &str = "http://qudt.org/schema/qudt/";
const UNIT: &str = "http://qudt.org/vocab/unit/";
const SKOS: &str = "http://www.w3.org/2004/02/skos/core#";
const GEO: &str = "http://www.opengis.net/ont/geosparql#";
const XSD: &str = "http://www.w3.org/2001/XMLSchema#";
const IFC4: &str = "https://standards.buildingsmart.org/IFC/DEV/IFC4/ADD2_TC1/OWL#";
const IFC2X3: &str = "https://standards.buildingsmart.org/IFC/DEV/IFC2x3/TC1/OWL#";

fn fixture(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/ifc")
        .join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn lift(step: &str) -> (String, IfcStats) {
    let mut bot = String::new();
    let mut owl = String::new();
    let stats = convert(
        step,
        // The lift namespace is derived from the graph IRI's origin:
        // http://ex.test/m/ → http://ex.test/ns/ifc-lift# (= LIFT).
        &ConvertOptions {
            inst_base: BASE.into(),
            ifc_file_url: Some("http://ex.test/files/model.ifc".into()),
            ..Default::default()
        },
        &mut |c| bot.push_str(c),
        &mut |c| owl.push_str(c),
    )
    .unwrap();
    (bot, stats)
}

fn t(s: &str, p: &str, o: &str) -> String {
    format!("{s} {p} {o} .")
}
fn i(iri: &str) -> String {
    format!("<{iri}>")
}
fn e(guid: &str) -> String {
    i(&format!("{BASE}{guid}"))
}
fn double(v: &str) -> String {
    format!("\"{v}\"^^<{XSD}double>")
}
fn has(bot: &str, triple: &str) -> bool {
    bot.lines().any(|l| l == triple)
}
fn assert_has(bot: &str, triple: &str) {
    assert!(
        has(bot, triple),
        "missing: {triple}\n--- emitted ---\n{bot}"
    );
}
fn assert_not(bot: &str, triple: &str) {
    assert!(!has(bot, triple), "unexpected: {triple}");
}
/// All objects of `(s, p)`, as written.
fn objects(bot: &str, s: &str, p: &str) -> Vec<String> {
    let prefix = format!("{s} {p} ");
    bot.lines()
        .filter_map(|l| l.strip_prefix(&prefix))
        .map(|rest| rest.trim_end_matches(" .").to_string())
        .collect()
}

// ── The contract that must not move ────────────────────────────────────────

/// What consumers already read: the BOT spine and its feed edges, the ifcOWL
/// class in the schema's namespace, GUIDs, file links, the flat
/// props:{Pset}_{Prop} literal. Pinned as exact triples.
#[test]
fn the_flat_bot_and_props_contract_is_unchanged() {
    let (bot, stats) = lift(&fixture("qto-and-classification.ifc"));
    assert_eq!(stats.schema, "IFC4");
    assert_eq!(stats.storeys, 1);
    assert_eq!(stats.elements, 1);
    let (site, building, storey, wall) = (
        e("0BBBBBBBBBBBBBBBBBBBS1"),
        e("0BBBBBBBBBBBBBBBBBBBB1"),
        e("0BBBBBBBBBBBBBBBBBBBL1"),
        e("0BBBBBBBBBBBBBBBBBBBW1"),
    );
    let a = i("http://www.w3.org/1999/02/22-rdf-syntax-ns#type");
    for (s, cls) in [
        (&site, "Site"),
        (&building, "Building"),
        (&storey, "Storey"),
    ] {
        assert_has(&bot, &t(s, &a, &i(&format!("{BOT}{cls}"))));
    }
    assert_has(&bot, &t(&wall, &a, &i(&format!("{BOT}Element"))));
    assert_has(&bot, &t(&wall, &a, &i(&format!("{IFC4}IfcWall"))));
    assert_has(&bot, &t(&site, &i(&format!("{BOT}hasBuilding")), &building));
    assert_has(
        &bot,
        &t(&site, &i(&format!("{BOT}containsElement")), &building),
    );
    assert_has(&bot, &t(&building, &i(&format!("{BOT}hasStorey")), &storey));
    assert_has(
        &bot,
        &t(&storey, &i(&format!("{BOT}containsElement")), &wall),
    );
    assert_has(
        &bot,
        &t(
            &wall,
            &i(&format!("{PROPS}ifcGuid")),
            "\"0BBBBBBBBBBBBBBBBBBBW1\"",
        ),
    );
    assert_has(
        &bot,
        &t(
            &wall,
            &i("http://www.w3.org/2000/01/rdf-schema#label"),
            "\"Wall-01\"",
        ),
    );
    assert_has(
        &bot,
        &t(
            &format!("{}/filelink>", wall.trim_end_matches('>')),
            &i("https://w3id.org/fog#asIfc_v4"),
            &format!("\"http://ex.test/files/model.ifc#0BBBBBBBBBBBBBBBBBBBW1\"^^<{XSD}anyURI>"),
        ),
    );
    assert_has(
        &bot,
        &t(
            &wall,
            &i(&format!("{PROPS}Pset_WallCommon_IsExternal")),
            &format!("\"true\"^^<{XSD}boolean>"),
        ),
    );
    // props:ifcName is emitted only when the root label is overridden.
    assert!(!bot.contains("props#ifcName"));
    // An IFC4 file types nothing under the lift namespace but the values
    // the lift itself adds — never an entity class.
    assert!(!bot.contains(&format!("{LIFT}Ifc")), "{bot}");
}

// ── IFC 4.3 ──────────────────────────────────────────────────────────────────

/// A bridge and its deck are zones with the lift's own classes; the site
/// contains the bridge (zone in zone) and the deck contains the girder; the
/// alignment and the bridge are typed under the lift namespace while the
/// girder — an entity IFC4 already has — keeps its IFC4 ifcOWL IRI.
#[test]
fn ifc43_facilities_form_a_zone_spine_and_only_new_entities_use_the_lift_namespace() {
    let (bot, stats) = lift(&fixture("ifc4x3-georef.ifc"));
    assert_eq!(stats.schema, "IFC4X3_ADD2");
    assert_eq!(stats.facilities, 2, "the bridge and its deck");
    let a = i("http://www.w3.org/1999/02/22-rdf-syntax-ns#type");
    let (site, bridge, deck, girder, alignment) = (
        e("0AAAAAAAAAAAAAAAAAAAS1"),
        e("0AAAAAAAAAAAAAAAAAAAB1"),
        e("0AAAAAAAAAAAAAAAAAAAD1"),
        e("0AAAAAAAAAAAAAAAAAAAE1"),
        e("0AAAAAAAAAAAAAAAAAAAA1"),
    );
    assert_has(&bot, &t(&bridge, &a, &i(&format!("{BOT}Zone"))));
    assert_has(&bot, &t(&bridge, &a, &i(&format!("{LIFT}Bridge"))));
    assert_has(&bot, &t(&bridge, &a, &i(&format!("{LIFT}IfcBridge"))));
    assert_has(&bot, &t(&deck, &a, &i(&format!("{BOT}Zone"))));
    assert_has(&bot, &t(&deck, &a, &i(&format!("{LIFT}BridgePart"))));
    assert_has(&bot, &t(&deck, &a, &i(&format!("{LIFT}IfcBridgePart"))));
    assert_not(&bot, &t(&bridge, &a, &i(&format!("{BOT}Element"))));
    // Camel-cased, under the lift namespace, not …/IFC4/…#IFCALIGNMENT.
    assert_has(&bot, &t(&alignment, &a, &i(&format!("{LIFT}IfcAlignment"))));
    assert!(!bot.contains("IFCALIGNMENT"), "{bot}");
    assert_has(&bot, &t(&girder, &a, &i(&format!("{IFC4}IfcBeam"))));
    assert_has(&bot, &t(&girder, &a, &i(&format!("{BOT}Element"))));
    // Zone in zone: BOT's containsZone plus the feed edge, and NEN parthood.
    for (p, k) in [(&site, &bridge), (&bridge, &deck)] {
        assert_has(&bot, &t(p, &i(&format!("{BOT}containsZone")), k));
        assert_has(&bot, &t(p, &i(&format!("{BOT}containsElement")), k));
        assert_has(&bot, &t(p, &i(&format!("{NEN}hasPart")), k));
    }
    assert_has(
        &bot,
        &t(&deck, &i(&format!("{BOT}containsElement")), &girder),
    );
    assert_has(&bot, &t(&deck, &i(&format!("{NEN}contains")), &girder));
    // Project units are metres and kilograms here.
    let length = format!(
        "{}/qto/Qto_BeamBaseQuantities/Length>",
        girder.trim_end_matches('>')
    );
    assert_has(
        &bot,
        &t(
            &length,
            &i(&format!("{QUDT}hasUnit")),
            &i(&format!("{UNIT}M")),
        ),
    );
    let weight = format!(
        "{}/qto/Qto_BeamBaseQuantities/GrossWeight>",
        girder.trim_end_matches('>')
    );
    assert_has(
        &bot,
        &t(
            &weight,
            &i(&format!("{QUDT}hasUnit")),
            &i(&format!("{UNIT}KiloGM")),
        ),
    );
}

// ── Quantities and properties ───────────────────────────────────────────────

#[test]
fn quantity_sets_become_nodes_with_qudt_units_beside_the_flat_literal() {
    let (bot, stats) = lift(&fixture("qto-and-classification.ifc"));
    let wall = e("0BBBBBBBBBBBBBBBBBBBW1");
    let q = |name: &str| {
        format!(
            "{}/qto/Qto_WallBaseQuantities/{name}>",
            wall.trim_end_matches('>')
        )
    };
    let a = i("http://www.w3.org/1999/02/22-rdf-syntax-ns#type");
    let has_unit = i(&format!("{QUDT}hasUnit"));
    let value = i(&format!("{QUDT}numericValue"));
    let unit = |u: &str| i(&format!("{UNIT}{u}"));

    // Project default (millimetres), and the flat literal beside the node.
    assert_has(
        &bot,
        &t(&wall, &i(&format!("{LIFT}hasQuantity")), &q("Length")),
    );
    assert_has(&bot, &t(&q("Length"), &a, &i(&format!("{LIFT}Quantity"))));
    assert_has(&bot, &t(&q("Length"), &value, &double("4000")));
    assert_has(&bot, &t(&q("Length"), &has_unit, &unit("MilliM")));
    assert_has(
        &bot,
        &t(
            &q("Length"),
            &i(&format!("{LIFT}quantityKind")),
            "\"Length\"",
        ),
    );
    assert_has(
        &bot,
        &t(
            &q("Length"),
            &i(&format!("{LIFT}quantitySet")),
            "\"Qto_WallBaseQuantities\"",
        ),
    );
    assert_has(
        &bot,
        &t(
            &wall,
            &i(&format!("{PROPS}Qto_WallBaseQuantities_Length")),
            &double("4000"),
        ),
    );
    assert_has(&bot, &t(&q("GrossSideArea"), &has_unit, &unit("M2")));
    // The quantity's own unit overrides the project default.
    assert_has(&bot, &t(&q("NetVolume"), &has_unit, &unit("M3")));
    // The kilogram trap, and seconds.
    assert_has(&bot, &t(&q("Weight"), &has_unit, &unit("KiloGM")));
    assert_has(&bot, &t(&q("Duration"), &has_unit, &unit("SEC")));
    // A count has no unit, and no unit label either.
    assert_has(
        &bot,
        &t(&q("Count"), &value, &format!("\"1\"^^<{XSD}integer>")),
    );
    assert!(objects(&bot, &q("Count"), &has_unit).is_empty());
    assert!(objects(&bot, &q("Count"), &i(&format!("{LIFT}unitLabel"))).is_empty());
    // A complex quantity nests its members and names the group.
    let thickness = q("Layers/Thickness");
    assert_has(&bot, &t(&thickness, &value, &double("200")));
    assert_has(&bot, &t(&thickness, &has_unit, &unit("MilliM")));
    assert_has(
        &bot,
        &t(
            &thickness,
            &i(&format!("{LIFT}inComplexQuantity")),
            "\"Layers\"",
        ),
    );
    assert_has(
        &bot,
        &t(
            &wall,
            &i(&format!("{PROPS}Qto_WallBaseQuantities_Layers_Thickness")),
            &double("200"),
        ),
    );
    // A conversion-based inch maps by name; a furlong does not and says so,
    // with its factor against the metre.
    assert_has(&bot, &t(&q("Width"), &has_unit, &unit("IN")));
    assert!(
        objects(&bot, &q("Run"), &has_unit).is_empty(),
        "no guessed unit"
    );
    assert_has(
        &bot,
        &t(&q("Run"), &i(&format!("{LIFT}unitLabel")), "\"FURLONG\""),
    );
    assert_has(
        &bot,
        &t(
            &q("Run"),
            &i(&format!("{LIFT}conversionFactor")),
            &double("201.168"),
        ),
    );
    assert_has(
        &bot,
        &t(&q("Run"), &i(&format!("{LIFT}conversionUnit")), &unit("M")),
    );
    assert_eq!(stats.quantities, 9);
    assert_eq!(stats.unmapped_units, 1);
}

#[test]
fn every_property_kind_becomes_a_typed_node_beside_the_flat_literal() {
    let (bot, stats) = lift(&fixture("qto-and-classification.ifc"));
    let wall = e("0BBBBBBBBBBBBBBBBBBBW1");
    let p = |name: &str| {
        format!(
            "{}/pset/Pset_WallCommon/{name}>",
            wall.trim_end_matches('>')
        )
    };
    let flat = |name: &str| i(&format!("{PROPS}Pset_WallCommon_{name}"));
    let value = i(&format!("{LIFT}value"));
    let has_unit = i(&format!("{QUDT}hasUnit"));

    // Single, unitless.
    assert_has(
        &bot,
        &t(&wall, &i(&format!("{LIFT}hasProperty")), &p("IsExternal")),
    );
    assert_has(
        &bot,
        &t(
            &p("IsExternal"),
            &value,
            &format!("\"true\"^^<{XSD}boolean>"),
        ),
    );
    assert_has(
        &bot,
        &t(
            &p("IsExternal"),
            &i(&format!("{LIFT}propertySet")),
            "\"Pset_WallCommon\"",
        ),
    );
    assert!(objects(&bot, &p("IsExternal"), &has_unit).is_empty());
    // Single, a length measure: the IFC type and the project's length unit.
    assert_has(&bot, &t(&wall, &flat("Height"), &double("2800")));
    assert_has(
        &bot,
        &t(
            &p("Height"),
            &i(&format!("{LIFT}ifcType")),
            "\"IfcPositiveLengthMeasure\"",
        ),
    );
    assert_has(
        &bot,
        &t(&p("Height"), &has_unit, &i(&format!("{UNIT}MilliM"))),
    );
    // Enumerated: the value, flat and on the node, and the enumeration.
    assert_has(&bot, &t(&wall, &flat("FireRating"), "\"REI60\""));
    assert_has(&bot, &t(&p("FireRating"), &value, "\"REI60\""));
    assert_has(
        &bot,
        &t(
            &p("FireRating"),
            &i(&format!("{LIFT}enumeration")),
            "\"PEnum_FireRating\"",
        ),
    );
    // List: one flat triple per value.
    assert_eq!(objects(&bot, &wall, &flat("Colours")).len(), 2);
    assert_eq!(objects(&bot, &p("Colours"), &value).len(), 2);
    // Bounded: no single value to flatten, the bounds on the node.
    assert!(objects(&bot, &wall, &flat("Span")).is_empty());
    assert_has(
        &bot,
        &t(
            &p("Span"),
            &i(&format!("{LIFT}upperBound")),
            &double("5000"),
        ),
    );
    assert_has(
        &bot,
        &t(
            &p("Span"),
            &i(&format!("{LIFT}lowerBound")),
            &double("3000"),
        ),
    );
    assert_has(
        &bot,
        &t(&p("Span"), &has_unit, &i(&format!("{UNIT}MilliM"))),
    );
    // Complex: nested under the complex name, flat as {Pset}_{Complex}_{Prop}.
    assert_has(&bot, &t(&wall, &flat("Layer1_Material"), "\"Brick\""));
    assert_has(&bot, &t(&p("Layer1/Material"), &value, "\"Brick\""));
    assert_has(
        &bot,
        &t(
            &p("Layer1/Material"),
            &i(&format!("{LIFT}inComplexProperty")),
            "\"Layer1\"",
        ),
    );
    // The IFC4 set form of RelatingPropertyDefinition, dropped before.
    assert_has(
        &bot,
        &t(
            &wall,
            &i(&format!("{PROPS}Pset_Extra_Note")),
            "\"via the set form\"",
        ),
    );
    assert_eq!(stats.properties, 7);
}

// ── Classification and material ─────────────────────────────────────────────

#[test]
fn classifications_become_skos_concepts_and_the_flat_literal_the_ids_importer_expects() {
    let (bot, stats) = lift(&fixture("qto-and-classification.ifc"));
    let wall = e("0BBBBBBBBBBBBBBBBBBBW1");
    let a = i("http://www.w3.org/1999/02/22-rdf-syntax-ns#type");
    // The leaf has no Location, so it is minted under the model; its parent
    // and the scheme have absolute Locations and keep them.
    let leaf = i(&format!("{BASE}classification/Uniclass_2015/Ss_25_10_30"));
    let parent = i("https://uniclass.thenbs.com/taxon/ss");
    let scheme = i("https://uniclass.thenbs.com");
    assert_has(
        &bot,
        &t(&wall, &i(&format!("{LIFT}hasClassification")), &leaf),
    );
    assert_has(
        &bot,
        &t(
            &wall,
            &i(&format!("{PROPS}ifcClassification")),
            "\"Ss_25_10_30\"",
        ),
    );
    assert_has(&bot, &t(&leaf, &a, &i(&format!("{SKOS}Concept"))));
    assert_has(
        &bot,
        &t(&leaf, &i(&format!("{SKOS}notation")), "\"Ss_25_10_30\""),
    );
    assert_has(
        &bot,
        &t(&leaf, &i(&format!("{SKOS}prefLabel")), "\"Wall systems\""),
    );
    assert_has(
        &bot,
        &t(
            &leaf,
            &i(&format!("{SKOS}definition")),
            "\"Wall and barrier systems\"",
        ),
    );
    assert_has(&bot, &t(&leaf, &i(&format!("{SKOS}broader")), &parent));
    assert_has(&bot, &t(&leaf, &i(&format!("{SKOS}inScheme")), &scheme));
    assert_has(&bot, &t(&parent, &i(&format!("{SKOS}notation")), "\"Ss\""));
    assert_has(
        &bot,
        &t(&parent, &i(&format!("{SKOS}topConceptOf")), &scheme),
    );
    assert_has(&bot, &t(&scheme, &a, &i(&format!("{SKOS}ConceptScheme"))));
    assert_has(
        &bot,
        &t(
            &scheme,
            &i("http://purl.org/dc/terms/title"),
            "\"Uniclass 2015\"",
        ),
    );
    assert_has(
        &bot,
        &t(
            &scheme,
            &i("http://purl.org/dc/terms/hasVersion"),
            "\"2015\"",
        ),
    );
    assert_has(
        &bot,
        &t(&scheme, &i("http://purl.org/dc/terms/publisher"), "\"NBS\""),
    );
    assert_eq!(stats.classifications, 1);
}

#[test]
fn materials_link_the_element_as_nen_constitution_and_the_flat_literal() {
    let (bot, stats) = lift(&fixture("nen-relations.ifc"));
    let a = i("http://www.w3.org/1999/02/22-rdf-syntax-ns#type");
    let (panel1, wall) = (e("0CCCCCCCCCCCCCCCCCCCC2"), e("0CCCCCCCCCCCCCCCCCCCW1"));
    let aluminium = i(&format!("{BASE}i60"));
    assert_has(
        &bot,
        &t(&panel1, &i(&format!("{LIFT}hasMaterial")), &aluminium),
    );
    assert_has(
        &bot,
        &t(&panel1, &i(&format!("{NEN}consistsOf")), &aluminium),
    );
    assert_has(
        &bot,
        &t(&panel1, &i(&format!("{PROPS}ifcMaterial")), "\"Aluminium\""),
    );
    assert_has(&bot, &t(&aluminium, &a, &i(&format!("{LIFT}Material"))));
    assert_has(
        &bot,
        &t(
            &aluminium,
            &i("http://www.w3.org/2000/01/rdf-schema#label"),
            "\"Aluminium\"",
        ),
    );
    // A layer-set usage resolves to the materials of its layers.
    let mut wall_materials = objects(&bot, &wall, &i(&format!("{PROPS}ifcMaterial")));
    wall_materials.sort();
    assert_eq!(wall_materials, vec!["\"Brick\"", "\"Insulation\""]);
    assert_eq!(
        stats.materials, 4,
        "two panels × aluminium, the wall × two layers"
    );
}

// ── NEN 2660-2 relations ───────────────────────────────────────────────────

/// Beside every BOT edge, the relation it means in NEN 2660-2 — and the P1
/// relation shapes (acyclic, irreflexive) run clean over the result.
#[test]
fn nen_relations_sit_beside_the_bot_edges_and_the_relation_shapes_pass() {
    let step = fixture("nen-relations.ifc");
    let (bot, stats) = lift(&step);
    assert_eq!(stats.schema, "IFC2X3");
    let (site, building, storey, facade, panel1, duct, port, wall) = (
        e("0CCCCCCCCCCCCCCCCCCCS1"),
        e("0CCCCCCCCCCCCCCCCCCCB1"),
        e("0CCCCCCCCCCCCCCCCCCCL1"),
        e("0CCCCCCCCCCCCCCCCCCCC1"),
        e("0CCCCCCCCCCCCCCCCCCCC2"),
        e("0CCCCCCCCCCCCCCCCCCCF1"),
        e("0CCCCCCCCCCCCCCCCCCCF2"),
        e("0CCCCCCCCCCCCCCCCCCCW1"),
    );
    let a = i("http://www.w3.org/1999/02/22-rdf-syntax-ns#type");
    assert_has(
        &bot,
        &t(&facade, &a, &i(&format!("{IFC2X3}IfcCurtainWall"))),
    );
    // Spatial aggregation is parthood; containment is location.
    assert_has(&bot, &t(&site, &i(&format!("{BOT}hasBuilding")), &building));
    assert_has(&bot, &t(&site, &i(&format!("{NEN}hasPart")), &building));
    assert_has(&bot, &t(&building, &i(&format!("{NEN}hasPart")), &storey));
    assert_has(
        &bot,
        &t(&storey, &i(&format!("{BOT}containsElement")), &facade),
    );
    assert_has(&bot, &t(&storey, &i(&format!("{NEN}contains")), &facade));
    assert_not(&bot, &t(&storey, &i(&format!("{NEN}hasPart")), &facade));
    // Element decomposition (aggregation and nesting) is technical parthood.
    assert_has(
        &bot,
        &t(&facade, &i(&format!("{BOT}hasSubElement")), &panel1),
    );
    assert_has(
        &bot,
        &t(&facade, &i(&format!("{NEN}hasTechnicalPart")), &panel1),
    );
    assert_has(&bot, &t(&duct, &i(&format!("{BOT}hasSubElement")), &port));
    assert_has(
        &bot,
        &t(&duct, &i(&format!("{NEN}hasTechnicalPart")), &port),
    );
    // A path connection.
    assert_has(
        &bot,
        &t(&facade, &i(&format!("{NEN}connectsObject")), &wall),
    );

    // The P1 relation shapes over the lifted graph: no cycle, nothing
    // contains or connects itself. The spatial shapes need geometries and
    // stay silent here, as they should.
    let store = TripleStore::in_memory().unwrap();
    let shapes = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("examples/seed-bundles/nen2660-relations/shapes.ttl"),
    )
    .unwrap();
    store
        .load_str(&shapes, RdfFormat::Turtle, Some("urn:shapes"))
        .unwrap();
    store
        .load_str(&bot, RdfFormat::NTriples, Some("urn:lift"))
        .unwrap();
    let report = validate(&store, "urn:shapes", &["urn:lift".to_string()]).unwrap();
    assert!(
        report.conforms,
        "the lifted relations satisfy the NEN shapes: {:?}",
        report.results
    );
    // A planted cycle is caught, so the run above was not vacuous.
    store
        .update(&format!(
            "INSERT DATA {{ GRAPH <urn:lift> {{ {panel1} <{NEN}hasPart> {facade} }} }}"
        ))
        .unwrap();
    let report = validate(&store, "urn:shapes", &["urn:lift".to_string()]).unwrap();
    assert!(!report.conforms, "a hasPart cycle is a violation");
}

// ── Map conversion ──────────────────────────────────────────────────────────

#[test]
fn the_map_conversion_is_read_as_a_crs_qualified_point_and_never_applied() {
    let step = fixture("ifc4x3-georef.ifc");
    let (bot, stats) = lift(&step);
    assert!(stats.map_conversion);
    let site = e("0AAAAAAAAAAAAAAAAAAAS1");
    let mc = format!("{}/map-conversion>", site.trim_end_matches('>'));
    let a = i("http://www.w3.org/1999/02/22-rdf-syntax-ns#type");
    assert_has(&bot, &t(&site, &i(&format!("{LIFT}mapConversion")), &mc));
    assert_has(&bot, &t(&mc, &a, &i(&format!("{LIFT}MapConversion"))));
    assert_has(
        &bot,
        &t(&mc, &i(&format!("{LIFT}eastings")), &double("155000")),
    );
    assert_has(
        &bot,
        &t(&mc, &i(&format!("{LIFT}northings")), &double("463000")),
    );
    assert_has(
        &bot,
        &t(&mc, &i(&format!("{LIFT}orthogonalHeight")), &double("10")),
    );
    assert_has(&bot, &t(&mc, &i(&format!("{LIFT}mapScale")), &double("1")));
    assert_has(
        &bot,
        &t(&mc, &i(&format!("{LIFT}projectedCrs")), "\"EPSG:28992\""),
    );
    assert_has(
        &bot,
        &t(&mc, &i(&format!("{LIFT}verticalDatum")), "\"NAP\""),
    );
    // atan2(0.02, 0.9998) ≈ 1.146°, an annotation only.
    let rotation: f64 = objects(&bot, &mc, &i(&format!("{LIFT}mapRotation")))[0]
        .trim_start_matches('"')
        .split('"')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    assert!((rotation - 1.146).abs() < 0.01, "{rotation}");
    // The origin as a CRS-qualified point: EPSG:28992 is a CRS this store knows.
    assert_has(&bot, &t(&mc, &a, &i(&format!("{GEO}Geometry"))));
    assert_has(
        &bot,
        &t(
            &mc,
            &i(&format!("{GEO}asWKT")),
            &format!("\"<http://www.opengis.net/def/crs/EPSG/0/28992> POINT(155000 463000)\"^^<{GEO}wktLiteral>"),
        ),
    );
    // The site's own RefLatitude/RefLongitude still provides the WGS84 anchor
    // (52°9' N, 5°23' E), and it is the only geo:hasGeometry on the site.
    let anchor = format!("{}/anchor>", site.trim_end_matches('>'));
    assert_eq!(
        objects(&bot, &site, &i(&format!("{GEO}hasGeometry"))),
        vec![anchor.clone()]
    );
    let wkt = objects(&bot, &anchor, &i(&format!("{GEO}asWKT")))[0].clone();
    assert!(wkt.starts_with("\"POINT(5.38"), "{wkt}");
    assert!(wkt.contains(" 52.15"), "{wkt}");

    // Without a site georeference, the map conversion's origin reprojected
    // to WGS84 becomes the anchor: RD (155000, 463000) is Amersfoort.
    let no_latlon = step.replace("(52,9,0,0),(5,23,0,0)", "$,$");
    assert_ne!(no_latlon, step);
    let (bot2, _) = lift(&no_latlon);
    let wkt = objects(&bot2, &anchor, &i(&format!("{GEO}asWKT")))[0].clone();
    let coords: Vec<f64> = wkt
        .trim_start_matches("\"POINT(")
        .split(')')
        .next()
        .unwrap()
        .split(' ')
        .map(|v| v.parse().unwrap())
        .collect();
    assert!((coords[0] - 5.387).abs() < 0.01, "{wkt}");
    assert!((coords[1] - 52.155).abs() < 0.01, "{wkt}");

    // An EPSG code the store does not know: the numbers and the name are
    // kept, but no CRS-qualified geometry is minted, and no anchor derived.
    let unknown = no_latlon.replace("'EPSG:28992'", "'EPSG:2154'");
    let (bot3, stats3) = lift(&unknown);
    assert!(stats3.map_conversion);
    assert_has(
        &bot3,
        &t(&mc, &i(&format!("{LIFT}projectedCrs")), "\"EPSG:2154\""),
    );
    assert!(
        objects(&bot3, &mc, &i(&format!("{GEO}asWKT"))).is_empty(),
        "{bot3}"
    );
    assert!(objects(&bot3, &site, &i(&format!("{GEO}hasGeometry"))).is_empty());
}

// ── The ontology ────────────────────────────────────────────────────────────

/// Every class and property the emitter mints under its own namespace is
/// declared by examples/seed-bundles/ifc-lift/ifc-lift.ttl — except the
/// IFC 4.3 entity classes (`ifcl:IfcBridge`, …), which are the schema's
/// names passed through, not terms of this ontology.
#[test]
fn every_lift_term_the_emitter_produces_is_declared_by_the_ontology() {
    let ttl = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/seed-bundles/ifc-lift/ifc-lift.ttl"),
    )
    .unwrap()
    .replace("{base_url}", "http://ex.test");
    let onto = TripleStore::in_memory().unwrap();
    onto.load_str(&ttl, RdfFormat::Turtle, Some("urn:onto"))
        .expect("the ontology parses once the base URL is expanded");
    let declared: std::collections::BTreeSet<String> = match onto.query(
        "SELECT ?t WHERE { GRAPH <urn:onto> { ?t a ?k . FILTER(?k IN (<http://www.w3.org/2002/07/owl#Class>, <http://www.w3.org/2002/07/owl#ObjectProperty>, <http://www.w3.org/2002/07/owl#DatatypeProperty>)) } }",
    ) {
        Ok(QueryResults::Solutions(s)) => s
            .flatten()
            .map(|r| r.get("t").unwrap().to_string().trim_matches(['<', '>']).to_string())
            .collect(),
        _ => panic!("query"),
    };
    assert!(
        declared.contains(&format!("{LIFT}Quantity")),
        "{declared:?}"
    );

    let mut used = std::collections::BTreeSet::new();
    for f in [
        "ifc4x3-georef.ifc",
        "qto-and-classification.ifc",
        "nen-relations.ifc",
    ] {
        let (bot, _) = lift(&fixture(f));
        for line in bot.lines() {
            for token in line.split(' ') {
                let iri = token.trim_matches(['<', '>']);
                if let Some(local) = iri.strip_prefix(LIFT) {
                    if !local.starts_with("Ifc") {
                        used.insert(iri.to_string());
                    }
                }
            }
        }
    }
    assert!(
        used.len() > 20,
        "the fixtures exercise the vocabulary: {used:?}"
    );
    let undeclared: Vec<&String> = used.iter().filter(|u| !declared.contains(*u)).collect();
    assert!(
        undeclared.is_empty(),
        "emitted but not declared: {undeclared:?}"
    );
}

/// The `ifc-lift` bundle seeds the ontology under this deployment's base
/// URL — `{base_url}` in the manifest and the payload expands to it — and
/// the registry serves it.
#[tokio::test]
async fn the_ifc_lift_bundle_seeds_the_ontology_under_the_base_url() {
    let (state, token) = admin_state();
    load_seed_dir(
        &state,
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/seed-bundles"),
    );
    let base = state.base_url.trim_end_matches('/').to_string();
    let ask = |q: &str| matches!(state.store.query(q), Ok(QueryResults::Boolean(true)));
    assert!(
        ask(&format!(
            "ASK {{ GRAPH <{base}/ns/ifc-lift> {{ <{base}/ns/ifc-lift#Quantity> a <http://www.w3.org/2002/07/owl#Class> }} }}"
        )),
        "the ontology graph sits under the base URL"
    );
    assert!(
        !ask("ASK { GRAPH ?g { ?s ?p ?o . FILTER(CONTAINS(STR(?s), \"{base_url}\")) } }"),
        "no placeholder survives"
    );
    let app = test_app(state.clone());
    let resp = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/api/models/ifc-lift")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let txt = body_text(resp.into_body()).await;
    assert!(txt.contains(&format!("{base}/ns/ifc-lift#")), "{txt}");
}
