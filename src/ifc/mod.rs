//! IFC (ISO 16739) → linked data.
//!
//! Converts IFC STEP files into two RDF layers, emitted as N-Triples chunks:
//!
//! * **BOT layer** — the queryable building topology the rest of the platform
//!   (viewer feed, browse, SPARQL demos) consumes: `bot:Site/Building/Storey/
//!   Space/Element` with `bot:containsElement`/`bot:hasSubElement` containment,
//!   `rdfs:label`s, IFC GlobalIds, property-set values as `props:` data
//!   properties, and FOG file references back to the original IFC.
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

/// Options for one conversion run.
pub struct ConvertOptions {
    /// Base IRI for minted instances; rooted entities get `{base}{GlobalId}`,
    /// unrooted ones `{base}i{stepId}`. Must end with `/` or `#`.
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
