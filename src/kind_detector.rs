//! Classify uploaded RDF by logical graph role.
//!
//! Single-pass scan over parsed quads. Each quad is examined to tally evidence
//! signals, then a heuristic maps the tallies to one of five `RegistryKind`
//! values that correspond 1-to-1 with [`GraphKind`].

use oxigraph::model::{Quad, Term};
use serde::{Deserialize, Serialize};

use crate::auth::models::GraphKind;

// ─── Namespace constants ──────────────────────────────────────────────────────

const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";

const OWL_ONTOLOGY: &str = "http://www.w3.org/2002/07/owl#Ontology";
const OWL_CLASS: &str = "http://www.w3.org/2002/07/owl#Class";
const OWL_OBJECT_PROPERTY: &str = "http://www.w3.org/2002/07/owl#ObjectProperty";
const OWL_DATATYPE_PROPERTY: &str = "http://www.w3.org/2002/07/owl#DatatypeProperty";
const OWL_ANNOTATION_PROPERTY: &str = "http://www.w3.org/2002/07/owl#AnnotationProperty";
const RDF_PROPERTY: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#Property";
const RDFS_CLASS: &str = "http://www.w3.org/2000/01/rdf-schema#Class";

const SH_NODE_SHAPE: &str = "http://www.w3.org/ns/shacl#NodeShape";
const SH_PROPERTY_SHAPE: &str = "http://www.w3.org/ns/shacl#PropertyShape";
const SH_TARGET_CLASS: &str = "http://www.w3.org/ns/shacl#targetClass";
const SH_NS: &str = "http://www.w3.org/ns/shacl#";

const SKOS_CONCEPT_SCHEME: &str = "http://www.w3.org/2004/02/skos/core#ConceptScheme";
const SKOS_CONCEPT: &str = "http://www.w3.org/2004/02/skos/core#Concept";
const SKOS_NS: &str = "http://www.w3.org/2004/02/skos/core#";

const SWRL_IMP: &str = "http://www.w3.org/2003/11/swrl#Imp";
const SPIN_RULE: &str = "http://spinrdf.org/spin#rule";
const SP_NS: &str = "http://spinrdf.org/sp#";

// Property (R-Box) axiom predicates — when seen as a predicate, the quad is
// Vocabulary content even without an rdf:type on the subject in the same quad.
const R_BOX_PREDICATES: &[&str] = &[
    "http://www.w3.org/2000/01/rdf-schema#domain",
    "http://www.w3.org/2000/01/rdf-schema#range",
    "http://www.w3.org/2000/01/rdf-schema#subPropertyOf",
    "http://www.w3.org/2002/07/owl#inverseOf",
    "http://www.w3.org/2002/07/owl#propertyChainAxiom",
    "http://www.w3.org/2002/07/owl#equivalentProperty",
];

// Class (T-Box) axiom predicates — the quad is Model content.
const T_BOX_PREDICATES: &[&str] = &[
    "http://www.w3.org/2000/01/rdf-schema#subClassOf",
    "http://www.w3.org/2002/07/owl#equivalentClass",
    "http://www.w3.org/2002/07/owl#disjointWith",
];

// IRIs whose objects are "schema-namespace" types when used as rdf:type objects
// (so subjects typed with one of these are schema resources, not instance data).
const SCHEMA_TYPE_OBJECTS: &[&str] = &[
    OWL_ONTOLOGY,
    OWL_CLASS,
    OWL_OBJECT_PROPERTY,
    OWL_DATATYPE_PROPERTY,
    OWL_ANNOTATION_PROPERTY,
    RDF_PROPERTY,
    RDFS_CLASS,
    SH_NODE_SHAPE,
    SH_PROPERTY_SHAPE,
    SKOS_CONCEPT_SCHEME,
    SKOS_CONCEPT,
    SWRL_IMP,
    "http://www.w3.org/2002/07/owl#NamedIndividual",
    "http://www.w3.org/2002/07/owl#AllDifferent",
    "http://www.w3.org/2002/07/owl#Restriction",
    "http://www.w3.org/2002/07/owl#FunctionalProperty",
    "http://www.w3.org/2002/07/owl#TransitiveProperty",
    "http://www.w3.org/2002/07/owl#SymmetricProperty",
];

// ─── Public types ─────────────────────────────────────────────────────────────

/// Detected logical graph role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RegistryKind {
    /// Model (T-Box): class definitions and class axioms (`owl:Class`,
    /// `rdfs:subClassOf`, restrictions). Dominant class signal.
    DataModel,
    /// Vocabulary (R-Box): object/datatype/annotation properties and relations,
    /// plus SKOS concept schemes / controlled vocabularies. The terms that
    /// describe how things relate, as opposed to the class structure.
    Vocabulary,
    /// SHACL shapes graph (no significant class definitions).
    Shapes,
    /// Entailment / rule set (SWRL, SPIN).
    Entailment,
    /// Instance data (A-Box).
    Instances,
}

impl RegistryKind {
    /// Map this kind to the equivalent [`GraphKind`].
    pub fn to_graph_role(self) -> GraphKind {
        match self {
            RegistryKind::DataModel => GraphKind::Model,
            RegistryKind::Vocabulary => GraphKind::Vocabulary,
            RegistryKind::Shapes => GraphKind::Shapes,
            RegistryKind::Entailment => GraphKind::Entailment,
            RegistryKind::Instances => GraphKind::Instances,
        }
    }

    /// Stable kebab-case string used to persist the kind in the registry
    /// (`ver:kind`). Matches the serde representation.
    pub fn as_str(self) -> &'static str {
        match self {
            RegistryKind::DataModel => "data-model",
            RegistryKind::Vocabulary => "vocabulary",
            RegistryKind::Shapes => "shapes",
            RegistryKind::Entailment => "entailment",
            RegistryKind::Instances => "instances",
        }
    }

    /// Parse a persisted kind string back into a [`RegistryKind`], tolerating the
    /// same aliases as [`parse_kind_override`]. Unknown values fall back to
    /// [`RegistryKind::DataModel`].
    pub fn from_persisted(s: &str) -> Self {
        parse_kind_override(s).unwrap_or(RegistryKind::DataModel)
    }
}

/// A registry entry whose kind has never been recorded is treated as a plain
/// data model (the original, pre-merge default).
impl Default for RegistryKind {
    fn default() -> Self {
        RegistryKind::DataModel
    }
}

/// Evidence tallies from a single-pass quad scan.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Evidence {
    pub owl_ontology: usize,
    pub owl_classes: usize,
    pub owl_properties: usize,
    pub rdfs_classes: usize,
    /// Subjects with an explicit sh:NodeShape / sh:PropertyShape type,
    /// or predicates in the sh: namespace (implicit shape).
    pub shacl_shapes: usize,
    pub skos_concept_schemes: usize,
    pub skos_concepts: usize,
    /// SWRL / SPIN entailment rules.
    pub entailment_rules: usize,
    /// Subjects typed with a non-schema class (heuristic instance count).
    pub instance_subjects: usize,
}

impl Evidence {
    /// Model score (T-Box): class definitions and class axioms. An `owl:Ontology`
    /// header counts here but does not, on its own, make a graph "have classes"
    /// (see [`Evidence::has_real_classes`]).
    fn class_score(&self) -> usize {
        self.owl_ontology + self.owl_classes + self.rdfs_classes
    }

    /// Vocabulary score (R-Box): object/datatype/annotation properties and
    /// relations, plus SKOS concept schemes and concepts. This is where the
    /// "vocabulary" of a model — the terms that describe how things relate —
    /// lives, as opposed to the class structure (Model) or the data (Instances).
    fn vocabulary_score(&self) -> usize {
        self.owl_properties + self.skos_concept_schemes + self.skos_concepts
    }

    /// True when the graph declares at least one real class (not merely an
    /// `owl:Ontology` header). Used to decide whether a property/SKOS-heavy graph
    /// is a pure Vocabulary (no class anchor) or a mixed Model.
    fn has_real_classes(&self) -> bool {
        self.owl_classes + self.rdfs_classes > 0
    }

    fn shapes_score(&self) -> usize {
        self.shacl_shapes
    }

    fn entailment_score(&self) -> usize {
        self.entailment_rules
    }

    fn abox_score(&self) -> usize {
        self.instance_subjects
    }
}

/// Result of a [`detect`] call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Detected {
    pub primary: Option<RegistryKind>,
    pub mixed: bool,
    pub evidence: Evidence,
}

impl Detected {
    /// Convert the detected primary kind to a [`GraphKind`], if known.
    pub fn to_graph_role(&self) -> Option<GraphKind> {
        Some(self.primary?.to_graph_role())
    }

    /// True when the scan saw any SHACL shape signal, regardless of the
    /// file-level `primary` verdict (mixed OWL+SHACL files classify as
    /// [`RegistryKind::DataModel`], since the shapes verdict requires
    /// `tbox == 0`). Exposed for tests that assert embedded-shape detection
    /// independently of the `primary` verdict.
    #[cfg(test)]
    pub fn has_shapes(&self) -> bool {
        self.evidence.shacl_shapes > 0
    }
}

// ─── Detection ────────────────────────────────────────────────────────────────

/// Scan parsed quads and classify them into a [`Detected`] result.
pub fn detect(quads: &[Quad]) -> Detected {
    let mut ev = Evidence::default();

    for q in quads {
        let p = q.predicate.as_str();

        if p == RDF_TYPE {
            if let Term::NamedNode(obj) = &q.object {
                let obj_str = obj.as_str();
                match obj_str {
                    OWL_ONTOLOGY => ev.owl_ontology += 1,
                    OWL_CLASS => ev.owl_classes += 1,
                    OWL_OBJECT_PROPERTY
                    | OWL_DATATYPE_PROPERTY
                    | OWL_ANNOTATION_PROPERTY
                    | RDF_PROPERTY => ev.owl_properties += 1,
                    RDFS_CLASS => ev.rdfs_classes += 1,
                    SH_NODE_SHAPE | SH_PROPERTY_SHAPE => ev.shacl_shapes += 1,
                    SKOS_CONCEPT_SCHEME => ev.skos_concept_schemes += 1,
                    SKOS_CONCEPT => ev.skos_concepts += 1,
                    SWRL_IMP => ev.entailment_rules += 1,
                    _ => {
                        // Subject is typed with something that isn't a schema construct —
                        // count as potential instance (deduplicated by subject below
                        // would be ideal but single-pass approximation is sufficient).
                        if !SCHEMA_TYPE_OBJECTS.contains(&obj_str) {
                            ev.instance_subjects += 1;
                        }
                    }
                }
            }
        } else if p == SH_TARGET_CLASS {
            // Implicit shape signal: subject is a shape even without rdf:type.
            ev.shacl_shapes += 1;
        } else if p.starts_with(SH_NS) {
            // Any predicate in the sh: namespace is a strong SHACL indicator.
            ev.shacl_shapes += 1;
        } else if p == SPIN_RULE || p.starts_with(SP_NS) {
            ev.entailment_rules += 1;
        } else if p.starts_with(SKOS_NS) {
            // Predicates like skos:broader, skos:narrower imply vocabulary content.
            ev.skos_concepts += 1;
        }
    }

    classify(ev)
}

fn classify(ev: Evidence) -> Detected {
    // The three logical layers plus the two orthogonal roles:
    //   class  → Model (T-Box: classes / class axioms)
    //   vocab  → Vocabulary (R-Box: properties / relations + SKOS concept schemes)
    //   abox   → Instances (A-Box: real data)
    //   shapes → Shapes (SHACL)   entail → Entailment (SWRL/SPIN)
    let class = ev.class_score();
    let vocab = ev.vocabulary_score();
    let shapes = ev.shapes_score();
    let entail = ev.entailment_score();
    let abox = ev.abox_score();
    let has_classes = ev.has_real_classes();

    let total_schema = class + shapes + vocab + entail;

    if total_schema == 0 && abox == 0 {
        return Detected {
            primary: None,
            mixed: false,
            evidence: ev,
        };
    }

    // Entailment: SWRL/SPIN rules dominate with minimal other signals.
    if entail > 0 && entail >= class.max(shapes).max(vocab).max(abox) {
        let mixed = class + shapes + vocab + abox > 0;
        return Detected {
            primary: Some(RegistryKind::Entailment),
            mixed,
            evidence: ev,
        };
    }

    // Shapes-dominant: SHACL shapes with no real class hierarchy. (A graph that
    // declares real classes alongside shapes is a Model that happens to carry
    // shapes, not a Shapes graph.)
    if shapes > 0 && !has_classes && shapes >= vocab * 3 && shapes >= abox * 3 {
        let mixed = vocab > 0 || entail > 0 || abox > 0;
        return Detected {
            primary: Some(RegistryKind::Shapes),
            mixed,
            evidence: ev,
        };
    }

    // Instance-dominant: mostly instance data with very little schema.
    if abox > total_schema * 3 {
        let mixed = total_schema > 0;
        return Detected {
            primary: Some(RegistryKind::Instances),
            mixed,
            evidence: ev,
        };
    }

    // Vocabulary-dominant: property/relation or SKOS content with no class anchor
    // (a pure R-Box graph — e.g. a SKOS concept scheme or a property-only file).
    if vocab > 0 && !has_classes && abox < vocab {
        let mixed = shapes > 0 || entail > 0 || abox > 0;
        return Detected {
            primary: Some(RegistryKind::Vocabulary),
            mixed,
            evidence: ev,
        };
    }

    // Both class and vocabulary content may be present (a typical OWL file mixes
    // classes and properties). Dominant signal wins; ties go to Model because the
    // class hierarchy anchors the schema. `mixed` is set whenever more than one
    // logical layer is present.
    if class > 0 || vocab > 0 {
        let both = class > 0 && vocab > 0;
        let mixed = both || shapes > 0 || entail > 0 || abox > 0;
        if class >= vocab {
            if abox <= class.max(shapes) {
                return Detected {
                    primary: Some(RegistryKind::DataModel),
                    mixed,
                    evidence: ev,
                };
            }
        } else if abox <= vocab.max(shapes) {
            return Detected {
                primary: Some(RegistryKind::Vocabulary),
                mixed,
                evidence: ev,
            };
        }
    }

    Detected {
        primary: None,
        mixed: true,
        evidence: ev,
    }
}

// ─── Per-quad role classification ─────────────────────────────────────────────

/// Classify a single quad into a [`GraphKind`] for graph-splitting purposes.
///
/// This is a fast, single-quad approximation used by the analyze endpoint and
/// the `auto_split` import path. It does not look at surrounding triples.
pub fn classify_quad_role(q: &Quad) -> GraphKind {
    let p = q.predicate.as_str();

    if p == RDF_TYPE {
        if let Term::NamedNode(obj) = &q.object {
            let obj_str = obj.as_str();
            if matches!(obj_str, SH_NODE_SHAPE | SH_PROPERTY_SHAPE) {
                return GraphKind::Shapes;
            }
            if matches!(obj_str, SWRL_IMP) {
                return GraphKind::Entailment;
            }
            // Classes (T-Box) → Model.
            if matches!(obj_str, OWL_ONTOLOGY | OWL_CLASS | RDFS_CLASS) {
                return GraphKind::Model;
            }
            // Properties / relations (R-Box) → Vocabulary.
            if matches!(
                obj_str,
                OWL_OBJECT_PROPERTY
                    | OWL_DATATYPE_PROPERTY
                    | OWL_ANNOTATION_PROPERTY
                    | RDF_PROPERTY
            ) {
                return GraphKind::Vocabulary;
            }
            // SKOS concept schemes / concepts → Vocabulary.
            if matches!(obj_str, SKOS_CONCEPT_SCHEME | SKOS_CONCEPT) {
                return GraphKind::Vocabulary;
            }
            if !SCHEMA_TYPE_OBJECTS.contains(&obj_str) {
                return GraphKind::Instances;
            }
        }
    } else if p == SH_TARGET_CLASS || p.starts_with(SH_NS) {
        return GraphKind::Shapes;
    } else if p == SPIN_RULE || p.starts_with(SP_NS) {
        return GraphKind::Entailment;
    } else if p.starts_with(SKOS_NS) {
        return GraphKind::Vocabulary;
    }

    // Predicate-level axiom routing (single-quad approximation): property axioms
    // are R-Box (Vocabulary), class axioms are T-Box (Model).
    if R_BOX_PREDICATES.contains(&p) {
        return GraphKind::Vocabulary;
    }
    if T_BOX_PREDICATES.contains(&p) {
        return GraphKind::Model;
    }

    // Fallback: residual predicates in OWL/RDFS namespaces suggest model content.
    if p.starts_with("http://www.w3.org/2002/07/owl#")
        || p.starts_with("http://www.w3.org/2000/01/rdf-schema#")
    {
        return GraphKind::Model;
    }

    GraphKind::Instances
}

// ─── Subject-tree role classification ─────────────────────────────────────────

const OWL_NAMED_INDIVIDUAL: &str = "http://www.w3.org/2002/07/owl#NamedIndividual";

/// Per-quad signal for the subject-tree classifier. Type-derived signals
/// (`*Type`) outrank predicate-namespace fallbacks (`*Pred`): an instance with
/// an `rdfs:label` stays an instance, a SKOS concept with OWL annotations stays
/// vocabulary.
enum TreeSignal {
    Shapes,
    Entailment,
    ModelType,
    VocabType,
    InstanceType,
    ModelPred,
    VocabPred,
    Neutral,
}

fn tree_signal(q: &Quad) -> TreeSignal {
    let p = q.predicate.as_str();
    if p == RDF_TYPE {
        return match &q.object {
            Term::NamedNode(obj) => {
                let o = obj.as_str();
                match o {
                    SH_NODE_SHAPE | SH_PROPERTY_SHAPE => TreeSignal::Shapes,
                    SWRL_IMP => TreeSignal::Entailment,
                    SKOS_CONCEPT_SCHEME | SKOS_CONCEPT => TreeSignal::VocabType,
                    OWL_NAMED_INDIVIDUAL => TreeSignal::InstanceType,
                    // Remaining schema-namespace types (owl:Class, properties,
                    // owl:Restriction, …) are model constructs.
                    _ if SCHEMA_TYPE_OBJECTS.contains(&o) => TreeSignal::ModelType,
                    _ => TreeSignal::InstanceType,
                }
            }
            _ => TreeSignal::Neutral,
        };
    }
    if p == SH_TARGET_CLASS || p.starts_with(SH_NS) {
        TreeSignal::Shapes
    } else if p == SPIN_RULE || p.starts_with(SP_NS) {
        TreeSignal::Entailment
    } else if p.starts_with(SKOS_NS) {
        TreeSignal::VocabPred
    } else if p.starts_with("http://www.w3.org/2002/07/owl#")
        || p.starts_with("http://www.w3.org/2000/01/rdf-schema#")
    {
        TreeSignal::ModelPred
    } else {
        TreeSignal::Neutral
    }
}

#[derive(Default)]
struct TreeTally {
    shapes: usize,
    entailment: usize,
    model_type: usize,
    vocab_type: usize,
    instance_type: usize,
    model_pred: usize,
    vocab_pred: usize,
}

impl TreeTally {
    fn add(&mut self, s: TreeSignal) {
        match s {
            TreeSignal::Shapes => self.shapes += 1,
            TreeSignal::Entailment => self.entailment += 1,
            TreeSignal::ModelType => self.model_type += 1,
            TreeSignal::VocabType => self.vocab_type += 1,
            TreeSignal::InstanceType => self.instance_type += 1,
            TreeSignal::ModelPred => self.model_pred += 1,
            TreeSignal::VocabPred => self.vocab_pred += 1,
            TreeSignal::Neutral => {}
        }
    }

    /// Priority: any SHACL signal makes the whole tree a shape (a shape's
    /// `rdfs:label` must not pull it into the model graph), then rules, then
    /// type-derived verdicts, then predicate-namespace fallbacks.
    fn decide(&self) -> GraphKind {
        if self.shapes > 0 {
            return GraphKind::Shapes;
        }
        if self.entailment > 0 {
            return GraphKind::Entailment;
        }
        if self.model_type > 0 {
            return GraphKind::Model;
        }
        if self.vocab_type > 0 {
            return GraphKind::Vocabulary;
        }
        if self.instance_type > 0 {
            return GraphKind::Instances;
        }
        if self.vocab_pred > self.model_pred {
            return GraphKind::Vocabulary;
        }
        if self.model_pred > 0 {
            return GraphKind::Model;
        }
        GraphKind::Instances
    }
}

/// Root a subject tree resolves to: a named IRI or a top-level blank node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum TreeRoot<'a> {
    Iri(&'a str),
    Bnode(&'a str),
}

/// Classify every quad by its *subject tree* instead of in isolation, so the
/// whole tree lands in one sub-graph when splitting.
///
/// Per-quad classification ([`classify_quad_role`]) severs RDF structure:
/// `rdf:first`/`rdf:rest` list spines under `sh:in` / `sh:or` / `owl:unionOf`
/// fall through to Instances, and `rdfs:label` on a shape goes to Model. Here:
///
/// 1. every blank node is owned by the (root) subject from which it is
///    reachable as an object — including through `rdf:first`/`rdf:rest`
///    chains and nested property shapes;
/// 2. each root (named subject, or a top-level blank node such as
///    `[] a sh:NodeShape`) is classified once from the signals of *all* quads
///    in its tree;
/// 3. every quad of the tree — bnode closure and annotation triples included —
///    gets the root's role.
///
/// Quads whose subject cannot be tied to a tree (RDF-star quoted subjects)
/// keep the legacy per-quad classification. Returns one [`GraphKind`] per
/// quad, parallel to `quads`.
pub fn classify_quad_roles(quads: &[Quad]) -> Vec<GraphKind> {
    use oxigraph::model::NamedOrBlankNode;
    use std::collections::HashMap;

    // (1) Blank-node ownership: first quad in which the bnode appears as an
    // object wins (a serialised bnode has at most one such occurrence).
    let mut owner: HashMap<&str, &NamedOrBlankNode> = HashMap::new();
    for q in quads {
        if let Term::BlankNode(b) = &q.object {
            owner.entry(b.as_str()).or_insert(&q.subject);
        }
    }

    // Resolve a blank node to its tree root by chasing the ownership chain.
    // Unowned blank nodes root their own tree; ownership cycles (pathological)
    // collapse onto a stable member so resolution is deterministic.
    fn resolve<'a>(
        start: &'a str,
        owner: &HashMap<&'a str, &'a NamedOrBlankNode>,
        memo: &mut HashMap<&'a str, TreeRoot<'a>>,
    ) -> TreeRoot<'a> {
        if let Some(r) = memo.get(start) {
            return *r;
        }
        let mut path: Vec<&'a str> = vec![start];
        let mut cur = start;
        let root = loop {
            match owner.get(cur) {
                Some(NamedOrBlankNode::NamedNode(n)) => break TreeRoot::Iri(n.as_str()),
                Some(NamedOrBlankNode::BlankNode(b)) => {
                    let next = b.as_str();
                    if let Some(r) = memo.get(next) {
                        break *r;
                    }
                    if path.contains(&next) {
                        break TreeRoot::Bnode(path.iter().copied().min().unwrap_or(next));
                    }
                    path.push(next);
                    cur = next;
                }
                // Quoted-triple owner or no owner at all: this bnode is a root.
                _ => break TreeRoot::Bnode(cur),
            }
        };
        for p in path {
            memo.insert(p, root);
        }
        root
    }

    let mut memo: HashMap<&str, TreeRoot> = HashMap::new();
    let roots: Vec<Option<TreeRoot>> = quads
        .iter()
        .map(|q| match &q.subject {
            NamedOrBlankNode::NamedNode(n) => Some(TreeRoot::Iri(n.as_str())),
            NamedOrBlankNode::BlankNode(b) => Some(resolve(b.as_str(), &owner, &mut memo)),
        })
        .collect();

    // (2) Tally signals per root over the whole tree.
    let mut tallies: HashMap<TreeRoot, TreeTally> = HashMap::new();
    for (q, root) in quads.iter().zip(&roots) {
        if let Some(root) = root {
            tallies.entry(*root).or_default().add(tree_signal(q));
        }
    }

    // (3) Every quad inherits its root's verdict.
    let mut decided: HashMap<TreeRoot, GraphKind> = HashMap::with_capacity(tallies.len());
    quads
        .iter()
        .zip(&roots)
        .map(|(q, root)| match root {
            Some(r) => *decided.entry(*r).or_insert_with(|| {
                tallies
                    .get(r)
                    .map(TreeTally::decide)
                    .unwrap_or(GraphKind::Instances)
            }),
            None => classify_quad_role(q),
        })
        .collect()
}

// ─── Override parsing ─────────────────────────────────────────────────────────

/// Parse a `?kind=` query param into a [`RegistryKind`].
pub fn parse_kind_override(s: &str) -> Option<RegistryKind> {
    match s {
        "model" | "data-model" | "datamodel" | "data_model" | "tbox" => {
            Some(RegistryKind::DataModel)
        }
        "vocabulary" | "vocab" => Some(RegistryKind::Vocabulary),
        "shapes" => Some(RegistryKind::Shapes),
        "entailment" => Some(RegistryKind::Entailment),
        "instances" | "instance" | "abox" => Some(RegistryKind::Instances),
        _ => None,
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use oxigraph::io::{RdfFormat, RdfParser};
    use std::io::BufReader;

    fn parse(ttl: &str) -> Vec<Quad> {
        RdfParser::from_format(RdfFormat::Turtle)
            .for_reader(BufReader::new(ttl.as_bytes()))
            .map(|r| r.unwrap())
            .collect()
    }

    #[test]
    fn owl_classes_and_properties_is_model_mixed() {
        // A typical OWL file mixes classes (T-Box → Model) and properties
        // (R-Box → Vocabulary). The class signal anchors the primary kind, but
        // the presence of properties flags it as mixed.
        let ttl = r#"
            @prefix owl: <http://www.w3.org/2002/07/owl#> .
            @prefix ex: <http://example.org/> .
            ex:Ont a owl:Ontology .
            ex:Person a owl:Class .
            ex:knows a owl:ObjectProperty .
        "#;
        let d = detect(&parse(ttl));
        assert_eq!(d.primary, Some(RegistryKind::DataModel));
        assert!(d.mixed);
        assert_eq!(d.evidence.owl_ontology, 1);
        assert_eq!(d.evidence.owl_classes, 1);
        assert_eq!(d.evidence.owl_properties, 1);
    }

    #[test]
    fn property_only_is_vocabulary() {
        // A graph of properties/relations with no class anchor is R-Box →
        // Vocabulary, even though it lives in the owl: namespace.
        let ttl = r#"
            @prefix owl: <http://www.w3.org/2002/07/owl#> .
            @prefix ex: <http://example.org/> .
            ex:knows a owl:ObjectProperty .
            ex:name a owl:DatatypeProperty .
            ex:label a owl:AnnotationProperty .
        "#;
        let d = detect(&parse(ttl));
        assert_eq!(d.primary, Some(RegistryKind::Vocabulary));
        assert!(!d.mixed);
        assert_eq!(d.evidence.owl_properties, 3);
    }

    #[test]
    fn ontology_header_with_properties_is_vocabulary() {
        // An owl:Ontology header alone must not block a property-only file from
        // classifying as Vocabulary (has_real_classes is false).
        let ttl = r#"
            @prefix owl: <http://www.w3.org/2002/07/owl#> .
            @prefix ex: <http://example.org/> .
            ex:Ont a owl:Ontology .
            ex:knows a owl:ObjectProperty .
            ex:likes a owl:ObjectProperty .
        "#;
        let d = detect(&parse(ttl));
        assert_eq!(d.primary, Some(RegistryKind::Vocabulary));
    }

    #[test]
    fn pure_skos_is_vocabulary() {
        let ttl = r#"
            @prefix skos: <http://www.w3.org/2004/02/skos/core#> .
            @prefix ex: <http://example.org/> .
            ex:Scheme a skos:ConceptScheme .
            ex:Red a skos:Concept .
            ex:Blue a skos:Concept .
        "#;
        let d = detect(&parse(ttl));
        assert_eq!(d.primary, Some(RegistryKind::Vocabulary));
        assert!(!d.mixed);
        assert_eq!(d.evidence.skos_concepts, 2);
    }

    #[test]
    fn shacl_only_is_shapes() {
        let ttl = r#"
            @prefix sh: <http://www.w3.org/ns/shacl#> .
            @prefix ex: <http://example.org/> .
            ex:PersonShape a sh:NodeShape ;
                sh:targetClass ex:Person .
        "#;
        let d = detect(&parse(ttl));
        assert_eq!(d.primary, Some(RegistryKind::Shapes));
    }

    #[test]
    fn owl_plus_shacl_is_data_model_with_mixed() {
        let ttl = r#"
            @prefix owl: <http://www.w3.org/2002/07/owl#> .
            @prefix sh:  <http://www.w3.org/ns/shacl#> .
            @prefix ex:  <http://example.org/> .
            ex:Person a owl:Class .
            ex:name a owl:DatatypeProperty .
            ex:PersonShape a sh:NodeShape ; sh:targetClass ex:Person .
        "#;
        let d = detect(&parse(ttl));
        assert_eq!(d.primary, Some(RegistryKind::DataModel));
        assert!(d.mixed);
    }

    #[test]
    fn instance_data_is_instances() {
        let ttl = r#"
            @prefix ex: <http://example.org/> .
            @prefix foaf: <http://xmlns.com/foaf/0.1/> .
            ex:Alice a foaf:Person .
            ex:Bob a foaf:Person .
            ex:Carol a foaf:Person .
            ex:Dave a foaf:Person .
            ex:Eve a foaf:Person .
            ex:Frank a foaf:Person .
            ex:Grace a foaf:Person .
            ex:Heidi a foaf:Person .
            ex:Ivan a foaf:Person .
            ex:Judy a foaf:Person .
        "#;
        let d = detect(&parse(ttl));
        assert_eq!(d.primary, Some(RegistryKind::Instances));
    }

    #[test]
    fn balanced_class_and_concept_is_model_mixed() {
        // Classes (Model) tie with SKOS concepts (Vocabulary): ties go to Model
        // because the class hierarchy anchors the schema; flagged mixed.
        let ttl = r#"
            @prefix owl: <http://www.w3.org/2002/07/owl#> .
            @prefix skos: <http://www.w3.org/2004/02/skos/core#> .
            @prefix ex: <http://example.org/> .
            ex:A a owl:Class .
            ex:B a owl:Class .
            ex:Red a skos:Concept .
            ex:Blue a skos:Concept .
        "#;
        let d = detect(&parse(ttl));
        assert_eq!(d.primary, Some(RegistryKind::DataModel));
        assert!(d.mixed);
    }

    #[test]
    fn dominant_data_model_wins_with_mixed_flag() {
        let ttl = r#"
            @prefix owl: <http://www.w3.org/2002/07/owl#> .
            @prefix skos: <http://www.w3.org/2004/02/skos/core#> .
            @prefix ex: <http://example.org/> .
            ex:Ont a owl:Ontology .
            ex:A a owl:Class . ex:B a owl:Class . ex:C a owl:Class .
            ex:D a owl:Class . ex:E a owl:Class . ex:F a owl:Class .
            ex:Red a skos:Concept .
        "#;
        let d = detect(&parse(ttl));
        assert_eq!(d.primary, Some(RegistryKind::DataModel));
        assert!(d.mixed);
    }

    #[test]
    fn empty_is_unclassified() {
        let d = detect(&[]);
        assert!(d.primary.is_none());
        assert!(!d.mixed);
    }

    #[test]
    fn override_parsing() {
        assert_eq!(
            parse_kind_override("data-model"),
            Some(RegistryKind::DataModel)
        );
        assert_eq!(parse_kind_override("tbox"), Some(RegistryKind::DataModel));
        assert_eq!(
            parse_kind_override("vocabulary"),
            Some(RegistryKind::Vocabulary)
        );
        assert_eq!(parse_kind_override("vocab"), Some(RegistryKind::Vocabulary));
        assert_eq!(parse_kind_override("shapes"), Some(RegistryKind::Shapes));
        assert_eq!(parse_kind_override("abox"), Some(RegistryKind::Instances));
        assert_eq!(parse_kind_override("nope"), None);
    }

    // ─── Subject-tree classification (classify_quad_roles) ────────────────────

    /// Roles for all quads whose subject-tree root role we want to inspect,
    /// keyed by a human-readable triple rendering for failure messages.
    fn roles_by_triple(ttl: &str) -> Vec<(String, GraphKind)> {
        let quads = parse(ttl);
        let roles = classify_quad_roles(&quads);
        quads
            .iter()
            .zip(roles)
            .map(|(q, r)| (format!("{} {} {}", q.subject, q.predicate, q.object), r))
            .collect()
    }

    #[test]
    fn sh_in_list_stays_with_its_shape() {
        let ttl = r#"
            @prefix sh: <http://www.w3.org/ns/shacl#> .
            @prefix ex: <http://example.org/> .
            ex:StatusShape a sh:NodeShape ;
                sh:targetClass ex:Thing ;
                sh:property [
                    sh:path ex:status ;
                    sh:in ( "open" "closed" "pending" ) ;
                ] .
        "#;
        for (t, role) in roles_by_triple(ttl) {
            assert_eq!(
                role,
                GraphKind::Shapes,
                "quad must stay in the shapes tree: {t}"
            );
        }
        // The fixture really contains a severable list spine.
        assert!(roles_by_triple(ttl)
            .iter()
            .any(|(t, _)| t.contains("rdf-syntax-ns#first")));
    }

    #[test]
    fn owl_unionof_list_stays_with_its_class() {
        let ttl = r#"
            @prefix owl: <http://www.w3.org/2002/07/owl#> .
            @prefix ex: <http://example.org/> .
            ex:Vehicle a owl:Class ;
                owl:unionOf ( ex:Car ex:Bike ) .
        "#;
        for (t, role) in roles_by_triple(ttl) {
            assert_eq!(
                role,
                GraphKind::Model,
                "quad must stay in the class's model tree: {t}"
            );
        }
    }

    #[test]
    fn rdfs_label_on_shape_stays_in_shapes() {
        let ttl = r#"
            @prefix sh: <http://www.w3.org/ns/shacl#> .
            @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
            @prefix ex: <http://example.org/> .
            ex:PersonShape a sh:NodeShape ;
                rdfs:label "Person shape" ;
                rdfs:comment "Validates people" ;
                sh:targetClass ex:Person .
        "#;
        for (t, role) in roles_by_triple(ttl) {
            assert_eq!(
                role,
                GraphKind::Shapes,
                "annotation severed from shape: {t}"
            );
        }
    }

    #[test]
    fn anonymous_root_shape_tree_is_shapes() {
        let ttl = r#"
            @prefix sh: <http://www.w3.org/ns/shacl#> .
            @prefix ex: <http://example.org/> .
            [] a sh:NodeShape ;
                sh:targetClass ex:Person ;
                sh:property [ sh:path ex:age ; sh:in ( 1 2 3 ) ] .
        "#;
        for (t, role) in roles_by_triple(ttl) {
            assert_eq!(role, GraphKind::Shapes, "anonymous shape tree severed: {t}");
        }
    }

    #[test]
    fn plain_instances_unaffected() {
        let ttl = r#"
            @prefix ex: <http://example.org/> .
            @prefix foaf: <http://xmlns.com/foaf/0.1/> .
            @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
            ex:Alice a foaf:Person ; foaf:name "Alice" ; rdfs:label "Alice" .
            ex:Bob a foaf:Person ; foaf:knows ex:Alice .
        "#;
        for (t, role) in roles_by_triple(ttl) {
            assert_eq!(
                role,
                GraphKind::Instances,
                "instance quad misclassified: {t}"
            );
        }
    }

    #[test]
    fn mixed_owl_shacl_trees_split_per_root() {
        let ttl = r#"
            @prefix owl: <http://www.w3.org/2002/07/owl#> .
            @prefix sh:  <http://www.w3.org/ns/shacl#> .
            @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
            @prefix ex:  <http://example.org/> .
            ex:Vehicle a owl:Class ; rdfs:label "Vehicle" ; owl:unionOf ( ex:Car ex:Bike ) .
            ex:VehicleShape a sh:NodeShape ;
                sh:targetClass ex:Vehicle ;
                sh:property [ sh:path ex:kind ; sh:in ( "car" "bike" ) ] .
        "#;
        let quads = parse(ttl);
        let roles = classify_quad_roles(&quads);
        let mut model = 0usize;
        let mut shapes = 0usize;
        for (q, role) in quads.iter().zip(&roles) {
            match role {
                GraphKind::Model => model += 1,
                GraphKind::Shapes => shapes += 1,
                other => panic!("unexpected role {other:?} for {q}"),
            }
        }
        // Class tree: type + label + unionOf + 4 list-spine quads = 7.
        assert_eq!(model, 7, "owl:Class tree (incl. union list) → model");
        // Shape tree: type + targetClass + property + (path + in) + 4 spine = 9.
        assert_eq!(shapes, 9, "shape tree (incl. sh:in list) → shapes");

        // File-level verdict stays single-role (Model) but exposes the shapes.
        let d = detect(&quads);
        assert_eq!(d.primary, Some(RegistryKind::DataModel));
        assert!(d.has_shapes(), "mixed file must surface embedded shapes");
    }

    #[test]
    fn skos_concept_with_rdfs_annotations_stays_vocabulary() {
        let ttl = r#"
            @prefix skos: <http://www.w3.org/2004/02/skos/core#> .
            @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
            @prefix ex: <http://example.org/> .
            ex:Red a skos:Concept ; skos:prefLabel "Red" ; rdfs:comment "A colour" .
        "#;
        for (t, role) in roles_by_triple(ttl) {
            assert_eq!(role, GraphKind::Vocabulary, "concept tree severed: {t}");
        }
    }

    #[test]
    fn to_graph_role_mapping() {
        assert_eq!(RegistryKind::DataModel.to_graph_role(), GraphKind::Model);
        assert_eq!(
            RegistryKind::Vocabulary.to_graph_role(),
            GraphKind::Vocabulary
        );
        assert_eq!(RegistryKind::Shapes.to_graph_role(), GraphKind::Shapes);
        assert_eq!(
            RegistryKind::Entailment.to_graph_role(),
            GraphKind::Entailment
        );
        assert_eq!(
            RegistryKind::Instances.to_graph_role(),
            GraphKind::Instances
        );
    }

    #[test]
    fn classify_quad_role_routes_classes_and_properties() {
        let pfx = "@prefix owl: <http://www.w3.org/2002/07/owl#> . \
                   @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> . \
                   @prefix skos: <http://www.w3.org/2004/02/skos/core#> . \
                   @prefix ex: <http://example.org/> .\n";
        let role1 = |triple: &str| classify_quad_role(&parse(&format!("{pfx}{triple}"))[0]);

        // Classes (T-Box) → Model.
        assert_eq!(role1("ex:Person a owl:Class ."), GraphKind::Model);
        assert_eq!(
            role1("ex:Student rdfs:subClassOf ex:Person ."),
            GraphKind::Model
        );
        // Properties / relations (R-Box) → Vocabulary.
        assert_eq!(
            role1("ex:knows a owl:ObjectProperty ."),
            GraphKind::Vocabulary
        );
        assert_eq!(
            role1("ex:age a owl:DatatypeProperty ."),
            GraphKind::Vocabulary
        );
        assert_eq!(
            role1("ex:knows rdfs:domain ex:Person ."),
            GraphKind::Vocabulary
        );
        assert_eq!(
            role1("ex:knows rdfs:range ex:Person ."),
            GraphKind::Vocabulary
        );
        // SKOS → Vocabulary.
        assert_eq!(role1("ex:Red a skos:Concept ."), GraphKind::Vocabulary);
        // Instance data → Instances.
        assert_eq!(role1("ex:alice a ex:Person ."), GraphKind::Instances);
    }
}
