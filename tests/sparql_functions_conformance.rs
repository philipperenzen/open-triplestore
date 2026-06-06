//! SPARQL 1.1 **function library** conformance (high-coverage, assertion-based).
//!
//! Grounded in *SPARQL 1.1 Query Language* §17.4 (Function Definitions). Each
//! builtin is exercised with a concrete input and an exact (or, where the
//! lexical form of a numeric datatype is implementation-flavoured,
//! value-equality) assertion. Scalar functions are evaluated with the
//! `SELECT ((expr) AS ?r) WHERE {}` idiom; aggregates run over a tiny dataset.
//!
//! Categories: string · numeric · date/time · hash · term/type · constructors &
//! casts · logical/conditional · aggregates · non-deterministic (shape checks).

use open_triplestore::store::TripleStore;
use oxigraph::model::Term;
use oxigraph::sparql::QueryResults;

/// Prefixes available to every evaluated expression.
const EPFX: &str = "PREFIX xsd: <http://www.w3.org/2001/XMLSchema#> \
PREFIX ex: <http://example.org/> \
PREFIX rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> ";

fn term_lex(t: &Term) -> String {
    match t {
        Term::NamedNode(n) => n.as_str().to_string(),
        Term::Literal(l) => l.value().to_string(),
        Term::BlankNode(b) => format!("_:{}", b.as_str()),
        Term::Triple(tr) => tr.to_string(),
    }
}

/// Evaluate `expr`, returning the **lexical value** of the bound result.
fn eval(expr: &str) -> String {
    let store = TripleStore::in_memory().unwrap();
    let q = format!("{EPFX} SELECT (({expr}) AS ?r) WHERE {{}}");
    match store.query(&q) {
        Ok(QueryResults::Solutions(s)) => s
            .filter_map(|r| r.ok())
            .next()
            .and_then(|sol| sol.get("r").map(term_lex))
            .unwrap_or_default(),
        _ => String::new(),
    }
}

/// Evaluate `expr`, returning the full term Display (`"v"^^<dt>`, `<iri>`, …).
fn eval_term(expr: &str) -> String {
    let store = TripleStore::in_memory().unwrap();
    let q = format!("{EPFX} SELECT (({expr}) AS ?r) WHERE {{}}");
    match store.query(&q) {
        Ok(QueryResults::Solutions(s)) => s
            .filter_map(|r| r.ok())
            .next()
            .and_then(|sol| sol.get("r").map(|t| t.to_string()))
            .unwrap_or_default(),
        _ => String::new(),
    }
}

/// Numeric value of an evaluated expression (datatype-lexical agnostic).
fn evalf(expr: &str) -> f64 {
    eval(expr).parse::<f64>().unwrap_or(f64::NAN)
}

/// Run an aggregate `select` (e.g. `(SUM(?v) AS ?r)`) over `?s ex:v ?v`.
fn agg(data: &str, select: &str) -> String {
    let store = TripleStore::in_memory().unwrap();
    store
        .load_str(
            &format!("@prefix ex: <http://example.org/> .\n{data}"),
            oxigraph::io::RdfFormat::Turtle,
            None,
        )
        .unwrap();
    let q = format!("{EPFX} SELECT {select} WHERE {{ ?s ex:v ?v }}");
    match store.query(&q) {
        Ok(QueryResults::Solutions(s)) => s
            .filter_map(|r| r.ok())
            .next()
            .and_then(|sol| sol.get("r").map(term_lex))
            .unwrap_or_default(),
        _ => String::new(),
    }
}

// ───────────────────────────── String functions ─────────────────────────────

#[test]
fn fn_string() {
    assert_eq!(eval(r#"STRLEN("hello")"#), "5");
    assert_eq!(eval(r#"SUBSTR("foobar", 4)"#), "bar");
    assert_eq!(eval(r#"SUBSTR("foobar", 4, 2)"#), "ba");
    assert_eq!(eval(r#"UCASE("aBc")"#), "ABC");
    assert_eq!(eval(r#"LCASE("aBc")"#), "abc");
    assert_eq!(eval(r#"CONCAT("a", "b", "c")"#), "abc");
    assert_eq!(eval(r#"STRBEFORE("abc", "b")"#), "a");
    assert_eq!(eval(r#"STRAFTER("abc", "b")"#), "c");
    assert_eq!(eval(r#"REPLACE("foobar", "o", "0")"#), "f00bar");
    assert_eq!(eval(r#"REPLACE("FooBar", "o+", "0", "i")"#), "F0Bar");
    assert_eq!(eval(r#"ENCODE_FOR_URI("a b/c")"#), "a%20b%2Fc");
    // Boolean string predicates
    assert_eq!(eval(r#"STRSTARTS("foobar", "foo")"#), "true");
    assert_eq!(eval(r#"STRENDS("foobar", "bar")"#), "true");
    assert_eq!(eval(r#"CONTAINS("foobar", "oba")"#), "true");
    assert_eq!(eval(r#"REGEX("Foobar", "foo", "i")"#), "true");
    assert_eq!(eval(r#"REGEX("foobar", "^foo")"#), "true");
    assert_eq!(eval(r#"REGEX("foobar", "^bar")"#), "false");
}

// ───────────────────────────── Numeric functions ─────────────────────────────

#[test]
fn fn_numeric() {
    assert_eq!(eval("ABS(-5)"), "5");
    assert_eq!(evalf("ROUND(2.5)"), 3.0);
    assert_eq!(evalf("ROUND(2.4)"), 2.0);
    assert_eq!(evalf("CEIL(2.1)"), 3.0);
    assert_eq!(evalf("FLOOR(2.9)"), 2.0);
    assert_eq!(eval("2 + 3 * 4"), "14"); // precedence
    assert_eq!(evalf("10 / 4"), 2.5); // int/int ⇒ decimal
    assert_eq!(eval("7 - 12"), "-5");
    // RAND ∈ [0, 1)
    let r = evalf("RAND()");
    assert!((0.0..1.0).contains(&r), "RAND() out of range: {r}");
}

// ───────────────────────────── Date/Time functions ─────────────────────────────

#[test]
fn fn_datetime() {
    let dt = r#""2020-01-02T03:04:05Z"^^xsd:dateTime"#;
    assert_eq!(eval(&format!("YEAR({dt})")), "2020");
    assert_eq!(eval(&format!("MONTH({dt})")), "1");
    assert_eq!(eval(&format!("DAY({dt})")), "2");
    assert_eq!(eval(&format!("HOURS({dt})")), "3");
    assert_eq!(eval(&format!("MINUTES({dt})")), "4");
    assert_eq!(evalf(&format!("SECONDS({dt})")), 5.0);
    assert_eq!(eval(&format!("TZ({dt})")), "Z");
    assert_eq!(eval(&format!("TIMEZONE({dt})")), "PT0S");
    // NOW() is an xsd:dateTime
    assert!(
        eval_term("NOW()").contains("dateTime"),
        "NOW() must be xsd:dateTime: {}",
        eval_term("NOW()"),
    );
}

// ───────────────────────────── Hash functions ─────────────────────────────

#[test]
fn fn_hash() {
    // Known vectors for the input "abc".
    assert_eq!(eval(r#"MD5("abc")"#), "900150983cd24fb0d6963f7d28e17f72");
    assert_eq!(
        eval(r#"SHA1("abc")"#),
        "a9993e364706816aba3e25717850c26c9cd0d89d"
    );
    assert_eq!(
        eval(r#"SHA256("abc")"#),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(
        eval(r#"SHA512("abc")"#),
        "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f"
    );
}

// ───────────────────────────── Term / type functions ─────────────────────────────

#[test]
fn fn_term_and_type() {
    assert_eq!(eval(r#"STR(42)"#), "42");
    assert_eq!(
        eval(r#"STR(<http://example.org/x>)"#),
        "http://example.org/x"
    );
    assert_eq!(eval(r#"LANG("chat"@fr)"#), "fr");
    assert_eq!(eval(r#"LANG("plain")"#), "");
    assert!(eval(r#"DATATYPE("plain")"#).ends_with("#string"));
    assert!(eval(r#"DATATYPE(42)"#).ends_with("#integer"));
    assert!(eval(r#"DATATYPE("chat"@fr)"#).ends_with("#langString"));
    assert_eq!(
        eval(r#"IRI("http://example.org/z")"#),
        "http://example.org/z"
    );
    assert_eq!(
        eval_term(r#"STRDT("3", xsd:integer)"#),
        r#""3"^^<http://www.w3.org/2001/XMLSchema#integer>"#
    );
    assert_eq!(eval_term(r#"STRLANG("chat", "fr")"#), r#""chat"@fr"#);
    // Type predicates
    assert_eq!(eval(r#"isIRI(<http://example.org/x>)"#), "true");
    assert_eq!(eval(r#"isLITERAL("x")"#), "true");
    assert_eq!(eval(r#"isBLANK(<http://example.org/x>)"#), "false");
    assert_eq!(eval(r#"isNUMERIC(42)"#), "true");
    assert_eq!(eval(r#"isNUMERIC("x")"#), "false");
    assert_eq!(eval(r#"sameTerm("a"@en, "a"@en)"#), "true");
    assert_eq!(eval(r#"sameTerm("a", "a"@en)"#), "false");
    // BNODE() yields a fresh blank node
    assert!(eval("BNODE()").starts_with("_:"));
}

// ───────────────────────────── Constructors / casts ─────────────────────────────

#[test]
fn fn_constructors_and_casts() {
    assert_eq!(eval(r#"xsd:integer("42")"#), "42");
    assert_eq!(evalf(r#"xsd:double("1.5")"#), 1.5);
    assert_eq!(evalf(r#"xsd:decimal("2.50")"#), 2.5);
    assert_eq!(eval(r#"xsd:string(42)"#), "42");
    assert_eq!(eval(r#"xsd:boolean("true")"#), "true");
    assert_eq!(eval(r#"xsd:boolean("0")"#), "false");
    assert!(eval_term(r#"xsd:integer("42")"#).ends_with(r#"#integer>"#));
    assert!(
        eval_term(r#"xsd:string(42)"#).ends_with(r#"#string>"#)
            || eval_term(r#"xsd:string(42)"#) == r#""42""#
    );
}

// ───────────────────────────── Logical / conditional ─────────────────────────────

#[test]
fn fn_logical_conditional() {
    assert_eq!(eval(r#"IF(3 > 2, "yes", "no")"#), "yes");
    assert_eq!(eval(r#"IF(1 > 2, "yes", "no")"#), "no");
    assert_eq!(eval(r#"COALESCE(?unbound, "fallback")"#), "fallback");
    assert_eq!(eval(r#"COALESCE("first", "second")"#), "first");
    assert_eq!(eval(r#"BOUND(?x)"#), "false");
    assert_eq!(eval(r#"42 IN (1, 2, 42)"#), "true");
    assert_eq!(eval(r#"42 NOT IN (1, 2, 3)"#), "true");
    assert_eq!(eval(r#"(3 > 2) && (1 < 2)"#), "true");
    assert_eq!(eval(r#"(3 < 2) || (1 < 2)"#), "true");
    assert_eq!(eval(r#"!(3 < 2)"#), "true");
    // Three-valued logic: error || true ⇒ true
    assert_eq!(eval(r#"(1/0 = 1) || (2 > 1)"#), "true");
}

// ───────────────────────────── Aggregates ─────────────────────────────

#[test]
fn fn_aggregates() {
    let data = "ex:a ex:v 1 . ex:b ex:v 2 . ex:c ex:v 3 .";
    assert_eq!(agg(data, "(COUNT(?v) AS ?r)"), "3");
    assert_eq!(agg(data, "(SUM(?v) AS ?r)"), "6");
    assert_eq!(agg(data, "(AVG(?v) AS ?r)").parse::<f64>().unwrap(), 2.0);
    assert_eq!(agg(data, "(MIN(?v) AS ?r)"), "1");
    assert_eq!(agg(data, "(MAX(?v) AS ?r)"), "3");
    // SAMPLE returns one of the values
    let s = agg(data, "(SAMPLE(?v) AS ?r)");
    assert!(["1", "2", "3"].contains(&s.as_str()), "SAMPLE: {s}");
    // GROUP_CONCAT with an explicit separator — STR() the numeric values (GROUP_CONCAT
    // concatenates lexical forms) and assert separator-agnostically since group order
    // is unspecified.
    let gc = agg(data, r#"(GROUP_CONCAT(STR(?v); SEPARATOR=",") AS ?r)"#);
    assert!(
        gc.contains('1') && gc.contains('2') && gc.contains('3') && gc.contains(','),
        "GROUP_CONCAT must join all values with the separator: {gc}"
    );
    // COUNT(DISTINCT …) collapses duplicates
    let dup = "ex:a ex:v 1 . ex:b ex:v 1 . ex:c ex:v 2 .";
    assert_eq!(agg(dup, "(COUNT(DISTINCT ?v) AS ?r)"), "2");
    assert_eq!(agg(dup, "(COUNT(?v) AS ?r)"), "3");
}

// ───────────────────────────── Non-deterministic (shape) ─────────────────────────────

#[test]
fn fn_nondeterministic_shapes() {
    // UUID() ⇒ a urn:uuid: IRI
    assert!(
        eval("UUID()").starts_with("urn:uuid:"),
        "UUID(): {}",
        eval("UUID()")
    );
    // STRUUID() ⇒ a 36-char hyphenated string
    let s = eval("STRUUID()");
    assert_eq!(s.len(), 36, "STRUUID len: {s}");
    assert_eq!(s.matches('-').count(), 4, "STRUUID hyphens: {s}");
    // Two UUIDs differ
    assert_ne!(eval("STRUUID()"), eval("STRUUID()"));
}
