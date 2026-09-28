//! Identity policy — what a dataset's reasoning does with `owl:sameAs`.
//!
//! `owl:sameAs` carries Leibniz identity: under the OWL 2 RL equality rules
//! (`eq-sym`, `eq-trans`, `eq-rep-s/p/o`) every property of one node becomes a
//! property of the other. That is right for two IRIs minted for one record,
//! and wrong for almost every cross-source link — a design-stage model element
//! is not the as-built asset, a registration record is not the physical
//! object, a spatial footprint is not the thing it outlines (Beck, Abualdenien
//! & Borrmann, LDAC 2021). Once such links are materialised, attributes,
//! geometries and lifecycle states leak between representations.
//!
//! The policy is set per organisation (inherited by its datasets) or per
//! dataset, and read by the entailment layer when it materialises:
//!
//! | policy | `eq-*` rules | linkset-role graphs as reasoning sources |
//! |---|---|---|
//! | `sameas-off` | never | no |
//! | `sameas-narrow` (built-in default) | yes, over the dataset's own graphs | no |
//! | `sameas-full` | yes | yes |
//!
//! Typed correspondences — `prov:specializationOf`, `prov:alternateOf`, the
//! `skos:*Match` family, `rdfs:seeAlso` — are never identity: no policy turns
//! them into `eq-*` premises ([`CORRESPONDENCE_PREDICATES`], pinned by
//! `tests/owl2_rl_conformance.rs`).

use serde::{Deserialize, Serialize};

/// The three settings a dataset or organisation can select.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum IdentityPolicy {
    /// `owl:sameAs` stays data: the equality rules do not run, nothing is
    /// propagated across a sameAs link, linksets are not premises.
    #[serde(rename = "sameas-off")]
    Off,
    /// The equality rules run over the dataset's own graphs (two IRIs for one
    /// record inside one registration merge); linkset-role graphs — the
    /// cross-source links — are left out of the premises.
    #[default]
    #[serde(rename = "sameas-narrow")]
    Narrow,
    /// Every `owl:sameAs` the dataset can see propagates, linksets included
    /// (the behaviour before the policy existed).
    #[serde(rename = "sameas-full")]
    Full,
}

impl IdentityPolicy {
    /// The accepted setting strings, in the order shown to users.
    pub const ALL: &'static [&'static str] = &["sameas-off", "sameas-narrow", "sameas-full"];

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "sameas-off" | "off" => Some(Self::Off),
            "sameas-narrow" | "narrow" => Some(Self::Narrow),
            "sameas-full" | "full" => Some(Self::Full),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Off => "sameas-off",
            Self::Narrow => "sameas-narrow",
            Self::Full => "sameas-full",
        }
    }

    /// Whether the OWL 2 RL equality rules (`eq-sym`, `eq-trans`, `eq-rep-*`)
    /// run at all.
    pub fn propagates_same_as(&self) -> bool {
        !matches!(self, Self::Off)
    }

    /// Whether `linkset`-role graphs count as reasoning premises.
    pub fn includes_linksets(&self) -> bool {
        matches!(self, Self::Full)
    }

    /// One-line, user-facing description (shown by the settings endpoints).
    pub fn description(&self) -> &'static str {
        match self {
            Self::Off => "owl:sameAs is kept as plain data: nothing is propagated across it and linksets are not reasoning premises.",
            Self::Narrow => "owl:sameAs propagates only inside the dataset's own graphs (two IRIs for one record); linksets — cross-source links — are not reasoning premises.",
            Self::Full => "every owl:sameAs the dataset can see propagates every property, linksets included.",
        }
    }
}

/// Predicates that express a typed correspondence between two resources
/// without claiming identity. They never feed the equality rules, whatever the
/// policy: `prov:specializationOf` (a model element specialises the asset),
/// `prov:alternateOf` (representations of one thing in different contexts),
/// the SKOS mapping family and `rdfs:seeAlso`.
pub const CORRESPONDENCE_PREDICATES: &[&str] = &[
    "http://www.w3.org/ns/prov#specializationOf",
    "http://www.w3.org/ns/prov#alternateOf",
    "http://www.w3.org/2004/02/skos/core#exactMatch",
    "http://www.w3.org/2004/02/skos/core#closeMatch",
    "http://www.w3.org/2004/02/skos/core#broadMatch",
    "http://www.w3.org/2004/02/skos/core#narrowMatch",
    "http://www.w3.org/2004/02/skos/core#relatedMatch",
    "http://www.w3.org/2000/01/rdf-schema#seeAlso",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_listed_setting_and_rejects_the_rest() {
        for s in IdentityPolicy::ALL {
            let p = IdentityPolicy::parse(s).unwrap();
            assert_eq!(p.as_str(), *s);
        }
        assert_eq!(
            IdentityPolicy::parse("SameAs-Full"),
            Some(IdentityPolicy::Full)
        );
        assert_eq!(IdentityPolicy::parse("sameas-maybe"), None);
        assert_eq!(IdentityPolicy::default(), IdentityPolicy::Narrow);
    }

    #[test]
    fn narrow_propagates_without_linksets_off_propagates_nothing() {
        assert!(!IdentityPolicy::Off.propagates_same_as());
        assert!(!IdentityPolicy::Off.includes_linksets());
        assert!(IdentityPolicy::Narrow.propagates_same_as());
        assert!(!IdentityPolicy::Narrow.includes_linksets());
        assert!(IdentityPolicy::Full.propagates_same_as());
        assert!(IdentityPolicy::Full.includes_linksets());
    }
}
