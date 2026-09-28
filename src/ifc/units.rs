//! IFC units → QUDT. An `IfcProject` carries an `IfcUnitAssignment` naming
//! the project default per unit type (length, area, mass, …); a quantity or
//! property may override it with its own `Unit`. Both resolve here to a QUDT
//! unit IRI — or, when the unit is not in the table, to a label and (for a
//! conversion-based unit) its factor against a unit that is. Nothing is ever
//! guessed: a wrong `qudt:hasUnit` silently corrupts every conversion
//! downstream, a missing one is honest and counted.
//!
//! The IRIs are QUDT 2.1 (`http://qudt.org/vocab/unit/`). QUDT does retire
//! IRIs between versions, so the table names its version.
//!
//! The `IfcSIUnitName`, `IfcSIPrefix` and `IfcUnitEnum` literals below are the
//! IFC schema's enumeration values, unchanged, © buildingSMART International
//! Ltd., used under its copyright notice, which requires full attribution (see
//! `names.rs`).

use std::collections::HashMap;

use super::step::{Arg, Instance, StepFile};

/// QUDT 2.1 unit IRIs.
pub const QUDT_UNIT: &str = "http://qudt.org/vocab/unit/";

/// SI unit names (`IfcSIUnitName`) without a prefix.
const SI: &[(&str, &str)] = &[
    ("METRE", "M"),
    ("SQUARE_METRE", "M2"),
    ("CUBIC_METRE", "M3"),
    ("GRAM", "GM"),
    ("SECOND", "SEC"),
    ("AMPERE", "A"),
    ("KELVIN", "K"),
    ("MOLE", "MOL"),
    ("CANDELA", "CD"),
    ("RADIAN", "RAD"),
    ("STERADIAN", "SR"),
    ("HERTZ", "HZ"),
    ("NEWTON", "N"),
    ("PASCAL", "PA"),
    ("JOULE", "J"),
    ("WATT", "W"),
    ("COULOMB", "C"),
    ("VOLT", "V"),
    ("FARAD", "FARAD"),
    ("OHM", "OHM"),
    ("SIEMENS", "S"),
    ("WEBER", "WB"),
    ("TESLA", "T"),
    ("HENRY", "H"),
    ("DEGREE_CELSIUS", "DEG_C"),
    ("LUMEN", "LM"),
    ("LUX", "LUX"),
    ("BECQUEREL", "BQ"),
    ("GRAY", "GRAY"),
    ("SIEVERT", "SV"),
];

/// Prefixed SI units. The prefix composes with the whole unit: KILO + GRAM is
/// the kilogram (`KiloGM`), MILLI + SQUARE_METRE the square millimetre
/// (`MilliM2`), not a milli-square-metre. Only combinations QUDT 2.1 names
/// are listed; anything else stays unmapped rather than mis-mapped.
const PREFIXED: &[(&str, &str, &str)] = &[
    ("KILO", "GRAM", "KiloGM"),
    ("MILLI", "GRAM", "MilliGM"),
    ("MICRO", "GRAM", "MicroGM"),
    ("KILO", "METRE", "KiloM"),
    ("DECI", "METRE", "DeciM"),
    ("CENTI", "METRE", "CentiM"),
    ("MILLI", "METRE", "MilliM"),
    ("MICRO", "METRE", "MicroM"),
    ("NANO", "METRE", "NanoM"),
    ("KILO", "SQUARE_METRE", "KiloM2"),
    ("CENTI", "SQUARE_METRE", "CentiM2"),
    ("MILLI", "SQUARE_METRE", "MilliM2"),
    ("DECI", "CUBIC_METRE", "DeciM3"),
    ("CENTI", "CUBIC_METRE", "CentiM3"),
    ("MILLI", "CUBIC_METRE", "MilliM3"),
    ("MILLI", "SECOND", "MilliSEC"),
    ("MICRO", "SECOND", "MicroSEC"),
    ("HECTO", "PASCAL", "HectoPA"),
    ("KILO", "PASCAL", "KiloPA"),
    ("MEGA", "PASCAL", "MegaPA"),
    ("GIGA", "PASCAL", "GigaPA"),
    ("KILO", "NEWTON", "KiloN"),
    ("MEGA", "NEWTON", "MegaN"),
    ("KILO", "JOULE", "KiloJ"),
    ("MEGA", "JOULE", "MegaJ"),
    ("MILLI", "WATT", "MilliW"),
    ("KILO", "WATT", "KiloW"),
    ("MEGA", "WATT", "MegaW"),
    ("KILO", "HERTZ", "KiloHZ"),
    ("MEGA", "HERTZ", "MegaHZ"),
    ("GIGA", "HERTZ", "GigaHZ"),
    ("MILLI", "AMPERE", "MilliA"),
    ("KILO", "VOLT", "KiloV"),
    ("MILLI", "VOLT", "MilliV"),
    ("KILO", "OHM", "KiloOHM"),
    ("MEGA", "OHM", "MegaOHM"),
];

/// Conversion-based units by the name exporters conventionally give them
/// (compared upper-cased with spaces and underscores removed). Time and angle
/// "minute" are told apart by the unit type.
const CONVERSION: &[(&str, &str, &str)] = &[
    ("INCH", "LENGTHUNIT", "IN"),
    ("FOOT", "LENGTHUNIT", "FT"),
    ("YARD", "LENGTHUNIT", "YD"),
    ("MILE", "LENGTHUNIT", "MI"),
    ("SQUAREINCH", "AREAUNIT", "IN2"),
    ("SQUAREFOOT", "AREAUNIT", "FT2"),
    ("SQUAREYARD", "AREAUNIT", "YD2"),
    ("ACRE", "AREAUNIT", "AC"),
    ("SQUAREMILE", "AREAUNIT", "MI2"),
    ("CUBICINCH", "VOLUMEUNIT", "IN3"),
    ("CUBICFOOT", "VOLUMEUNIT", "FT3"),
    ("CUBICYARD", "VOLUMEUNIT", "YD3"),
    ("LITRE", "VOLUMEUNIT", "L"),
    ("LITER", "VOLUMEUNIT", "L"),
    ("POUND", "MASSUNIT", "LB"),
    ("OUNCE", "MASSUNIT", "OZ"),
    ("DEGREE", "PLANEANGLEUNIT", "DEG"),
    ("MINUTE", "PLANEANGLEUNIT", "ARCMIN"),
    ("SECOND", "PLANEANGLEUNIT", "ARCSEC"),
    ("MINUTE", "TIMEUNIT", "MIN"),
    ("HOUR", "TIMEUNIT", "HR"),
    ("DAY", "TIMEUNIT", "DAY"),
    ("FAHRENHEIT", "THERMODYNAMICTEMPERATUREUNIT", "DEG_F"),
    ("DEGREEFAHRENHEIT", "THERMODYNAMICTEMPERATUREUNIT", "DEG_F"),
];

/// What a unit resolved to.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Unit {
    /// The QUDT unit IRI, when the unit is in the table.
    pub qudt: Option<String>,
    /// The IFC unit as written (`KILO GRAM`, `FURLONG`), always present.
    pub label: String,
    /// For an unmapped conversion-based unit: `(factor, base)` — the value in
    /// this unit times `factor` is the value in `base`, itself a QUDT IRI when
    /// resolvable.
    pub conversion: Option<(f64, Option<String>)>,
    /// The IFC unit type (`LENGTHUNIT`, …), when known.
    pub unit_type: Option<String>,
}

fn enum_arg(inst: &Instance, idx: usize) -> Option<&str> {
    match inst.args.get(idx) {
        Some(Arg::Enum(e)) => Some(e.as_str()),
        _ => None,
    }
}

fn qudt(local: &str) -> String {
    format!("{QUDT_UNIT}{local}")
}

/// Resolve one unit instance (`IfcSIUnit`, `IfcConversionBasedUnit`; anything
/// else — derived units, monetary units — is unmapped with its entity name
/// as the label).
pub fn resolve(file: &StepFile, unit_id: u64) -> Option<Unit> {
    let inst = file.get(unit_id)?;
    match inst.entity.as_str() {
        "IFCSIUNIT" => {
            // (Dimensions, UnitType, Prefix, Name)
            let unit_type = enum_arg(inst, 1).map(str::to_string);
            let prefix = enum_arg(inst, 2);
            let name = enum_arg(inst, 3)?;
            let local = match prefix {
                None => SI.iter().find(|(n, _)| *n == name).map(|(_, q)| *q),
                Some(p) => PREFIXED
                    .iter()
                    .find(|(pp, n, _)| *pp == p && *n == name)
                    .map(|(_, _, q)| *q),
            };
            Some(Unit {
                qudt: local.map(qudt),
                label: match prefix {
                    Some(p) => format!("{p} {name}"),
                    None => name.to_string(),
                },
                conversion: None,
                unit_type,
            })
        }
        "IFCCONVERSIONBASEDUNIT" => {
            // (Dimensions, UnitType, Name, ConversionFactor)
            let unit_type = enum_arg(inst, 1).map(str::to_string);
            let name = inst
                .args
                .get(2)
                .and_then(Arg::as_str)
                .unwrap_or("")
                .to_string();
            let key: String = name
                .to_ascii_uppercase()
                .chars()
                .filter(|c| !c.is_whitespace() && *c != '_')
                .collect();
            let local = CONVERSION
                .iter()
                .find(|(n, t, _)| *n == key && unit_type.as_deref() == Some(*t))
                .map(|(_, _, q)| *q);
            let conversion = if local.is_none() {
                // IfcMeasureWithUnit(ValueComponent, UnitComponent)
                inst.args
                    .get(3)
                    .and_then(Arg::as_ref_id)
                    .and_then(|m| file.get(m))
                    .and_then(|m| {
                        let factor = m.args.first().and_then(Arg::as_f64)?;
                        let base = m
                            .args
                            .get(1)
                            .and_then(Arg::as_ref_id)
                            .and_then(|b| resolve(file, b))
                            .and_then(|u| u.qudt);
                        Some((factor, base))
                    })
            } else {
                None
            };
            Some(Unit {
                qudt: local.map(qudt),
                label: name,
                conversion,
                unit_type,
            })
        }
        other => Some(Unit {
            qudt: None,
            label: other.to_string(),
            conversion: None,
            unit_type: enum_arg(inst, 1).map(str::to_string),
        }),
    }
}

/// The project's default unit per unit type (`LENGTHUNIT` → …), from
/// `IfcProject.UnitsInContext` → `IfcUnitAssignment.Units`.
pub fn project_defaults(file: &StepFile) -> HashMap<String, Unit> {
    let mut out = HashMap::new();
    let Some(project) = file.of_entity("IFCPROJECT").next() else {
        return out;
    };
    // IFC2x3 and IFC4 both keep UnitsInContext as the last attribute (8).
    let Some(assignment) = project
        .args
        .get(8)
        .and_then(Arg::as_ref_id)
        .and_then(|id| file.get(id))
    else {
        return out;
    };
    let Some(units) = assignment.args.first().and_then(Arg::as_list) else {
        return out;
    };
    for unit_id in units.iter().filter_map(Arg::as_ref_id) {
        if let Some(u) = resolve(file, unit_id) {
            if let Some(t) = &u.unit_type {
                out.entry(t.clone()).or_insert(u);
            }
        }
    }
    out
}

/// The unit type a quantity entity measures in, for the project default.
pub fn quantity_unit_type(entity: &str) -> Option<&'static str> {
    Some(match entity {
        "IFCQUANTITYLENGTH" => "LENGTHUNIT",
        "IFCQUANTITYAREA" => "AREAUNIT",
        "IFCQUANTITYVOLUME" => "VOLUMEUNIT",
        "IFCQUANTITYWEIGHT" => "MASSUNIT",
        "IFCQUANTITYTIME" => "TIMEUNIT",
        _ => return None,
    })
}

/// The unit type an IFC measure type is expressed in (`IfcLengthMeasure` →
/// `LENGTHUNIT`), so a typed property value without its own unit takes the
/// project default. Only the common measures; anything else is unitless.
pub fn measure_unit_type(measure: &str) -> Option<&'static str> {
    let m = measure.to_ascii_uppercase();
    Some(match m.as_str() {
        "IFCLENGTHMEASURE" | "IFCPOSITIVELENGTHMEASURE" | "IFCNONNEGATIVELENGTHMEASURE" => {
            "LENGTHUNIT"
        }
        "IFCAREAMEASURE" => "AREAUNIT",
        "IFCVOLUMEMEASURE" => "VOLUMEUNIT",
        "IFCMASSMEASURE" => "MASSUNIT",
        "IFCTIMEMEASURE" => "TIMEUNIT",
        "IFCPLANEANGLEMEASURE" | "IFCPOSITIVEPLANEANGLEMEASURE" => "PLANEANGLEUNIT",
        "IFCTHERMODYNAMICTEMPERATUREMEASURE" => "THERMODYNAMICTEMPERATUREUNIT",
        "IFCFORCEMEASURE" => "FORCEUNIT",
        "IFCPRESSUREMEASURE" => "PRESSUREUNIT",
        "IFCPOWERMEASURE" => "POWERUNIT",
        "IFCENERGYMEASURE" => "ENERGYUNIT",
        "IFCFREQUENCYMEASURE" => "FREQUENCYUNIT",
        "IFCELECTRICCURRENTMEASURE" => "ELECTRICCURRENTUNIT",
        "IFCELECTRICVOLTAGEMEASURE" => "ELECTRICVOLTAGEUNIT",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ifc::step::parse;

    const UNITS: &str = "ISO-10303-21;\nHEADER;\nFILE_SCHEMA(('IFC4'));\nENDSEC;\nDATA;\n\
#1= IFCPROJECT('0AAAAAAAAAAAAAAAAAAAP1',$,'P',$,$,$,$,(),#10);\n\
#10= IFCUNITASSIGNMENT((#11,#12,#13,#14,#15,#16));\n\
#11= IFCSIUNIT(*,.LENGTHUNIT.,.MILLI.,.METRE.);\n\
#12= IFCSIUNIT(*,.AREAUNIT.,$,.SQUARE_METRE.);\n\
#13= IFCSIUNIT(*,.MASSUNIT.,.KILO.,.GRAM.);\n\
#14= IFCSIUNIT(*,.VOLUMEUNIT.,.MILLI.,.CUBIC_METRE.);\n\
#15= IFCCONVERSIONBASEDUNIT(#20,.PLANEANGLEUNIT.,'DEGREE',#21);\n\
#16= IFCCONVERSIONBASEDUNIT(#20,.LENGTHUNIT.,'FURLONG',#22);\n\
#17= IFCSIUNIT(*,.LENGTHUNIT.,$,.METRE.);\n\
#18= IFCSIUNIT(*,.PLANEANGLEUNIT.,$,.RADIAN.);\n\
#19= IFCSIUNIT(*,.LENGTHUNIT.,.DECA.,.METRE.);\n\
#20= IFCDIMENSIONALEXPONENTS(0,0,0,0,0,0,0);\n\
#21= IFCMEASUREWITHUNIT(IFCPLANEANGLEMEASURE(0.0174532925199433),#18);\n\
#22= IFCMEASUREWITHUNIT(IFCLENGTHMEASURE(201.168),#17);\n\
ENDSEC;\nEND-ISO-10303-21;";

    #[test]
    fn si_prefixes_compose_with_the_whole_unit() {
        let f = parse(UNITS).unwrap();
        let d = project_defaults(&f);
        assert_eq!(d["LENGTHUNIT"].qudt.as_deref(), Some(&*qudt("MilliM")));
        assert_eq!(d["AREAUNIT"].qudt.as_deref(), Some(&*qudt("M2")));
        // The kilogram trap: KILO + GRAM is the kilogram, not a "kilo-gram".
        assert_eq!(d["MASSUNIT"].qudt.as_deref(), Some(&*qudt("KiloGM")));
        assert_eq!(d["MASSUNIT"].label, "KILO GRAM");
        // MILLI + CUBIC_METRE is the cubic millimetre.
        assert_eq!(d["VOLUMEUNIT"].qudt.as_deref(), Some(&*qudt("MilliM3")));
        // A conversion-based degree maps by name and unit type.
        assert_eq!(d["PLANEANGLEUNIT"].qudt.as_deref(), Some(&*qudt("DEG")));
        assert!(d["PLANEANGLEUNIT"].conversion.is_none());
    }

    #[test]
    fn an_unknown_unit_is_never_guessed() {
        let f = parse(UNITS).unwrap();
        // A furlong: no QUDT IRI, but the factor against the metre survives.
        let u = resolve(&f, 16).unwrap();
        assert!(u.qudt.is_none());
        assert_eq!(u.label, "FURLONG");
        let (factor, base) = u.conversion.clone().unwrap();
        assert!((factor - 201.168).abs() < 1e-9);
        assert_eq!(base.as_deref(), Some(&*qudt("M")));
        // A prefix combination QUDT does not name stays unmapped.
        let u = resolve(&f, 19).unwrap();
        assert!(u.qudt.is_none());
        assert_eq!(u.label, "DECA METRE");
        assert_eq!(u.unit_type.as_deref(), Some("LENGTHUNIT"));
    }

    #[test]
    fn measures_and_quantities_name_their_unit_type() {
        assert_eq!(
            measure_unit_type("IfcPositiveLengthMeasure"),
            Some("LENGTHUNIT")
        );
        assert_eq!(measure_unit_type("IFCAREAMEASURE"), Some("AREAUNIT"));
        assert_eq!(measure_unit_type("IfcLabel"), None);
        assert_eq!(quantity_unit_type("IFCQUANTITYWEIGHT"), Some("MASSUNIT"));
        assert_eq!(quantity_unit_type("IFCQUANTITYCOUNT"), None);
    }
}
