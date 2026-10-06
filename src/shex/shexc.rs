//! ShExC — the ShEx 2.1 compact syntax (§6 of the ShEx 2.1 specification).
//!
//! A scanner-driven recursive-descent parser. The grammar's terminals depend
//! on context (`{` opens a shape, a REPEAT_RANGE or, after `%iri`, a CODE
//! block; `/` opens a REGEXP or a `//` annotation), so the parser reads
//! characters directly instead of running a separate tokenizer.
//!
//! The parser checks the grammar and the rules the grammar section states
//! (prefixes declared, labels unique across shape and triple expressions,
//! value-set exclusions of one kind). Reference resolution, imports and the
//! negation requirement are checked later, on the whole schema
//! ([`super::check`]). ShEx 2.next syntax (`EXTENDS`, `ABSTRACT`,
//! `RESTRICTS`) is rejected with a message naming it.

use std::collections::{HashMap, HashSet};

use super::ast::*;
use super::xsd::Numeric;

const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";

/// Parse a ShExC document. `base` resolves relative IRIs until a `BASE`
/// directive replaces it; without one, a relative IRI is an error.
pub fn parse(text: &str, base: Option<&str>) -> Result<Schema, String> {
    let mut p = Parser {
        s: text,
        pos: 0,
        base: base.map(str::to_string),
        prefixes: HashMap::new(),
        prefix_order: Vec::new(),
    };
    let schema = p.doc().map_err(|e| p.located(&e))?;
    check_label_collisions(&schema)?;
    Ok(schema)
}

struct Parser<'a> {
    s: &'a str,
    pos: usize,
    base: Option<String>,
    prefixes: HashMap<String, String>,
    prefix_order: Vec<(String, String)>,
}

type R<T> = Result<T, String>;

fn is_pn_chars_base(c: char) -> bool {
    matches!(c,
        'A'..='Z' | 'a'..='z'
        | '\u{00C0}'..='\u{00D6}' | '\u{00D8}'..='\u{00F6}' | '\u{00F8}'..='\u{02FF}'
        | '\u{0370}'..='\u{037D}' | '\u{037F}'..='\u{1FFF}' | '\u{200C}'..='\u{200D}'
        | '\u{2070}'..='\u{218F}' | '\u{2C00}'..='\u{2FEF}' | '\u{3001}'..='\u{D7FF}'
        | '\u{F900}'..='\u{FDCF}' | '\u{FDF0}'..='\u{FFFD}' | '\u{10000}'..='\u{EFFFF}')
}

fn is_pn_chars_u(c: char) -> bool {
    is_pn_chars_base(c) || c == '_'
}

fn is_pn_chars(c: char) -> bool {
    is_pn_chars_u(c)
        || c == '-'
        || c.is_ascii_digit()
        || c == '\u{00B7}'
        || ('\u{0300}'..='\u{036F}').contains(&c)
        || ('\u{203F}'..='\u{2040}').contains(&c)
}

/// The characters a `PN_LOCAL_ESC` may escape.
fn is_local_esc(c: char) -> bool {
    "_~.-!$&'()*+,;=/?#@%".contains(c)
}

impl<'a> Parser<'a> {
    // ── scanning ──────────────────────────────────────────────────────────

    fn rest(&self) -> &'a str {
        &self.s[self.pos..]
    }

    fn peek(&self) -> Option<char> {
        self.rest().chars().next()
    }

    fn peek_at(&self, n: usize) -> Option<char> {
        self.rest().chars().nth(n)
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += c.len_utf8();
        Some(c)
    }

    fn located(&self, msg: &str) -> String {
        let before = &self.s[..self.pos.min(self.s.len())];
        let line = before.matches('\n').count() + 1;
        let col = before.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
        format!("ShExC syntax error at line {line}, column {col}: {msg}")
    }

    /// Skip whitespace and comments (`# …` and `/* … */`).
    fn ws(&mut self) {
        loop {
            match self.peek() {
                Some(c) if c.is_whitespace() => {
                    self.bump();
                }
                Some('#') => {
                    while let Some(c) = self.peek() {
                        if c == '\n' || c == '\r' {
                            break;
                        }
                        self.bump();
                    }
                }
                Some('/') if self.peek_at(1) == Some('*') => {
                    self.pos += 2;
                    match self.rest().find("*/") {
                        Some(i) => self.pos += i + 2,
                        None => self.pos = self.s.len(),
                    }
                }
                _ => return,
            }
        }
    }

    fn at(&mut self, c: char) -> bool {
        self.ws();
        self.peek() == Some(c)
    }

    fn eat(&mut self, c: char) -> bool {
        if self.at(c) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, c: char) -> R<()> {
        if self.eat(c) {
            Ok(())
        } else {
            Err(format!("expected '{c}', found {}", self.found()))
        }
    }

    fn found(&self) -> String {
        match self.peek() {
            None => "end of input".to_string(),
            Some(_) => {
                let t: String = self.rest().chars().take(20).collect();
                format!("'{t}'")
            }
        }
    }

    /// A case-insensitive keyword not followed by a name character.
    fn at_keyword(&mut self, kw: &str) -> bool {
        self.ws();
        let r = self.rest();
        if r.len() < kw.len() || !r.is_char_boundary(kw.len()) {
            return false;
        }
        if !r[..kw.len()].eq_ignore_ascii_case(kw) {
            return false;
        }
        !r[kw.len()..]
            .chars()
            .next()
            .is_some_and(|c| is_pn_chars(c) || c == ':' || c == '.' || c == '\\' || c == '%')
    }

    fn eat_keyword(&mut self, kw: &str) -> bool {
        if self.at_keyword(kw) {
            self.pos += kw.len();
            true
        } else {
            false
        }
    }

    // ── document ──────────────────────────────────────────────────────────

    fn doc(&mut self) -> R<Schema> {
        let mut schema = Schema::default();
        let mut seen_statement = false;
        loop {
            self.ws();
            if self.peek().is_none() {
                break;
            }
            if self.directive(&mut schema)? {
                continue;
            }
            if self.at('%') {
                if seen_statement {
                    return Err(
                        "a semantic action here must follow a shape definition's '}'".into(),
                    );
                }
                while self.at('%') {
                    schema.start_acts.push(self.code_decl()?);
                }
                seen_statement = true;
                continue;
            }
            if self.at_keyword("start") && {
                let save = self.pos;
                self.pos += 5;
                let eq = self.at('=');
                self.pos = save;
                eq
            } {
                self.pos += 5;
                self.expect('=')?;
                if schema.start.is_some() {
                    return Err("start defined twice".into());
                }
                schema.start = Some(self.shape_or(true)?);
                seen_statement = true;
                continue;
            }
            for kw in ["ABSTRACT", "RESTRICTS"] {
                if self.at_keyword(kw) {
                    return Err(format!(
                        "{kw} is ShEx 2.next syntax, which this engine does not implement"
                    ));
                }
            }
            let label = self.shape_expr_label()?;
            let expr = if self.eat_keyword("EXTERNAL") {
                ShapeExpr::External
            } else {
                self.shape_or(false)?
            };
            schema.shapes.push(ShapeDecl { label, expr });
            seen_statement = true;
        }
        schema.prefixes = std::mem::take(&mut self.prefix_order);
        schema.base = self.base.clone();
        Ok(schema)
    }

    /// `BASE`, `PREFIX` or `IMPORT`; `false` when none is next.
    fn directive(&mut self, schema: &mut Schema) -> R<bool> {
        if self.eat_keyword("BASE") {
            self.ws();
            let iri = self.iriref()?;
            self.base = Some(iri);
            return Ok(true);
        }
        if self.eat_keyword("PREFIX") {
            self.ws();
            let prefix = self.pname_ns_decl()?;
            self.ws();
            let iri = self.iriref()?;
            self.prefixes.insert(prefix.clone(), iri.clone());
            self.prefix_order.retain(|(p, _)| *p != prefix);
            self.prefix_order.push((prefix, iri));
            return Ok(true);
        }
        if self.eat_keyword("IMPORT") {
            self.ws();
            let iri = self.iri()?;
            schema.imports.push(iri);
            return Ok(true);
        }
        Ok(false)
    }

    // ── shape expressions ────────────────────────────────────────────────

    fn shape_or(&mut self, inline: bool) -> R<ShapeExpr> {
        let first = self.shape_and(inline)?;
        if !self.at_keyword("OR") {
            return Ok(first);
        }
        let mut items = vec![first];
        while self.eat_keyword("OR") {
            items.push(self.shape_and(inline)?);
        }
        Ok(ShapeExpr::Or(items))
    }

    fn shape_and(&mut self, inline: bool) -> R<ShapeExpr> {
        let (first, combo) = self.shape_not(inline)?;
        if !self.at_keyword("AND") {
            return Ok(first);
        }
        // The conjunction an atom builds (`IRI @<S>`, `BNODE { … }`) joins
        // the surrounding AND, as in ShExJ; a parenthesised one does not.
        let mut items = Vec::new();
        let push = |e: ShapeExpr, combo: bool, items: &mut Vec<ShapeExpr>| match e {
            ShapeExpr::And(v) if combo => items.extend(v),
            e => items.push(e),
        };
        push(first, combo, &mut items);
        while self.eat_keyword("AND") {
            let (e, combo) = self.shape_not(inline)?;
            push(e, combo, &mut items);
        }
        Ok(ShapeExpr::And(items))
    }

    /// The expression, and whether it is an atom's node-constraint-and-shape
    /// conjunction.
    fn shape_not(&mut self, inline: bool) -> R<(ShapeExpr, bool)> {
        if self.eat_keyword("NOT") {
            Ok((ShapeExpr::Not(Box::new(self.shape_atom(inline)?.0)), false))
        } else {
            self.shape_atom(inline)
        }
    }

    fn shape_atom(&mut self, inline: bool) -> R<(ShapeExpr, bool)> {
        self.ws();
        if self.eat('(') {
            let e = self.shape_or(false)?;
            self.expect(')')?;
            return Ok((e, false));
        }
        if self.at('.') && !self.peek_at(1).is_some_and(|c| c.is_ascii_digit()) {
            self.bump();
            return Ok((ShapeExpr::empty_shape(), false));
        }
        if let Some(nc) = self.non_lit_node_constraint()? {
            return Ok(match self.shape_or_ref(inline)? {
                Some(s) => (ShapeExpr::And(vec![ShapeExpr::NodeConstraint(nc), s]), true),
                None => (ShapeExpr::NodeConstraint(self.nc_tail(nc, inline)?), false),
            });
        }
        if let Some(nc) = self.lit_node_constraint()? {
            return Ok((ShapeExpr::NodeConstraint(self.nc_tail(nc, inline)?), false));
        }
        if let Some(s) = self.shape_or_ref(inline)? {
            return Ok(match self.non_lit_node_constraint()? {
                Some(nc) => (ShapeExpr::And(vec![s, ShapeExpr::NodeConstraint(nc)]), true),
                None => (s, false),
            });
        }
        Err(format!(
            "expected a shape expression, found {}",
            self.found()
        ))
    }

    /// Annotations and semantic actions after a node constraint outside a
    /// triple constraint (`<S> [1 2] // ex:a 1 %ex:act{ … %}`).
    fn nc_tail(&mut self, mut nc: NodeConstraint, inline: bool) -> R<NodeConstraint> {
        if !inline {
            nc.annotations = self.annotations()?;
            nc.sem_acts = self.sem_acts()?;
        }
        Ok(nc)
    }

    fn shape_or_ref(&mut self, inline: bool) -> R<Option<ShapeExpr>> {
        self.ws();
        if self.at_keyword("EXTENDS") || (self.at('&') && !inline) {
            return Err(
                "EXTENDS is ShEx 2.next syntax, which this engine does not implement".into(),
            );
        }
        if self.at('{') || self.at_keyword("EXTRA") || self.at_keyword("CLOSED") {
            return Ok(Some(self.shape_definition(inline)?));
        }
        if self.at('@') {
            return Ok(Some(ShapeExpr::Ref(self.shape_ref()?)));
        }
        Ok(None)
    }

    fn shape_ref(&mut self) -> R<Label> {
        self.expect('@')?;
        self.ws();
        match self.peek() {
            Some('<') => Ok(Label::Iri(self.iriref()?)),
            Some('_') if self.peek_at(1) == Some(':') => Ok(Label::BNode(self.bnode_label()?)),
            _ => Ok(Label::Iri(self.pname()?)),
        }
    }

    fn shape_expr_label(&mut self) -> R<Label> {
        self.ws();
        match self.peek() {
            Some('_') if self.peek_at(1) == Some(':') => Ok(Label::BNode(self.bnode_label()?)),
            Some('<') => Ok(Label::Iri(self.iriref()?)),
            Some(c) if is_pn_chars_base(c) || c == ':' => Ok(Label::Iri(self.pname()?)),
            _ => Err(format!(
                "expected a shape label, a directive or 'start', found {}",
                self.found()
            )),
        }
    }

    fn shape_definition(&mut self, inline: bool) -> R<ShapeExpr> {
        let mut shape = Shape::default();
        loop {
            if self.eat_keyword("EXTRA") {
                let mut n = 0;
                while let Some(p) = self.try_predicate()? {
                    shape.extra.push(p);
                    n += 1;
                }
                if n == 0 {
                    return Err("EXTRA needs at least one predicate".into());
                }
            } else if self.eat_keyword("CLOSED") {
                shape.closed = true;
            } else if self.at_keyword("EXTENDS") || self.at('&') {
                return Err(
                    "EXTENDS is ShEx 2.next syntax, which this engine does not implement".into(),
                );
            } else {
                break;
            }
        }
        self.expect('{')?;
        if !self.at('}') {
            shape.expression = Some(self.triple_expression()?);
        }
        self.expect('}')?;
        if !inline {
            shape.annotations = self.annotations()?;
            shape.sem_acts = self.sem_acts()?;
        }
        Ok(ShapeExpr::Shape(Box::new(shape)))
    }

    // ── node constraints ─────────────────────────────────────────────────

    fn non_lit_node_constraint(&mut self) -> R<Option<NodeConstraint>> {
        let kind = if self.eat_keyword("IRI") {
            Some(NodeKind::Iri)
        } else if self.eat_keyword("BNODE") {
            Some(NodeKind::BNode)
        } else if self.eat_keyword("NONLITERAL") {
            Some(NodeKind::NonLiteral)
        } else {
            None
        };
        let mut nc = NodeConstraint {
            node_kind: kind,
            ..Default::default()
        };
        while let Some(f) = self.string_facet()? {
            nc.string_facets.push(f);
        }
        if kind.is_none() && nc.string_facets.is_empty() {
            return Ok(None);
        }
        if self.at_numeric_facet() {
            return Err("numeric facets do not apply to IRIs or blank nodes".into());
        }
        check_facets(&nc)?;
        nc.normalize();
        Ok(Some(nc))
    }

    fn lit_node_constraint(&mut self) -> R<Option<NodeConstraint>> {
        let mut nc = NodeConstraint::default();
        if self.eat_keyword("LITERAL") {
            nc.node_kind = Some(NodeKind::Literal);
        } else if self.at('[') {
            nc.values = Some(self.value_set()?);
        } else if self.at_numeric_facet() {
            // numericFacet+ alone.
        } else if self.at_iri_start() {
            nc.datatype = Some(self.iri()?);
        } else {
            return Ok(None);
        }
        loop {
            if let Some(f) = self.string_facet()? {
                nc.string_facets.push(f);
            } else if let Some(f) = self.numeric_facet()? {
                nc.numeric_facets.push(f);
            } else {
                break;
            }
        }
        if nc.is_empty() {
            return Err(format!(
                "expected a node constraint, found {}",
                self.found()
            ));
        }
        check_facets(&nc)?;
        nc.normalize();
        Ok(Some(nc))
    }

    fn at_iri_start(&mut self) -> bool {
        self.ws();
        match self.peek() {
            Some('<') => true,
            Some(':') => true,
            Some(c) if is_pn_chars_base(c) => {
                // A prefixed name: PN_PREFIX followed by ':'.
                let r = self.rest();
                let mut end = 0;
                for (i, ch) in r.char_indices() {
                    if is_pn_chars(ch) || ch == '.' {
                        end = i + ch.len_utf8();
                    } else {
                        break;
                    }
                }
                r[end..].starts_with(':')
            }
            _ => false,
        }
    }

    fn at_numeric_facet(&mut self) -> bool {
        [
            "MININCLUSIVE",
            "MINEXCLUSIVE",
            "MAXINCLUSIVE",
            "MAXEXCLUSIVE",
            "TOTALDIGITS",
            "FRACTIONDIGITS",
        ]
        .iter()
        .any(|k| self.at_keyword(k))
    }

    fn string_facet(&mut self) -> R<Option<StringFacet>> {
        for (kw, mk) in [
            ("LENGTH", StringFacet::Length as fn(u64) -> StringFacet),
            ("MINLENGTH", StringFacet::MinLength),
            ("MAXLENGTH", StringFacet::MaxLength),
        ] {
            if self.eat_keyword(kw) {
                return Ok(Some(mk(self.non_negative_integer()?)));
            }
        }
        // ShEx 2.0's `PATTERN "regex"`, kept for compatibility.
        if self.eat_keyword("PATTERN") {
            self.ws();
            let lit = self.literal()?;
            return Ok(Some(StringFacet::Pattern(lit.value, None)));
        }
        self.ws();
        if self.peek() == Some('/') && self.peek_at(1) != Some('/') && self.peek_at(1) != Some('*')
        {
            let (pattern, flags) = self.regexp()?;
            return Ok(Some(StringFacet::Pattern(pattern, flags)));
        }
        Ok(None)
    }

    fn numeric_facet(&mut self) -> R<Option<NumericFacet>> {
        for (kw, mk) in [
            (
                "MININCLUSIVE",
                NumericFacet::MinInclusive as fn(Numeric) -> NumericFacet,
            ),
            ("MINEXCLUSIVE", NumericFacet::MinExclusive),
            ("MAXINCLUSIVE", NumericFacet::MaxInclusive),
            ("MAXEXCLUSIVE", NumericFacet::MaxExclusive),
        ] {
            if self.eat_keyword(kw) {
                return Ok(Some(mk(self.numeric_literal()?)));
            }
        }
        if self.eat_keyword("TOTALDIGITS") {
            return Ok(Some(NumericFacet::TotalDigits(
                self.non_negative_integer()?,
            )));
        }
        if self.eat_keyword("FRACTIONDIGITS") {
            return Ok(Some(NumericFacet::FractionDigits(
                self.non_negative_integer()?,
            )));
        }
        Ok(None)
    }

    /// A facet bound: a numeric token, or a literal typed with a numeric
    /// XSD datatype whose lexical form is valid.
    fn numeric_literal(&mut self) -> R<Numeric> {
        self.ws();
        if matches!(self.peek(), Some('"' | '\'')) {
            let lit = self.literal()?;
            let dt = lit.datatype_iri().to_string();
            return super::xsd::numeric_value(&lit.value, &dt)
                .ok_or_else(|| format!("\"{}\"^^<{dt}> is not a numeric literal", lit.value));
        }
        let tok = self.number_token()?;
        Numeric::parse_token(&tok).ok_or_else(|| format!("bad number {tok}"))
    }

    fn non_negative_integer(&mut self) -> R<u64> {
        self.ws();
        let tok = self.number_token()?;
        if !tok.bytes().all(|b| b.is_ascii_digit()) {
            return Err(format!("expected a non-negative integer, found {tok}"));
        }
        tok.parse()
            .map_err(|_| format!("integer {tok} is out of range"))
    }

    /// INTEGER | DECIMAL | DOUBLE, not followed by a name character.
    fn number_token(&mut self) -> R<String> {
        let start = self.pos;
        if matches!(self.peek(), Some('+' | '-')) {
            self.bump();
        }
        let mut digits = 0;
        while self.peek().is_some_and(|c| c.is_ascii_digit()) {
            self.bump();
            digits += 1;
        }
        if self.peek() == Some('.') && self.peek_at(1).is_some_and(|c| c.is_ascii_digit()) {
            self.bump();
            while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                self.bump();
                digits += 1;
            }
        }
        if digits == 0 {
            self.pos = start;
            return Err(format!("expected a number, found {}", self.found()));
        }
        if matches!(self.peek(), Some('e' | 'E')) {
            let save = self.pos;
            self.bump();
            if matches!(self.peek(), Some('+' | '-')) {
                self.bump();
            }
            if self.peek().is_some_and(|c| c.is_ascii_digit()) {
                while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                    self.bump();
                }
            } else {
                self.pos = save;
                return Err("malformed exponent".into());
            }
        }
        if self
            .peek()
            .is_some_and(|c| is_pn_chars_base(c) || c == '_' || c == ':')
        {
            let next = self.peek().map_or(0, char::len_utf8);
            return Err(format!(
                "malformed number '{}'",
                &self.s[start..self.pos + next]
            ));
        }
        Ok(self.s[start..self.pos].to_string())
    }

    fn regexp(&mut self) -> R<(String, Option<String>)> {
        self.bump(); // '/'
        let mut out = String::new();
        loop {
            match self.bump() {
                None | Some('\n') | Some('\r') => {
                    return Err("unterminated regular expression".into())
                }
                Some('/') => break,
                Some('\\') => match self.peek() {
                    Some('/') => {
                        self.bump();
                        out.push('/');
                    }
                    Some('u') | Some('U') => out.push(self.uchar()?),
                    Some(c) if "nrt\\|.?*+(){}$-[]^".contains(c) || "dDsSwWiIcCpP".contains(c) => {
                        self.bump();
                        out.push('\\');
                        out.push(c);
                    }
                    _ => return Err("invalid escape in regular expression".into()),
                },
                Some(c) => out.push(c),
            }
        }
        if out.is_empty() {
            return Err("empty regular expression".into());
        }
        let mut flags = String::new();
        while let Some(c) = self.peek() {
            if "smix".contains(c) {
                flags.push(c);
                self.bump();
            } else {
                break;
            }
        }
        Ok((out, (!flags.is_empty()).then_some(flags)))
    }

    // ── value sets ───────────────────────────────────────────────────────

    fn value_set(&mut self) -> R<Vec<ValueSetValue>> {
        self.expect('[')?;
        let mut out = Vec::new();
        loop {
            self.ws();
            if self.eat(']') {
                return Ok(out);
            }
            out.push(self.value_set_value()?);
        }
    }

    fn value_set_value(&mut self) -> R<ValueSetValue> {
        self.ws();
        // `.` with exclusions.
        if self.peek() == Some('.') {
            self.bump();
            let excls = self.exclusions()?;
            let Some(first) = excls.first() else {
                return Err("'.' in a value set needs at least one exclusion".into());
            };
            let kind = first.0;
            if excls.iter().any(|e| e.0 != kind) {
                return Err("value-set exclusions mix IRIs, literals and languages".into());
            }
            let ex: Vec<Exclusion> = excls.into_iter().map(|e| e.1).collect();
            return Ok(match kind {
                ExKind::Iri => ValueSetValue::IriStemRange(Stem::Wildcard, ex),
                ExKind::Literal => ValueSetValue::LiteralStemRange(Stem::Wildcard, ex),
                ExKind::Language => {
                    if ex
                        .iter()
                        .any(|e| matches!(e, Exclusion::Stem(s) if s.is_empty()))
                    {
                        return Err("an empty language stem cannot be excluded".into());
                    }
                    ValueSetValue::LanguageStemRange(Stem::Wildcard, ex)
                }
            });
        }
        // `@~` (any language) or a language tag.
        if self.peek() == Some('@') {
            if self.peek_at(1) == Some('~') {
                self.pos += 2;
                let ex = self.typed_exclusions(ExKind::Language)?;
                if ex
                    .iter()
                    .any(|e| matches!(e, Exclusion::Stem(s) if s.is_empty()))
                {
                    return Err("an empty language stem cannot be excluded".into());
                }
                return Ok(if ex.is_empty() {
                    ValueSetValue::LanguageStem(String::new())
                } else {
                    ValueSetValue::LanguageStemRange(Stem::Value(String::new()), ex)
                });
            }
            let tag = self.langtag()?;
            if self.eat_tilde() {
                let ex = self.typed_exclusions(ExKind::Language)?;
                return Ok(if ex.is_empty() {
                    ValueSetValue::LanguageStem(tag)
                } else {
                    ValueSetValue::LanguageStemRange(Stem::Value(tag), ex)
                });
            }
            self.no_exclusion_here()?;
            return Ok(ValueSetValue::Language(tag));
        }
        if self.at_literal_start() {
            let lit = self.literal()?;
            if self.eat_tilde() {
                let ex = self.typed_exclusions(ExKind::Literal)?;
                return Ok(if ex.is_empty() {
                    ValueSetValue::LiteralStem(lit.value)
                } else {
                    ValueSetValue::LiteralStemRange(Stem::Value(lit.value), ex)
                });
            }
            self.no_exclusion_here()?;
            return Ok(ValueSetValue::Object(ObjectValue::Literal(lit)));
        }
        if self.at_iri_start() {
            let iri = self.iri()?;
            if self.eat_tilde() {
                let ex = self.typed_exclusions(ExKind::Iri)?;
                return Ok(if ex.is_empty() {
                    ValueSetValue::IriStem(iri)
                } else {
                    ValueSetValue::IriStemRange(Stem::Value(iri), ex)
                });
            }
            self.no_exclusion_here()?;
            return Ok(ValueSetValue::Object(ObjectValue::Iri(iri)));
        }
        Err(format!(
            "expected a value-set value, found {}",
            self.found()
        ))
    }

    fn eat_tilde(&mut self) -> bool {
        // '~' binds to the preceding term without whitespace in practice,
        // but the grammar allows it.
        self.eat('~')
    }

    fn no_exclusion_here(&mut self) -> R<()> {
        if self.at('-') && !self.peek_at(1).is_some_and(|c| c.is_ascii_digit()) {
            return Err("an exclusion must follow a stem ('~') or '.'".into());
        }
        Ok(())
    }

    fn typed_exclusions(&mut self, kind: ExKind) -> R<Vec<Exclusion>> {
        let ex = self.exclusions()?;
        if ex.iter().any(|e| e.0 != kind) {
            return Err("an exclusion is not of the stem's kind (IRI, literal or language)".into());
        }
        Ok(ex.into_iter().map(|e| e.1).collect())
    }

    fn exclusions(&mut self) -> R<Vec<(ExKind, Exclusion)>> {
        let mut out = Vec::new();
        loop {
            self.ws();
            if !(self.peek() == Some('-') && !self.peek_at(1).is_some_and(|c| c.is_ascii_digit())) {
                return Ok(out);
            }
            self.bump();
            self.ws();
            let (kind, value) = if self.peek() == Some('@') {
                if self.peek_at(1) == Some('~') {
                    self.pos += 1;
                    (ExKind::Language, String::new())
                } else {
                    (ExKind::Language, self.langtag()?)
                }
            } else if self.at_literal_start() {
                let lit = self.literal()?;
                (ExKind::Literal, lit.value)
            } else if self.at_iri_start() {
                (ExKind::Iri, self.iri()?)
            } else {
                return Err(format!("expected an exclusion, found {}", self.found()));
            };
            let ex = if self.eat_tilde() {
                Exclusion::Stem(value)
            } else {
                if kind == ExKind::Language && value.is_empty() {
                    return Err("'@' needs a language tag or '~'".into());
                }
                Exclusion::Value(value)
            };
            out.push((kind, ex));
        }
    }

    // ── triple expressions ───────────────────────────────────────────────

    fn triple_expression(&mut self) -> R<TripleExpr> {
        let first = self.group_triple_expr()?;
        if !self.at('|') {
            return Ok(first);
        }
        let mut items = vec![first];
        while self.eat('|') {
            items.push(self.group_triple_expr()?);
        }
        Ok(TripleExpr::OneOf(Group {
            id: None,
            expressions: items,
            min: 1,
            max: Max::Bounded(1),
            sem_acts: vec![],
            annotations: vec![],
        }))
    }

    fn group_triple_expr(&mut self) -> R<TripleExpr> {
        let first = self.unary_triple_expr()?;
        let mut items = vec![first];
        while self.eat(';') {
            if self.at('|') || self.at(')') || self.at('}') {
                break;
            }
            items.push(self.unary_triple_expr()?);
        }
        if items.len() == 1 {
            return Ok(items.pop().unwrap());
        }
        Ok(TripleExpr::EachOf(Group {
            id: None,
            expressions: items,
            min: 1,
            max: Max::Bounded(1),
            sem_acts: vec![],
            annotations: vec![],
        }))
    }

    fn unary_triple_expr(&mut self) -> R<TripleExpr> {
        self.ws();
        if self.eat('&') {
            self.ws();
            return Ok(TripleExpr::Ref(self.triple_expr_label()?));
        }
        let id = if self.eat('$') {
            self.ws();
            Some(self.triple_expr_label()?)
        } else {
            None
        };
        if self.eat('(') {
            let inner = self.triple_expression()?;
            self.expect(')')?;
            let (min, max) = self.cardinality()?.unwrap_or((1, Max::Bounded(1)));
            let annotations = self.annotations()?;
            let sem_acts = self.sem_acts()?;
            return bracketed(inner, id, min, max, annotations, sem_acts);
        }
        if self.at('&') {
            return Err("an inclusion cannot carry a label".into());
        }
        let mut tc = self.triple_constraint()?;
        tc.id = id;
        Ok(TripleExpr::TripleConstraint(tc))
    }

    fn triple_expr_label(&mut self) -> R<Label> {
        self.ws();
        match self.peek() {
            Some('_') if self.peek_at(1) == Some(':') => Ok(Label::BNode(self.bnode_label()?)),
            Some('<') => Ok(Label::Iri(self.iriref()?)),
            _ => Ok(Label::Iri(self.pname()?)),
        }
    }

    fn triple_constraint(&mut self) -> R<TripleConstraint> {
        self.ws();
        let inverse = self.eat('^');
        if self.at('^') {
            return Err("'^' given twice".into());
        }
        if self.at('!') {
            return Err("'!' (negated triple constraints) is not ShEx 2.1".into());
        }
        let predicate = self
            .try_predicate()?
            .ok_or_else(|| format!("expected a predicate, found {}", self.found()))?;
        let value = self.shape_or(true)?;
        let (min, max) = self.cardinality()?.unwrap_or((1, Max::Bounded(1)));
        let annotations = self.annotations()?;
        let sem_acts = self.sem_acts()?;
        Ok(TripleConstraint {
            id: None,
            inverse,
            predicate,
            value_expr: (!value.is_empty_shape()).then(|| Box::new(value)),
            min,
            max,
            sem_acts,
            annotations,
        })
    }

    /// `iri | 'a'`, or `None` when no predicate is next.
    fn try_predicate(&mut self) -> R<Option<String>> {
        self.ws();
        if self.peek() == Some('a')
            && !self
                .peek_at(1)
                .is_some_and(|c| is_pn_chars(c) || c == ':' || c == '.')
        {
            self.bump();
            return Ok(Some(RDF_TYPE.to_string()));
        }
        if self.at_iri_start() {
            return Ok(Some(self.iri()?));
        }
        Ok(None)
    }

    fn cardinality(&mut self) -> R<Option<(u32, Max)>> {
        self.ws();
        match self.peek() {
            Some('*') => {
                self.bump();
                Ok(Some((0, Max::Unbounded)))
            }
            Some('+') => {
                self.bump();
                Ok(Some((1, Max::Unbounded)))
            }
            Some('?') => {
                self.bump();
                Ok(Some((0, Max::Bounded(1))))
            }
            Some('{') => {
                self.bump();
                self.ws();
                let min = self.small_int()?;
                let max = if self.eat(',') {
                    self.ws();
                    if self.eat('*') || self.at('}') {
                        Max::Unbounded
                    } else {
                        Max::Bounded(self.small_int()?)
                    }
                } else {
                    Max::Bounded(min)
                };
                self.expect('}')?;
                if let Max::Bounded(m) = max {
                    if m < min {
                        return Err(format!("cardinality {{{min},{m}}} has max < min"));
                    }
                }
                if self.at('*') || self.at('+') || self.at('?') || self.at('{') {
                    return Err("two cardinalities".into());
                }
                Ok(Some((min, max)))
            }
            _ => Ok(None),
        }
        .and_then(|c| {
            if c.is_some() && matches!(self.peek_ws(), Some('*' | '+' | '?')) {
                return Err("two cardinalities".into());
            }
            Ok(c)
        })
    }

    fn peek_ws(&mut self) -> Option<char> {
        self.ws();
        self.peek()
    }

    fn small_int(&mut self) -> R<u32> {
        let start = self.pos;
        while self.peek().is_some_and(|c| c.is_ascii_digit()) {
            self.bump();
        }
        self.s[start..self.pos]
            .parse()
            .map_err(|_| format!("expected an integer, found {}", self.found()))
    }

    fn annotations(&mut self) -> R<Vec<Annotation>> {
        let mut out = Vec::new();
        loop {
            self.ws();
            if !(self.peek() == Some('/') && self.peek_at(1) == Some('/')) {
                return Ok(out);
            }
            self.pos += 2;
            let predicate = self.try_predicate()?.ok_or_else(|| {
                format!("expected an annotation predicate, found {}", self.found())
            })?;
            self.ws();
            let object = if self.at_literal_start() {
                ObjectValue::Literal(self.literal()?)
            } else if self.at_iri_start() {
                ObjectValue::Iri(self.iri()?)
            } else {
                return Err(format!(
                    "expected an annotation object, found {}",
                    self.found()
                ));
            };
            out.push(Annotation { predicate, object });
        }
    }

    fn sem_acts(&mut self) -> R<Vec<SemAct>> {
        let mut out = Vec::new();
        while self.at('%') {
            out.push(self.code_decl()?);
        }
        Ok(out)
    }

    /// `'%' iri (CODE | '%')`.
    fn code_decl(&mut self) -> R<SemAct> {
        self.expect('%')?;
        self.ws();
        let name = self.iri()?;
        self.ws();
        if self.eat('%') {
            return Ok(SemAct { name, code: None });
        }
        if self.peek() != Some('{') {
            return Err(format!(
                "expected '{{' or '%' after a semantic action name, found {}",
                self.found()
            ));
        }
        self.bump();
        let mut code = String::new();
        loop {
            match self.bump() {
                None => return Err("unterminated semantic action code".into()),
                Some('%') => {
                    if self.peek() == Some('}') {
                        self.bump();
                        break;
                    }
                    return Err("'%' in code must be escaped as '\\%'".into());
                }
                Some('\\') => match self.peek() {
                    Some('%') => {
                        self.bump();
                        code.push('%');
                    }
                    Some('\\') => {
                        self.bump();
                        code.push('\\');
                    }
                    Some('u' | 'U') => code.push(self.uchar()?),
                    _ => return Err("invalid escape in semantic action code".into()),
                },
                Some(c) => code.push(c),
            }
        }
        Ok(SemAct {
            name,
            code: Some(code),
        })
    }

    // ── terms ────────────────────────────────────────────────────────────

    fn at_literal_start(&mut self) -> bool {
        self.ws();
        match self.peek() {
            Some('"' | '\'') => true,
            Some('+' | '-') => self
                .peek_at(1)
                .is_some_and(|c| c.is_ascii_digit() || c == '.'),
            Some('.') => self.peek_at(1).is_some_and(|c| c.is_ascii_digit()),
            Some(c) if c.is_ascii_digit() => true,
            _ => {
                let r = self.rest();
                (r.starts_with("true") || r.starts_with("false")) && {
                    let n = if r.starts_with("true") { 4 } else { 5 };
                    !r[n..]
                        .chars()
                        .next()
                        .is_some_and(|c| is_pn_chars(c) || c == ':')
                }
            }
        }
    }

    fn literal(&mut self) -> R<ObjectLiteral> {
        self.ws();
        match self.peek() {
            Some('"' | '\'') => {
                let value = self.string()?;
                if self.peek() == Some('@') {
                    let tag = self.langtag()?;
                    if self.rest().starts_with("^^") {
                        return Err(
                            "a literal cannot have both a language tag and a datatype".into()
                        );
                    }
                    return Ok(ObjectLiteral {
                        value,
                        language: Some(tag),
                        datatype: None,
                    });
                }
                if self.rest().starts_with("^^") {
                    self.pos += 2;
                    let dt = self.iri()?;
                    let datatype = (dt != XSD_STRING).then_some(dt);
                    return Ok(ObjectLiteral {
                        value,
                        language: None,
                        datatype,
                    });
                }
                Ok(ObjectLiteral {
                    value,
                    language: None,
                    datatype: None,
                })
            }
            _ if self.eat_word("true") => Ok(bool_lit("true")),
            _ if self.eat_word("false") => Ok(bool_lit("false")),
            _ => {
                let tok = self.number_token()?;
                let dt = if tok.contains(['e', 'E']) {
                    "double"
                } else if tok.contains('.') {
                    "decimal"
                } else {
                    "integer"
                };
                Ok(ObjectLiteral {
                    value: tok,
                    language: None,
                    datatype: Some(format!("{XSD}{dt}")),
                })
            }
        }
    }

    /// A case-sensitive word (`true`, `false`) not followed by a name
    /// character.
    fn eat_word(&mut self, w: &str) -> bool {
        self.ws();
        let r = self.rest();
        if r.starts_with(w)
            && !r[w.len()..]
                .chars()
                .next()
                .is_some_and(|c| is_pn_chars(c) || c == ':')
        {
            self.pos += w.len();
            true
        } else {
            false
        }
    }

    fn string(&mut self) -> R<String> {
        let q = self.bump().unwrap();
        let long = self.peek() == Some(q) && self.peek_at(1) == Some(q);
        if long {
            self.pos += 2;
        }
        let mut out = String::new();
        loop {
            let c = self
                .bump()
                .ok_or_else(|| "unterminated string literal".to_string())?;
            if c == q {
                if !long {
                    break;
                }
                if self.peek() == Some(q) && self.peek_at(1) == Some(q) {
                    // The first run of three quotes closes the string; a
                    // quote inside must be followed by another character.
                    self.pos += 2;
                    break;
                }
                out.push(c);
                continue;
            }
            match c {
                '\\' => match self.bump() {
                    Some('t') => out.push('\t'),
                    Some('b') => out.push('\u{8}'),
                    Some('n') => out.push('\n'),
                    Some('r') => out.push('\r'),
                    Some('f') => out.push('\u{c}'),
                    Some('"') => out.push('"'),
                    Some('\'') => out.push('\''),
                    Some('\\') => out.push('\\'),
                    Some('u' | 'U') => {
                        self.pos -= 1;
                        out.push(self.uchar()?);
                    }
                    _ => return Err("invalid escape in string literal".into()),
                },
                '\n' | '\r' if !long => return Err("newline in a short string literal".into()),
                c => out.push(c),
            }
        }
        Ok(out)
    }

    /// `\uXXXX` / `\UXXXXXXXX` at `self.pos` (pointing at the `u`/`U`).
    fn uchar(&mut self) -> R<char> {
        let n = match self.bump() {
            Some('u') => 4,
            Some('U') => 8,
            _ => return Err("bad numeric escape".into()),
        };
        let hex: String = self.rest().chars().take(n).collect();
        if hex.len() != n || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err("bad numeric escape".into());
        }
        self.pos += n;
        let v = u32::from_str_radix(&hex, 16).map_err(|_| "bad numeric escape".to_string())?;
        char::from_u32(v).ok_or_else(|| format!("\\u{hex} is not a Unicode scalar value"))
    }

    fn langtag(&mut self) -> R<String> {
        if self.bump() != Some('@') {
            return Err("expected '@'".into());
        }
        let start = self.pos;
        while self.peek().is_some_and(|c| c.is_ascii_alphabetic()) {
            self.bump();
        }
        if self.pos == start {
            return Err("empty language tag".into());
        }
        while self.peek() == Some('-') && self.peek_at(1).is_some_and(|c| c.is_ascii_alphanumeric())
        {
            self.bump();
            while self.peek().is_some_and(|c| c.is_ascii_alphanumeric()) {
                self.bump();
            }
        }
        if self.peek().is_some_and(|c| is_pn_chars(c) || c == ':') && self.peek() != Some('-') {
            return Err(format!("malformed language tag at {}", self.found()));
        }
        // Language tags compare case-insensitively; RDF normalises them to
        // lower case.
        Ok(self.s[start..self.pos].to_ascii_lowercase())
    }

    fn bnode_label(&mut self) -> R<String> {
        self.pos += 2; // "_:"
        let start = self.pos;
        match self.peek() {
            Some(c) if is_pn_chars_u(c) || c.is_ascii_digit() => {
                self.bump();
            }
            _ => return Err("malformed blank node label".into()),
        }
        let mut last_good = self.pos;
        while let Some(c) = self.peek() {
            if is_pn_chars(c) {
                self.bump();
                last_good = self.pos;
            } else if c == '.' {
                self.bump();
            } else {
                break;
            }
        }
        self.pos = last_good;
        Ok(self.s[start..self.pos].to_string())
    }

    /// `IRIREF | PNAME_LN | PNAME_NS`, as an absolute IRI.
    fn iri(&mut self) -> R<String> {
        self.ws();
        if self.peek() == Some('<') {
            self.iriref()
        } else {
            self.pname()
        }
    }

    /// `<…>` with numeric escapes unescaped, resolved against the base.
    fn iriref(&mut self) -> R<String> {
        if self.bump() != Some('<') {
            return Err(format!("expected an IRI, found {}", self.found()));
        }
        let mut raw = String::new();
        loop {
            match self.bump() {
                None => return Err("unterminated IRI".into()),
                Some('>') => break,
                Some('\\') => {
                    let c = self.uchar()?;
                    if c <= ' ' || "<>\"{}|^`\\".contains(c) {
                        return Err(format!(
                            "character U+{:04X} is not allowed in an IRI",
                            c as u32
                        ));
                    }
                    raw.push(c);
                }
                Some(c) if c <= ' ' || "<\"{}|^`".contains(c) => {
                    return Err(format!("character {c:?} is not allowed in an IRI"));
                }
                Some(c) => raw.push(c),
            }
        }
        self.resolve(&raw)
    }

    fn resolve(&self, raw: &str) -> R<String> {
        resolve_iri(self.base.as_deref(), raw)
    }

    /// `PNAME_NS` in a PREFIX declaration: the (possibly empty) prefix.
    fn pname_ns_decl(&mut self) -> R<String> {
        let prefix = self.pn_prefix()?;
        if self.peek() != Some(':') {
            return Err("expected 'prefix:' in a PREFIX declaration".into());
        }
        self.bump();
        Ok(prefix)
    }

    fn pn_prefix(&mut self) -> R<String> {
        let start = self.pos;
        match self.peek() {
            Some(c) if is_pn_chars_base(c) => {
                self.bump();
            }
            Some(':') => return Ok(String::new()),
            _ => return Err(format!("expected a prefixed name, found {}", self.found())),
        }
        let mut last_good = self.pos;
        while let Some(c) = self.peek() {
            if is_pn_chars(c) {
                self.bump();
                last_good = self.pos;
            } else if c == '.' {
                self.bump();
            } else {
                break;
            }
        }
        if self.pos != last_good {
            return Err("a prefix cannot end with '.'".into());
        }
        Ok(self.s[start..self.pos].to_string())
    }

    /// `PNAME_LN | PNAME_NS` expanded through the declared prefixes.
    fn pname(&mut self) -> R<String> {
        self.ws();
        let prefix = self.pn_prefix()?;
        if self.peek() != Some(':') {
            return Err(format!("expected a prefixed name, found {}", self.found()));
        }
        self.bump();
        let ns = self
            .prefixes
            .get(&prefix)
            .cloned()
            .ok_or_else(|| format!("undeclared prefix '{prefix}:'"))?;
        let local = self.pn_local()?;
        Ok(format!("{ns}{local}"))
    }

    fn pn_local(&mut self) -> R<String> {
        let mut out = String::new();
        let mut committed = self.pos;
        let mut committed_len = 0;
        let mut first = true;
        while let Some(c) = self.peek() {
            let ok_first = is_pn_chars_u(c) || c == ':' || c.is_ascii_digit();
            let ok_rest = is_pn_chars(c) || c == ':' || c == '.';
            if c == '%' {
                let h: String = self.rest().chars().skip(1).take(2).collect();
                if h.len() != 2 || !h.chars().all(|x| x.is_ascii_hexdigit()) {
                    return Err("malformed percent escape in a local name".into());
                }
                self.pos += 3;
                out.push('%');
                out.push_str(&h);
            } else if c == '\\' {
                match self.peek_at(1) {
                    Some(e) if is_local_esc(e) => {
                        self.pos += 1 + e.len_utf8();
                        out.push(e);
                    }
                    _ => return Err("invalid escape in a local name".into()),
                }
            } else if (first && ok_first) || (!first && ok_rest) {
                self.bump();
                out.push(c);
                if c == '.' {
                    first = false;
                    continue;
                }
            } else {
                break;
            }
            first = false;
            committed = self.pos;
            committed_len = out.len();
        }
        // A local name does not end with '.'.
        self.pos = committed;
        out.truncate(committed_len);
        Ok(out)
    }
}

/// A facet given twice (ShExJ has one key per facet), or a numeric facet
/// on a datatype that has no numeric value, is not a node constraint.
fn check_facets(nc: &NodeConstraint) -> R<()> {
    let s: Vec<u8> = nc
        .string_facets
        .iter()
        .map(|f| match f {
            StringFacet::Length(_) => 0,
            StringFacet::MinLength(_) => 1,
            StringFacet::MaxLength(_) => 2,
            StringFacet::Pattern(..) => 3,
        })
        .collect();
    let n: Vec<u8> = nc
        .numeric_facets
        .iter()
        .map(|f| match f {
            NumericFacet::MinInclusive(_) => 0,
            NumericFacet::MinExclusive(_) => 1,
            NumericFacet::MaxInclusive(_) => 2,
            NumericFacet::MaxExclusive(_) => 3,
            NumericFacet::TotalDigits(_) => 4,
            NumericFacet::FractionDigits(_) => 5,
        })
        .collect();
    for v in [&s, &n] {
        let mut sorted = v.clone();
        sorted.sort_unstable();
        sorted.dedup();
        if sorted.len() != v.len() {
            return Err("a facet is given twice".into());
        }
    }
    if let Some(dt) = &nc.datatype {
        if !nc.numeric_facets.is_empty() && !super::xsd::is_numeric_datatype(dt) {
            return Err(format!("numeric facets do not apply to <{dt}>"));
        }
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ExKind {
    Iri,
    Literal,
    Language,
}

fn bool_lit(v: &str) -> ObjectLiteral {
    ObjectLiteral {
        value: v.to_string(),
        language: None,
        datatype: Some(format!("{XSD}boolean")),
    }
}

/// Apply a bracketed group's cardinality, label, annotations and semantic
/// actions to its content.
fn bracketed(
    inner: TripleExpr,
    id: Option<Label>,
    min: u32,
    max: Max,
    annotations: Vec<Annotation>,
    sem_acts: Vec<SemAct>,
) -> R<TripleExpr> {
    let default_card = min == 1 && max == Max::Bounded(1);
    if matches!(inner, TripleExpr::Ref(_))
        && !(default_card && id.is_none() && annotations.is_empty() && sem_acts.is_empty())
    {
        return Err(
            "an inclusion (&label) cannot take a cardinality, label, annotation or semantic action"
                .into(),
        );
    }
    let bare_group = |g: &Group| {
        g.id.is_none()
            && g.min == 1
            && g.max == Max::Bounded(1)
            && g.annotations.is_empty()
            && g.sem_acts.is_empty()
    };
    let fill = |mut g: Group, id, annotations, sem_acts| {
        g.id = id;
        g.min = min;
        g.max = max;
        g.annotations = annotations;
        g.sem_acts = sem_acts;
        g
    };
    match inner {
        TripleExpr::EachOf(g) if bare_group(&g) => {
            Ok(TripleExpr::EachOf(fill(g, id, annotations, sem_acts)))
        }
        TripleExpr::OneOf(g) if bare_group(&g) => {
            Ok(TripleExpr::OneOf(fill(g, id, annotations, sem_acts)))
        }
        TripleExpr::TripleConstraint(mut tc)
            if (default_card || (tc.min == 1 && tc.max == Max::Bounded(1)))
                && (id.is_none() || tc.id.is_none()) =>
        {
            if !default_card {
                tc.min = min;
                tc.max = max;
            }
            if id.is_some() {
                tc.id = id;
            }
            tc.annotations.extend(annotations);
            tc.sem_acts.extend(sem_acts);
            Ok(TripleExpr::TripleConstraint(tc))
        }
        other if default_card && id.is_none() && annotations.is_empty() && sem_acts.is_empty() => {
            Ok(other)
        }
        other => Ok(TripleExpr::EachOf(Group {
            id,
            expressions: vec![other],
            min,
            max,
            sem_acts,
            annotations,
        })),
    }
}

/// Resolve `raw` against `base` per RFC 3986 (Turtle §6.3). An absolute
/// `raw` is returned as is; a relative one without a base is an error.
pub fn resolve_iri(base: Option<&str>, raw: &str) -> Result<String, String> {
    if let Ok(iri) = oxiri::Iri::parse(raw) {
        return Ok(iri.into_inner().to_string());
    }
    let Some(base) = base else {
        return Err(format!("relative IRI <{raw}> with no base IRI"));
    };
    let b = oxiri::Iri::parse(base).map_err(|e| format!("bad base IRI <{base}>: {e}"))?;
    b.resolve(raw)
        .map(|i| i.into_inner())
        .map_err(|e| format!("cannot resolve <{raw}> against <{base}>: {e}"))
}

/// Shape-expression and triple-expression labels share one namespace; a
/// label declared twice, in either role, is an error.
pub fn check_label_collisions(schema: &Schema) -> Result<(), String> {
    let mut seen: HashSet<Label> = HashSet::new();
    for d in &schema.shapes {
        if !seen.insert(d.label.clone()) {
            return Err(format!("shape label {} is declared twice", d.label));
        }
    }
    let mut te_labels = Vec::new();
    for d in &schema.shapes {
        collect_te_labels_se(&d.expr, &mut te_labels);
    }
    if let Some(s) = &schema.start {
        collect_te_labels_se(s, &mut te_labels);
    }
    for l in te_labels {
        if !seen.insert(l.clone()) {
            return Err(format!(
                "label {l} names more than one shape or triple expression"
            ));
        }
    }
    Ok(())
}

fn collect_te_labels_se(se: &ShapeExpr, out: &mut Vec<Label>) {
    match se {
        ShapeExpr::Or(v) | ShapeExpr::And(v) => v.iter().for_each(|e| collect_te_labels_se(e, out)),
        ShapeExpr::Not(e) => collect_te_labels_se(e, out),
        ShapeExpr::Shape(s) => {
            if let Some(te) = &s.expression {
                collect_te_labels(te, out);
            }
        }
        _ => {}
    }
}

fn collect_te_labels(te: &TripleExpr, out: &mut Vec<Label>) {
    if let Some(id) = te.id() {
        out.push(id.clone());
    }
    match te {
        TripleExpr::EachOf(g) | TripleExpr::OneOf(g) => {
            g.expressions.iter().for_each(|e| collect_te_labels(e, out))
        }
        TripleExpr::TripleConstraint(tc) => {
            if let Some(v) = &tc.value_expr {
                collect_te_labels_se(v, out);
            }
        }
        TripleExpr::Ref(_) => {}
    }
}

// ── ShapeMap language ────────────────────────────────────────────────────

/// A term in a ShapeMap.
#[derive(Debug, Clone, PartialEq)]
pub enum MapTerm {
    Iri(String),
    BNode(String),
    Literal(ObjectLiteral),
}

/// A ShapeMap node selector.
#[derive(Debug, Clone, PartialEq)]
pub enum MapSelector {
    /// One node.
    Node(MapTerm),
    /// `{FOCUS p o}` / `{FOCUS p _}`: subjects of matching triples.
    Subjects(String, Option<MapTerm>),
    /// `{s p FOCUS}` / `{_ p FOCUS}`: objects of matching triples.
    Objects(Option<MapTerm>, String),
}

/// A shape label or `START`.
#[derive(Debug, Clone, PartialEq)]
pub enum MapShape {
    Label(Label),
    Start,
}

/// Parse a query ShapeMap (<https://shexspec.github.io/shape-map/>):
/// `nodeSelector@shapeLabel` associations separated by commas, with the
/// schema's prefixes and base for prefixed names and relative IRIs.
pub fn parse_shape_map(
    text: &str,
    prefixes: &[(String, String)],
    base: Option<&str>,
) -> Result<Vec<(MapSelector, MapShape)>, String> {
    let mut p = Parser {
        s: text,
        pos: 0,
        base: base.map(str::to_string),
        prefixes: prefixes.iter().cloned().collect(),
        prefix_order: Vec::new(),
    };
    p.shape_map().map_err(|e| {
        p.located(&e)
            .replacen("ShExC syntax error", "ShapeMap syntax error", 1)
    })
}

impl Parser<'_> {
    fn shape_map(&mut self) -> R<Vec<(MapSelector, MapShape)>> {
        let mut out = Vec::new();
        self.ws();
        if self.peek().is_none() {
            return Ok(out);
        }
        loop {
            let sel = self.node_selector()?;
            self.expect('@')?;
            if self.at('!') {
                return Err("'@!' belongs to result shape maps, not to a query".into());
            }
            let shape = if self.eat_keyword("START") {
                MapShape::Start
            } else {
                self.ws();
                MapShape::Label(match self.peek() {
                    Some('_') if self.peek_at(1) == Some(':') => Label::BNode(self.bnode_label()?),
                    Some('<') => Label::Iri(self.iriref()?),
                    _ => Label::Iri(self.pname()?),
                })
            };
            out.push((sel, shape));
            if !self.eat(',') {
                break;
            }
        }
        self.ws();
        if self.peek().is_some() {
            return Err(format!(
                "expected ',' or the end of the shape map, found {}",
                self.found()
            ));
        }
        Ok(out)
    }

    fn map_term(&mut self, allow_literal: bool) -> R<MapTerm> {
        self.ws();
        match self.peek() {
            Some('_') if self.peek_at(1) == Some(':') => Ok(MapTerm::BNode(self.bnode_label()?)),
            _ if allow_literal && self.at_literal_start() => Ok(MapTerm::Literal(self.literal()?)),
            _ if self.at_iri_start() => Ok(MapTerm::Iri(self.iri()?)),
            _ => Err(format!("expected a node, found {}", self.found())),
        }
    }

    fn node_selector(&mut self) -> R<MapSelector> {
        self.ws();
        if self.at_keyword("SPARQL") {
            return Err(
                "SPARQL node selectors are not supported; use a node or a {FOCUS …} triple pattern"
                    .into(),
            );
        }
        if !self.eat('{') {
            return Ok(MapSelector::Node(self.map_term(true)?));
        }
        let sel = if self.eat_keyword("FOCUS") {
            let p = self
                .try_predicate()?
                .ok_or_else(|| format!("expected a predicate, found {}", self.found()))?;
            self.ws();
            let o = if self.peek() == Some('_') && self.peek_at(1) != Some(':') {
                self.bump();
                None
            } else {
                Some(self.map_term(true)?)
            };
            MapSelector::Subjects(p, o)
        } else {
            self.ws();
            let s = if self.peek() == Some('_') && self.peek_at(1) != Some(':') {
                self.bump();
                None
            } else {
                Some(self.map_term(false)?)
            };
            let p = self
                .try_predicate()?
                .ok_or_else(|| format!("expected a predicate, found {}", self.found()))?;
            if !self.eat_keyword("FOCUS") {
                return Err("a triple pattern needs FOCUS as its subject or object".into());
            }
            MapSelector::Objects(s, p)
        };
        self.expect('}')?;
        Ok(sel)
    }
}
