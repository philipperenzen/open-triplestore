//! RDF emission for parsed IFC files — see the module docs in [`super`].

use std::collections::{HashMap, HashSet};

use super::names;
use super::step::{decode_ifc_guid, Arg, Instance, StepFile};
use super::{ConvertOptions, IfcStats};

const BOT: &str = "https://w3id.org/bot#";
const PROPS: &str = "https://w3id.org/props#";
const OMG: &str = "https://w3id.org/omg#";
const FOG: &str = "https://w3id.org/fog#";
const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const RDFS_LABEL: &str = "http://www.w3.org/2000/01/rdf-schema#label";
const XSD: &str = "http://www.w3.org/2001/XMLSchema#";
const GEO: &str = "http://www.opengis.net/ont/geosparql#";

// Chunk size for the N-Triples sinks. Each chunk costs a store load round-trip
// (which rebuilds the graph index), so bigger chunks load multi-million-triple
// lifts far faster; 32 MB keeps peak memory modest while cutting round-trips ~8×.
const FLUSH_AT: usize = 32 * 1024 * 1024;

/// ifcOWL namespace for a FILE_SCHEMA id.
fn ifcowl_ns(schema: &str) -> &'static str {
    if schema.starts_with("IFC4") {
        "https://standards.buildingsmart.org/IFC/DEV/IFC4/ADD2_TC1/OWL#"
    } else {
        "https://standards.buildingsmart.org/IFC/DEV/IFC2x3/TC1/OWL#"
    }
}

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

pub fn emit(
    file: &StepFile,
    opts: &ConvertOptions,
    bot_out: &mut dyn FnMut(&str),
    ifcowl_out: &mut dyn FnMut(&str),
) -> Result<IfcStats, String> {
    let base = &opts.inst_base;
    let ifc_ns = ifcowl_ns(&file.schema);
    let mut stats = IfcStats {
        schema: file.schema.clone(),
        instances: file.instances.len(),
        ..Default::default()
    };

    let spatial_class: HashMap<&str, &str> = names::SPATIAL_BOT.iter().copied().collect();

    // ── Containment & aggregation edges ────────────────────────────────────────
    // (parent, child) pairs from the two structural relationship entities.
    let mut contains: Vec<(u64, u64)> = Vec::new(); // spatial → element
    let mut aggregates: Vec<(u64, u64)> = Vec::new(); // object → sub-object
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
            _ => {}
        }
    }

    // Everything reachable as a child of containment/aggregation that is not
    // itself spatial counts as an element for the BOT layer.
    let mut element_ids: HashSet<u64> = HashSet::new();
    for &(_, k) in contains.iter().chain(aggregates.iter()) {
        if let Some(inst) = file.get(k) {
            if !spatial_class.contains_key(inst.entity.as_str()) && inst.entity != "IFCPROJECT" {
                element_ids.insert(k);
            }
        }
    }

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

    let emit_node =
        |bot: &mut NtSink, inst: &Instance, bot_class: Option<&str>, is_element: bool| {
            let s = inst_iri(base, inst);
            if let Some(cls) = bot_class {
                bot.triple(&s, &iri(RDF_TYPE), &iri(&format!("{BOT}{cls}")));
            }
            if is_element {
                bot.triple(&s, &iri(RDF_TYPE), &iri(&format!("{BOT}Element")));
            }
            bot.triple(
                &s,
                &iri(RDF_TYPE),
                &iri(&format!("{ifc_ns}{}", names::camel(&inst.entity))),
            );
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
                    let node = format!("{}/filelink>", s.trim_end_matches('>'));
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
                }
            }
        };

    // Spatial structure nodes.
    for inst in file.instances.values() {
        if let Some(cls) = spatial_class.get(inst.entity.as_str()) {
            emit_node(&mut bot, inst, Some(cls), false);
            match inst.entity.as_str() {
                "IFCBUILDINGSTOREY" => stats.storeys += 1,
                "IFCSPACE" => stats.spaces += 1,
                _ => {}
            }
        }
    }
    // Element nodes.
    for &id in &element_ids {
        if let Some(inst) = file.get(id) {
            emit_node(&mut bot, inst, None, true);
        }
    }
    stats.elements = element_ids.len();

    // Containment edges. Spatial→spatial aggregation uses the canonical BOT
    // predicate AND bot:containsElement so the viewer feed (which walks
    // containsElement|hasSubElement) sees the full tree.
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
        if let Some(pred) = canonical(&pi.entity, &ki.entity) {
            bot.triple(&ps, &iri(&format!("{BOT}{pred}")), &ks);
            bot.triple(&ps, &iri(&format!("{BOT}containsElement")), &ks);
        } else if element_ids.contains(&k) && element_ids.contains(&p) {
            bot.triple(&ps, &iri(&format!("{BOT}hasSubElement")), &ks);
        } else {
            bot.triple(&ps, &iri(&format!("{BOT}containsElement")), &ks);
        }
    }
    for &(p, k) in &contains {
        let (Some(pi), Some(ki)) = (file.get(p), file.get(k)) else {
            continue;
        };
        bot.triple(
            &inst_iri(base, pi),
            &iri(&format!("{BOT}containsElement")),
            &inst_iri(base, ki),
        );
    }

    // Anchor geometry on the site/building so the map can place the model —
    // the caller's anchor wins, else the file's own IfcSite georeference.
    let anchor_wkt = opts.anchor_wkt.clone().or_else(|| site_anchor_wkt(file));
    if let (Some(anchor_id), Some(wkt)) = (site_or_building_anchor, &anchor_wkt) {
        if let Some(inst) = file.get(anchor_id) {
            let s = inst_iri(base, inst);
            // Stable IRI, not a blank node — see the FOG node comment above.
            let b = format!("{}/anchor>", s.trim_end_matches('>'));
            bot.triple(&s, &iri(&format!("{GEO}hasGeometry")), &b);
            bot.triple(&b, &iri(RDF_TYPE), &iri(&format!("{GEO}Geometry")));
            bot.triple(
                &b,
                &iri(&format!("{GEO}asWKT")),
                &format!("{}^^<{GEO}wktLiteral>", lit(wkt)),
            );
        }
    }

    // Provenance on the root: where this model came from, its license and the
    // attribution line — real open BIM datasets (Schependomlaan, the KIT
    // models) require credit, and the root element is where viewers look.
    if let Some(inst) = root_id.and_then(|id| file.get(id)) {
        let s = inst_iri(base, inst);
        const DCT: &str = "http://purl.org/dc/terms/";
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

    // Property sets → direct data properties props:{Pset}_{Prop} on the object.
    for inst in file.instances.values() {
        if inst.entity != "IFCRELDEFINESBYPROPERTIES" {
            continue;
        }
        let Some(objs) = inst.args.get(4).and_then(Arg::as_list) else {
            continue;
        };
        let Some(pset_id) = inst.args.get(5).and_then(Arg::as_ref_id) else {
            continue;
        };
        let Some(pset) = file.get(pset_id) else {
            continue;
        };
        if pset.entity != "IFCPROPERTYSET" {
            continue;
        }
        let pset_name = sanitize(name_arg(pset, 2).unwrap_or("Pset"));
        let Some(props) = pset.args.get(4).and_then(Arg::as_list) else {
            continue;
        };
        for obj in objs.iter().filter_map(Arg::as_ref_id) {
            let Some(obj_inst) = file.get(obj) else {
                continue;
            };
            // Only attach to nodes that exist in the BOT layer.
            if !element_ids.contains(&obj) && !spatial_class.contains_key(obj_inst.entity.as_str())
            {
                continue;
            }
            let s = inst_iri(base, obj_inst);
            for prop in props.iter().filter_map(Arg::as_ref_id) {
                let Some(p) = file.get(prop) else { continue };
                if p.entity != "IFCPROPERTYSINGLEVALUE" {
                    continue;
                }
                let Some(pname) = name_arg(p, 0) else {
                    continue;
                };
                let Some(value) = p.args.get(2) else { continue };
                if let Some(obj_nt) = arg_to_literal(value) {
                    let pred = format!("{PROPS}{}_{}", pset_name, sanitize(pname));
                    bot.triple(&s, &iri(&pred), &obj_nt);
                }
            }
        }
    }

    stats.bot_triples = bot.finish();

    // ── ifcOWL layer ───────────────────────────────────────────────────────────
    if opts.include_ifcowl {
        let mut owl = NtSink::new(ifcowl_out);
        for inst in file.instances.values() {
            let s = inst_iri(base, inst);
            owl.triple(
                &s,
                &iri(RDF_TYPE),
                &iri(&format!("{ifc_ns}{}", names::camel(&inst.entity))),
            );
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
    fn bot_layer_has_topology_labels_guids_props_and_anchor() {
        let (bot, _, stats) = run(false);
        assert_eq!(stats.schema, "IFC2X3");
        assert_eq!(stats.storeys, 1);
        assert_eq!(stats.elements, 1, "the beam");
        // Topology spine: site→building→storey (canonical + feed edges) and
        // storey→beam containment.
        assert!(bot.contains("https://w3id.org/bot#hasBuilding"));
        assert!(bot.contains("https://w3id.org/bot#hasStorey"));
        assert!(bot.contains(&format!(
            "<http://ex.test/m/0AAAAAAAAAAAAAAAAAAAA4> <https://w3id.org/bot#containsElement> <http://ex.test/m/0AAAAAAAAAAAAAAAAAAAA5>"
        )));
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
