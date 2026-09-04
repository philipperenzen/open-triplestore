//! buildingSMART IDS (Information Delivery Specification, 1.0) → SHACL Core.
//!
//! Every `ids:specification` becomes a node shape targeting the applicable
//! entity's ifcOWL class. Applicability facets beyond the entity (properties,
//! attributes, classification, material, part-of) become a separate
//! "applies" shape and the requirements a "requires" shape, combined as the
//! implication `sh:or ( [ sh:not applies ] requires )` — SHACL Core, no
//! SPARQL. Facets map to the RDF the built-in IFC importer emits:
//!
//! | IDS facet | RDF |
//! |---|---|
//! | entity name | `rdf:type ifc:<Entity>` (ifcOWL) |
//! | property `Pset.Name` | `props:<Pset>_<Name>` |
//! | attribute `Name` / `GlobalId` / other | `props:ifcName` / `props:ifcGuid` / `props:ifc<Attr>` |
//! | partOf `IFCRELCONTAINEDINSPATIALSTRUCTURE` | `^bot:containsElement` |
//! | partOf `IFCRELAGGREGATES` | `^bot:hasSubElement` |
//! | classification / material | `props:ifcClassification` / `props:ifcMaterial` (convention) |
//!
//! Value restrictions: `simpleValue` → `sh:hasValue`; `xs:enumeration` →
//! `sh:in`; `xs:pattern` → `sh:pattern`; bounds → `sh:min/maxInclusive` /
//! `Exclusive`; lengths → `sh:min/maxLength`. Cardinality: `required` →
//! `sh:minCount 1`, `prohibited` → `sh:maxCount 0`, `optional` → value
//! constraints only. Anything approximated is listed in the report.

use std::fmt::Write as _;

use quick_xml::events::Event;
use quick_xml::Reader;

use super::{ImportedShapes, SpecImporter, SpecSummary};

pub struct IdsImporter;

impl SpecImporter for IdsImporter {
    fn id(&self) -> &'static str {
        "ids"
    }
    fn label(&self) -> &'static str {
        "buildingSMART IDS 1.0"
    }
    fn media_types(&self) -> &'static [&'static str] {
        &["application/xml", "text/xml"]
    }
    fn import(&self, bytes: &[u8]) -> anyhow::Result<ImportedShapes> {
        let doc = parse_xml(bytes)?;
        convert(&doc)
    }
}

// ── a minimal DOM ───────────────────────────────────────────────────────────

#[derive(Debug, Default, Clone)]
pub(crate) struct El {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub text: String,
    pub children: Vec<El>,
}

impl El {
    fn attr(&self, k: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(a, _)| a == k)
            .map(|(_, v)| v.as_str())
    }
    fn child(&self, name: &str) -> Option<&El> {
        self.children.iter().find(|c| c.name == name)
    }
    fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a El> + 'a {
        self.children.iter().filter(move |c| c.name == name)
    }
    /// Text of `<name><ids:simpleValue>…</ids:simpleValue></name>`.
    fn simple(&self, name: &str) -> Option<String> {
        self.child(name)
            .and_then(|c| c.child("simpleValue"))
            .map(|s| s.text.trim().to_string())
            .filter(|s| !s.is_empty())
    }
}

fn local(name: &str) -> String {
    name.rsplit(':').next().unwrap_or(name).to_string()
}

pub(crate) fn parse_xml(bytes: &[u8]) -> anyhow::Result<El> {
    let mut reader = Reader::from_reader(bytes);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut stack: Vec<El> = vec![El {
        name: "#root".into(),
        ..Default::default()
    }];
    fn el_from(e: &quick_xml::events::BytesStart<'_>) -> El {
        let mut el = El {
            name: local(e.name().as_ref()),
            ..Default::default()
        };
        for a in e.attributes().flatten() {
            el.attrs.push((
                local(a.key.as_ref()),
                a.normalized_value(quick_xml::XmlVersion::default())
                    .map(|v| v.into_owned())
                    .unwrap_or_default(),
            ));
        }
        el
    }
    loop {
        match reader.read_event_into(&mut buf)? {
            Event::Start(e) => stack.push(el_from(&e)),
            Event::Empty(e) => {
                let el = el_from(&e);
                stack.last_mut().unwrap().children.push(el);
            }
            Event::End(_) => {
                if stack.len() > 1 {
                    let el = stack.pop().unwrap();
                    stack.last_mut().unwrap().children.push(el);
                }
            }
            Event::Text(t) => {
                let s = t.into_inner();
                stack.last_mut().unwrap().text.push_str(&s);
            }
            Event::CData(c) => {
                let s = c.into_inner();
                stack.last_mut().unwrap().text.push_str(&s);
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    let mut root = stack.pop().ok_or_else(|| anyhow::anyhow!("no XML root"))?;
    while stack.len() > 1 {
        // Unclosed elements: fold them in rather than lose them.
        let parent = stack.pop().unwrap();
        let mut parent = parent;
        parent.children.push(root);
        root = parent;
    }
    if root.name == "#root" {
        root = root
            .children
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("empty document"))?;
    }
    Ok(root)
}

// ── vocabulary ──────────────────────────────────────────────────────────────

const IFC4_OWL: &str = "https://standards.buildingsmart.org/IFC/DEV/IFC4/ADD2_TC1/OWL#";
const IFC2X3_OWL: &str = "https://standards.buildingsmart.org/IFC/DEV/IFC2x3/TC1/OWL#";
const PROPS: &str = "https://w3id.org/props#";
const BOT: &str = "https://w3id.org/bot#";
const SHAPE_NS: &str = "urn:ids:";

/// ifcOWL class names for the upper-case IDS entity names (IDS writes
/// `IFCWALL`, ifcOWL `IfcWall`). Unknown names are title-cased with a warning.
const IFC_CLASSES: &[&str] = &[
    "IfcActuator",
    "IfcAirTerminal",
    "IfcAirTerminalBox",
    "IfcAirToAirHeatRecovery",
    "IfcAlarm",
    "IfcAnnotation",
    "IfcAudioVisualAppliance",
    "IfcBeam",
    "IfcBeamStandardCase",
    "IfcBearing",
    "IfcBoiler",
    "IfcBridge",
    "IfcBridgePart",
    "IfcBuilding",
    "IfcBuildingElementPart",
    "IfcBuildingElementProxy",
    "IfcBuildingStorey",
    "IfcBuildingSystem",
    "IfcBurner",
    "IfcCableCarrierFitting",
    "IfcCableCarrierSegment",
    "IfcCableFitting",
    "IfcCableSegment",
    "IfcCaissonFoundation",
    "IfcChiller",
    "IfcChimney",
    "IfcCoil",
    "IfcColumn",
    "IfcColumnStandardCase",
    "IfcCommunicationsAppliance",
    "IfcCompressor",
    "IfcCondenser",
    "IfcController",
    "IfcCooledBeam",
    "IfcCoolingTower",
    "IfcCourse",
    "IfcCovering",
    "IfcCurtainWall",
    "IfcDamper",
    "IfcDeepFoundation",
    "IfcDiscreteAccessory",
    "IfcDistributionChamberElement",
    "IfcDistributionControlElement",
    "IfcDistributionElement",
    "IfcDistributionFlowElement",
    "IfcDistributionPort",
    "IfcDistributionSystem",
    "IfcDoor",
    "IfcDoorStandardCase",
    "IfcDuctFitting",
    "IfcDuctSegment",
    "IfcDuctSilencer",
    "IfcEarthworksCut",
    "IfcEarthworksElement",
    "IfcEarthworksFill",
    "IfcElectricAppliance",
    "IfcElectricDistributionBoard",
    "IfcElectricFlowStorageDevice",
    "IfcElectricGenerator",
    "IfcElectricMotor",
    "IfcElectricTimeControl",
    "IfcElement",
    "IfcElementAssembly",
    "IfcEnergyConversionDevice",
    "IfcEngine",
    "IfcEvaporativeCooler",
    "IfcEvaporator",
    "IfcExternalSpatialElement",
    "IfcFacility",
    "IfcFacilityPart",
    "IfcFan",
    "IfcFastener",
    "IfcFilter",
    "IfcFireSuppressionTerminal",
    "IfcFlowController",
    "IfcFlowFitting",
    "IfcFlowInstrument",
    "IfcFlowMeter",
    "IfcFlowMovingDevice",
    "IfcFlowSegment",
    "IfcFlowStorageDevice",
    "IfcFlowTerminal",
    "IfcFlowTreatmentDevice",
    "IfcFooting",
    "IfcFurnishingElement",
    "IfcFurniture",
    "IfcGeographicElement",
    "IfcGeotechnicalElement",
    "IfcGrid",
    "IfcGroup",
    "IfcHeatExchanger",
    "IfcHumidifier",
    "IfcInterceptor",
    "IfcJunctionBox",
    "IfcKerb",
    "IfcLamp",
    "IfcLightFixture",
    "IfcMarineFacility",
    "IfcMechanicalFastener",
    "IfcMedicalDevice",
    "IfcMember",
    "IfcMemberStandardCase",
    "IfcMotorConnection",
    "IfcNavigationElement",
    "IfcOpeningElement",
    "IfcOpeningStandardCase",
    "IfcOutlet",
    "IfcPavement",
    "IfcPile",
    "IfcPipeFitting",
    "IfcPipeSegment",
    "IfcPlate",
    "IfcPlateStandardCase",
    "IfcProduct",
    "IfcProject",
    "IfcProtectiveDevice",
    "IfcProtectiveDeviceTrippingUnit",
    "IfcPump",
    "IfcRail",
    "IfcRailing",
    "IfcRailway",
    "IfcRailwayPart",
    "IfcRamp",
    "IfcRampFlight",
    "IfcReinforcingBar",
    "IfcReinforcingElement",
    "IfcReinforcingMesh",
    "IfcRoad",
    "IfcRoadPart",
    "IfcRoof",
    "IfcSanitaryTerminal",
    "IfcSensor",
    "IfcShadingDevice",
    "IfcSign",
    "IfcSignal",
    "IfcSite",
    "IfcSlab",
    "IfcSlabElementedCase",
    "IfcSlabStandardCase",
    "IfcSolarDevice",
    "IfcSpace",
    "IfcSpaceHeater",
    "IfcSpatialElement",
    "IfcSpatialStructureElement",
    "IfcSpatialZone",
    "IfcStackTerminal",
    "IfcStair",
    "IfcStairFlight",
    "IfcStructuralMember",
    "IfcSwitchingDevice",
    "IfcSystem",
    "IfcSystemFurnitureElement",
    "IfcTank",
    "IfcTendon",
    "IfcTendonAnchor",
    "IfcTrackElement",
    "IfcTransformer",
    "IfcTransportElement",
    "IfcTubeBundle",
    "IfcUnitaryControlElement",
    "IfcUnitaryEquipment",
    "IfcValve",
    "IfcVehicle",
    "IfcVibrationDamper",
    "IfcVibrationIsolator",
    "IfcVirtualElement",
    "IfcWall",
    "IfcWallElementedCase",
    "IfcWallStandardCase",
    "IfcWasteTerminal",
    "IfcWindow",
    "IfcWindowStandardCase",
    "IfcZone",
];

fn ifc_class(name: &str, warnings: &mut Vec<String>) -> String {
    let upper = name.trim().to_ascii_uppercase();
    if let Some(c) = IFC_CLASSES.iter().find(|c| c.to_ascii_uppercase() == upper) {
        return (*c).to_string();
    }
    let rest = upper.strip_prefix("IFC").unwrap_or(&upper);
    let mut chars = rest.chars();
    let guess = match chars.next() {
        Some(f) => format!("Ifc{}{}", f, chars.as_str().to_ascii_lowercase()),
        None => "IfcProduct".to_string(),
    };
    warnings.push(format!(
        "entity `{name}` is not a known ifcOWL class name; using `{guess}`"
    ));
    guess
}

/// The IFC importer's property-name sanitiser.
fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn ttl_str(s: &str) -> String {
    format!(
        "\"{}\"",
        s.replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
    )
}

/// `xsd:` datatype for an IDS `dataType` (IFC defined type), where the built-in
/// importer emits one; numeric measures are left untyped (their RDF datatype
/// depends on the importer's number handling).
fn xsd_for(ifc_type: &str) -> Option<&'static str> {
    let t = ifc_type.trim().to_ascii_uppercase();
    match t.as_str() {
        "IFCLABEL"
        | "IFCTEXT"
        | "IFCIDENTIFIER"
        | "IFCDESCRIPTIVEMEASURE"
        | "IFCPRESENTABLETEXT"
        | "IFCURIREFERENCE"
        | "IFCDATE"
        | "IFCDATETIME"
        | "IFCTIME"
        | "IFCDURATION" => Some("xsd:string"),
        "IFCBOOLEAN" | "IFCLOGICAL" => Some("xsd:boolean"),
        "IFCINTEGER" | "IFCCOUNTMEASURE" | "IFCINTEGERCOUNTRATEMEASURE" => Some("xsd:integer"),
        _ => None,
    }
}

// ── conversion ──────────────────────────────────────────────────────────────

/// A value restriction, as SHACL property-constraint lines.
fn value_constraints(
    value: Option<&El>,
    datatype: Option<&str>,
    warnings: &mut Vec<String>,
) -> Vec<String> {
    let mut out = Vec::new();
    let Some(v) = value else { return out };
    if let Some(s) = v.child("simpleValue") {
        let s = s.text.trim();
        let lit = match datatype {
            Some("xsd:boolean") => format!("{}", s.eq_ignore_ascii_case("true")),
            Some("xsd:integer") if s.parse::<i64>().is_ok() => s.to_string(),
            _ => ttl_str(s),
        };
        out.push(format!("sh:hasValue {lit}"));
        return out;
    }
    if let Some(r) = v.child("restriction") {
        let base = r.attr("base").unwrap_or("xs:string");
        let enums: Vec<String> = r
            .children_named("enumeration")
            .filter_map(|e| e.attr("value"))
            .map(|s| match datatype {
                Some("xsd:integer") if s.parse::<i64>().is_ok() => s.to_string(),
                Some("xsd:boolean") => s.eq_ignore_ascii_case("true").to_string(),
                _ => ttl_str(s),
            })
            .collect();
        if !enums.is_empty() {
            out.push(format!("sh:in ( {} )", enums.join(" ")));
        }
        if let Some(p) = r.child("pattern").and_then(|p| p.attr("value")) {
            out.push(format!("sh:pattern {}", ttl_str(p)));
        }
        for (facet, sh) in [
            ("minInclusive", "sh:minInclusive"),
            ("maxInclusive", "sh:maxInclusive"),
            ("minExclusive", "sh:minExclusive"),
            ("maxExclusive", "sh:maxExclusive"),
            ("minLength", "sh:minLength"),
            ("maxLength", "sh:maxLength"),
        ] {
            if let Some(val) = r.child(facet).and_then(|f| f.attr("value")) {
                out.push(format!("{sh} {val}"));
            }
        }
        if let Some(len) = r.child("length").and_then(|f| f.attr("value")) {
            out.push(format!("sh:minLength {len}"));
            out.push(format!("sh:maxLength {len}"));
        }
        if out.is_empty() {
            warnings.push(format!(
                "value restriction on {base} has no facet this importer maps (enumeration, pattern, bounds, length)"
            ));
        }
    }
    out
}

/// Cardinality of a facet: IDS 1.0 `cardinality`, IDS 0.9 `minOccurs`/`maxOccurs`.
fn cardinality(f: &El) -> &'static str {
    match f.attr("cardinality").map(|c| c.to_ascii_lowercase()) {
        Some(c) if c == "prohibited" => "prohibited",
        Some(c) if c == "optional" => "optional",
        Some(_) => "required",
        None => {
            if f.attr("maxOccurs") == Some("0") {
                "prohibited"
            } else if f.attr("minOccurs") == Some("0") {
                "optional"
            } else {
                "required"
            }
        }
    }
}

/// One facet as a property shape body (`sh:path …; …`), or an entity/partOf
/// constraint. `as_requirement` applies cardinality; applicability facets are
/// pure conditions.
fn facet_constraint(f: &El, as_requirement: bool, warnings: &mut Vec<String>) -> Option<String> {
    let card = if as_requirement {
        cardinality(f)
    } else {
        "optional"
    };
    let count = |lines: &mut Vec<String>| match card {
        "prohibited" => lines.push("sh:maxCount 0".into()),
        "required" => lines.push("sh:minCount 1".into()),
        _ => {}
    };
    match f.name.as_str() {
        "property" => {
            let pset = f.simple("propertySet").unwrap_or_default();
            let name = f
                .simple("baseName")
                .or_else(|| f.simple("name"))
                .unwrap_or_default();
            if pset.is_empty() || name.is_empty() {
                warnings.push("property facet needs a simpleValue propertySet and baseName; enumerated names are not expanded".into());
                return None;
            }
            let path = format!("props:{}_{}", sanitize(&pset), sanitize(&name));
            let dt = f
                .attr("dataType")
                .or_else(|| f.attr("datatype"))
                .and_then(xsd_for);
            let mut lines = vec![
                format!("sh:path {path}"),
                format!("sh:name {}", ttl_str(&format!("{pset}.{name}"))),
            ];
            if card != "prohibited" {
                if let Some(dt) = dt {
                    lines.push(format!("sh:datatype {dt}"));
                }
                lines.extend(value_constraints(f.child("value"), dt, warnings));
            }
            count(&mut lines);
            Some(lines.join(" ;\n        "))
        }
        "attribute" => {
            let name = f.simple("name").unwrap_or_default();
            if name.is_empty() {
                warnings.push("attribute facet without a simpleValue name skipped".into());
                return None;
            }
            let path = match name.as_str() {
                "Name" => "props:ifcName".to_string(),
                "GlobalId" => "props:ifcGuid".to_string(),
                other => {
                    warnings.push(format!(
                        "attribute `{other}` maps to props:ifc{other}, which the built-in IFC importer does not populate"
                    ));
                    format!("props:ifc{}", sanitize(other))
                }
            };
            let mut lines = vec![
                format!("sh:path {path}"),
                format!("sh:name {}", ttl_str(&name)),
            ];
            if card != "prohibited" {
                lines.extend(value_constraints(
                    f.child("value"),
                    Some("xsd:string"),
                    warnings,
                ));
            }
            count(&mut lines);
            Some(lines.join(" ;\n        "))
        }
        "classification" | "material" => {
            let (path, label) = if f.name == "classification" {
                ("props:ifcClassification", "classification")
            } else {
                ("props:ifcMaterial", "material")
            };
            warnings.push(format!(
                "{label} facet maps to {path} by convention; the built-in IFC importer does not emit it"
            ));
            let mut lines = vec![format!("sh:path {path}")];
            if let Some(sys) = f.simple("system") {
                lines.push(format!(
                    "sh:description {}",
                    ttl_str(&format!("system: {sys}"))
                ));
            }
            if card != "prohibited" {
                lines.extend(value_constraints(
                    f.child("value"),
                    Some("xsd:string"),
                    warnings,
                ));
            }
            count(&mut lines);
            Some(lines.join(" ;\n        "))
        }
        "partOf" => {
            let relation = f
                .attr("relation")
                .unwrap_or("IFCRELCONTAINEDINSPATIALSTRUCTURE")
                .to_ascii_uppercase();
            let path = match relation.as_str() {
                "IFCRELCONTAINEDINSPATIALSTRUCTURE" => {
                    "[ sh:inversePath bot:containsElement ]".to_string()
                }
                "IFCRELAGGREGATES" => "[ sh:inversePath bot:hasSubElement ]".to_string(),
                other => {
                    warnings.push(format!(
                        "partOf relation {other} has no BOT equivalent; using ots:partOf_{other}"
                    ));
                    format!("ots:partOf_{other}")
                }
            };
            let mut lines = vec![format!("sh:path {path}")];
            if let Some(ent) = f.child("entity").and_then(|e| e.simple("name")) {
                lines.push(format!("sh:class ifc:{}", ifc_class(&ent, warnings)));
            }
            count(&mut lines);
            Some(lines.join(" ;\n        "))
        }
        "entity" => None, // handled as the target
        other => {
            warnings.push(format!("facet `{other}` is not supported"));
            None
        }
    }
}

fn entity_classes(applicability: &El, warnings: &mut Vec<String>) -> Vec<String> {
    let mut out = Vec::new();
    for e in applicability.children_named("entity") {
        if let Some(n) = e.simple("name") {
            out.push(ifc_class(&n, warnings));
        } else if let Some(r) = e.child("name").and_then(|n| n.child("restriction")) {
            for v in r
                .children_named("enumeration")
                .filter_map(|x| x.attr("value"))
            {
                out.push(ifc_class(v, warnings));
            }
        }
        if let Some(pt) = e.simple("predefinedType") {
            warnings.push(format!(
                "predefinedType `{pt}` restricts applicability via props:ifcPredefinedType, which the built-in IFC importer does not populate"
            ));
        }
    }
    out
}

pub(crate) fn convert(doc: &El) -> anyhow::Result<ImportedShapes> {
    if doc.name != "ids" {
        anyhow::bail!("not an IDS document (root element is `{}`)", doc.name);
    }
    let info = doc.child("info");
    let title = info
        .and_then(|i| i.child("title"))
        .map(|t| t.text.trim().to_string())
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| "IDS import".to_string());
    let description = info
        .and_then(|i| i.child("description"))
        .map(|t| t.text.trim().to_string())
        .filter(|t| !t.is_empty());
    let specs: Vec<&El> = doc
        .child("specifications")
        .map(|s| s.children_named("specification").collect())
        .unwrap_or_default();
    if specs.is_empty() {
        anyhow::bail!("the IDS has no ids:specification");
    }

    let mut warnings = Vec::new();
    let mut ttl = String::new();
    ttl.push_str("@prefix sh: <http://www.w3.org/ns/shacl#> .\n");
    ttl.push_str("@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .\n");
    ttl.push_str("@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n");
    ttl.push_str("@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .\n");
    ttl.push_str("@prefix dct: <http://purl.org/dc/terms/> .\n");
    ttl.push_str(&format!("@prefix ifc: <{IFC4_OWL}> .\n"));
    ttl.push_str(&format!("@prefix ifc2x3: <{IFC2X3_OWL}> .\n"));
    ttl.push_str(&format!("@prefix props: <{PROPS}> .\n"));
    ttl.push_str(&format!("@prefix bot: <{BOT}> .\n"));
    ttl.push_str("@prefix ots: <https://opentriplestore.org/ns#> .\n\n");
    writeln!(ttl, "<{SHAPE_NS}spec> dct:title {} ;", ttl_str(&title)).unwrap();
    if let Some(d) = &description {
        writeln!(ttl, "    dct:description {} ;", ttl_str(d)).unwrap();
    }
    writeln!(
        ttl,
        "    dct:conformsTo <https://standards.buildingsmart.org/IDS> .\n"
    )
    .unwrap();

    let mut summaries = Vec::new();
    let mut shape_count = 0;
    for (i, spec) in specs.iter().enumerate() {
        let n = i + 1;
        let name = spec
            .attr("name")
            .map(str::to_string)
            .unwrap_or_else(|| format!("Specification {n}"));
        let shape = format!("{SHAPE_NS}spec{n}");
        let ifc_version = spec.attr("ifcVersion").unwrap_or("IFC4");
        let prefix = if ifc_version.to_ascii_uppercase().starts_with("IFC2X3") {
            "ifc2x3"
        } else {
            "ifc"
        };
        let Some(applicability) = spec.child("applicability") else {
            warnings.push(format!(
                "specification `{name}` has no applicability; skipped"
            ));
            continue;
        };
        let classes = entity_classes(applicability, &mut warnings);
        if classes.is_empty() {
            warnings.push(format!(
                "specification `{name}` names no entity; SHACL needs a target class — skipped"
            ));
            continue;
        }
        let applies: Vec<String> = applicability
            .children
            .iter()
            .filter(|f| f.name != "entity")
            .filter_map(|f| facet_constraint(f, false, &mut warnings))
            .collect();
        let requirements: Vec<String> = spec
            .child("requirements")
            .map(|r| {
                r.children
                    .iter()
                    .filter_map(|f| facet_constraint(f, true, &mut warnings))
                    .collect()
            })
            .unwrap_or_default();
        let prohibited_spec = spec.attr("maxOccurs") == Some("0");
        if spec.attr("minOccurs").is_some_and(|m| m != "0") && !prohibited_spec {
            warnings.push(format!(
                "specification `{name}` requires at least one applicable entity to exist; SHACL validates per node, so existence is not enforced"
            ));
        }

        writeln!(ttl, "<{shape}> a sh:NodeShape ;").unwrap();
        writeln!(ttl, "    sh:name {} ;", ttl_str(&name)).unwrap();
        if let Some(d) = spec.attr("description") {
            writeln!(ttl, "    sh:description {} ;", ttl_str(d)).unwrap();
        }
        writeln!(
            ttl,
            "    rdfs:comment {} ;",
            ttl_str(&format!("IDS specification {n} ({ifc_version})"))
        )
        .unwrap();
        for c in &classes {
            writeln!(ttl, "    sh:targetClass {prefix}:{c} ;").unwrap();
        }
        let props_block = |lines: &[String]| -> String {
            lines
                .iter()
                .map(|l| format!("    sh:property [\n        {l}\n    ]"))
                .collect::<Vec<_>>()
                .join(" ;\n")
        };
        if prohibited_spec {
            // No applicable entity may exist: every target violates.
            writeln!(ttl, "    sh:not [ sh:class {prefix}:{} ] .\n", classes[0]).unwrap();
        } else if applies.is_empty() {
            if requirements.is_empty() {
                writeln!(ttl, "    sh:deactivated false .\n").unwrap();
            } else {
                writeln!(ttl, "{} .\n", props_block(&requirements)).unwrap();
            }
        } else {
            writeln!(
                ttl,
                "    sh:or ( [ sh:not <{shape}-applies> ] <{shape}-requires> ) .\n"
            )
            .unwrap();
            writeln!(ttl, "<{shape}-applies> a sh:NodeShape ;").unwrap();
            writeln!(
                ttl,
                "    sh:name {} ;",
                ttl_str(&format!("{name} — applicability"))
            )
            .unwrap();
            writeln!(ttl, "{} .\n", props_block(&applies)).unwrap();
            writeln!(ttl, "<{shape}-requires> a sh:NodeShape ;").unwrap();
            writeln!(
                ttl,
                "    sh:name {} ;",
                ttl_str(&format!("{name} — requirements"))
            )
            .unwrap();
            if requirements.is_empty() {
                writeln!(ttl, "    sh:deactivated false .\n").unwrap();
            } else {
                writeln!(ttl, "{} .\n", props_block(&requirements)).unwrap();
            }
            shape_count += 2;
        }
        shape_count += 1;
        summaries.push(SpecSummary {
            name,
            shape,
            target_classes: classes.iter().map(|c| format!("{prefix}:{c}")).collect(),
            requirements: requirements.len(),
        });
    }
    if summaries.is_empty() {
        anyhow::bail!(
            "no specification could be converted: {}",
            warnings.join("; ")
        );
    }
    Ok(ImportedShapes {
        title,
        description,
        turtle: ttl,
        shape_count,
        specifications: summaries,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) const SAMPLE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<ids:ids xmlns:ids="http://standards.buildingsmart.org/IDS" xmlns:xs="http://www.w3.org/2001/XMLSchema">
  <ids:info><ids:title>Wall fire ratings</ids:title><ids:description>External walls carry a fire rating.</ids:description></ids:info>
  <ids:specifications>
    <ids:specification name="External walls need a fire rating" ifcVersion="IFC4" minOccurs="1" maxOccurs="unbounded">
      <ids:applicability>
        <ids:entity><ids:name><ids:simpleValue>IFCWALL</ids:simpleValue></ids:name></ids:entity>
        <ids:property><ids:propertySet><ids:simpleValue>Pset_WallCommon</ids:simpleValue></ids:propertySet><ids:baseName><ids:simpleValue>IsExternal</ids:simpleValue></ids:baseName><ids:value><ids:simpleValue>true</ids:simpleValue></ids:value></ids:property>
      </ids:applicability>
      <ids:requirements>
        <ids:property cardinality="required" dataType="IFCLABEL"><ids:propertySet><ids:simpleValue>Pset_WallCommon</ids:simpleValue></ids:propertySet><ids:baseName><ids:simpleValue>FireRating</ids:simpleValue></ids:baseName><ids:value><xs:restriction base="xs:string"><xs:enumeration value="REI30"/><xs:enumeration value="REI60"/></xs:restriction></ids:value></ids:property>
        <ids:attribute cardinality="required"><ids:name><ids:simpleValue>Name</ids:simpleValue></ids:name></ids:attribute>
      </ids:requirements>
    </ids:specification>
    <ids:specification name="Windows sit in a storey" ifcVersion="IFC4">
      <ids:applicability><ids:entity><ids:name><ids:simpleValue>IFCWINDOW</ids:simpleValue></ids:name></ids:entity></ids:applicability>
      <ids:requirements><ids:partOf cardinality="required" relation="IFCRELCONTAINEDINSPATIALSTRUCTURE"><ids:entity><ids:name><ids:simpleValue>IFCBUILDINGSTOREY</ids:simpleValue></ids:name></ids:entity></ids:partOf></ids:requirements>
    </ids:specification>
  </ids:specifications>
</ids:ids>"#;

    #[test]
    fn xml_dom_handles_nesting_empty_elements_and_attributes() {
        let doc = parse_xml(SAMPLE.as_bytes()).unwrap();
        assert_eq!(doc.name, "ids");
        let specs = doc.child("specifications").unwrap();
        assert_eq!(specs.children_named("specification").count(), 2);
        let first = specs.child("specification").unwrap();
        assert_eq!(
            first.attr("name"),
            Some("External walls need a fire rating")
        );
        let enums: Vec<_> = first
            .child("requirements")
            .unwrap()
            .child("property")
            .unwrap()
            .child("value")
            .unwrap()
            .child("restriction")
            .unwrap()
            .children_named("enumeration")
            .filter_map(|e| e.attr("value"))
            .collect();
        assert_eq!(enums, vec!["REI30", "REI60"]);
    }

    #[test]
    fn ids_maps_to_shacl_core_over_the_ifc_rdf_vocabulary() {
        let out = IdsImporter.import(SAMPLE.as_bytes()).unwrap();
        assert_eq!(out.title, "Wall fire ratings");
        assert_eq!(out.specifications.len(), 2);
        let t = &out.turtle;
        assert!(t.contains("sh:targetClass ifc:IfcWall"), "{t}");
        assert!(
            t.contains("sh:or ( [ sh:not <urn:ids:spec1-applies> ] <urn:ids:spec1-requires> )"),
            "{t}"
        );
        assert!(t.contains("sh:path props:Pset_WallCommon_IsExternal ;\n        sh:name \"Pset_WallCommon.IsExternal\" ;\n        sh:hasValue \"true\""), "{t}");
        assert!(
            t.contains("sh:path props:Pset_WallCommon_FireRating"),
            "{t}"
        );
        assert!(t.contains("sh:datatype xsd:string"), "{t}");
        assert!(t.contains("sh:in ( \"REI30\" \"REI60\" )"), "{t}");
        assert!(
            t.contains(
                "sh:path props:ifcName ;\n        sh:name \"Name\" ;\n        sh:minCount 1"
            ),
            "{t}"
        );
        assert!(t.contains("sh:targetClass ifc:IfcWindow"), "{t}");
        assert!(t.contains("sh:path [ sh:inversePath bot:containsElement ] ;\n        sh:class ifc:IfcBuildingStorey ;\n        sh:minCount 1"), "{t}");
        // The applicability's IsExternal has no dataType attribute → typed as a string literal
        // (documented convention); the existence requirement is reported, not enforced.
        assert!(
            out.warnings
                .iter()
                .any(|w| w.contains("existence is not enforced")),
            "{:?}",
            out.warnings
        );
        // It is valid Turtle.
        let tmp = crate::store::TripleStore::in_memory().unwrap();
        tmp.load_str(t, oxigraph::io::RdfFormat::Turtle, Some("urn:x"))
            .expect("valid Turtle");
    }

    #[test]
    fn unknown_entities_are_title_cased_with_a_warning_and_non_ids_is_refused() {
        let mut w = Vec::new();
        assert_eq!(ifc_class("IFCFOOBAR", &mut w), "IfcFoobar");
        assert_eq!(w.len(), 1);
        assert_eq!(
            ifc_class("ifcwallstandardcase", &mut w),
            "IfcWallStandardCase"
        );
        assert_eq!(w.len(), 1);
        let err = IdsImporter.import(b"<root/>").unwrap_err().to_string();
        assert!(err.contains("not an IDS document"), "{err}");
    }
}
