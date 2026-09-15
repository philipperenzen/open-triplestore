//! SHACL → buildingSMART IDS 1.0.
//!
//! The inverse of [`super::ids`], and deliberately a narrower claim than its
//! name suggests. IDS's whole expressive surface for a requirement is a facet
//! kind (entity / partOf / classification / attribute / property / material), a
//! cardinality of `required` | `prohibited` | `optional`, and one value
//! restriction. Most of SHACL has no IDS form at all: of the constraint
//! variants the engine models, roughly half can be expressed and the rest
//! cannot, so **every export reports what it dropped** and an export that could
//! express nothing is an error rather than a schema-valid but empty document.
//!
//! What this can honestly claim: it exports shape graphs written over *this
//! store's* IFC RDF vocabulary — the `props:`/`bot:` convention the IFC lift
//! emits and the IDS importer targets — back to IDS. It is not a general
//! SHACL-to-IDS translator, and shapes produced by other tools will mostly
//! land in the loss list.
//!
//! Round-tripping an imported document is the design target: import → export
//! → import reaches a fixpoint on the subset both directions share.

use std::fmt::Write as _;

use crate::shacl::shapes::{Constraint, PropertyPath, Shape, Target};
use oxigraph::model::Term;

use super::{ExportedSpec, SpecExporter};

pub struct IdsExporter;

impl SpecExporter for IdsExporter {
    fn id(&self) -> &'static str {
        "ids"
    }
    fn label(&self) -> &'static str {
        "buildingSMART IDS 1.0"
    }
    fn media_type(&self) -> &'static str {
        "application/xml"
    }
    fn file_extension(&self) -> &'static str {
        "ids"
    }
    fn export(&self, shapes: &[Shape], title: &str) -> anyhow::Result<ExportedSpec> {
        export(shapes, title)
    }
}

/// The IFC entity name for a `sh:targetClass` IRI: the local name, uppercased.
/// `ifc:IfcWall` → `IFCWALL`, the inverse of the importer's title-caser.
fn entity_name(class_iri: &str) -> String {
    class_iri
        .rsplit(['#', '/'])
        .next()
        .unwrap_or(class_iri)
        .to_ascii_uppercase()
}

/// The lexical form of a term, without its datatype or language tag — IDS
/// values are plain strings.
fn lexical(t: &Term) -> String {
    match t {
        Term::Literal(l) => l.value().to_string(),
        Term::NamedNode(n) => n.as_str().to_string(),
        other => other.to_string(),
    }
}

/// Which IDS facet a property path denotes, if any.
///
/// This is the exporter's real classifier, and it is entirely a property of
/// *this store's* IFC vocabulary — see the module note.
#[derive(Debug, PartialEq, Eq)]
enum Facet {
    /// `<ids:property>` with a property set and a base name.
    Property {
        pset: String,
        base: String,
    },
    /// `<ids:attribute>` with a name.
    Attribute(String),
    /// `<ids:partOf relation="…">`.
    PartOf(String),
    Classification,
    Material,
}

// The namespaces the IFC lift emits and the IDS importer targets. These must
// stay in step with `super::ids`: a mismatch sends every property and partOf
// facet to the loss list instead of into the document, silently.
const PROPS: &str = "https://w3id.org/props#";
const BOT: &str = "https://w3id.org/bot#";

fn local(iri: &str, ns: &str) -> Option<String> {
    iri.strip_prefix(ns).map(str::to_string)
}

/// `sh:name` carries the unsanitised `"Pset.Name"` the importer wrote, which
/// is the only faithful source: the path itself is sanitised and ambiguous
/// when a property-set name already contains an underscore.
fn classify(path: &PropertyPath, name: Option<&str>) -> Option<Facet> {
    match path {
        PropertyPath::Predicate(p) => {
            let l = local(p, PROPS)?;
            match l.as_str() {
                "ifcName" => Some(Facet::Attribute("Name".into())),
                "ifcGuid" => Some(Facet::Attribute("GlobalId".into())),
                "ifcClassification" => Some(Facet::Classification),
                "ifcMaterial" => Some(Facet::Material),
                other if other.starts_with("ifc") => Some(Facet::Attribute(
                    other.trim_start_matches("ifc").to_string(),
                )),
                _ => {
                    // A property-set property. Prefer `sh:name "Pset.Base"`.
                    let (pset, base) = match name.and_then(|n| n.split_once('.')) {
                        Some((p, b)) => (p.to_string(), b.to_string()),
                        None => {
                            let (p, b) = l.split_once('_')?;
                            (p.to_string(), b.to_string())
                        }
                    };
                    Some(Facet::Property { pset, base })
                }
            }
        }
        PropertyPath::Inverse(inner) => match inner.as_ref() {
            PropertyPath::Predicate(p) => match local(p, BOT)?.as_str() {
                "containsElement" => {
                    Some(Facet::PartOf("IFCRELCONTAINEDINSPATIALSTRUCTURE".into()))
                }
                "hasSubElement" => Some(Facet::PartOf("IFCRELAGGREGATES".into())),
                _ => None,
            },
            _ => None,
        },
        _ => None,
    }
}

/// XML-escape text content and attribute values.
fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// One facet rendered to XML, with the cardinality and value restriction it
/// carried, plus whatever had to be dropped.
struct RenderedFacet {
    /// Sort key: the XSD `xs:sequence` position, then the facet's own name.
    order: (u8, String),
    xml: String,
}

fn facet_order(f: &Facet) -> u8 {
    // The `applicabilityType`/`requirementsType` sequence order.
    match f {
        Facet::PartOf(_) => 1,
        Facet::Classification => 2,
        Facet::Attribute(_) => 3,
        Facet::Property { .. } => 4,
        Facet::Material => 5,
    }
}

/// Build the `<ids:value>` child from the value-restricting constraints, and
/// note the ones that cannot be expressed.
fn value_element(constraints: &[Constraint], losses: &mut Vec<String>, indent: &str) -> String {
    // `sh:hasValue` is a simple value and wins outright.
    for c in constraints {
        if let Constraint::HasValue(t) = c {
            return format!(
                "{indent}<ids:value><ids:simpleValue>{}</ids:simpleValue></ids:value>\n",
                esc(&lexical(t))
            );
        }
    }
    let mut facets = String::new();
    for c in constraints {
        match c {
            Constraint::In(terms) => {
                for t in terms {
                    let _ = writeln!(
                        facets,
                        "{indent}    <xs:enumeration value=\"{}\"/>",
                        esc(&lexical(t))
                    );
                }
            }
            Constraint::MinInclusive(t) => {
                let _ = writeln!(
                    facets,
                    "{indent}    <xs:minInclusive value=\"{}\"/>",
                    esc(&lexical(t))
                );
            }
            Constraint::MaxInclusive(t) => {
                let _ = writeln!(
                    facets,
                    "{indent}    <xs:maxInclusive value=\"{}\"/>",
                    esc(&lexical(t))
                );
            }
            Constraint::MinExclusive(t) => {
                let _ = writeln!(
                    facets,
                    "{indent}    <xs:minExclusive value=\"{}\"/>",
                    esc(&lexical(t))
                );
            }
            Constraint::MaxExclusive(t) => {
                let _ = writeln!(
                    facets,
                    "{indent}    <xs:maxExclusive value=\"{}\"/>",
                    esc(&lexical(t))
                );
            }
            Constraint::MinLength(n) => {
                let _ = writeln!(facets, "{indent}    <xs:minLength value=\"{n}\"/>\n");
            }
            Constraint::MaxLength(n) => {
                let _ = writeln!(facets, "{indent}    <xs:maxLength value=\"{n}\"/>\n");
            }
            Constraint::Pattern { pattern, flags } => {
                // `xs:pattern` is implicitly anchored and has no flags, while
                // `sh:pattern` is an XPath/SPARQL regex with optional flags.
                // Exporting a flagged pattern would change its meaning.
                if flags.is_some() {
                    losses.push(format!(
                        "sh:pattern `{pattern}` carries sh:flags, which xs:pattern has no form for — dropped"
                    ));
                } else {
                    losses.push(format!(
                        "sh:pattern `{pattern}` exported as xs:pattern, which is implicitly anchored — the match semantics differ"
                    ));
                    let _ = writeln!(
                        facets,
                        "{indent}    <xs:pattern value=\"{}\"/>",
                        esc(pattern)
                    );
                }
            }
            _ => {}
        }
    }
    if facets.is_empty() {
        return String::new();
    }
    format!(
        "{indent}<ids:value>\n{indent}  <xs:restriction base=\"xs:string\">\n{facets}{indent}  </xs:restriction>\n{indent}</ids:value>\n"
    )
}

/// `cardinality` from the counting constraints. IDS carries no multiplicity,
/// so only the two degenerate cases map.
fn cardinality(
    constraints: &[Constraint],
    losses: &mut Vec<String>,
    what: &str,
) -> Option<&'static str> {
    let mut card = None;
    for c in constraints {
        match c {
            Constraint::MaxCount(0) => card = Some("prohibited"),
            Constraint::MinCount(1) => {
                if card != Some("prohibited") {
                    card = Some("required")
                }
            }
            Constraint::MinCount(n) => losses.push(format!(
                "{what}: sh:minCount {n} has no IDS form (IDS carries no multiplicity) — dropped"
            )),
            Constraint::MaxCount(n) => losses.push(format!(
                "{what}: sh:maxCount {n} has no IDS form (only 0, as `prohibited`) — dropped"
            )),
            _ => {}
        }
    }
    card
}

/// Every constraint variant with no IDS expression at all, named so the caller
/// can report it rather than silently dropping it.
fn note_unexportable(c: &Constraint, what: &str, losses: &mut Vec<String>) {
    let name = match c {
        Constraint::NodeKind(_) => "sh:nodeKind",
        Constraint::LanguageIn(_) => "sh:languageIn",
        Constraint::UniqueLang(_) => "sh:uniqueLang",
        Constraint::Equals(_) => "sh:equals",
        Constraint::Disjoint(_) => "sh:disjoint",
        Constraint::LessThan(_) => "sh:lessThan",
        Constraint::LessThanOrEquals(_) => "sh:lessThanOrEquals",
        Constraint::Xone(_) => "sh:xone",
        Constraint::Node(_) => "sh:node",
        Constraint::Property(_) => "a nested sh:property",
        Constraint::QualifiedValueShape { .. } => "sh:qualifiedValueShape",
        Constraint::Closed { .. } => "sh:closed",
        Constraint::SparqlConstraint { .. } => "sh:sparql",
        Constraint::Custom(_) => "a custom constraint component",
        Constraint::Expression { .. } => "sh:expression",
        _ => return,
    };
    losses.push(format!("{what}: {name} has no IDS facet — dropped"));
}

/// Render one property shape as a facet, or record why it could not be.
fn render_facet(
    ps: &crate::shacl::shapes::PropertyShape,
    requirement: bool,
    losses: &mut Vec<String>,
) -> Option<RenderedFacet> {
    let Some(facet) = classify(&ps.path, ps.name.as_deref()) else {
        losses.push(format!(
            "path `{}` is not part of the IFC RDF vocabulary this exporter understands — the whole facet was dropped",
            ps.path.to_sparql()
        ));
        return None;
    };
    let what = format!("path `{}`", ps.path.to_sparql());
    for c in &ps.constraints {
        note_unexportable(c, &what, losses);
    }
    let card = cardinality(&ps.constraints, losses, &what);
    let card_attr = match (requirement, card) {
        (true, Some(c)) => format!(" cardinality=\"{c}\""),
        // In the applicability position IDS has no cardinality attribute.
        _ => String::new(),
    };
    let value = value_element(&ps.constraints, losses, "        ");

    let (tag, body, key) = match &facet {
        Facet::Property { pset, base } => (
            "property".to_string(),
            format!(
                "        <ids:propertySet><ids:simpleValue>{}</ids:simpleValue></ids:propertySet>\n\
                         <ids:baseName><ids:simpleValue>{}</ids:simpleValue></ids:baseName>\n{value}",
                esc(pset),
                esc(base)
            ),
            format!("{pset}.{base}"),
        ),
        Facet::Attribute(n) => (
            "attribute".to_string(),
            format!(
                "        <ids:name><ids:simpleValue>{}</ids:simpleValue></ids:name>\n{value}",
                esc(n)
            ),
            n.clone(),
        ),
        Facet::PartOf(rel) => {
            // The nested entity comes from an `sh:class` on the same shape.
            let entity = ps.constraints.iter().find_map(|c| match c {
                Constraint::Class(c) => Some(entity_name(c)),
                _ => None,
            });
            let Some(entity) = entity else {
                losses.push(format!(
                    "{what}: a partOf facet needs an sh:class naming the containing entity — dropped"
                ));
                return None;
            };
            (
                format!("partOf relation=\"{}\"", esc(rel)),
                format!(
                    "        <ids:entity><ids:name><ids:simpleValue>{}</ids:simpleValue></ids:name></ids:entity>\n",
                    esc(&entity)
                ),
                rel.clone(),
            )
        }
        Facet::Classification => {
            // `ids:classificationType/system` is mandatory, and the importer
            // kept it only as free text. Synthesising one would emit a
            // document that lies, so the facet is dropped instead.
            losses.push(format!(
                "{what}: a classification facet needs its `system`, which is not recoverable from the shape — dropped"
            ));
            return None;
        }
        Facet::Material => (
            "material".to_string(),
            value.clone(),
            "material".to_string(),
        ),
    };
    // Close with the bare element name: the open tag may carry attributes.
    let close = tag.split_whitespace().next().unwrap_or(&tag).to_string();
    let xml = format!("      <ids:{tag}{card_attr}>\n{body}      </ids:{close}>\n");
    Some(RenderedFacet {
        order: (facet_order(&facet), key),
        xml,
    })
}

/// Is this a helper shape the importer generates (`…-applies` / `…-requires`)?
/// `load_shapes` returns them as top-level shapes because they carry
/// `sh:property`, and exporting them would triple the specification count.
fn is_helper(iri: &str) -> bool {
    iri.ends_with("-applies") || iri.ends_with("-requires")
}

fn target_classes(shape: &Shape) -> Vec<String> {
    shape
        .targets
        .iter()
        .filter_map(|t| match t {
            Target::TargetClass(c) => Some(entity_name(c)),
            _ => None,
        })
        .collect()
}

/// Export `shapes` as an IDS 1.0 document.
pub fn export(shapes: &[Shape], title: &str) -> anyhow::Result<ExportedSpec> {
    let mut losses: Vec<String> = Vec::new();
    let mut specs = String::new();
    let mut count = 0usize;

    let by_iri: std::collections::HashMap<&str, &Shape> =
        shapes.iter().map(|s| (s.iri.as_str(), s)).collect();

    for shape in shapes {
        if is_helper(&shape.iri) {
            continue;
        }
        let classes = target_classes(shape);
        if classes.is_empty() {
            for t in &shape.targets {
                let kind = match t {
                    Target::TargetNode(_) => "sh:targetNode",
                    Target::TargetSubjectsOf(_) => "sh:targetSubjectsOf",
                    Target::TargetObjectsOf(_) => "sh:targetObjectsOf",
                    Target::SparqlTarget(_) => "a SPARQL target",
                    Target::TargetClass(_) => continue,
                };
                losses.push(format!(
                    "shape <{}>: IDS applicability is class-based, so {kind} cannot be expressed — the shape was not exported",
                    shape.iri
                ));
            }
            if shape.targets.is_empty() {
                losses.push(format!(
                    "shape <{}> has no sh:targetClass, so it cannot become an IDS specification — not exported",
                    shape.iri
                ));
            }
            continue;
        }

        // `sh:name` is what the importer wrote the specification name into.
        let name = shape.name.clone().unwrap_or_else(|| {
            shape
                .iri
                .rsplit(['#', '/'])
                .next()
                .unwrap_or("")
                .to_string()
        });

        // The importer's implication idiom: sh:or ( [ sh:not <X-applies> ] <X-requires> ).
        // Recover the applicability and requirement halves from it when present.
        let mut applies: Vec<&crate::shacl::shapes::PropertyShape> = Vec::new();
        let mut requires: Vec<&crate::shacl::shapes::PropertyShape> = Vec::new();
        let mut prohibited = false;

        for c in &shape.constraints {
            match c {
                Constraint::Not(inner)
                    if inner
                        .constraints
                        .iter()
                        .any(|c| matches!(c, Constraint::Class(_))) =>
                {
                    prohibited = true;
                }
                Constraint::Or(members) if members.len() == 2 => {
                    if let Some(Constraint::Not(applies_shape)) = members[0].constraints.first() {
                        let a = by_iri
                            .get(applies_shape.iri.as_str())
                            .copied()
                            .unwrap_or(applies_shape.as_ref());
                        applies.extend(a.property_shapes.iter());
                        let r = by_iri
                            .get(members[1].iri.as_str())
                            .copied()
                            .unwrap_or(&members[1]);
                        requires.extend(r.property_shapes.iter());
                    }
                }
                other => note_unexportable(other, &format!("shape <{}>", shape.iri), &mut losses),
            }
        }
        if applies.is_empty() && requires.is_empty() {
            requires.extend(shape.property_shapes.iter());
        }

        let mut app_facets: Vec<RenderedFacet> = applies
            .iter()
            .filter_map(|ps| render_facet(ps, false, &mut losses))
            .collect();
        app_facets.sort_by(|a, b| a.order.cmp(&b.order));
        let mut req_facets: Vec<RenderedFacet> = requires
            .iter()
            .filter_map(|ps| render_facet(ps, true, &mut losses))
            .collect();
        req_facets.sort_by(|a, b| a.order.cmp(&b.order));

        let occurs = if prohibited {
            " minOccurs=\"0\" maxOccurs=\"0\""
        } else {
            ""
        };
        let entity = if classes.len() == 1 {
            format!(
                "      <ids:entity><ids:name><ids:simpleValue>{}</ids:simpleValue></ids:name></ids:entity>\n",
                esc(&classes[0])
            )
        } else {
            let enums: String = classes
                .iter()
                .map(|c| format!("            <xs:enumeration value=\"{}\"/>", esc(c)))
                .collect();
            format!(
                "      <ids:entity>\n        <ids:name>\n          <xs:restriction base=\"xs:string\">\n{enums}          </xs:restriction>\n        </ids:name>\n      </ids:entity>\n"
            )
        };

        let _ = write!(
            specs,
            "    <ids:specification name=\"{}\" ifcVersion=\"IFC4\">\n      <ids:applicability{occurs}>\n{entity}{}      </ids:applicability>\n",
            esc(&name),
            app_facets.iter().map(|f| f.xml.as_str()).collect::<String>()
        );
        if !req_facets.is_empty() {
            let _ = write!(
                specs,
                "      <ids:requirements>\n{}      </ids:requirements>\n",
                req_facets
                    .iter()
                    .map(|f| f.xml.as_str())
                    .collect::<String>()
            );
        }
        specs.push_str("    </ids:specification>\n");
        count += 1;
    }

    if count == 0 {
        anyhow::bail!(
            "no shape could be expressed as an IDS specification: {}",
            if losses.is_empty() {
                "the shape graph is empty".to_string()
            } else {
                losses.join("; ")
            }
        );
    }

    let document = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <ids:ids xmlns:ids=\"http://standards.buildingsmart.org/IDS\" \
         xmlns:xs=\"http://www.w3.org/2001/XMLSchema\" \
         xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\">\n\
         \x20 <ids:info>\n    <ids:title>{}</ids:title>\n  </ids:info>\n\
         \x20 <ids:specifications>\n{specs}  </ids:specifications>\n\
         </ids:ids>\n",
        esc(title)
    );

    Ok(ExportedSpec {
        document,
        specification_count: count,
        losses,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shacl::engine::load_shapes;
    use crate::store::TripleStore;
    use oxigraph::io::RdfFormat;

    fn shapes_of(ttl: &str) -> Vec<Shape> {
        let store = TripleStore::in_memory().unwrap();
        store
            .load_str(ttl, RdfFormat::Turtle, Some("urn:shapes"))
            .unwrap();
        load_shapes(&store, "urn:shapes").unwrap()
    }

    /// The importer's own sample round-trips: import → export puts the entity,
    /// the applicability property and the requirement back.
    #[test]
    fn the_importers_own_sample_exports_its_specifications_back() {
        let imported = super::super::ids::convert(
            &super::super::ids::parse_xml(super::super::ids::tests_sample().as_bytes()).unwrap(),
        )
        .unwrap();
        let shapes = shapes_of(&imported.turtle);
        let out = export(&shapes, "Wall fire ratings").expect("exports");
        assert!(
            out.specification_count >= 2,
            "the sample has two specifications: {}",
            out.document
        );
        assert!(out.document.contains("IFCWALL"), "{}", out.document);
        assert!(
            out.document.contains("Pset_WallCommon"),
            "the property set survives: {}",
            out.document
        );
        assert!(
            out.document.contains("IFCRELCONTAINEDINSPATIALSTRUCTURE"),
            "the partOf relation survives: {}",
            out.document
        );
        // The helper shapes must not become specifications of their own.
        assert!(
            !out.document.contains("-applies"),
            "helper shapes are not specifications: {}",
            out.document
        );
    }

    /// A shape with no `sh:targetClass` cannot be a specification, and an
    /// export that can express nothing is an error rather than an empty but
    /// schema-valid document.
    #[test]
    fn a_shape_graph_with_nothing_expressible_is_an_error() {
        let shapes = shapes_of(
            "@prefix sh: <http://www.w3.org/ns/shacl#> .\n\
             @prefix ex: <http://example.org/> .\n\
             ex:S a sh:NodeShape ; sh:targetNode ex:a ; sh:property [ sh:path ex:p ; sh:minCount 1 ] .",
        );
        let err = export(&shapes, "nothing").expect_err("must refuse");
        assert!(
            err.to_string().contains("class-based"),
            "the reason is reported: {err}"
        );
    }

    /// Constraints outside the IDS facet model are reported, never silently
    /// dropped — the honesty budget this exporter is built around.
    #[test]
    fn unexportable_constraints_are_reported_as_losses() {
        let shapes = shapes_of(
            "@prefix sh: <http://www.w3.org/ns/shacl#> .\n\
             @prefix ifc: <https://standards.buildingsmart.org/IFC/DEV/IFC4/ADD2_TC1/OWL#> .\n\
             @prefix props: <https://w3id.org/props#> .\n\
             @prefix ex: <http://example.org/> .\n\
             ex:S a sh:NodeShape ; sh:targetClass ifc:IfcWall ;\n\
               sh:property [ sh:path props:Pset_WallCommon_FireRating ; sh:name \"Pset_WallCommon.FireRating\" ;\n\
                             sh:minCount 1 ; sh:nodeKind sh:Literal ; sh:uniqueLang true ] ;\n\
               sh:property [ sh:path ex:notIfc ; sh:minCount 1 ] .",
        );
        let out = export(&shapes, "losses").expect("exports the part it can");
        assert!(out.document.contains("Pset_WallCommon"), "{}", out.document);
        let joined = out.losses.join("\n");
        assert!(joined.contains("sh:nodeKind"), "{joined}");
        assert!(joined.contains("sh:uniqueLang"), "{joined}");
        assert!(
            joined.contains("not part of the IFC RDF vocabulary"),
            "a foreign path is reported, not dropped in silence: {joined}"
        );
    }
}
