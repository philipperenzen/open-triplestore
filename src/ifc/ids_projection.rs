//! The IDS projection of an IFC file: the facts an IDS 1.0 checker reads,
//! as RDF, beside (never instead of) the building-topology layer.
//!
//! The BOT layer is shaped for the viewer and SHACL Studio: only spatial
//! elements and the elements contained in or aggregated into them, property
//! values in project units, one merged parthood edge. IDS needs more and
//! different facts, so this projection is a separate output (its own graph,
//! `{building graph}/ids`), and the BOT output stays byte-identical:
//!
//! - **every instance** with its **exact class**, schema-qualified —
//!   `rdf:type <https://opentriplestore.org/ns/ifc-ids/IFC4#IFCWALL>` — and no
//!   subclass axioms anywhere, so `sh:targetClass` matches the class itself and
//!   nothing below it. An IFC2X3 occurrence typed by a type object listed in
//!   the IDS IFC2X3 occurrence/type mapping table also carries the IFC4 name it
//!   stands for (`IfcFlowTerminal` + `IfcAirTerminalType` → `IFCAIRTERMINAL`);
//! - **every explicit attribute** from the schema tables (`schema.rs`), one
//!   triple each under `ifc-ids/attr#<Name>`: strings, enumerations, numbers and
//!   booleans as literals, references as the target's IRI, a non-empty
//!   aggregate as `ifcids:Aggregate`, and an empty string, a logical UNKNOWN or
//!   an empty aggregate as `ifcids:Empty` (present, but never a value);
//! - the **resolved predefined type** (`ifcids:predefinedType`): the type
//!   object's unless it is NOTDEFINED, else the instance's own, with the
//!   user-defined label beside `USERDEFINED`;
//! - **property and quantity sets** as IDS sees them: the type object's,
//!   overridden property by property by the occurrence's, plus material and
//!   predefined property sets; each property carries its name, its IFC data
//!   type(s) and its value(s) — numbers converted to the SI units IDS compares
//!   in (`ids_units.txt`), and no value for a null, an empty string or a
//!   logical UNKNOWN;
//! - **part-of edges per relation**: aggregation, nesting, spatial
//!   containment, group assignment and voids/fills, direct (the IDS importer
//!   walks aggregation and nesting transitively);
//! - **materials** (every name and category in the associated material, layer,
//!   profile and constituent sets and lists; the type's unless the occurrence
//!   has its own) and **classifications** (the system of each reference and the
//!   identification of the reference and its parents; the type's, overridden
//!   per system by the occurrence's).

use std::collections::{BTreeMap, HashMap, HashSet};

use super::rdf::{inst_iri, iri, lit, typed_lit, NtSink};
use super::schema::{self, AttrKind, Primitive, Schema, SchemaId};
use super::step::{Arg, Instance, StepFile};
use super::ConvertOptions;

/// The projection's own terms.
pub const NS: &str = "https://opentriplestore.org/ns/ifc-ids#";
/// Attribute predicates: `<…/attr#Name>`.
pub const ATTR_NS: &str = "https://opentriplestore.org/ns/ifc-ids/attr#";
const CLASS_NS_BASE: &str = "https://opentriplestore.org/ns/ifc-ids/";
const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";

/// The namespace of a schema's exact classes, e.g.
/// `https://opentriplestore.org/ns/ifc-ids/IFC4#`.
pub fn class_ns(schema: SchemaId) -> String {
    format!("{CLASS_NS_BASE}{}#", schema.ids_name())
}

/// The model node of a projection written under `inst_base`.
pub fn model_iri(inst_base: &str) -> String {
    format!("{inst_base}ids-model")
}

/// What the projection wrote.
#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct ProjectionStats {
    pub triples: usize,
    pub instances: usize,
    pub property_sets: usize,
}

struct AttrInfo {
    name: String,
    kind: AttrKind,
    ty: String,
}

#[derive(Clone, Copy)]
enum PropRef {
    /// An IfcProperty / IfcPhysicalQuantity instance.
    Inst(u64),
    /// Attribute `index` of a predefined property set instance.
    PreAttr(u64, usize),
}

struct Ix<'f> {
    file: &'f StepFile,
    base: &'f str,
    sid: Option<SchemaId>,
    schema: Option<&'static Schema>,
    attrs: HashMap<String, Vec<AttrInfo>>,
    type_of: HashMap<u64, u64>,
    rel_psets: HashMap<u64, Vec<u64>>,
    material_psets: HashMap<u64, Vec<u64>>,
    materials: HashMap<u64, Vec<u64>>,
    classifications: HashMap<u64, Vec<u64>>,
    aggregated_in: HashMap<u64, Vec<u64>>,
    nested_in: HashMap<u64, Vec<u64>>,
    contained_in: HashMap<u64, Vec<u64>>,
    in_group: HashMap<u64, Vec<u64>>,
    voids_fills: HashMap<u64, Vec<u64>>,
    unit_defaults: HashMap<String, u64>,
}

fn refs(arg: Option<&Arg>) -> Vec<u64> {
    let mut out = Vec::new();
    fn walk(a: &Arg, out: &mut Vec<u64>) {
        match a {
            Arg::Ref(r) => out.push(*r),
            Arg::List(items) => items.iter().for_each(|i| walk(i, out)),
            Arg::Typed(_, inner) => inner.iter().for_each(|i| walk(i, out)),
            _ => {}
        }
    }
    if let Some(a) = arg {
        walk(a, &mut out);
    }
    out
}

fn push(map: &mut HashMap<u64, Vec<u64>>, k: u64, v: u64) {
    let e = map.entry(k).or_default();
    if !e.contains(&v) {
        e.push(v);
    }
}

fn double_nt(v: f64) -> String {
    let s = if v.is_nan() {
        "NaN".to_string()
    } else if v.is_infinite() {
        if v > 0.0 { "INF" } else { "-INF" }.to_string()
    } else {
        format!("{v:?}")
    };
    typed_lit(&s, "double")
}

/// The power of ten an `IfcSIPrefix` stands for.
fn prefix_exponent(p: &str) -> Option<i32> {
    Some(match p {
        "EXA" => 18,
        "PETA" => 15,
        "TERA" => 12,
        "GIGA" => 9,
        "MEGA" => 6,
        "KILO" => 3,
        "HECTO" => 2,
        "DECA" => 1,
        "DECI" => -1,
        "CENTI" => -2,
        "MILLI" => -3,
        "MICRO" => -6,
        "NANO" => -9,
        "PICO" => -12,
        "FEMTO" => -15,
        "ATTO" => -18,
        _ => return None,
    })
}

/// `v · 10^exp`, dividing for a negative exponent so that 2800 mm is exactly
/// the double nearest 2.8 m (`2800 · 0.001` is not).
fn scale10(v: f64, exp: i32) -> f64 {
    if exp >= 0 {
        v * 10f64.powi(exp)
    } else {
        v / 10f64.powi(-exp)
    }
}

/// Percent-encode a name for use inside an IRI path segment.
fn segment(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for b in name.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    if out.is_empty() {
        out.push_str("%00");
    }
    out
}

fn sub(iri_nt: &str, suffix: &str) -> String {
    format!("{}/{suffix}>", iri_nt.trim_end_matches('>'))
}

impl<'f> Ix<'f> {
    fn build(file: &'f StepFile, base: &'f str) -> Ix<'f> {
        let sid = SchemaId::from_file_schema(&file.schema);
        let schema = sid.map(SchemaId::schema);
        let mut attrs: HashMap<String, Vec<AttrInfo>> = HashMap::new();
        if let Some(s) = schema {
            let mut seen: HashSet<&str> = HashSet::new();
            for inst in file.instances.values() {
                if seen.insert(inst.entity.as_str()) {
                    let list = s
                        .attributes(&inst.entity)
                        .into_iter()
                        .map(|(a, _)| AttrInfo {
                            name: a.name.clone(),
                            kind: s.attr_kind(&a.ty),
                            ty: a.ty.clone(),
                        })
                        .collect();
                    attrs.insert(inst.entity.clone(), list);
                }
            }
        }
        let mut ix = Ix {
            file,
            base,
            sid,
            schema,
            attrs,
            type_of: HashMap::new(),
            rel_psets: HashMap::new(),
            material_psets: HashMap::new(),
            materials: HashMap::new(),
            classifications: HashMap::new(),
            aggregated_in: HashMap::new(),
            nested_in: HashMap::new(),
            contained_in: HashMap::new(),
            in_group: HashMap::new(),
            voids_fills: HashMap::new(),
            unit_defaults: HashMap::new(),
        };
        ix.index_relations();
        ix
    }

    /// An attribute of `inst` by name.
    fn arg<'a>(&self, inst: &'a Instance, name: &str) -> Option<&'a Arg> {
        let i = self
            .attrs
            .get(&inst.entity)?
            .iter()
            .position(|a| a.name == name)?;
        inst.args.get(i)
    }

    fn str_attr<'a>(&self, inst: &'a Instance, name: &str) -> Option<&'a str> {
        match self.arg(inst, name) {
            Some(Arg::Str(s)) if !s.is_empty() => Some(s.as_str()),
            Some(Arg::Typed(_, inner)) => match inner.first() {
                Some(Arg::Str(s)) if !s.is_empty() => Some(s.as_str()),
                _ => None,
            },
            _ => None,
        }
    }

    fn enum_attr<'a>(&self, inst: &'a Instance, name: &str) -> Option<&'a str> {
        match self.arg(inst, name) {
            Some(Arg::Enum(e)) => Some(e.as_str()),
            _ => None,
        }
    }

    fn index_relations(&mut self) {
        let file = self.file;
        let mut ids: Vec<u64> = file.instances.keys().copied().collect();
        ids.sort_unstable();
        for id in ids {
            let inst = &file.instances[&id];
            let e = inst.entity.as_str();
            match e {
                "IFCRELDEFINESBYTYPE" => {
                    if let Some(t) = refs(self.arg(inst, "RelatingType")).first().copied() {
                        for o in refs(self.arg(inst, "RelatedObjects")) {
                            self.type_of.insert(o, t);
                        }
                    }
                }
                "IFCRELDEFINESBYPROPERTIES" => {
                    let defs = refs(self.arg(inst, "RelatingPropertyDefinition"));
                    for o in refs(self.arg(inst, "RelatedObjects")) {
                        for &d in &defs {
                            push(&mut self.rel_psets, o, d);
                        }
                    }
                }
                "IFCRELASSOCIATESMATERIAL" => {
                    let mats = refs(self.arg(inst, "RelatingMaterial"));
                    for o in refs(self.arg(inst, "RelatedObjects")) {
                        for &m in &mats {
                            push(&mut self.materials, o, m);
                        }
                    }
                }
                "IFCRELASSOCIATESCLASSIFICATION" => {
                    let cs = refs(self.arg(inst, "RelatingClassification"));
                    for o in refs(self.arg(inst, "RelatedObjects")) {
                        for &c in &cs {
                            push(&mut self.classifications, o, c);
                        }
                    }
                }
                "IFCEXTERNALREFERENCERELATIONSHIP" => {
                    let cs = refs(self.arg(inst, "RelatingReference"));
                    for o in refs(self.arg(inst, "RelatedResourceObjects")) {
                        for &c in &cs {
                            let is_class = file.get(c).is_some_and(|ci| {
                                matches!(
                                    ci.entity.as_str(),
                                    "IFCCLASSIFICATIONREFERENCE" | "IFCCLASSIFICATION"
                                )
                            });
                            if is_class {
                                push(&mut self.classifications, o, c);
                            }
                        }
                    }
                }
                "IFCRELAGGREGATES" | "IFCRELNESTS" => {
                    let whole = refs(self.arg(inst, "RelatingObject"));
                    let parts = refs(self.arg(inst, "RelatedObjects"));
                    let map = if e == "IFCRELAGGREGATES" {
                        &mut self.aggregated_in
                    } else {
                        &mut self.nested_in
                    };
                    for part in parts {
                        for &w in &whole {
                            push(map, part, w);
                        }
                    }
                }
                "IFCRELCONTAINEDINSPATIALSTRUCTURE" => {
                    let s = refs(self.arg(inst, "RelatingStructure"));
                    for el in refs(self.arg(inst, "RelatedElements")) {
                        for &c in &s {
                            push(&mut self.contained_in, el, c);
                        }
                    }
                }
                "IFCRELASSIGNSTOGROUP" | "IFCRELASSIGNSTOGROUPBYFACTOR" => {
                    let g = refs(self.arg(inst, "RelatingGroup"));
                    for o in refs(self.arg(inst, "RelatedObjects")) {
                        for &gg in &g {
                            push(&mut self.in_group, o, gg);
                        }
                    }
                }
                "IFCRELVOIDSELEMENT" => {
                    let w = refs(self.arg(inst, "RelatingBuildingElement"));
                    for o in refs(self.arg(inst, "RelatedOpeningElement")) {
                        for &ww in &w {
                            push(&mut self.voids_fills, o, ww);
                        }
                    }
                }
                "IFCMATERIALPROPERTIES" | "IFCEXTENDEDMATERIALPROPERTIES" => {
                    if let Some(m) = refs(self.arg(inst, "Material")).first().copied() {
                        push(&mut self.material_psets, m, id);
                    }
                }
                _ => {}
            }
        }
        // A filling element's whole is the element its opening voids.
        for inst in file.of_entity("IFCRELFILLSELEMENT") {
            let openings = refs(self.arg(inst, "RelatingOpeningElement"));
            for filler in refs(self.arg(inst, "RelatedBuildingElement")) {
                for o in &openings {
                    let wholes = self.voids_fills.get(o).cloned().unwrap_or_default();
                    for w in wholes {
                        push(&mut self.voids_fills, filler, w);
                    }
                }
            }
        }
        // Project default units, by unit type.
        if let Some(project) = file.of_entity("IFCPROJECT").next() {
            if let Some(assignment) = refs(self.arg(project, "UnitsInContext"))
                .first()
                .and_then(|a| file.get(*a))
            {
                for u in refs(self.arg(assignment, "Units")) {
                    let Some(ui) = file.get(u) else { continue };
                    let t = match ui.entity.as_str() {
                        "IFCMONETARYUNIT" => None,
                        _ => self.enum_attr(ui, "UnitType").map(str::to_string),
                    };
                    if let Some(t) = t {
                        self.unit_defaults.entry(t).or_insert(u);
                    }
                }
            }
        }
    }

    // ── units ──────────────────────────────────────────────────────────────

    /// A value in the unit `id`, in SI units (kilogram for mass, kelvin for
    /// temperature, radian for angles, as the IDS units table nominates).
    fn unit_to_si(&self, id: u64, v: f64, depth: u8) -> Option<f64> {
        if depth > 8 {
            return None;
        }
        let u = self.file.get(id)?;
        match u.entity.as_str() {
            "IFCSIUNIT" => {
                let name = self.enum_attr(u, "Name")?;
                let p = match self.enum_attr(u, "Prefix") {
                    Some(p) => prefix_exponent(p)?,
                    None => 0,
                };
                Some(match name {
                    "SQUARE_METRE" => scale10(v, 2 * p),
                    "CUBIC_METRE" => scale10(v, 3 * p),
                    // The SI unit of mass is the kilogram.
                    "GRAM" => scale10(v, p - 3),
                    "DEGREE_CELSIUS" => scale10(v, p) + 273.15,
                    _ => scale10(v, p),
                })
            }
            "IFCCONVERSIONBASEDUNIT" | "IFCCONVERSIONBASEDUNITWITHOFFSET" => {
                let m = self
                    .file
                    .get(*refs(self.arg(u, "ConversionFactor")).first()?)?;
                let factor = self.arg(m, "ValueComponent").and_then(Arg::as_f64)?;
                let offset = self
                    .arg(u, "ConversionOffset")
                    .and_then(Arg::as_f64)
                    .unwrap_or(0.0);
                let in_component = v * factor + offset;
                match refs(self.arg(m, "UnitComponent")).first() {
                    Some(c) => self.unit_to_si(*c, in_component, depth + 1),
                    None => Some(in_component),
                }
            }
            "IFCDERIVEDUNIT" => {
                let mut scale = 1.0;
                for el in refs(self.arg(u, "Elements")) {
                    let e = self.file.get(el)?;
                    let unit = refs(self.arg(e, "Unit")).first().copied()?;
                    let exp = match self.arg(e, "Exponent") {
                        Some(Arg::Int(i)) => *i as i32,
                        Some(a) => a.as_f64()? as i32,
                        None => 1,
                    };
                    scale *= self.unit_to_si(unit, 1.0, depth + 1)?.powi(exp);
                }
                Some(v * scale)
            }
            _ => None,
        }
    }

    /// A measure value in the SI unit IDS compares it in.
    fn to_si(&self, measure_upper: &str, own_unit: Option<&Arg>, v: f64) -> f64 {
        let Some(unit_type) = schema::ids_unit_type(measure_upper) else {
            return v;
        };
        let unit = refs(own_unit)
            .first()
            .copied()
            .or_else(|| self.unit_defaults.get(unit_type).copied());
        unit.and_then(|u| self.unit_to_si(u, v, 0)).unwrap_or(v)
    }

    // ── values ─────────────────────────────────────────────────────────────

    /// A typed value (`IFCLABEL('x')`, `IFCLENGTHMEASURE(2.)`) as its IDS data
    /// type and, when it is a value, its literal.
    fn typed_value(&self, arg: &Arg, unit: Option<&Arg>) -> Option<(String, Option<String>)> {
        let Arg::Typed(name, inner) = arg else {
            return None;
        };
        let prim = self.schema.and_then(|s| s.primitive_of(name));
        let literal = inner.first().and_then(|v| match (prim, v) {
            (Some(Primitive::Boolean | Primitive::Logical), Arg::Enum(e)) => match e.as_str() {
                "T" => Some(typed_lit("true", "boolean")),
                "F" => Some(typed_lit("false", "boolean")),
                _ => None,
            },
            (_, Arg::Str(s)) => (!s.is_empty()).then(|| lit(s)),
            (Some(Primitive::Integer), Arg::Int(i)) => Some(typed_lit(&i.to_string(), "integer")),
            (_, Arg::Int(i)) => {
                let si = self.to_si(name, unit, *i as f64);
                if si.fract() == 0.0 && schema::ids_unit_type(name).is_none() {
                    Some(typed_lit(&i.to_string(), "integer"))
                } else {
                    Some(double_nt(si))
                }
            }
            (_, Arg::Float(f)) => Some(double_nt(self.to_si(name, unit, *f))),
            (_, Arg::Enum(e)) => Some(lit(e)),
            _ => None,
        });
        Some((name.to_ascii_uppercase(), literal))
    }

    /// An explicit attribute value, or `None` when it is unset or derived.
    fn attr_object(&self, info: &AttrInfo, arg: &Arg) -> Option<String> {
        let empty = || iri(&format!("{NS}Empty"));
        Some(match arg {
            Arg::Null | Arg::Star => return None,
            Arg::Str(s) if s.is_empty() => empty(),
            Arg::Str(s) => lit(s),
            Arg::Enum(e) => match info.kind {
                AttrKind::Primitive(Primitive::Boolean | Primitive::Logical) => match e.as_str() {
                    "T" => typed_lit("true", "boolean"),
                    "F" => typed_lit("false", "boolean"),
                    _ => empty(),
                },
                _ => lit(e),
            },
            Arg::Int(i) => match info.kind {
                AttrKind::Primitive(Primitive::Real | Primitive::Number) => double_nt(*i as f64),
                _ => typed_lit(&i.to_string(), "integer"),
            },
            Arg::Float(f) => double_nt(*f),
            Arg::Ref(r) => match self.file.get(*r) {
                Some(t) => inst_iri(self.base, t),
                None => format!("<{}i{r}>", self.base),
            },
            Arg::List(items) if items.is_empty() => empty(),
            Arg::List(_) => iri(&format!("{NS}Aggregate")),
            Arg::Typed(_, inner) => match inner.first() {
                Some(Arg::Str(s)) if s.is_empty() => empty(),
                Some(Arg::Str(s)) => lit(s),
                Some(Arg::Int(i)) => typed_lit(&i.to_string(), "integer"),
                Some(Arg::Float(f)) => double_nt(*f),
                Some(Arg::Enum(e)) if e == "T" => typed_lit("true", "boolean"),
                Some(Arg::Enum(e)) if e == "F" => typed_lit("false", "boolean"),
                Some(Arg::Enum(e)) if e == "U" => empty(),
                Some(Arg::Enum(e)) => lit(e),
                Some(Arg::List(l)) if l.is_empty() => empty(),
                Some(Arg::List(_)) => iri(&format!("{NS}Aggregate")),
                _ => return None,
            },
        })
    }

    // ── predefined type ────────────────────────────────────────────────────

    fn user_label<'a>(&self, inst: &'a Instance) -> Option<&'a str> {
        ["ObjectType", "ElementType", "ProcessType", "ResourceType"]
            .iter()
            .find_map(|a| self.str_attr(inst, a))
    }

    fn own_predefined(&self, inst: &Instance) -> Vec<String> {
        let Some(pt) = self.enum_attr(inst, "PredefinedType") else {
            return Vec::new();
        };
        let mut out = vec![pt.to_string()];
        if pt == "USERDEFINED" {
            if let Some(l) = self.user_label(inst) {
                out.push(l.to_string());
            }
        }
        out
    }

    fn predefined_types(&self, inst: &Instance) -> Vec<String> {
        if let Some(t) = self.type_of.get(&inst.id).and_then(|t| self.file.get(*t)) {
            let from_type = self.own_predefined(t);
            if from_type.first().is_some_and(|p| p != "NOTDEFINED") {
                return from_type;
            }
            let own = self.own_predefined(inst);
            if own.is_empty() {
                return from_type;
            }
            return own;
        }
        self.own_predefined(inst)
    }

    // ── property sets ──────────────────────────────────────────────────────

    fn prop_name(&self, p: PropRef) -> Option<String> {
        match p {
            PropRef::Inst(id) => {
                let i = self.file.get(id)?;
                match i.args.first() {
                    Some(Arg::Str(s)) => Some(s.clone()),
                    _ => None,
                }
            }
            PropRef::PreAttr(set, idx) => {
                let i = self.file.get(set)?;
                Some(self.attrs.get(&i.entity)?.get(idx)?.name.clone())
            }
        }
    }

    /// A property set definition's name and members.
    fn pset_members(&self, def: u64) -> Option<(String, Vec<PropRef>)> {
        let d = self.file.get(def)?;
        let name = |n: &str| self.str_attr(d, n).map(str::to_string);
        match d.entity.as_str() {
            "IFCPROPERTYSET" => Some((
                name("Name")?,
                refs(self.arg(d, "HasProperties"))
                    .into_iter()
                    .map(PropRef::Inst)
                    .collect(),
            )),
            "IFCELEMENTQUANTITY" => Some((
                name("Name")?,
                refs(self.arg(d, "Quantities"))
                    .into_iter()
                    .map(PropRef::Inst)
                    .collect(),
            )),
            "IFCMATERIALPROPERTIES" => Some((
                name("Name")?,
                refs(self.arg(d, "Properties"))
                    .into_iter()
                    .map(PropRef::Inst)
                    .collect(),
            )),
            "IFCEXTENDEDMATERIALPROPERTIES" => Some((
                name("Name")?,
                refs(self.arg(d, "ExtendedProperties"))
                    .into_iter()
                    .map(PropRef::Inst)
                    .collect(),
            )),
            "IFCPROPERTYSETDEFINITIONSET" => None,
            _ => {
                // A predefined property set: its own attributes beyond the
                // IfcRoot four are its properties.
                let s = self.schema?;
                if !(s.is_a(&d.entity, "IfcPropertySetDefinition"))
                    || s.is_a(&d.entity, "IfcQuantitySet")
                {
                    return None;
                }
                let n = self.attrs.get(&d.entity)?.len();
                Some((
                    name("Name")?,
                    (4..n).map(|i| PropRef::PreAttr(def, i)).collect(),
                ))
            }
        }
    }

    fn own_psets(&self, id: u64) -> Vec<(String, Vec<PropRef>)> {
        let mut defs: Vec<u64> = self.rel_psets.get(&id).cloned().unwrap_or_default();
        if let Some(inst) = self.file.get(id) {
            for d in refs(self.arg(inst, "HasPropertySets")) {
                if !defs.contains(&d) {
                    defs.push(d);
                }
            }
        }
        if let Some(m) = self.material_psets.get(&id) {
            defs.extend(m.iter().copied());
        }
        defs.into_iter()
            .filter_map(|d| self.pset_members(d))
            .collect()
    }

    /// Pset name → property name → property, the type object's first and
    /// the occurrence's over it.
    fn effective_psets(&self, id: u64) -> BTreeMap<String, BTreeMap<String, PropRef>> {
        let mut out: BTreeMap<String, BTreeMap<String, PropRef>> = BTreeMap::new();
        let mut layers = Vec::new();
        if let Some(t) = self.type_of.get(&id) {
            if *t != id {
                layers.push(*t);
            }
        }
        layers.push(id);
        for layer in layers {
            for (name, props) in self.own_psets(layer) {
                let set = out.entry(name).or_default();
                for p in props {
                    if let Some(pn) = self.prop_name(p) {
                        set.insert(pn, p);
                    }
                }
            }
        }
        out
    }

    /// The data types and values of one property.
    fn prop_values(&self, p: PropRef) -> (Vec<String>, Vec<String>) {
        let mut types = Vec::new();
        let mut values = Vec::new();
        let mut add = |tv: Option<(String, Option<String>)>| {
            if let Some((t, v)) = tv {
                if !types.contains(&t) {
                    types.push(t);
                }
                if let Some(v) = v {
                    values.push(v);
                }
            }
        };
        match p {
            PropRef::PreAttr(set, idx) => {
                let Some(inst) = self.file.get(set) else {
                    return (types, values);
                };
                let Some(info) = self.attrs.get(&inst.entity).and_then(|a| a.get(idx)) else {
                    return (types, values);
                };
                let t = info.ty.to_ascii_uppercase();
                let v = inst.args.get(idx).and_then(|a| match (a, info.kind) {
                    (
                        Arg::Enum(e),
                        AttrKind::Primitive(Primitive::Boolean | Primitive::Logical),
                    ) => match e.as_str() {
                        "T" => Some(typed_lit("true", "boolean")),
                        "F" => Some(typed_lit("false", "boolean")),
                        _ => None,
                    },
                    (Arg::Enum(e), _) => Some(lit(e)),
                    (Arg::Str(s), _) if !s.is_empty() => Some(lit(s)),
                    (Arg::Int(i), AttrKind::Primitive(Primitive::Integer)) => {
                        Some(typed_lit(&i.to_string(), "integer"))
                    }
                    (Arg::Int(i), _) => Some(double_nt(self.to_si(&t, None, *i as f64))),
                    (Arg::Float(f), _) => Some(double_nt(self.to_si(&t, None, *f))),
                    _ => None,
                });
                if matches!(info.kind, AttrKind::Primitive(_) | AttrKind::Enum)
                    && !matches!(inst.args.get(idx), None | Some(Arg::Null) | Some(Arg::Star))
                {
                    add(Some((t, v)));
                }
            }
            PropRef::Inst(id) => {
                let Some(pi) = self.file.get(id) else {
                    return (types, values);
                };
                match pi.entity.as_str() {
                    "IFCPROPERTYSINGLEVALUE" => {
                        let unit = self.arg(pi, "Unit");
                        if let Some(v) = self.arg(pi, "NominalValue") {
                            add(self.typed_value(v, unit));
                        }
                    }
                    "IFCPROPERTYENUMERATEDVALUE" => {
                        if let Some(Arg::List(items)) = self.arg(pi, "EnumerationValues") {
                            for v in items {
                                add(self.typed_value(v, None));
                            }
                        }
                    }
                    "IFCPROPERTYLISTVALUE" => {
                        let unit = self.arg(pi, "Unit");
                        if let Some(Arg::List(items)) = self.arg(pi, "ListValues") {
                            for v in items {
                                add(self.typed_value(v, unit));
                            }
                        }
                    }
                    "IFCPROPERTYBOUNDEDVALUE" => {
                        let unit = self.arg(pi, "Unit");
                        for a in ["UpperBoundValue", "LowerBoundValue", "SetPointValue"] {
                            if let Some(v) = self.arg(pi, a) {
                                add(self.typed_value(v, unit));
                            }
                        }
                    }
                    "IFCPROPERTYTABLEVALUE" => {
                        for (vals, unit) in [
                            ("DefiningValues", "DefiningUnit"),
                            ("DefinedValues", "DefinedUnit"),
                        ] {
                            let unit = self.arg(pi, unit);
                            if let Some(Arg::List(items)) = self.arg(pi, vals) {
                                for v in items {
                                    add(self.typed_value(v, unit));
                                }
                            }
                        }
                    }
                    q if q.starts_with("IFCQUANTITY") => {
                        let (value_attr, measure) = match q {
                            "IFCQUANTITYLENGTH" => ("LengthValue", "IFCLENGTHMEASURE"),
                            "IFCQUANTITYAREA" => ("AreaValue", "IFCAREAMEASURE"),
                            "IFCQUANTITYVOLUME" => ("VolumeValue", "IFCVOLUMEMEASURE"),
                            "IFCQUANTITYCOUNT" => ("CountValue", "IFCCOUNTMEASURE"),
                            "IFCQUANTITYWEIGHT" => ("WeightValue", "IFCMASSMEASURE"),
                            "IFCQUANTITYTIME" => ("TimeValue", "IFCTIMEMEASURE"),
                            "IFCQUANTITYNUMBER" => ("NumberValue", "IFCNUMERICMEASURE"),
                            _ => return (types, values),
                        };
                        let unit = self.arg(pi, "Unit");
                        let v = self.arg(pi, value_attr).and_then(|a| match a {
                            Arg::Int(i) if measure == "IFCCOUNTMEASURE" => {
                                Some(typed_lit(&i.to_string(), "integer"))
                            }
                            other => other
                                .as_f64()
                                .map(|f| double_nt(self.to_si(measure, unit, f))),
                        });
                        add(Some((measure.to_string(), v)));
                    }
                    // Reference and complex properties carry no value IDS can
                    // check; the property is still present by name.
                    _ => {}
                }
            }
        }
        (types, values)
    }

    // ── classifications and materials ──────────────────────────────────────

    /// The system name and the identifications (the reference's and its
    /// parents') of an associated classification or reference.
    fn classification_info(&self, id: u64) -> (Option<String>, Vec<String>) {
        let mut values = Vec::new();
        let mut cur = id;
        for _ in 0..32 {
            let Some(inst) = self.file.get(cur) else {
                break;
            };
            match inst.entity.as_str() {
                "IFCCLASSIFICATION" => {
                    let system = match self.arg(inst, "Name") {
                        Some(Arg::Str(s)) => Some(s.clone()),
                        _ => None,
                    };
                    return (system, values);
                }
                "IFCCLASSIFICATIONREFERENCE" => {
                    let ident = self
                        .str_attr(inst, "Identification")
                        .or_else(|| self.str_attr(inst, "ItemReference"));
                    if let Some(v) = ident {
                        values.push(v.to_string());
                    }
                    match refs(self.arg(inst, "ReferencedSource")).first() {
                        Some(next) => cur = *next,
                        None => break,
                    }
                }
                _ => break,
            }
        }
        (None, values)
    }

    fn effective_classifications(&self, id: u64) -> Vec<u64> {
        let own = self.classifications.get(&id).cloned().unwrap_or_default();
        let own_systems: HashSet<Option<String>> =
            own.iter().map(|c| self.classification_info(*c).0).collect();
        let mut out = own;
        if let Some(t) = self.type_of.get(&id) {
            for c in self.classifications.get(t).into_iter().flatten() {
                if !own_systems.contains(&self.classification_info(*c).0) && !out.contains(c) {
                    out.push(*c);
                }
            }
        }
        out
    }

    fn material_values(&self, id: u64, out: &mut Vec<String>, depth: u8) {
        if depth > 8 {
            return;
        }
        let Some(inst) = self.file.get(id) else {
            return;
        };
        for a in ["Name", "Category", "LayerSetName"] {
            if let Some(v) = self.str_attr(inst, a) {
                if !out.iter().any(|o| o == v) {
                    out.push(v.to_string());
                }
            }
        }
        for a in [
            "Material",
            "Materials",
            "MaterialLayers",
            "MaterialProfiles",
            "MaterialConstituents",
            "ForLayerSet",
            "ForProfileSet",
        ] {
            for r in refs(self.arg(inst, a)) {
                self.material_values(r, out, depth + 1);
            }
        }
    }

    fn effective_materials(&self, id: u64) -> Vec<u64> {
        match self.materials.get(&id) {
            Some(m) if !m.is_empty() => m.clone(),
            _ => self
                .type_of
                .get(&id)
                .and_then(|t| self.materials.get(t))
                .cloned()
                .unwrap_or_default(),
        }
    }
}

/// Emit the IDS projection of a parsed file as N-Triples chunks.
pub fn emit(
    file: &StepFile,
    opts: &ConvertOptions,
    out: &mut dyn FnMut(&str),
) -> Result<ProjectionStats, String> {
    let base = opts.inst_base.as_str();
    let ix = Ix::build(file, base);
    let mut sink = NtSink::new(out);
    let mut stats = ProjectionStats::default();
    let rdf_type = iri(RDF_TYPE);
    let p = |local: &str| iri(&format!("{NS}{local}"));

    let model = iri(&model_iri(base));
    sink.triple(&model, &rdf_type, &p("Model"));
    sink.triple(
        &model,
        &p("schema"),
        &lit(&match ix.sid {
            Some(s) => s.ids_name().to_string(),
            None => file.schema.clone(),
        }),
    );
    let Some(sid) = ix.sid else {
        stats.triples = sink.finish();
        return Ok(stats);
    };
    let cns = class_ns(sid);
    let type_map: Vec<&(String, String, String)> = if sid == SchemaId::Ifc2x3 {
        schema::ifc2x3_type_map().iter().collect()
    } else {
        Vec::new()
    };

    let mut ids: Vec<u64> = file.instances.keys().copied().collect();
    ids.sort_unstable();
    let mut described_props: HashSet<String> = HashSet::new();
    let mut described_class: HashSet<u64> = HashSet::new();
    let mut described_mat: HashSet<u64> = HashSet::new();
    for id in ids {
        let inst = &file.instances[&id];
        let s = inst_iri(base, inst);
        stats.instances += 1;
        sink.triple(&s, &rdf_type, &iri(&format!("{cns}{}", inst.entity)));
        if !type_map.is_empty() {
            if let Some(t) = ix.type_of.get(&id).and_then(|t| file.get(*t)) {
                for (alias, occ, ty) in &type_map {
                    if *occ == inst.entity && *ty == t.entity {
                        sink.triple(&s, &rdf_type, &iri(&format!("{cns}{alias}")));
                    }
                }
            }
        }
        // Attributes.
        if let Some(list) = ix.attrs.get(&inst.entity) {
            for (i, info) in list.iter().enumerate() {
                let Some(arg) = inst.args.get(i) else { break };
                if let Some(o) = ix.attr_object(info, arg) {
                    sink.triple(&s, &iri(&format!("{ATTR_NS}{}", info.name)), &o);
                }
            }
        }
        for pt in ix.predefined_types(inst) {
            sink.triple(&s, &p("predefinedType"), &lit(&pt));
        }
        // Property and quantity sets.
        for (pset, props) in ix.effective_psets(id) {
            let ps = sub(&s, &format!("ids/pset/{}", segment(&pset)));
            sink.triple(&s, &p("pset"), &ps);
            sink.triple(&ps, &p("name"), &lit(&pset));
            stats.property_sets += 1;
            for (pname, pr) in props {
                let node = match pr {
                    PropRef::Inst(pid) => match file.get(pid) {
                        Some(pi) => inst_iri(base, pi),
                        None => continue,
                    },
                    PropRef::PreAttr(set, idx) => match file.get(set) {
                        Some(si) => sub(&inst_iri(base, si), &format!("ids/attr/{idx}")),
                        None => continue,
                    },
                };
                sink.triple(&ps, &p("property"), &node);
                if described_props.insert(node.clone()) {
                    sink.triple(&node, &p("name"), &lit(&pname));
                    let (types, values) = ix.prop_values(pr);
                    for t in types {
                        sink.triple(&node, &p("dataType"), &lit(&t));
                    }
                    for v in values {
                        sink.triple(&node, &p("value"), &v);
                    }
                }
            }
        }
        // Classifications.
        for c in ix.effective_classifications(id) {
            let Some(ci) = file.get(c) else { continue };
            let cs = inst_iri(base, ci);
            sink.triple(&s, &p("classification"), &cs);
            if described_class.insert(c) {
                let (system, values) = ix.classification_info(c);
                if let Some(sys) = system {
                    sink.triple(&cs, &p("system"), &lit(&sys));
                }
                for v in values {
                    sink.triple(&cs, &p("reference"), &lit(&v));
                }
            }
        }
        // Materials.
        for m in ix.effective_materials(id) {
            let Some(mi) = file.get(m) else { continue };
            let ms = inst_iri(base, mi);
            sink.triple(&s, &p("material"), &ms);
            if described_mat.insert(m) {
                let mut values = Vec::new();
                ix.material_values(m, &mut values, 0);
                for v in values {
                    sink.triple(&ms, &p("materialValue"), &lit(&v));
                }
            }
        }
        // Part-of edges.
        for (map, pred) in [
            (&ix.aggregated_in, "aggregatedIn"),
            (&ix.nested_in, "nestedIn"),
            (&ix.contained_in, "containedIn"),
            (&ix.in_group, "inGroup"),
            (&ix.voids_fills, "voidsFillsIn"),
        ] {
            for w in map.get(&id).into_iter().flatten() {
                if let Some(wi) = file.get(*w) {
                    sink.triple(&s, &p(pred), &inst_iri(base, wi));
                }
            }
        }
    }
    stats.triples = sink.finish();
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ifc::step::parse;

    fn project(src: &str) -> String {
        let file = parse(src).expect("parses");
        let mut out = String::new();
        emit(
            &file,
            &ConvertOptions {
                inst_base: "http://ex.test/m/".into(),
                ..Default::default()
            },
            &mut |c| out.push_str(c),
        )
        .expect("projects");
        out
    }

    const WALLS: &str = "ISO-10303-21;\nHEADER;\nFILE_SCHEMA(('IFC4'));\nENDSEC;\nDATA;\n\
#1=IFCPROJECT('1hqIFTRjfV6AWq_bMtnZwI',$,$,$,$,$,$,$,#6);\n\
#2=IFCSIUNIT(*,.LENGTHUNIT.,.MILLI.,.METRE.);\n\
#6=IFCUNITASSIGNMENT((#2));\n\
#7=IFCWALL('2nJrDaLQfJ1QPhdJR0o97J',$,'',$,'X',$,$,$,.USERDEFINED.);\n\
#8=IFCWALLTYPE('16MocU_IDOF8_x3Iqllz0d',$,$,$,$,(#10),$,$,$,.NOTDEFINED.);\n\
#9=IFCRELDEFINESBYTYPE('1xdwj8qGXK4hzoNbvMdXJW',$,$,$,(#7),#8);\n\
#10=IFCPROPERTYSET('0WTUhjMwvT39YBFH2pryoM',$,'Foo_Bar',$,(#11,#15));\n\
#11=IFCPROPERTYSINGLEVALUE('Foo',$,IFCLABEL('Baz'),$);\n\
#12=IFCPROPERTYSET('3b0AoFivPN6RDJO6UL_GfZ',$,'Foo_Bar',$,(#14));\n\
#13=IFCRELDEFINESBYPROPERTIES('1UJX0DW6PGVvNXUEmD0sBq',$,$,$,(#7),#12);\n\
#14=IFCPROPERTYSINGLEVALUE('Foo',$,IFCLABEL('Bar'),$);\n\
#15=IFCPROPERTYSINGLEVALUE('Len',$,IFCLENGTHMEASURE(2000.),$);\n\
#16=IFCSLAB('0BbkGoC6vPvRW13UT7D8zH',$,$,$,$,$,$,$,$);\n\
#17=IFCRELAGGREGATES('05rScmOVzMoQXOfbYdtLYj',$,$,$,#16,(#7));\n\
ENDSEC;\nEND-ISO-10303-21;";

    #[test]
    fn instances_carry_exact_classes_attributes_and_resolved_types() {
        let nt = project(WALLS);
        let wall = "<http://ex.test/m/2nJrDaLQfJ1QPhdJR0o97J>";
        assert!(
            nt.contains(&format!(
                "{wall} <{RDF_TYPE}> <https://opentriplestore.org/ns/ifc-ids/IFC4#IFCWALL>"
            )),
            "{nt}"
        );
        // An empty string is present but never a value.
        assert!(
            nt.contains(&format!("{wall} <{ATTR_NS}Name> <{NS}Empty>")),
            "{nt}"
        );
        assert!(
            nt.contains(&format!("{wall} <{ATTR_NS}ObjectType> \"X\"")),
            "{nt}"
        );
        // The type is NOTDEFINED, so the occurrence's own USERDEFINED + label win.
        assert!(
            nt.contains(&format!("{wall} <{NS}predefinedType> \"USERDEFINED\"")),
            "{nt}"
        );
        assert!(
            nt.contains(&format!("{wall} <{NS}predefinedType> \"X\"")),
            "{nt}"
        );
        assert!(
            nt.contains(&format!(
                "{wall} <{NS}aggregatedIn> <http://ex.test/m/0BbkGoC6vPvRW13UT7D8zH>"
            )),
            "{nt}"
        );
        assert!(
            nt.contains(&format!(
                "<http://ex.test/m/ids-model> <{NS}schema> \"IFC4\""
            )),
            "{nt}"
        );
    }

    #[test]
    fn occurrence_properties_override_the_type_and_measures_are_si() {
        let nt = project(WALLS);
        let wall = "<http://ex.test/m/2nJrDaLQfJ1QPhdJR0o97J>";
        let pset = "<http://ex.test/m/2nJrDaLQfJ1QPhdJR0o97J/ids/pset/Foo_Bar>";
        assert!(nt.contains(&format!("{wall} <{NS}pset> {pset}")), "{nt}");
        // Foo comes from the occurrence (#14 'Bar'), not the type (#11 'Baz').
        assert!(
            nt.contains(&format!("{pset} <{NS}property> <http://ex.test/m/i14>")),
            "{nt}"
        );
        assert!(
            !nt.contains(&format!("{pset} <{NS}property> <http://ex.test/m/i11>")),
            "{nt}"
        );
        // Len is inherited from the type, 2000 mm → 2 m.
        assert!(
            nt.contains(&format!("{pset} <{NS}property> <http://ex.test/m/i15>")),
            "{nt}"
        );
        assert!(
            nt.contains("<http://ex.test/m/i15> <https://opentriplestore.org/ns/ifc-ids#value> \"2.0\"^^<http://www.w3.org/2001/XMLSchema#double>"),
            "{nt}"
        );
        assert!(
            nt.contains(&format!(
                "<http://ex.test/m/i15> <{NS}dataType> \"IFCLENGTHMEASURE\""
            )),
            "{nt}"
        );
    }

    #[test]
    fn an_ifc2x3_occurrence_takes_the_mapped_ifc4_name_from_its_type() {
        let nt = project(
            "ISO-10303-21;\nHEADER;\nFILE_SCHEMA(('IFC2X3'));\nENDSEC;\nDATA;\n\
#1=IFCFLOWTERMINAL('3DfdUozX589eoywUhuqpni',$,$,$,$,$,$,$);\n\
#2=IFCAIRTERMINALTYPE('0CxV3poQjDqf1V11SKqYWT',$,$,$,$,$,$,$,$,.DIFFUSER.);\n\
#3=IFCRELDEFINESBYTYPE('1iA4ifE7v18vYAHPT$6r$_',$,$,$,(#1),#2);\n\
ENDSEC;\nEND-ISO-10303-21;",
        );
        let t = "<http://ex.test/m/3DfdUozX589eoywUhuqpni>";
        assert!(
            nt.contains(&format!(
                "{t} <{RDF_TYPE}> <https://opentriplestore.org/ns/ifc-ids/IFC2X3#IFCAIRTERMINAL>"
            )),
            "{nt}"
        );
        assert!(
            nt.contains(&format!(
                "{t} <{RDF_TYPE}> <https://opentriplestore.org/ns/ifc-ids/IFC2X3#IFCFLOWTERMINAL>"
            )),
            "{nt}"
        );
        assert!(
            nt.contains(&format!("{t} <{NS}predefinedType> \"DIFFUSER\"")),
            "{nt}"
        );
    }
    /// Scale check on a real model, when `OTS_IDS_SCALE_IFC` names one (a
    /// no-op otherwise, CI included): the projection must stay linear and
    /// stream through the sink.
    #[test]
    fn projects_a_real_model_when_one_is_named() {
        let Some(path) = std::env::var_os("OTS_IDS_SCALE_IFC") else {
            return;
        };
        let input = std::fs::read_to_string(&path).expect("readable IFC");
        let t0 = std::time::Instant::now();
        let file = parse(&input).expect("parses");
        let parsed = t0.elapsed();
        let mut bytes = 0usize;
        let mut chunks = 0usize;
        let stats = emit(
            &file,
            &ConvertOptions {
                inst_base: "http://ex.test/big/".into(),
                ..Default::default()
            },
            &mut |c| {
                bytes += c.len();
                chunks += 1;
            },
        )
        .expect("projects");
        eprintln!(
            "IDS projection of {}: {} instances, {} triples, {} property sets, {} MB in {chunks} chunks; parse {:?}, project {:?}",
            std::path::Path::new(&path).display(),
            stats.instances,
            stats.triples,
            stats.property_sets,
            bytes / (1024 * 1024),
            parsed,
            t0.elapsed() - parsed
        );
        assert_eq!(stats.instances, file.instances.len());
        assert!(stats.triples > stats.instances);
    }
}
