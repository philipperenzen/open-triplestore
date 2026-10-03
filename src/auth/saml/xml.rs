//! A small, strict, namespace-aware XML reader for SAML messages.
//!
//! samael parses responses itself, but some steps happen before or around it:
//! refusing a DOCTYPE, finding and replacing an `EncryptedAssertion`, reading
//! logout messages and IdP metadata. This reader builds a tree of elements
//! with their namespace, attributes, text and byte span in the source, and
//! refuses anything a SAML message never needs: a DOCTYPE (entity expansion),
//! very deep nesting or very many elements.

use quick_xml::events::Event;
use quick_xml::name::ResolveResult;
use quick_xml::NsReader;

pub const NS_PROTOCOL: &str = "urn:oasis:names:tc:SAML:2.0:protocol";
pub const NS_ASSERTION: &str = "urn:oasis:names:tc:SAML:2.0:assertion";
pub const NS_METADATA: &str = "urn:oasis:names:tc:SAML:2.0:metadata";
pub const NS_DSIG: &str = "http://www.w3.org/2000/09/xmldsig#";
pub const NS_XENC: &str = "http://www.w3.org/2001/04/xmlenc#";
pub const NS_XENC11: &str = "http://www.w3.org/2009/xmlenc11#";

const MAX_DEPTH: usize = 64;
const MAX_ELEMENTS: usize = 20_000;

/// Largest SAML message (decoded XML) the store accepts.
pub const MAX_MESSAGE_BYTES: usize = 1 << 20;

#[derive(Debug, Clone, Default)]
pub struct XmlEl {
    pub ns: String,
    pub local: String,
    /// Attributes by local name (namespace declarations excluded), values
    /// unescaped.
    pub attrs: Vec<(String, String)>,
    /// Concatenated character data directly inside this element, unescaped.
    pub text: String,
    pub children: Vec<XmlEl>,
    /// Byte range of the whole element in the parsed source.
    pub span: (usize, usize),
}

impl XmlEl {
    pub fn is(&self, ns: &str, local: &str) -> bool {
        self.ns == ns && self.local == local
    }

    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    pub fn child(&self, ns: &str, local: &str) -> Option<&XmlEl> {
        self.children.iter().find(|c| c.is(ns, local))
    }

    pub fn children_named<'a>(
        &'a self,
        ns: &'a str,
        local: &'a str,
    ) -> impl Iterator<Item = &'a XmlEl> + 'a {
        self.children.iter().filter(move |c| c.is(ns, local))
    }

    /// Depth-first search for the first descendant (or self) with this name.
    pub fn find(&self, ns: &str, local: &str) -> Option<&XmlEl> {
        if self.is(ns, local) {
            return Some(self);
        }
        self.children.iter().find_map(|c| c.find(ns, local))
    }

    /// Every descendant (or self) with this name, in document order.
    pub fn find_all<'a>(&'a self, ns: &str, local: &str, out: &mut Vec<&'a XmlEl>) {
        if self.is(ns, local) {
            out.push(self);
        }
        for c in &self.children {
            c.find_all(ns, local, out);
        }
    }

    pub fn trimmed_text(&self) -> &str {
        self.text.trim()
    }
}

/// Refuse a document type declaration or entity declaration anywhere in the
/// text, before any XML parser (ours or libxml's) sees it.
pub fn refuse_doctype(xml: &str) -> anyhow::Result<()> {
    let lower = xml.to_ascii_lowercase();
    if lower.contains("<!doctype") || lower.contains("<!entity") {
        anyhow::bail!("SAML message carries a DOCTYPE, which is refused");
    }
    Ok(())
}

/// Parse `xml` and return its root element.
pub fn parse(xml: &str) -> anyhow::Result<XmlEl> {
    if xml.len() > MAX_MESSAGE_BYTES {
        anyhow::bail!("SAML message is larger than {MAX_MESSAGE_BYTES} bytes");
    }
    refuse_doctype(xml)?;
    let mut reader = NsReader::from_str(xml);
    let mut stack: Vec<XmlEl> = Vec::new();
    let mut root: Option<XmlEl> = None;
    let mut count = 0usize;

    loop {
        let before = reader.buffer_position() as usize;
        let (ns, event) = reader.read_resolved_event()?;
        let ns = match ns {
            ResolveResult::Bound(n) => n.into_inner().to_string(),
            ResolveResult::Unbound => String::new(),
            ResolveResult::Unknown(p) => anyhow::bail!("undeclared XML namespace prefix `{p}`"),
        };
        match event {
            Event::Start(_) | Event::Empty(_) if root.is_some() && stack.is_empty() => {
                anyhow::bail!("content after the root element")
            }
            Event::Start(e) => {
                count += 1;
                if count > MAX_ELEMENTS || stack.len() >= MAX_DEPTH {
                    anyhow::bail!("SAML message is too deeply nested or too large");
                }
                let mut el = element(&e, ns)?;
                el.span.0 = before;
                stack.push(el);
            }
            Event::Empty(e) => {
                count += 1;
                if count > MAX_ELEMENTS || stack.len() >= MAX_DEPTH {
                    anyhow::bail!("SAML message is too deeply nested or too large");
                }
                let mut el = element(&e, ns)?;
                el.span = (before, reader.buffer_position() as usize);
                match stack.last_mut() {
                    Some(parent) => parent.children.push(el),
                    None => root = Some(el),
                }
            }
            Event::End(_) => {
                let mut el = stack
                    .pop()
                    .ok_or_else(|| anyhow::anyhow!("unbalanced XML end tag"))?;
                el.span.1 = reader.buffer_position() as usize;
                match stack.last_mut() {
                    Some(parent) => parent.children.push(el),
                    None => root = Some(el),
                }
            }
            Event::Text(t) => {
                if let Some(cur) = stack.last_mut() {
                    cur.text.push_str(&t.into_inner());
                } else if !t.into_inner().trim().is_empty() {
                    anyhow::bail!("text outside the root element");
                }
            }
            Event::CData(c) => {
                if let Some(cur) = stack.last_mut() {
                    cur.text.push_str(&c.into_inner());
                }
            }
            Event::GeneralRef(r) => {
                let resolved = if r.is_char_ref() {
                    r.resolve_char_ref()?
                        .map(String::from)
                        .ok_or_else(|| anyhow::anyhow!("bad character reference"))?
                } else {
                    let name = r.into_inner();
                    quick_xml::escape::resolve_predefined_entity(&name)
                        .map(str::to_string)
                        .ok_or_else(|| anyhow::anyhow!("undefined XML entity `{name}`"))?
                };
                if let Some(cur) = stack.last_mut() {
                    cur.text.push_str(&resolved);
                }
            }
            Event::DocType(_) => anyhow::bail!("SAML message carries a DOCTYPE, which is refused"),
            Event::Decl(_) if root.is_none() && stack.is_empty() => {}
            Event::Decl(_) => anyhow::bail!("misplaced XML declaration"),
            Event::Comment(_) | Event::PI(_) => {}
            Event::Eof => break,
        }
    }
    if !stack.is_empty() {
        anyhow::bail!("unclosed XML element");
    }
    root.ok_or_else(|| anyhow::anyhow!("empty XML document"))
}

fn element(e: &quick_xml::events::BytesStart<'_>, ns: String) -> anyhow::Result<XmlEl> {
    let local = e.local_name().as_ref().to_string();
    let mut attrs = Vec::new();
    for a in e.attributes() {
        let a = a?;
        let key: &str = a.key.as_ref();
        if key == "xmlns" || key.starts_with("xmlns:") {
            continue;
        }
        let local_key = key.rsplit(':').next().unwrap_or(key).to_string();
        attrs.push((
            local_key,
            a.normalized_value(quick_xml::XmlVersion::default())?
                .into_owned(),
        ));
    }
    Ok(XmlEl {
        ns,
        local,
        attrs,
        ..Default::default()
    })
}

/// Escape text for an XML attribute value or element content.
pub fn esc(s: &str) -> String {
    quick_xml::escape::escape(s).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spans_cover_whole_elements_and_text_is_unescaped() {
        let xml = r#"<?xml version="1.0"?><p:R xmlns:p="urn:oasis:names:tc:SAML:2.0:protocol" xmlns:a="urn:oasis:names:tc:SAML:2.0:assertion"><a:Issuer>x &amp; y</a:Issuer><a:E Id="1"><a:In/></a:E></p:R>"#;
        let root = parse(xml).unwrap();
        assert!(root.is(NS_PROTOCOL, "R"));
        let issuer = root.child(NS_ASSERTION, "Issuer").unwrap();
        assert_eq!(issuer.text, "x & y");
        let e = root.child(NS_ASSERTION, "E").unwrap();
        assert_eq!(&xml[e.span.0..e.span.1], r#"<a:E Id="1"><a:In/></a:E>"#);
        assert_eq!(e.attr("Id"), Some("1"));
    }

    #[test]
    fn doctype_and_unknown_entities_are_refused() {
        assert!(parse(r#"<!DOCTYPE r [<!ENTITY x "y">]><r>&x;</r>"#).is_err());
        assert!(parse("<r>&nbsp;</r>").is_err());
        assert!(parse("<p:r/>").is_err(), "undeclared prefix");
        assert!(parse("<r/><r/>").is_err(), "two roots");
    }
}
