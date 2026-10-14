//! The controlled vocabularies the catalogue's values come from — the EU
//! Publications Office authority tables (data theme, dataset status, access
//! right, frequency) and the ADMS status and publisher-type vocabularies —
//! with the `skos:prefLabel`s DCAT-AP needs on every `skos:Concept` it
//! references (`dcat:theme`, `adms:status`, an agent's `dct:type`).
//!
//! Only the concepts the catalogue can emit are listed. The labels are the
//! vocabularies' own `skos:prefLabel`s: English and Dutch from the EU
//! authority tables (EU Vocabularies, © European Union, reused under
//! Commission Decision 2011/833/EU; see NOTICE), English only from ADMS. An IRI outside these tables is passed through as given; the
//! catalogue then reports the missing label as a profile warning instead of
//! inventing one.

pub const EU_THEME_SCHEME: &str = "http://publications.europa.eu/resource/authority/data-theme";
pub const EU_STATUS: &str = "http://publications.europa.eu/resource/authority/dataset-status/";
pub const EU_ACCESS: &str = "http://publications.europa.eu/resource/authority/access-right/";
pub const EU_FREQUENCY: &str = "http://publications.europa.eu/resource/authority/frequency/";
pub const ADMS_PUBLISHER_TYPE: &str = "http://purl.org/adms/publishertype/";

/// A concept of a controlled vocabulary: its IRI and English / Dutch labels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Concept {
    pub iri: &'static str,
    pub en: &'static str,
    pub nl: &'static str,
}

impl Concept {
    /// The label and its language tag for a catalogue in `lang`: Dutch when
    /// asked for and the vocabulary has it, English otherwise.
    pub fn label(&self, lang: &str) -> (&'static str, &'static str) {
        if lang == "nl" && !self.nl.is_empty() {
            (self.nl, "nl")
        } else {
            (self.en, "en")
        }
    }
}

macro_rules! concepts {
    ($($iri:expr, $en:expr, $nl:expr;)*) => { &[$(Concept { iri: $iri, en: $en, nl: $nl }),*] };
}

/// EU data themes (`dcat:theme`).
pub const THEMES: &[Concept] = concepts![
    "http://publications.europa.eu/resource/authority/data-theme/AGRI", "Agriculture, fisheries, forestry and food", "Landbouw, visserij, bosbouw en voeding";
    "http://publications.europa.eu/resource/authority/data-theme/ECON", "Economy and finance", "Economie en financiën";
    "http://publications.europa.eu/resource/authority/data-theme/EDUC", "Education, culture and sport", "Onderwijs, cultuur en sport";
    "http://publications.europa.eu/resource/authority/data-theme/ENER", "Energy", "Energie";
    "http://publications.europa.eu/resource/authority/data-theme/ENVI", "Environment", "Milieu";
    "http://publications.europa.eu/resource/authority/data-theme/GOVE", "Government and public sector", "Overheid en publieke sector";
    "http://publications.europa.eu/resource/authority/data-theme/HEAL", "Health", "Gezondheid";
    "http://publications.europa.eu/resource/authority/data-theme/INTR", "International issues", "Internationale vraagstukken";
    "http://publications.europa.eu/resource/authority/data-theme/JUST", "Justice, legal system and public safety", "Justitie, rechtsstelsel en openbare veiligheid";
    "http://publications.europa.eu/resource/authority/data-theme/REGI", "Regions and cities", "Regio's en steden";
    "http://publications.europa.eu/resource/authority/data-theme/SOCI", "Population and society", "Bevolking en samenleving";
    "http://publications.europa.eu/resource/authority/data-theme/TECH", "Science and technology", "Wetenschap en technologie";
    "http://publications.europa.eu/resource/authority/data-theme/TRAN", "Transport", "Vervoer";
];

/// Dataset statuses: the EU dataset-status table and the ADMS status
/// vocabulary the web UI offers.
pub const STATUSES: &[Concept] = concepts![
    "http://publications.europa.eu/resource/authority/dataset-status/COMPLETED", "completed", "voltooid";
    "http://publications.europa.eu/resource/authority/dataset-status/DEPRECATED", "deprecated", "verouderd";
    "http://publications.europa.eu/resource/authority/dataset-status/DEVELOP", "under development", "in ontwikkeling";
    "http://publications.europa.eu/resource/authority/dataset-status/DISCONT", "discontinued", "stopgezet";
    "http://publications.europa.eu/resource/authority/dataset-status/WITHDRAWN", "withdrawn", "ingetrokken";
    "http://purl.org/adms/status/Completed", "Completed", "";
    "http://purl.org/adms/status/UnderDevelopment", "Under Development", "";
    "http://purl.org/adms/status/Deprecated", "Deprecated", "";
    "http://purl.org/adms/status/Withdrawn", "Withdrawn", "";
];

/// EU access rights (`dct:accessRights`).
pub const ACCESS_RIGHTS: &[Concept] = concepts![
    "http://publications.europa.eu/resource/authority/access-right/PUBLIC", "public", "openbaar";
    "http://publications.europa.eu/resource/authority/access-right/RESTRICTED", "restricted", "beperkt";
    "http://publications.europa.eu/resource/authority/access-right/NON_PUBLIC", "non-public", "niet-openbaar";
];

/// EU frequencies (`dct:accrualPeriodicity`).
pub const FREQUENCIES: &[Concept] = concepts![
    "http://publications.europa.eu/resource/authority/frequency/CONT", "continuous", "voortdurend";
    "http://publications.europa.eu/resource/authority/frequency/UPDATE_CONT", "continuously updated", "voortdurend geactualiseerd";
    "http://publications.europa.eu/resource/authority/frequency/HOURLY", "hourly", "om het uur";
    "http://publications.europa.eu/resource/authority/frequency/DAILY_2", "twice a day", "tweemaal per dag";
    "http://publications.europa.eu/resource/authority/frequency/DAILY", "daily", "dagelijks";
    "http://publications.europa.eu/resource/authority/frequency/WEEKLY_3", "three times a week", "drie keer per week";
    "http://publications.europa.eu/resource/authority/frequency/WEEKLY_2", "semiweekly", "twee keer per week";
    "http://publications.europa.eu/resource/authority/frequency/WEEKLY", "weekly", "wekelijks";
    "http://publications.europa.eu/resource/authority/frequency/BIWEEKLY", "biweekly", "veertiendaags";
    "http://publications.europa.eu/resource/authority/frequency/MONTHLY_3", "three times a month", "drie keer per maand";
    "http://publications.europa.eu/resource/authority/frequency/MONTHLY_2", "semimonthly", "twee keer per maand";
    "http://publications.europa.eu/resource/authority/frequency/MONTHLY", "monthly", "maandelijks";
    "http://publications.europa.eu/resource/authority/frequency/BIMONTHLY", "bimonthly", "tweemaandelijks";
    "http://publications.europa.eu/resource/authority/frequency/QUARTERLY", "quarterly", "driemaandelijks";
    "http://publications.europa.eu/resource/authority/frequency/ANNUAL_3", "three times a year", "drie keer per jaar";
    "http://publications.europa.eu/resource/authority/frequency/ANNUAL_2", "semiannual", "halfjaarlijks";
    "http://publications.europa.eu/resource/authority/frequency/ANNUAL", "annual", "jaarlijks";
    "http://publications.europa.eu/resource/authority/frequency/BIENNIAL", "biennial", "tweejaarlijks";
    "http://publications.europa.eu/resource/authority/frequency/TRIENNIAL", "triennial", "driejaarlijks";
    "http://publications.europa.eu/resource/authority/frequency/QUADRENNIAL", "quadrennial", "om de vier jaar";
    "http://publications.europa.eu/resource/authority/frequency/QUINQUENNIAL", "quinquennial", "om de vijf jaar";
    "http://publications.europa.eu/resource/authority/frequency/DECENNIAL", "decennial", "om de tien jaar";
    "http://publications.europa.eu/resource/authority/frequency/IRREG", "irregular", "onregelmatig";
    "http://publications.europa.eu/resource/authority/frequency/AS_NEEDED", "as needed", "voor zover nodig";
    "http://publications.europa.eu/resource/authority/frequency/NOT_PLANNED", "not planned", "niet gepland";
    "http://publications.europa.eu/resource/authority/frequency/NEVER", "never", "nooit";
    "http://publications.europa.eu/resource/authority/frequency/UNKNOWN", "unknown", "onbekend";
    "http://publications.europa.eu/resource/authority/frequency/OTHER", "other", "overige";
];

/// ADMS publisher types (an agent's `dct:type`).
pub const PUBLISHER_TYPES: &[Concept] = concepts![
    "http://purl.org/adms/publishertype/Academia-ScientificOrganisation", "Academia/Scientific organisation", "";
    "http://purl.org/adms/publishertype/Company", "Company", "";
    "http://purl.org/adms/publishertype/IndustryConsortium", "Industry consortium", "";
    "http://purl.org/adms/publishertype/LocalAuthority", "Local Authority", "";
    "http://purl.org/adms/publishertype/NationalAuthority", "National authority", "";
    "http://purl.org/adms/publishertype/NonGovernmentalOrganisation", "Non-Governmental Organisation", "";
    "http://purl.org/adms/publishertype/NonProfitOrganisation", "Non-Profit Organisation", "";
    "http://purl.org/adms/publishertype/PrivateIndividual(s)", "Private Individual(s)", "";
    "http://purl.org/adms/publishertype/RegionalAuthority", "Regional authority", "";
    "http://purl.org/adms/publishertype/StandardisationBody", "Standardisation body", "";
    "http://purl.org/adms/publishertype/SupraNationalAuthority", "Supra-national authority", "";
];

/// ADMS asset types (a registry entry's `dct:type`).
pub const ASSET_TYPES: &[Concept] = concepts![
    "http://purl.org/adms/assettype/CodeList", "Code List", "";
    "http://purl.org/adms/assettype/Ontology", "Ontology", "";
];

/// The publisher type of a person: ADMS `PrivateIndividual(s)`.
pub const PRIVATE_INDIVIDUAL: &str = "http://purl.org/adms/publishertype/PrivateIndividual(s)";

/// The labelled concept with this IRI, in any of the tables.
pub fn lookup(iri: &str) -> Option<&'static Concept> {
    [
        THEMES,
        STATUSES,
        ACCESS_RIGHTS,
        FREQUENCIES,
        PUBLISHER_TYPES,
        ASSET_TYPES,
    ]
    .into_iter()
    .flatten()
    .find(|c| c.iri == iri)
}

/// Normalise a code (`ANNUAL`, `annual`) or IRI against a table whose IRIs
/// share `ns`: a known code becomes its IRI; an absolute IRI is returned as
/// given; anything else is `None`.
fn code_or_iri(table: &[Concept], ns: &str, value: &str) -> Option<String> {
    let v = value.trim();
    if v.is_empty() {
        return None;
    }
    if v.contains("://") || v.starts_with("urn:") {
        return Some(v.to_string());
    }
    let key = v.to_ascii_uppercase().replace([' ', '-'], "_");
    table
        .iter()
        .find(|c| c.iri.strip_prefix(ns) == Some(key.as_str()))
        .map(|c| c.iri.to_string())
}

/// A frequency code (`ANNUAL`) or IRI, as an IRI.
pub fn frequency_iri(value: &str) -> Option<String> {
    code_or_iri(FREQUENCIES, EU_FREQUENCY, value)
}

/// A publisher-type code (`LocalAuthority`) or IRI, as an IRI.
pub fn publisher_type_iri(value: &str) -> Option<String> {
    let v = value.trim();
    if v.contains("://") {
        return Some(v.to_string());
    }
    PUBLISHER_TYPES
        .iter()
        .find(|c| {
            c.iri
                .strip_prefix(ADMS_PUBLISHER_TYPE)
                .is_some_and(|code| code.eq_ignore_ascii_case(v))
        })
        .map(|c| c.iri.to_string())
}

/// A dataset status — an EU dataset-status code (`COMPLETED`, `DEVELOP`), an
/// ADMS-style alias (`under development`, `discontinued`) or an IRI — as an
/// IRI.
pub fn status_iri(value: &str) -> Option<String> {
    let v = value.trim();
    if v.is_empty() {
        return None;
    }
    if v.contains("://") || v.starts_with("urn:") {
        return Some(v.to_string());
    }
    let key = v.to_ascii_uppercase().replace([' ', '-'], "_");
    let code = match key.as_str() {
        "UNDER_DEVELOPMENT" | "UNDERDEVELOPMENT" | "DEVELOPMENT" => "DEVELOP",
        "DISCONTINUED" => "DISCONT",
        other => other,
    };
    code_or_iri(STATUSES, EU_STATUS, code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_map_onto_the_authority_tables() {
        assert_eq!(
            frequency_iri("annual").as_deref(),
            Some("http://publications.europa.eu/resource/authority/frequency/ANNUAL")
        );
        assert_eq!(frequency_iri("fortnightly"), None);
        assert_eq!(
            status_iri("under development").as_deref(),
            Some("http://publications.europa.eu/resource/authority/dataset-status/DEVELOP")
        );
        assert_eq!(
            status_iri("http://purl.org/adms/status/Completed").as_deref(),
            Some("http://purl.org/adms/status/Completed")
        );
        assert_eq!(
            publisher_type_iri("localauthority").as_deref(),
            Some("http://purl.org/adms/publishertype/LocalAuthority")
        );
        assert_eq!(
            lookup(PRIVATE_INDIVIDUAL).unwrap().label("nl"),
            ("Private Individual(s)", "en")
        );
        assert_eq!(
            lookup("http://publications.europa.eu/resource/authority/data-theme/TRAN")
                .unwrap()
                .label("nl"),
            ("Vervoer", "nl")
        );
    }
}
