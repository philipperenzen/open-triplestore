//! SHACLC → Turtle parser using nom combinators.
//!
//! Supports the W3C SHACL Compact Syntax:
//! - PREFIX declarations
//! - `shape <IRI> -> <TargetClass> { property constraints }`
//! - Property constraints: `<path> <datatype> [min..max] // "message"`
//! - Logical: `or`, `and`, `not`, `xone`
//! - `shapeRef <IRI>` for referencing other shapes
//! - `imports <IRI>` declarations

use nom::{
    branch::alt,
    bytes::complete::{tag, tag_no_case, take_while},
    character::complete::{char, multispace0, multispace1, digit1},
    combinator::{map, map_res, opt},
    multi::many0,
    sequence::{delimited, pair, preceded},
    IResult,
};
use std::fmt::Write;

/// Parse SHACLC text and return equivalent Turtle.
pub fn parse(input: &str) -> Result<String, String> {
    match parse_shaclc(input) {
        Ok((_, doc)) => Ok(doc.to_turtle()),
        Err(e) => Err(format!("SHACLC parse error: {}", e)),
    }
}

// ── AST ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct ShaclcDoc {
    prefixes: Vec<(String, String)>,
    imports: Vec<String>,
    shapes: Vec<ShapeDecl>,
}

#[derive(Debug, Clone)]
struct ShapeDecl {
    iri: String,
    target_class: Option<String>,
    closed: bool,
    properties: Vec<PropertyDecl>,
    node_constraints: Vec<NodeConstraint>,
}

#[derive(Debug, Clone)]
struct PropertyDecl {
    path: String,
    datatype: Option<String>,
    node_kind: Option<String>,
    min_count: Option<usize>,
    max_count: Option<usize>,
    pattern: Option<String>,
    min_length: Option<usize>,
    max_length: Option<usize>,
    shape_ref: Option<String>,
    in_values: Vec<String>,
    message: Option<String>,
    logical: Vec<LogicalConstraint>,
}

#[derive(Debug, Clone)]
enum LogicalConstraint {
    Or(Vec<String>),
    And(Vec<String>),
    Not(String),
    Xone(Vec<String>),
}

#[derive(Debug, Clone)]
enum NodeConstraint {
    Class(String),
    NodeKind(String),
}

impl ShaclcDoc {
    fn to_turtle(&self) -> String {
        let mut out = String::with_capacity(2048);

        // Prefixes
        for (prefix, iri) in &self.prefixes {
            writeln!(out, "@prefix {prefix}: <{iri}> .").unwrap();
        }
        writeln!(out, "@prefix sh: <http://www.w3.org/ns/shacl#> .").unwrap();
        if !self.prefixes.iter().any(|(p, _)| p == "rdf") {
            writeln!(out, "@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .").unwrap();
        }
        if !self.prefixes.iter().any(|(p, _)| p == "xsd") {
            writeln!(out, "@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .").unwrap();
        }
        writeln!(out).unwrap();

        // Imports
        for import_iri in &self.imports {
            writeln!(out, "<> owl:imports <{import_iri}> .").unwrap();
        }
        if !self.imports.is_empty() {
            writeln!(out).unwrap();
        }

        // Shapes
        for shape in &self.shapes {
            shape.to_turtle(&mut out, &self.prefixes);
            writeln!(out).unwrap();
        }

        out
    }
}

impl ShapeDecl {
    fn to_turtle(&self, out: &mut String, prefixes: &[(String, String)]) {
        let iri_ref = format_iri(&self.iri, prefixes);
        writeln!(out, "{iri_ref}").unwrap();
        writeln!(out, "    a sh:NodeShape ;").unwrap();

        if let Some(ref tc) = self.target_class {
            let tc_ref = format_iri(tc, prefixes);
            writeln!(out, "    sh:targetClass {tc_ref} ;").unwrap();
        }

        if self.closed {
            writeln!(out, "    sh:closed true ;").unwrap();
        }

        for nc in &self.node_constraints {
            match nc {
                NodeConstraint::Class(c) => {
                    writeln!(out, "    sh:class {} ;", format_iri(c, prefixes)).unwrap();
                }
                NodeConstraint::NodeKind(nk) => {
                    writeln!(out, "    sh:nodeKind sh:{nk} ;").unwrap();
                }
            }
        }

        for (i, prop) in self.properties.iter().enumerate() {
            let last = i == self.properties.len() - 1;
            let term = if last { "." } else { ";" };
            prop.to_turtle(out, prefixes, term);
        }

        if self.properties.is_empty() {
            // Close the shape
            out.push_str("    .\n");
        }
    }
}

impl PropertyDecl {
    fn to_turtle(&self, out: &mut String, prefixes: &[(String, String)], terminator: &str) {
        let path_ref = format_iri(&self.path, prefixes);
        writeln!(out, "    sh:property [").unwrap();
        writeln!(out, "        sh:path {path_ref} ;").unwrap();

        if let Some(ref dt) = self.datatype {
            writeln!(out, "        sh:datatype {} ;", format_iri(dt, prefixes)).unwrap();
        }
        if let Some(ref nk) = self.node_kind {
            writeln!(out, "        sh:nodeKind sh:{nk} ;").unwrap();
        }
        if let Some(min) = self.min_count {
            writeln!(out, "        sh:minCount {min} ;").unwrap();
        }
        if let Some(max) = self.max_count {
            writeln!(out, "        sh:maxCount {max} ;").unwrap();
        }
        if let Some(ref pat) = self.pattern {
            writeln!(out, "        sh:pattern \"{}\" ;", pat.replace('\\', "\\\\").replace('"', "\\\"")).unwrap();
        }
        if let Some(min) = self.min_length {
            writeln!(out, "        sh:minLength {min} ;").unwrap();
        }
        if let Some(max) = self.max_length {
            writeln!(out, "        sh:maxLength {max} ;").unwrap();
        }
        if let Some(ref sr) = self.shape_ref {
            writeln!(out, "        sh:node {} ;", format_iri(sr, prefixes)).unwrap();
        }
        if !self.in_values.is_empty() {
            let vals: Vec<String> = self.in_values.iter().map(|v| format_iri(v, prefixes)).collect();
            writeln!(out, "        sh:in ({}) ;", vals.join(" ")).unwrap();
        }
        if let Some(ref msg) = self.message {
            writeln!(out, "        sh:message \"{}\" ;", msg.replace('"', "\\\"")).unwrap();
        }

        for lc in &self.logical {
            match lc {
                LogicalConstraint::Or(refs) => {
                    let vals: Vec<String> = refs.iter().map(|r| format_iri(r, prefixes)).collect();
                    writeln!(out, "        sh:or ({}) ;", vals.join(" ")).unwrap();
                }
                LogicalConstraint::And(refs) => {
                    let vals: Vec<String> = refs.iter().map(|r| format_iri(r, prefixes)).collect();
                    writeln!(out, "        sh:and ({}) ;", vals.join(" ")).unwrap();
                }
                LogicalConstraint::Not(r) => {
                    writeln!(out, "        sh:not {} ;", format_iri(r, prefixes)).unwrap();
                }
                LogicalConstraint::Xone(refs) => {
                    let vals: Vec<String> = refs.iter().map(|r| format_iri(r, prefixes)).collect();
                    writeln!(out, "        sh:xone ({}) ;", vals.join(" ")).unwrap();
                }
            }
        }

        writeln!(out, "    ] {terminator}").unwrap();
    }
}

/// Format an IRI using prefixes if possible, otherwise wrap in <>.
fn format_iri(iri: &str, prefixes: &[(String, String)]) -> String {
    // If already prefixed (e.g. "schema:name"), return as-is
    if iri.contains(':') && !iri.starts_with("http://") && !iri.starts_with("https://") && !iri.starts_with('<') {
        return iri.to_string();
    }
    // Try to compact using prefixes
    for (prefix, ns) in prefixes {
        if iri.starts_with(ns.as_str()) {
            let local = &iri[ns.len()..];
            return format!("{prefix}:{local}");
        }
    }
    format!("<{iri}>")
}

// ── Nom Parsers ─────────────────────────────────────────────────────────

fn comment(input: &str) -> IResult<&str, ()> {
    let (input, _) = char('#')(input)?;
    let (input, _) = take_while(|c| c != '\n')(input)?;
    Ok((input, ()))
}

fn ws_comments(input: &str) -> IResult<&str, ()> {
    let (mut input, _) = multispace0(input)?;
    while let Ok((rest, _)) = comment(input) {
        let (rest, _) = multispace0(rest)?;
        input = rest;
    }
    Ok((input, ()))
}

/// Parse a prefixed name like `schema:name`
fn prefixed_name(input: &str) -> IResult<&str, String> {
    let (input, prefix) = take_while(|c: char| c.is_alphanumeric() || c == '_')(input)?;
    let (input, _) = char(':')(input)?;
    let (input, local) = take_while(|c: char| c.is_alphanumeric() || c == '_' || c == '-' || c == '.')(input)?;
    Ok((input, format!("{prefix}:{local}")))
}

/// Parse an IRI in angle brackets: `<http://...>`
fn iri_ref(input: &str) -> IResult<&str, String> {
    let (input, iri) = delimited(char('<'), take_while(|c| c != '>'), char('>'))(input)?;
    Ok((input, iri.to_string()))
}

/// Parse either a prefixed name or a full IRI
fn iri_or_prefixed(input: &str) -> IResult<&str, String> {
    alt((iri_ref, prefixed_name))(input)
}

/// Parse a PREFIX declaration
fn prefix_decl(input: &str) -> IResult<&str, (String, String)> {
    let (input, _) = ws_comments(input)?;
    let (input, _) = tag_no_case("PREFIX")(input)?;
    let (input, _) = multispace1(input)?;
    let (input, prefix) = take_while(|c: char| c.is_alphanumeric() || c == '_')(input)?;
    let (input, _) = char(':')(input)?;
    let (input, _) = multispace0(input)?;
    let (input, iri) = iri_ref(input)?;
    let (input, _) = ws_comments(input)?;
    Ok((input, (prefix.to_string(), iri)))
}

/// Parse an imports declaration
fn imports_decl(input: &str) -> IResult<&str, String> {
    let (input, _) = ws_comments(input)?;
    let (input, _) = tag_no_case("imports")(input)?;
    let (input, _) = multispace1(input)?;
    let (input, iri) = iri_or_prefixed(input)?;
    let (input, _) = ws_comments(input)?;
    Ok((input, iri))
}

/// Parse a quoted string
fn quoted_string(input: &str) -> IResult<&str, String> {
    let (input, _) = char('"')(input)?;
    let mut result = String::new();
    let mut chars = input.char_indices();
    loop {
        match chars.next() {
            Some((_, '\\')) => {
                if let Some((_, c)) = chars.next() {
                    match c {
                        'n' => result.push('\n'),
                        'r' => result.push('\r'),
                        't' => result.push('\t'),
                        '"' => result.push('"'),
                        '\\' => result.push('\\'),
                        _ => { result.push('\\'); result.push(c); }
                    }
                }
            }
            Some((i, '"')) => {
                return Ok((&input[i + 1..], result));
            }
            Some((_, c)) => result.push(c),
            None => return Err(nom::Err::Error(nom::error::Error::new(input, nom::error::ErrorKind::Char))),
        }
    }
}

/// Parse a cardinality like `[1..1]`, `[0..*]`, `[1..]`
fn cardinality(input: &str) -> IResult<&str, (Option<usize>, Option<usize>)> {
    let (input, _) = char('[')(input)?;
    let (input, _) = multispace0(input)?;
    let (input, min) = opt(map_res(digit1, |s: &str| s.parse::<usize>()))(input)?;
    let (input, _) = multispace0(input)?;
    let (input, _) = tag("..")(input)?;
    let (input, _) = multispace0(input)?;
    let (input, max) = alt((
        map(char('*'), |_| None),
        map(map_res(digit1, |s: &str| s.parse::<usize>()), Some),
    ))(input)?;
    let (input, _) = multispace0(input)?;
    let (input, _) = char(']')(input)?;
    Ok((input, (min, max)))
}

/// Parse a parenthesized list of IRIs: `( iri1 iri2 ... )`
fn paren_iri_list(input: &str) -> IResult<&str, Vec<String>> {
    let (input, _) = char('(')(input)?;
    let mut iris = Vec::new();
    let mut cur = input;
    loop {
        let (rest, _) = multispace0(cur)?;
        if let Ok((rest, _)) = char::<&str, nom::error::Error<&str>>(')')(rest) {
            return Ok((rest, iris));
        }
        let (rest, iri) = iri_or_prefixed(rest)?;
        iris.push(iri);
        cur = rest;
    }
}

/// Parse a node-level constraint keyword inside a shape body: `class <IRI>` or `nodeKind <value>`
fn node_constraint_kw(input: &str) -> IResult<&str, NodeConstraint> {
    let (input, _) = ws_comments(input)?;
    let (input, kw) = alt((
        tag_no_case::<&str, &str, nom::error::Error<&str>>("nodeKind"),
        tag_no_case("class"),
    ))(input)?;
    let (input, _) = multispace1(input)?;
    let (input, val) = iri_or_prefixed(input)?;
    let (input, _) = opt(pair(multispace0, alt((char(';'), char('.')))))(input)?;
    let (input, _) = ws_comments(input)?;
    let nc = match kw.to_lowercase().as_str() {
        "class" => NodeConstraint::Class(val),
        _ => NodeConstraint::NodeKind(val),
    };
    Ok((input, nc))
}

/// A single item in a shape body: either a node-level constraint or a property constraint.
enum ShapeBodyItem {
    Node(NodeConstraint),
    Property(PropertyDecl),
}

fn shape_body_item(input: &str) -> IResult<&str, ShapeBodyItem> {
    alt((
        map(node_constraint_kw, ShapeBodyItem::Node),
        map(property_constraint, ShapeBodyItem::Property),
    ))(input)
}

/// Parse a property constraint inside a shape body
fn property_constraint(input: &str) -> IResult<&str, PropertyDecl> {
    let (input, _) = ws_comments(input)?;
    let (input, path) = iri_or_prefixed(input)?;
    let (input, _) = multispace0(input)?;

    let mut prop = PropertyDecl {
        path,
        datatype: None,
        node_kind: None,
        min_count: None,
        max_count: None,
        pattern: None,
        min_length: None,
        max_length: None,
        shape_ref: None,
        in_values: Vec::new(),
        message: None,
        logical: Vec::new(),
    };

    // Parse optional datatype / nodeKind / shapeRef
    let (mut input, dt_or_ref) = opt(iri_or_prefixed)(input)?;
    if let Some(ref val) = dt_or_ref {
        match val.as_str() {
            "IRI" | "BlankNode" | "Literal" | "BlankNodeOrIRI" | "BlankNodeOrLiteral" | "IRIOrLiteral" => {
                prop.node_kind = Some(val.clone());
            }
            _ => {
                // Could be a datatype or a shape reference
                if val.starts_with("xsd:") || val.contains("XMLSchema") || val.starts_with("rdf:") {
                    prop.datatype = Some(val.clone());
                } else {
                    // Treat as shape reference
                    prop.shape_ref = Some(val.clone());
                }
            }
        }
    }

    // Parse optional cardinality
    let (input2, _) = multispace0(input)?;
    input = input2;
    if let Ok((rest, (min, max))) = cardinality(input) {
        prop.min_count = min;
        prop.max_count = max;
        let (rest, _) = multispace0(rest)?;
        input = rest;
    }

    // Parse optional pattern
    if let Ok((rest, _)) = tag::<&str, &str, nom::error::Error<&str>>("pattern")(input) {
        let (rest, _) = multispace0(rest)?;
        if let Ok((rest, pat)) = quoted_string(rest) {
            prop.pattern = Some(pat);
            let (rest, _) = multispace0(rest)?;
            input = rest;
        }
    }

    // Parse optional message (// "message")
    if let Ok((rest, _)) = tag::<&str, &str, nom::error::Error<&str>>("//")(input) {
        let (rest, _) = multispace0(rest)?;
        if let Ok((rest, msg)) = quoted_string(rest) {
            prop.message = Some(msg);
            let (rest, _) = multispace0(rest)?;
            input = rest;
        }
    }

    // Parse optional logical constraints (sh:or, sh:and, sh:not, sh:xone at property level)
    loop {
        let (input_tmp, _) = multispace0(input)?;
        if let Ok((rest, _)) =
            tag_no_case::<&str, &str, nom::error::Error<&str>>("or")(input_tmp)
        {
            let (rest, _) = multispace0(rest)?;
            if let Ok((rest, refs)) = paren_iri_list(rest) {
                prop.logical.push(LogicalConstraint::Or(refs));
                input = rest;
            } else {
                break;
            }
        } else if let Ok((rest, _)) =
            tag_no_case::<&str, &str, nom::error::Error<&str>>("and")(input_tmp)
        {
            let (rest, _) = multispace0(rest)?;
            if let Ok((rest, refs)) = paren_iri_list(rest) {
                prop.logical.push(LogicalConstraint::And(refs));
                input = rest;
            } else {
                break;
            }
        } else if let Ok((rest, _)) =
            tag_no_case::<&str, &str, nom::error::Error<&str>>("not")(input_tmp)
        {
            if let Ok((rest, _)) = multispace1::<&str, nom::error::Error<&str>>(rest) {
                if let Ok((rest, r)) = iri_or_prefixed(rest) {
                    prop.logical.push(LogicalConstraint::Not(r));
                    input = rest;
                } else {
                    break;
                }
            } else {
                break;
            }
        } else if let Ok((rest, _)) =
            tag_no_case::<&str, &str, nom::error::Error<&str>>("xone")(input_tmp)
        {
            let (rest, _) = multispace0(rest)?;
            if let Ok((rest, refs)) = paren_iri_list(rest) {
                prop.logical.push(LogicalConstraint::Xone(refs));
                input = rest;
            } else {
                break;
            }
        } else {
            break;
        }
    }

    // Consume optional semicolon or period
    let (input, _) = opt(pair(multispace0, alt((char(';'), char('.')))))(input)?;
    let (input, _) = ws_comments(input)?;

    Ok((input, prop))
}

/// Parse a shape declaration
fn shape_decl(input: &str) -> IResult<&str, ShapeDecl> {
    let (input, _) = ws_comments(input)?;
    let (input, _) = tag_no_case("shape")(input)?;
    let (input, _) = multispace1(input)?;
    let (input, iri) = iri_or_prefixed(input)?;
    let (input, _) = multispace0(input)?;

    // Optional -> TargetClass
    let (input, target) = opt(preceded(
        pair(tag("->"), multispace0),
        iri_or_prefixed,
    ))(input)?;

    let (input, _) = multispace0(input)?;

    // Optional "closed" keyword
    let (input, closed) = opt(tag_no_case("closed"))(input)?;
    let is_closed = closed.is_some();
    let (input, _) = multispace0(input)?;

    // Shape body in braces
    let (input, _) = char('{')(input)?;
    let (input, _) = ws_comments(input)?;

    // Parse body items: node-level constraints (class, nodeKind) and property constraints
    let (input, items) = many0(shape_body_item)(input)?;
    let mut node_constraints = Vec::new();
    let mut properties = Vec::new();
    for item in items {
        match item {
            ShapeBodyItem::Node(nc) => node_constraints.push(nc),
            ShapeBodyItem::Property(pc) => properties.push(pc),
        }
    };

    let (input, _) = ws_comments(input)?;
    let (input, _) = char('}')(input)?;
    let (input, _) = ws_comments(input)?;

    Ok((input, ShapeDecl {
        iri,
        target_class: target,
        closed: is_closed,
        properties,
        node_constraints,
    }))
}

/// Parse a complete SHACLC document
fn parse_shaclc(input: &str) -> IResult<&str, ShaclcDoc> {
    let (input, _) = ws_comments(input)?;
    let (input, prefixes) = many0(prefix_decl)(input)?;
    let (input, imports) = many0(imports_decl)(input)?;
    let (input, shapes) = many0(shape_decl)(input)?;
    let (input, _) = ws_comments(input)?;
    Ok((input, ShaclcDoc { prefixes, imports, shapes }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_shape() {
        let input = r#"
PREFIX schema: <http://schema.org/>
PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>

shape schema:PersonShape -> schema:Person {
    schema:name xsd:string [1..1] ;
    schema:age xsd:integer [0..1] ;
    schema:email xsd:string [0..*] ;
}
"#;
        let result = parse(input);
        assert!(result.is_ok(), "Parse failed: {:?}", result.err());
        let turtle = result.unwrap();
        assert!(turtle.contains("sh:NodeShape"));
        assert!(turtle.contains("sh:targetClass"));
        assert!(turtle.contains("sh:path schema:name"));
        assert!(turtle.contains("sh:minCount 1"));
        assert!(turtle.contains("sh:maxCount 1"));
    }

    #[test]
    fn test_closed_shape() {
        let input = r#"
PREFIX ex: <http://example.org/>

shape ex:ClosedShape -> ex:Thing closed {
    ex:name xsd:string [1..1] ;
}
"#;
        let result = parse(input);
        assert!(result.is_ok());
        let turtle = result.unwrap();
        assert!(turtle.contains("sh:closed true"));
    }

    #[test]
    fn test_message() {
        let input = r#"
PREFIX ex: <http://example.org/>

shape ex:TestShape -> ex:Thing {
    ex:name xsd:string [1..1] // "Name is required" ;
}
"#;
        let result = parse(input);
        assert!(result.is_ok());
        let turtle = result.unwrap();
        assert!(turtle.contains("sh:message \"Name is required\""));
    }
}
