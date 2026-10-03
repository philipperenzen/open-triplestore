//! Term-map evaluation, shared by the file-based and the relational executor.
//!
//! One logical iteration in, RDF terms out — serialised in N-Triples form so a
//! caller can concatenate terms into a document and stream it into the store.
//!
//! **Multi-valued references.** A relational row or a CSV row has one value
//! per column, but a JSONPath or XPath reference can select several values
//! from one iteration (RML-Core §expressions). A term map then generates one
//! term per value, a template one per element of the n-ary cartesian product
//! of its references' values, and a literal with a language or datatype map
//! one per value × tag (or datatype). An iteration is read through
//! [`Record`], which a relational row ([`Cells`]) answers with at most one
//! value per column and a file iteration ([`Iteration`]) with as many as the
//! expression selected.
//!
//! Two things beyond plain templating live here because both executors need
//! them: the **natural datatype** rule (R2RML §10.2 — a column with no
//! `rr:datatype` takes the XSD type its SQL type implies; RML-IO: a JSON
//! number is an `xsd:integer` or `xsd:double`) and the **FNML function** used
//! for enumerations.
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

/// One source row: column name → lexical value. A NULL — a SQL NULL, a JSON
/// `null` or absent key, a value the logical source lists under `rml:null` —
/// is "no key", so a term map over one produces no term and therefore no
/// triple. An empty string is a value (RML-IO), except under
/// [`Semantics::Legacy`], where an empty value produces no term either.
pub type Row = HashMap<String, String>;
/// Per-column generic types, when the source reports them (relational only).
pub type Kinds = HashMap<String, ValueKind>;

/// One value a reference expression selected, with the generic type the
/// source gave it when it has types (a SQL column, a JSON number).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Val {
    pub lex: String,
    pub kind: Option<ValueKind>,
}

impl Val {
    pub fn text(lex: impl Into<String>) -> Self {
        Self {
            lex: lex.into(),
            kind: None,
        }
    }
}

/// The values a reference expression evaluates to over one logical
/// iteration, in the order the source produced them. Empty: NULL.
#[derive(Debug, Clone, Copy)]
pub enum Values<'a> {
    Empty,
    One(&'a str, Option<ValueKind>),
    Many(&'a [Val]),
}

impl<'a> Values<'a> {
    pub fn len(&self) -> usize {
        match self {
            Values::Empty => 0,
            Values::One(..) => 1,
            Values::Many(v) => v.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The `i`-th value; `i` must be below [`Values::len`].
    pub fn get(&self, i: usize) -> (&'a str, Option<ValueKind>) {
        match self {
            Values::One(v, k) => (v, *k),
            Values::Many(v) => (v[i].lex.as_str(), v[i].kind),
            Values::Empty => unreachable!("no values"),
        }
    }

    pub fn first(&self) -> Option<&'a str> {
        (!self.is_empty()).then(|| self.get(0).0)
    }

    pub fn iter(self) -> impl Iterator<Item = (&'a str, Option<ValueKind>)> {
        (0..self.len()).map(move |i| self.get(i))
    }
}

/// A logical iteration, as term maps read it.
pub trait Record {
    /// The values `reference` evaluates to. A reference the iteration does
    /// not answer is NULL.
    fn values(&self, reference: &str) -> Values<'_>;
}

/// A relational (or CSV) row: one value per column, with the column types
/// the connector reported.
#[derive(Debug, Clone, Copy)]
pub struct Cells<'a> {
    pub row: &'a Row,
    pub kinds: Option<&'a Kinds>,
}

impl Record for Cells<'_> {
    fn values(&self, reference: &str) -> Values<'_> {
        match self.row.get(reference) {
            Some(v) => Values::One(v, self.kinds.and_then(|k| k.get(reference).copied())),
            None => Values::Empty,
        }
    }
}

/// A file source's logical iteration: the values every reference the
/// triples map reads evaluated to, by the expression as the mapping wrote
/// it. A reference that selected nothing is absent.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Iteration {
    pub values: HashMap<String, Vec<Val>>,
}

impl Iteration {
    /// Drop the values that count as NULL (`rml:null`, `csvw:null`).
    pub fn drop_nulls(&mut self, nulls: &[String]) {
        if nulls.is_empty() {
            return;
        }
        self.values.retain(|_, vals| {
            vals.retain(|v| !nulls.contains(&v.lex));
            !vals.is_empty()
        });
    }
}

impl Record for Iteration {
    fn values(&self, reference: &str) -> Values<'_> {
        match self.values.get(reference) {
            Some(v) if v.len() == 1 => Values::One(&v[0].lex, v[0].kind),
            Some(v) if !v.is_empty() => Values::Many(v),
            _ => Values::Empty,
        }
    }
}

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
#[cfg(test)]
pub fn expand_slug_template(template: &str, row: &Row, semantics: Semantics) -> Option<String> {
    expand_slug_template_in(template, &Cells { row, kinds: None }, semantics)
}

fn expand_slug_template_in<R: Record + ?Sized>(
    template: &str,
    rec: &R,
    semantics: Semantics,
) -> Option<String> {
    let mut result = String::with_capacity(template.len());
    for piece in parse_template(template) {
        match piece {
            Piece::Text(t) => result.push_str(&t),
            Piece::Ref(name) => {
                let value = match rec.values(&name).first() {
                    Some(v) => Encoding::iri(semantics).apply(v),
                    None => {
                        let base = name.strip_suffix("_slug")?;
                        let slugged = slug(rec.values(base).first()?);
                        // A value that slugs to nothing names nothing.
                        (!slugged.is_empty()).then_some(slugged)?
                    }
                };
                result.push_str(&value);
            }
        }
    }
    Some(result)
}

/// `otsfn:mintIri`: an IRI from `otsfn:template` over the current row.
fn mint_iri<R: Record + ?Sized>(
    f: &FunctionMap,
    rec: &R,
    gen: &mut TermGen,
) -> Result<Option<String>, String> {
    let semantics = gen.semantics;
    let template = arg_value(f.first(&format!("{FN_NS}template")), rec).ok_or_else(|| {
        format!("{FN_LABEL}:mintIri needs {FN_LABEL}:template with an absolute IRI template")
    })?;
    if !(template.contains("://") || template.starts_with("urn:")) {
        return Err(format!(
            "{FN_LABEL}:template '{template}' is not absolute; minting under an undeclared \
             prefix would put IRIs in a namespace nobody owns"
        ));
    }
    let Some(filled) = expand_slug_template_in(&template, rec, semantics) else {
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
    /// The fresh blank node of the current iteration, for a blank-node term
    /// map with no expression (RML-Core).
    fresh: Option<String>,
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
            fresh: None,
            errors: Vec::new(),
        }
    }

    /// Record a data error: a value that cannot become the term its map asks
    /// for. The term is not generated; whether the run goes on is the
    /// caller's decision ([`OnDataError`](super::checks::OnDataError)).
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

    /// [`TermGen::apply_nulls`] for a file iteration, `csvw:null` included.
    pub fn apply_nulls_to(&self, source: &LogicalSource, it: &mut Iteration) {
        if self.semantics == Semantics::R2rml {
            it.drop_nulls(&source.nulls);
            it.drop_nulls(&source.access.dialect.nulls);
        }
    }

    pub fn semantics(&self) -> Semantics {
        self.semantics
    }

    /// A new source row begins. Legacy blank nodes do not outlive it, and
    /// neither does the iteration's fresh blank node.
    pub fn start_row(&mut self) {
        self.row.clear();
        self.fresh = None;
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

    /// The fresh blank node of the current iteration: every blank-node term
    /// map with no expression in it denotes the same new node.
    fn fresh_blank_node(&mut self) -> String {
        if let Some(f) = &self.fresh {
            return f.clone();
        }
        self.counter += 1;
        let label = format!("_:{}f{}", self.prefix, self.counter);
        self.fresh = Some(label.clone());
        label
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
    /// As it is (R2RML: a template that does not generate an IRI; RML-Core:
    /// `rml:UnsafeIRI`, `rml:UnsafeURI`).
    None,
    /// The IRI-safe version (R2RML §7.3).
    IriSafe,
    /// The URI-safe version (RML-Core: `rml:URI`).
    UriSafe,
    /// Every byte but `[A-Za-z0-9]` percent-encoded, in every template — what
    /// this engine did before, kept for legacy mapping versions.
    Legacy,
}

impl Encoding {
    fn for_term(semantics: Semantics, term_type: &TermType) -> Self {
        match (semantics, term_type) {
            (Semantics::Legacy, _) => Encoding::Legacy,
            (Semantics::R2rml, TermType::IRI) => Encoding::IriSafe,
            (Semantics::R2rml, TermType::URI) => Encoding::UriSafe,
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
            Encoding::UriSafe => iri::uri_safe(value),
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

/// Evaluate a term map against a relational row: its first term.
///
/// Returns the term in N-Triples syntax (`<iri>`, `"lex"^^<dt>`, `_:b1`), or
/// `None` when the row does not supply what the term map needs. A row has
/// one value per column, so it generates at most one term unless the term
/// map has a language or datatype map.
pub fn eval_term(
    tm: &TermMap,
    row: &Row,
    kinds: Option<&Kinds>,
    gen: &mut TermGen,
    at: At<'_>,
) -> Option<String> {
    let mut out = Vec::with_capacity(1);
    eval_terms(tm, &Cells { row, kinds }, gen, at, &mut out);
    out.into_iter().next()
}

/// Evaluate a term map against a logical iteration, appending every term it
/// generates to `out` (RML-Core §term generation rules): one per value of a
/// reference, one per element of a template's cartesian product, times the
/// language tags or datatypes of a language or datatype map. Nothing for a
/// NULL, and nothing for a value that raises a data error (which is
/// recorded on `gen`).
pub fn eval_terms<R: Record + ?Sized>(
    tm: &TermMap,
    rec: &R,
    gen: &mut TermGen,
    at: At<'_>,
    out: &mut Vec<String>,
) {
    match &tm.kind {
        // A constant is the term the mapping wrote, datatype and language
        // included (R2RML §7.4).
        TermMapKind::Constant(term) => out.extend(render_constant(term)),
        TermMapKind::Fresh => out.push(gen.fresh_blank_node()),
        TermMapKind::Template(template) => {
            let encoding = Encoding::for_term(gen.semantics, &tm.term_type);
            for value in expand_template_all(template, rec, encoding) {
                render_value(tm, value, None, rec, gen, at, out);
            }
        }
        TermMapKind::Reference(col) => {
            for (value, kind) in rec.values(col).iter() {
                let value = natural_lexical(value, kind).into_owned();
                render_value(tm, value, kind, rec, gen, at, out);
            }
        }
    }
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

/// One value of a term map as the term its term type asks for.
fn render_value<R: Record + ?Sized>(
    tm: &TermMap,
    raw_value: String,
    kind: Option<ValueKind>,
    rec: &R,
    gen: &mut TermGen,
    at: At<'_>,
    out: &mut Vec<String>,
) {
    // An empty value is a value (RML-IO: nothing is NULL unless the source
    // says so). Before, it produced no term, which a legacy version keeps.
    if raw_value.is_empty() && gen.semantics == Semantics::Legacy {
        return;
    }

    match tm.term_type {
        // Build the IRI through oxrdf so it is validated and serialised, not
        // pasted between angle brackets. A `rml:reference`/`rr:column` value
        // with `rr:termType rr:IRI` went in raw: a value containing a space
        // produced invalid Turtle and failed the WHOLE mapping, and one
        // containing `>` could terminate the IRI and inject further triples.
        // A value that is not absolute is appended to the base IRI (§11.2).
        TermType::IRI | TermType::URI | TermType::UnsafeIRI | TermType::UnsafeURI => {
            let uri = matches!(tm.term_type, TermType::URI | TermType::UnsafeURI);
            match iri::absolute_iri(&raw_value, at.base).filter(|n| !uri || n.as_str().is_ascii()) {
                Some(n) => out.push(n.to_string()),
                None => gen.data_error(format!(
                    "{} generates {}, which is not a valid {}{}",
                    describe(tm),
                    quoted(&raw_value),
                    if uri { "URI" } else { "IRI" },
                    if at.base.is_some() {
                        " (relative to the base IRI either)"
                    } else {
                        ""
                    }
                )),
            }
        }
        TermType::BlankNode => out.push(gen.blank_node(raw_value, at.graph)),
        // Likewise for literals: hand-escaping only `\` and `"` left raw
        // newlines, carriage returns and tabs in the output, which Turtle's
        // STRING_LITERAL_QUOTE forbids — so one multi-line CSV field made the
        // entire generated document unparseable and the mapping failed with
        // "Failed to load generated triples" rather than skipping a row.
        TermType::Literal => {
            if let Some(ref lang) = tm.language {
                // The parser has checked the tag, so this cannot fail on it.
                out.extend(
                    Literal::new_language_tagged_literal(&raw_value, lang)
                        .ok()
                        .map(|l| l.to_string()),
                );
            } else if let Some(lm) = &tm.language_map {
                // RML-Core: every value × every tag the language map
                // generates, each a well-formed BCP 47 tag or a data error.
                for tag in expression_values(lm, rec, gen) {
                    match Literal::new_language_tagged_literal(&raw_value, &tag)
                        .ok()
                        .filter(|_| super::parser::valid_language_tag(&tag))
                    {
                        Some(l) => out.push(l.to_string()),
                        None => gen.data_error(format!(
                            "{} generates the language tag {}, which is not a valid BCP 47 \
                             language tag",
                            describe(lm),
                            quoted(&tag)
                        )),
                    }
                }
            } else if let Some(dt) = &tm.datatype {
                out.extend(typed_literal(tm, &raw_value, dt, gen));
            } else if let Some(dm) = &tm.datatype_map {
                let mut datatypes = Vec::new();
                eval_terms(
                    dm,
                    rec,
                    gen,
                    At {
                        base: at.base,
                        graph: None,
                    },
                    &mut datatypes,
                );
                for dt in datatypes {
                    let Some(dt) = dt.strip_prefix('<').and_then(|d| d.strip_suffix('>')) else {
                        continue;
                    };
                    out.extend(typed_literal(tm, &raw_value, dt, gen));
                }
            } else if let Some(dt) = natural_datatype_for(tm, kind) {
                out.extend(
                    NamedNode::new(format!("{XSD}{dt}"))
                        .ok()
                        .map(|n| Literal::new_typed_literal(&raw_value, n).to_string()),
                );
            } else {
                out.push(Literal::new_simple_literal(&raw_value).to_string());
            }
        }
    }
}

/// A datatype-override literal (R2RML §10.3): a data error when the value is
/// not in the datatype's lexical space.
fn typed_literal(tm: &TermMap, raw_value: &str, dt: &str, gen: &mut TermGen) -> Option<String> {
    if let Some(why) = super::xsd::ill_typed(raw_value, dt) {
        gen.data_error(format!(
            "{} generates {} as <{dt}>, which is {why}",
            describe(tm),
            quoted(raw_value)
        ));
        return None;
    }
    Some(Literal::new_typed_literal(raw_value, NamedNode::new(dt).ok()?).to_string())
}

/// The plain string values an expression map (a language map) evaluates
/// to: a constant's lexical form, a reference's values, a template's
/// product — no term types, no encoding.
fn expression_values<R: Record + ?Sized>(m: &TermMap, rec: &R, gen: &mut TermGen) -> Vec<String> {
    match &m.kind {
        TermMapKind::Constant(Term::Literal(l)) => vec![l.value().to_string()],
        TermMapKind::Constant(Term::NamedNode(n)) => vec![n.as_str().to_string()],
        TermMapKind::Constant(_) | TermMapKind::Fresh => Vec::new(),
        TermMapKind::Reference(c) => rec
            .values(c)
            .iter()
            .map(|(v, _)| v.to_string())
            .filter(|v| !(v.is_empty() && gen.semantics == Semantics::Legacy))
            .collect(),
        TermMapKind::Template(t) => expand_template_all(t, rec, Encoding::None),
    }
}

/// The values one side of a join condition evaluates to: a reference's
/// values, a template's product (as it is, unencoded), or a constant.
pub fn join_side_values<R: Record + ?Sized>(side: &JoinSide, rec: &R) -> Vec<String> {
    match side {
        JoinSide::Reference(c) => rec.values(c).iter().map(|(v, _)| v.to_string()).collect(),
        JoinSide::Template(t) => expand_template_all(t, rec, Encoding::None),
        JoinSide::Constant(c) => vec![c.clone()],
    }
}

/// A term map as an error message names it.
fn describe(tm: &TermMap) -> String {
    match &tm.kind {
        TermMapKind::Template(t) => format!("rr:template \"{t}\""),
        TermMapKind::Reference(c) => format!("column \"{c}\""),
        TermMapKind::Constant(c) => format!("rr:constant {c}"),
        TermMapKind::Fresh => "a blank-node term map".to_string(),
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

/// The natural datatype for a direct reference with no declared one. Only a
/// plain reference gets it: a template composes text, and a constant is
/// whatever the mapping author wrote.
fn natural_datatype_for(tm: &TermMap, kind: Option<ValueKind>) -> Option<&'static str> {
    if !matches!(tm.kind, TermMapKind::Reference(_)) {
        return None;
    }
    natural_datatype(kind?)
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
    if matches!(tm.kind, TermMapKind::Fresh) {
        return false;
    }
    match tm.term_type {
        TermType::BlankNode => semantics == Semantics::R2rml,
        TermType::Literal => false,
        _ => true,
    }
}

/// Evaluate a term map that must yield IRIs, returning them without the
/// angle brackets (for graph names).
pub fn eval_iris<R: Record + ?Sized>(
    tm: &TermMap,
    rec: &R,
    gen: &mut TermGen,
    base: Option<&str>,
) -> Vec<String> {
    let mut rendered = Vec::new();
    eval_terms(tm, rec, gen, At { base, graph: None }, &mut rendered);
    rendered
        .into_iter()
        .filter_map(|r| {
            r.strip_prefix('<')
                .and_then(|s| s.strip_suffix('>'))
                .map(str::to_string)
        })
        .collect()
}

/// One part of a parsed string template.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Piece {
    Text(String),
    Ref(String),
}

/// Split a template into its text and its `{reference}` placeholders. A
/// backslash escapes the next character, inside a placeholder as well as
/// outside it (RML-Core §template).
fn parse_template(template: &str) -> Vec<Piece> {
    let mut out = Vec::new();
    let mut text = String::new();
    let mut chars = template.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                if let Some(next) = chars.next() {
                    text.push(next);
                }
            }
            '{' => {
                let mut name = String::new();
                while let Some(inner) = chars.next() {
                    match inner {
                        '\\' => {
                            if let Some(next) = chars.next() {
                                name.push(next);
                            }
                        }
                        '}' => break,
                        other => name.push(other),
                    }
                }
                if !text.is_empty() {
                    out.push(Piece::Text(std::mem::take(&mut text)));
                }
                out.push(Piece::Ref(name));
            }
            other => text.push(other),
        }
    }
    if !text.is_empty() {
        out.push(Piece::Text(text));
    }
    out
}

/// Expand a template: every element of the cartesian product of its
/// references' values, each value written in as `encoding` says. Empty when
/// any reference is NULL.
fn expand_template_all<R: Record + ?Sized>(
    template: &str,
    rec: &R,
    encoding: Encoding,
) -> Vec<String> {
    let pieces = parse_template(template);
    let mut results = vec![String::with_capacity(template.len())];
    for piece in &pieces {
        match piece {
            Piece::Text(t) => results.iter_mut().for_each(|r| r.push_str(t)),
            Piece::Ref(name) => {
                let values = rec.values(name);
                match values.len() {
                    0 => return Vec::new(),
                    1 => {
                        let (v, kind) = values.get(0);
                        let v = encoding.apply(&natural_lexical(v, kind));
                        results.iter_mut().for_each(|r| r.push_str(&v));
                    }
                    _ => {
                        let encoded: Vec<String> = values
                            .iter()
                            .map(|(v, kind)| encoding.apply(&natural_lexical(v, kind)))
                            .collect();
                        results = results
                            .into_iter()
                            .flat_map(|r| encoded.iter().map(move |v| format!("{r}{v}")))
                            .collect();
                    }
                }
            }
        }
    }
    results
}

/// Expand an `rr:template` over a relational row: replace `{column}` with the
/// row's value, encoded as `encoding` says. `\{` and `\}` are literal
/// braces. A column the row does not supply yields `None`, which skips the
/// term (and so the triple).
#[cfg(test)]
fn expand_template_encoded(template: &str, row: &Row, encoding: Encoding) -> Option<String> {
    expand_template_all(template, &Cells { row, kinds: None }, encoding)
        .into_iter()
        .next()
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

/// Evaluate an FNML function over a relational row to an N-Triples term.
#[cfg(test)]
pub fn eval_function(
    f: &FunctionMap,
    row: &Row,
    kinds: Option<&Kinds>,
    gen: &mut TermGen,
) -> Result<Option<String>, String> {
    eval_function_in(f, &Cells { row, kinds }, gen)
}

/// Evaluate an FNML function over a logical iteration to an N-Triples term.
/// A parameter that reads a reference takes the reference's first value.
pub fn eval_function_in<R: Record + ?Sized>(
    f: &FunctionMap,
    rec: &R,
    gen: &mut TermGen,
) -> Result<Option<String>, String> {
    let semantics = gen.semantics;
    if f.function == FN_MINT_IRI {
        return mint_iri(f, rec, gen);
    }
    if f.function != FN_MAP_VALUE {
        return Err(format!(
            "unsupported function <{}>; this engine implements <{FN_MAP_VALUE}> and <{FN_MINT_IRI}>",
            f.function
        ));
    }
    let Some(raw) = arg_value(f.first(&format!("{FN_NS}value")), rec) else {
        // No input value (a NULL column) → no triple, like any term map.
        return Ok(None);
    };
    if raw.is_empty() && semantics == Semantics::Legacy {
        return Ok(None);
    }

    let normalize =
        arg_value(f.first(&format!("{FN_NS}normalize")), rec).unwrap_or_else(|| "none".to_string());
    let key = normalize_value(&raw, &normalize)?;

    let mut table: HashMap<String, String> = HashMap::new();
    for entry in f.all(&format!("{FN_NS}mapping")) {
        let Some(text) = arg_value(Some(entry), rec) else {
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

    match unmapped_policy(f, rec)? {
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
                let values = rec.values(col);
                let (_, kind) = (!values.is_empty()).then(|| values.get(0))?;
                natural_datatype(kind?).map(|d| format!("{XSD}{d}"))
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
            let expanded = expand_template_all(&t, rec, encoding)
                .into_iter()
                .next()
                .unwrap_or_default();
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

fn unmapped_policy<R: Record + ?Sized>(f: &FunctionMap, rec: &R) -> Result<UnmappedPolicy, String> {
    let policy = arg_value(f.first(&format!("{FN_NS}unmapped")), rec)
        .unwrap_or_else(|| "literal".to_string());
    match policy.trim().to_ascii_lowercase().as_str() {
        "literal" | "" => Ok(UnmappedPolicy::Literal),
        "omit" | "skip" => Ok(UnmappedPolicy::Omit),
        "template" | "mint" => {
            let t =
                arg_value(f.first(&format!("{FN_NS}unmappedTemplate")), rec).ok_or_else(|| {
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

fn arg_value<R: Record + ?Sized>(arg: Option<&FunctionArg>, rec: &R) -> Option<String> {
    match arg? {
        FunctionArg::Constant(c) => Some(c.clone()),
        FunctionArg::Reference(col) => rec.values(col).first().map(str::to_string),
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
        TermMap::new(kind, tt)
    }

    fn eval(tm: &TermMap, r: &Row, kinds: Option<&Kinds>) -> Option<String> {
        eval_as(Semantics::R2rml, tm, r, kinds)
    }

    fn eval_as(s: Semantics, tm: &TermMap, r: &Row, kinds: Option<&Kinds>) -> Option<String> {
        let mut g = TermGen::new(s, "b");
        eval_term(tm, r, kinds, &mut g, At::default())
    }

    fn iteration(pairs: &[(&str, &[&str])]) -> Iteration {
        Iteration {
            values: pairs
                .iter()
                .map(|(k, vs)| (k.to_string(), vs.iter().map(|v| Val::text(*v)).collect()))
                .collect(),
        }
    }

    fn all(tm: &TermMap, it: &Iteration) -> Vec<String> {
        let mut g = TermGen::new(Semantics::R2rml, "b");
        let mut out = Vec::new();
        eval_terms(tm, it, &mut g, At::default(), &mut out);
        out
    }

    #[test]
    fn a_multi_valued_reference_makes_a_term_per_value() {
        let it = iteration(&[("tags", &["a", "b"])]);
        assert_eq!(
            all(
                &term(TermMapKind::Reference("tags".into()), TermType::Literal),
                &it
            ),
            vec!["\"a\"", "\"b\""]
        );
    }

    #[test]
    fn a_template_is_the_cartesian_product_of_its_references() {
        // RML-Core §template value: the n-ary cartesian product.
        let it = iteration(&[("x", &["1", "2"]), ("y", &["a", "b", "c"]), ("z", &["Z"])]);
        let tm = term(
            TermMapKind::Template("http://e/{x}/{y}/{z}".into()),
            TermType::IRI,
        );
        let terms = all(&tm, &it);
        assert_eq!(terms.len(), 6);
        assert!(terms.contains(&"<http://e/2/c/Z>".to_string()));
        // A reference with no value leaves no term at all.
        let none = iteration(&[("x", &["1"])]);
        assert!(all(&tm, &none).is_empty());
    }

    #[test]
    fn language_and_datatype_maps_multiply_the_values() {
        let it = iteration(&[("v", &["x"]), ("l", &["en", "nl"]), ("bad", &["not a tag"])]);
        let mut tm = term(TermMapKind::Reference("v".into()), TermType::Literal);
        tm.language_map = Some(Box::new(term(
            TermMapKind::Reference("l".into()),
            TermType::Literal,
        )));
        assert_eq!(all(&tm, &it), vec!["\"x\"@en", "\"x\"@nl"]);
        tm.language_map = Some(Box::new(term(
            TermMapKind::Reference("bad".into()),
            TermType::Literal,
        )));
        let mut g = TermGen::new(Semantics::R2rml, "b");
        let mut out = Vec::new();
        eval_terms(&tm, &it, &mut g, At::default(), &mut out);
        assert!(out.is_empty());
        assert!(g.take_errors()[0].contains("BCP 47"));
        let mut typed = term(TermMapKind::Reference("v".into()), TermType::Literal);
        typed.datatype_map = Some(Box::new(term(
            TermMapKind::Template("http://example.org/dt/{l}".into()),
            TermType::IRI,
        )));
        assert_eq!(
            all(&typed, &it),
            vec![
                "\"x\"^^<http://example.org/dt/en>",
                "\"x\"^^<http://example.org/dt/nl>"
            ]
        );
    }

    #[test]
    fn sql_timestamps_and_booleans_take_their_natural_lexical_form() {
        // R2RML §10.2: a timestamp's space becomes T; a boolean is true/false.
        let r = row(&[("ts", "2009-10-10 12:12:22"), ("b", "0")]);
        let kinds: Kinds = HashMap::from([
            ("ts".to_string(), ValueKind::DateTime),
            ("b".to_string(), ValueKind::Boolean),
        ]);
        assert_eq!(
            eval(
                &term(TermMapKind::Reference("ts".into()), TermType::Literal),
                &r,
                Some(&kinds)
            )
            .unwrap(),
            format!("\"2009-10-10T12:12:22\"^^<{XSD}dateTime>")
        );
        assert_eq!(
            eval(
                &term(TermMapKind::Reference("b".into()), TermType::Literal),
                &r,
                Some(&kinds)
            )
            .unwrap(),
            format!("\"false\"^^<{XSD}boolean>")
        );
        assert_eq!(
            eval(
                &term(TermMapKind::Template("http://x/{ts}".into()), TermType::IRI),
                &r,
                Some(&kinds)
            )
            .unwrap(),
            "<http://x/2009-10-10T12%3A12%3A22>"
        );
    }

    #[test]
    fn a_fresh_blank_node_is_one_per_iteration() {
        let mut g = TermGen::new(Semantics::R2rml, "b");
        let tm = term(TermMapKind::Fresh, TermType::BlankNode);
        let it = Iteration::default();
        let mut a = Vec::new();
        eval_terms(&tm, &it, &mut g, At::default(), &mut a);
        eval_terms(&tm, &it, &mut g, At::default(), &mut a);
        assert_eq!(a[0], a[1], "the same iteration, the same node");
        g.start_row();
        let mut b = Vec::new();
        eval_terms(&tm, &it, &mut g, At::default(), &mut b);
        assert_ne!(a[0], b[0], "a new iteration, a new node");
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
            access: Access::default(),
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
