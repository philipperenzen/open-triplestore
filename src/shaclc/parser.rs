//! Parser for the W3C SHACL Compact Syntax.
//!
//! Implements the whole grammar of the SHACL Community Group report
//! (<https://w3c.github.io/shacl/shacl-compact-syntax/>, `SHACLC.g4`) and its
//! production rules, building an `oxrdf` graph directly: a hand-written lexer
//! with the grammar's longest-match token rules, a recursive-descent parser
//! into a small syntax tree, and an emitter that applies the report's
//! "Grammar and Production Rules" section rule by rule.
//!
//! Deviations, all of them supersets that accept documents a strict parser of
//! `SHACLC.g4` would refuse, never changing what an accepted document means:
//! - `IRIREF` may contain `=` (the grammar's lexer excludes it, Turtle and
//!   SPARQL do not); the serializer escapes it as `\u003D` so its output
//!   stays readable by strict parsers.
//! - Prefixed names and language tags accept the supplementary Unicode planes
//!   Turtle allows (ANTLR's UTF-16 ranges stop at U+FFFD).

use super::vocab::*;
use oxiri::Iri;
use oxrdf::{BlankNode, Graph, Literal, NamedNode, NamedOrBlankNode, Term, TripleRef};
use std::collections::HashMap;
use std::fmt;

/// Nesting bound for `{ … }` bodies and parenthesised paths: input is
/// caller-supplied, and the parser and emitter recurse once per level.
const MAX_DEPTH: usize = 64;

/// A parse error with the position (1-based line and column) it was found at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub line: usize,
    pub column: usize,
    pub message: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "SHACL-C parse error at line {}, column {}: {}",
            self.line, self.column, self.message
        )
    }
}

impl std::error::Error for ParseError {}

/// A parsed document: its graph, the final `?baseURI` and the prefixes it
/// declared (in declaration order; a later declaration of a label wins).
#[derive(Debug, Clone)]
pub struct Document {
    pub graph: Graph,
    pub base: Option<String>,
    pub prefixes: Vec<(String, String)>,
}

/// Parse a SHACL-C document. `base` is the initial `?baseURI` (the report's
/// "optional base URI"); a `BASE` directive replaces it. Without either, the
/// document produces no `owl:Ontology` triple and may not use `IMPORTS` or
/// relative IRIs.
pub fn parse_document(input: &str, base: Option<&str>) -> Result<Document, ParseError> {
    let tokens = Lexer::new(input).tokenize()?;
    let base = match base {
        Some(b) => Some(Iri::parse(b.to_string()).map_err(|e| ParseError {
            line: 1,
            column: 1,
            message: format!("the base IRI <{b}> is not an absolute IRI: {e}"),
        })?),
        None => None,
    };
    let mut p = Parser {
        tokens,
        pos: 0,
        base,
        prefixes: INITIAL_PREFIXES
            .iter()
            .map(|(p, n)| (p.to_string(), n.to_string()))
            .collect(),
        declared: Vec::new(),
        imports: Vec::new(),
        depth: 0,
    };
    let shapes = p.document()?;
    let mut e = Emitter {
        graph: Graph::new(),
    };
    if let Some(base) = &p.base {
        let base = NamedNode::new_unchecked(base.as_str());
        e.add(
            &base.clone().into(),
            RDF_TYPE,
            NamedNode::new_unchecked(OWL_ONTOLOGY).into(),
        );
        for import in &p.imports {
            e.add(&base.clone().into(), OWL_IMPORTS, import.clone().into());
        }
    }
    for shape in &shapes {
        e.shape(shape);
    }
    Ok(Document {
        graph: e.graph,
        base: p.base.map(|b| b.into_inner()),
        prefixes: p.declared,
    })
}

/// Serialise a parsed document as Turtle, declaring the parser's initial
/// prefixes, `owl:` and every prefix the document declared.
pub fn document_to_turtle(doc: &Document) -> Result<String, String> {
    use oxigraph::io::{RdfFormat, RdfSerializer};
    let mut labels: Vec<(String, String)> = INITIAL_PREFIXES
        .iter()
        .map(|(p, n)| (p.to_string(), n.to_string()))
        .collect();
    labels.push(("owl".into(), OWL.into()));
    for (p, n) in &doc.prefixes {
        labels.retain(|(q, _)| q != p);
        labels.push((p.clone(), n.clone()));
    }
    let mut ser = RdfSerializer::from_format(RdfFormat::Turtle);
    for (p, n) in &labels {
        // A label Turtle cannot carry is only a shorter spelling lost.
        if let Ok(s) = ser.clone().with_prefix(p.as_str(), n.as_str()) {
            ser = s;
        }
    }
    let mut w = ser.for_writer(Vec::new());
    for t in doc.graph.iter() {
        w.serialize_triple(t).map_err(|e| e.to_string())?;
    }
    let bytes = w.finish().map_err(|e| e.to_string())?;
    String::from_utf8(bytes).map_err(|e| e.to_string())
}

// ── Lexer ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    /// `<…>`, escapes decoded, not yet resolved against the base.
    IriRef(String),
    /// `prefix:local`, local part with `PN_LOCAL_ESC` decoded.
    PName(String, String),
    /// `@prefix:local` (a shape reference).
    AtPName(String, String),
    /// A lone `@`, before an `IRIREF` shape reference.
    At,
    LangTag(String),
    Integer(String),
    Decimal(String),
    Double(String),
    Str(String),
    /// A bare keyword: directive, `shape`, parameter, node kind, boolean.
    Word(String),
    Punct(&'static str),
    Eof,
}

#[derive(Debug, Clone)]
struct Token {
    tok: Tok,
    line: usize,
    col: usize,
}

struct Lexer {
    chars: Vec<char>,
    i: usize,
    line: usize,
    col: usize,
}

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

const PN_LOCAL_ESCAPABLE: &str = "_~.-!$&'()*+,;=/?#@%";

impl Lexer {
    fn new(input: &str) -> Self {
        Lexer {
            chars: input.chars().collect(),
            i: 0,
            line: 1,
            col: 1,
        }
    }

    fn peek(&self, k: usize) -> Option<char> {
        self.chars.get(self.i + k).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.chars.get(self.i).copied()?;
        self.i += 1;
        if c == '\n' {
            self.line += 1;
            self.col = 1;
        } else {
            self.col += 1;
        }
        Some(c)
    }

    fn err<T>(&self, line: usize, col: usize, message: impl Into<String>) -> Result<T, ParseError> {
        Err(ParseError {
            line,
            column: col,
            message: message.into(),
        })
    }

    fn tokenize(mut self) -> Result<Vec<Token>, ParseError> {
        let mut out = Vec::new();
        loop {
            // PASS and COMMENT are skipped.
            loop {
                match self.peek(0) {
                    Some(' ' | '\t' | '\r' | '\n') => {
                        self.bump();
                    }
                    Some('#') => {
                        while !matches!(self.peek(0), None | Some('\n' | '\r')) {
                            self.bump();
                        }
                    }
                    _ => break,
                }
            }
            let (line, col) = (self.line, self.col);
            let Some(c) = self.peek(0) else {
                out.push(Token {
                    tok: Tok::Eof,
                    line,
                    col,
                });
                return Ok(out);
            };
            let tok = match c {
                '<' => self.iriref()?,
                '"' | '\'' => self.string()?,
                '@' => self.at()?,
                '0'..='9' => self.number()?,
                '+' | '-' => {
                    let n = self.peek(1);
                    if n.is_some_and(|d| d.is_ascii_digit())
                        || (n == Some('.') && self.peek(2).is_some_and(|d| d.is_ascii_digit()))
                    {
                        self.number()?
                    } else if c == '-' && n == Some('>') {
                        self.bump();
                        self.bump();
                        Tok::Punct("->")
                    } else if c == '+' {
                        self.bump();
                        Tok::Punct("+")
                    } else {
                        return self.err(line, col, "unexpected `-`");
                    }
                }
                '.' => {
                    if self.peek(1) == Some('.') {
                        self.bump();
                        self.bump();
                        Tok::Punct("..")
                    } else if self.peek(1).is_some_and(|d| d.is_ascii_digit()) {
                        self.number()?
                    } else {
                        self.bump();
                        Tok::Punct(".")
                    }
                }
                '^' => {
                    self.bump();
                    if self.peek(0) == Some('^') {
                        self.bump();
                        Tok::Punct("^^")
                    } else {
                        Tok::Punct("^")
                    }
                }
                '{' | '}' | '(' | ')' | '[' | ']' | '|' | '/' | '?' | '*' | '!' | '=' => {
                    self.bump();
                    Tok::Punct(match c {
                        '{' => "{",
                        '}' => "}",
                        '(' => "(",
                        ')' => ")",
                        '[' => "[",
                        ']' => "]",
                        '|' => "|",
                        '/' => "/",
                        '?' => "?",
                        '*' => "*",
                        '!' => "!",
                        _ => "=",
                    })
                }
                ':' => {
                    self.bump();
                    let local = self.pn_local()?;
                    Tok::PName(String::new(), local)
                }
                c if is_pn_chars_base(c) => self.word_or_pname()?,
                ';' => {
                    return self.err(
                        line,
                        col,
                        format!("unexpected `;`: every constraint ends with `.` in the W3C syntax{LEGACY_HINT}"),
                    );
                }
                other => {
                    return self.err(line, col, format!("unexpected character `{other}`"));
                }
            };
            out.push(Token { tok, line, col });
        }
    }

    fn uchar(&mut self, len: usize) -> Result<char, ParseError> {
        let (line, col) = (self.line, self.col);
        let mut hex = String::new();
        for _ in 0..len {
            match self.bump() {
                Some(h) if h.is_ascii_hexdigit() => hex.push(h),
                _ => return self.err(line, col, "a \\u escape needs hexadecimal digits"),
            }
        }
        u32::from_str_radix(&hex, 16)
            .ok()
            .and_then(char::from_u32)
            .map_or_else(
                || self.err(line, col, format!("\\u{hex} is not a Unicode scalar value")),
                Ok,
            )
    }

    fn iriref(&mut self) -> Result<Tok, ParseError> {
        let (line, col) = (self.line, self.col);
        self.bump(); // '<'
        let mut iri = String::new();
        loop {
            match self.bump() {
                Some('>') => return Ok(Tok::IriRef(iri)),
                Some('\\') => match self.bump() {
                    Some('u') => iri.push(self.uchar(4)?),
                    Some('U') => iri.push(self.uchar(8)?),
                    _ => {
                        return self.err(
                            line,
                            col,
                            "only \\u and \\U escapes are allowed in an IRI",
                        )
                    }
                },
                Some(c) if c <= ' ' || "<\"{}|^`".contains(c) => {
                    return self.err(
                        line,
                        col,
                        format!("the IRI contains a character IRIREF does not allow ({c:?})"),
                    )
                }
                Some(c) => iri.push(c),
                None => return self.err(line, col, "unterminated IRI (missing `>`)"),
            }
        }
    }

    fn string(&mut self) -> Result<Tok, ParseError> {
        let (line, col) = (self.line, self.col);
        let q = self.bump().unwrap_or('"');
        let long = self.peek(0) == Some(q) && self.peek(1) == Some(q);
        if long {
            self.bump();
            self.bump();
        }
        let mut s = String::new();
        loop {
            match self.bump() {
                None => return self.err(line, col, "unterminated string"),
                Some(c) if c == q => {
                    if !long {
                        return Ok(Tok::Str(s));
                    }
                    if self.peek(0) == Some(q) && self.peek(1) == Some(q) {
                        self.bump();
                        self.bump();
                        // `""""` ends with the last three quotes.
                        while self.peek(0) == Some(q) {
                            s.push(q);
                            self.bump();
                        }
                        return Ok(Tok::Str(s));
                    }
                    s.push(c);
                }
                Some('\\') => match self.bump() {
                    Some('t') => s.push('\t'),
                    Some('b') => s.push('\u{8}'),
                    Some('n') => s.push('\n'),
                    Some('r') => s.push('\r'),
                    Some('f') => s.push('\u{c}'),
                    Some('\\') => s.push('\\'),
                    Some('"') => s.push('"'),
                    Some('\'') => s.push('\''),
                    Some('u') => s.push(self.uchar(4)?),
                    Some('U') => s.push(self.uchar(8)?),
                    other => {
                        return self.err(
                            self.line,
                            self.col,
                            format!("invalid string escape `\\{}`", other.unwrap_or(' ')),
                        )
                    }
                },
                Some(c @ ('\n' | '\r')) if !long => {
                    return self.err(
                        line,
                        col,
                        format!(
                            "a line break ({c:?}) inside a single-line string; use \\n or a \"\"\"long string\"\"\""
                        ),
                    )
                }
                Some(c) => s.push(c),
            }
        }
    }

    /// `PN_PREFIX` starting at the cursor, as a length, or `None`. A prefix
    /// may not end with `.`.
    fn pn_prefix_len(&self) -> Option<usize> {
        if !self.peek(0).is_some_and(is_pn_chars_base) {
            return None;
        }
        let mut end = 1;
        let mut k = 1;
        while let Some(c) = self.peek(k) {
            if is_pn_chars(c) {
                k += 1;
                end = k;
            } else if c == '.' {
                k += 1;
            } else {
                break;
            }
        }
        Some(end)
    }

    /// `PN_LOCAL` (possibly empty) at the cursor, decoded. Trailing dots are
    /// not part of it: `ex:a.` is `ex:a` followed by the terminator.
    fn pn_local(&mut self) -> Result<String, ParseError> {
        let mut value = String::new();
        let mut committed_len = 0usize; // value length at the last non-'.' char
        let mut committed_i = self.i;
        let mut committed_pos = (self.line, self.col);
        let mut first = true;
        while let Some(c) = self.peek(0) {
            let ok_first = is_pn_chars_u(c) || c == ':' || c.is_ascii_digit();
            let ok_rest = is_pn_chars(c) || c == ':' || c == '.';
            if c == '%' {
                if self.peek(1).is_some_and(|h| h.is_ascii_hexdigit())
                    && self.peek(2).is_some_and(|h| h.is_ascii_hexdigit())
                {
                    for _ in 0..3 {
                        value.push(self.bump().unwrap_or('%'));
                    }
                } else {
                    return self.err(
                        self.line,
                        self.col,
                        "`%` in a local name needs two hex digits",
                    );
                }
            } else if c == '\\' {
                match self.peek(1) {
                    Some(e) if PN_LOCAL_ESCAPABLE.contains(e) => {
                        self.bump();
                        self.bump();
                        value.push(e);
                    }
                    _ => return self.err(self.line, self.col, "invalid escape in a local name"),
                }
            } else if (first && ok_first) || (!first && ok_rest) {
                self.bump();
                value.push(c);
                if c == '.' {
                    first = false;
                    continue;
                }
            } else {
                break;
            }
            first = false;
            committed_len = value.len();
            committed_i = self.i;
            committed_pos = (self.line, self.col);
        }
        // Give back trailing dots.
        value.truncate(committed_len);
        self.i = committed_i;
        (self.line, self.col) = committed_pos;
        Ok(value)
    }

    fn at(&mut self) -> Result<Tok, ParseError> {
        let (line, col) = (self.line, self.col);
        self.bump(); // '@'
        if self.peek(0) == Some('<') {
            return Ok(Tok::At);
        }
        // ATPNAME_NS / ATPNAME_LN: '@' PN_PREFIX? ':' PN_LOCAL?
        let plen = if self.peek(0) == Some(':') {
            Some(0)
        } else {
            self.pn_prefix_len()
        };
        if let Some(n) = plen {
            if self.peek(n) == Some(':') {
                let prefix: String = self.chars[self.i..self.i + n].iter().collect();
                for _ in 0..=n {
                    self.bump();
                }
                let local = self.pn_local()?;
                return Ok(Tok::AtPName(prefix, local));
            }
        }
        // LANGTAG: '@' [a-zA-Z]+ ('-' [a-zA-Z0-9]+)*
        let mut tag = String::new();
        while self.peek(0).is_some_and(|c| c.is_ascii_alphabetic()) {
            tag.push(self.bump().unwrap_or('x'));
        }
        if tag.is_empty() {
            return self.err(
                line,
                col,
                "`@` must start a language tag, a shape reference `@ex:Shape` or `@<iri>`",
            );
        }
        while self.peek(0) == Some('-') && self.peek(1).is_some_and(|c| c.is_ascii_alphanumeric()) {
            tag.push(self.bump().unwrap_or('-'));
            while self.peek(0).is_some_and(|c| c.is_ascii_alphanumeric()) {
                tag.push(self.bump().unwrap_or('x'));
            }
        }
        Ok(Tok::LangTag(tag))
    }

    fn number(&mut self) -> Result<Tok, ParseError> {
        let (line, col) = (self.line, self.col);
        let mut s = String::new();
        if matches!(self.peek(0), Some('+' | '-')) {
            s.push(self.bump().unwrap_or('+'));
        }
        let mut int_digits = 0;
        while self.peek(0).is_some_and(|c| c.is_ascii_digit()) {
            s.push(self.bump().unwrap_or('0'));
            int_digits += 1;
        }
        let mut dot = false;
        let exp_follows = |l: &Lexer, k: usize| {
            matches!(l.peek(k), Some('e' | 'E'))
                && (l.peek(k + 1).is_some_and(|c| c.is_ascii_digit())
                    || (matches!(l.peek(k + 1), Some('+' | '-'))
                        && l.peek(k + 2).is_some_and(|c| c.is_ascii_digit())))
        };
        if self.peek(0) == Some('.') {
            if self.peek(1).is_some_and(|c| c.is_ascii_digit()) {
                dot = true;
                s.push(self.bump().unwrap_or('.'));
                while self.peek(0).is_some_and(|c| c.is_ascii_digit()) {
                    s.push(self.bump().unwrap_or('0'));
                }
            } else if int_digits > 0 && exp_follows(&*self, 1) {
                // DOUBLE: [0-9]+ '.' [0-9]* EXPONENT
                dot = true;
                s.push(self.bump().unwrap_or('.'));
            }
        }
        if int_digits == 0 && !dot {
            return self.err(line, col, "a number needs digits");
        }
        if exp_follows(&*self, 0) {
            s.push(self.bump().unwrap_or('e'));
            if matches!(self.peek(0), Some('+' | '-')) {
                s.push(self.bump().unwrap_or('+'));
            }
            while self.peek(0).is_some_and(|c| c.is_ascii_digit()) {
                s.push(self.bump().unwrap_or('0'));
            }
            return Ok(Tok::Double(s));
        }
        Ok(if dot {
            Tok::Decimal(s)
        } else {
            Tok::Integer(s)
        })
    }

    fn word_or_pname(&mut self) -> Result<Tok, ParseError> {
        if let Some(n) = self.pn_prefix_len() {
            if self.peek(n) == Some(':') {
                let prefix: String = self.chars[self.i..self.i + n].iter().collect();
                for _ in 0..=n {
                    self.bump();
                }
                let local = self.pn_local()?;
                return Ok(Tok::PName(prefix, local));
            }
        }
        let (line, col) = (self.line, self.col);
        let mut w = String::new();
        while self.peek(0).is_some_and(|c| c.is_ascii_alphabetic()) {
            w.push(self.bump().unwrap_or('x'));
        }
        if w.is_empty() {
            return self.err(line, col, "unexpected character");
        }
        Ok(Tok::Word(w))
    }
}

// ── Syntax tree ─────────────────────────────────────────────────────────

#[derive(Debug)]
struct ShapeDecl {
    iri: NamedNode,
    is_class: bool,
    targets: Vec<NamedNode>,
    body: Vec<Constraint>,
}

#[derive(Debug)]
enum Constraint {
    /// `nodeOr+`
    Node(Vec<Vec<NodeNot>>),
    Property(PropertyShape),
}

#[derive(Debug)]
struct NodeNot {
    negated: bool,
    param: String,
    value: Value,
}

#[derive(Debug)]
enum Value {
    Term(Term),
    Array(Vec<Term>),
}

#[derive(Debug)]
struct PropertyShape {
    path: Path,
    items: Vec<PropItem>,
}

#[derive(Debug)]
enum PropItem {
    Count { min: String, max: Option<String> },
    Or(Vec<PropNot>),
}

#[derive(Debug)]
struct PropNot {
    negated: bool,
    atom: Atom,
}

#[derive(Debug)]
enum Atom {
    Type(NamedNode),
    NodeKind(String),
    ShapeRef(NamedNode),
    Value(String, Value),
    Body(Vec<Constraint>),
}

#[derive(Debug)]
enum Path {
    Iri(NamedNode),
    Alternative(Vec<Path>),
    Sequence(Vec<Path>),
    Inverse(Box<Path>),
    Modified(Box<Path>, &'static str),
}

// ── Parser ──────────────────────────────────────────────────────────────

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    base: Option<Iri<String>>,
    prefixes: HashMap<String, String>,
    declared: Vec<(String, String)>,
    imports: Vec<NamedNode>,
    depth: usize,
}

fn describe(t: &Tok) -> String {
    match t {
        Tok::IriRef(i) => format!("<{i}>"),
        Tok::PName(p, l) => format!("{p}:{l}"),
        Tok::AtPName(p, l) => format!("@{p}:{l}"),
        Tok::At => "@".into(),
        Tok::LangTag(l) => format!("@{l}"),
        Tok::Integer(s) | Tok::Decimal(s) | Tok::Double(s) => s.clone(),
        Tok::Str(s) => format!("{s:?}"),
        Tok::Word(w) => w.clone(),
        Tok::Punct(p) => (*p).to_string(),
        Tok::Eof => "end of input".into(),
    }
}

impl Parser {
    fn peek(&self) -> &Tok {
        &self.tokens[self.pos.min(self.tokens.len() - 1)].tok
    }

    fn peek_at(&self, k: usize) -> &Tok {
        &self.tokens[(self.pos + k).min(self.tokens.len() - 1)].tok
    }

    fn next(&mut self) -> Token {
        let t = self.tokens[self.pos.min(self.tokens.len() - 1)].clone();
        if self.pos < self.tokens.len() - 1 {
            self.pos += 1;
        }
        t
    }

    fn err_here<T>(&self, message: impl Into<String>) -> Result<T, ParseError> {
        let t = &self.tokens[self.pos.min(self.tokens.len() - 1)];
        Err(ParseError {
            line: t.line,
            column: t.col,
            message: message.into(),
        })
    }

    fn unexpected<T>(&self, expected: &str) -> Result<T, ParseError> {
        let found = describe(self.peek());
        let mut msg = format!("expected {expected}, found `{found}`");
        if let Tok::Word(w) = self.peek() {
            if let Some(hint) = legacy_hint(w) {
                msg.push_str(hint);
            }
        }
        self.err_here(msg)
    }

    fn is_punct(&self, p: &str) -> bool {
        matches!(self.peek(), Tok::Punct(q) if *q == p)
    }

    fn expect_punct(&mut self, p: &str) -> Result<(), ParseError> {
        if self.is_punct(p) {
            self.next();
            Ok(())
        } else {
            self.unexpected(&format!("`{p}`"))
        }
    }

    fn is_word(&self, w: &str) -> bool {
        matches!(self.peek(), Tok::Word(x) if x == w)
    }

    fn enter(&mut self) -> Result<(), ParseError> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return self.err_here(format!("nesting deeper than {MAX_DEPTH} levels"));
        }
        Ok(())
    }

    fn document(&mut self) -> Result<Vec<ShapeDecl>, ParseError> {
        loop {
            if self.is_word("BASE") {
                self.next();
                let iri = self.iriref()?;
                self.base = Some(
                    Iri::parse(iri.as_str().to_string()).map_err(|e| ParseError {
                        line: 0,
                        column: 0,
                        message: e.to_string(),
                    })?,
                );
            } else if self.is_word("IMPORTS") {
                self.next();
                let iri = self.iriref()?;
                self.imports.push(iri);
            } else if self.is_word("PREFIX") {
                self.next();
                let (prefix, local) = match self.peek().clone() {
                    Tok::PName(p, l) => (p, l),
                    _ => return self.unexpected("a prefix label such as `ex:`"),
                };
                if !local.is_empty() {
                    return self.err_here(format!(
                        "a PREFIX label ends at the colon: write `{prefix}:`, not `{prefix}:{local}`"
                    ));
                }
                self.next();
                let ns = self.iriref()?;
                self.prefixes
                    .insert(prefix.clone(), ns.as_str().to_string());
                self.declared.retain(|(p, _)| *p != prefix);
                self.declared.push((prefix, ns.into_string()));
            } else {
                break;
            }
        }
        let mut shapes = Vec::new();
        loop {
            match self.peek() {
                Tok::Eof => break,
                Tok::Word(w) if w == "shape" || w == "shapeClass" => {
                    let is_class = w == "shapeClass";
                    self.next();
                    let iri = self.iri()?;
                    let mut targets = Vec::new();
                    if !is_class && self.is_punct("->") {
                        self.next();
                        while matches!(self.peek(), Tok::IriRef(_) | Tok::PName(..)) {
                            targets.push(self.iri()?);
                        }
                        if targets.is_empty() {
                            return self.unexpected("a target class after `->`");
                        }
                    }
                    let body = self.body()?;
                    shapes.push(ShapeDecl {
                        iri,
                        is_class,
                        targets,
                        body,
                    });
                }
                Tok::Word(w) if w == "BASE" || w == "IMPORTS" || w == "PREFIX" => {
                    return self.err_here(format!(
                        "`{w}` after a shape: BASE, IMPORTS and PREFIX directives come before every shape"
                    ));
                }
                _ => return self.unexpected("`shape`, `shapeClass` or the end of the document"),
            }
        }
        if self.base.is_none() && !self.imports.is_empty() {
            return self.err_here(
                "IMPORTS needs a base IRI to attach owl:imports to: add a BASE directive",
            );
        }
        Ok(shapes)
    }

    /// `IRIREF`, resolved against the current base.
    fn iriref(&mut self) -> Result<NamedNode, ParseError> {
        match self.peek().clone() {
            Tok::IriRef(raw) => {
                let iri = self.resolve(&raw)?;
                self.next();
                Ok(iri)
            }
            _ => self.unexpected("an IRI in angle brackets"),
        }
    }

    fn resolve(&self, raw: &str) -> Result<NamedNode, ParseError> {
        let resolved = match &self.base {
            Some(base) => base
                .resolve(raw)
                .map(|i| i.into_inner())
                .map_err(|e| e.to_string()),
            None => Iri::parse(raw.to_string())
                .map(|i| i.into_inner())
                .map_err(|e| format!("{e} (a relative IRI needs a BASE)")),
        };
        match resolved {
            Ok(iri) => Ok(NamedNode::new_unchecked(iri)),
            Err(e) => self.err_here(format!("invalid IRI <{raw}>: {e}")),
        }
    }

    fn pname(&self, prefix: &str, local: &str) -> Result<NamedNode, ParseError> {
        match self.prefixes.get(prefix) {
            Some(ns) => NamedNode::new(format!("{ns}{local}")).or_else(|e| {
                self.err_here(format!(
                    "`{prefix}:{local}` does not expand to a valid IRI: {e}"
                ))
            }),
            None => self.err_here(format!("undeclared prefix `{prefix}:`")),
        }
    }

    /// `iri : IRIREF | prefixedName`
    fn iri(&mut self) -> Result<NamedNode, ParseError> {
        match self.peek().clone() {
            Tok::IriRef(_) => self.iriref(),
            Tok::PName(p, l) => {
                let n = self.pname(&p, &l)?;
                self.next();
                Ok(n)
            }
            _ => self.unexpected("an IRI or prefixed name"),
        }
    }

    fn body(&mut self) -> Result<Vec<Constraint>, ParseError> {
        self.enter()?;
        self.expect_punct("{")?;
        let mut out = Vec::new();
        while !self.is_punct("}") {
            if matches!(self.peek(), Tok::Eof) {
                return self.unexpected("`}`");
            }
            out.push(self.constraint()?);
        }
        self.next();
        self.depth -= 1;
        Ok(out)
    }

    fn starts_node_or(&self) -> bool {
        match self.peek() {
            Tok::Punct("!") => true,
            Tok::Word(w) => is_node_param(w) && matches!(self.peek_at(1), Tok::Punct("=")),
            _ => false,
        }
    }

    /// `constraint : ( nodeOr+ | propertyShape ) '.'`
    fn constraint(&mut self) -> Result<Constraint, ParseError> {
        let c = if self.starts_node_or() {
            let mut ors = Vec::new();
            loop {
                ors.push(self.node_or()?);
                if self.is_punct(".") {
                    break;
                }
                if !self.starts_node_or() {
                    return self.unexpected("`.` ending the constraint, or another `name=value`");
                }
            }
            Constraint::Node(ors)
        } else if matches!(
            self.peek(),
            Tok::IriRef(_) | Tok::PName(..) | Tok::Punct("^") | Tok::Punct("(")
        ) {
            Constraint::Property(self.property_shape()?)
        } else {
            return self.unexpected("a constraint (`name=value` or a property path)");
        };
        self.expect_punct(".")?;
        Ok(c)
    }

    fn node_or(&mut self) -> Result<Vec<NodeNot>, ParseError> {
        let mut nots = vec![self.node_not()?];
        while self.is_punct("|") {
            self.next();
            nots.push(self.node_not()?);
        }
        Ok(nots)
    }

    fn node_not(&mut self) -> Result<NodeNot, ParseError> {
        let negated = self.is_punct("!");
        if negated {
            self.next();
        }
        let param = match self.peek().clone() {
            Tok::Word(w) if is_node_param(&w) => w,
            _ => return self.unexpected("a node parameter such as `class=` or `datatype=`"),
        };
        self.next();
        self.expect_punct("=")?;
        let value = self.value()?;
        Ok(NodeNot {
            negated,
            param,
            value,
        })
    }

    /// `iriOrLiteralOrArray`
    fn value(&mut self) -> Result<Value, ParseError> {
        if self.is_punct("[") {
            self.next();
            let mut items = Vec::new();
            while !self.is_punct("]") {
                items.push(self.iri_or_literal()?);
            }
            self.next();
            Ok(Value::Array(items))
        } else {
            Ok(Value::Term(self.iri_or_literal()?))
        }
    }

    fn iri_or_literal(&mut self) -> Result<Term, ParseError> {
        let xsd = |l: &str| NamedNode::new_unchecked(format!("{XSD}{l}"));
        match self.peek().clone() {
            Tok::IriRef(_) | Tok::PName(..) => Ok(self.iri()?.into()),
            Tok::Integer(s) => {
                self.next();
                Ok(Literal::new_typed_literal(s, xsd("integer")).into())
            }
            Tok::Decimal(s) => {
                self.next();
                Ok(Literal::new_typed_literal(s, xsd("decimal")).into())
            }
            Tok::Double(s) => {
                self.next();
                Ok(Literal::new_typed_literal(s, xsd("double")).into())
            }
            Tok::Word(w) if w == "true" || w == "false" => {
                self.next();
                Ok(Literal::new_typed_literal(w, xsd("boolean")).into())
            }
            Tok::Str(s) => {
                self.next();
                match self.peek().clone() {
                    Tok::LangTag(tag) => {
                        let lit = Literal::new_language_tagged_literal(s, &tag).or_else(|e| {
                            self.err_here(format!("invalid language tag `@{tag}`: {e}"))
                        })?;
                        self.next();
                        Ok(lit.into())
                    }
                    Tok::Punct("^^") => {
                        self.next();
                        let dt = self.iri()?;
                        Ok(Literal::new_typed_literal(s, dt).into())
                    }
                    _ => Ok(Literal::new_simple_literal(s).into()),
                }
            }
            _ => self.unexpected("an IRI or a literal"),
        }
    }

    /// `propertyShape : path ( propertyCount | propertyOr )*`
    fn property_shape(&mut self) -> Result<PropertyShape, ParseError> {
        let path = self.path()?;
        let mut items = Vec::new();
        loop {
            if self.is_punct("[") {
                self.next();
                let min = match self.peek().clone() {
                    Tok::Integer(s) => s,
                    _ => return self.unexpected("an integer minimum count"),
                };
                self.next();
                self.expect_punct("..")?;
                let max = match self.peek().clone() {
                    Tok::Integer(s) => Some(s),
                    Tok::Punct("*") => None,
                    _ => return self.unexpected("an integer maximum count or `*`"),
                };
                self.next();
                self.expect_punct("]")?;
                items.push(PropItem::Count { min, max });
            } else if self.starts_property_atom() {
                let mut nots = vec![self.prop_not()?];
                while self.is_punct("|") {
                    self.next();
                    nots.push(self.prop_not()?);
                }
                items.push(PropItem::Or(nots));
            } else {
                break;
            }
        }
        Ok(PropertyShape { path, items })
    }

    fn starts_property_atom(&self) -> bool {
        match self.peek() {
            Tok::Punct("!") | Tok::Punct("{") => true,
            Tok::IriRef(_) | Tok::PName(..) | Tok::AtPName(..) | Tok::At => true,
            Tok::Word(w) => {
                is_node_kind(w)
                    || (is_property_param(w) && matches!(self.peek_at(1), Tok::Punct("=")))
            }
            _ => false,
        }
    }

    fn prop_not(&mut self) -> Result<PropNot, ParseError> {
        let negated = self.is_punct("!");
        if negated {
            self.next();
        }
        let atom = match self.peek().clone() {
            Tok::IriRef(_) | Tok::PName(..) => Atom::Type(self.iri()?),
            Tok::AtPName(p, l) => {
                let n = self.pname(&p, &l)?;
                self.next();
                Atom::ShapeRef(n)
            }
            Tok::At => {
                self.next();
                Atom::ShapeRef(self.iriref()?)
            }
            Tok::Punct("{") => Atom::Body(self.body()?),
            Tok::Word(w) if is_node_kind(&w) => {
                self.next();
                Atom::NodeKind(w)
            }
            Tok::Word(w) if is_property_param(&w) => {
                self.next();
                self.expect_punct("=")?;
                Atom::Value(w, self.value()?)
            }
            _ => {
                return self.unexpected(
                    "a datatype or class, a node kind, `@shape`, `name=value` or `{ … }`",
                )
            }
        };
        Ok(PropNot { negated, atom })
    }

    /// `path : pathAlternative`
    fn path(&mut self) -> Result<Path, ParseError> {
        self.enter()?;
        let mut seqs = vec![self.path_sequence()?];
        while self.is_punct("|") {
            self.next();
            seqs.push(self.path_sequence()?);
        }
        self.depth -= 1;
        Ok(if seqs.len() == 1 {
            seqs.pop().expect("one")
        } else {
            Path::Alternative(seqs)
        })
    }

    fn path_sequence(&mut self) -> Result<Path, ParseError> {
        let mut elts = vec![self.path_elt_or_inverse()?];
        while self.is_punct("/") {
            self.next();
            elts.push(self.path_elt_or_inverse()?);
        }
        Ok(if elts.len() == 1 {
            elts.pop().expect("one")
        } else {
            Path::Sequence(elts)
        })
    }

    fn path_elt_or_inverse(&mut self) -> Result<Path, ParseError> {
        if self.is_punct("^") {
            self.next();
            Ok(Path::Inverse(Box::new(self.path_elt()?)))
        } else {
            self.path_elt()
        }
    }

    fn path_elt(&mut self) -> Result<Path, ParseError> {
        let primary = if self.is_punct("(") {
            self.next();
            let p = self.path()?;
            self.expect_punct(")")?;
            p
        } else {
            Path::Iri(self.iri()?)
        };
        Ok(match self.peek() {
            Tok::Punct("?") => {
                self.next();
                Path::Modified(Box::new(primary), "zeroOrOnePath")
            }
            Tok::Punct("*") => {
                self.next();
                Path::Modified(Box::new(primary), "zeroOrMorePath")
            }
            Tok::Punct("+") => {
                self.next();
                Path::Modified(Box::new(primary), "oneOrMorePath")
            }
            _ => primary,
        })
    }
}

const LEGACY_HINT: &str =
    " — this looks like the SHACL-C dialect of 0.7 and earlier; write the W3C form \
     (`closed=true`, `pattern=\"…\"`, `message=\"…\"`, `a|b`, `!x`, `@ex:Shape`, `.` after each \
     constraint) or pass `?dialect=legacy` (deprecated) to parse the old one";

/// A pointer to the pre-W3C dialect for a keyword only it had.
fn legacy_hint(word: &str) -> Option<&'static str> {
    match word {
        "closed" | "pattern" | "class" | "nodeKind" | "or" | "and" | "not" | "xone" | "imports"
        | "shapeRef" => Some(LEGACY_HINT),
        _ => None,
    }
}

// ── Emitter (the report's production rules) ─────────────────────────────

struct Emitter {
    graph: Graph,
}

impl Emitter {
    fn add(&mut self, s: &NamedOrBlankNode, p: &str, o: Term) {
        let p = NamedNode::new_unchecked(p);
        self.graph
            .insert(TripleRef::new(s.as_ref(), p.as_ref(), o.as_ref()));
    }

    fn sh(&mut self, s: &NamedOrBlankNode, local: &str, o: Term) {
        self.add(s, &format!("{SH}{local}"), o);
    }

    fn list(&mut self, items: Vec<Term>) -> Term {
        let mut head: Term = NamedNode::new_unchecked(RDF_NIL).into();
        for item in items.into_iter().rev() {
            let cell: NamedOrBlankNode = BlankNode::default().into();
            self.add(&cell, RDF_FIRST, item);
            self.add(&cell, RDF_REST, head);
            head = cell.into();
        }
        head
    }

    fn value(&mut self, v: &Value) -> Term {
        match v {
            Value::Term(t) => t.clone(),
            Value::Array(items) => self.list(items.clone()),
        }
    }

    fn shape(&mut self, s: &ShapeDecl) {
        let ctx: NamedOrBlankNode = s.iri.clone().into();
        self.add(
            &ctx,
            RDF_TYPE,
            NamedNode::new_unchecked(format!("{SH}NodeShape")).into(),
        );
        if s.is_class {
            self.add(&ctx, RDF_TYPE, NamedNode::new_unchecked(RDFS_CLASS).into());
        }
        for t in &s.targets {
            self.sh(&ctx, "targetClass", t.clone().into());
        }
        self.body(&ctx, &s.body);
    }

    fn body(&mut self, ctx: &NamedOrBlankNode, body: &[Constraint]) {
        for c in body {
            match c {
                Constraint::Node(ors) => {
                    for or in ors {
                        self.node_or(ctx, or);
                    }
                }
                Constraint::Property(p) => self.property(ctx, p),
            }
        }
    }

    fn node_or(&mut self, ctx: &NamedOrBlankNode, nots: &[NodeNot]) {
        if nots.len() == 1 {
            self.node_not(ctx, &nots[0]);
            return;
        }
        let mut members = Vec::new();
        for n in nots {
            let m: NamedOrBlankNode = BlankNode::default().into();
            self.node_not(&m, n);
            members.push(m.into());
        }
        let list = self.list(members);
        self.sh(ctx, "or", list);
    }

    fn node_not(&mut self, ctx: &NamedOrBlankNode, n: &NodeNot) {
        let target = if n.negated {
            let not: NamedOrBlankNode = BlankNode::default().into();
            self.sh(ctx, "not", not.clone().into());
            not
        } else {
            ctx.clone()
        };
        let v = self.value(&n.value);
        self.sh(&target, &n.param, v);
    }

    fn property(&mut self, ctx: &NamedOrBlankNode, p: &PropertyShape) {
        let prop: NamedOrBlankNode = BlankNode::default().into();
        self.sh(ctx, "property", prop.clone().into());
        let path = self.path(&p.path);
        self.sh(&prop, "path", path);
        let integer = NamedNode::new_unchecked(format!("{XSD}integer"));
        for item in &p.items {
            match item {
                PropItem::Count { min, max } => {
                    if min != "0" {
                        self.sh(
                            &prop,
                            "minCount",
                            Literal::new_typed_literal(min.as_str(), integer.clone()).into(),
                        );
                    }
                    if let Some(max) = max {
                        self.sh(
                            &prop,
                            "maxCount",
                            Literal::new_typed_literal(max.as_str(), integer.clone()).into(),
                        );
                    }
                }
                PropItem::Or(nots) => {
                    if nots.len() == 1 {
                        self.prop_not(&prop, &nots[0]);
                    } else {
                        let mut members = Vec::new();
                        for n in nots {
                            let m: NamedOrBlankNode = BlankNode::default().into();
                            self.prop_not(&m, n);
                            members.push(m.into());
                        }
                        let list = self.list(members);
                        self.sh(&prop, "or", list);
                    }
                }
            }
        }
    }

    fn prop_not(&mut self, ctx: &NamedOrBlankNode, n: &PropNot) {
        let target = if n.negated {
            let not: NamedOrBlankNode = BlankNode::default().into();
            self.sh(ctx, "not", not.clone().into());
            not
        } else {
            ctx.clone()
        };
        match &n.atom {
            Atom::Type(iri) => {
                let p = if is_builtin_datatype(iri.as_str()) {
                    "datatype"
                } else {
                    "class"
                };
                self.sh(&target, p, iri.clone().into());
            }
            Atom::NodeKind(k) => self.sh(
                &target,
                "nodeKind",
                NamedNode::new_unchecked(format!("{SH}{k}")).into(),
            ),
            Atom::ShapeRef(iri) => self.sh(&target, "node", iri.clone().into()),
            Atom::Value(param, v) => {
                let v = self.value(v);
                self.sh(&target, param, v);
            }
            Atom::Body(body) => {
                let node: NamedOrBlankNode = BlankNode::default().into();
                self.sh(&target, "node", node.clone().into());
                self.body(&node, body);
            }
        }
    }

    fn path(&mut self, p: &Path) -> Term {
        match p {
            Path::Iri(n) => n.clone().into(),
            Path::Alternative(alts) => {
                let members: Vec<Term> = alts.iter().map(|a| self.path(a)).collect();
                let list = self.list(members);
                let alt: NamedOrBlankNode = BlankNode::default().into();
                self.sh(&alt, "alternativePath", list);
                alt.into()
            }
            Path::Sequence(elts) => {
                let members: Vec<Term> = elts.iter().map(|a| self.path(a)).collect();
                self.list(members)
            }
            Path::Inverse(inner) => {
                let inv = self.path(inner);
                let node: NamedOrBlankNode = BlankNode::default().into();
                self.sh(&node, "inversePath", inv);
                node.into()
            }
            Path::Modified(inner, pred) => {
                let primary = self.path(inner);
                let node: NamedOrBlankNode = BlankNode::default().into();
                self.sh(&node, pred, primary);
                node.into()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turtle_of(input: &str) -> String {
        document_to_turtle(&parse_document(input, None).expect("parse")).expect("turtle")
    }

    #[test]
    fn bare_iri_after_a_path_is_a_class_and_xsd_is_a_datatype() {
        let t = turtle_of(
            "PREFIX ex: <http://example.org/>\nshape ex:S { ex:p ex:C . ex:q xsd:string . }",
        );
        assert!(t.contains("sh:class ex:C"), "{t}");
        assert!(t.contains("sh:datatype xsd:string"), "{t}");
        assert!(!t.contains("sh:node"), "{t}");
    }

    #[test]
    fn trailing_dot_is_the_terminator_not_part_of_the_name() {
        let doc = parse_document(
            "PREFIX ex: <http://example.org/>\nshape ex:S { ex:p ex:C. }",
            None,
        )
        .expect("parse");
        let objects: Vec<String> = doc.graph.iter().map(|t| t.object.to_string()).collect();
        assert!(
            objects.contains(&"<http://example.org/C>".to_string()),
            "{objects:?}"
        );
        assert!(!objects.iter().any(|o| o.contains("C.")), "{objects:?}");
    }

    #[test]
    fn messages_with_line_breaks_and_unicode_escapes() {
        let doc = parse_document(
            "PREFIX ex: <http://example.org/>\nshape ex:S { ex:p message=\"\"\"two\nlines\"\"\" message='caf\\u00E9'@fr . }",
            None,
        )
        .expect("parse");
        let lits: Vec<String> = doc
            .graph
            .iter()
            .filter(|t| t.predicate.as_str().ends_with("message"))
            .map(|t| t.object.to_string())
            .collect();
        assert!(lits.contains(&"\"two\\nlines\"".to_string()), "{lits:?}");
        assert!(lits.contains(&"\"café\"@fr".to_string()), "{lits:?}");
    }

    #[test]
    fn urn_iris_stay_iris() {
        let t = turtle_of("shape <urn:example:S> { <urn:example:p> [1..1] . }");
        assert!(t.contains("<urn:example:S>"), "{t}");
        assert!(t.contains("<urn:example:p>"), "{t}");
    }

    #[test]
    fn imports_need_a_base_and_produce_owl_imports() {
        let err = parse_document("IMPORTS <http://example.org/o>", None).unwrap_err();
        assert!(err.message.contains("BASE"), "{err}");
        let t = turtle_of("BASE <http://example.org/s>\nIMPORTS <http://example.org/o>");
        assert!(t.contains("owl:imports"), "{t}");
        assert!(t.contains("owl:Ontology"), "{t}");
    }

    #[test]
    fn undeclared_prefix_is_an_error_with_a_position() {
        let err = parse_document("shape foo:S {\n}", None).unwrap_err();
        assert_eq!((err.line, err.column), (1, 7), "{err}");
        assert!(err.message.contains("undeclared prefix"), "{err}");
    }

    #[test]
    fn legacy_keywords_point_at_the_dialect_switch() {
        let err = parse_document(
            "PREFIX ex: <http://example.org/>\nshape ex:S -> ex:T closed { }",
            None,
        )
        .unwrap_err();
        assert!(err.message.contains("dialect=legacy"), "{err}");
    }

    #[test]
    fn deep_nesting_is_refused_not_a_stack_overflow() {
        let mut s = String::from("PREFIX ex: <http://example.org/>\nshape ex:S { ex:p ");
        for _ in 0..200 {
            s.push_str("{ ex:p ");
        }
        let err = parse_document(&s, None).unwrap_err();
        assert!(err.message.contains("nesting"), "{err}");
    }

    #[test]
    fn numbers_and_counts_lex_apart() {
        let t = turtle_of(
            "PREFIX ex: <http://example.org/>\nshape ex:S { ex:p [0..1] minInclusive=1.5 maxInclusive=2e3 minLength=+1 . }",
        );
        assert!(t.contains("sh:maxCount 1"), "{t}");
        assert!(!t.contains("sh:minCount"), "{t}");
        assert!(t.contains("1.5"), "{t}");
        assert!(
            t.contains("2e3") || t.contains("2E3") || t.contains("2000"),
            "{t}"
        );
    }
}
