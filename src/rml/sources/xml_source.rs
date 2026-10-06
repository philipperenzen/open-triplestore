//! XML source for RML (RML-IO registry §XPath).
//!
//! The document is parsed whole, and the iterator and every reference are
//! XPath 1.0 expressions over it: every node the iterator selects is one
//! logical iteration and the context node of the references evaluated on it,
//! so `@id`, `../@id`, `name/text()` and `ex:name` (with the namespaces the
//! reference formulation declares) all work. A node-set reference has one
//! value per node, its string value, in document order; an expression that
//! returns a string, number or boolean has that one value.
//!
//! The RML-IO registry asks for XPath 3.1. This engine evaluates XPath 1.0,
//! which every path, predicate and axis the RML test cases use belongs to;
//! the 2.0 and 3.1 additions (sequences, `if`, `for`, typed comparisons) are
//! refused as invalid expressions rather than misread.
//!
//! A relative iterator (`student`, written by mappings of the legacy RML
//! vocabulary, whose old engine matched elements anywhere by name) is read
//! as `//student`.

use std::collections::HashMap;

use sxd_xpath::{Context, Factory, Value, XPath};

use super::ReadAs;
use crate::rml::terms::{Iteration, Val};

fn compile(factory: &Factory, expr: &str, what: &str) -> Result<XPath, String> {
    match factory.build(expr) {
        Ok(Some(x)) => Ok(x),
        Ok(None) => Err(format!("the XPath {what} \"{expr}\" is empty")),
        Err(e) => Err(format!("the XPath {what} \"{expr}\" is invalid: {e}")),
    }
}

/// An XPath number as RML's natural lexical form: an integer without a
/// fraction, anything else as XPath writes it.
fn number(n: f64) -> String {
    if n.is_finite() && n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{}", n as i64)
    } else {
        n.to_string()
    }
}

/// The logical iterations of an XML source.
///
/// The XPath engine panics on some expressions it cannot evaluate — a
/// namespace prefix the reference formulation does not declare is one — so a
/// panic is caught here and reported as the mapping error it is, rather than
/// taking the request down.
pub fn load(
    text: &str,
    iterator: Option<&str>,
    references: &[String],
    namespaces: &[(String, String)],
    read_as: ReadAs,
) -> Result<Vec<Iteration>, String> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        load_inner(text, iterator, references, namespaces, read_as)
    }))
    .unwrap_or_else(|panic| {
        let why = panic
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| panic.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "the XPath engine failed".to_string());
        Err(format!(
            "an XPath iterator or reference could not be evaluated: {why} (is every \
             namespace prefix declared with rml:namespace?)"
        ))
    })
}

fn load_inner(
    text: &str,
    iterator: Option<&str>,
    references: &[String],
    namespaces: &[(String, String)],
    read_as: ReadAs,
) -> Result<Vec<Iteration>, String> {
    let package = sxd_document::parser::parse(text).map_err(|e| format!("XML parse error: {e}"))?;
    let document = package.as_document();
    let factory = Factory::new();
    let mut context = Context::new();
    for (prefix, uri) in namespaces {
        context.set_namespace(prefix, uri);
    }

    let iterator_expr = match iterator.map(str::trim).filter(|i| !i.is_empty()) {
        None => "/".to_string(),
        Some(i) if i.starts_with('/') || i.starts_with('(') => i.to_string(),
        Some(i) if !read_as.rml_core => format!("//{i}"),
        Some(i) => i.to_string(),
    };
    let iterator_path = compile(&factory, &iterator_expr, "iterator")?;
    let mut compiled: Vec<(&String, XPath)> = Vec::with_capacity(references.len());
    for r in references {
        compiled.push((r, compile(&factory, r, "reference")?));
    }

    let nodes = match iterator_path
        .evaluate(&context, document.root())
        .map_err(|e| format!("evaluating the XPath iterator \"{iterator_expr}\": {e}"))?
    {
        Value::Nodeset(ns) => ns.document_order(),
        other => {
            return Err(format!(
                "the XPath iterator \"{iterator_expr}\" selects {other:?}, not nodes"
            ))
        }
    };

    let mut out = Vec::with_capacity(nodes.len());
    for node in nodes {
        let mut values: HashMap<String, Vec<Val>> = HashMap::with_capacity(compiled.len());
        for (expr, path) in &compiled {
            let result = path
                .evaluate(&context, node)
                .map_err(|e| format!("evaluating the XPath reference \"{expr}\": {e}"))?;
            let vals: Vec<Val> = match result {
                Value::Nodeset(ns) => ns
                    .document_order()
                    .into_iter()
                    .map(|n| n.string_value())
                    // RML-IO: XML has no NULL, so an empty element is the
                    // value "". A legacy version read it as no value.
                    .filter(|v| read_as.empty_is_value || !v.is_empty())
                    .map(Val::text)
                    .collect(),
                Value::String(s) => vec![Val::text(s)],
                Value::Number(n) => vec![Val::text(number(n))],
                Value::Boolean(b) => vec![Val::text(b.to_string())],
            };
            if !vals.is_empty() {
                values.insert((*expr).clone(), vals);
            }
        }
        out.push(Iteration { values });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rml::terms::Record;

    const CORE: ReadAs = ReadAs {
        rml_core: true,
        empty_is_value: true,
    };

    fn refs(r: &[&str]) -> Vec<String> {
        r.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn attributes_parents_and_several_values() {
        let xml = r#"<companies><company id="c1"><dept id="d1"><e><name>Ann</name><skill>a</skill><skill>b</skill></e></dept></company></companies>"#;
        let its = load(
            xml,
            Some("/companies/company/dept/e"),
            &refs(&[
                "name",
                "skill",
                "../@id",
                "../../@id",
                "missing",
                "count(skill)",
            ]),
            &[],
            CORE,
        )
        .unwrap();
        assert_eq!(its.len(), 1);
        let it = &its[0];
        assert_eq!(it.values("name").first(), Some("Ann"));
        assert_eq!(it.values("skill").len(), 2);
        assert_eq!(it.values("../@id").first(), Some("d1"));
        assert_eq!(it.values("../../@id").first(), Some("c1"));
        assert!(it.values("missing").is_empty());
        assert_eq!(it.values("count(skill)").first(), Some("2"));
    }

    #[test]
    fn namespaces_are_declared_by_the_reference_formulation() {
        let xml = r#"<f xmlns:x="http://example.org/"><x:c><x:name>Ross</x:name></x:c></f>"#;
        let ns = vec![("ex".to_string(), "http://example.org/".to_string())];
        let its = load(xml, Some("//ex:c"), &refs(&["ex:name/text()"]), &ns, CORE).unwrap();
        assert_eq!(its[0].values("ex:name/text()").first(), Some("Ross"));
        let err = load(xml, Some("//ex:c"), &refs(&["ex:name"]), &[], CORE).unwrap_err();
        assert!(
            err.contains("namespace prefix"),
            "an undeclared prefix is an error: {err}"
        );
    }

    #[test]
    fn an_empty_element_is_a_value_unless_legacy() {
        let xml = "<r><i><a/></i></r>";
        let its = load(xml, Some("/r/i"), &refs(&["a"]), &[], CORE).unwrap();
        assert_eq!(its[0].values("a").first(), Some(""));
        let legacy = ReadAs {
            rml_core: false,
            empty_is_value: false,
        };
        let its = load(xml, Some("i"), &refs(&["a"]), &[], legacy).unwrap();
        assert_eq!(its.len(), 1, "a relative legacy iterator matches anywhere");
        assert!(its[0].values("a").is_empty());
    }

    #[test]
    fn an_invalid_xpath_is_a_mapping_error() {
        let err = load("<r/>", Some("/r[@"), &[], &[], CORE).unwrap_err();
        assert!(err.contains("invalid"), "{err}");
    }
}
