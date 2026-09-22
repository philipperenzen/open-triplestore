//! Term-map evaluation, shared by the file-based and the relational executor.
//!
//! One row in, one RDF term out — serialised in N-Triples form so a caller can
//! concatenate terms into a document and stream it into the store.
//!
//! Two things beyond plain templating live here because both executors need
//! them: the **natural datatype** rule (R2RML §10.2 — a column with no
//! `rr:datatype` takes the XSD type its SQL type implies) and the **FNML
//! function** used for enumerations.

use std::collections::HashMap;

use ots_plugin_api::sources::ValueKind;
use oxigraph::model::{Literal, NamedNode};

use super::model::*;

const XSD: &str = "http://www.w3.org/2001/XMLSchema#";
/// This store's own function vocabulary.
pub const FN_NS: &str = "https://w3id.org/open-triplestore/fn#";

/// The label [`FN_NS`] is documented and rendered under. Not `fn:`, which is
/// the XPath functions namespace in every prefix list and in SPARQL's own
/// specification text; a store that rebound it would expand a user's
/// `fn:concat` to a mapping function.
pub const FN_LABEL: &str = "otsfn";
/// A normalise-and-look-up over a declared value map, for SQL enumerations
/// and code lists.
pub const FN_MAP_VALUE: &str = "https://w3id.org/open-triplestore/fn#mapValue";
/// Mint an IRI from a template whose placeholders are `{column}` or
/// `{column_slug}` — the latter the column's value as an ASCII slug. What the
/// legacy `{value_slug}` placeholder needs, and the one thing an `rr:template`
/// cannot say.
pub const FN_MINT_IRI: &str = "https://w3id.org/open-triplestore/fn#mintIri";

/// A value as an ASCII slug: lower-case letters and digits, runs of anything
/// else folded to one hyphen, no hyphen at either end. `"Rolling Stock (2024)"`
/// becomes `rolling-stock-2024`.
pub fn slug(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut pending = false;
    for c in value.chars() {
        if c.is_ascii_alphanumeric() {
            if pending && !out.is_empty() {
                out.push('-');
            }
            pending = false;
            out.push(c.to_ascii_lowercase());
        } else {
            pending = true;
        }
    }
    out
}

/// Expand a template whose placeholders may be `{column}` (percent-encoded)
/// or `{column_slug}` (slugged). A placeholder the row cannot supply yields
/// `None`, and so no term.
pub fn expand_slug_template(template: &str, row: &Row) -> Option<String> {
    let mut result = String::with_capacity(template.len());
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                if let Some(next) = chars.next() {
                    result.push(next);
                }
            }
            '{' => {
                let mut name = String::new();
                for inner in chars.by_ref() {
                    if inner == '}' {
                        break;
                    }
                    name.push(inner);
                }
                let piece = match row.get(&name) {
                    Some(v) => percent_encode(v),
                    None => {
                        let base = name.strip_suffix("_slug")?;
                        let slugged = slug(row.get(base)?);
                        // A value that slugs to nothing names nothing.
                        (!slugged.is_empty()).then_some(slugged)?
                    }
                };
                result.push_str(&piece);
            }
            _ => result.push(c),
        }
    }
    Some(result)
}

/// `otsfn:mintIri`: an IRI from `otsfn:template` over the current row.
fn mint_iri(f: &FunctionMap, row: &Row) -> Result<Option<String>, String> {
    let template = arg_value(f.first(&format!("{FN_NS}template")), row).ok_or_else(|| {
        format!("{FN_LABEL}:mintIri needs {FN_LABEL}:template with an absolute IRI template")
    })?;
    if !(template.contains("://") || template.starts_with("urn:")) {
        return Err(format!(
            "{FN_LABEL}:template '{template}' is not absolute; minting under an undeclared \
             prefix would put IRIs in a namespace nobody owns"
        ));
    }
    let Some(filled) = expand_slug_template(&template, row) else {
        return Ok(None);
    };
    // An IRI the template cannot make well-formed skips the term, as an
    // `rr:template` does.
    Ok(NamedNode::new(&filled).ok().map(|n| n.to_string()))
}

/// One source row: column name → lexical value. A SQL NULL, an empty CSV cell
/// and an absent JSON key are all "no key", so a term map over one produces
/// no term and therefore no triple.
pub type Row = HashMap<String, String>;
/// Per-column generic types, when the source reports them (relational only).
pub type Kinds = HashMap<String, ValueKind>;

/// Blank-node minting state for one execution. Labels are unique across the
/// whole run, so a document flushed in batches never merges two rows' nodes.
#[derive(Debug, Default)]
pub struct BlankNodes {
    counter: u64,
    prefix: String,
}

impl BlankNodes {
    pub fn new(prefix: impl Into<String>) -> Self {
        Self {
            counter: 0,
            prefix: prefix.into(),
        }
    }
    fn mint(&mut self) -> String {
        self.counter += 1;
        format!("_:{}{}", self.prefix, self.counter)
    }
}

/// The XSD datatype a column's generic type implies.
pub fn natural_datatype(kind: ValueKind) -> Option<&'static str> {
    Some(match kind {
        ValueKind::Integer => "integer",
        ValueKind::Decimal => "decimal",
        ValueKind::Float => "double",
        ValueKind::Boolean => "boolean",
        ValueKind::Date => "date",
        ValueKind::Time => "time",
        ValueKind::DateTime => "dateTime",
        ValueKind::Binary => "hexBinary",
        // Text, UUID, JSON and anything unclassified stay plain literals:
        // inventing a datatype for them would be a claim the source never made.
        ValueKind::Text | ValueKind::Uuid | ValueKind::Json | ValueKind::Other => return None,
    })
}

/// Evaluate a term map against a row.
///
/// Returns the term in N-Triples syntax (`<iri>`, `"lex"^^<dt>`, `_:b1`), or
/// `None` when the row does not supply what the term map needs.
pub fn eval_term(
    tm: &TermMap,
    row: &Row,
    kinds: Option<&Kinds>,
    bnodes: &mut BlankNodes,
    row_bnodes: &mut HashMap<String, String>,
) -> Option<String> {
    let raw_value = match &tm.kind {
        TermMapKind::Constant(val) => val.clone(),
        TermMapKind::Template(template) => expand_template(template, row)?,
        TermMapKind::Reference(col) => row.get(col)?.clone(),
    };

    if raw_value.is_empty() {
        return None;
    }

    Some(match tm.term_type {
        // Build the IRI through oxrdf so it is validated and serialised, not
        // pasted between angle brackets. A `rml:reference`/`rr:column` value
        // with `rr:termType rr:IRI` went in raw: a value containing a space
        // produced invalid Turtle and failed the WHOLE mapping, and one
        // containing `>` could terminate the IRI and inject further triples.
        // Only `rr:template` values were percent-encoded.
        TermType::IRI => NamedNode::new(&raw_value).ok()?.to_string(),
        TermType::BlankNode => {
            if let Some(existing) = row_bnodes.get(&raw_value) {
                existing.clone()
            } else {
                let label = bnodes.mint();
                row_bnodes.insert(raw_value, label.clone());
                label
            }
        }
        // Likewise for literals: hand-escaping only `\` and `"` left raw
        // newlines, carriage returns and tabs in the output, which Turtle's
        // STRING_LITERAL_QUOTE forbids — so one multi-line CSV field made the
        // entire generated document unparseable and the mapping failed with
        // "Failed to load generated triples" rather than skipping a row.
        TermType::Literal => {
            if let Some(ref lang) = tm.language {
                match Literal::new_language_tagged_literal(&raw_value, lang) {
                    Ok(l) => l.to_string(),
                    // An invalid language tag is a mapping error, not a reason
                    // to emit a broken document.
                    Err(_) => return None,
                }
            } else if let Some(dt) = tm
                .datatype
                .clone()
                .or_else(|| natural_datatype_for(tm, kinds).map(|d| format!("{XSD}{d}")))
            {
                Literal::new_typed_literal(&raw_value, NamedNode::new(dt).ok()?).to_string()
            } else {
                Literal::new_simple_literal(&raw_value).to_string()
            }
        }
    })
}

/// The natural datatype for a direct column reference with no declared one.
/// Only a plain reference gets it: a template composes text, and a constant
/// is whatever the mapping author wrote.
fn natural_datatype_for(tm: &TermMap, kinds: Option<&Kinds>) -> Option<&'static str> {
    let TermMapKind::Reference(col) = &tm.kind else {
        return None;
    };
    natural_datatype(*kinds?.get(col)?)
}

/// Evaluate a term map that can only produce an IRI, in N-Triples form.
///
/// Separate from [`eval_term`] because it needs no blank-node state: the join
/// planner builds a parent subject out of columns carried along on a child row,
/// and a blank-node subject there would mint one node per CHILD row instead of
/// per parent row. Returns `None` for any term map that is not IRI-typed, which
/// is how the planner declines to push such a join down.
pub fn eval_iri_term(tm: &TermMap, row: &Row, kinds: Option<&Kinds>) -> Option<String> {
    if tm.term_type != TermType::IRI {
        return None;
    }
    let mut unused = BlankNodes::new("unused");
    let mut no_bnodes = HashMap::new();
    eval_term(tm, row, kinds, &mut unused, &mut no_bnodes)
}

/// Evaluate a term map that must yield an IRI, returning it without the angle
/// brackets (for a graph name, or a run's target graph).
pub fn eval_iri(
    tm: &TermMap,
    row: &Row,
    kinds: Option<&Kinds>,
    bnodes: &mut BlankNodes,
    row_bnodes: &mut HashMap<String, String>,
) -> Option<String> {
    let rendered = eval_term(tm, row, kinds, bnodes, row_bnodes)?;
    rendered
        .strip_prefix('<')
        .and_then(|s| s.strip_suffix('>'))
        .map(str::to_string)
}

/// Expand an `rr:template`: replace `{column}` with the row's value,
/// percent-encoded so the result is a well-formed IRI. `\{` and `\}` are
/// literal braces. A column the row does not supply yields `None`, which
/// skips the term (and so the triple).
pub fn expand_template(template: &str, row: &Row) -> Option<String> {
    let mut result = String::with_capacity(template.len());
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                if let Some(next) = chars.next() {
                    result.push(next);
                }
            }
            '{' => {
                let mut col = String::new();
                for inner in chars.by_ref() {
                    if inner == '}' {
                        break;
                    }
                    col.push(inner);
                }
                result.push_str(&percent_encode(row.get(&col)?));
            }
            _ => result.push(c),
        }
    }
    Some(result)
}

/// Percent-encoding for IRI template substitutions.
fn percent_encode(s: &str) -> String {
    use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
    utf8_percent_encode(s, NON_ALPHANUMERIC).to_string()
}

/// How a value that the map does not cover is handled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnmappedPolicy {
    /// Emit the raw value as a plain literal, so a `sh:class` / `sh:in` shape
    /// reports it. The default: a value the mapping did not anticipate is a
    /// data finding, not something to hide.
    Literal,
    /// Emit nothing.
    Omit,
    /// Mint an IRI from a template. The template must be absolute — an
    /// undeclared prefix would mint IRIs in a namespace nobody owns.
    Template(String),
}

/// Evaluate an FNML function to an N-Triples term.
pub fn eval_function(
    f: &FunctionMap,
    row: &Row,
    kinds: Option<&Kinds>,
) -> Result<Option<String>, String> {
    if f.function == FN_MINT_IRI {
        return mint_iri(f, row);
    }
    if f.function != FN_MAP_VALUE {
        return Err(format!(
            "unsupported function <{}>; this engine implements <{FN_MAP_VALUE}> and <{FN_MINT_IRI}>",
            f.function
        ));
    }
    let Some(raw) = arg_value(f.first(&format!("{FN_NS}value")), row) else {
        // No input value (a NULL column) → no triple, like any term map.
        return Ok(None);
    };
    if raw.is_empty() {
        return Ok(None);
    }

    let normalize =
        arg_value(f.first(&format!("{FN_NS}normalize")), row).unwrap_or_else(|| "none".to_string());
    let key = normalize_value(&raw, &normalize)?;

    let mut table: HashMap<String, String> = HashMap::new();
    for entry in f.all(&format!("{FN_NS}mapping")) {
        let Some(text) = arg_value(Some(entry), row) else {
            continue;
        };
        let (k, v) = text.split_once('=').ok_or_else(|| {
            format!("{FN_LABEL}:mapping entry '{text}' is not in the form '<value>=<IRI>'")
        })?;
        table.insert(normalize_value(k, &normalize)?, v.trim().to_string());
    }

    if let Some(target) = table.get(&key) {
        let iri = NamedNode::new(target).map_err(|_| {
            format!("{FN_LABEL}:mapping maps '{key}' to '{target}', which is not an IRI")
        })?;
        return Ok(Some(iri.to_string()));
    }

    match unmapped_policy(f, row)? {
        UnmappedPolicy::Omit => Ok(None),
        UnmappedPolicy::Literal => {
            // The ORIGINAL value, not the normalised key: the report should
            // name what the database actually holds. The declared datatype
            // wins; otherwise the source column's own type applies, so an
            // unmapped numeric code stays a number.
            let declared = f.datatype.clone().or_else(|| {
                let FunctionArg::Reference(col) = f.first(&format!("{FN_NS}value"))? else {
                    return None;
                };
                natural_datatype(*kinds?.get(col)?).map(|d| format!("{XSD}{d}"))
            });
            let term = match declared {
                Some(dt) => Literal::new_typed_literal(
                    &raw,
                    NamedNode::new(&dt).map_err(|_| format!("rr:datatype <{dt}> is not an IRI"))?,
                ),
                None => Literal::new_simple_literal(&raw),
            };
            Ok(Some(term.to_string()))
        }
        UnmappedPolicy::Template(t) => {
            let expanded = expand_template(&t, row).unwrap_or_default();
            let filled = if expanded.contains("{value}") || t.contains("{value}") {
                expanded
            } else {
                format!("{expanded}{}", percent_encode(&key))
            };
            let iri = NamedNode::new(&filled).map_err(|_| {
                format!(
                    "{FN_LABEL}:unmappedTemplate produced '{filled}', which is not an absolute IRI"
                )
            })?;
            Ok(Some(iri.to_string()))
        }
    }
}

fn unmapped_policy(f: &FunctionMap, row: &Row) -> Result<UnmappedPolicy, String> {
    let policy = arg_value(f.first(&format!("{FN_NS}unmapped")), row)
        .unwrap_or_else(|| "literal".to_string());
    match policy.trim().to_ascii_lowercase().as_str() {
        "literal" | "" => Ok(UnmappedPolicy::Literal),
        "omit" | "skip" => Ok(UnmappedPolicy::Omit),
        "template" | "mint" => {
            let t =
                arg_value(f.first(&format!("{FN_NS}unmappedTemplate")), row).ok_or_else(|| {
                    format!(
                        "{FN_LABEL}:unmapped 'template' needs {FN_LABEL}:unmappedTemplate with \
                         an absolute IRI template"
                    )
                })?;
            if !(t.contains("://") || t.starts_with("urn:")) {
                return Err(format!(
                    "{FN_LABEL}:unmappedTemplate '{t}' is not absolute; minting under an \
                     undeclared prefix would put IRIs in a namespace nobody owns"
                ));
            }
            Ok(UnmappedPolicy::Template(t))
        }
        other => Err(format!(
            "unknown {FN_LABEL}:unmapped policy '{other}'; expected literal, omit or template"
        )),
    }
}

fn arg_value(arg: Option<&FunctionArg>, row: &Row) -> Option<String> {
    match arg? {
        FunctionArg::Constant(c) => Some(c.clone()),
        FunctionArg::Reference(col) => row.get(col).cloned(),
    }
}

/// The normalisation applied to both the source value and every map key, so
/// `"ACTIVE "` and `active` meet.
pub fn normalize_value(value: &str, rule: &str) -> Result<String, String> {
    Ok(match rule.trim().to_ascii_lowercase().as_str() {
        "none" | "" => value.to_string(),
        "trim" => value.trim().to_string(),
        "lower" => value.to_lowercase(),
        "upper" => value.to_uppercase(),
        "lower_trim" | "trim_lower" => value.trim().to_lowercase(),
        "upper_trim" | "trim_upper" => value.trim().to_uppercase(),
        other => {
            return Err(format!(
                "unknown {FN_LABEL}:normalize rule '{other}'; expected none, trim, lower, \
                 upper, lower_trim or upper_trim"
            ))
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn row(pairs: &[(&str, &str)]) -> Row {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn term(kind: TermMapKind, tt: TermType) -> TermMap {
        TermMap {
            kind,
            term_type: tt,
            datatype: None,
            language: None,
        }
    }

    fn eval(tm: &TermMap, r: &Row, kinds: Option<&Kinds>) -> Option<String> {
        let mut b = BlankNodes::new("b");
        let mut rb = HashMap::new();
        eval_term(tm, r, kinds, &mut b, &mut rb)
    }

    #[test]
    fn a_missing_column_produces_no_term() {
        let r = row(&[("a", "1")]);
        assert_eq!(
            eval(
                &term(TermMapKind::Reference("nope".into()), TermType::Literal),
                &r,
                None
            ),
            None
        );
        assert_eq!(
            eval(
                &term(
                    TermMapKind::Template("http://x/{nope}".into()),
                    TermType::IRI
                ),
                &r,
                None
            ),
            None
        );
        // …and an empty value is treated the same way.
        assert_eq!(
            eval(
                &term(TermMapKind::Reference("e".into()), TermType::Literal),
                &row(&[("e", "")]),
                None
            ),
            None
        );
    }

    #[test]
    fn natural_datatypes_apply_only_to_bare_column_references() {
        let r = row(&[("n", "42"), ("t", "hello"), ("d", "1.50")]);
        let kinds: Kinds = HashMap::from([
            ("n".to_string(), ValueKind::Integer),
            ("t".to_string(), ValueKind::Text),
            ("d".to_string(), ValueKind::Decimal),
        ]);
        assert_eq!(
            eval(
                &term(TermMapKind::Reference("n".into()), TermType::Literal),
                &r,
                Some(&kinds)
            )
            .unwrap(),
            format!("\"42\"^^<{XSD}integer>")
        );
        assert_eq!(
            eval(
                &term(TermMapKind::Reference("d".into()), TermType::Literal),
                &r,
                Some(&kinds)
            )
            .unwrap(),
            format!("\"1.50\"^^<{XSD}decimal>")
        );
        assert_eq!(
            eval(
                &term(TermMapKind::Reference("t".into()), TermType::Literal),
                &r,
                Some(&kinds)
            )
            .unwrap(),
            "\"hello\"",
            "text stays a plain literal"
        );
        // An explicit datatype always wins over the column's own type.
        let mut tm = term(TermMapKind::Reference("n".into()), TermType::Literal);
        tm.datatype = Some(format!("{XSD}token"));
        assert_eq!(
            eval(&tm, &r, Some(&kinds)).unwrap(),
            format!("\"42\"^^<{XSD}token>")
        );
        // …including `xsd:string`, which RDF 1.1 writes as a plain literal.
        tm.datatype = Some(format!("{XSD}string"));
        assert_eq!(eval(&tm, &r, Some(&kinds)).unwrap(), "\"42\"");
        // Without kinds (a CSV source) nothing is invented.
        assert_eq!(
            eval(
                &term(TermMapKind::Reference("n".into()), TermType::Literal),
                &r,
                None
            )
            .unwrap(),
            "\"42\""
        );
    }

    #[test]
    fn hostile_values_cannot_break_out_of_a_term() {
        let r = row(&[("v", "a\"b\nc\\d"), ("i", "has space")]);
        let lit = eval(
            &term(TermMapKind::Reference("v".into()), TermType::Literal),
            &r,
            None,
        )
        .unwrap();
        assert!(!lit.contains('\n'), "raw newline in a literal: {lit}");
        assert_eq!(lit, r#""a\"b\nc\\d""#);
        // A column with termType IRI that is not a valid IRI yields no term at
        // all, rather than an injected one.
        assert_eq!(
            eval(
                &term(TermMapKind::Reference("i".into()), TermType::IRI),
                &r,
                None
            ),
            None
        );
        // A template percent-encodes, so the same value is safe there.
        assert_eq!(
            eval(
                &term(TermMapKind::Template("http://x/{i}".into()), TermType::IRI),
                &r,
                None
            )
            .unwrap(),
            "<http://x/has%20space>"
        );
    }

    #[test]
    fn eval_iri_term_refuses_anything_that_is_not_an_iri() {
        let r = row(&[("id", "7")]);
        let iri = term(TermMapKind::Template("http://x/{id}".into()), TermType::IRI);
        assert_eq!(eval_iri_term(&iri, &r, None).unwrap(), "<http://x/7>");
        // A blank-node subject must not be minted from a child row, so the
        // planner is told "no" rather than handed a fresh node.
        let bn = term(TermMapKind::Template("n{id}".into()), TermType::BlankNode);
        assert_eq!(eval_iri_term(&bn, &r, None), None);
        let lit = term(TermMapKind::Reference("id".into()), TermType::Literal);
        assert_eq!(eval_iri_term(&lit, &r, None), None);
        // A column the row does not supply still yields nothing.
        assert_eq!(
            eval_iri_term(
                &term(
                    TermMapKind::Template("http://x/{nope}".into()),
                    TermType::IRI
                ),
                &r,
                None
            ),
            None
        );
    }

    #[test]
    fn escaped_braces_in_a_template_are_literal() {
        let r = row(&[("c", "v")]);
        assert_eq!(
            expand_template(r"http://x/\{lit\}/{c}", &r).unwrap(),
            "http://x/{lit}/v"
        );
    }

    #[test]
    fn blank_nodes_co_refer_in_a_row_and_never_collide_across_rows() {
        let mut b = BlankNodes::new("r7_");
        let tm = term(TermMapKind::Reference("k".into()), TermType::BlankNode);
        let r1 = row(&[("k", "same")]);
        let mut rb = HashMap::new();
        let a = eval_term(&tm, &r1, None, &mut b, &mut rb).unwrap();
        let a2 = eval_term(&tm, &r1, None, &mut b, &mut rb).unwrap();
        assert_eq!(a, a2, "same value in one row is the same node");
        assert!(a.starts_with("_:r7_"), "labels carry the run prefix: {a}");
        let mut rb2 = HashMap::new();
        let c = eval_term(&tm, &r1, None, &mut b, &mut rb2).unwrap();
        assert_ne!(a, c, "a new row mints a new node even for the same value");
    }

    fn map_fn(pairs: &[(&str, FunctionArg)]) -> FunctionMap {
        let mut params: BTreeMap<String, Vec<FunctionArg>> = BTreeMap::new();
        for (k, v) in pairs {
            params
                .entry(format!("{FN_NS}{k}"))
                .or_default()
                .push(v.clone());
        }
        FunctionMap {
            function: FN_MAP_VALUE.to_string(),
            params,
            datatype: None,
        }
    }

    #[test]
    fn map_value_normalises_then_looks_up() {
        let f = map_fn(&[
            ("value", FunctionArg::Reference("status".into())),
            ("normalize", FunctionArg::Constant("lower_trim".into())),
            (
                "mapping",
                FunctionArg::Constant("active=http://x/Active".into()),
            ),
            (
                "mapping",
                FunctionArg::Constant("Retired=http://x/Retired".into()),
            ),
        ]);
        for raw in ["active", "ACTIVE ", " Active"] {
            assert_eq!(
                eval_function(&f, &row(&[("status", raw)]), None)
                    .unwrap()
                    .unwrap(),
                "<http://x/Active>",
                "{raw}"
            );
        }
        assert_eq!(
            eval_function(&f, &row(&[("status", "retired")]), None)
                .unwrap()
                .unwrap(),
            "<http://x/Retired>",
            "map keys are normalised too"
        );
    }

    #[test]
    fn an_unmapped_value_defaults_to_a_literal_so_shacl_sees_it() {
        let f = map_fn(&[
            ("value", FunctionArg::Reference("s".into())),
            ("normalize", FunctionArg::Constant("lower_trim".into())),
            ("mapping", FunctionArg::Constant("a=http://x/A".into())),
        ]);
        assert_eq!(
            eval_function(&f, &row(&[("s", "Weird Value")]), None)
                .unwrap()
                .unwrap(),
            "\"Weird Value\"",
            "the raw value is kept, not the normalised key"
        );
        // NULL in, nothing out.
        assert_eq!(eval_function(&f, &row(&[]), None).unwrap(), None);
    }

    #[test]
    fn an_unmapped_numeric_code_keeps_its_column_type() {
        let f = map_fn(&[
            ("value", FunctionArg::Reference("code".into())),
            ("mapping", FunctionArg::Constant("1=http://x/One".into())),
        ]);
        let kinds: Kinds = HashMap::from([("code".to_string(), ValueKind::Integer)]);
        assert_eq!(
            eval_function(&f, &row(&[("code", "7")]), Some(&kinds))
                .unwrap()
                .unwrap(),
            format!("\"7\"^^<{XSD}integer>")
        );
        // A mapped value is still the IRI, whatever the column type.
        assert_eq!(
            eval_function(&f, &row(&[("code", "1")]), Some(&kinds))
                .unwrap()
                .unwrap(),
            "<http://x/One>"
        );
    }

    #[test]
    fn the_omit_and_template_policies_behave_as_declared() {
        let base = [
            ("value", FunctionArg::Reference("s".into())),
            ("mapping", FunctionArg::Constant("a=http://x/A".into())),
        ];
        let mut omit = map_fn(&base);
        omit.params.insert(
            format!("{FN_NS}unmapped"),
            vec![FunctionArg::Constant("omit".into())],
        );
        assert_eq!(
            eval_function(&omit, &row(&[("s", "zzz")]), None).unwrap(),
            None
        );

        let mut mint = map_fn(&base);
        mint.params.insert(
            format!("{FN_NS}unmapped"),
            vec![FunctionArg::Constant("template".into())],
        );
        mint.params.insert(
            format!("{FN_NS}unmappedTemplate"),
            vec![FunctionArg::Constant("http://x/status/".into())],
        );
        assert_eq!(
            eval_function(&mint, &row(&[("s", "zz z")]), None)
                .unwrap()
                .unwrap(),
            "<http://x/status/zz%20z>"
        );
    }

    #[test]
    fn a_relative_mint_template_is_refused() {
        let mut f = map_fn(&[("value", FunctionArg::Reference("s".into()))]);
        f.params.insert(
            format!("{FN_NS}unmapped"),
            vec![FunctionArg::Constant("template".into())],
        );
        f.params.insert(
            format!("{FN_NS}unmappedTemplate"),
            vec![FunctionArg::Constant("status/".into())],
        );
        let err = eval_function(&f, &row(&[("s", "x")]), None).unwrap_err();
        assert!(err.contains("not absolute"), "{err}");
    }

    #[test]
    fn unknown_functions_and_rules_fail_loudly() {
        let mut f = map_fn(&[("value", FunctionArg::Constant("x".into()))]);
        f.function = "http://example.org/nope".into();
        assert!(eval_function(&f, &row(&[]), None)
            .unwrap_err()
            .contains("unsupported function"));

        let bad = map_fn(&[
            ("value", FunctionArg::Constant("x".into())),
            ("normalize", FunctionArg::Constant("sideways".into())),
        ]);
        assert!(eval_function(&bad, &row(&[]), None)
            .unwrap_err()
            .contains("fn:normalize"));

        let malformed = map_fn(&[
            ("value", FunctionArg::Constant("x".into())),
            ("mapping", FunctionArg::Constant("no-equals-sign".into())),
        ]);
        assert!(eval_function(&malformed, &row(&[]), None)
            .unwrap_err()
            .contains("fn:mapping"));
    }
}
