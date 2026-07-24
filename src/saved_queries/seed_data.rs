//! Static content for the bundled "Open Triplestore" public demo.
//!
//! Declares the demo organisation's datasets, the per-standard named graphs
//! (loaded via a variety of RDF serializations to exercise the parsers), and
//! the saved queries that demonstrate each query-able standard. The protocol
//! and auth standards (Graph Store HTTP, SPARQL Update, Service Description,
//! LDP, DCAT/VoID, SHACL-C, JWT, OAuth/OIDC) are exercised by the e2e suite and
//! advertised in the `capabilities` dataset rather than via a SPARQL query.

use serde_json::json;

use crate::auth::models::GraphKind;

use super::models::{CreateSavedQueryRequest, ParamSpec, ParamType};

/// Serialization a graph's bundled data is written in. Defined by the generic
/// seed-bundle engine ([`crate::seed_bundles`]) that this demo now runs through;
/// re-exported here so the demo data tables read naturally.
pub use crate::seed_bundles::Fmt;

/// Base IRI for every demo graph: `{DEMO_BASE}/{dataset_slug}/{graph_suffix}`.
pub const DEMO_BASE: &str = "https://opentriplestore.org/demo";

pub const ORG_NAME: &str = "Open Triplestore";
pub const ORG_SLUG: &str = "open-triplestore";

pub struct GraphSpec {
    pub suffix: &'static str,
    pub role: GraphKind,
    pub fmt: Fmt,
    pub data: &'static str,
}

pub struct DatasetSpec {
    pub slug: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub graphs: &'static [GraphSpec],
}

/// The full demo plan: every dataset and its graphs.
pub fn datasets() -> &'static [DatasetSpec] {
    DATASETS
}

// ─── Saved-query builders ───────────────────────────────────────────────────────

fn svc(name: &str, slug: &str, description: &str, sparql: &str) -> CreateSavedQueryRequest {
    CreateSavedQueryRequest {
        name: name.to_string(),
        slug: Some(slug.to_string()),
        description: Some(description.to_string()),
        sparql: sparql.to_string(),
        parameters: Vec::new(),
        test_parameters: Some(json!({})),
        visibility: None,
        version_name: None,
        note: None,
    }
}

fn svc_param(
    name: &str,
    slug: &str,
    description: &str,
    sparql: &str,
    params: Vec<ParamSpec>,
    test_parameters: serde_json::Value,
) -> CreateSavedQueryRequest {
    CreateSavedQueryRequest {
        name: name.to_string(),
        slug: Some(slug.to_string()),
        description: Some(description.to_string()),
        sparql: sparql.to_string(),
        parameters: params,
        test_parameters: Some(test_parameters),
        visibility: None,
        version_name: None,
        note: None,
    }
}

fn iri_param(name: &str, default: &str, description: &str) -> ParamSpec {
    ParamSpec {
        name: name.to_string(),
        param_type: ParamType::Iri,
        required: true,
        default: Some(default.to_string()),
        description: Some(description.to_string()),
    }
}

/// Saved queries for a dataset, keyed by slug. Empty for datasets whose
/// standards are protocol/auth only (exercised via e2e).
pub fn services_for(dataset_slug: &str) -> Vec<CreateSavedQueryRequest> {
    match dataset_slug {
        "core-rdf-sparql" => vec![
            svc(
                "All statements",
                "all-statements",
                "SPARQL 1.1 — a basic SELECT over every triple in the dataset.",
                "SELECT ?s ?p ?o WHERE { ?s ?p ?o } LIMIT 200",
            ),
            svc(
                "Typed & multilingual literals",
                "typed-and-multilingual",
                "RDF 1.1 — surfaces each literal's datatype and language tag (DATATYPE / LANG).",
                "SELECT ?s ?p ?value (DATATYPE(?value) AS ?datatype) (LANG(?value) AS ?language) \
                 WHERE { ?s ?p ?value . FILTER(isLiteral(?value)) } ORDER BY ?s",
            ),
            svc(
                "Is anyone marked notable?",
                "ask-notable",
                "SPARQL 1.1 — ASK query returning a boolean.",
                "PREFIX ex: <https://opentriplestore.org/demo/core#>\nASK { ?p ex:notable true }",
            ),
            svc_param(
                "Describe a person",
                "describe-person",
                "SPARQL 1.1 — CONSTRUCT graph result with an injected {{subject}} IRI.",
                "CONSTRUCT { {{subject}} ?p ?o } WHERE { {{subject}} ?p ?o }",
                vec![iri_param(
                    "subject",
                    "https://opentriplestore.org/demo/core#Ada",
                    "Subject IRI to describe.",
                )],
                json!({ "subject": "https://opentriplestore.org/demo/core#Ada" }),
            ),
            svc(
                "Statements about statements",
                "rdf-star-provenance",
                "RDF-star / SPARQL 1.2 — query a quoted triple and the metadata asserted about it.",
                "PREFIX ex: <https://opentriplestore.org/demo/core#>\n\
                 PREFIX dct: <http://purl.org/dc/terms/>\n\
                 SELECT ?s ?p ?o ?source ?confidence WHERE { \
                   << ?s ?p ?o >> dct:source ?source ; ex:confidence ?confidence }",
            ),
        ],
        "reasoning" => vec![
            svc(
                "Class hierarchy",
                "class-hierarchy",
                "RDFS — transitive subclass paths (rdfs:subClassOf+).",
                "PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\n\
                 SELECT ?subClass ?superClass WHERE { ?subClass rdfs:subClassOf+ ?superClass } ORDER BY ?subClass",
            ),
            svc(
                "Property domains and ranges",
                "property-domains-ranges",
                "RDFS — declared rdfs:domain / rdfs:range for each property.",
                "PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\n\
                 SELECT ?property ?domain ?range WHERE { \
                   { ?property rdfs:domain ?domain } UNION { ?property rdfs:range ?range } } ORDER BY ?property",
            ),
            svc(
                "Transitive ancestors",
                "transitive-ancestors",
                "OWL 2 RL — query-time transitive closure over an owl:TransitiveProperty (ex:ancestorOf+).",
                "PREFIX ex: <https://opentriplestore.org/demo/reasoning#>\n\
                 SELECT ?descendant ?ancestor WHERE { ?descendant ex:ancestorOf+ ?ancestor } ORDER BY ?descendant",
            ),
            svc(
                "Asserted equivalences",
                "owl-equivalences",
                "OWL 2 — owl:sameAs individual equivalences and owl:equivalentClass axioms.",
                "PREFIX owl: <http://www.w3.org/2002/07/owl#>\n\
                 SELECT ?a ?relation ?b WHERE { \
                   { ?a owl:sameAs ?b . BIND(\"sameAs\" AS ?relation) } UNION \
                   { ?a owl:equivalentClass ?b . BIND(\"equivalentClass\" AS ?relation) } }",
            ),
            svc(
                "Ontology axioms overview",
                "ontology-axioms",
                "OWL 2 QL/EL/DL — counts axiom shapes (restrictions, disjointness, cardinality) across the ontologies.",
                "PREFIX owl: <http://www.w3.org/2002/07/owl#>\n\
                 SELECT ?axiomType (COUNT(*) AS ?count) WHERE { \
                   { ?r a owl:Restriction . BIND(\"Restriction\" AS ?axiomType) } UNION \
                   { ?c owl:disjointWith ?d . BIND(\"disjointWith\" AS ?axiomType) } UNION \
                   { ?p a owl:TransitiveProperty . BIND(\"TransitiveProperty\" AS ?axiomType) } \
                 } GROUP BY ?axiomType ORDER BY ?axiomType",
            ),
        ],
        "spatial" => vec![
            svc(
                "Cities within a bounding box",
                "cities-in-bbox",
                "GeoSPARQL — geof:sfWithin over WKT geometries (a box around the Low Countries).",
                "PREFIX ex: <https://opentriplestore.org/demo/spatial#>\n\
                 PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\n\
                 PREFIX geo: <http://www.opengis.net/ont/geosparql#>\n\
                 PREFIX geof: <http://www.opengis.net/def/function/geosparql/>\n\
                 SELECT ?city ?name ?wkt WHERE { \
                   ?city rdfs:label ?name ; geo:hasGeometry/geo:asWKT ?wkt . \
                   FILTER(geof:sfWithin(?wkt, \"POLYGON((3 50, 7 50, 7 54, 3 54, 3 50))\"^^geo:wktLiteral)) \
                 } ORDER BY ?name",
            ),
            svc_param(
                "Distance from a city",
                "distance-from-city",
                "GeoSPARQL — geof:distance (metres) from a chosen city to every other.",
                "PREFIX ex: <https://opentriplestore.org/demo/spatial#>\n\
                 PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\n\
                 PREFIX geo: <http://www.opengis.net/ont/geosparql#>\n\
                 PREFIX geof: <http://www.opengis.net/def/function/geosparql/>\n\
                 PREFIX uom: <http://www.opengis.net/def/uom/OGC/1.0/>\n\
                 SELECT ?other ?name (geof:distance(?w0, ?w1, uom:metre) AS ?metres) WHERE { \
                   {{from}} geo:hasGeometry/geo:asWKT ?w0 . \
                   ?other geo:hasGeometry/geo:asWKT ?w1 ; rdfs:label ?name . \
                   FILTER(?other != {{from}}) } ORDER BY ?metres",
                vec![iri_param(
                    "from",
                    "https://opentriplestore.org/demo/spatial#Amsterdam",
                    "City to measure distances from.",
                )],
                json!({ "from": "https://opentriplestore.org/demo/spatial#Amsterdam" }),
            ),
            svc(
                "All geometries",
                "all-geometries",
                "GeoSPARQL — the raw geo:wktLiteral geometry of every feature.",
                "PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\n\
                 PREFIX geo: <http://www.opengis.net/ont/geosparql#>\n\
                 SELECT ?feature ?name ?wkt WHERE { \
                   ?feature geo:hasGeometry/geo:asWKT ?wkt . OPTIONAL { ?feature rdfs:label ?name } } ORDER BY ?name",
            ),
        ],
        "validation" => vec![
            svc(
                "Declared shapes",
                "declared-shapes",
                "SHACL Core — every sh:NodeShape and the class it targets.",
                "PREFIX sh: <http://www.w3.org/ns/shacl#>\n\
                 SELECT ?shape ?targetClass WHERE { ?shape a sh:NodeShape . OPTIONAL { ?shape sh:targetClass ?targetClass } }",
            ),
            svc(
                "Property constraints",
                "property-constraints",
                "SHACL Core — sh:path / sh:minCount / sh:datatype constraints declared by the shapes.",
                "PREFIX sh: <http://www.w3.org/ns/shacl#>\n\
                 SELECT ?shape ?path ?minCount ?datatype WHERE { \
                   ?shape sh:property ?ps . ?ps sh:path ?path . \
                   OPTIONAL { ?ps sh:minCount ?minCount } OPTIONAL { ?ps sh:datatype ?datatype } } ORDER BY ?shape",
            ),
            svc(
                "SPARQL-based constraints",
                "sparql-constraints",
                "SHACL Advanced — constraints expressed with sh:sparql / sh:SPARQLConstraint.",
                "PREFIX sh: <http://www.w3.org/ns/shacl#>\n\
                 SELECT ?shape ?message ?select WHERE { \
                   ?shape sh:sparql ?c . OPTIONAL { ?c sh:message ?message } OPTIONAL { ?c sh:select ?select } }",
            ),
            svc(
                "ShEx-described resources",
                "shex-resources",
                "ShEx — resources the bundled ShEx schema (stored as RDF) is written to validate.",
                "PREFIX ex: <https://opentriplestore.org/demo/validation#>\n\
                 SELECT ?resource ?type WHERE { ?resource a ?type . FILTER(STRSTARTS(STR(?type), STR(ex:))) } ORDER BY ?resource",
            ),
        ],
        "rules" => vec![
            svc(
                "SWRL rules",
                "swrl-rules",
                "SWRL — the declared rule implications (swrl:body ⇒ swrl:head).",
                "PREFIX swrl: <http://www.w3.org/2003/11/swrl#>\n\
                 SELECT ?rule ?body ?head WHERE { ?rule a swrl:Imp . OPTIONAL { ?rule swrl:body ?body } OPTIONAL { ?rule swrl:head ?head } }",
            ),
            svc(
                "Rule antecedent data",
                "rule-antecedent-data",
                "SWRL — the asserted hasParent facts a rule's antecedent matches before inferring hasGrandparent.",
                "PREFIX ex: <https://opentriplestore.org/demo/rules#>\n\
                 SELECT ?child ?parent WHERE { ?child ex:hasParent ?parent } ORDER BY ?child",
            ),
        ],
        "linked-data" => vec![
            svc(
                "LDP container members",
                "ldp-members",
                "Linked Data Platform — the members of the bundled ldp:BasicContainer (ldp:contains).",
                "PREFIX ldp: <http://www.w3.org/ns/ldp#>\n\
                 SELECT ?container ?member WHERE { ?container a ldp:BasicContainer ; ldp:contains ?member } ORDER BY ?member",
            ),
            svc(
                "Catalog datasets",
                "catalog-datasets",
                "DCAT 2 — datasets advertised by the bundled dcat:Catalog.",
                "PREFIX dcat: <http://www.w3.org/ns/dcat#>\n\
                 PREFIX dct: <http://purl.org/dc/terms/>\n\
                 SELECT ?dataset ?title WHERE { ?cat a dcat:Catalog ; dcat:dataset ?dataset . OPTIONAL { ?dataset dct:title ?title } }",
            ),
            svc(
                "Dataset distributions",
                "dataset-distributions",
                "DCAT 2 — distributions and their media types / download URLs.",
                "PREFIX dcat: <http://www.w3.org/ns/dcat#>\n\
                 SELECT ?dataset ?distribution ?mediaType ?downloadURL WHERE { \
                   ?dataset dcat:distribution ?distribution . \
                   OPTIONAL { ?distribution dcat:mediaType ?mediaType } OPTIONAL { ?distribution dcat:downloadURL ?downloadURL } }",
            ),
        ],
        "capabilities" => vec![
            svc(
                "Supported standards",
                "supported-standards",
                "The machine-readable list of standards this server implements, with conformance level.",
                "PREFIX ots: <https://opentriplestore.org/ns#>\n\
                 PREFIX dct: <http://purl.org/dc/terms/>\n\
                 SELECT ?standard ?title ?level WHERE { \
                   ?standard a ots:Standard ; dct:title ?title ; ots:conformance ?level } ORDER BY ?title",
            ),
            svc(
                "Authentication methods",
                "authentication-methods",
                "JWT and OAuth 2.0 / OIDC session authentication advertised as capabilities.",
                "PREFIX ots: <https://opentriplestore.org/ns#>\n\
                 PREFIX dct: <http://purl.org/dc/terms/>\n\
                 SELECT ?method ?title WHERE { ?method a ots:AuthMethod ; dct:title ?title }",
            ),
        ],
        "ots-ontology" => vec![
            svc(
                "Graph roles in the model",
                "ots-graph-roles",
                "OWL/RDFS — the graph-role individuals the codebase classifies named graphs with.",
                "PREFIX otso: <https://opentriplestore.org/ns#>\n\
                 PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\n\
                 SELECT ?role ?label WHERE { ?role a otso:GraphRole ; rdfs:label ?label } ORDER BY ?label",
            ),
            svc(
                "Model classes & properties",
                "ots-model-terms",
                "OWL — the classes and properties defined in the Open Triplestore model.",
                "PREFIX owl: <http://www.w3.org/2002/07/owl#>\n\
                 PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\n\
                 SELECT ?term ?type ?label WHERE { ?term a ?type ; rdfs:label ?label . \
                 FILTER(?type IN (owl:Class, owl:ObjectProperty, owl:DatatypeProperty)) } ORDER BY ?type ?label",
            ),
            svc(
                "Vocabulary concepts",
                "ots-vocab-concepts",
                "SKOS — the controlled values (graph roles, conformance levels, standards) in the vocabulary.",
                "PREFIX skos: <http://www.w3.org/2004/02/skos/core#>\n\
                 SELECT ?concept ?label WHERE { ?concept a skos:Concept ; skos:prefLabel ?label } ORDER BY ?label",
            ),
        ],
        "viewer-3d-demo" => vec![
            svc(
                "Features by geometry type",
                "geometry-types",
                "GeoSPARQL — every geo:asWKT geometry with the WKT type keyword (POINT/LINESTRING/POLYGON/POLYHEDRALSURFACE …).",
                "PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\n\
                 PREFIX geo:  <http://www.opengis.net/ont/geosparql#>\n\
                 SELECT ?feature ?label ?geomType WHERE { \
                   ?feature geo:hasGeometry/geo:asWKT ?wkt . \
                   OPTIONAL { ?feature rdfs:label ?label } \
                   BIND(IF(CONTAINS(STR(?wkt), \">\"), STRAFTER(STR(?wkt), \"> \"), STR(?wkt)) AS ?body) \
                   BIND(STRBEFORE(?body, \"(\") AS ?geomType) \
                 } ORDER BY ?geomType ?label",
            ),
            svc(
                "Volumetric (3D) geometries",
                "volumetric-geometries",
                "GeoSPARQL — the native WKT-Z solids (POLYHEDRALSURFACE/TIN/SOLID/Z) the 3D engine reasons over.",
                "PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\n\
                 PREFIX geo:  <http://www.opengis.net/ont/geosparql#>\n\
                 SELECT ?feature ?label ?wkt WHERE { \
                   ?feature geo:hasGeometry/geo:asWKT ?wkt . \
                   OPTIONAL { ?feature rdfs:label ?label } \
                   BIND(UCASE(STR(?wkt)) AS ?u) \
                   FILTER(CONTAINS(?u, \"POLYHEDRALSURFACE\") || CONTAINS(?u, \"SOLID\") \
                       || CONTAINS(?u, \"TIN\") || CONTAINS(?u, \" Z \") || CONTAINS(?u, \" Z(\")) \
                 } ORDER BY ?label",
            ),
            svc(
                "3D model files (FOG)",
                "model-files",
                "OMG/FOG — every linked 3D-model file (glTF/STL/CityJSON/CityGML/IFC) referenced via fog:as….",
                "PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\n\
                 PREFIX omg:  <https://w3id.org/omg#>\n\
                 SELECT ?feature ?label ?format ?file WHERE { \
                   ?feature omg:hasGeometry ?g . ?g ?p ?file . \
                   FILTER(STRSTARTS(STR(?p), \"https://w3id.org/fog#as\")) \
                   BIND(STRAFTER(STR(?p), \"https://w3id.org/fog#as\") AS ?format) \
                   OPTIONAL { ?feature rdfs:label ?label } \
                 } ORDER BY ?format ?label",
            ),
            svc(
                "Latest sensor observations",
                "latest-observations",
                "SOSA/SSN — the digital-twin observations: sensor, feature of interest, observed property, result and time.",
                "PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\n\
                 PREFIX sosa: <http://www.w3.org/ns/sosa/>\n\
                 SELECT ?observation ?sensor ?feature ?property ?result ?time WHERE { \
                   ?observation a sosa:Observation ; \
                     sosa:madeBySensor ?sensor ; \
                     sosa:hasFeatureOfInterest ?feature ; \
                     sosa:observedProperty ?property ; \
                     sosa:hasSimpleResult ?result ; \
                     sosa:resultTime ?time . \
                 } ORDER BY DESC(?time)",
            ),
            // BOT building-structure queries — the merged-in Buildings & BIM data
            // and the IFC buildings all carry bot:Building / bot:hasStorey.
            svc(
                "Buildings by storey count",
                "buildings-by-storeys",
                "BOT — each bot:Building with the number of bot:Storey levels it decomposes into (bot:hasStorey).",
                "PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\n\
                 PREFIX bot:  <https://w3id.org/bot#>\n\
                 SELECT ?building ?label (COUNT(?storey) AS ?storeys) WHERE { \
                   ?building a bot:Building . \
                   OPTIONAL { ?building rdfs:label ?label } \
                   OPTIONAL { ?building bot:hasStorey ?storey } \
                 } GROUP BY ?building ?label ORDER BY DESC(?storeys) ?label",
            ),
            svc(
                "Building locations & footprints",
                "building-footprints",
                "GeoSPARQL — every building's CRS84 map geometry: POINT anchors for the IFC models, real POLYGON footprints for the BAG panden.",
                "PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\n\
                 PREFIX geo:  <http://www.opengis.net/ont/geosparql#>\n\
                 SELECT ?building ?label ?wkt WHERE { \
                   ?building a <https://w3id.org/bot#Building> ; geo:hasGeometry/geo:asWKT ?wkt . \
                   FILTER(STRSTARTS(STR(?wkt), \"POINT\") || STRSTARTS(STR(?wkt), \"POLYGON\")) \
                   OPTIONAL { ?building rdfs:label ?label } \
                 } ORDER BY ?label",
            ),
            // Real-BAG queries — the per-building 3DBAG layer with registry facts.
            svc(
                "Real buildings by year built",
                "buildings-by-year",
                "BAG registry — every real pand in the block with its construction year, status and roof type (bag:oorspronkelijkbouwjaar).",
                "PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\n\
                 PREFIX bag:  <https://data.3dbag.nl/def/>\n\
                 SELECT ?pand ?label ?year ?status ?roofType WHERE { \
                   ?pand bag:oorspronkelijkbouwjaar ?year . \
                   OPTIONAL { ?pand rdfs:label ?label } \
                   OPTIONAL { ?pand bag:status ?status } \
                   OPTIONAL { ?pand bag:b3_dak_type ?roofType } \
                 } ORDER BY ?year ?label",
            ),
            svc(
                "Tallest real buildings",
                "buildings-by-height",
                "3DBAG metrics — roof height above sea level, ground level, LoD2.2 volume and floor count per real building, tallest first.",
                "PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\n\
                 PREFIX bag:  <https://data.3dbag.nl/def/>\n\
                 SELECT ?pand ?label ?roofMax ?ground ?volume ?floors WHERE { \
                   ?pand bag:b3_h_dak_max ?roofMax . \
                   OPTIONAL { ?pand rdfs:label ?label } \
                   OPTIONAL { ?pand bag:b3_h_maaiveld ?ground } \
                   OPTIONAL { ?pand bag:b3_volume_lod22 ?volume } \
                   OPTIONAL { ?pand bag:b3_bouwlagen ?floors } \
                 } ORDER BY DESC(?roofMax) LIMIT 25",
            ),
            svc(
                "Links into the national registry",
                "registry-links",
                "Linked data — each real building's owl:sameAs into the dereferenceable BAG registry (bag.basisregistraties.overheid.nl) and its 3DBAG API document.",
                "PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\n\
                 PREFIX owl:  <http://www.w3.org/2002/07/owl#>\n\
                 SELECT ?building ?label ?registryUri ?apiDoc WHERE { \
                   ?building owl:sameAs ?registryUri . \
                   OPTIONAL { ?building rdfs:label ?label } \
                   OPTIONAL { ?building rdfs:seeAlso ?apiDoc } \
                   FILTER(CONTAINS(STR(?registryUri), \"basisregistraties.overheid.nl\")) \
                 } ORDER BY ?label",
            ),
            svc(
                "Rooms by storey",
                "rooms-by-storey",
                "BOT — the storeys, the spaces (rooms) they contain and the elements on each (bot:hasStorey / bot:hasSpace / bot:hasElement).",
                "PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\n\
                 PREFIX bot:  <https://w3id.org/bot#>\n\
                 SELECT ?building ?storey ?part ?kind ?label WHERE { \
                   ?building a bot:Building ; bot:hasStorey ?storey . \
                   { ?storey bot:hasSpace ?part . BIND(\"Space\" AS ?kind) } UNION \
                   { ?storey bot:hasElement ?part . BIND(\"Element\" AS ?kind) } \
                   OPTIONAL { ?part rdfs:label ?label } \
                 } ORDER BY ?building ?storey ?kind ?label",
            ),
        ],
        _ => Vec::new(),
    }
}

// ─── Dataset / graph declarations ────────────────────────────────────────────────

static DATASETS: &[DatasetSpec] = &[
    DatasetSpec {
        slug: "core-rdf-sparql",
        name: "Core RDF & SPARQL",
        description: "RDF 1.1, RDF-star / RDF 1.2 and the SPARQL 1.1/1.2 query forms. \
                      SPARQL Update, the Graph Store HTTP protocol and the Service Description \
                      endpoint are exercised by the conformance and e2e suites.",
        graphs: &[
            GraphSpec { suffix: "rdf11", role: GraphKind::Instances, fmt: Fmt::Turtle, data: RDF11_TTL },
            GraphSpec { suffix: "rdf-star", role: GraphKind::Instances, fmt: Fmt::SparqlStarUpdate, data: RDF_STAR_INSERT },
        ],
    },
    DatasetSpec {
        slug: "reasoning",
        name: "Reasoning & Ontologies",
        description: "RDFS schema inference plus the four OWL 2 profiles (QL, EL, RL, DL). \
                      The saved queries demonstrate the relevant data patterns; full entailment \
                      is verified by the conformance suites and the /api/reasoning endpoints.",
        graphs: &[
            GraphSpec { suffix: "rdfs", role: GraphKind::Model, fmt: Fmt::Turtle, data: RDFS_TTL },
            GraphSpec { suffix: "owl-ql", role: GraphKind::Model, fmt: Fmt::Turtle, data: OWL_QL_TTL },
            GraphSpec { suffix: "owl-el", role: GraphKind::Model, fmt: Fmt::RdfXml, data: OWL_EL_RDFXML },
            GraphSpec { suffix: "owl-rl", role: GraphKind::Model, fmt: Fmt::Turtle, data: OWL_RL_TTL },
            GraphSpec { suffix: "owl-dl", role: GraphKind::Model, fmt: Fmt::RdfXml, data: OWL_DL_RDFXML },
        ],
    },
    DatasetSpec {
        slug: "spatial",
        name: "Spatial (GeoSPARQL)",
        description: "GeoSPARQL 1.1 — WKT geometry literals and the geof: relation/measurement functions.",
        graphs: &[GraphSpec { suffix: "geo", role: GraphKind::Instances, fmt: Fmt::Turtle, data: GEO_TTL }],
    },
    DatasetSpec {
        slug: "validation",
        name: "Validation (SHACL & ShEx)",
        description: "SHACL Core and Advanced (SPARQL constraints) plus ShEx. SHACL-C compact \
                      syntax parsing and actual validation runs are covered by the e2e suite.",
        graphs: &[
            GraphSpec { suffix: "shacl-shapes", role: GraphKind::Shapes, fmt: Fmt::Turtle, data: SHACL_SHAPES_TTL },
            GraphSpec { suffix: "shacl-data", role: GraphKind::Instances, fmt: Fmt::NTriples, data: SHACL_DATA_NT },
            GraphSpec { suffix: "shex", role: GraphKind::Shapes, fmt: Fmt::Turtle, data: SHEX_TTL },
        ],
    },
    DatasetSpec {
        slug: "rules",
        name: "Rules (SWRL)",
        description: "SWRL rule definitions expressed in RDF, plus the facts their antecedents match.",
        graphs: &[GraphSpec { suffix: "swrl", role: GraphKind::Model, fmt: Fmt::Turtle, data: SWRL_TTL }],
    },
    DatasetSpec {
        slug: "linked-data",
        name: "Linked Data & Catalog",
        description: "Linked Data Platform containers and DCAT 2 / VoID dataset description. \
                      The VoID document is also served live at /.well-known/void.",
        graphs: &[
            GraphSpec { suffix: "ldp", role: GraphKind::Instances, fmt: Fmt::Turtle, data: LDP_TTL },
            GraphSpec { suffix: "dcat", role: GraphKind::Instances, fmt: Fmt::Turtle, data: DCAT_TTL },
        ],
    },
    DatasetSpec {
        slug: "capabilities",
        name: "Platform Capabilities & Security",
        description: "A machine-readable description (loaded from JSON-LD) of every standard this \
                      server supports and its authentication methods (JWT, OAuth 2.0 / OIDC, SAML).",
        graphs: &[GraphSpec { suffix: "capabilities", role: GraphKind::Instances, fmt: Fmt::JsonLd, data: CAPABILITIES_JSONLD }],
    },
    DatasetSpec {
        slug: "ots-ontology",
        name: "Open Triplestore Ontology & Vocabulary",
        description: "The data model (OWL/RDFS) and controlled vocabulary (SKOS) that the Open Triplestore \
                      codebase itself uses and publishes: standards, authentication methods, graph roles \
                      and conformance levels — the terms behind ots:Standard, ots:graphRole and the catalogue.",
        graphs: &[
            GraphSpec { suffix: "ots-model", role: GraphKind::Model, fmt: Fmt::Turtle, data: OTS_MODEL_TTL },
            GraphSpec { suffix: "ots-vocabulary", role: GraphKind::Vocabulary, fmt: Fmt::Turtle, data: OTS_VOCAB_TTL },
        ],
    },
    DatasetSpec {
        slug: "viewer-3d-demo",
        name: "3D, Map & BIM Demo",
        description: "Real linked building data for the map and 3D viewers. Four real, openly \
                      licensed IFC models — the Schependomlaan housing project (Nijmegen, the \
                      canonical open Dutch BIM dataset, CC BY 4.0), the KIT FZK-Haus, the KIT \
                      Smiley West student housing (Karlsruhe) and the buildingSMART Duplex \
                      Apartment — are downloaded on first boot, stored as downloadable assets and \
                      transformed to linked data (BOT topology, property sets and a full ifcOWL \
                      lift), so storeys, spaces, walls and beams are individually selectable; each \
                      stands at its real-world site from the file's own IfcSite georeference \
                      (KIT Campus North, Karlsruhe, Chicago) or, for Schependomlaan, its actual \
                      street in Nijmegen. \
                      The real city block around the Schependomlaan site comes from 3DBAG (LoD2.2 \
                      CityJSON, © 3DBAG by tudelft3d and 3DGI, CC BY 4.0): every one of its ~77 \
                      buildings is a live BAG pand carrying its registry attributes (year built, \
                      status, roof type and heights, volume, floor count), linked to the national \
                      BAG registry with owl:sameAs — pick any building on the map or in 3D to see \
                      its data. Real Wikidata landmarks (CC0) with open 3D models from Wikimedia \
                      Commons, mixed GeoSPARQL feature types, native WKT-Z volumetric solids for \
                      the 3D engine, a SOSA/SSN digital-twin sensor layer on the real buildings, \
                      and an OTL/IMBOR asset-management alignment complete the picture. Served \
                      per element by /api/datasets/:id/viewer-feed, reprojected to WGS84.",
        graphs: &[
            GraphSpec { suffix: "landmarks", role: GraphKind::Instances, fmt: Fmt::Turtle, data: LANDMARKS_TTL },
            GraphSpec { suffix: "geo-features", role: GraphKind::Instances, fmt: Fmt::Turtle, data: GEO_FEATURES_TTL },
            GraphSpec { suffix: "volumes-3d", role: GraphKind::Instances, fmt: Fmt::Turtle, data: VOLUMES_3D_TTL },
            GraphSpec { suffix: "sensors", role: GraphKind::Instances, fmt: Fmt::Turtle, data: SENSORS_SOSA_TTL },
            GraphSpec { suffix: "assets", role: GraphKind::Instances, fmt: Fmt::Turtle, data: ASSETS_OTL_TTL },
        ],
    },
];

/// Seed copies of the viewer-demo fixtures. Canonical sources live under
/// tests/fixtures/ (the conformance oracle's home); these copies exist because
/// the Docker image build only ships src/, and a drift-guard test in
/// tests/waalbrug_viewer_e2e.rs keeps them byte-identical below their header.
const LANDMARKS_TTL: &str = include_str!("data/landmarks.ttl");
/// Mixed GeoSPARQL geometry types (POINT/LINESTRING/POLYGON) for the 2D map.
const GEO_FEATURES_TTL: &str = include_str!("data/geo-features.ttl");
/// Native WKT-Z volumetric solids (POLYHEDRALSURFACE Z) for the 3D engine/viewer.
const VOLUMES_3D_TTL: &str = include_str!("data/volumes-3d.ttl");
/// SOSA/SSN digital-twin layer: sensors + observations on the REAL demo
/// buildings (BAG panden + the extruded worked-example solids).
const SENSORS_SOSA_TTL: &str = include_str!("data/sensors-sosa.ttl");
/// OTL/IMBOR-style asset-management alignment (third-party vocab not bundled),
/// aligned with a real BAG pand.
const ASSETS_OTL_TTL: &str = include_str!("data/assets-otl.ttl");

/// OWL/RDFS data model for the `ots:` terms the codebase uses. All terms live in
/// the single `…/ns#` namespace: Standard, AuthMethod, conformance, plus the
/// graph roles and the `graphRole` relation written into dataset metadata.
const OTS_MODEL_TTL: &str = r#"@prefix owl:  <http://www.w3.org/2002/07/owl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix xsd:  <http://www.w3.org/2001/XMLSchema#> .
@prefix ots:  <https://opentriplestore.org/ns#> .
@prefix otso: <https://opentriplestore.org/ns#> .

<https://opentriplestore.org/ns> a owl:Ontology ;
    rdfs:label "Open Triplestore ontology" ;
    rdfs:comment "Terms describing standards, authentication and graph roles used across the Open Triplestore codebase." .

ots:Standard a owl:Class ;
    rdfs:label "Standard" ;
    rdfs:comment "A specification (RDF, SPARQL, SHACL, …) the server conforms to." .
ots:AuthMethod a owl:Class ;
    rdfs:label "Authentication method" ;
    rdfs:comment "A supported way of authenticating to the API (JWT, OAuth, SAML)." .
ots:conformance a owl:DatatypeProperty ;
    rdfs:label "conformance" ;
    rdfs:comment "Declared conformance level for a standard." ;
    rdfs:domain ots:Standard ;
    rdfs:range xsd:string .

otso:GraphRole a owl:Class ;
    rdfs:label "Graph role" ;
    rdfs:comment "The classification of a named graph within a dataset." .
otso:graphRole a owl:ObjectProperty ;
    rdfs:label "graph role" ;
    rdfs:comment "Relates a named graph to its role." ;
    rdfs:range otso:GraphRole .
otso:visibility a owl:DatatypeProperty ;
    rdfs:label "visibility" ;
    rdfs:comment "Access level of a dataset (public, members, private)." ;
    rdfs:range xsd:string .

otso:Instances  a otso:GraphRole ; rdfs:label "Instances"  ; rdfs:comment "Instance data (A-Box)." .
otso:Model      a otso:GraphRole ; rdfs:label "Model"      ; rdfs:comment "OWL/RDFS terminology (T-Box)." .
otso:Vocabulary a otso:GraphRole ; rdfs:label "Vocabulary" ; rdfs:comment "SKOS concept schemes." .
otso:Shapes     a otso:GraphRole ; rdfs:label "Shapes"     ; rdfs:comment "SHACL shape graphs." .
otso:Entailment a otso:GraphRole ; rdfs:label "Entailment" ; rdfs:comment "Materialised inferred triples." .
otso:System     a otso:GraphRole ; rdfs:label "System"     ; rdfs:comment "Internal system metadata." .
"#;

/// SKOS controlled vocabulary for the codebase's value sets (graph roles,
/// conformance levels, standards). Classified as a `Vocabulary` graph.
const OTS_VOCAB_TTL: &str = r#"@prefix skos: <http://www.w3.org/2004/02/skos/core#> .
@prefix dct:  <http://purl.org/dc/terms/> .
@prefix otsv: <https://opentriplestore.org/vocab#> .

otsv:scheme a skos:ConceptScheme ;
    skos:prefLabel "Open Triplestore vocabulary"@en ;
    dct:description "Controlled values used by the Open Triplestore: graph roles, conformance levels and supported standards."@en .

otsv:role-instances  a skos:Concept ; skos:inScheme otsv:scheme ; skos:prefLabel "Instances"@en .
otsv:role-model      a skos:Concept ; skos:inScheme otsv:scheme ; skos:prefLabel "Model"@en .
otsv:role-vocabulary a skos:Concept ; skos:inScheme otsv:scheme ; skos:prefLabel "Vocabulary"@en .
otsv:role-shapes     a skos:Concept ; skos:inScheme otsv:scheme ; skos:prefLabel "Shapes"@en .
otsv:role-entailment a skos:Concept ; skos:inScheme otsv:scheme ; skos:prefLabel "Entailment"@en .
otsv:role-system     a skos:Concept ; skos:inScheme otsv:scheme ; skos:prefLabel "System"@en .

otsv:conf-full    a skos:Concept ; skos:inScheme otsv:scheme ; skos:prefLabel "Full"@en .
otsv:conf-partial a skos:Concept ; skos:inScheme otsv:scheme ; skos:prefLabel "Partial"@en .
otsv:conf-none    a skos:Concept ; skos:inScheme otsv:scheme ; skos:prefLabel "None"@en .

otsv:std-rdf11     a skos:Concept ; skos:inScheme otsv:scheme ; skos:prefLabel "RDF 1.1"@en .
otsv:std-rdfstar   a skos:Concept ; skos:inScheme otsv:scheme ; skos:prefLabel "RDF-star / RDF 1.2"@en .
otsv:std-sparql11  a skos:Concept ; skos:inScheme otsv:scheme ; skos:prefLabel "SPARQL 1.1"@en .
otsv:std-sparql12  a skos:Concept ; skos:inScheme otsv:scheme ; skos:prefLabel "SPARQL 1.2"@en .
otsv:std-rdfs      a skos:Concept ; skos:inScheme otsv:scheme ; skos:prefLabel "RDFS"@en .
otsv:std-owlql     a skos:Concept ; skos:inScheme otsv:scheme ; skos:prefLabel "OWL 2 QL"@en .
otsv:std-owlel     a skos:Concept ; skos:inScheme otsv:scheme ; skos:prefLabel "OWL 2 EL"@en .
otsv:std-owlrl     a skos:Concept ; skos:inScheme otsv:scheme ; skos:prefLabel "OWL 2 RL"@en .
otsv:std-owldl     a skos:Concept ; skos:inScheme otsv:scheme ; skos:prefLabel "OWL 2 DL"@en .
otsv:std-geosparql a skos:Concept ; skos:inScheme otsv:scheme ; skos:prefLabel "GeoSPARQL 1.1"@en .
otsv:std-shacl     a skos:Concept ; skos:inScheme otsv:scheme ; skos:prefLabel "SHACL"@en .
otsv:std-shex      a skos:Concept ; skos:inScheme otsv:scheme ; skos:prefLabel "ShEx"@en .
otsv:std-swrl      a skos:Concept ; skos:inScheme otsv:scheme ; skos:prefLabel "SWRL"@en .
otsv:std-ldp       a skos:Concept ; skos:inScheme otsv:scheme ; skos:prefLabel "Linked Data Platform 1.0"@en .
otsv:std-dcat      a skos:Concept ; skos:inScheme otsv:scheme ; skos:prefLabel "DCAT 2 / VoID"@en .
"#;

// ─── Graph data ───────────────────────────────────────────────────────────────

const RDF11_TTL: &str = r#"
@prefix ex:   <https://opentriplestore.org/demo/core#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix xsd:  <http://www.w3.org/2001/XMLSchema#> .

ex:Person a rdfs:Class ; rdfs:label "Person"@en, "Persoon"@nl .

ex:Ada a ex:Person ;
    rdfs:label "Ada Lovelace"@en ;
    ex:born "1815-12-10"^^xsd:date ;
    ex:age 36 ;
    ex:notable true ;
    ex:knows ex:Charles ;
    ex:aliases ( "Countess of Lovelace" "Augusta Ada Byron" ) .

ex:Charles a ex:Person ;
    rdfs:label "Charles Babbage"@en ;
    ex:born "1791-12-26"^^xsd:date ;
    ex:age 79 ;
    ex:notable true .
"#;

// Loaded via a SPARQL-star INSERT DATA (full IRIs; the graph is supplied by the
// seeder). Requires the rdf-12 build feature at runtime.
const RDF_STAR_INSERT: &str = r#"
<< <https://opentriplestore.org/demo/core#Ada> <https://opentriplestore.org/demo/core#knows> <https://opentriplestore.org/demo/core#Charles> >>
    <http://purl.org/dc/terms/source> <https://en.wikipedia.org/wiki/Ada_Lovelace> ;
    <https://opentriplestore.org/demo/core#confidence> "0.9"^^<http://www.w3.org/2001/XMLSchema#decimal> .
"#;

const RDFS_TTL: &str = r#"
@prefix ex:   <https://opentriplestore.org/demo/reasoning#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix xsd:  <http://www.w3.org/2001/XMLSchema#> .

ex:Animal   a rdfs:Class ; rdfs:label "Animal"@en .
ex:Mammal   a rdfs:Class ; rdfs:subClassOf ex:Animal ; rdfs:label "Mammal"@en .
ex:Dog      a rdfs:Class ; rdfs:subClassOf ex:Mammal ; rdfs:label "Dog"@en .

ex:name      a rdfs:Property ; rdfs:domain ex:Animal ; rdfs:range xsd:string .
ex:caresFor  a rdfs:Property ; rdfs:subPropertyOf ex:knows ; rdfs:domain ex:Animal ; rdfs:range ex:Animal .
ex:knows     a rdfs:Property ; rdfs:domain ex:Animal ; rdfs:range ex:Animal .

ex:rex a ex:Dog ; ex:name "Rex" ; ex:caresFor ex:fido .
ex:fido a ex:Dog ; ex:name "Fido" .
"#;

const OWL_QL_TTL: &str = r#"
@prefix ex:   <https://opentriplestore.org/demo/reasoning#> .
@prefix owl:  <http://www.w3.org/2002/07/owl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .

# OWL 2 QL: subclass axioms + an existential (someValuesFrom) role restriction —
# the kind of TBox the QL profile rewrites into UNIONs at query time.
ex:Employee rdfs:subClassOf ex:Person .
ex:Manager  rdfs:subClassOf ex:Employee ;
    rdfs:subClassOf [ a owl:Restriction ; owl:onProperty ex:manages ; owl:someValuesFrom ex:Employee ] .
ex:manages a owl:ObjectProperty ; rdfs:domain ex:Manager ; rdfs:range ex:Employee .
"#;

const OWL_EL_RDFXML: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"
         xmlns:owl="http://www.w3.org/2002/07/owl#"
         xmlns:rdfs="http://www.w3.org/2000/01/rdf-schema#"
         xmlns:ex="https://opentriplestore.org/demo/reasoning#">
  <!-- OWL 2 EL: intersection + existential restriction (SNOMED/GO style). -->
  <owl:Class rdf:about="https://opentriplestore.org/demo/reasoning#HeartDisease">
    <rdfs:label>Heart disease</rdfs:label>
    <rdfs:subClassOf>
      <owl:Restriction>
        <owl:onProperty rdf:resource="https://opentriplestore.org/demo/reasoning#findingSite"/>
        <owl:someValuesFrom rdf:resource="https://opentriplestore.org/demo/reasoning#Heart"/>
      </owl:Restriction>
    </rdfs:subClassOf>
  </owl:Class>
  <owl:Class rdf:about="https://opentriplestore.org/demo/reasoning#Heart"/>
  <owl:ObjectProperty rdf:about="https://opentriplestore.org/demo/reasoning#findingSite"/>
</rdf:RDF>
"#;

const OWL_RL_TTL: &str = r#"
@prefix ex:   <https://opentriplestore.org/demo/reasoning#> .
@prefix owl:  <http://www.w3.org/2002/07/owl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .

# OWL 2 RL: a transitive property + owl:sameAs identity.
ex:ancestorOf a owl:TransitiveProperty, owl:ObjectProperty ; rdfs:label "ancestor of"@en .

ex:Alice ex:ancestorOf ex:Bob .
ex:Bob   ex:ancestorOf ex:Carol .
ex:Carol ex:ancestorOf ex:Dave .

ex:William owl:sameAs ex:Bill .
ex:Bill ex:ancestorOf ex:Erin .
"#;

const OWL_DL_RDFXML: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"
         xmlns:owl="http://www.w3.org/2002/07/owl#"
         xmlns:rdfs="http://www.w3.org/2000/01/rdf-schema#">
  <!-- OWL 2 DL: class disjointness + a qualified cardinality restriction. -->
  <owl:Class rdf:about="https://opentriplestore.org/demo/reasoning#Cat">
    <owl:disjointWith rdf:resource="https://opentriplestore.org/demo/reasoning#Dog"/>
  </owl:Class>
  <owl:Class rdf:about="https://opentriplestore.org/demo/reasoning#Parent">
    <rdfs:subClassOf>
      <owl:Restriction>
        <owl:onProperty rdf:resource="https://opentriplestore.org/demo/reasoning#hasChild"/>
        <owl:minQualifiedCardinality rdf:datatype="http://www.w3.org/2001/XMLSchema#nonNegativeInteger">1</owl:minQualifiedCardinality>
        <owl:onClass rdf:resource="https://opentriplestore.org/demo/reasoning#Person"/>
      </owl:Restriction>
    </rdfs:subClassOf>
  </owl:Class>
  <owl:ObjectProperty rdf:about="https://opentriplestore.org/demo/reasoning#hasChild"/>
</rdf:RDF>
"#;

const GEO_TTL: &str = r#"
@prefix ex:   <https://opentriplestore.org/demo/spatial#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix geo:  <http://www.opengis.net/ont/geosparql#> .

ex:Amsterdam a geo:Feature ; rdfs:label "Amsterdam" ;
    geo:hasGeometry [ a geo:Geometry ; geo:asWKT "POINT(4.9041 52.3676)"^^geo:wktLiteral ] .
ex:Rotterdam a geo:Feature ; rdfs:label "Rotterdam" ;
    geo:hasGeometry [ a geo:Geometry ; geo:asWKT "POINT(4.4777 51.9244)"^^geo:wktLiteral ] .
ex:Brussels a geo:Feature ; rdfs:label "Brussels" ;
    geo:hasGeometry [ a geo:Geometry ; geo:asWKT "POINT(4.3517 50.8503)"^^geo:wktLiteral ] .
ex:Paris a geo:Feature ; rdfs:label "Paris" ;
    geo:hasGeometry [ a geo:Geometry ; geo:asWKT "POINT(2.3522 48.8566)"^^geo:wktLiteral ] .
"#;

const SHACL_SHAPES_TTL: &str = r#"
@prefix sh:   <http://www.w3.org/ns/shacl#> .
@prefix ex:   <https://opentriplestore.org/demo/validation#> .
@prefix xsd:  <http://www.w3.org/2001/XMLSchema#> .

ex:PersonShape a sh:NodeShape ;
    sh:targetClass ex:Person ;
    sh:property ex:NameConstraint ;
    sh:property ex:AgeConstraint ;
    sh:sparql ex:AdultConstraint .

ex:NameConstraint sh:path ex:name ; sh:minCount 1 ; sh:datatype xsd:string .
ex:AgeConstraint  sh:path ex:age  ; sh:datatype xsd:integer ; sh:maxCount 1 .

# SHACL Advanced: a SPARQL-based constraint.
ex:AdultConstraint a sh:SPARQLConstraint ;
    sh:message "Person must be at least 18." ;
    sh:select "SELECT ?value WHERE { $this <https://opentriplestore.org/demo/validation#age> ?value . FILTER(?value < 18) }" .
"#;

const SHACL_DATA_NT: &str = r#"<https://opentriplestore.org/demo/validation#alice> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <https://opentriplestore.org/demo/validation#Person> .
<https://opentriplestore.org/demo/validation#alice> <https://opentriplestore.org/demo/validation#name> "Alice" .
<https://opentriplestore.org/demo/validation#alice> <https://opentriplestore.org/demo/validation#age> "30"^^<http://www.w3.org/2001/XMLSchema#integer> .
<https://opentriplestore.org/demo/validation#bob> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <https://opentriplestore.org/demo/validation#Person> .
<https://opentriplestore.org/demo/validation#bob> <https://opentriplestore.org/demo/validation#age> "15"^^<http://www.w3.org/2001/XMLSchema#integer> .
"#;

const SHEX_TTL: &str = r#"
@prefix ex:    <https://opentriplestore.org/demo/validation#> .
@prefix shex:  <http://www.w3.org/ns/shex#> .
@prefix xsd:   <http://www.w3.org/2001/XMLSchema#> .

# A ShEx schema represented in RDF (ShExR). The compact-syntax form and live
# validation are exercised by the e2e suite via /api/shex/validate.
ex:UserShape a shex:Shape ;
    shex:expression [
        a shex:TripleConstraint ;
        shex:predicate ex:email ;
        shex:valueExpr [ a shex:NodeConstraint ; shex:datatype xsd:string ]
    ] .

ex:carol a ex:User ; ex:email "carol@example.org" .
"#;

const SWRL_TTL: &str = r#"
@prefix ex:   <https://opentriplestore.org/demo/rules#> .
@prefix swrl: <http://www.w3.org/2003/11/swrl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .

ex:hasParent      a rdfs:Property .
ex:hasGrandparent a rdfs:Property .

# SWRL: hasParent(?x,?y) ∧ hasParent(?y,?z) ⇒ hasGrandparent(?x,?z)
ex:GrandparentRule a swrl:Imp ;
    rdfs:comment "hasParent(?x,?y) ^ hasParent(?y,?z) -> hasGrandparent(?x,?z)" ;
    swrl:body ( [ a swrl:IndividualPropertyAtom ; swrl:propertyPredicate ex:hasParent ]
                [ a swrl:IndividualPropertyAtom ; swrl:propertyPredicate ex:hasParent ] ) ;
    swrl:head ( [ a swrl:IndividualPropertyAtom ; swrl:propertyPredicate ex:hasGrandparent ] ) .

ex:Tom   ex:hasParent ex:Mary .
ex:Mary  ex:hasParent ex:Sophie .
"#;

const LDP_TTL: &str = r#"
@prefix ldp:  <http://www.w3.org/ns/ldp#> .
@prefix dct:  <http://purl.org/dc/terms/> .
@prefix ex:   <https://opentriplestore.org/demo/linked-data#> .

ex:notes a ldp:BasicContainer ;
    dct:title "Notes container" ;
    ldp:contains ex:note-1, ex:note-2 .

ex:note-1 dct:title "First note" .
ex:note-2 dct:title "Second note" .
"#;

const DCAT_TTL: &str = r#"
@prefix dcat: <http://www.w3.org/ns/dcat#> .
@prefix dct:  <http://purl.org/dc/terms/> .
@prefix ex:   <https://opentriplestore.org/demo/linked-data#> .

ex:catalog a dcat:Catalog ;
    dct:title "Open Triplestore demo catalog" ;
    dcat:dataset ex:cities-dataset .

ex:cities-dataset a dcat:Dataset ;
    dct:title "Cities" ;
    dct:description "Demo cities with geometries." ;
    dcat:distribution ex:cities-ttl, ex:cities-jsonld .

ex:cities-ttl a dcat:Distribution ;
    dcat:mediaType "text/turtle" ;
    dcat:downloadURL <https://opentriplestore.org/demo/spatial/geo.ttl> .
ex:cities-jsonld a dcat:Distribution ;
    dcat:mediaType "application/ld+json" ;
    dcat:downloadURL <https://opentriplestore.org/demo/spatial/geo.jsonld> .
"#;

const CAPABILITIES_JSONLD: &str = r#"{
  "@context": {
    "ots": "https://opentriplestore.org/ns#",
    "dct": "http://purl.org/dc/terms/",
    "title": "dct:title",
    "conformance": "ots:conformance",
    "Standard": "ots:Standard",
    "AuthMethod": "ots:AuthMethod"
  },
  "@graph": [
    { "@id": "ots:rdf11",      "@type": "Standard", "title": "RDF 1.1",                 "conformance": "Full" },
    { "@id": "ots:rdf12",      "@type": "Standard", "title": "RDF-star / RDF 1.2",      "conformance": "Full" },
    { "@id": "ots:sparql11",   "@type": "Standard", "title": "SPARQL 1.1 Query & Update","conformance": "Full" },
    { "@id": "ots:sparql12",   "@type": "Standard", "title": "SPARQL 1.2",              "conformance": "Full" },
    { "@id": "ots:gsp",        "@type": "Standard", "title": "SPARQL Graph Store HTTP", "conformance": "Full" },
    { "@id": "ots:sd",         "@type": "Standard", "title": "SPARQL Service Description","conformance": "Full" },
    { "@id": "ots:rdfs",       "@type": "Standard", "title": "RDFS",                    "conformance": "Full" },
    { "@id": "ots:owlql",      "@type": "Standard", "title": "OWL 2 QL",                "conformance": "Full" },
    { "@id": "ots:owlel",      "@type": "Standard", "title": "OWL 2 EL",                "conformance": "Full" },
    { "@id": "ots:owlrl",      "@type": "Standard", "title": "OWL 2 RL",                "conformance": "Full" },
    { "@id": "ots:owldl",      "@type": "Standard", "title": "OWL 2 DL",                "conformance": "Full" },
    { "@id": "ots:geosparql",  "@type": "Standard", "title": "GeoSPARQL 1.1",           "conformance": "Full" },
    { "@id": "ots:shaclcore",  "@type": "Standard", "title": "SHACL Core",              "conformance": "Full" },
    { "@id": "ots:shacladv",   "@type": "Standard", "title": "SHACL Advanced",          "conformance": "Full" },
    { "@id": "ots:shaclc",     "@type": "Standard", "title": "SHACL Compact Syntax",    "conformance": "Full" },
    { "@id": "ots:shex",       "@type": "Standard", "title": "ShEx",                    "conformance": "Full" },
    { "@id": "ots:swrl",       "@type": "Standard", "title": "SWRL",                    "conformance": "Full" },
    { "@id": "ots:ldp",        "@type": "Standard", "title": "Linked Data Platform 1.0","conformance": "Full" },
    { "@id": "ots:dcat",       "@type": "Standard", "title": "DCAT 2 / VoID",           "conformance": "Full" },
    { "@id": "ots:jwt",        "@type": "AuthMethod", "title": "JSON Web Tokens (JWT)" },
    { "@id": "ots:oauth",      "@type": "AuthMethod", "title": "OAuth 2.0 / OIDC" },
    { "@id": "ots:saml",       "@type": "AuthMethod", "title": "SAML 2.0 SSO" }
  ]
}"#;
