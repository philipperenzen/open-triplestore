//! IFC (ISO 16739) → linked data.
//!
//! Converts IFC STEP files into two RDF layers, emitted as N-Triples chunks:
//!
//! * **BOT layer** — the queryable building topology the rest of the platform
//!   (viewer feed, browse, SPARQL demos) consumes: `bot:Site/Building/Storey/
//!   Space/Element` with `bot:containsElement`/`bot:hasSubElement` containment,
//!   `rdfs:label`s, IFC GlobalIds, property-set values as `props:` data
//!   properties, and FOG file references back to the original IFC. On top of
//!   that flat contract, and never instead of it: the NEN 2660-2 relation
//!   family beside the BOT edges, property and quantity values as typed nodes
//!   with QUDT units, classifications as SKOS concepts, materials, the IFC 4.3
//!   facility spine under `bot:Zone`, and the map conversion as a
//!   CRS-qualified geometry — all under the lift's own namespace,
//!   `{base_url}/ns/ifc-lift#`, whose ontology ships as the `ifc-lift` seed
//!   bundle.
//! * **ifcOWL layer** (optional) — a complete instance-level lift of the STEP
//!   file: every instance typed in the schema's ifcOWL namespace with all its
//!   attributes. Encoding is the pragmatic "direct" style (literals attached
//!   directly, reference lists repeat the predicate, numeric lists collapse to
//!   one literal) rather than the canonical express:hasX indirection — lossless
//!   at the instance level and far cheaper to store and query.
//!
//! The parser tolerates the formatting quirks of real exporters (ArchiCAD,
//! Synchro, Revit): see [`step`].

pub mod names;
pub mod rdf;
pub mod step;
pub mod units;

/// Options for one conversion run.
#[derive(Default)]
pub struct ConvertOptions {
    /// Base IRI for minted instances; rooted entities get `{base}{GlobalId}`,
    /// unrooted ones `{base}i{stepId}`. Must end with `/` or `#`. The lift's
    /// own namespace is derived from it — see [`rdf::lift_namespace`].
    pub inst_base: String,
    /// Public URL of the stored IFC file — emitted as `fog:asIfc…` references
    /// (per element with a `#GlobalId` fragment) so viewers and downloads can
    /// reach the original. `None` skips the references.
    pub ifc_file_url: Option<String>,
    /// Optional site anchor as a (possibly CRS-prefixed) WKT literal value,
    /// e.g. `POINT(5.83 51.84)` — attached to the site (or building) so the
    /// map viewer can place the model.
    pub anchor_wkt: Option<String>,
    /// Also produce the full ifcOWL-style lift (large!).
    pub include_ifcowl: bool,
    /// Authored `ots:modelHeading` (degrees clockwise from north for the model's
    /// +X axis), stamped on every element file-link node when set.
    pub model_heading: Option<f64>,
    /// Friendly display label for the ROOT spatial element (the IfcSite, or the
    /// building when the file has no site). Exporters routinely leave "Site" /
    /// "Default" / "Gelaende" there, which then headlines the whole model in
    /// every viewer tree. Overrides the root's `rdfs:label` only (the authored
    /// name is preserved as `props:ifcName`); all other elements keep their
    /// file-authored names.
    pub root_label: Option<String>,
    /// Provenance stamped on the root element: where the model came from
    /// (`dct:source`), its license (`dct:license`) and the attribution line
    /// (`dct:rightsHolder`). All optional.
    pub provenance_source: Option<String>,
    pub license: Option<String>,
    pub attribution: Option<String>,
}

/// Counters reported after a conversion.
#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct IfcStats {
    pub schema: String,
    pub instances: usize,
    pub elements: usize,
    pub storeys: usize,
    pub spaces: usize,
    pub bot_triples: usize,
    pub ifcowl_triples: usize,
    /// IFC 4.3 facilities and facility parts (bridges, roads, …) in the
    /// spatial spine.
    #[serde(default)]
    pub facilities: usize,
    /// Physical quantities lifted from element quantity sets.
    #[serde(default)]
    pub quantities: usize,
    /// Property values lifted as typed nodes (every kind, not only single).
    #[serde(default)]
    pub properties: usize,
    /// Units a quantity or property declared that have no QUDT IRI in the
    /// table — emitted as a label (and a conversion factor when there is
    /// one), never as a guessed IRI.
    #[serde(default)]
    pub unmapped_units: usize,
    /// Classification references lifted to SKOS concepts.
    #[serde(default)]
    pub classifications: usize,
    /// Element–material associations.
    #[serde(default)]
    pub materials: usize,
    /// Whether the model context carries an `IfcMapConversion` that was
    /// lifted (its CRS may still be unrecognised).
    #[serde(default)]
    pub map_conversion: bool,
}

/// Parse `input` and emit RDF. `bot_sink` / `ifcowl_sink` receive N-Triples
/// chunks (several MB each) suitable for `graph_store_post` into two graphs.
pub fn convert(
    input: &str,
    opts: &ConvertOptions,
    bot_sink: &mut dyn FnMut(&str),
    ifcowl_sink: &mut dyn FnMut(&str),
) -> Result<IfcStats, String> {
    let file = step::parse(input)?;
    rdf::emit(&file, opts, bot_sink, ifcowl_sink)
}
