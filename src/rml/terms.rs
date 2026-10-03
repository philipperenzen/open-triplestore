//! Term-map evaluation, shared by the file-based and the relational executor.
//!
//! One row in, one RDF term out — serialised in N-Triples form so a caller can
//! concatenate terms into a document and stream it into the store.
//!
//! Two things beyond plain templating live here because both executors need
//! them: the **natural datatype** rule (R2RML §10.2 — a column with no
//! `rr:datatype` takes the XSD type its SQL type implies) and the **FNML
//! function** used for enumerations.
//!
//! How a value becomes a term depends on the mapping's [`Semantics`]: the
//! encoding of template values and the scope of blank nodes changed when this
//! engine moved to R2RML's rules, and a mapping version frozen before then
//! keeps the old ones (see [`Semantics`] for the table).

use std::collections::HashMap;

use ots_plugin_api::sources::ValueKind;
use oxigraph::model::{Literal, NamedNode, Term};

use super::iri;
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

/// Expand a template whose placeholders may be `{column}` (encoded for an
/// IRI) or `{column_slug}` (slugged). A placeholder the row cannot supply
/// yields `None`, and so no term.
pub fn expand_slug_template(template: &str, row: &Row, semantics: Semantics) -> Option<String> {
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
                    Some(v) => Encoding::iri(semantics).apply(v),
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
fn mint_iri(f: &FunctionMap, row: &Row, gen: &mut TermGen) -> Result<Option<String>, String> {
    let semantics = gen.semantics;
    let template = arg_value(f.first(&format!("{FN_NS}template")), row).ok_or_else(|| {
        format!("{FN_LABEL}:mintIri needs {FN_LABEL}:template with an absolute IRI template")
    })?;
    if !(template.contains("://") || template.starts_with("urn:")) {
        return Err(format!(
            "{FN_LABEL}:template '{template}' is not absolute; minting under an undeclared \
             prefix would put IRIs in a namespace nobody owns"
        ));
    }
    let Some(filled) = expand_slug_template(&template, row, semantics) else {
        return Ok(None);
    };
    // An IRI the template cannot make well-formed is a data error, as it is
    // for an `rr:template`.
    match NamedNode::new(&filled) {
        Ok(n) => Ok(Some(n.to_string())),
        Err(_) => {
            gen.data_error(format!(
                "{FN_LABEL}:mintIri template \"{template}\" generates {}, which is not a valid IRI",
                quoted(&filled)
            ));
            Ok(None)
        }
    }
}

/// One source row: column name → lexical value. A NULL — a SQL NULL, a JSON
/// `null` or absent key, a value the logical source lists under `rml:null` —
/// is "no key", so a term map over one produces no term and therefore no
/// triple. An empty string is a value (RML-IO), except under
/// [`Semantics::Legacy`], where an empty value produces no term either.
pub type Row = HashMap<String, String>;
/// Per-column generic types, when the source reports them (relational only).
pub type Kinds = HashMap<String, ValueKind>;

/// Term-generation state for one execution: the rules it runs under, and the
/// blank-node labelling that keeps two runs' nodes apart.
///
/// Under [`Semantics::R2rml`] a blank node is unique to its value within a
/// graph (R2RML §11.2, §9.1), so its label is a hash of the run, the graph and
/// the value: the same in every batch, and the same whether a join was pushed
/// down or indexed. Under [`Semantics::Legacy`] a node is minted per row and
/// value, as this engine did before.
#[derive(Debug)]
pub struct TermGen {
    semantics: Semantics,
    prefix: String,
    counter: u64,
    /// Legacy only: the nodes minted for the current row, by value.
    row: HashMap<String, String>,
    /// Data errors (R2RML §4.3) met since the caller last took them.
    errors: Vec<String>,
}

impl TermGen {
    pub fn new(semantics: Semantics, prefix: impl Into<String>) -> Self {
        Self {
            semantics,
            prefix: prefix.into(),
            counter: 0,
            row: HashMap::new(),
            errors: Vec::new(),
        }
    }

    /// Record a data error: a value that cannot become the term its map asks
    /// for. The term is not generated; whether the run goes on is the
    /// caller's decision ([`OnDataError`]).
    pub fn data_error(&mut self, message: String) {
        self.errors.push(message);
    }

    /// The data errors recorded since the last call.
    pub fn take_errors(&mut self) -> Vec<String> {
        std::mem::take(&mut self.errors)
    }

    /// Drop the cells of `row` that count as NULL in `source`: its
    /// `rml:null` values. A legacy version ignores `rml:null` and instead
    /// generates no term from an empty value, as it always did.
    pub fn apply_nulls(&self, source: &LogicalSource, row: &mut Row) {
        if self.semantics == Semantics::R2rml && !source.nulls.is_empty() {
            row.retain(|_, v| !source.nulls.iter().any(|n| n == v));
        }
    }

    pub fn semantics(&self) -> Semantics {
        self.semantics
    }

    /// A new source row begins. Legacy blank nodes do not outlive it.
    pub fn start_row(&mut self) {
        self.row.clear();
    }

    fn blank_node(&mut self, value: String, graph: Option<&str>) -> String {
        match self.semantics {
            Semantics::R2rml => iri::blank_node_label(&self.prefix, graph, &value),
            Semantics::Legacy => {
                if let Some(existing) = self.row.get(&value) {
                    return existing.clone();
                }
                self.counter += 1;
                let label = format!("_:{}{}", self.prefix, self.counter);
                self.row.insert(value, label.clone());
                label
            }
        }
    }
}

/// Where a term is generated: the base IRI a relative IRI resolves against,
/// and the graph the triple lands in (`None` = the run's target graph), to
/// which a blank node is scoped.
#[derive(Debug, Clone, Copy, Default)]
pub struct At<'a> {
    pub base: Option<&'a str>,
    pub graph: Option<&'a str>,
}

/// How a template value is written into the result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Encoding {
    /// As it is (R2RML: a template that does not generate an IRI).
    None,
    /// The IRI-safe version (R2RML §7.3).
    IriSafe,
    /// Every byte but `[A-Za-z0-9]` percent-encoded, in every template — what
    /// this engine did before, kept for legacy mapping versions.
    Legacy,
}

impl Encoding {
    fn for_term(semantics: Semantics, term_type: &TermType) -> Self {
        match (semantics, term_type) {
            (Semantics::Legacy, _) => Encoding::Legacy,
            (Semantics::R2rml, TermType::IRI) => Encoding::IriSafe,
            (Semantics::R2rml, _) => Encoding::None,
        }
    }

    fn iri(semantics: Semantics) -> Self {
        Self::for_term(semantics, &TermType::IRI)
    }

    fn apply(self, value: &str) -> String {
        match self {
            Encoding::None => value.to_string(),
            Encoding::IriSafe => iri::iri_safe(value),
            Encoding::Legacy => legacy_percent_encode(value),
        }
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
    gen: &mut TermGen,
    at: At<'_>,
) -> Option<String> {
    let raw_value = match &tm.kind {
        // A constant is the term the mapping wrote, datatype and language
        // included (R2RML §7.4).
        TermMapKind::Constant(term) => return render_constant(term),
        TermMapKind::Template(template) => expand_template_typed(
            template,
            row,
            kinds,
            Encoding::for_term(gen.semantics, &tm.term_type),
        )?,
        TermMapKind::Reference(col) => {
            natural_lexical(row.get(col)?, kinds.and_then(|k| k.get(col).copied())).into_owned()
        }
    };

    // An empty value is a value (RML-IO: nothing is NULL unless the source
    // says so). Before, it produced no term, which a legacy version keeps.
    if raw_value.is_empty() && gen.semantics == Semantics::Legacy {
        return None;
    }

    Some(match tm.term_type {
        // Build the IRI through oxrdf so it is validated and serialised, not
        // pasted between angle brackets. A `rml:reference`/`rr:column` value
        // with `rr:termType rr:IRI` went in raw: a value containing a space
        // produced invalid Turtle and failed the WHOLE mapping, and one
        // containing `>` could terminate the IRI and inject further triples.
        // A value that is not absolute is appended to the base IRI (§11.2).
        TermType::IRI => match iri::absolute_iri(&raw_value, at.base) {
            Some(n) => n.to_string(),
            None => {
                gen.data_error(format!(
                    "{} generates {}, which is not a valid IRI{}",
                    describe(tm),
                    quoted(&raw_value),
                    if at.base.is_some() {
                        " (relative to the base IRI either)"
                    } else {
                        ""
                    }
                ));
                return None;
            }
        },
        TermType::BlankNode => gen.blank_node(raw_value, at.graph),
        // Likewise for literals: hand-escaping only `\` and `"` left raw
        // newlines, carriage returns and tabs in the output, which Turtle's
        // STRING_LITERAL_QUOTE forbids — so one multi-line CSV field made the
        // entire generated document unparseable and the mapping failed with
        // "Failed to load generated triples" rather than skipping a row.
        TermType::Literal => {
            if let Some(ref lang) = tm.language {
                // The parser has checked the tag, so this cannot fail on it.
                Literal::new_language_tagged_literal(&raw_value, lang)
                    .ok()?
                    .to_string()
            } else if let Some(dt) = &tm.datatype {
                // A datatype-override literal that is ill-typed is a data error
                // (R2RML §10.3); a natural one is well-typed by construction.
                if let Some(why) = super::xsd::ill_typed(&raw_value, dt) {
                    gen.data_error(format!(
                        "{} generates {} as <{dt}>, which is {why}",
                        describe(tm),
                        quoted(&raw_value)
                    ));
                    return None;
                }
                Literal::new_typed_literal(&raw_value, NamedNode::new(dt).ok()?).to_string()
            } else if let Some(dt) = natural_datatype_for(tm, kinds) {
                Literal::new_typed_literal(&raw_value, NamedNode::new(format!("{XSD}{dt}")).ok()?)
                    .to_string()
            } else {
                Literal::new_simple_literal(&raw_value).to_string()
            }
        }
    })
}

/// A term map as an error message names it.
fn describe(tm: &TermMap) -> String {
    match &tm.kind {
        TermMapKind::Template(t) => format!("rr:template \"{t}\""),
        TermMapKind::Reference(c) => format!("column \"{c}\""),
        TermMapKind::Constant(c) => format!("rr:constant {c}"),
    }
}

/// A source value quoted for an error message, shortened past 120 characters.
fn quoted(value: &str) -> String {
    const MAX: usize = 120;
    // Escaped, so a value with a line break stays on the report's one line.
    match value.char_indices().nth(MAX) {
        Some((i, _)) => format!("\"{}…\"", value[..i].escape_debug()),
        None => format!("\"{}\"", value.escape_debug()),
    }
}

/// A constant in N-Triples form. The parser admits only IRIs and literals.
fn render_constant(term: &Term) -> Option<String> {
    match term {
        Term::NamedNode(_) | Term::Literal(_) => Some(term.to_string()),
        _ => None,
    }
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

/// Evaluate a join parent's subject from columns carried on a child row.
///
/// The join planner builds a parent subject out of columns projected onto a
/// child row, so the term must not depend on which row computes it. An IRI
/// never does, and neither does an R2RML blank node, whose label is a function
/// of its value. A legacy blank node is minted per row — pushed down, two
/// children of one parent would stop sharing it — so `None` here is how the
/// planner declines to push such a join down.
pub fn eval_parent_subject(
    tm: &TermMap,
    row: &Row,
    kinds: Option<&Kinds>,
    gen: &mut TermGen,
    base: Option<&str>,
) -> Option<String> {
    if !position_independent(tm, gen.semantics) {
        return None;
    }
    eval_term(tm, row, kinds, gen, At { base, graph: None })
}

/// Whether a subject term map generates the same term for the same values
/// wherever it is evaluated. See [`eval_parent_subject`].
pub fn position_independent(tm: &TermMap, semantics: Semantics) -> bool {
    match tm.term_type {
        TermType::IRI => true,
        TermType::BlankNode => semantics == Semantics::R2rml,
        TermType::Literal => false,
    }
}

/// Evaluate a term map that must yield an IRI, returning it without the angle
/// brackets (for a graph name, or a run's target graph).
pub fn eval_iri(
    tm: &TermMap,
    row: &Row,
    kinds: Option<&Kinds>,
    gen: &mut TermGen,
    base: Option<&str>,
) -> Option<String> {
    let rendered = eval_term(tm, row, kinds, gen, At { base, graph: None })?;
    rendered
        .strip_prefix('<')
        .and_then(|s| s.strip_suffix('>'))
        .map(str::to_string)
}

/// The natural RDF lexical form of a typed source value (R2RML §10.2): an
/// SQL timestamp's space becomes the `T` of `xsd:dateTime`, and an SQL
/// boolean written as a number or a letter becomes `true` / `false`. Every
/// other value is already its own lexical form.
fn natural_lexical(value: &str, kind: Option<ValueKind>) -> std::borrow::Cow<'_, str> {
    use std::borrow::Cow;
    match kind {
        Some(ValueKind::DateTime) if value.len() > 10 && value.as_bytes()[10] == b' ' => {
            Cow::Owned(format!("{}T{}", &value[..10], &value[11..]))
        }
        Some(ValueKind::Boolean) => match value {
            "1" | "t" | "T" | "TRUE" | "True" => Cow::Borrowed("true"),
            "0" | "f" | "F" | "FALSE" | "False" => Cow::Borrowed("false"),
            _ => Cow::Borrowed(value),
        },
        _ => Cow::Borrowed(value),
    }
}

/// [`expand_template_encoded`], with each value in its natural lexical form
/// (R2RML §10.2: a template uses the natural RDF lexical form of its values).
fn expand_template_typed(
    template: &str,
    row: &Row,
    kinds: Option<&Kinds>,
    encoding: Encoding,
) -> Option<String> {
    let Some(kinds) = kinds else {
        return expand_template_encoded(template, row, encoding);
    };
    let typed: Row = row
        .iter()
        .map(|(k, v)| {
            (
                k.clone(),
                natural_lexical(v, kinds.get(k).copied()).into_owned(),
            )
        })
        .collect();
    expand_template_encoded(template, &typed, encoding)
}

/// Expand an `rr:template`: replace `{column}` with the row's value, encoded
/// as `encoding` says. `\{` and `\}` are literal braces. A column the row
/// does not supply yields `None`, which skips the term (and so the triple).
fn expand_template_encoded(template: &str, row: &Row, encoding: Encoding) -> Option<String> {
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
                result.push_str(&encoding.apply(row.get(&col)?));
            }
            _ => result.push(c),
        }
    }
    Some(result)
}

/// The percent-encoding this engine applied to every template value before
/// it followed R2RML §7.3: everything but ASCII letters and digits.
fn legacy_percent_encode(s: &str) -> String {
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
    gen: &mut TermGen,
) -> Result<Option<String>, String> {
    let semantics = gen.semantics;
    if f.function == FN_MINT_IRI {
        return mint_iri(f, row, gen);
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
    if raw.is_empty() && semantics == Semantics::Legacy {
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
            let encoding = Encoding::iri(semantics);
            let expanded = expand_template_encoded(&t, row, encoding).unwrap_or_default();
            let filled = if expanded.contains("{value}") || t.contains("{value}") {
                expanded
            } else {
                format!("{expanded}{}", encoding.apply(&key))
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
        eval_as(Semantics::R2rml, tm, r, kinds)
    }

    fn eval_as(s: Semantics, tm: &TermMap, r: &Row, kinds: Option<&Kinds>) -> Option<String> {
        let mut g = TermGen::new(s, "b");
        eval_term(tm, r, kinds, &mut g, At::default())
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
        // An empty value is a value under R2RML's rules (RML-IO: nothing is
        // NULL unless `rml:null` says so)…
        let empty = term(TermMapKind::Reference("e".into()), TermType::Literal);
        assert_eq!(
            eval(&empty, &row(&[("e", "")]), None).as_deref(),
            Some("\"\"")
        );
        // …and no term under a legacy version's.
        assert_eq!(
            eval_as(Semantics::Legacy, &empty, &row(&[("e", "")]), None),
            None
        );
    }

    #[test]
    fn rml_null_values_are_dropped_from_the_row_under_r2rml_only() {
        let source = LogicalSource {
            source: SourceRef::File("d.csv".into()),
            reference_formulation: ReferenceFormulation::Csv,
            iterator: None,
            query: None,
            table_name: None,
            nulls: vec!["".into(), "NULL".into()],
        };
        let mut r = row(&[("a", ""), ("b", "NULL"), ("c", "x")]);
        TermGen::new(Semantics::R2rml, "b").apply_nulls(&source, &mut r);
        assert_eq!(r, row(&[("c", "x")]));
        let mut r = row(&[("a", ""), ("b", "NULL")]);
        TermGen::new(Semantics::Legacy, "b").apply_nulls(&source, &mut r);
        assert_eq!(r.len(), 2, "a legacy version ignores rml:null");
    }

    #[test]
    fn data_errors_are_recorded_not_generated() {
        let mut g = TermGen::new(Semantics::R2rml, "b");
        let iri = term(TermMapKind::Reference("t".into()), TermType::IRI);
        assert_eq!(
            eval_term(
                &iri,
                &row(&[("t", "not an iri")]),
                None,
                &mut g,
                At::default()
            ),
            None
        );
        let mut typed = term(TermMapKind::Reference("n".into()), TermType::Literal);
        typed.datatype = Some(format!("{XSD}integer"));
        assert_eq!(
            eval_term(&typed, &row(&[("n", "1.5")]), None, &mut g, At::default()),
            None
        );
        assert_eq!(
            eval_term(&typed, &row(&[("n", "15")]), None, &mut g, At::default()).as_deref(),
            Some(format!("\"15\"^^<{XSD}integer>").as_str())
        );
        let errors = g.take_errors();
        assert_eq!(errors.len(), 2, "{errors:?}");
        assert!(errors[0].contains("\"not an iri\"") && errors[0].contains("not a valid IRI"));
        assert!(errors[1].contains("\"1.5\"") && errors[1].contains("integer"));
        assert!(g.take_errors().is_empty(), "taking them clears them");
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
    fn a_parent_subject_is_computed_only_when_no_row_can_change_it() {
        let r = row(&[("id", "7")]);
        let mut legacy = TermGen::new(Semantics::Legacy, "p");
        let mut r2rml = TermGen::new(Semantics::R2rml, "p");
        let iri = term(TermMapKind::Template("http://x/{id}".into()), TermType::IRI);
        assert_eq!(
            eval_parent_subject(&iri, &r, None, &mut legacy, None).unwrap(),
            "<http://x/7>"
        );
        // A legacy blank-node subject is minted per row, so a child row must
        // not mint the parent's: the planner is told "no".
        let bn = term(TermMapKind::Template("n{id}".into()), TermType::BlankNode);
        assert_eq!(eval_parent_subject(&bn, &r, None, &mut legacy, None), None);
        // An R2RML blank node is a function of its value: any row agrees.
        let mut own = TermGen::new(Semantics::R2rml, "p");
        let own_label = eval_term(&bn, &r, None, &mut own, At::default()).unwrap();
        assert_eq!(
            eval_parent_subject(&bn, &r, None, &mut r2rml, None).unwrap(),
            own_label
        );
        let lit = term(TermMapKind::Reference("id".into()), TermType::Literal);
        assert_eq!(eval_parent_subject(&lit, &r, None, &mut r2rml, None), None);
        // A column the row does not supply still yields nothing.
        assert_eq!(
            eval_parent_subject(
                &term(
                    TermMapKind::Template("http://x/{nope}".into()),
                    TermType::IRI
                ),
                &r,
                None,
                &mut r2rml,
                None
            ),
            None
        );
    }

    #[test]
    fn escaped_braces_in_a_template_are_literal() {
        let r = row(&[("c", "v")]);
        assert_eq!(
            expand_template_encoded(r"http://x/\{lit\}/{c}", &r, Encoding::IriSafe).unwrap(),
            "http://x/{lit}/v"
        );
    }

    #[test]
    fn legacy_blank_nodes_co_refer_in_a_row_and_never_across_rows() {
        let mut b = TermGen::new(Semantics::Legacy, "r7_");
        let tm = term(TermMapKind::Reference("k".into()), TermType::BlankNode);
        let r1 = row(&[("k", "same")]);
        b.start_row();
        let a = eval_term(&tm, &r1, None, &mut b, At::default()).unwrap();
        let a2 = eval_term(&tm, &r1, None, &mut b, At::default()).unwrap();
        assert_eq!(a, a2, "same value in one row is the same node");
        assert!(a.starts_with("_:r7_"), "labels carry the run prefix: {a}");
        b.start_row();
        let c = eval_term(&tm, &r1, None, &mut b, At::default()).unwrap();
        assert_ne!(a, c, "a new row mints a new node even for the same value");
    }

    #[test]
    fn r2rml_blank_nodes_are_unique_to_their_value_within_a_graph() {
        // R2RML §11.2: "a blank node that is unique to the natural RDF lexical
        // form corresponding to value"; §9.1: scoped to one graph.
        let mut b = TermGen::new(Semantics::R2rml, "r7_");
        let tm = term(TermMapKind::Reference("k".into()), TermType::BlankNode);
        let same = row(&[("k", "same")]);
        b.start_row();
        let a = eval_term(&tm, &same, None, &mut b, At::default()).unwrap();
        b.start_row();
        let again = eval_term(&tm, &same, None, &mut b, At::default()).unwrap();
        assert_eq!(a, again, "the same value in another row is the same node");
        assert!(a.starts_with("_:r7_"), "labels carry the run prefix: {a}");
        let other = eval_term(&tm, &row(&[("k", "other")]), None, &mut b, At::default());
        assert_ne!(Some(a.clone()), other, "another value is another node");
        let in_g = eval_term(
            &tm,
            &same,
            None,
            &mut b,
            At {
                base: None,
                graph: Some("http://g"),
            },
        );
        assert_ne!(Some(a), in_g, "another graph is another node");
    }

    #[test]
    fn template_values_are_encoded_per_the_semantics() {
        let r = row(&[("v", "~A_17.1-2 葉 x/y")]);
        let iri = term(TermMapKind::Template("http://x/{v}".into()), TermType::IRI);
        assert_eq!(
            eval(&iri, &r, None).unwrap(),
            "<http://x/~A_17.1-2%20葉%20x%2Fy>",
            "R2RML §7.3: only what is outside iunreserved"
        );
        assert_eq!(
            eval_as(Semantics::Legacy, &iri, &r, None).unwrap(),
            "<http://x/%7EA%5F17%2E1%2D2%20%E8%91%89%20x%2Fy>",
            "a legacy version keeps its IRIs"
        );
        let lit = term(TermMapKind::Template("Hi {v}!".into()), TermType::Literal);
        assert_eq!(
            eval(&lit, &r, None).unwrap(),
            "\"Hi ~A_17.1-2 葉 x/y!\"",
            "a literal template is not encoded"
        );
        assert_eq!(
            eval_as(Semantics::Legacy, &lit, &r, None).unwrap(),
            "\"Hi %7EA%5F17%2E1%2D2%20%E8%91%89%20x%2Fy!\""
        );
    }

    #[test]
    fn a_constant_is_the_term_the_mapping_wrote() {
        let r = row(&[]);
        let typed =
            Literal::new_typed_literal("5", NamedNode::new_unchecked(format!("{XSD}integer")));
        let tagged = Literal::new_language_tagged_literal("hallo", "nl").unwrap();
        for (t, expected) in [
            (Term::from(typed), format!("\"5\"^^<{XSD}integer>")),
            (Term::from(tagged), "\"hallo\"@nl".to_string()),
            (
                Term::from(NamedNode::new_unchecked("http://x/T")),
                "<http://x/T>".to_string(),
            ),
        ] {
            // rr:termType has no effect on a constant (R2RML §7.4).
            let tm = term(TermMapKind::Constant(t), TermType::Literal);
            assert_eq!(eval(&tm, &r, None).unwrap(), expected);
            assert_eq!(eval_as(Semantics::Legacy, &tm, &r, None).unwrap(), expected);
        }
    }

    #[test]
    fn a_relative_iri_resolves_against_the_base() {
        let r = row(&[("id", "10")]);
        let tm = term(TermMapKind::Template("Student/{id}".into()), TermType::IRI);
        assert_eq!(eval(&tm, &r, None), None, "no base, no IRI");
        let mut g = TermGen::new(Semantics::R2rml, "b");
        let at = At {
            base: Some("http://example.com/base/"),
            graph: None,
        };
        assert_eq!(
            eval_term(&tm, &r, None, &mut g, at).unwrap(),
            "<http://example.com/base/Student/10>"
        );
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
                eval_function(
                    &f,
                    &row(&[("status", raw)]),
                    None,
                    &mut TermGen::new(Semantics::R2rml, "t")
                )
                .unwrap()
                .unwrap(),
                "<http://x/Active>",
                "{raw}"
            );
        }
        assert_eq!(
            eval_function(
                &f,
                &row(&[("status", "retired")]),
                None,
                &mut TermGen::new(Semantics::R2rml, "t")
            )
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
            eval_function(
                &f,
                &row(&[("s", "Weird Value")]),
                None,
                &mut TermGen::new(Semantics::R2rml, "t")
            )
            .unwrap()
            .unwrap(),
            "\"Weird Value\"",
            "the raw value is kept, not the normalised key"
        );
        // NULL in, nothing out.
        assert_eq!(
            eval_function(
                &f,
                &row(&[]),
                None,
                &mut TermGen::new(Semantics::R2rml, "t")
            )
            .unwrap(),
            None
        );
    }

    #[test]
    fn an_unmapped_numeric_code_keeps_its_column_type() {
        let f = map_fn(&[
            ("value", FunctionArg::Reference("code".into())),
            ("mapping", FunctionArg::Constant("1=http://x/One".into())),
        ]);
        let kinds: Kinds = HashMap::from([("code".to_string(), ValueKind::Integer)]);
        assert_eq!(
            eval_function(
                &f,
                &row(&[("code", "7")]),
                Some(&kinds),
                &mut TermGen::new(Semantics::R2rml, "t")
            )
            .unwrap()
            .unwrap(),
            format!("\"7\"^^<{XSD}integer>")
        );
        // A mapped value is still the IRI, whatever the column type.
        assert_eq!(
            eval_function(
                &f,
                &row(&[("code", "1")]),
                Some(&kinds),
                &mut TermGen::new(Semantics::R2rml, "t")
            )
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
            eval_function(
                &omit,
                &row(&[("s", "zzz")]),
                None,
                &mut TermGen::new(Semantics::R2rml, "t")
            )
            .unwrap(),
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
            eval_function(
                &mint,
                &row(&[("s", "zz z")]),
                None,
                &mut TermGen::new(Semantics::R2rml, "t")
            )
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
        let err = eval_function(
            &f,
            &row(&[("s", "x")]),
            None,
            &mut TermGen::new(Semantics::R2rml, "t"),
        )
        .unwrap_err();
        assert!(err.contains("not absolute"), "{err}");
    }

    #[test]
    fn unknown_functions_and_rules_fail_loudly() {
        let mut f = map_fn(&[("value", FunctionArg::Constant("x".into()))]);
        f.function = "http://example.org/nope".into();
        assert!(eval_function(
            &f,
            &row(&[]),
            None,
            &mut TermGen::new(Semantics::R2rml, "t")
        )
        .unwrap_err()
        .contains("unsupported function"));

        let bad = map_fn(&[
            ("value", FunctionArg::Constant("x".into())),
            ("normalize", FunctionArg::Constant("sideways".into())),
        ]);
        assert!(eval_function(
            &bad,
            &row(&[]),
            None,
            &mut TermGen::new(Semantics::R2rml, "t")
        )
        .unwrap_err()
        .contains("fn:normalize"));

        let malformed = map_fn(&[
            ("value", FunctionArg::Constant("x".into())),
            ("mapping", FunctionArg::Constant("no-equals-sign".into())),
        ]);
        assert!(eval_function(
            &malformed,
            &row(&[]),
            None,
            &mut TermGen::new(Semantics::R2rml, "t")
        )
        .unwrap_err()
        .contains("fn:mapping"));
    }
}
