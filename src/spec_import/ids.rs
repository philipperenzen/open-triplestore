//! buildingSMART IDS (Information Delivery Specification, 1.0) → SHACL.
//!
//! Every `ids:specification` becomes a node shape targeting the applicable
//! entity's ifcOWL class, in the namespace of each schema its `ifcVersion`
//! list names (`IFC2X3 IFC4` targets both). Applicability facets beyond the
//! entity (properties, attributes, classification, material, part-of) become a
//! separate "applies" shape and the requirements a "requires" shape, combined
//! as the implication `sh:or ( [ sh:not applies ] requires )`. Facets map to
//! the RDF the built-in IFC importer emits:
//!
//! | IDS facet | RDF |
//! |---|---|
//! | entity name | `rdf:type ifc:<Entity>` (ifcOWL); a pattern is expanded against the schema's entities |
//! | property `Pset.Name` | `props:<Pset>_<Name>` |
//! | attribute `Name` / `GlobalId` / other | `props:ifcName` / `props:ifcGuid` / `props:ifc<Attr>` |
//! | partOf `IFCRELCONTAINEDINSPATIALSTRUCTURE` | `^bot:containsElement` |
//! | partOf `IFCRELAGGREGATES` | `^bot:hasSubElement` |
//! | classification / material | `props:ifcClassification` / `props:ifcMaterial` (the lift emits both) |
//!
//! Values are compared by the XSD base type of the facet's IDS `dataType`
//! (the IDS data-type table) or of the restriction's `base`: a double
//! `simpleValue` becomes the tolerance range of the IDS implementer
//! documentation (`v ± (|v|·1e-6 + 1e-6)`, the bound included as the corpus requires), an integer a closed
//! range, a boolean or string `sh:hasValue`; `xs:enumeration` → `sh:in` (or
//! `sh:or` of ranges for numbers); `xs:pattern` → an anchored, translated
//! `sh:pattern` (several are alternatives); bounds → typed
//! `sh:min/maxInclusive` / `Exclusive` with no tolerance; lengths →
//! `sh:min/maxLength`.
//!
//! Cardinality: `required` → `sh:minCount 1` plus the value constraints;
//! `prohibited` → `sh:not` of the required facet (the opposite of required,
//! not a count of zero); `optional` → the value constraints only. In the
//! applicability every facet has `required` semantics. A required
//! specification (the XSD default) also gets an existence shape: a SPARQL
//! constraint that fails when no node of an applicable class exists. A
//! prohibited specification violates on every applicable node and may not
//! carry requirements. Anything approximated is listed in the report.

use std::fmt::Write as _;

use quick_xml::events::Event;
use quick_xml::Reader;

use super::{xsd_regex, ImportedShapes, SpecImporter, SpecSummary};
use crate::ifc::schema::{self, SchemaId, XsBase};

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

// ── values ──────────────────────────────────────────────────────────────

/// The IDS tolerance for floating-point equality (implementer docs,
/// tolerance.md): `x == v ⇔ v − |v|·ε − ε < x < v + |v|·ε + ε`.
const TOLERANCE: f64 = 1.0e-6;

/// How a value is compared: the XSD base type of the facet's data type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Base {
    String,
    Double,
    Integer,
    Boolean,
}

impl Base {
    fn of(xs: XsBase) -> Base {
        match xs {
            XsBase::Double => Base::Double,
            XsBase::Integer => Base::Integer,
            XsBase::Boolean => Base::Boolean,
            _ => Base::String,
        }
    }
}

/// `sh:datatype` (when the lift's literal type is fixed) and comparison base
/// of an IDS `dataType`, from the IDS data-type table.
fn datatype_info(ifc_type: &str, warnings: &mut Vec<String>) -> (Option<&'static str>, Base) {
    let t = ifc_type.trim().to_ascii_uppercase();
    match schema::ids_datatype(&t).and_then(|d| d.base) {
        Some(XsBase::Boolean) => (Some("xsd:boolean"), Base::Boolean),
        // Numbers come out of the lift as xsd:integer or xsd:double depending
        // on how the file wrote them, so they are compared by value, not type.
        Some(XsBase::Double) => (None, Base::Double),
        Some(XsBase::Integer) => (None, Base::Integer),
        Some(_) => (Some("xsd:string"), Base::String),
        None => {
            if schema::ids_datatype(&t).is_none() {
                warnings.push(format!(
                    "dataType `{ifc_type}` is not an IFC data type IDS knows; values are compared as text"
                ));
            }
            (None, Base::String)
        }
    }
}

fn dbl(v: f64) -> String {
    format!("\"{v:?}\"^^xsd:double")
}

/// The buildingSMART corpus accepts a value exactly on the bound, so the range
/// is closed, widened by a few ulps so the bound computed in binary floating
/// point does not reject the decimal boundary itself.
fn tolerance_range(v: f64) -> (f64, f64) {
    let lo = v - v.abs() * TOLERANCE - TOLERANCE;
    let hi = v + v.abs() * TOLERANCE + TOLERANCE;
    let slack = |b: f64| b.abs() * 4.0 * f64::EPSILON;
    (lo - slack(lo), hi + slack(hi))
}

/// A bound or enumeration member as a typed Turtle literal.
fn typed_value(raw: &str, base: Base) -> Result<String, String> {
    let s = raw.trim();
    match base {
        Base::Double => s
            .parse::<f64>()
            .ok()
            .filter(|_| XsBase::Double.lexically_valid(s))
            .map(dbl)
            .ok_or_else(|| format!("`{raw}` is not a valid xs:double")),
        Base::Integer => s
            .parse::<i64>()
            .ok()
            .filter(|_| XsBase::Integer.lexically_valid(s))
            .map(|i| i.to_string())
            .ok_or_else(|| format!("`{raw}` is not a valid xs:integer")),
        Base::Boolean => match s {
            "true" | "1" => Ok("true".into()),
            "false" | "0" => Ok("false".into()),
            _ => Err(format!(
                "`{raw}` is not a valid xs:boolean (IDS booleans are lowercase `true` / `false`)"
            )),
        },
        Base::String => Ok(ttl_str(raw)),
    }
}

/// Equality with one value, as constraint lines on the value nodes: a
/// tolerance range for doubles, a closed range for integers (so an integer
/// stored as a double still compares), the term itself otherwise.
fn equals(raw: &str, base: Base) -> Result<Vec<String>, String> {
    Ok(match base {
        Base::Double => {
            let v: f64 = raw
                .trim()
                .parse()
                .map_err(|_| format!("`{raw}` is not a valid xs:double"))?;
            if !XsBase::Double.lexically_valid(raw.trim()) {
                return Err(format!("`{raw}` is not a valid xs:double"));
            }
            let (lo, hi) = tolerance_range(v);
            vec![
                format!("sh:minInclusive {}", dbl(lo)),
                format!("sh:maxInclusive {}", dbl(hi)),
            ]
        }
        Base::Integer => {
            let n = typed_value(raw, base)?;
            vec![
                format!("sh:minInclusive {n}"),
                format!("sh:maxInclusive {n}"),
            ]
        }
        _ => vec![format!("sh:hasValue {}", typed_value(raw, base)?)],
    })
}

/// A value restriction, as SHACL property-constraint lines.
fn value_constraints(
    value: Option<&El>,
    base: Base,
    warnings: &mut Vec<String>,
) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let Some(v) = value else { return Ok(out) };
    if let Some(s) = v.child("simpleValue") {
        return equals(&s.text, base);
    }
    let Some(r) = v.child("restriction") else {
        return Ok(out);
    };
    // A restriction's own base wins over the facet's data type.
    let base = match r.attr("base").and_then(XsBase::parse) {
        Some(b) => Base::of(b),
        None => base,
    };
    let enums: Vec<&str> = r
        .children_named("enumeration")
        .filter_map(|e| e.attr("value"))
        .collect();
    if !enums.is_empty() {
        match base {
            Base::Double | Base::Integer => {
                let members = enums
                    .iter()
                    .map(|e| equals(e, base).map(|lines| format!("[ {} ]", lines.join(" ; "))))
                    .collect::<Result<Vec<_>, _>>()?;
                out.push(format!("sh:or ( {} )", members.join(" ")));
            }
            _ => {
                let members = enums
                    .iter()
                    .map(|e| typed_value(e, base))
                    .collect::<Result<Vec<_>, _>>()?;
                out.push(format!("sh:in ( {} )", members.join(" ")));
            }
        }
    }
    // Several xs:pattern facets in one restriction are alternatives (XSD).
    let patterns = r
        .children_named("pattern")
        .filter_map(|p| p.attr("value"))
        .map(xsd_regex::to_sparql)
        .collect::<Result<Vec<_>, _>>()?;
    match patterns.len() {
        0 => {}
        1 => out.push(format!("sh:pattern {}", ttl_str(&patterns[0]))),
        _ => out.push(format!(
            "sh:or ( {} )",
            patterns
                .iter()
                .map(|p| format!("[ sh:pattern {} ]", ttl_str(p)))
                .collect::<Vec<_>>()
                .join(" ")
        )),
    }
    // Ranges carry no tolerance (tolerance.md) and are typed by the base.
    for (facet, sh) in [
        ("minInclusive", "sh:minInclusive"),
        ("maxInclusive", "sh:maxInclusive"),
        ("minExclusive", "sh:minExclusive"),
        ("maxExclusive", "sh:maxExclusive"),
    ] {
        if let Some(val) = r.child(facet).and_then(|f| f.attr("value")) {
            out.push(format!("{sh} {}", typed_value(val, base)?));
        }
    }
    for (facet, sh) in [("minLength", "sh:minLength"), ("maxLength", "sh:maxLength")] {
        if let Some(val) = r.child(facet).and_then(|f| f.attr("value")) {
            out.push(format!("{sh} {}", typed_value(val, Base::Integer)?));
        }
    }
    if let Some(len) = r.child("length").and_then(|f| f.attr("value")) {
        let n = typed_value(len, Base::Integer)?;
        out.push(format!("sh:minLength {n}"));
        out.push(format!("sh:maxLength {n}"));
    }
    if out.is_empty() {
        warnings.push(
            "value restriction has no facet this importer maps (enumeration, pattern, bounds, length)"
                .to_string(),
        );
    }
    Ok(out)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Card {
    Required,
    Optional,
    Prohibited,
}

/// Cardinality of a facet: IDS 1.0 `cardinality` (default `required`), IDS
/// 0.9 `minOccurs`/`maxOccurs`.
fn cardinality(f: &El) -> Card {
    match f.attr("cardinality").map(|c| c.to_ascii_lowercase()) {
        Some(c) if c == "prohibited" => Card::Prohibited,
        Some(c) if c == "optional" => Card::Optional,
        Some(_) => Card::Required,
        None => {
            if f.attr("maxOccurs") == Some("0") {
                Card::Prohibited
            } else if f.attr("minOccurs") == Some("0") {
                Card::Optional
            } else {
                Card::Required
            }
        }
    }
}

/// What one facet becomes: a property shape body (`sh:path …; …`), or a
/// constraint on the focus node itself.
enum FacetOut {
    Property(String),
    Node(String),
}

/// One facet with its cardinality applied. In the applicability position a
/// facet is a condition with `required` semantics (the facet must hold for a
/// node to be applicable). `prohibited` is the negation of `required` — not a
/// count of zero, which would let a matching value through when the
/// requirement carried one; `optional` keeps the value constraints only, so
/// an absent value passes and a present one must satisfy them.
fn facet_constraint(
    f: &El,
    as_requirement: bool,
    ctx: &SpecCtx,
    warnings: &mut Vec<String>,
) -> Result<Option<FacetOut>, String> {
    let card = if as_requirement {
        cardinality(f)
    } else {
        Card::Required
    };
    let Some(mut lines) = facet_lines(f, ctx, warnings)? else {
        return Ok(None);
    };
    Ok(Some(match card {
        Card::Required => {
            lines.push("sh:minCount 1".into());
            FacetOut::Property(lines.join(" ;\n        "))
        }
        Card::Optional => FacetOut::Property(lines.join(" ;\n        ")),
        Card::Prohibited => {
            lines.push("sh:minCount 1".into());
            FacetOut::Node(format!(
                "sh:not [ sh:property [\n        {}\n    ] ]",
                lines.join(" ;\n        ")
            ))
        }
    }))
}

/// A facet's path and value constraints, without cardinality.
fn facet_lines(
    f: &El,
    ctx: &SpecCtx,
    warnings: &mut Vec<String>,
) -> Result<Option<Vec<String>>, String> {
    Ok(match f.name.as_str() {
        "property" => {
            let pset = f.simple("propertySet").unwrap_or_default();
            let name = f
                .simple("baseName")
                .or_else(|| f.simple("name"))
                .unwrap_or_default();
            if pset.is_empty() || name.is_empty() {
                warnings.push("property facet needs a simpleValue propertySet and baseName; enumerated names are not expanded".into());
                return Ok(None);
            }
            let path = format!("props:{}_{}", sanitize(&pset), sanitize(&name));
            let (dt, base) = match f.attr("dataType").or_else(|| f.attr("datatype")) {
                Some(d) => datatype_info(d, warnings),
                None => (None, Base::String),
            };
            let mut lines = vec![
                format!("sh:path {path}"),
                format!("sh:name {}", ttl_str(&format!("{pset}.{name}"))),
            ];
            if let Some(dt) = dt {
                lines.push(format!("sh:datatype {dt}"));
            }
            lines.extend(value_constraints(f.child("value"), base, warnings)?);
            Some(lines)
        }
        "attribute" => {
            let name = f.simple("name").unwrap_or_default();
            if name.is_empty() {
                warnings.push("attribute facet without a simpleValue name skipped".into());
                return Ok(None);
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
            lines.extend(value_constraints(f.child("value"), Base::String, warnings)?);
            Some(lines)
        }
        "classification" | "material" => {
            let (path, label) = if f.name == "classification" {
                ("props:ifcClassification", "classification")
            } else {
                ("props:ifcMaterial", "material")
            };
            warnings.push(format!(
                "{label} facet maps to {path}, which the IFC lift emits from IfcRelAssociates{} (the reference's Identification / the material's Name); a model lifted before that carries no such value and the requirement fails on it",
                if f.name == "classification" { "Classification" } else { "Material" }
            ));
            let mut lines = vec![format!("sh:path {path}")];
            if let Some(sys) = f.simple("system") {
                lines.push(format!(
                    "sh:description {}",
                    ttl_str(&format!("system: {sys}"))
                ));
            }
            lines.extend(value_constraints(f.child("value"), Base::String, warnings)?);
            Some(lines)
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
                    format!("ots:partOf_{}", sanitize(other))
                }
            };
            let mut lines = vec![format!("sh:path {path}")];
            if let Some(e) = f.child("entity") {
                let classes = ctx.entity_class_iris(e, warnings)?;
                if !classes.is_empty() {
                    lines.push(class_constraint(&classes));
                }
            }
            Some(lines)
        }
        // In the applicability position an entity facet IS the target, and is
        // consumed by `entity_classes`. In the requirements position it is a
        // requirement on the focus node itself, not on a path, so it cannot be
        // a property shape: `requirement_node_constraint` handles it and this
        // arm must not silently swallow it.
        "entity" => None,
        other => {
            warnings.push(format!("facet `{other}` is not supported"));
            None
        }
    })
}

/// `sh:class` for one class, `sh:or` of them for several.
fn class_constraint(classes: &[String]) -> String {
    if classes.len() == 1 {
        format!("sh:class {}", classes[0])
    } else {
        format!(
            "sh:or ( {} )",
            classes
                .iter()
                .map(|c| format!("[ sh:class {c} ]"))
                .collect::<Vec<_>>()
                .join(" ")
        )
    }
}

/// A requirement facet that constrains the focus NODE rather than a path.
///
/// Only `<ids:entity>` is one: inside `<ids:requirements>` it says the
/// applicable node must be of that IFC class (IDS gives the requirement entity
/// no cardinality: it is always required).
fn requirement_node_constraint(
    f: &El,
    ctx: &SpecCtx,
    warnings: &mut Vec<String>,
) -> Result<Option<String>, String> {
    if f.name != "entity" {
        return Ok(None);
    }
    let classes = ctx.entity_class_iris(f, warnings)?;
    if classes.is_empty() {
        return Ok(None);
    }
    Ok(Some(class_constraint(&classes)))
}

/// The schema context of one specification: its `ifcVersion` list.
struct SpecCtx {
    versions: Vec<SchemaId>,
}

impl SpecCtx {
    /// `ifc:` for IFC4 and IFC4X3 (the lift types 4.3 files in the IFC4
    /// namespace), `ifc2x3:` for IFC2X3.
    fn prefixes(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        for v in &self.versions {
            let p = if *v == SchemaId::Ifc2x3 {
                "ifc2x3"
            } else {
                "ifc"
            };
            if !out.contains(&p) {
                out.push(p);
            }
        }
        out
    }

    /// The entity names an `<ids:entity>` facet's `name` admits: a simple
    /// value, an enumeration, or a pattern expanded against the schemas.
    fn entity_names(&self, e: &El, warnings: &mut Vec<String>) -> Result<Vec<String>, String> {
        let mut out: Vec<String> = Vec::new();
        if let Some(n) = e.simple("name") {
            out.push(n);
        } else if let Some(r) = e.child("name").and_then(|n| n.child("restriction")) {
            for v in r
                .children_named("enumeration")
                .filter_map(|x| x.attr("value"))
            {
                out.push(v.to_string());
            }
            let patterns = r
                .children_named("pattern")
                .filter_map(|p| p.attr("value"))
                .map(xsd_regex::to_sparql)
                .collect::<Result<Vec<_>, _>>()?;
            if !patterns.is_empty() {
                let res: Vec<regex::Regex> = patterns
                    .iter()
                    .filter_map(|p| regex::Regex::new(p).ok())
                    .collect();
                let mut names: Vec<String> = self
                    .versions
                    .iter()
                    .flat_map(|v| {
                        v.schema()
                            .entity_names()
                            .map(str::to_string)
                            .collect::<Vec<_>>()
                    })
                    .filter(|n| res.iter().any(|r| r.is_match(n)))
                    .collect();
                names.sort();
                names.dedup();
                if names.is_empty() {
                    warnings.push(format!(
                        "entity pattern {patterns:?} matches no entity of the specification's schemas"
                    ));
                }
                out.extend(names);
            }
        }
        if e.simple("predefinedType").is_some() || e.child("predefinedType").is_some() {
            warnings.push(
                "predefinedType is not checked: the building-topology lift does not record predefined types"
                    .to_string(),
            );
        }
        Ok(out)
    }

    /// Class IRIs (prefixed) for an entity facet, in every namespace the
    /// specification's schemas use.
    fn entity_class_iris(&self, e: &El, warnings: &mut Vec<String>) -> Result<Vec<String>, String> {
        let mut out = Vec::new();
        for name in self.entity_names(e, warnings)? {
            // The schema tables' CamelCase, else the title-casing fallback.
            let upper = name.trim().to_ascii_uppercase();
            let class = match self.versions.iter().find_map(|v| v.schema().camel(&upper)) {
                Some(c) => c.to_string(),
                None => ifc_class(&name, warnings),
            };
            for p in self.prefixes() {
                let iri = format!("{p}:{class}");
                if !out.contains(&iri) {
                    out.push(iri);
                }
            }
            if self.versions.contains(&SchemaId::Ifc4x3)
                && crate::ifc::names::IFC4X3_ONLY.contains(&name.to_ascii_uppercase().as_str())
            {
                warnings.push(format!(
                    "entity `{name}` exists only in IFC 4.3, which the lift types in its own namespace; this target does not reach it"
                ));
            }
        }
        Ok(out)
    }
}

/// The importer's own sample document, so the exporter's tests can
/// round-trip against exactly what this importer produces.
#[cfg(test)]
pub(crate) fn tests_sample() -> &'static str {
    tests::SAMPLE
}

/// The IFC versions a specification names (`ifcVersion` is a list).
fn spec_versions(spec: &El) -> Result<Vec<SchemaId>, String> {
    let raw = spec.attr("ifcVersion").unwrap_or("IFC4");
    let mut out = Vec::new();
    for tok in raw.split_whitespace() {
        let v = SchemaId::from_ids(tok)
            .ok_or_else(|| format!("ifcVersion `{tok}` is not IFC2X3, IFC4 or IFC4X3_ADD2"))?;
        if !out.contains(&v) {
            out.push(v);
        }
    }
    if out.is_empty() {
        return Err("ifcVersion is empty".into());
    }
    Ok(out)
}

/// The full IRI behind a `prefix:Class` written by this importer.
fn expand(prefixed: &str) -> String {
    match prefixed.split_once(':') {
        Some(("ifc2x3", c)) => format!("{IFC2X3_OWL}{c}"),
        Some((_, c)) => format!("{IFC4_OWL}{c}"),
        None => prefixed.to_string(),
    }
}

/// One specification as shapes; `Ok(None)` when it is skipped with a warning.
fn convert_spec(
    spec: &El,
    n: usize,
    name: &str,
    ttl: &mut String,
    warnings: &mut Vec<String>,
) -> Result<Option<(SpecSummary, usize)>, String> {
    let shape = format!("{SHAPE_NS}spec{n}");
    let ctx = SpecCtx {
        versions: spec_versions(spec)?,
    };
    let Some(applicability) = spec.child("applicability") else {
        warnings.push(format!(
            "specification `{name}` has no applicability; skipped"
        ));
        return Ok(None);
    };
    // The applicable classes, in every namespace the ifcVersion list uses. A
    // specification without an entity facet applies to every typed node.
    let classes: Vec<String> = match applicability.child("entity") {
        Some(e) => {
            let c = ctx.entity_class_iris(e, warnings)?;
            if c.is_empty() {
                warnings.push(format!(
                    "specification `{name}`: the entity facet admits no entity; skipped"
                ));
                return Ok(None);
            }
            c
        }
        None => Vec::new(),
    };
    let applies: Vec<FacetOut> = applicability
        .children
        .iter()
        .filter(|f| f.name != "entity")
        .map(|f| facet_constraint(f, false, &ctx, warnings))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .flatten()
        .collect();
    let requirements: Vec<FacetOut> = match spec.child("requirements") {
        Some(r) => r
            .children
            .iter()
            .map(|f| facet_constraint(f, true, &ctx, warnings))
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .flatten()
            .collect(),
        None => Vec::new(),
    };
    let req_node: Vec<String> = match spec.child("requirements") {
        Some(r) => r
            .children
            .iter()
            .map(|f| requirement_node_constraint(f, &ctx, warnings))
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .flatten()
            .collect(),
        None => Vec::new(),
    };
    // IDS 1.0 puts `xs:occurs` (minOccurs/maxOccurs) on `applicabilityType`,
    // i.e. on <ids:applicability>; IDS 0.9 carried them on the
    // <ids:specification>. Read the 1.0 position first and fall back. The XSD
    // default is minOccurs="1": a specification is required unless it says
    // otherwise.
    let occurs = |a: &str| -> Option<&str> { applicability.attr(a).or_else(|| spec.attr(a)) };
    let prohibited_spec = occurs("maxOccurs") == Some("0");
    let required_spec = !prohibited_spec && occurs("minOccurs") != Some("0");
    if prohibited_spec && !(requirements.is_empty() && req_node.is_empty()) {
        return Err(
            "a prohibited specification (maxOccurs=\"0\") cannot carry requirements".into(),
        );
    }

    let version_list = ctx
        .versions
        .iter()
        .map(|v| v.ids_name())
        .collect::<Vec<_>>()
        .join(" ");
    writeln!(ttl, "<{shape}> a sh:NodeShape ;").unwrap();
    writeln!(ttl, "    sh:name {} ;", ttl_str(name)).unwrap();
    if let Some(d) = spec.attr("description") {
        writeln!(ttl, "    sh:description {} ;", ttl_str(d)).unwrap();
    }
    writeln!(
        ttl,
        "    rdfs:comment {} ;",
        ttl_str(&format!("IDS specification {n} ({version_list})"))
    )
    .unwrap();
    if classes.is_empty() {
        writeln!(ttl, "    sh:targetSubjectsOf rdf:type ;").unwrap();
    }
    for c in &classes {
        writeln!(ttl, "    sh:targetClass {c} ;").unwrap();
    }
    let render = |outs: &[FacetOut]| -> Vec<String> {
        outs.iter()
            .map(|o| match o {
                FacetOut::Property(l) => format!("    sh:property [\n        {l}\n    ]"),
                FacetOut::Node(c) => format!("    {c}"),
            })
            .collect()
    };
    let mut shapes = 1;
    let requirement_blocks = || -> Vec<String> {
        let mut blocks: Vec<String> = req_node.iter().map(|c| format!("    {c}")).collect();
        blocks.extend(render(&requirements));
        blocks
    };
    if prohibited_spec {
        if applies.is_empty() {
            // Every applicable node violates: `sh:not` of the empty shape.
            writeln!(ttl, "    sh:not [ a sh:NodeShape ] .\n").unwrap();
        } else {
            writeln!(ttl, "    sh:not <{shape}-applies> .\n").unwrap();
            writeln!(ttl, "<{shape}-applies> a sh:NodeShape ;").unwrap();
            writeln!(
                ttl,
                "    sh:name {} ;",
                ttl_str(&format!("{name} — applicability"))
            )
            .unwrap();
            writeln!(ttl, "{} .\n", render(&applies).join(" ;\n")).unwrap();
            shapes += 1;
        }
    } else if applies.is_empty() {
        let blocks = requirement_blocks();
        if blocks.is_empty() {
            writeln!(ttl, "    sh:deactivated false .\n").unwrap();
        } else {
            writeln!(ttl, "{} .\n", blocks.join(" ;\n")).unwrap();
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
        writeln!(ttl, "{} .\n", render(&applies).join(" ;\n")).unwrap();
        writeln!(ttl, "<{shape}-requires> a sh:NodeShape ;").unwrap();
        writeln!(
            ttl,
            "    sh:name {} ;",
            ttl_str(&format!("{name} — requirements"))
        )
        .unwrap();
        let blocks = requirement_blocks();
        if blocks.is_empty() {
            writeln!(ttl, "    sh:deactivated false .\n").unwrap();
        } else {
            writeln!(ttl, "{} .\n", blocks.join(" ;\n")).unwrap();
        }
        shapes += 2;
    }
    if required_spec {
        // "At least one applicable entity must exist" is a statement about the
        // whole model, not about any one node, so it is a SPARQL constraint on
        // a fixed focus node. Only the entity part of the applicability is
        // checked here: the other facets are SHACL Core shapes a SPARQL query
        // cannot call.
        if !applies.is_empty() {
            warnings.push(format!(
                "specification `{name}` is required: the existence check counts applicable entities by class only, not by its other applicability facets"
            ));
        }
        let exists = if classes.is_empty() {
            "?x a ?class .".to_string()
        } else {
            format!(
                "?x a ?class . FILTER(?class IN ({}))",
                classes
                    .iter()
                    .map(|c| format!("<{}>", expand(c)))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        writeln!(ttl, "<{shape}-exists> a sh:NodeShape ;").unwrap();
        writeln!(
            ttl,
            "    sh:name {} ;",
            ttl_str(&format!("{name} — at least one applicable entity"))
        )
        .unwrap();
        writeln!(ttl, "    sh:targetNode <{shape}-exists> ;").unwrap();
        writeln!(
            ttl,
            "    sh:sparql [ a sh:SPARQLConstraint ;\n        sh:message {} ;\n        sh:select {} ] .\n",
            ttl_str(&format!("the required specification `{name}` has no applicable entity in the model")),
            ttl_long(&format!("SELECT $this WHERE {{ FILTER NOT EXISTS {{ {exists} }} }}"))
        )
        .unwrap();
        shapes += 1;
    }
    Ok(Some((
        SpecSummary {
            name: name.to_string(),
            shape,
            target_classes: classes,
            requirements: requirements.len() + req_node.len(),
        },
        shapes,
    )))
}

fn ttl_long(s: &str) -> String {
    format!(
        "\"\"\"{}\"\"\"",
        s.replace('\\', "\\\\").replace("\"\"\"", "\\\"\\\"\\\"")
    )
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
    let mut errors: Vec<String> = Vec::new();
    for (i, spec) in specs.iter().enumerate() {
        let n = i + 1;
        let name = spec
            .attr("name")
            .map(str::to_string)
            .unwrap_or_else(|| format!("Specification {n}"));
        match convert_spec(spec, n, &name, &mut ttl, &mut warnings) {
            Ok(Some((summary, shapes))) => {
                shape_count += shapes;
                summaries.push(summary);
            }
            Ok(None) => {}
            Err(e) => errors.push(format!("specification `{name}`: {e}")),
        }
    }
    if !errors.is_empty() {
        anyhow::bail!("the IDS cannot be converted: {}", errors.join("; "));
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

    /// IDS 1.0 puts the occurrence attributes on `<ids:applicability>`. Reading
    /// them from `<ids:specification>` made the prohibited-specification branch
    /// dead code for every conformant 1.0 document.
    #[test]
    fn a_prohibited_specification_is_recognised_in_the_ids_1_0_position() {
        let doc = r#"<?xml version="1.0" encoding="UTF-8"?>
<ids:ids xmlns:ids="http://standards.buildingsmart.org/IDS" xmlns:xs="http://www.w3.org/2001/XMLSchema">
  <ids:info><ids:title>No plastic pipes</ids:title></ids:info>
  <ids:specifications>
    <ids:specification name="No plastic pipes" ifcVersion="IFC4">
      <ids:applicability minOccurs="0" maxOccurs="0">
        <ids:entity><ids:name><ids:simpleValue>IFCPIPESEGMENT</ids:simpleValue></ids:name></ids:entity>
      </ids:applicability>
    </ids:specification>
  </ids:specifications>
</ids:ids>"#;
        let out = convert(&parse_xml(doc.as_bytes()).expect("parses")).expect("converts");
        assert!(
            out.turtle.contains("sh:not [ a sh:NodeShape ]"),
            "a prohibited specification must become sh:not: {}",
            out.turtle
        );
        assert!(
            !out.turtle.contains("-exists>"),
            "a prohibited specification has no existence requirement: {}",
            out.turtle
        );
    }

    /// An `<ids:entity>` inside `<ids:requirements>` constrains the focus node.
    /// It used to be dropped with no constraint and no warning.
    #[test]
    fn an_entity_requirement_is_enforced_not_dropped() {
        let doc = r#"<?xml version="1.0" encoding="UTF-8"?>
<ids:ids xmlns:ids="http://standards.buildingsmart.org/IDS" xmlns:xs="http://www.w3.org/2001/XMLSchema">
  <ids:info><ids:title>Doors are doors</ids:title></ids:info>
  <ids:specifications>
    <ids:specification name="Doors are doors" ifcVersion="IFC4">
      <ids:applicability>
        <ids:entity><ids:name><ids:simpleValue>IFCDOOR</ids:simpleValue></ids:name></ids:entity>
      </ids:applicability>
      <ids:requirements>
        <ids:entity cardinality="required"><ids:name><ids:simpleValue>IFCDOOR</ids:simpleValue></ids:name></ids:entity>
      </ids:requirements>
    </ids:specification>
  </ids:specifications>
</ids:ids>"#;
        let out = convert(&parse_xml(doc.as_bytes()).expect("parses")).expect("converts");
        assert!(
            out.turtle.contains("sh:class"),
            "the entity requirement must produce a class constraint: {}",
            out.turtle
        );
    }

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
        // The applicability's IsExternal has no dataType attribute → compared as a
        // string; a required specification gets its existence shape.
        assert!(t.contains("<urn:ids:spec1-exists> a sh:NodeShape"), "{t}");
        assert!(t.contains("FILTER NOT EXISTS { ?x a ?class . FILTER(?class IN (<https://standards.buildingsmart.org/IFC/DEV/IFC4/ADD2_TC1/OWL#IfcWall>))"), "{t}");
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
    fn spec(body: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<ids:ids xmlns:ids="http://standards.buildingsmart.org/IDS" xmlns:xs="http://www.w3.org/2001/XMLSchema">
  <ids:info><ids:title>t</ids:title></ids:info>
  <ids:specifications>{body}</ids:specifications>
</ids:ids>"#
        )
    }

    fn turtle_of(body: &str) -> String {
        let out = IdsImporter.import(spec(body).as_bytes()).expect("converts");
        let tmp = crate::store::TripleStore::in_memory().unwrap();
        tmp.load_str(&out.turtle, oxigraph::io::RdfFormat::Turtle, Some("urn:x"))
            .unwrap_or_else(|e| panic!("valid Turtle ({e}): {}", out.turtle));
        out.turtle
    }

    const WALL: &str =
        "<ids:entity><ids:name><ids:simpleValue>IFCWALL</ids:simpleValue></ids:name></ids:entity>";

    #[test]
    fn a_double_value_becomes_the_ids_tolerance_range() {
        let t = turtle_of(&format!(
            r#"<ids:specification name="s" ifcVersion="IFC4"><ids:applicability>{WALL}</ids:applicability>
            <ids:requirements><ids:property dataType="IFCREAL"><ids:propertySet><ids:simpleValue>P</ids:simpleValue></ids:propertySet><ids:baseName><ids:simpleValue>V</ids:simpleValue></ids:baseName><ids:value><ids:simpleValue>1.</ids:simpleValue></ids:value></ids:property></ids:requirements></ids:specification>"#
        ));
        // 1 − 1e-6 − 1e-6 and 1 + 1e-6 + 1e-6, exclusive.
        assert!(t.contains("sh:minInclusive \"0.99999799999999"), "{t}");
        assert!(t.contains("sh:maxInclusive \"1.00000200000000"), "{t}");
        assert!(!t.contains("sh:hasValue"), "{t}");
    }

    #[test]
    fn patterns_are_anchored_and_alternatives_or_together() {
        let t = turtle_of(&format!(
            r#"<ids:specification name="s" ifcVersion="IFC4"><ids:applicability>{WALL}</ids:applicability>
            <ids:requirements><ids:attribute><ids:name><ids:simpleValue>Name</ids:simpleValue></ids:name><ids:value><xs:restriction base="xs:string"><xs:pattern value="[A-Z]{{2}}"/><xs:pattern value="[a-z]{{2}}"/></xs:restriction></ids:value></ids:attribute></ids:requirements></ids:specification>"#
        ));
        assert!(
            t.contains(
                r#"sh:or ( [ sh:pattern "^(?:[A-Z]{2})$" ] [ sh:pattern "^(?:[a-z]{2})$" ] )"#
            ),
            "{t}"
        );
    }

    #[test]
    fn a_prohibited_facet_is_the_negation_of_the_required_one() {
        let t = turtle_of(&format!(
            r#"<ids:specification name="s" ifcVersion="IFC4"><ids:applicability>{WALL}</ids:applicability>
            <ids:requirements><ids:attribute cardinality="prohibited"><ids:name><ids:simpleValue>Name</ids:simpleValue></ids:name><ids:value><ids:simpleValue>X</ids:simpleValue></ids:value></ids:attribute></ids:requirements></ids:specification>"#
        ));
        assert!(t.contains("sh:not [ sh:property ["), "{t}");
        assert!(
            t.contains("sh:hasValue \"X\" ;\n        sh:minCount 1"),
            "{t}"
        );
        assert!(!t.contains("sh:maxCount 0"), "{t}");
    }

    #[test]
    fn every_listed_ifc_version_is_targeted_and_an_entityless_spec_targets_all() {
        let t = turtle_of(&format!(
            r#"<ids:specification name="a" ifcVersion="IFC2X3 IFC4"><ids:applicability>{WALL}</ids:applicability></ids:specification>
            <ids:specification name="b" ifcVersion="IFC4"><ids:applicability minOccurs="0" maxOccurs="unbounded"><ids:attribute><ids:name><ids:simpleValue>Name</ids:simpleValue></ids:name></ids:attribute></ids:applicability>
            <ids:requirements><ids:attribute><ids:name><ids:simpleValue>GlobalId</ids:simpleValue></ids:name></ids:attribute></ids:requirements></ids:specification>"#
        ));
        assert!(t.contains("sh:targetClass ifc2x3:IfcWall"), "{t}");
        assert!(t.contains("sh:targetClass ifc:IfcWall"), "{t}");
        assert!(t.contains("sh:targetSubjectsOf rdf:type"), "{t}");
        // b is optional: no existence shape.
        assert!(t.contains("<urn:ids:spec1-exists>"), "{t}");
        assert!(!t.contains("<urn:ids:spec2-exists>"), "{t}");
        // An applicability facet is a condition the node must meet.
        assert!(
            t.contains(
                "sh:path props:ifcName ;\n        sh:name \"Name\" ;\n        sh:minCount 1"
            ),
            "{t}"
        );
    }

    #[test]
    fn a_prohibited_specification_with_requirements_and_a_bad_version_are_refused() {
        let err = IdsImporter
            .import(spec(&format!(
                r#"<ids:specification name="s" ifcVersion="IFC4"><ids:applicability minOccurs="0" maxOccurs="0">{WALL}</ids:applicability>
                <ids:requirements><ids:attribute><ids:name><ids:simpleValue>Name</ids:simpleValue></ids:name></ids:attribute></ids:requirements></ids:specification>"#
            )).as_bytes())
            .unwrap_err()
            .to_string();
        assert!(err.contains("cannot carry requirements"), "{err}");
        let err = IdsImporter
            .import(spec(&format!(
                r#"<ids:specification name="s" ifcVersion="IFC5"><ids:applicability>{WALL}</ids:applicability></ids:specification>"#
            )).as_bytes())
            .unwrap_err()
            .to_string();
        assert!(err.contains("ifcVersion `IFC5`"), "{err}");
        let err = IdsImporter
            .import(spec(&format!(
                r#"<ids:specification name="s" ifcVersion="IFC4"><ids:applicability>{WALL}</ids:applicability>
                <ids:requirements><ids:property dataType="IFCINTEGER"><ids:propertySet><ids:simpleValue>P</ids:simpleValue></ids:propertySet><ids:baseName><ids:simpleValue>V</ids:simpleValue></ids:baseName><ids:value><ids:simpleValue>42.0</ids:simpleValue></ids:value></ids:property></ids:requirements></ids:specification>"#
            )).as_bytes())
            .unwrap_err()
            .to_string();
        assert!(err.contains("not a valid xs:integer"), "{err}");
    }
}
