//! IRI resolution shared by the rule readers: prefixed names against declared
//! prefixes, relative IRIs against a base.
//!
//! Every reader hands its predicates and individuals to the engine as full
//! IRIs; the engine refuses anything that is not one. This is where the
//! shorthand of each syntax is expanded first.

use std::collections::HashMap;

use oxiri::Iri;

/// The prefixes OWL 2 declares in every ontology (OWL 2 Structural
/// Specification §2.4), plus `swrl:` and `swrlb:`, which rule documents use
/// without declaring.
const PREDECLARED: &[(&str, &str)] = &[
    ("rdf", "http://www.w3.org/1999/02/22-rdf-syntax-ns#"),
    ("rdfs", "http://www.w3.org/2000/01/rdf-schema#"),
    ("xsd", "http://www.w3.org/2001/XMLSchema#"),
    ("owl", "http://www.w3.org/2002/07/owl#"),
    ("swrl", "http://www.w3.org/2003/11/swrl#"),
    ("swrlb", "http://www.w3.org/2003/11/swrlb#"),
];

/// A prefix lookup consulted after the document's own declarations.
pub(crate) type PrefixFallback = super::PrefixLookup;

/// Declared prefixes and a base IRI.
#[derive(Clone)]
pub(crate) struct Names {
    prefixes: HashMap<String, String>,
    base: Option<Iri<String>>,
    /// Consulted for a prefix the document did not declare (the server's
    /// prefix registry, for the human-readable syntax).
    fallback: Option<PrefixFallback>,
}

impl Default for Names {
    fn default() -> Self {
        Names {
            prefixes: PREDECLARED
                .iter()
                .map(|(p, ns)| (p.to_string(), ns.to_string()))
                .collect(),
            base: None,
            fallback: None,
        }
    }
}

impl Names {
    /// Declare `prefix` (without the colon; `""` is the default prefix).
    pub(crate) fn declare(&mut self, prefix: &str, namespace: &str) {
        self.prefixes
            .insert(prefix.to_string(), namespace.to_string());
    }

    /// Consult `lookup` for prefixes the document does not declare.
    pub(crate) fn with_fallback(mut self, lookup: PrefixFallback) -> Self {
        self.fallback = Some(lookup);
        self
    }

    /// Set the base IRI relative IRIs resolve against (`xml:base`).
    pub(crate) fn set_base(&mut self, base: &str) -> Result<(), String> {
        let resolved = match &self.base {
            Some(current) => current
                .resolve(base)
                .map_err(|e| format!("Invalid xml:base '{base}': {e}"))?,
            None => Iri::parse(base.to_string())
                .map_err(|e| format!("Invalid xml:base '{base}': {e} (it must be absolute)"))?,
        };
        self.base = Some(resolved);
        Ok(())
    }

    /// Resolve an IRI that may be relative. Without a base a relative IRI
    /// is returned unchanged, and the engine refuses it by name.
    pub(crate) fn resolve(&self, iri: &str) -> String {
        match &self.base {
            Some(base) => base
                .resolve(iri)
                .map(Iri::into_inner)
                .unwrap_or_else(|_| iri.to_string()),
            None => iri.to_string(),
        }
    }

    /// Expand a prefixed name `prefix:local` (`:local` for the default
    /// prefix).
    pub(crate) fn expand(&self, pname: &str) -> Result<String, String> {
        let Some((prefix, local)) = pname.split_once(':') else {
            return Err(format!(
                "'{pname}' is not a prefixed name: write prefix:name or a full IRI"
            ));
        };
        if let Some(ns) = self.prefixes.get(prefix) {
            return Ok(format!("{ns}{local}"));
        }
        if !prefix.is_empty() {
            if let Some(ns) = self.fallback.as_ref().and_then(|f| f(prefix)) {
                return Ok(format!("{ns}{local}"));
            }
        }
        Err(if prefix.is_empty() {
            format!("'{pname}' uses the default prefix, which is not declared")
        } else {
            format!("Unknown prefix '{prefix}:' in '{pname}'")
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefixes_base_and_fallback() {
        let mut names = Names::default();
        assert_eq!(
            names.expand("xsd:integer").unwrap(),
            "http://www.w3.org/2001/XMLSchema#integer"
        );
        assert!(names.expand(":A").is_err());
        names.declare("", "http://ex/");
        assert_eq!(names.expand(":A").unwrap(), "http://ex/A");
        assert!(names.expand("foaf:name").unwrap_err().contains("foaf"));
        let names = names.with_fallback(std::sync::Arc::new(|p: &str| {
            (p == "foaf").then(|| "http://xmlns.com/foaf/0.1/".to_string())
        }));
        assert_eq!(
            names.expand("foaf:name").unwrap(),
            "http://xmlns.com/foaf/0.1/name"
        );

        let mut names = Names::default();
        assert_eq!(names.resolve("#x"), "#x");
        names.set_base("http://ex/onto").unwrap();
        assert_eq!(names.resolve("#x"), "http://ex/onto#x");
        assert_eq!(names.resolve("http://other/y"), "http://other/y");
        assert!(Names::default().set_base("relative").is_err());
    }
}
