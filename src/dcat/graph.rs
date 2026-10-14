//! The triple builder both catalogues (the dataset catalogue in
//! [`super::catalog`] and the model registry's in `crate::catalog::builder`)
//! write through, and the DCAT range typing they share.
//!
//! Every user-supplied value becomes a real RDF term: a literal is a literal
//! whatever it contains, and a string that is not an IRI is dropped with a
//! warning instead of being interpolated into a document.
//!
//! **Range typing (DCAT 3, DCAT-AP 3 ranges).** An object is typed with the
//! class the property's range names — a licence is a `dct:LicenseDocument`, a
//! media type a `dct:MediaType`, a theme a `skos:Concept` with its
//! `skos:prefLabel` — once per object, so a catalogue validates against the
//! DCAT-AP shapes without the EU vocabularies loaded as background knowledge.

use std::collections::HashSet;

use oxigraph::io::{RdfFormat, RdfSerializer};
use oxigraph::model::{Literal, NamedNode, NamedOrBlankNode, Term, Triple};

use super::authority;
use super::vocabulary::*;

pub const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
pub const RDFS_RESOURCE: &str = "http://www.w3.org/2000/01/rdf-schema#Resource";
pub const SKOS: &str = "http://www.w3.org/2004/02/skos/core#";
pub const OTS: &str = "https://opentriplestore.org/ns#";

/// The class a property's range names, for the properties the catalogues emit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Range {
    LicenseDocument,
    LinguisticSystem,
    RightsStatement,
    Standard,
    MediaType,
    MediaTypeOrExtent,
    Document,
    Location,
    Frequency,
    Activity,
    Resource,
}

impl Range {
    fn class(self) -> String {
        match self {
            Range::LicenseDocument => format!("{DCT}LicenseDocument"),
            Range::LinguisticSystem => format!("{DCT}LinguisticSystem"),
            Range::RightsStatement => format!("{DCT}RightsStatement"),
            Range::Standard => format!("{DCT}Standard"),
            Range::MediaType => format!("{DCT}MediaType"),
            Range::MediaTypeOrExtent => format!("{DCT}MediaTypeOrExtent"),
            Range::Document => format!("{FOAF}Document"),
            Range::Location => format!("{DCT}Location"),
            Range::Frequency => format!("{DCT}Frequency"),
            Range::Activity => format!("{PROV}Activity"),
            Range::Resource => RDFS_RESOURCE.to_string(),
        }
    }
}

pub fn nn(s: &str) -> NamedNode {
    NamedNode::new_unchecked(s)
}

pub fn p(ns: &str, local: &str) -> String {
    format!("{ns}{local}")
}

/// A catalogue under construction.
pub struct G {
    pub triples: Vec<Triple>,
    /// Agents already described (an agent is described once).
    pub agents: HashSet<String>,
    /// `(node, class)` pairs already typed.
    typed: HashSet<(String, String)>,
    /// Profile warnings: what an application profile asks for that the
    /// catalogue could not say without inventing it.
    pub warnings: Vec<String>,
    /// The catalogue's language tag (`en`, `nl`), for concept labels.
    pub lang_tag: &'static str,
}

impl G {
    pub fn new(lang_tag: &'static str) -> Self {
        Self {
            triples: Vec::with_capacity(256),
            agents: HashSet::new(),
            typed: HashSet::new(),
            warnings: Vec::new(),
            lang_tag,
        }
    }

    /// Record a profile warning (once).
    pub fn warn(&mut self, msg: String) {
        if !self.warnings.contains(&msg) {
            self.warnings.push(msg);
        }
    }

    /// A user-supplied IRI, or none (with a warning) when it is not one.
    pub fn iri(&self, s: &str, what: &str) -> Option<NamedNode> {
        match NamedNode::new(s.trim()) {
            Ok(n) => Some(n),
            Err(e) => {
                tracing::warn!("dcat: dropping {what} `{s}`: not an IRI ({e})");
                None
            }
        }
    }
    pub fn add(&mut self, s: impl Into<NamedOrBlankNode>, p: &str, o: impl Into<Term>) {
        self.triples.push(Triple::new(s, nn(p), o));
    }
    pub fn typ(&mut self, s: impl Into<NamedOrBlankNode>, class: &str) {
        let s = s.into();
        if self.typed.insert((s.to_string(), class.to_string())) {
            self.add(s, RDF_TYPE, nn(class));
        }
    }
    pub fn lit(&mut self, s: impl Into<NamedOrBlankNode>, p: &str, text: &str) {
        self.add(s, p, Literal::new_simple_literal(text));
    }
    pub fn lang(&mut self, s: impl Into<NamedOrBlankNode>, p: &str, text: &str, lang: &str) {
        match Literal::new_language_tagged_literal(text, lang) {
            Ok(l) => self.add(s, p, l),
            Err(_) => self.lit(s, p, text),
        }
    }
    pub fn typed(&mut self, s: impl Into<NamedOrBlankNode>, p: &str, text: &str, dt: &str) {
        self.add(s, p, Literal::new_typed_literal(text, nn(dt)));
    }
    pub fn int(&mut self, s: impl Into<NamedOrBlankNode>, p: &str, n: usize) {
        self.add(s, p, Literal::from(n as i64));
    }
    /// `s p "when"^^xsd:dateTime` (or `xsd:date` for a bare date).
    pub fn when(&mut self, s: impl Into<NamedOrBlankNode>, p: &str, when: &str) {
        if let Some((v, dt)) = temporal_literal(when) {
            self.typed(s, p, &v, dt);
        }
    }
    /// Add `s p <o>` when `o` is a valid IRI.
    pub fn link(&mut self, s: impl Into<NamedOrBlankNode>, p: &str, o: &str, what: &str) -> bool {
        match self.iri(o, what) {
            Some(n) => {
                self.add(s, p, n);
                true
            }
            None => false,
        }
    }
    /// Add `s p <o>` and type `o` with the property's range class.
    pub fn ranged(
        &mut self,
        s: impl Into<NamedOrBlankNode>,
        p: &str,
        o: &str,
        range: Range,
        what: &str,
    ) -> bool {
        match self.iri(o, what) {
            Some(n) => {
                self.add(s, p, n.clone());
                self.typ(n, &range.class());
                true
            }
            None => false,
        }
    }
    /// Add `s p <o>` with `o` a `skos:Concept`: labelled from the controlled
    /// vocabularies when it is one of theirs, otherwise a profile warning
    /// (DCAT-AP asks every concept for a `skos:prefLabel`).
    pub fn concept(&mut self, s: impl Into<NamedOrBlankNode>, p: &str, o: &str, what: &str) {
        let Some(n) = self.iri(o, what) else {
            return;
        };
        self.add(s, p, n.clone());
        let first = self.typed.insert((n.to_string(), format!("{SKOS}Concept")));
        if !first {
            return;
        }
        self.add(n.clone(), RDF_TYPE, nn(&format!("{SKOS}Concept")));
        match authority::lookup(n.as_str()) {
            Some(c) => {
                let (label, tag) = c.label(self.lang_tag);
                self.lang(n, &format!("{SKOS}prefLabel"), label, tag);
            }
            None => self.warn(format!(
                "{what} <{}> is not in a vocabulary the catalogue knows, so it has no \
                 skos:prefLabel (DCAT-AP requires one on every skos:Concept)",
                n.as_str()
            )),
        }
    }
}

/// A date or date-time as an `xsd:date` / `xsd:dateTime` literal value, or
/// `None` when it is neither. A bare `YYYY-MM-DD` is a date; an RFC 3339
/// timestamp is a date-time.
pub fn temporal_literal(s: &str) -> Option<(String, &'static str)> {
    let t = s.trim();
    if chrono::NaiveDate::parse_from_str(t, "%Y-%m-%d").is_ok() {
        return Some((t.to_string(), "http://www.w3.org/2001/XMLSchema#date"));
    }
    if chrono::DateTime::parse_from_rfc3339(t).is_ok()
        || chrono::NaiveDateTime::parse_from_str(t, "%Y-%m-%dT%H:%M:%S%.f").is_ok()
    {
        return Some((t.to_string(), "http://www.w3.org/2001/XMLSchema#dateTime"));
    }
    None
}

/// Serialise triples in `format` (Turtle and TriG get the usual prefixes).
pub fn serialize(triples: &[Triple], format: RdfFormat) -> Result<Vec<u8>, String> {
    let mut ser = RdfSerializer::from_format(format);
    if matches!(format, RdfFormat::Turtle | RdfFormat::TriG) {
        for (pfx, ns) in [
            ("dcat", DCAT),
            ("dct", DCT),
            ("void", VOID),
            ("foaf", FOAF),
            ("prov", PROV),
            ("org", ORG),
            ("adms", ADMS),
            ("schema", SCHEMA),
            ("vcard", VCARD),
            ("xsd", XSD),
            ("sd", SD),
            ("skos", SKOS),
            ("ots", OTS),
        ] {
            ser = ser.with_prefix(pfx, ns).map_err(|e| e.to_string())?;
        }
    }
    let mut buf = Vec::with_capacity(triples.len() * 96);
    let mut w = ser.for_writer(&mut buf);
    for t in triples {
        w.serialize_triple(t.as_ref()).map_err(|e| e.to_string())?;
    }
    w.finish().map_err(|e| e.to_string())?;
    Ok(buf)
}
