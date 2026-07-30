//! Administrative place inference: country → region → city for viewer elements.
//!
//! The viewer's sidebar can group elements by where they are, which needs a
//! place hierarchy the data does not usually state outright. This module infers
//! one **from the RDF itself**, in descending order of trust:
//!
//! 1. **Explicit place statements** — `schema:addressCountry` / `addressRegion` /
//!    `addressLocality` (also via `schema:address`), `dct:spatial`, and the
//!    ifcOWL lift of `IfcPostalAddress` (`Country` / `Region` / `Town`).
//! 2. **Authority identifiers** reached by `owl:sameAs` — a national register
//!    IRI pins the country, and often the municipality, with certainty. The
//!    Dutch BAG is wired up here: `bag.basisregistraties.overheid.nl/…/pand/0268…`
//!    is Nijmegen, Gelderland, Netherlands, because the first four digits of a
//!    pand id are the CBS municipality code.
//!
//! Each resolved place gets a **stable minted IRI** ([`place_iri`]), nested under
//! its parent so two same-named cities in different regions stay distinct, and
//! carries the well-known public identifier for the same real-world place where
//! one is known (Wikidata) — so the identifier the viewer hands out is a local
//! handle on a real thing rather than a private invention. Materialising those
//! IRIs as `schema:Place` triples in a dataset graph is a separate write-path
//! step and is deliberately NOT done from the read-only feed.
//!
//! Nothing here guesses from coordinates: a lat/lon alone cannot name a country
//! without a boundary dataset, and a *wrong* country silently attached to
//! someone's building is worse than an honest "Ungrouped".

use std::collections::BTreeMap;

/// Where a place sits in the administrative hierarchy, broad → narrow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PlaceLevel {
    Country,
    Region,
    City,
}

impl PlaceLevel {
    /// Slug used in the minted IRI and reported to the viewer feed.
    pub fn slug(self) -> &'static str {
        match self {
            PlaceLevel::Country => "country",
            PlaceLevel::Region => "region",
            PlaceLevel::City => "city",
        }
    }
}

/// One resolved place in an element's hierarchy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    pub level: PlaceLevel,
    pub label: String,
    /// Public identifier for the same real-world place, when one is known —
    /// emitted as `owl:sameAs` on the minted entity.
    pub same_as: Option<String>,
}

impl Place {
    fn new(level: PlaceLevel, label: &str, same_as: Option<&str>) -> Self {
        Place {
            level,
            label: label.to_string(),
            same_as: same_as.map(str::to_string),
        }
    }
}

/// Slug a label into an IRI-safe segment: lowercase, non-alphanumerics folded to
/// single hyphens. Two labels that differ only in punctuation or case therefore
/// mint the SAME entity, which is the point — "Baden-Württemberg" must not
/// become two provinces because one source wrote "Baden Wurttemberg".
pub fn slugify(label: &str) -> String {
    let mut out = String::with_capacity(label.len());
    let mut pending_sep = false;
    for ch in label.chars() {
        if ch.is_alphanumeric() {
            if pending_sep && !out.is_empty() {
                out.push('-');
            }
            pending_sep = false;
            for lower in ch.to_lowercase() {
                out.push(lower);
            }
        } else {
            pending_sep = true;
        }
    }
    out
}

/// The IRI minted for a place, stable across runs: `{base}/places/{level}/{slug}`,
/// nested under its parent so two "Springfield"s in different states stay apart.
pub fn place_iri(base_url: &str, path: &[Place]) -> String {
    let base = base_url.trim_end_matches('/');
    let mut iri = format!("{base}/places");
    for p in path {
        iri.push('/');
        iri.push_str(p.level.slug());
        iri.push('/');
        iri.push_str(&slugify(&p.label));
    }
    iri
}

// ── Authority identifiers ───────────────────────────────────────────────────

/// CBS municipality codes seen in Dutch BAG identifiers, mapped to their
/// municipality and province. The BAG pand id encodes the municipality in its
/// first four digits, so this is a lookup, not a guess. Extend as datasets
/// bring new municipalities in; an unknown code still yields the country.
const NL_MUNICIPALITIES: &[(&str, &str, &str, &str)] = &[
    // (CBS code, municipality, province, Wikidata id for the municipality)
    (
        "0268",
        "Nijmegen",
        "Gelderland",
        "https://www.wikidata.org/entity/Q9807",
    ),
    (
        "0363",
        "Amsterdam",
        "Noord-Holland",
        "https://www.wikidata.org/entity/Q727",
    ),
    (
        "0599",
        "Rotterdam",
        "Zuid-Holland",
        "https://www.wikidata.org/entity/Q34370",
    ),
    (
        "0518",
        "'s-Gravenhage",
        "Zuid-Holland",
        "https://www.wikidata.org/entity/Q36600",
    ),
    (
        "0344",
        "Utrecht",
        "Utrecht",
        "https://www.wikidata.org/entity/Q803",
    ),
    (
        "0772",
        "Eindhoven",
        "Noord-Brabant",
        "https://www.wikidata.org/entity/Q9832",
    ),
    (
        "0014",
        "Groningen",
        "Groningen",
        "https://www.wikidata.org/entity/Q749",
    ),
    (
        "0503",
        "Delft",
        "Zuid-Holland",
        "https://www.wikidata.org/entity/Q690",
    ),
];

const NL_COUNTRY: (&str, &str) = ("Netherlands", "https://www.wikidata.org/entity/Q55");

/// Resolve a place path from an authority IRI (typically an `owl:sameAs` target).
/// Returns the broadest → narrowest chain, or `None` when the authority is not
/// one we can read.
pub fn place_from_authority(iri: &str) -> Option<Vec<Place>> {
    // Dutch BAG: https://bag.basisregistraties.overheid.nl/bag/id/pand/0268100000007417
    if iri.contains("bag.basisregistraties.overheid.nl") {
        let mut path = vec![Place::new(
            PlaceLevel::Country,
            NL_COUNTRY.0,
            Some(NL_COUNTRY.1),
        )];
        // The pand/verblijfsobject id's leading 4 digits are the CBS code.
        let id = iri.rsplit('/').next().unwrap_or("");
        let digits: String = id.chars().take_while(char::is_ascii_digit).collect();
        if digits.len() >= 4 {
            if let Some((_, town, province, wd)) =
                NL_MUNICIPALITIES.iter().find(|(c, ..)| *c == &digits[..4])
            {
                path.push(Place::new(PlaceLevel::Region, province, None));
                path.push(Place::new(PlaceLevel::City, town, Some(wd)));
            }
        }
        return Some(path);
    }
    None
}

// ── Explicit place statements ───────────────────────────────────────────────

/// Predicate IRI → the level it states. Matched on the full IRI where the
/// vocabulary is unambiguous, and on the local name for the ifcOWL lift (whose
/// namespace carries an IFC-version segment).
fn level_of_predicate(predicate: &str) -> Option<PlaceLevel> {
    let local = predicate
        .rsplit(|c| c == '#' || c == '/')
        .next()
        .unwrap_or(predicate);
    match local {
        "addressCountry" | "country" | "Country" => Some(PlaceLevel::Country),
        "addressRegion" | "region" | "Region" => Some(PlaceLevel::Region),
        "addressLocality" | "town" | "Town" | "locality" => Some(PlaceLevel::City),
        _ => None,
    }
}

/// Assemble a place path from the (predicate, value) pairs stated on an element
/// (or on its address node). Values are taken verbatim as labels; levels that
/// are absent are simply skipped, so "country + city, no region" is a valid,
/// two-deep hierarchy rather than an error.
pub fn place_from_statements<'a, I>(statements: I) -> Vec<Place>
where
    I: IntoIterator<Item = (&'a str, &'a str)>,
{
    let mut by_level: BTreeMap<PlaceLevel, String> = BTreeMap::new();
    for (predicate, value) in statements {
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        if let Some(level) = level_of_predicate(predicate) {
            // First statement per level wins — later duplicates are ignored
            // rather than concatenated into a nonsense label.
            by_level.entry(level).or_insert_with(|| value.to_string());
        }
    }
    // BTreeMap over PlaceLevel iterates Country → Region → City by construction.
    by_level
        .into_iter()
        .map(|(level, label)| Place::new(level, &label, None))
        .collect()
}

/// Merge inferred paths for one element, preferring the longest (most specific)
/// and, at equal length, the first — sources are passed in trust order.
pub fn best_path(candidates: Vec<Vec<Place>>) -> Vec<Place> {
    candidates
        .into_iter()
        .filter(|p| !p.is_empty())
        .max_by_key(|p| p.len())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_folds_case_and_punctuation() {
        assert_eq!(slugify("Baden-Württemberg"), "baden-württemberg");
        assert_eq!(slugify("'s-Gravenhage"), "s-gravenhage");
        assert_eq!(slugify("New  York"), "new-york");
        // The whole point: punctuation/case variants collapse onto one entity.
        assert_eq!(slugify("Noord-Holland"), slugify("noord holland"));
    }

    #[test]
    fn place_iris_nest_under_their_parent() {
        let path = vec![
            Place::new(PlaceLevel::Country, "Netherlands", None),
            Place::new(PlaceLevel::Region, "Gelderland", None),
            Place::new(PlaceLevel::City, "Nijmegen", None),
        ];
        assert_eq!(
            place_iri("https://data.example.org/", &path),
            "https://data.example.org/places/country/netherlands/region/gelderland/city/nijmegen"
        );
        // A same-named city in another region is a DIFFERENT entity.
        let a = vec![
            Place::new(PlaceLevel::Country, "US", None),
            Place::new(PlaceLevel::Region, "Illinois", None),
            Place::new(PlaceLevel::City, "Springfield", None),
        ];
        let b = vec![
            Place::new(PlaceLevel::Country, "US", None),
            Place::new(PlaceLevel::Region, "Missouri", None),
            Place::new(PlaceLevel::City, "Springfield", None),
        ];
        assert_ne!(
            place_iri("https://x.test", &a),
            place_iri("https://x.test", &b)
        );
    }

    #[test]
    fn bag_identifier_resolves_country_region_and_municipality() {
        let path = place_from_authority(
            "https://bag.basisregistraties.overheid.nl/bag/id/pand/0268100000007417",
        )
        .expect("BAG authority is recognised");
        assert_eq!(
            path.iter().map(|p| p.label.as_str()).collect::<Vec<_>>(),
            ["Netherlands", "Gelderland", "Nijmegen"]
        );
        assert_eq!(path[0].level, PlaceLevel::Country);
        assert_eq!(path[2].level, PlaceLevel::City);
        // The minted entities carry public identifiers, not just local IRIs.
        assert_eq!(
            path[0].same_as.as_deref(),
            Some("https://www.wikidata.org/entity/Q55")
        );
        assert!(path[2].same_as.is_some());
    }

    #[test]
    fn unknown_municipality_code_still_yields_the_country() {
        let path = place_from_authority(
            "https://bag.basisregistraties.overheid.nl/bag/id/pand/9999100000000001",
        )
        .expect("still a BAG identifier");
        assert_eq!(path.len(), 1);
        assert_eq!(path[0].label, "Netherlands");
    }

    #[test]
    fn unrecognised_authority_infers_nothing() {
        assert!(place_from_authority("https://example.org/thing/1").is_none());
        // Never guess a country from a coordinate-bearing IRI.
        assert!(place_from_authority("https://opentriplestore.org/demo/x#geom").is_none());
    }

    #[test]
    fn explicit_statements_order_by_level_and_ignore_blanks() {
        let path = place_from_statements([
            ("https://schema.org/addressLocality", "Karlsruhe"),
            ("https://schema.org/addressCountry", "Germany"),
            ("https://schema.org/addressRegion", "  "),
            // ifcOWL's IfcPostalAddress lift, namespace-versioned.
            (
                "https://standards.buildingsmart.org/IFC/DEV/IFC4/ADD2_TC1/OWL#Region",
                "Baden-Württemberg",
            ),
        ]);
        assert_eq!(
            path.iter()
                .map(|p| (p.level, p.label.as_str()))
                .collect::<Vec<_>>(),
            [
                (PlaceLevel::Country, "Germany"),
                (PlaceLevel::Region, "Baden-Württemberg"),
                (PlaceLevel::City, "Karlsruhe"),
            ]
        );
    }

    #[test]
    fn statements_without_any_place_predicate_yield_nothing() {
        assert!(place_from_statements([
            ("http://www.w3.org/2000/01/rdf-schema#label", "A wall"),
            ("https://w3id.org/props#ifcGuid", "1aB2cD"),
        ])
        .is_empty());
    }

    #[test]
    fn best_path_prefers_the_most_specific_candidate() {
        let shallow = vec![Place::new(PlaceLevel::Country, "Netherlands", None)];
        let deep = vec![
            Place::new(PlaceLevel::Country, "Netherlands", None),
            Place::new(PlaceLevel::City, "Nijmegen", None),
        ];
        assert_eq!(best_path(vec![shallow.clone(), deep.clone()]), deep);
        assert_eq!(best_path(vec![vec![], shallow.clone()]), shallow);
        assert!(best_path(vec![vec![], vec![]]).is_empty());
    }
}
