//! ShExC compact syntax parser.
//!
//! Parses the W3C ShExC grammar into the AST types defined in `schema.rs`.
//! Uses a simple recursive descent approach for shape bodies (avoiding
//! nom lifetime issues with borrowed prefix maps).

use std::collections::HashMap;

use super::schema::*;

/// Parse a ShExC document into a ShExSchema.
pub fn parse_shexc(input: &str) -> Result<ShExSchema, String> {
    let cleaned = strip_comments(input);
    let mut parser = ShExParser::new(&cleaned);
    parser.parse()
}

/// Strip line comments (# to end-of-line) outside of IRIs and strings.
fn strip_comments(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut in_iri = false;
    let mut in_string = false;
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '<' if !in_string => {
                in_iri = true;
                result.push(c);
            }
            '>' if in_iri => {
                in_iri = false;
                result.push(c);
            }
            '"' if !in_iri => {
                in_string = !in_string;
                result.push(c);
            }
            '#' if !in_iri && !in_string => {
                for ch in chars.by_ref() {
                    if ch == '\n' {
                        result.push('\n');
                        break;
                    }
                }
            }
            _ => result.push(c),
        }
    }
    result
}

// ── Recursive descent parser ────────────────────────────────────────────

struct ShExParser<'a> {
    input: &'a str,
    pos: usize,
    prefixes: HashMap<String, String>,
}

impl<'a> ShExParser<'a> {
    fn new(input: &'a str) -> Self {
        Self {
            input,
            pos: 0,
            prefixes: HashMap::new(),
        }
    }

    fn remaining(&self) -> &str {
        &self.input[self.pos..]
    }

    fn skip_ws(&mut self) {
        while self.pos < self.input.len() {
            let ch = self.input.as_bytes()[self.pos];
            if ch == b' ' || ch == b'\t' || ch == b'\n' || ch == b'\r' {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    fn peek(&self) -> Option<char> {
        self.remaining().chars().next()
    }

    fn starts_with(&self, s: &str) -> bool {
        self.remaining().starts_with(s)
    }

    fn starts_with_ci(&self, s: &str) -> bool {
        let rem = self.remaining();
        if rem.len() < s.len() {
            return false;
        }
        rem[..s.len()].eq_ignore_ascii_case(s)
    }

    fn advance(&mut self, n: usize) {
        self.pos += n;
    }

    fn expect_char(&mut self, c: char) -> Result<(), String> {
        self.skip_ws();
        if self.peek() == Some(c) {
            self.advance(c.len_utf8());
            Ok(())
        } else {
            Err(format!(
                "Expected '{}' at position {}, got {:?}",
                c,
                self.pos,
                self.peek()
            ))
        }
    }

    // ── Top-level ───────────────────────────────────────────────────────

    fn parse(&mut self) -> Result<ShExSchema, String> {
        let mut base = None;

        // Parse directives
        loop {
            self.skip_ws();
            if self.starts_with_ci("PREFIX") {
                self.advance(6);
                self.skip_ws();
                let label = self.parse_pname_ns()?;
                self.skip_ws();
                let iri = self.parse_iri_ref()?;
                self.prefixes.insert(label, iri);
            } else if self.starts_with_ci("BASE") {
                self.advance(4);
                self.skip_ws();
                base = Some(self.parse_iri_ref()?);
            } else {
                break;
            }
        }

        // Parse optional start declaration
        self.skip_ws();
        let start = if self.starts_with_ci("start") {
            let rem = &self.remaining()[5..];
            let next_non_ws = rem.chars().find(|c| !c.is_whitespace());
            if next_non_ws == Some('=') {
                self.advance(5);
                self.skip_ws();
                self.expect_char('=')?;
                self.skip_ws();
                Some(Box::new(self.parse_shape_expr()?))
            } else {
                None
            }
        } else {
            None
        };

        // Parse shape declarations
        let mut shapes = Vec::new();
        loop {
            self.skip_ws();
            if self.pos >= self.input.len() {
                break;
            }
            // Try to parse a shape declaration: IRI shapeExpr
            let id = match self.try_parse_iri_or_prefixed() {
                Some(iri) => iri,
                None => break,
            };
            self.skip_ws();
            let expr = self.parse_shape_expr()?;
            shapes.push(ShapeDecl {
                id,
                shape_expr: expr,
            });
        }

        Ok(ShExSchema {
            prefixes: self.prefixes.clone(),
            base,
            shapes,
            start,
        })
    }

    // ── Shape expressions ───────────────────────────────────────────────

    fn parse_shape_expr(&mut self) -> Result<ShapeExpr, String> {
        self.parse_shape_or()
    }

    fn parse_shape_or(&mut self) -> Result<ShapeExpr, String> {
        let first = self.parse_shape_and()?;
        let mut exprs = vec![first];
        loop {
            self.skip_ws();
            if self.starts_with("OR")
                && self
                    .remaining()
                    .as_bytes()
                    .get(2)
                    .map_or(true, |b| b.is_ascii_whitespace())
            {
                self.advance(2);
                self.skip_ws();
                exprs.push(self.parse_shape_and()?);
            } else {
                break;
            }
        }
        if exprs.len() == 1 {
            Ok(exprs.remove(0))
        } else {
            Ok(ShapeExpr::ShapeOr(exprs))
        }
    }

    fn parse_shape_and(&mut self) -> Result<ShapeExpr, String> {
        let first = self.parse_shape_not()?;
        let mut exprs = vec![first];
        loop {
            self.skip_ws();
            if self.starts_with("AND")
                && self
                    .remaining()
                    .as_bytes()
                    .get(3)
                    .map_or(true, |b| b.is_ascii_whitespace())
            {
                self.advance(3);
                self.skip_ws();
                exprs.push(self.parse_shape_not()?);
            } else {
                break;
            }
        }
        if exprs.len() == 1 {
            Ok(exprs.remove(0))
        } else {
            Ok(ShapeExpr::ShapeAnd(exprs))
        }
    }

    fn parse_shape_not(&mut self) -> Result<ShapeExpr, String> {
        self.skip_ws();
        if self.starts_with("NOT")
            && self
                .remaining()
                .as_bytes()
                .get(3)
                .map_or(true, |b| b.is_ascii_whitespace())
        {
            self.advance(3);
            self.skip_ws();
            let expr = self.parse_shape_atom()?;
            Ok(ShapeExpr::ShapeNot(Box::new(expr)))
        } else {
            self.parse_shape_atom()
        }
    }

    fn parse_shape_atom(&mut self) -> Result<ShapeExpr, String> {
        self.skip_ws();
        match self.peek() {
            Some('{') => self.parse_shape_body(false, vec![]),
            Some('@') => {
                self.advance(1);
                let iri = self.parse_iri_or_prefixed()?;
                Ok(ShapeExpr::ShapeRef(iri))
            }
            Some('(') => {
                self.advance(1);
                self.skip_ws();
                let expr = self.parse_shape_expr()?;
                self.skip_ws();
                self.expect_char(')')?;
                Ok(expr)
            }
            Some('.') => {
                self.advance(1);
                Ok(ShapeExpr::NodeConstraintAny)
            }
            Some('[') => {
                let nc = self.parse_value_set()?;
                Ok(ShapeExpr::NodeConstraint(nc))
            }
            _ => {
                // Try CLOSED keyword
                if self.starts_with_ci("CLOSED") {
                    self.advance(6);
                    self.skip_ws();
                    let extra = self.parse_extra_list()?;
                    return self.parse_shape_body(true, extra);
                }
                // Try EXTRA keyword
                if self.starts_with_ci("EXTRA") {
                    let extra = self.parse_extra_list()?;
                    self.skip_ws();
                    return self.parse_shape_body(false, extra);
                }
                // Try node kind keywords
                if let Some(nc) = self.try_parse_node_kind() {
                    return Ok(ShapeExpr::NodeConstraint(nc));
                }
                // Try datatype (prefixed name)
                if let Some(dt) = self.try_parse_prefixed_name() {
                    let mut nc = NodeConstraint {
                        datatype: Some(dt),
                        ..Default::default()
                    };
                    // Parse optional string facets
                    nc.string_facets = self.parse_string_facets();
                    return Ok(ShapeExpr::NodeConstraint(nc));
                }
                Err(format!(
                    "Expected shape expression at position {}: {:?}",
                    self.pos,
                    &self.remaining().chars().take(20).collect::<String>()
                ))
            }
        }
    }

    fn parse_extra_list(&mut self) -> Result<Vec<String>, String> {
        let mut extras = Vec::new();
        loop {
            self.skip_ws();
            if self.starts_with_ci("EXTRA") {
                self.advance(5);
                self.skip_ws();
                extras.push(self.parse_iri_or_prefixed()?);
            } else {
                break;
            }
        }
        Ok(extras)
    }

    // ── Shape body { ... } ──────────────────────────────────────────────

    fn parse_shape_body(&mut self, closed: bool, extra: Vec<String>) -> Result<ShapeExpr, String> {
        self.expect_char('{')?;
        self.skip_ws();
        let expr = if self.peek() != Some('}') {
            Some(self.parse_triple_expr()?)
        } else {
            None
        };
        self.skip_ws();
        self.expect_char('}')?;
        Ok(ShapeExpr::Shape {
            expression: expr,
            closed,
            extra,
        })
    }

    // ── Triple expressions ──────────────────────────────────────────────

    fn parse_triple_expr(&mut self) -> Result<TripleExpr, String> {
        self.parse_one_of()
    }

    fn parse_one_of(&mut self) -> Result<TripleExpr, String> {
        let first = self.parse_each_of()?;
        let mut exprs = vec![first];
        loop {
            self.skip_ws();
            if self.peek() == Some('|') {
                self.advance(1);
                self.skip_ws();
                exprs.push(self.parse_each_of()?);
            } else {
                break;
            }
        }
        if exprs.len() == 1 {
            Ok(exprs.remove(0))
        } else {
            Ok(TripleExpr::OneOf(exprs))
        }
    }

    fn parse_each_of(&mut self) -> Result<TripleExpr, String> {
        let first = self.parse_triple_constraint()?;
        let mut exprs = vec![first];
        loop {
            self.skip_ws();
            if self.peek() == Some(';') {
                self.advance(1);
                self.skip_ws();
                // Allow trailing semicolon before }
                if self.peek() == Some('}') || self.peek() == Some('|') {
                    break;
                }
                exprs.push(self.parse_triple_constraint()?);
            } else {
                break;
            }
        }
        if exprs.len() == 1 {
            Ok(exprs.remove(0))
        } else {
            Ok(TripleExpr::EachOf(exprs))
        }
    }

    fn parse_triple_constraint(&mut self) -> Result<TripleExpr, String> {
        self.skip_ws();
        let inverse = if self.peek() == Some('^') {
            self.advance(1);
            true
        } else {
            false
        };

        let predicate = self.parse_iri_or_prefixed()?;
        self.skip_ws();

        // Parse value expression (optional)
        let value_expr = self.try_parse_value_expr()?;
        self.skip_ws();

        // Parse cardinality
        let (min, max) = self.parse_cardinality();

        Ok(TripleExpr::TripleConstraint {
            predicate,
            inverse,
            value_expr,
            min,
            max,
            annotations: vec![],
        })
    }

    fn try_parse_value_expr(&mut self) -> Result<Option<Box<ShapeExpr>>, String> {
        self.skip_ws();
        match self.peek() {
            Some('@') => {
                self.advance(1);
                let iri = self.parse_iri_or_prefixed()?;
                Ok(Some(Box::new(ShapeExpr::ShapeRef(iri))))
            }
            Some('[') => {
                let nc = self.parse_value_set()?;
                Ok(Some(Box::new(ShapeExpr::NodeConstraint(nc))))
            }
            Some('{') => {
                let shape = self.parse_shape_body(false, vec![])?;
                Ok(Some(Box::new(shape)))
            }
            Some('.') => {
                // Check it's not a prefixed name containing a dot
                let after_dot = self.remaining().chars().nth(1);
                if after_dot.map_or(true, |c| {
                    c.is_whitespace()
                        || c == ';'
                        || c == '}'
                        || c == '|'
                        || c == '*'
                        || c == '+'
                        || c == '?'
                }) {
                    self.advance(1);
                    Ok(Some(Box::new(ShapeExpr::NodeConstraintAny)))
                } else {
                    Ok(None)
                }
            }
            _ => {
                // Try node kind keywords
                if let Some(nc) = self.try_parse_node_kind() {
                    return Ok(Some(Box::new(ShapeExpr::NodeConstraint(nc))));
                }
                // Try datatype (prefixed name) — only if not a cardinality or separator
                if let Some(dt) = self.try_parse_prefixed_name() {
                    let mut nc = NodeConstraint {
                        datatype: Some(dt),
                        ..Default::default()
                    };
                    nc.string_facets = self.parse_string_facets();
                    return Ok(Some(Box::new(ShapeExpr::NodeConstraint(nc))));
                }
                Ok(None)
            }
        }
    }

    // ── Cardinality ─────────────────────────────────────────────────────

    fn parse_cardinality(&mut self) -> (usize, Cardinality) {
        self.skip_ws();
        match self.peek() {
            Some('*') => {
                self.advance(1);
                (0, Cardinality::Unbounded)
            }
            Some('+') => {
                self.advance(1);
                (1, Cardinality::Unbounded)
            }
            Some('?') => {
                self.advance(1);
                (0, Cardinality::Exact(1))
            }
            Some('{') => {
                self.advance(1);
                self.skip_ws();
                let min = self.parse_usize().unwrap_or(1);
                self.skip_ws();
                if self.peek() == Some(',') {
                    self.advance(1);
                    self.skip_ws();
                    let max = if self.peek() == Some('*') {
                        self.advance(1);
                        Cardinality::Unbounded
                    } else {
                        Cardinality::Exact(self.parse_usize().unwrap_or(min))
                    };
                    self.skip_ws();
                    let _ = self.expect_char('}');
                    (min, max)
                } else {
                    self.skip_ws();
                    let _ = self.expect_char('}');
                    (min, Cardinality::Exact(min))
                }
            }
            _ => (1, Cardinality::Exact(1)),
        }
    }

    fn parse_usize(&mut self) -> Option<usize> {
        let start = self.pos;
        while self.pos < self.input.len() && self.input.as_bytes()[self.pos].is_ascii_digit() {
            self.pos += 1;
        }
        if self.pos > start {
            self.input[start..self.pos].parse().ok()
        } else {
            None
        }
    }

    // ── Node constraints ────────────────────────────────────────────────

    fn try_parse_node_kind(&mut self) -> Option<NodeConstraint> {
        let keywords = [
            ("NonLiteral", NodeKind::NonLiteral),
            ("Literal", NodeKind::Literal),
            ("BNode", NodeKind::BNode),
            ("IRI", NodeKind::IRI),
        ];
        for (kw, nk) in &keywords {
            if self.starts_with_ci(kw) {
                let after = self.remaining().as_bytes().get(kw.len());
                if after.map_or(true, |b| !b.is_ascii_alphanumeric() && *b != b'_') {
                    self.advance(kw.len());
                    return Some(NodeConstraint {
                        node_kind: Some(nk.clone()),
                        ..Default::default()
                    });
                }
            }
        }
        None
    }

    fn parse_value_set(&mut self) -> Result<NodeConstraint, String> {
        self.expect_char('[')?;
        let mut values = Vec::new();
        loop {
            self.skip_ws();
            if self.peek() == Some(']') {
                self.advance(1);
                break;
            }
            values.push(self.parse_iri_or_prefixed()?);
        }
        Ok(NodeConstraint {
            values,
            ..Default::default()
        })
    }

    fn parse_string_facets(&mut self) -> Vec<StringFacet> {
        let mut facets = Vec::new();
        loop {
            self.skip_ws();
            if self.starts_with_ci("PATTERN")
                && self
                    .remaining()
                    .as_bytes()
                    .get(7)
                    .map_or(true, |b| b.is_ascii_whitespace())
            {
                self.advance(7);
                self.skip_ws();
                if let Ok(pat) = self.parse_quoted_string() {
                    facets.push(StringFacet::Pattern(pat, None));
                }
            } else if self.starts_with_ci("MINLENGTH")
                && self
                    .remaining()
                    .as_bytes()
                    .get(9)
                    .map_or(true, |b| b.is_ascii_whitespace())
            {
                self.advance(9);
                self.skip_ws();
                if let Some(n) = self.parse_usize() {
                    facets.push(StringFacet::MinLength(n));
                }
            } else if self.starts_with_ci("MAXLENGTH")
                && self
                    .remaining()
                    .as_bytes()
                    .get(9)
                    .map_or(true, |b| b.is_ascii_whitespace())
            {
                self.advance(9);
                self.skip_ws();
                if let Some(n) = self.parse_usize() {
                    facets.push(StringFacet::MaxLength(n));
                }
            } else if self.starts_with_ci("LENGTH")
                && self
                    .remaining()
                    .as_bytes()
                    .get(6)
                    .map_or(true, |b| b.is_ascii_whitespace())
            {
                self.advance(6);
                self.skip_ws();
                if let Some(n) = self.parse_usize() {
                    facets.push(StringFacet::Length(n));
                }
            } else {
                break;
            }
        }
        facets
    }

    // ── IRI / prefixed name helpers ─────────────────────────────────────

    fn parse_iri_ref(&mut self) -> Result<String, String> {
        self.expect_char('<')?;
        let start = self.pos;
        while self.pos < self.input.len() && self.input.as_bytes()[self.pos] != b'>' {
            self.pos += 1;
        }
        let iri = self.input[start..self.pos].to_string();
        self.expect_char('>')?;
        Ok(iri)
    }

    fn parse_pname_ns(&mut self) -> Result<String, String> {
        let start = self.pos;
        while self.pos < self.input.len() {
            let ch = self.input.as_bytes()[self.pos];
            if ch == b':' {
                let label = self.input[start..self.pos].to_string();
                self.pos += 1; // consume ':'
                return Ok(label);
            }
            if ch.is_ascii_alphanumeric() || ch == b'_' || ch == b'-' {
                self.pos += 1;
            } else {
                break;
            }
        }
        Err(format!("Expected prefix label at position {}", start))
    }

    fn try_parse_prefixed_name(&mut self) -> Option<String> {
        let saved = self.pos;
        let start = self.pos;

        // Parse prefix part
        while self.pos < self.input.len() {
            let ch = self.input.as_bytes()[self.pos];
            if ch.is_ascii_alphanumeric() || ch == b'_' || ch == b'-' {
                self.pos += 1;
            } else {
                break;
            }
        }

        if self.pos >= self.input.len() || self.input.as_bytes()[self.pos] != b':' {
            self.pos = saved;
            return None;
        }

        let prefix = &self.input[start..self.pos];
        self.pos += 1; // consume ':'

        // Parse local part
        let local_start = self.pos;
        while self.pos < self.input.len() {
            let ch = self.input.as_bytes()[self.pos];
            if ch.is_ascii_alphanumeric() || ch == b'_' || ch == b'-' || ch == b'.' {
                self.pos += 1;
            } else {
                break;
            }
        }
        // Don't end on a dot
        while self.pos > local_start && self.input.as_bytes()[self.pos - 1] == b'.' {
            self.pos -= 1;
        }
        let local = &self.input[local_start..self.pos];

        if let Some(ns) = self.prefixes.get(prefix) {
            Some(format!("{}{}", ns, local))
        } else {
            Some(format!("{}:{}", prefix, local))
        }
    }

    fn parse_iri_or_prefixed(&mut self) -> Result<String, String> {
        self.skip_ws();
        if self.peek() == Some('<') {
            self.parse_iri_ref()
        } else {
            self.try_parse_prefixed_name()
                .ok_or_else(|| format!("Expected IRI or prefixed name at position {}", self.pos))
        }
    }

    fn try_parse_iri_or_prefixed(&mut self) -> Option<String> {
        self.skip_ws();
        if self.peek() == Some('<') {
            self.parse_iri_ref().ok()
        } else {
            self.try_parse_prefixed_name()
        }
    }

    fn parse_quoted_string(&mut self) -> Result<String, String> {
        self.expect_char('"')?;
        let start = self.pos;
        while self.pos < self.input.len() && self.input.as_bytes()[self.pos] != b'"' {
            self.pos += 1;
        }
        let content = self.input[start..self.pos].to_string();
        self.expect_char('"')?;
        Ok(content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_empty_schema() {
        let schema = parse_shexc("").unwrap();
        assert!(schema.shapes.is_empty());
        assert!(schema.start.is_none());
    }

    #[test]
    fn test_parse_prefix() {
        let input = r#"PREFIX ex: <http://example.org/>
PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>

ex:PersonShape {
    ex:name xsd:string ;
    ex:age xsd:integer ?
}
"#;
        let schema = parse_shexc(input).unwrap();
        assert_eq!(schema.prefixes.len(), 2);
        assert_eq!(schema.shapes.len(), 1);
        assert_eq!(schema.shapes[0].id, "http://example.org/PersonShape");
    }

    #[test]
    fn test_parse_cardinality() {
        let mut p = ShExParser::new("*");
        assert_eq!(p.parse_cardinality(), (0, Cardinality::Unbounded));
        let mut p = ShExParser::new("+");
        assert_eq!(p.parse_cardinality(), (1, Cardinality::Unbounded));
        let mut p = ShExParser::new("?");
        assert_eq!(p.parse_cardinality(), (0, Cardinality::Exact(1)));
        let mut p = ShExParser::new("{2,5}");
        assert_eq!(p.parse_cardinality(), (2, Cardinality::Exact(5)));
    }

    #[test]
    fn test_parse_shape_with_node_constraints() {
        let input = r#"PREFIX ex: <http://example.org/>
PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>

ex:Shape {
    ex:id IRI ;
    ex:label xsd:string MINLENGTH 1 MAXLENGTH 100
}
"#;
        let schema = parse_shexc(input).unwrap();
        assert_eq!(schema.shapes.len(), 1);
    }

    #[test]
    fn test_strip_comments() {
        let input = "PREFIX ex: <http://example.org/> # a comment\nex:Shape {}";
        let cleaned = strip_comments(input);
        assert!(!cleaned.contains("# a comment"));
        assert!(cleaned.contains("PREFIX"));
    }

    #[test]
    fn test_parse_shape_ref() {
        let input = r#"PREFIX ex: <http://example.org/>

ex:PersonShape {
    ex:knows @ex:PersonShape *
}
"#;
        let schema = parse_shexc(input).unwrap();
        assert_eq!(schema.shapes.len(), 1);
    }

    #[test]
    fn test_parse_closed_shape() {
        let input = r#"PREFIX ex: <http://example.org/>
PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>

ex:StrictShape CLOSED {
    ex:name xsd:string
}
"#;
        let schema = parse_shexc(input).unwrap();
        assert_eq!(schema.shapes.len(), 1);
        if let ShapeExpr::Shape { closed, .. } = &schema.shapes[0].shape_expr {
            assert!(*closed);
        } else {
            panic!("Expected Shape variant");
        }
    }

    #[test]
    fn test_parse_value_set() {
        let input = r#"PREFIX ex: <http://example.org/>

ex:StatusShape {
    ex:status [ex:Active ex:Inactive ex:Pending]
}
"#;
        let schema = parse_shexc(input).unwrap();
        assert_eq!(schema.shapes.len(), 1);
    }
}
