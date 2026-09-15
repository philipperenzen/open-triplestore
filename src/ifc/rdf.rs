//! RDF emission for parsed IFC files — see the module docs in [`super`].

use std::collections::{HashMap, HashSet};

use super::names;
use super::step::{decode_ifc_guid, Arg, Instance, StepFile};
use super::units::{self, Unit};
use super::{ConvertOptions, IfcStats};

const BOT: &str = "https://w3id.org/bot#";
const PROPS: &str = "https://w3id.org/props#";
const OMG: &str = "https://w3id.org/omg#";
const FOG: &str = "https://w3id.org/fog#";
const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const RDFS_LABEL: &str = "http://www.w3.org/2000/01/rdf-schema#label";
const XSD: &str = "http://www.w3.org/2001/XMLSchema#";
const OTS: &str = "https://opentriplestore.org/ns#";
const GEO: &str = "http://www.opengis.net/ont/geosparql#";
const NEN: &str = "https://w3id.org/nen2660/def#";
const QUDT: &str = "http://qudt.org/schema/qudt/";
const SKOS: &str = "http://www.w3.org/2004/02/skos/core#";
const DCT: &str = "http://purl.org/dc/terms/";

/// Where the lift's own terms live when no base URL can be derived at all.
pub const LIFT_NS_FALLBACK: &str = "https://opentriplestore.org/ns/ifc-lift#";

// Chunk size for the N-Triples sinks. Each chunk costs a store load round-trip
// (which rebuilds the graph index), so bigger chunks load multi-million-triple
// lifts far faster; 32 MB keeps peak memory modest while cutting round-trips ~8×.
const FLUSH_AT: usize = 32 * 1024 * 1024;

/// ifcOWL namespace for a FILE_SCHEMA id. IFC 4.3 files answer with the IFC4
/// namespace too: every entity IFC4 already had keeps its IFC4 IRI, and only
/// the entities new in 4.3 (`names::IFC4X3_ONLY`) go to the lift's own
/// namespace — see [`entity_class`].
fn ifcowl_ns(schema: &str) -> &'static str {
    if schema.starts_with("IFC4") {
        "https://standards.buildingsmart.org/IFC/DEV/IFC4/ADD2_TC1/OWL#"
    } else {
        "https://standards.buildingsmart.org/IFC/DEV/IFC2x3/TC1/OWL#"
    }
}

/// The lift's own namespace, `{base_url}/ns/ifc-lift#`, derived from
/// `inst_base`: the import writes into `{base_url}/dataset/{id}/building/`,
/// so the base URL is what precedes `/dataset/`; a custom graph IRI falls
/// back to its origin; anything else to [`LIFT_NS_FALLBACK`]. The same
/// namespace is what the `ifc-lift` seed bundle expands `{base_url}` to.
pub fn lift_namespace(opts: &ConvertOptions) -> String {
    let base = &opts.inst_base;
    if let Some(i) = base.find("/dataset/") {
        return format!("{}/ns/ifc-lift#", &base[..i]);
    }
    if let Some(scheme_end) = base.find("://") {
        let rest = &base[scheme_end + 3..];
        let host_end = rest.find('/').unwrap_or(rest.len());
        if host_end > 0 {
            return format!(
                "{}{}/ns/ifc-lift#",
                &base[..scheme_end + 3],
                &rest[..host_end]
            );
        }
    }
    LIFT_NS_FALLBACK.to_string()
}

/// The class IRI an instance is typed with: IFC 4.3-only entities under the
/// lift namespace, everything else in the schema's ifcOWL namespace.
fn entity_class(ifc_ns: &str, lift_ns: &str, schema: &str, entity: &str) -> String {
    let ns = if schema.starts_with("IFC4X3") && names::IFC4X3_ONLY.contains(&entity) {
        lift_ns
    } else {
        ifc_ns
    };
    format!("{ns}{}", names::camel(entity))
}

/// The unit of a value: its own `Unit` reference when it has one, else the
/// project default for the given unit type.
type UnitResolver<'a> = dyn Fn(Option<&Arg>, Option<&str>) -> Option<Unit> + 'a;
/// Writes a resolved unit onto a value node (QUDT IRI, or label + factor).
type UnitEmitter<'a> = dyn Fn(&mut NtSink<'_>, &str, &Unit, &mut IfcStats) + 'a;

/// A buffered N-Triples writer that flushes through a chunk callback.
struct NtSink<'a> {
    buf: String,
    count: usize,
    out: &'a mut dyn FnMut(&str),
}

impl<'a> NtSink<'a> {
    fn new(out: &'a mut dyn FnMut(&str)) -> Self {
        Self {
            buf: String::with_capacity(FLUSH_AT + 4096),
            count: 0,
            out,
        }
    }
    fn triple(&mut self, s: &str, p: &str, o: &str) {
        self.buf.push_str(s);
        self.buf.push(' ');
        self.buf.push_str(p);
        self.buf.push(' ');
        self.buf.push_str(o);
        self.buf.push_str(" .\n");
        self.count += 1;
        if self.buf.len() >= FLUSH_AT {
            (self.out)(&self.buf);
            self.buf.clear();
        }
    }
    fn finish(self) -> usize {
        if !self.buf.is_empty() {
            (self.out)(&self.buf);
        }
        self.count
    }
}

fn iri(v: &str) -> String {
    format!("<{v}>")
}

fn lit(v: &str) -> String {
    let mut out = String::with_capacity(v.len() + 2);
    out.push('"');
    for c in v.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

fn typed_lit(v: &str, dt: &str) -> String {
    format!("{}^^<{XSD}{dt}>", lit(v))
}

fn double_lit(v: f64) -> String {
    typed_lit(&format!("{v}"), "double")
}

/// Keep IRI-safe local names for pset/property names.
fn sanitize(name: &str) -> String {
    let mut out: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if out.is_empty() {
        out.push('p');
    }
    out
}

fn lower_first(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_lowercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// The 22-char GlobalId of a rooted instance (first attribute), when valid.
fn guid_of(inst: &Instance) -> Option<&str> {
    match inst.args.first() {
        Some(Arg::Str(s))
            if s.len() == 22
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'$') =>
        {
            Some(s)
        }
        _ => None,
    }
}

/// Instance IRI: GlobalId-based for rooted entities (stable across exports of
/// the same model), step-id-based otherwise.
fn inst_iri(base: &str, inst: &Instance) -> String {
    match guid_of(inst) {
        Some(g) => format!("<{base}{g}>"),
        None => format!("<{base}i{}>", inst.id),
    }
}

/// A stable IRI *under* an instance IRI: `<…/{guid}/{suffix}>`.
fn sub_iri(inst_s: &str, suffix: &str) -> String {
    format!("{}/{suffix}>", inst_s.trim_end_matches('>'))
}

fn name_arg(inst: &Instance, idx: usize) -> Option<&str> {
    inst.args
        .get(idx)
        .and_then(|a| a.as_str())
        .filter(|s| !s.trim().is_empty())
}

/// IFC compound plane angle → decimal degrees.
///
/// Handles the STEP list form `(deg, min, sec[, millionth-sec])` and, for
/// robustness against exporters/lifts that stringify it, a plain-text form like
/// `"(49 1 59 680200)"` (whitespace- or comma-separated, optional parens).
/// Per `IfcCompoundPlaneAngleMeasure` every component carries the sign of the
/// whole angle — so a Chicago longitude arrives as `(-87, -38, -21, -839999)`
/// and the components must be combined by magnitude under one overall sign
/// (summing signed components would cancel the minutes against the degrees).
fn dms_to_deg(arg: &Arg) -> Option<f64> {
    let parts: Vec<f64> = match arg.as_list() {
        Some(items) => items.iter().filter_map(Arg::as_f64).collect(),
        None => arg
            .as_str()?
            .trim()
            .trim_start_matches('(')
            .trim_end_matches(')')
            .split(|c: char| c.is_whitespace() || c == ',')
            .filter(|s| !s.is_empty())
            .map(str::parse::<f64>)
            .collect::<Result<_, _>>()
            .ok()?,
    };
    let mut vals = parts.into_iter();
    let deg = vals.next()?;
    let min = vals.next().unwrap_or(0.0);
    let sec = vals.next().unwrap_or(0.0);
    let micro = vals.next().unwrap_or(0.0);
    let sign = if deg < 0.0 || min < 0.0 || sec < 0.0 || micro < 0.0 {
        -1.0
    } else {
        1.0
    };
    Some(sign * (deg.abs() + min.abs() / 60.0 + sec.abs() / 3600.0 + micro.abs() / 3_600_000_000.0))
}

// NOT extracted: `IfcGeometricRepresentationContext.TrueNorth`.
//
// It looks like the right source for a survey rotation — the FZK-Haus declares
// `IFCDIRECTION((0.766, 0.643))`, a clean 40° — and emitting it as
// `ots:modelHeading` so the map could turn the building is an obvious idea. It
// is wrong in practice: web-ifc resolves each element's IfcObjectPlacement
// chain when it tessellates, so the geometry the viewer receives is ALREADY in
// its final orientation. Applying TrueNorth on top double-rotates it.
//
// Measured, rather than argued: the KIT Campus North site grid that the
// FZK-Haus stands on runs 15°/105° (principal axes of the surrounding ways in
// OpenStreetMap). Un-rotated the model sits at 90° — 15° off the grid, which is
// the model's own design. With the TrueNorth rotation applied it sits at 40°,
// i.e. 65° off, visibly diagonal across its own plot.
//
// If a genuine survey rotation is ever needed, verify it against a real
// footprint before shipping it — not against the IFC schema alone. The
// sanctioned path is an AUTHORED heading (`ConvertOptions::model_heading`,
// stamped below as `ots:modelHeading`): a human states the bearing after
// checking the rendered model against the real site, exactly like the demo
// seeds' authored anchors. The same holds for `IfcMapConversion`'s rotation
// (IFC 4.3 says TrueNorth shall not be added on top of a map conversion, and
// the geometry the viewer receives is already placed): it is emitted as an
// annotation on the map-conversion node, never applied to a geometry.

/// WGS84 anchor from the file's own IfcSite georeference (RefLatitude /
/// RefLongitude, attributes 9/10), when present and plausible. A site at
/// exactly (0, 0) is an exporter default (Null Island), not a georeference.
pub fn site_anchor_wkt(file: &StepFile) -> Option<String> {
    let site = file.of_entity("IFCSITE").next()?;
    let lat = site.args.get(9).and_then(dms_to_deg)?;
    let lon = site.args.get(10).and_then(dms_to_deg)?;
    if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) {
        return None;
    }
    if lat.abs() < 1e-9 && lon.abs() < 1e-9 {
        return None;
    }
    Some(format!("POINT({lon} {lat})"))
}

/// The model's `IfcMapConversion` (IFC4 / IFC 4.3 georeferencing): the map
/// coordinates of the model origin, the rotation and scale, and the target
/// `IfcProjectedCRS` — read, never applied.
#[derive(Debug, Clone, PartialEq)]
pub struct MapConversion {
    pub eastings: f64,
    pub northings: f64,
    pub orthogonal_height: Option<f64>,
    /// Degrees, anticlockwise from the map's easting axis to the model's X
    /// axis: `atan2(XAxisOrdinate, XAxisAbscissa)`.
    pub rotation_deg: Option<f64>,
    pub scale: Option<f64>,
    /// `IfcProjectedCRS.Name`, conventionally `EPSG:<code>`.
    pub crs_name: Option<String>,
    /// The EPSG code parsed out of the name.
    pub epsg: Option<u32>,
    pub vertical_datum: Option<String>,
    pub map_projection: Option<String>,
    pub map_zone: Option<String>,
}

impl MapConversion {
    /// The OGC CRS URI of the EPSG code — only when this store's CRS registry
    /// knows it, so an unrecognised code gets no prefix rather than a wrong
    /// one.
    pub fn crs_uri(&self) -> Option<&'static str> {
        let code = self.epsg?;
        let uri = format!("http://www.opengis.net/def/crs/EPSG/0/{code}");
        crate::geo::crs::Crs::from_uri(&uri).map(|c| c.to_uri())
    }

    /// `<crs> POINT(E N)` — a CRS-qualified WKT literal value, when the CRS
    /// is recognised.
    pub fn wkt(&self) -> Option<String> {
        self.crs_uri()
            .map(|uri| format!("<{uri}> POINT({} {})", self.eastings, self.northings))
    }

    /// The origin in WGS84 `(lon, lat)`, when the CRS is recognised.
    pub fn wgs84(&self) -> Option<(f64, f64)> {
        let uri = self.crs_uri()?;
        let from = crate::geo::crs::Crs::from_uri(uri)?;
        crate::geo::crs::transform_xy(
            from,
            crate::geo::crs::Crs::Wgs84,
            self.eastings,
            self.northings,
        )
    }
}

/// Read the first `IfcMapConversion` (or `IfcMapConversionScaled`) of a file.
pub fn map_conversion(file: &StepFile) -> Option<MapConversion> {
    let mc = file
        .of_entity("IFCMAPCONVERSION")
        .next()
        .or_else(|| file.of_entity("IFCMAPCONVERSIONSCALED").next())?;
    // (SourceCRS, TargetCRS, Eastings, Northings, OrthogonalHeight,
    //  XAxisAbscissa, XAxisOrdinate, Scale)
    let eastings = mc.args.get(2).and_then(Arg::as_f64)?;
    let northings = mc.args.get(3).and_then(Arg::as_f64)?;
    let abscissa = mc.args.get(5).and_then(Arg::as_f64);
    let ordinate = mc.args.get(6).and_then(Arg::as_f64);
    let target = mc
        .args
        .get(1)
        .and_then(Arg::as_ref_id)
        .and_then(|id| file.get(id));
    let crs_name = target.and_then(|t| name_arg(t, 0)).map(str::to_string);
    let epsg = crs_name.as_deref().and_then(|n| {
        let (auth, code) = n.split_once(':')?;
        if auth.trim().eq_ignore_ascii_case("EPSG") {
            code.trim().parse().ok()
        } else {
            None
        }
    });
    Some(MapConversion {
        eastings,
        northings,
        orthogonal_height: mc.args.get(4).and_then(Arg::as_f64),
        rotation_deg: match (abscissa, ordinate) {
            (Some(a), Some(o)) if a != 0.0 || o != 0.0 => Some(o.atan2(a).to_degrees()),
            _ => None,
        },
        scale: mc.args.get(7).and_then(Arg::as_f64),
        crs_name,
        epsg,
        vertical_datum: target.and_then(|t| name_arg(t, 3)).map(str::to_string),
        map_projection: target.and_then(|t| name_arg(t, 4)).map(str::to_string),
        map_zone: target.and_then(|t| name_arg(t, 5)).map(str::to_string),
    })
}

/// Best human label of a rooted instance: Name (arg 2), else LongName for
/// spatial entities (varies), else None.
fn label_of(inst: &Instance) -> Option<&str> {
    name_arg(inst, 2).or_else(|| match inst.entity.as_str() {
        "IFCBUILDINGSTOREY" | "IFCBUILDING" | "IFCSITE" | "IFCSPACE" | "IFCPROJECT" => {
            name_arg(inst, 7)
        }
        _ => None,
    })
}

/// The property-set definitions a `RelatingPropertyDefinition` names: one
/// reference (IFC2x3, and IFC4's common case), or the IFC4 set form
/// `IFCPROPERTYSETDEFINITIONSET((#a,#b))`, which `as_ref_id` cannot see.
fn pset_definition_ids(arg: &Arg, out: &mut Vec<u64>) {
    match arg {
        Arg::Ref(id) => out.push(*id),
        Arg::List(items) => items.iter().for_each(|a| pset_definition_ids(a, out)),
        Arg::Typed(_, inner) => inner.iter().for_each(|a| pset_definition_ids(a, out)),
        _ => {}
    }
}

/// The `IfcMaterial` leaves behind a `RelatingMaterial`: a material itself,
/// or the materials of a layer set (usage), profile set (usage), constituent
/// set or material list.
fn material_leaves(file: &StepFile, id: u64, out: &mut Vec<u64>, depth: usize) {
    if depth > 6 {
        return;
    }
    let Some(inst) = file.get(id) else { return };
    let refs_at = |idx: usize| -> Vec<u64> {
        match inst.args.get(idx) {
            Some(Arg::Ref(r)) => vec![*r],
            Some(Arg::List(items)) => items.iter().filter_map(Arg::as_ref_id).collect(),
            _ => vec![],
        }
    };
    let next: Vec<u64> = match inst.entity.as_str() {
        "IFCMATERIAL" => {
            out.push(id);
            return;
        }
        // (ForLayerSet, …) / (ForProfileSet, …)
        "IFCMATERIALLAYERSETUSAGE" | "IFCMATERIALPROFILESETUSAGE" => refs_at(0),
        // (MaterialLayers, …) / (Materials)
        "IFCMATERIALLAYERSET" | "IFCMATERIALLIST" => refs_at(0),
        // (Material, LayerThickness, …)
        "IFCMATERIALLAYER" => refs_at(0),
        // (Name, Description, MaterialProfiles / MaterialConstituents, …)
        "IFCMATERIALPROFILESET" | "IFCMATERIALCONSTITUENTSET" => refs_at(2),
        // (Name, Description, Material, …)
        "IFCMATERIALPROFILE" | "IFCMATERIALCONSTITUENT" => refs_at(2),
        _ => vec![],
    };
    for n in next {
        material_leaves(file, n, out, depth + 1);
    }
}

/// One link of a classification chain, innermost reference first.
struct ClassificationRef {
    concept: String,
    identification: Option<String>,
    name: Option<String>,
    description: Option<String>,
    location: Option<String>,
}

struct ClassificationScheme {
    iri: String,
    name: Option<String>,
    source: Option<String>,
    edition: Option<String>,
    edition_date: Option<String>,
    location: Option<String>,
}

fn is_absolute_url(s: &str) -> bool {
    s.starts_with("http://") || s.starts_with("https://")
}

/// Walk `ReferencedSource` from a reference up to its `IfcClassification`.
/// IFC2x3 (`ItemReference`) and IFC4 (`Identification`) keep the same
/// attribute positions, so one reader serves both.
fn classification_chain(
    file: &StepFile,
    base: &str,
    ref_id: u64,
) -> Option<(Vec<ClassificationRef>, ClassificationScheme)> {
    let mut chain = Vec::new();
    let mut cur = ref_id;
    let mut scheme_inst = None;
    for _ in 0..16 {
        let inst = file.get(cur)?;
        match inst.entity.as_str() {
            "IFCCLASSIFICATIONREFERENCE" => {
                chain.push(inst);
                match inst.args.get(3).and_then(Arg::as_ref_id) {
                    Some(next) => cur = next,
                    None => break,
                }
            }
            "IFCCLASSIFICATION" => {
                scheme_inst = Some(inst);
                break;
            }
            _ => break,
        }
    }
    if chain.is_empty() {
        return None;
    }
    let scheme_name = scheme_inst.and_then(|s| name_arg(s, 3)).map(str::to_string);
    let scheme_location = scheme_inst
        .and_then(|s| name_arg(s, 5))
        .filter(|l| is_absolute_url(l))
        .map(str::to_string);
    let scheme_key = sanitize(scheme_name.as_deref().unwrap_or("classification"));
    let scheme_iri = scheme_location
        .clone()
        .unwrap_or_else(|| format!("{base}classification/{scheme_key}"));
    let refs = chain
        .iter()
        .map(|r| {
            let location = name_arg(r, 0).map(str::to_string);
            let identification = name_arg(r, 1).map(str::to_string);
            let name = name_arg(r, 2).map(str::to_string);
            let concept = match location.as_deref().filter(|l| is_absolute_url(l)) {
                Some(l) => l.to_string(),
                None => format!(
                    "{base}classification/{scheme_key}/{}",
                    sanitize(
                        identification
                            .as_deref()
                            .or(name.as_deref())
                            .map(str::to_string)
                            .unwrap_or_else(|| format!("ref{}", r.id))
                            .as_str()
                    )
                ),
            };
            ClassificationRef {
                concept,
                identification,
                name,
                description: name_arg(r, 4).map(str::to_string),
                location,
            }
        })
        .collect();
    Some((
        refs,
        ClassificationScheme {
            iri: scheme_iri,
            name: scheme_name,
            source: scheme_inst.and_then(|s| name_arg(s, 0)).map(str::to_string),
            edition: scheme_inst.and_then(|s| name_arg(s, 1)).map(str::to_string),
            edition_date: scheme_inst.and_then(|s| match s.args.get(2) {
                Some(Arg::Str(d)) if !d.trim().is_empty() => Some(d.clone()),
                Some(Arg::Ref(d)) => file.get(*d).and_then(|di| {
                    // IFC2x3 IfcCalendarDate(DayComponent, MonthComponent, YearComponent)
                    let (d, m, y) = (
                        di.args.first().and_then(Arg::as_f64)?,
                        di.args.get(1).and_then(Arg::as_f64)?,
                        di.args.get(2).and_then(Arg::as_f64)?,
                    );
                    Some(format!("{y:04.0}-{m:02.0}-{d:02.0}"))
                }),
                _ => None,
            }),
            location: scheme_location,
        },
    ))
}

pub fn emit(
    file: &StepFile,
    opts: &ConvertOptions,
    bot_out: &mut dyn FnMut(&str),
    ifcowl_out: &mut dyn FnMut(&str),
) -> Result<IfcStats, String> {
    let base = &opts.inst_base;
    let ifc_ns = ifcowl_ns(&file.schema);
    let lift_ns = lift_namespace(opts);
    let class_of = |entity: &str| entity_class(ifc_ns, &lift_ns, &file.schema, entity);
    let lift = |local: &str| iri(&format!("{lift_ns}{local}"));
    let mut stats = IfcStats {
        schema: file.schema.clone(),
        instances: file.instances.len(),
        ..Default::default()
    };

    let spatial_class: HashMap<&str, &str> = names::SPATIAL_BOT.iter().copied().collect();
    let facility_class: HashMap<&str, &str> = names::FACILITY_ZONE.iter().copied().collect();
    let is_spatial =
        |entity: &str| spatial_class.contains_key(entity) || facility_class.contains_key(entity);

    // ── Containment & aggregation edges ────────────────────────────────────────
    // (parent, child) pairs from the two structural relationship entities.
    let mut contains: Vec<(u64, u64)> = Vec::new(); // spatial → element
    let mut aggregates: Vec<(u64, u64)> = Vec::new(); // object → sub-object
    let mut connects: Vec<(u64, u64)> = Vec::new(); // element ↔ element
    for inst in file.instances.values() {
        match inst.entity.as_str() {
            "IFCRELCONTAINEDINSPATIALSTRUCTURE" => {
                // (GlobalId, OH, Name, Desc, RelatedElements, RelatingStructure)
                let parent = inst.args.get(5).and_then(Arg::as_ref_id);
                let kids = inst.args.get(4).and_then(Arg::as_list);
                if let (Some(p), Some(kids)) = (parent, kids) {
                    for k in kids.iter().filter_map(Arg::as_ref_id) {
                        contains.push((p, k));
                    }
                }
            }
            // IfcRelNests is ordered decomposition (ports, sub-parts); both are
            // parthood for the BOT and NEN layers.
            "IFCRELAGGREGATES" | "IFCRELNESTS" => {
                // (GlobalId, OH, Name, Desc, RelatingObject, RelatedObjects)
                let parent = inst.args.get(4).and_then(Arg::as_ref_id);
                let kids = inst.args.get(5).and_then(Arg::as_list);
                if let (Some(p), Some(kids)) = (parent, kids) {
                    for k in kids.iter().filter_map(Arg::as_ref_id) {
                        aggregates.push((p, k));
                    }
                }
            }
            "IFCRELCONNECTSPATHELEMENTS" | "IFCRELCONNECTSELEMENTS" => {
                // (GlobalId, OH, Name, Desc, ConnectionGeometry, RelatingElement, RelatedElement, …)
                if let (Some(a), Some(b)) = (
                    inst.args.get(5).and_then(Arg::as_ref_id),
                    inst.args.get(6).and_then(Arg::as_ref_id),
                ) {
                    connects.push((a, b));
                }
            }
            _ => {}
        }
    }

    // Everything reachable as a child of containment/aggregation that is not
    // itself spatial counts as an element for the BOT layer.
    let mut element_ids: HashSet<u64> = HashSet::new();
    for &(_, k) in contains.iter().chain(aggregates.iter()) {
        if let Some(inst) = file.get(k) {
            if !is_spatial(inst.entity.as_str()) && inst.entity != "IFCPROJECT" {
                element_ids.insert(k);
            }
        }
    }
    // A node is in the BOT layer when it is spatial or an element.
    let in_layer = |id: u64| -> bool {
        element_ids.contains(&id) || file.get(id).is_some_and(|i| is_spatial(i.entity.as_str()))
    };

    // ── BOT layer ───────────────────────────────────────────────────────────────
    let mut bot = NtSink::new(bot_out);
    let fog_ifc_pred = if file.schema.starts_with("IFC4") {
        format!("{FOG}asIfc_v4")
    } else {
        format!("{FOG}asIfc_v2x3")
    };

    // Root spatial element (the last site, else the first building) — resolved
    // up front so the friendly `root_label` override applies during emission.
    let mut site_or_building_anchor: Option<u64> = None;
    for inst in file.instances.values() {
        match inst.entity.as_str() {
            "IFCSITE" => site_or_building_anchor = Some(inst.id),
            "IFCBUILDING" if site_or_building_anchor.is_none() => {
                site_or_building_anchor = Some(inst.id)
            }
            _ => {}
        }
    }
    let root_id = site_or_building_anchor;

    let emit_node = |bot: &mut NtSink,
                     inst: &Instance,
                     bot_class: Option<&str>,
                     lift_class: Option<&str>,
                     is_element: bool| {
        let s = inst_iri(base, inst);
        if let Some(cls) = bot_class {
            bot.triple(&s, &iri(RDF_TYPE), &iri(&format!("{BOT}{cls}")));
        }
        if let Some(cls) = lift_class {
            bot.triple(&s, &iri(RDF_TYPE), &lift(cls));
        }
        if is_element {
            bot.triple(&s, &iri(RDF_TYPE), &iri(&format!("{BOT}Element")));
        }
        bot.triple(&s, &iri(RDF_TYPE), &iri(&class_of(&inst.entity)));
        // The caller's friendly label wins on the ROOT only (exporters leave
        // "Site" / "Default" / "Gelaende" there, which then headlines the whole
        // model in every viewer tree); the file's own name survives as
        // props:ifcName. Every other element keeps its authored name.
        let friendly = opts
            .root_label
            .as_deref()
            .filter(|_| Some(inst.id) == root_id);
        match (friendly, label_of(inst)) {
            (Some(f), authored) => {
                bot.triple(&s, &iri(RDFS_LABEL), &lit(f));
                if let Some(a) = authored {
                    bot.triple(&s, &iri(&format!("{PROPS}ifcName")), &lit(a));
                }
            }
            (None, Some(label)) => bot.triple(&s, &iri(RDFS_LABEL), &lit(label)),
            (None, None) => {}
        }
        if let Some(g) = guid_of(inst) {
            bot.triple(&s, &iri(&format!("{PROPS}ifcGuid")), &lit(g));
            if let Some(uuid) = decode_ifc_guid(g) {
                bot.triple(&s, &iri(&format!("{PROPS}uuid")), &lit(&uuid));
            }
            // FOG reference into the stored IFC (fragment = GlobalId) so every
            // element row in the viewer can reach the file it came from. The
            // node is a STABLE IRI, not a blank node: `_:fog1`-style labels
            // repeat across separately imported buildings, and a union query
            // over their graphs joins equal labels into ONE node — so every
            // building inherited every other building's file URL (the
            // "duplicate models" bug). Per-GUID IRIs cannot collide.
            if let Some(url) = &opts.ifc_file_url {
                let node = sub_iri(&s, "filelink");
                bot.triple(&s, &iri(&format!("{OMG}hasGeometry")), &node);
                bot.triple(&node, &iri(RDF_TYPE), &iri(&format!("{OMG}Geometry")));
                let target = if is_element {
                    format!("{url}#{g}")
                } else {
                    url.clone()
                };
                bot.triple(
                    &node,
                    &iri(&fog_ifc_pred),
                    &format!("{}^^<{XSD}anyURI>", lit(&target)),
                );
                // Authored survey rotation (never TrueNorth — see above).
                if let Some(h) = opts.model_heading.filter(|h| h.is_finite()) {
                    bot.triple(
                        &node,
                        &iri(&format!("{OTS}modelHeading")),
                        &format!("\"{h}\"^^<{XSD}double>"),
                    );
                }
            }
        }
    };

    // Spatial structure nodes: BOT's four, then the IFC 4.3 facilities BOT
    // has no class for — a bot:Zone with the lift's own class.
    for inst in file.instances.values() {
        if let Some(cls) = spatial_class.get(inst.entity.as_str()) {
            emit_node(&mut bot, inst, Some(cls), None, false);
            match inst.entity.as_str() {
                "IFCBUILDINGSTOREY" => stats.storeys += 1,
                "IFCSPACE" => stats.spaces += 1,
                _ => {}
            }
        } else if let Some(cls) = facility_class.get(inst.entity.as_str()) {
            emit_node(&mut bot, inst, Some("Zone"), Some(cls), false);
            stats.facilities += 1;
        }
    }
    // Element nodes.
    for &id in &element_ids {
        if let Some(inst) = file.get(id) {
            emit_node(&mut bot, inst, None, None, true);
        }
    }
    stats.elements = element_ids.len();

    // Containment edges. Spatial→spatial aggregation uses the canonical BOT
    // predicate AND bot:containsElement so the viewer feed (which walks
    // containsElement|hasSubElement) sees the full tree. Beside every BOT
    // edge, the NEN 2660-2 relation it means: location (`contains`) is not
    // parthood (`hasPart`, transitive) and a physical decomposition is
    // `hasTechnicalPart`. Nothing is removed — the BOT edges are the
    // contract of the viewer feed, the IDS importer and the Studio shapes.
    let canonical = |parent: &str, child: &str| -> Option<&'static str> {
        match (parent, child) {
            ("IFCSITE", "IFCBUILDING") => Some("hasBuilding"),
            ("IFCBUILDING", "IFCBUILDINGSTOREY") => Some("hasStorey"),
            ("IFCBUILDINGSTOREY", "IFCSPACE") => Some("hasSpace"),
            _ => None,
        }
    };
    for &(p, k) in &aggregates {
        let (Some(pi), Some(ki)) = (file.get(p), file.get(k)) else {
            continue;
        };
        if pi.entity == "IFCPROJECT" {
            continue; // project → site aggregation isn't part of the BOT tree
        }
        let ps = inst_iri(base, pi);
        let ks = inst_iri(base, ki);
        let both_spatial = is_spatial(&pi.entity) && is_spatial(&ki.entity);
        let both_elements = element_ids.contains(&k) && element_ids.contains(&p);
        if let Some(pred) = canonical(&pi.entity, &ki.entity) {
            bot.triple(&ps, &iri(&format!("{BOT}{pred}")), &ks);
            bot.triple(&ps, &iri(&format!("{BOT}containsElement")), &ks);
        } else if both_elements {
            bot.triple(&ps, &iri(&format!("{BOT}hasSubElement")), &ks);
        } else if both_spatial {
            // A bridge and its deck, a site and its road: zone in zone.
            bot.triple(&ps, &iri(&format!("{BOT}containsZone")), &ks);
            bot.triple(&ps, &iri(&format!("{BOT}containsElement")), &ks);
        } else {
            bot.triple(&ps, &iri(&format!("{BOT}containsElement")), &ks);
        }
        let nen = if both_elements {
            "hasTechnicalPart"
        } else {
            "hasPart"
        };
        bot.triple(&ps, &iri(&format!("{NEN}{nen}")), &ks);
    }
    for &(p, k) in &contains {
        let (Some(pi), Some(ki)) = (file.get(p), file.get(k)) else {
            continue;
        };
        let (ps, ks) = (inst_iri(base, pi), inst_iri(base, ki));
        bot.triple(&ps, &iri(&format!("{BOT}containsElement")), &ks);
        bot.triple(&ps, &iri(&format!("{NEN}contains")), &ks);
    }
    for &(a, b) in &connects {
        if !(in_layer(a) && in_layer(b)) {
            continue;
        }
        let (Some(ai), Some(bi)) = (file.get(a), file.get(b)) else {
            continue;
        };
        bot.triple(
            &inst_iri(base, ai),
            &iri(&format!("{NEN}connectsObject")),
            &inst_iri(base, bi),
        );
    }

    // Anchor geometry on the site/building so the map can place the model —
    // the caller's anchor wins, else the file's own IfcSite georeference,
    // else the map conversion's origin reprojected to WGS84.
    let map_conv = map_conversion(file);
    let anchor_wkt = opts
        .anchor_wkt
        .clone()
        .or_else(|| site_anchor_wkt(file))
        .or_else(|| {
            map_conv
                .as_ref()
                .and_then(MapConversion::wgs84)
                .map(|(lon, lat)| format!("POINT({lon} {lat})"))
        });
    if let (Some(anchor_id), Some(wkt)) = (site_or_building_anchor, &anchor_wkt) {
        if let Some(inst) = file.get(anchor_id) {
            let s = inst_iri(base, inst);
            // Stable IRI, not a blank node — see the FOG node comment above.
            let b = sub_iri(&s, "anchor");
            bot.triple(&s, &iri(&format!("{GEO}hasGeometry")), &b);
            bot.triple(&b, &iri(RDF_TYPE), &iri(&format!("{GEO}Geometry")));
            bot.triple(
                &b,
                &iri(&format!("{GEO}asWKT")),
                &format!("{}^^<{GEO}wktLiteral>", lit(wkt)),
            );
        }
    }

    // The map conversion itself, as read: the origin in the projected CRS
    // (a CRS-qualified WKT when the CRS is one this store knows), the height,
    // the rotation and scale as annotations. It hangs off the root through
    // its own predicate rather than a second geo:hasGeometry, so a consumer
    // that takes "the" anchor still gets the WGS84 one.
    if let (Some(anchor_id), Some(mc)) = (site_or_building_anchor, &map_conv) {
        if let Some(inst) = file.get(anchor_id) {
            let s = inst_iri(base, inst);
            let node = sub_iri(&s, "map-conversion");
            bot.triple(&s, &lift("mapConversion"), &node);
            bot.triple(&node, &iri(RDF_TYPE), &lift("MapConversion"));
            bot.triple(&node, &lift("eastings"), &double_lit(mc.eastings));
            bot.triple(&node, &lift("northings"), &double_lit(mc.northings));
            if let Some(h) = mc.orthogonal_height {
                bot.triple(&node, &lift("orthogonalHeight"), &double_lit(h));
            }
            if let Some(r) = mc.rotation_deg {
                bot.triple(&node, &lift("mapRotation"), &double_lit(r));
            }
            if let Some(sc) = mc.scale {
                bot.triple(&node, &lift("mapScale"), &double_lit(sc));
            }
            if let Some(n) = &mc.crs_name {
                bot.triple(&node, &lift("projectedCrs"), &lit(n));
            }
            if let Some(v) = &mc.vertical_datum {
                bot.triple(&node, &lift("verticalDatum"), &lit(v));
            }
            if let Some(v) = &mc.map_projection {
                bot.triple(&node, &lift("mapProjection"), &lit(v));
            }
            if let Some(v) = &mc.map_zone {
                bot.triple(&node, &lift("mapZone"), &lit(v));
            }
            if let Some(wkt) = mc.wkt() {
                bot.triple(&node, &iri(RDF_TYPE), &iri(&format!("{GEO}Geometry")));
                bot.triple(
                    &node,
                    &iri(&format!("{GEO}asWKT")),
                    &format!("{}^^<{GEO}wktLiteral>", lit(&wkt)),
                );
            }
            stats.map_conversion = true;
        }
    }

    // Provenance on the root: where this model came from, its license and the
    // attribution line — real open BIM datasets (Schependomlaan, the KIT
    // models) require credit, and the root element is where viewers look.
    if let Some(inst) = root_id.and_then(|id| file.get(id)) {
        let s = inst_iri(base, inst);
        if let Some(src) = &opts.provenance_source {
            bot.triple(&s, &iri(&format!("{DCT}source")), &iri(src));
        }
        if let Some(l) = &opts.license {
            bot.triple(&s, &iri(&format!("{DCT}license")), &iri(l));
        }
        if let Some(a) = &opts.attribution {
            bot.triple(&s, &iri(&format!("{DCT}rightsHolder")), &lit(a));
        }
    }

    // ── Property sets and quantity sets ────────────────────────────────────────
    // Property values keep their flat form, props:{Pset}_{Prop} with a typed
    // literal — the contract of the IDS importer and the Studio shapes — and
    // gain a node per value carrying the datatype, the IFC measure type and
    // the unit (its own, else the project default for the measure). Quantity
    // sets, dropped before, become one node per quantity with a QUDT unit
    // plus the same flat props:{Qto}_{Name} literal.
    let project_units = units::project_defaults(file);
    let emit_unit =
        |bot: &mut NtSink, node: &str, unit: &Unit, stats: &mut IfcStats| match &unit.qudt {
            Some(q) => bot.triple(node, &iri(&format!("{QUDT}hasUnit")), &iri(q)),
            None => {
                bot.triple(node, &lift("unitLabel"), &lit(&unit.label));
                if let Some((factor, base_unit)) = &unit.conversion {
                    bot.triple(node, &lift("conversionFactor"), &double_lit(*factor));
                    if let Some(b) = base_unit {
                        bot.triple(node, &lift("conversionUnit"), &iri(b));
                    }
                }
                stats.unmapped_units += 1;
            }
        };
    // The unit of a value: its own `Unit` reference, else the project default
    // for `unit_type`.
    let unit_for = |own: Option<&Arg>, unit_type: Option<&str>| -> Option<Unit> {
        own.and_then(Arg::as_ref_id)
            .and_then(|id| units::resolve(file, id))
            .or_else(|| unit_type.and_then(|t| project_units.get(t).cloned()))
    };

    struct PropCtx<'a> {
        obj: &'a str,
        set_name: &'a str,
        flat_prefix: String,
        path_prefix: String,
        complex: Option<&'a str>,
    }

    #[allow(clippy::too_many_arguments)]
    fn emit_property(
        bot: &mut NtSink,
        stats: &mut IfcStats,
        file: &StepFile,
        lift: &dyn Fn(&str) -> String,
        unit_for: &UnitResolver<'_>,
        emit_unit: &UnitEmitter<'_>,
        ctx: &PropCtx<'_>,
        p: &Instance,
        depth: usize,
    ) {
        let Some(pname) = name_arg(p, 0) else { return };
        let flat = format!("{PROPS}{}_{}", ctx.flat_prefix, sanitize(pname));
        let node = format!("{}/{}>", ctx.path_prefix, sanitize(pname));
        let describe = |bot: &mut NtSink, stats: &mut IfcStats| {
            bot.triple(ctx.obj, &lift("hasProperty"), &node);
            bot.triple(&node, &iri(RDF_TYPE), &lift("Property"));
            bot.triple(&node, &iri(RDFS_LABEL), &lit(pname));
            bot.triple(&node, &lift("propertySet"), &lit(ctx.set_name));
            if let Some(c) = ctx.complex {
                bot.triple(&node, &lift("inComplexProperty"), &lit(c));
            }
            stats.properties += 1;
        };
        let measure_type = |v: &Arg| -> Option<String> {
            match v {
                Arg::Typed(name, _) => Some(names::camel(name).to_string()),
                _ => None,
            }
        };
        match p.entity.as_str() {
            // (Name, Description, NominalValue, Unit)
            "IFCPROPERTYSINGLEVALUE" => {
                let Some(value) = p.args.get(2) else { return };
                let Some(obj_nt) = arg_to_literal(value) else {
                    return;
                };
                bot.triple(ctx.obj, &iri(&flat), &obj_nt);
                describe(bot, stats);
                bot.triple(&node, &lift("value"), &obj_nt);
                if let Some(t) = measure_type(value) {
                    bot.triple(&node, &lift("ifcType"), &lit(&t));
                }
                let unit_type = match value {
                    Arg::Typed(name, _) => units::measure_unit_type(name),
                    _ => None,
                };
                if let Some(u) = unit_for(p.args.get(3), unit_type) {
                    emit_unit(bot, &node, &u, stats);
                }
            }
            // (Name, Description, EnumerationValues, EnumerationReference)
            "IFCPROPERTYENUMERATEDVALUE" => {
                let values: Vec<String> = p
                    .args
                    .get(2)
                    .and_then(Arg::as_list)
                    .map(|l| l.iter().filter_map(arg_to_literal).collect())
                    .unwrap_or_default();
                if values.is_empty() {
                    return;
                }
                for v in &values {
                    bot.triple(ctx.obj, &iri(&flat), v);
                }
                describe(bot, stats);
                for v in &values {
                    bot.triple(&node, &lift("value"), v);
                }
                if let Some(e) = p
                    .args
                    .get(3)
                    .and_then(Arg::as_ref_id)
                    .and_then(|id| file.get(id))
                    .and_then(|e| name_arg(e, 0))
                {
                    bot.triple(&node, &lift("enumeration"), &lit(e));
                }
            }
            // (Name, Description, ListValues, Unit)
            "IFCPROPERTYLISTVALUE" => {
                let items = p.args.get(2).and_then(Arg::as_list).unwrap_or(&[]);
                let values: Vec<String> = items.iter().filter_map(arg_to_literal).collect();
                if values.is_empty() {
                    return;
                }
                for v in &values {
                    bot.triple(ctx.obj, &iri(&flat), v);
                }
                describe(bot, stats);
                for v in &values {
                    bot.triple(&node, &lift("value"), v);
                }
                if let Some(t) = items.first().and_then(measure_type) {
                    bot.triple(&node, &lift("ifcType"), &lit(&t));
                }
                let unit_type = match items.first() {
                    Some(Arg::Typed(name, _)) => units::measure_unit_type(name),
                    _ => None,
                };
                if let Some(u) = unit_for(p.args.get(3), unit_type) {
                    emit_unit(bot, &node, &u, stats);
                }
            }
            // (Name, Description, UpperBoundValue, LowerBoundValue, Unit, SetPointValue)
            // No single value, so no flat triple: the node carries the bounds.
            "IFCPROPERTYBOUNDEDVALUE" => {
                let upper = p.args.get(2).and_then(arg_to_literal);
                let lower = p.args.get(3).and_then(arg_to_literal);
                let set_point = p.args.get(5).and_then(arg_to_literal);
                if upper.is_none() && lower.is_none() && set_point.is_none() {
                    return;
                }
                describe(bot, stats);
                if let Some(v) = &upper {
                    bot.triple(&node, &lift("upperBound"), v);
                }
                if let Some(v) = &lower {
                    bot.triple(&node, &lift("lowerBound"), v);
                }
                if let Some(v) = &set_point {
                    bot.triple(&node, &lift("setPoint"), v);
                }
                let bound = p.args.get(2).or(p.args.get(3)).or(p.args.get(5));
                if let Some(t) = bound.and_then(measure_type) {
                    bot.triple(&node, &lift("ifcType"), &lit(&t));
                }
                let unit_type = match bound {
                    Some(Arg::Typed(name, _)) => units::measure_unit_type(name),
                    _ => None,
                };
                if let Some(u) = unit_for(p.args.get(4), unit_type) {
                    emit_unit(bot, &node, &u, stats);
                }
            }
            // (Name, Description, UsageName, HasProperties): the nested
            // properties are flattened as {Pset}_{Complex}_{Prop} and nested
            // under the complex node's path.
            "IFCCOMPLEXPROPERTY" => {
                if depth > 4 {
                    return;
                }
                let Some(inner) = p.args.get(3).and_then(Arg::as_list) else {
                    return;
                };
                let sub = PropCtx {
                    obj: ctx.obj,
                    set_name: ctx.set_name,
                    flat_prefix: format!("{}_{}", ctx.flat_prefix, sanitize(pname)),
                    path_prefix: node.trim_end_matches('>').to_string(),
                    complex: Some(pname),
                };
                for q in inner.iter().filter_map(Arg::as_ref_id) {
                    if let Some(qi) = file.get(q) {
                        emit_property(
                            bot,
                            stats,
                            file,
                            lift,
                            unit_for,
                            emit_unit,
                            &sub,
                            qi,
                            depth + 1,
                        );
                    }
                }
            }
            _ => {}
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn emit_quantity(
        bot: &mut NtSink,
        stats: &mut IfcStats,
        file: &StepFile,
        lift: &dyn Fn(&str) -> String,
        unit_for: &UnitResolver<'_>,
        emit_unit: &UnitEmitter<'_>,
        ctx: &PropCtx<'_>,
        q: &Instance,
        depth: usize,
    ) {
        let Some(qname) = name_arg(q, 0) else { return };
        match q.entity.as_str() {
            // (Name, Description, HasQuantities, Discrimination, Quality, Usage)
            "IFCPHYSICALCOMPLEXQUANTITY" => {
                if depth > 4 {
                    return;
                }
                let Some(inner) = q.args.get(2).and_then(Arg::as_list) else {
                    return;
                };
                let sub = PropCtx {
                    obj: ctx.obj,
                    set_name: ctx.set_name,
                    flat_prefix: format!("{}_{}", ctx.flat_prefix, sanitize(qname)),
                    path_prefix: format!("{}/{}", ctx.path_prefix, sanitize(qname)),
                    complex: Some(qname),
                };
                for id in inner.iter().filter_map(Arg::as_ref_id) {
                    if let Some(qi) = file.get(id) {
                        emit_quantity(
                            bot,
                            stats,
                            file,
                            lift,
                            unit_for,
                            emit_unit,
                            &sub,
                            qi,
                            depth + 1,
                        );
                    }
                }
            }
            // (Name, Description, Unit, <Value>, Formula)
            "IFCQUANTITYLENGTH" | "IFCQUANTITYAREA" | "IFCQUANTITYVOLUME" | "IFCQUANTITYCOUNT"
            | "IFCQUANTITYWEIGHT" | "IFCQUANTITYTIME" | "IFCQUANTITYNUMBER" => {
                let Some(value) = q.args.get(3) else { return };
                let Some(obj_nt) = arg_to_literal(value) else {
                    return;
                };
                let kind = q.entity.trim_start_matches("IFCQUANTITY");
                let kind = format!("{}{}", &kind[..1], kind[1..].to_ascii_lowercase());
                let flat = format!("{PROPS}{}_{}", ctx.flat_prefix, sanitize(qname));
                let node = format!("{}/{}>", ctx.path_prefix, sanitize(qname));
                bot.triple(ctx.obj, &iri(&flat), &obj_nt);
                bot.triple(ctx.obj, &lift("hasQuantity"), &node);
                bot.triple(&node, &iri(RDF_TYPE), &lift("Quantity"));
                bot.triple(&node, &iri(RDFS_LABEL), &lit(qname));
                bot.triple(&node, &lift("quantitySet"), &lit(ctx.set_name));
                bot.triple(&node, &lift("quantityKind"), &lit(&kind));
                if let Some(c) = ctx.complex {
                    bot.triple(&node, &lift("inComplexQuantity"), &lit(c));
                }
                bot.triple(&node, &iri(&format!("{QUDT}numericValue")), &obj_nt);
                if let Some(u) = unit_for(q.args.get(2), units::quantity_unit_type(&q.entity)) {
                    emit_unit(bot, &node, &u, stats);
                }
                stats.quantities += 1;
            }
            _ => {}
        }
    }

    for inst in file.instances.values() {
        if inst.entity != "IFCRELDEFINESBYPROPERTIES" {
            continue;
        }
        let Some(objs) = inst.args.get(4).and_then(Arg::as_list) else {
            continue;
        };
        let mut defs = Vec::new();
        if let Some(arg) = inst.args.get(5) {
            pset_definition_ids(arg, &mut defs);
        }
        for def_id in defs {
            let Some(def) = file.get(def_id) else {
                continue;
            };
            // (GlobalId, OwnerHistory, Name, Description, HasProperties)
            // (GlobalId, OwnerHistory, Name, Description, MethodOfMeasurement, Quantities)
            let (members_idx, is_qto) = match def.entity.as_str() {
                "IFCPROPERTYSET" => (4, false),
                "IFCELEMENTQUANTITY" => (5, true),
                _ => continue,
            };
            let set_name = name_arg(def, 2).unwrap_or(if is_qto { "Qto" } else { "Pset" });
            let set_key = sanitize(set_name);
            let Some(members) = def.args.get(members_idx).and_then(Arg::as_list) else {
                continue;
            };
            for obj in objs.iter().filter_map(Arg::as_ref_id) {
                let Some(obj_inst) = file.get(obj) else {
                    continue;
                };
                // Only attach to nodes that exist in the BOT layer.
                if !in_layer(obj) {
                    continue;
                }
                let s = inst_iri(base, obj_inst);
                let ctx = PropCtx {
                    obj: &s,
                    set_name,
                    flat_prefix: set_key.clone(),
                    path_prefix: format!(
                        "{}/{}/{}",
                        s.trim_end_matches('>'),
                        if is_qto { "qto" } else { "pset" },
                        set_key
                    ),
                    complex: None,
                };
                for m in members.iter().filter_map(Arg::as_ref_id) {
                    let Some(mi) = file.get(m) else { continue };
                    if is_qto {
                        emit_quantity(
                            &mut bot, &mut stats, file, &lift, &unit_for, &emit_unit, &ctx, mi, 0,
                        );
                    } else {
                        emit_property(
                            &mut bot, &mut stats, file, &lift, &unit_for, &emit_unit, &ctx, mi, 0,
                        );
                    }
                }
            }
        }
    }

    // ── Materials ──────────────────────────────────────────────────────────────
    // The matter an element is made of: the lift's own link, the NEN 2660-2
    // constitution relation (`consistsOf` — not parthood) and the flat
    // props:ifcMaterial literal the IDS importer already targets.
    let mut typed_materials: HashSet<u64> = HashSet::new();
    for inst in file.of_entity("IFCRELASSOCIATESMATERIAL") {
        // (GlobalId, OH, Name, Desc, RelatedObjects, RelatingMaterial)
        let Some(objs) = inst.args.get(4).and_then(Arg::as_list) else {
            continue;
        };
        let Some(mat) = inst.args.get(5).and_then(Arg::as_ref_id) else {
            continue;
        };
        let mut leaves = Vec::new();
        material_leaves(file, mat, &mut leaves, 0);
        for obj in objs.iter().filter_map(Arg::as_ref_id) {
            if !in_layer(obj) {
                continue;
            }
            let Some(obj_inst) = file.get(obj) else {
                continue;
            };
            let s = inst_iri(base, obj_inst);
            for &m in &leaves {
                let Some(mi) = file.get(m) else { continue };
                let ms = inst_iri(base, mi);
                bot.triple(&s, &lift("hasMaterial"), &ms);
                bot.triple(&s, &iri(&format!("{NEN}consistsOf")), &ms);
                if let Some(n) = name_arg(mi, 0) {
                    bot.triple(&s, &iri(&format!("{PROPS}ifcMaterial")), &lit(n));
                }
                if typed_materials.insert(m) {
                    bot.triple(&ms, &iri(RDF_TYPE), &lift("Material"));
                    if let Some(n) = name_arg(mi, 0) {
                        bot.triple(&ms, &iri(RDFS_LABEL), &lit(n));
                    }
                }
                stats.materials += 1;
            }
        }
    }

    // ── Classifications → SKOS ─────────────────────────────────────────────────
    // A reference becomes a skos:Concept (notation = Identification, label =
    // Name) in a skos:ConceptScheme for the IfcClassification, chained with
    // skos:broader when the file carries the hierarchy. The element links to
    // the concept, and to the flat props:ifcClassification literal the IDS
    // importer targets — which nothing emitted before.
    let mut described_concepts: HashSet<String> = HashSet::new();
    for inst in file.of_entity("IFCRELASSOCIATESCLASSIFICATION") {
        // (GlobalId, OH, Name, Desc, RelatedObjects, RelatingClassification)
        let Some(objs) = inst.args.get(4).and_then(Arg::as_list) else {
            continue;
        };
        let Some(cref) = inst.args.get(5).and_then(Arg::as_ref_id) else {
            continue;
        };
        let Some((chain, scheme)) = classification_chain(file, base, cref) else {
            continue;
        };
        let scheme_iri = iri(&scheme.iri);
        if described_concepts.insert(scheme.iri.clone()) {
            bot.triple(
                &scheme_iri,
                &iri(RDF_TYPE),
                &iri(&format!("{SKOS}ConceptScheme")),
            );
            if let Some(n) = &scheme.name {
                bot.triple(&scheme_iri, &iri(&format!("{DCT}title")), &lit(n));
            }
            if let Some(v) = &scheme.source {
                bot.triple(&scheme_iri, &iri(&format!("{DCT}publisher")), &lit(v));
            }
            if let Some(v) = &scheme.edition {
                bot.triple(&scheme_iri, &iri(&format!("{DCT}hasVersion")), &lit(v));
            }
            if let Some(v) = &scheme.edition_date {
                bot.triple(&scheme_iri, &iri(&format!("{DCT}issued")), &lit(v));
            }
            if let Some(l) = &scheme.location {
                bot.triple(&scheme_iri, &iri(&format!("{DCT}source")), &iri(l));
            }
        }
        for (i, r) in chain.iter().enumerate() {
            let c = iri(&r.concept);
            if !described_concepts.insert(r.concept.clone()) {
                continue;
            }
            bot.triple(&c, &iri(RDF_TYPE), &iri(&format!("{SKOS}Concept")));
            if let Some(v) = &r.identification {
                bot.triple(&c, &iri(&format!("{SKOS}notation")), &lit(v));
            }
            if let Some(v) = &r.name {
                bot.triple(&c, &iri(&format!("{SKOS}prefLabel")), &lit(v));
            }
            if let Some(v) = &r.description {
                bot.triple(&c, &iri(&format!("{SKOS}definition")), &lit(v));
            }
            if let Some(l) = r.location.as_deref().filter(|l| is_absolute_url(l)) {
                if l != r.concept {
                    bot.triple(&c, &iri(&format!("{DCT}source")), &iri(l));
                }
            }
            bot.triple(&c, &iri(&format!("{SKOS}inScheme")), &scheme_iri);
            match chain.get(i + 1) {
                Some(parent) => {
                    bot.triple(&c, &iri(&format!("{SKOS}broader")), &iri(&parent.concept))
                }
                None => bot.triple(&c, &iri(&format!("{SKOS}topConceptOf")), &scheme_iri),
            }
        }
        let leaf = &chain[0];
        for obj in objs.iter().filter_map(Arg::as_ref_id) {
            if !in_layer(obj) {
                continue;
            }
            let Some(obj_inst) = file.get(obj) else {
                continue;
            };
            let s = inst_iri(base, obj_inst);
            bot.triple(&s, &lift("hasClassification"), &iri(&leaf.concept));
            if let Some(v) = leaf.identification.as_deref().or(leaf.name.as_deref()) {
                bot.triple(&s, &iri(&format!("{PROPS}ifcClassification")), &lit(v));
            }
            stats.classifications += 1;
        }
    }

    stats.bot_triples = bot.finish();

    // ── ifcOWL layer ───────────────────────────────────────────────────────────
    if opts.include_ifcowl {
        let mut owl = NtSink::new(ifcowl_out);
        for inst in file.instances.values() {
            let s = inst_iri(base, inst);
            owl.triple(&s, &iri(RDF_TYPE), &iri(&class_of(&inst.entity)));
            let attr_names = names::attrs_of(&inst.entity);
            for (i, arg) in inst.args.iter().enumerate() {
                if matches!(arg, Arg::Null | Arg::Star) {
                    continue;
                }
                let pred = match attr_names.and_then(|a| a.get(i)) {
                    Some(name) => format!("{ifc_ns}{}", lower_first(name)),
                    None => format!("{ifc_ns}arg{i:02}"),
                };
                emit_owl_value(&mut owl, base, file, &s, &pred, arg);
            }
        }
        stats.ifcowl_triples = owl.finish();
    }

    Ok(stats)
}

/// Emit one attribute value in the direct encoding: refs become IRIs (lists of
/// refs repeat the predicate), primitives become typed literals, primitive
/// lists collapse to a single space-joined literal.
fn emit_owl_value(sink: &mut NtSink, base: &str, file: &StepFile, s: &str, pred: &str, arg: &Arg) {
    match arg {
        Arg::Ref(id) => {
            let o = match file.get(*id) {
                Some(target) => inst_iri(base, target),
                None => format!("<{base}i{id}>"),
            };
            sink.triple(s, &iri(pred), &o);
        }
        Arg::List(items) => {
            if items
                .iter()
                .any(|a| matches!(a, Arg::Ref(_) | Arg::List(_)))
            {
                for item in items {
                    emit_owl_value(sink, base, file, s, pred, item);
                }
            } else if let Some(joined) = join_primitives(items) {
                sink.triple(s, &iri(pred), &lit(&joined));
            }
        }
        other => {
            if let Some(o) = arg_to_literal(other) {
                sink.triple(s, &iri(pred), &o);
            }
        }
    }
}

/// A primitive list as one compact literal: `(0.0 1.5 -2.0)`.
fn join_primitives(items: &[Arg]) -> Option<String> {
    if items.is_empty() {
        return None;
    }
    let parts: Vec<String> = items
        .iter()
        .map(|a| match a {
            Arg::Int(i) => i.to_string(),
            Arg::Float(f) => format!("{f}"),
            Arg::Str(s) => s.clone(),
            Arg::Enum(e) => e.clone(),
            Arg::Typed(_, inner) => inner
                .first()
                .and_then(|x| join_primitives(std::slice::from_ref(x)))
                .map(|s| s.trim_matches(['(', ')']).to_string())
                .unwrap_or_default(),
            _ => String::new(),
        })
        .collect();
    Some(format!("({})", parts.join(" ")))
}

/// One primitive argument as an N-Triples literal (with datatype).
fn arg_to_literal(arg: &Arg) -> Option<String> {
    match arg {
        Arg::Str(s) => Some(lit(s)),
        Arg::Int(i) => Some(typed_lit(&i.to_string(), "integer")),
        Arg::Float(f) => Some(typed_lit(&format!("{f}"), "double")),
        Arg::Enum(e) => match e.as_str() {
            "T" => Some(typed_lit("true", "boolean")),
            "F" => Some(typed_lit("false", "boolean")),
            other => Some(lit(other)),
        },
        Arg::Typed(name, inner) => {
            let v = inner.first()?;
            match (name.as_str(), v) {
                ("IFCBOOLEAN" | "IFCLOGICAL", Arg::Enum(e)) => Some(typed_lit(
                    if e == "T" { "true" } else { "false" },
                    "boolean",
                )),
                (_, Arg::Int(i)) => Some(typed_lit(&i.to_string(), "integer")),
                (_, Arg::Float(f)) => Some(typed_lit(&format!("{f}"), "double")),
                (_, Arg::Str(s)) => Some(lit(s)),
                (_, Arg::Enum(e)) => Some(lit(e)),
                _ => None,
            }
        }
        Arg::List(items) => join_primitives(items).map(|j| lit(&j)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ifc::{convert, ConvertOptions};

    #[test]
    fn dms_handles_positive_negative_and_string_forms() {
        let list = |v: &[f64]| Arg::List(v.iter().map(|&f| Arg::Float(f)).collect());
        // KIT Smiley West latitude: (49 1 59 680200) ≈ 49.0332°.
        let lat = dms_to_deg(&list(&[49.0, 1.0, 59.0, 680200.0])).unwrap();
        assert!((lat - 49.0333).abs() < 1e-3, "{lat}");
        // Chicago longitude, all components negative per IfcCompoundPlaneAngleMeasure:
        // magnitudes must ADD under one sign, not cancel each other.
        let lon = dms_to_deg(&list(&[-87.0, -38.0, -21.0, -839999.0])).unwrap();
        assert!((lon - -87.6394).abs() < 1e-3, "{lon}");
        // Stringified form (as it appears in ifcOWL lifts / lax exporters).
        let s = dms_to_deg(&Arg::Str("(49 1 59 680200)".to_string())).unwrap();
        assert!((s - lat).abs() < 1e-9, "{s}");
        let s2 = dms_to_deg(&Arg::Str("-87, -38, -21, -839999".to_string())).unwrap();
        assert!((s2 - lon).abs() < 1e-9, "{s2}");
    }

    #[test]
    fn the_lift_namespace_sits_under_the_base_url() {
        let ns = |inst_base: &str| {
            lift_namespace(&ConvertOptions {
                inst_base: inst_base.into(),
                ..Default::default()
            })
        };
        assert_eq!(
            ns("https://data.example.org/dataset/x/building/"),
            "https://data.example.org/ns/ifc-lift#"
        );
        // A path-prefixed deployment keeps its prefix.
        assert_eq!(
            ns("https://example.org/ots/dataset/x/building/"),
            "https://example.org/ots/ns/ifc-lift#"
        );
        // A custom graph IRI: the origin.
        assert_eq!(
            ns("http://localhost:7878/graphs/site-a#"),
            "http://localhost:7878/ns/ifc-lift#"
        );
        assert_eq!(ns("urn:x:"), LIFT_NS_FALLBACK);
    }

    const SAMPLE: &str = "ISO-10303-21;\nHEADER;\nFILE_SCHEMA(('IFC2X3'));\nENDSEC;\nDATA;\n\
#1= IFCPROJECT('0AAAAAAAAAAAAAAAAAAAA1',$,'Proj',$,$,$,$,(),$);\n\
#2= IFCSITE('0AAAAAAAAAAAAAAAAAAAA2',$,'Site',$,$,$,$,$,.ELEMENT.,$,$,$,$,$);\n\
#3= IFCBUILDING('0AAAAAAAAAAAAAAAAAAAA3',$,'Huis',$,$,$,$,$,.ELEMENT.,$,$,$);\n\
#4= IFCBUILDINGSTOREY('0AAAAAAAAAAAAAAAAAAAA4',$,'00 begane grond',$,$,$,$,$,.ELEMENT.,0.);\n\
#5= IFCBEAM('0AAAAAAAAAAAAAAAAAAAA5',$,'HEA180',$,$,$,$,'tag');\n\
#6= IFCRELAGGREGATES('0AAAAAAAAAAAAAAAAAAAB1',$,$,$,#1,(#2));\n\
#7= IFCRELAGGREGATES('0AAAAAAAAAAAAAAAAAAAB2',$,$,$,#2,(#3));\n\
#8= IFCRELAGGREGATES('0AAAAAAAAAAAAAAAAAAAB3',$,$,$,#3,(#4));\n\
#9= IFCRELCONTAINEDINSPATIALSTRUCTURE('0AAAAAAAAAAAAAAAAAAAB4',$,$,$,(#5),#4);\n\
#10= IFCPROPERTYSET('0AAAAAAAAAAAAAAAAAAAB5',$,'Pset_BeamCommon',$,(#11));\n\
#11= IFCPROPERTYSINGLEVALUE('LoadBearing',$,IFCBOOLEAN(.T.),$);\n\
#12= IFCRELDEFINESBYPROPERTIES('0AAAAAAAAAAAAAAAAAAAB6',$,$,$,(#5),#10);\n\
ENDSEC;\nEND-ISO-10303-21;";

    fn run(include_ifcowl: bool) -> (String, String, IfcStats) {
        let mut bot = String::new();
        let mut owl = String::new();
        let stats = convert(
            SAMPLE,
            &ConvertOptions {
                inst_base: "http://ex.test/m/".into(),
                ifc_file_url: Some("http://ex.test/files/model.ifc".into()),
                anchor_wkt: Some("POINT(5.83 51.84)".into()),
                include_ifcowl,
                ..Default::default()
            },
            &mut |c| bot.push_str(c),
            &mut |c| owl.push_str(c),
        )
        .unwrap();
        (bot, owl, stats)
    }

    #[test]
    fn authored_heading_is_stamped_on_file_links_and_absent_by_default() {
        // Default: no heading triple anywhere (the viewer's +X-east default).
        let (bot, _, _) = run(false);
        assert!(!bot.contains("modelHeading"), "no heading unless authored");
        // Authored: every element file-link node carries it, typed xsd:double —
        // the exact shape the viewer feed parses (`?og ots:modelHeading ?mhead`).
        let mut bot = String::new();
        let mut owl = String::new();
        convert(
            SAMPLE,
            &ConvertOptions {
                inst_base: "http://ex.test/m/".into(),
                ifc_file_url: Some("http://ex.test/files/model.ifc".into()),
                model_heading: Some(68.2),
                ..Default::default()
            },
            &mut |c| bot.push_str(c),
            &mut |c| owl.push_str(c),
        )
        .unwrap();
        assert!(
            bot.contains(
                "<https://opentriplestore.org/ns#modelHeading> \"68.2\"^^<http://www.w3.org/2001/XMLSchema#double>"
            ),
            "authored heading must be emitted as xsd:double: {bot}"
        );
        // A non-finite value must be dropped, not serialised as NaN.
        let mut bot2 = String::new();
        let mut owl2 = String::new();
        convert(
            SAMPLE,
            &ConvertOptions {
                inst_base: "http://ex.test/m/".into(),
                ifc_file_url: Some("http://ex.test/files/model.ifc".into()),
                model_heading: Some(f64::NAN),
                ..Default::default()
            },
            &mut |c| bot2.push_str(c),
            &mut |c| owl2.push_str(c),
        )
        .unwrap();
        assert!(!bot2.contains("modelHeading"));
    }

    #[test]
    fn bot_layer_has_topology_labels_guids_props_and_anchor() {
        let (bot, _, stats) = run(false);
        assert_eq!(stats.schema, "IFC2X3");
        assert_eq!(stats.storeys, 1);
        assert_eq!(stats.elements, 1, "the beam");
        // Topology spine: site→building→storey (canonical + feed edges) and
        // storey→beam containment.
        assert!(bot.contains("https://w3id.org/bot#hasBuilding"));
        assert!(bot.contains("https://w3id.org/bot#hasStorey"));
        assert!(bot.contains(&"<http://ex.test/m/0AAAAAAAAAAAAAAAAAAAA4> <https://w3id.org/bot#containsElement> <http://ex.test/m/0AAAAAAAAAAAAAAAAAAAA5>".to_string()));
        // Beam: element type + ifcOWL class + label + guid + per-element IFC ref.
        assert!(bot.contains("https://w3id.org/bot#Element"));
        assert!(bot.contains("OWL#IfcBeam"));
        assert!(bot.contains("\"HEA180\""));
        assert!(bot.contains("\"0AAAAAAAAAAAAAAAAAAAA5\""));
        assert!(bot.contains("model.ifc#0AAAAAAAAAAAAAAAAAAAA5"));
        // Property set value lands as a direct boolean.
        assert!(bot.contains("https://w3id.org/props#Pset_BeamCommon_LoadBearing"));
        assert!(bot.contains("\"true\"^^<http://www.w3.org/2001/XMLSchema#boolean>"));
        // Site anchor WKT.
        assert!(bot.contains("asWKT"));
        assert!(bot.contains("POINT(5.83 51.84)"));
    }

    /// Scale validation against the real Schependomlaan design model. Runs only
    /// when the (git-ignored) scratch download is present — a no-op in CI.
    #[test]
    fn converts_real_schependomlaan_when_present() {
        let path = std::path::Path::new("scratch/Schependomlaan.ifc");
        if !path.exists() {
            return;
        }
        let input = std::fs::read_to_string(path).unwrap();
        let t0 = std::time::Instant::now();
        let mut bot_len = 0usize;
        let mut owl_len = 0usize;
        let stats = convert(
            &input,
            &ConvertOptions {
                inst_base: "http://ex.test/sd/".into(),
                ifc_file_url: Some("http://ex.test/files/schependomlaan.ifc".into()),
                anchor_wkt: Some("POINT(5.8337 51.8411)".into()),
                include_ifcowl: true,
                ..Default::default()
            },
            &mut |c| bot_len += c.len(),
            &mut |c| owl_len += c.len(),
        )
        .unwrap();
        eprintln!(
            "schependomlaan: {} instances, {} elements, {} storeys, {} spaces, bot {} triples ({} KB), ifcowl {} triples ({} MB), in {:?}",
            stats.instances, stats.elements, stats.storeys, stats.spaces,
            stats.bot_triples, bot_len / 1024, stats.ifcowl_triples, owl_len / (1024 * 1024),
            t0.elapsed()
        );
        assert_eq!(stats.schema, "IFC2X3");
        assert!(stats.instances > 700_000, "{}", stats.instances);
        assert_eq!(stats.storeys, 6);
        assert!(stats.elements > 1_500, "{}", stats.elements);
        assert!(stats.bot_triples > 10_000);
        assert!(stats.ifcowl_triples > 1_000_000);
    }

    #[test]
    fn ifcowl_layer_lifts_every_instance_with_named_attrs() {
        let (_, owl, stats) = run(true);
        assert!(stats.ifcowl_triples > stats.bot_triples / 2);
        // Storey elevation: named attribute from the schema table.
        assert!(owl.contains("OWL#elevation"));
        assert!(owl.contains("OWL#IfcBuildingStorey"));
        // Relationship references resolved to the GUID-based IRIs.
        assert!(owl.contains("OWL#relatingObject"));
        assert!(owl.contains("<http://ex.test/m/0AAAAAAAAAAAAAAAAAAAA2>"));
        // The property value keeps its boolean typing.
        assert!(owl.contains("\"true\"^^<http://www.w3.org/2001/XMLSchema#boolean>"));
    }
}
