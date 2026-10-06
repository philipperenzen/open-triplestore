//! Relative IRI reference resolution as [RFC 3986 §5.2](https://www.rfc-editor.org/rfc/rfc3986#section-5.2) defines it.
//!
//! [`Iri::resolve`] removes the dot segments of the reference's own path, but keeps
//! the ones of the base IRI path it merges with (`http://a/b/./c` + `d` gives
//! `http://a/b/./d`) and the ones of a network-path reference (`//g/../h`). RFC 3986
//! §5.2.2 removes the dot segments of the whole target path in both cases
//! (`http://a/b/d`, `http://g/h`), and the JSON-LD algorithms resolve IRIs that way.

use oxiri::{Iri, IriParseError};

/// Resolves `reference` against `base` (RFC 3986 §5.2), validating the result.
pub fn resolve(base: &Iri<String>, reference: &str) -> Result<Iri<String>, IriParseError> {
    let resolved = base.resolve(reference)?;
    Ok(match normalized_path(base, reference, &resolved) {
        Some(path) => Iri::parse(with_path(&resolved, &path))?,
        None => resolved,
    })
}

/// Resolves `reference` against `base` (RFC 3986 §5.2) without validation.
pub fn resolve_unchecked(base: &Iri<String>, reference: &str) -> Iri<String> {
    let resolved = base.resolve_unchecked(reference);
    match normalized_path(base, reference, &resolved) {
        Some(path) => Iri::parse_unchecked(with_path(&resolved, &path)),
        None => resolved,
    }
}

/// The target path RFC 3986 §5.2.2 gives where it differs from the one `Iri::resolve` built.
fn normalized_path(base: &Iri<String>, reference: &str, resolved: &Iri<String>) -> Option<String> {
    if has_scheme(reference) {
        // T.path = remove_dot_segments(R.path), but JSON-LD never resolves an absolute IRI
        // and `Iri::resolve` returns it unchanged: keep it.
        return None;
    }
    let reference_path = reference.split(['?', '#']).next().unwrap_or_default();
    if reference.starts_with("//") {
        // A network-path reference: T.path = remove_dot_segments(R.path), and the resolved
        // path is R.path as written.
        let path = resolved.path();
        return has_dot_segment(path).then(|| remove_dot_segments(path));
    }
    if reference_path.is_empty() || reference_path.starts_with('/') {
        // The base path is used as is (empty path), or the reference path alone, whose dot
        // segments `Iri::resolve` already removes.
        return None;
    }
    // T.path = remove_dot_segments(merge(Base.path, R.path))
    let base_path = base.path();
    if !has_dot_segment(base_path) {
        return None;
    }
    let merged = match base_path.rfind('/') {
        Some(last_slash) => format!("{}{reference_path}", &base_path[..=last_slash]),
        None if base.authority().is_some() => format!("/{reference_path}"),
        None => reference_path.to_owned(),
    };
    Some(remove_dot_segments(&merged))
}

/// `iri` with its path replaced.
fn with_path(iri: &Iri<String>, path: &str) -> String {
    let mut out = String::with_capacity(iri.as_str().len());
    out.push_str(iri.scheme());
    out.push(':');
    if let Some(authority) = iri.authority() {
        out.push_str("//");
        out.push_str(authority);
    }
    out.push_str(path);
    if let Some(query) = iri.query() {
        out.push('?');
        out.push_str(query);
    }
    if let Some(fragment) = iri.fragment() {
        out.push('#');
        out.push_str(fragment);
    }
    out
}

/// Whether `reference` starts with a scheme (RFC 3986 §3.1), i.e. is in absolute form.
pub fn has_scheme(reference: &str) -> bool {
    let mut chars = reference.chars();
    if !chars.next().is_some_and(|c| c.is_ascii_alphabetic()) {
        return false;
    }
    for c in chars {
        if c == ':' {
            return true;
        }
        if !c.is_ascii_alphanumeric() && !matches!(c, '+' | '-' | '.') {
            return false;
        }
    }
    false
}

fn has_dot_segment(path: &str) -> bool {
    path.split('/')
        .any(|segment| segment == "." || segment == "..")
}

/// [RFC 3986 §5.2.4](https://www.rfc-editor.org/rfc/rfc3986#section-5.2.4)
fn remove_dot_segments(path: &str) -> String {
    let mut input = path;
    let mut output = String::with_capacity(path.len());
    while !input.is_empty() {
        if let Some(rest) = input.strip_prefix("../") {
            // A
            input = rest;
        } else if let Some(rest) = input.strip_prefix("./") {
            // A
            input = rest;
        } else if input.starts_with("/./") {
            // B
            input = &input[2..];
        } else if input == "/." {
            // B
            input = "/";
        } else if input.starts_with("/../") || input == "/.." {
            // C
            input = if input == "/.." { "/" } else { &input[3..] };
            match output.rfind('/') {
                Some(last_slash) => output.truncate(last_slash),
                None => output.clear(),
            }
        } else if input == "." || input == ".." {
            // D
            input = "";
        } else {
            // E
            let start = usize::from(input.starts_with('/'));
            let end = input[start..].find('/').map_or(input.len(), |i| i + start);
            output.push_str(&input[..end]);
            input = &input[end..];
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base(iri: &str) -> Iri<String> {
        Iri::parse(iri.to_owned()).unwrap()
    }

    #[test]
    fn remove_dot_segments_rfc_examples() {
        assert_eq!(remove_dot_segments("/a/b/c/./../../g"), "/a/g");
        assert_eq!(remove_dot_segments("mid/content=5/../6"), "mid/6");
        assert_eq!(remove_dot_segments("/b/c/./g"), "/b/c/g");
        assert_eq!(remove_dot_segments("/.."), "/");
        assert_eq!(remove_dot_segments("/b/c/.."), "/b/");
    }

    #[test]
    fn base_path_dot_segments_are_removed_when_merging() {
        let b = base("http://a/bb/ccc/./d;p?q");
        assert_eq!(resolve(&b, "g").unwrap().as_str(), "http://a/bb/ccc/g");
        assert_eq!(resolve(&b, "../g").unwrap().as_str(), "http://a/bb/g");
        assert_eq!(resolve(&b, ".").unwrap().as_str(), "http://a/bb/ccc/");
        // An empty path keeps the base path as written (RFC 3986 §5.2.2).
        assert_eq!(
            resolve(&b, "?y").unwrap().as_str(),
            "http://a/bb/ccc/./d;p?y"
        );
        assert_eq!(resolve(&b, "").unwrap().as_str(), "http://a/bb/ccc/./d;p?q");
        assert_eq!(
            resolve(&b, "#s").unwrap().as_str(),
            "http://a/bb/ccc/./d;p?q#s"
        );
        let b = base("http://a/bb/ccc/../d;p?q");
        assert_eq!(resolve(&b, "g").unwrap().as_str(), "http://a/bb/g");
        assert_eq!(resolve(&b, "../../../g").unwrap().as_str(), "http://a/g");
        assert_eq!(
            resolve_unchecked(&b, "g;x=1/../y").as_str(),
            "http://a/bb/y"
        );
    }

    #[test]
    fn network_path_dot_segments_are_removed() {
        let b = base("http://example.com/some/file");
        assert_eq!(
            resolve(&b, "//example.org/../scheme-relative")
                .unwrap()
                .as_str(),
            "http://example.org/scheme-relative"
        );
        assert_eq!(
            resolve(&b, "//example.org/.././useless/../../scheme-relative?q#f")
                .unwrap()
                .as_str(),
            "http://example.org/scheme-relative?q#f"
        );
    }

    #[test]
    fn plain_cases_are_unchanged() {
        let b = base("http://a/b/c/d;p?q");
        assert_eq!(resolve(&b, "g:h").unwrap().as_str(), "g:h");
        assert_eq!(resolve(&b, "../g").unwrap().as_str(), "http://a/b/g");
        assert_eq!(resolve(&b, "/./g").unwrap().as_str(), "http://a/g");
        assert_eq!(
            resolve(&b, "g?y/./x").unwrap().as_str(),
            "http://a/b/c/g?y/./x"
        );
    }
}
