//! XPath 3.1 regular expressions (`fn:matches`, as ShEx's `pattern` facet
//! uses them) on top of the `regex` crate.
//!
//! The dialects mostly agree; the translation covers where they do not:
//! XPath's `.` excludes `\r` as well as `\n`; `\i` / `\c` (XML name
//! characters) have no `regex` equivalent; character-class subtraction is
//! `[a-z-[aeiou]]` in XPath and `[a-z--[aeiou]]` in `regex`; the `x` flag
//! removes whitespace (outside classes) and nothing else, where `regex`'s
//! also enables `#` comments; `q` matches the pattern literally. A pattern
//! `regex` cannot express (back-references) is an error, so it matches
//! nothing instead of something else.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use regex::{Regex, RegexBuilder};

const NAME_START: &str = r":A-Z_a-z\u{C0}-\u{D6}\u{D8}-\u{F6}\u{F8}-\u{2FF}\u{370}-\u{37D}\u{37F}-\u{1FFF}\u{200C}-\u{200D}\u{2070}-\u{218F}\u{2C00}-\u{2FEF}\u{3001}-\u{D7FF}\u{F900}-\u{FDCF}\u{FDF0}-\u{FFFD}\u{10000}-\u{EFFFF}";
const NAME_EXTRA: &str = r"\-.0-9\u{B7}\u{300}-\u{36F}\u{203F}-\u{2040}";

/// Compiled patterns by `(pattern, flags)`.
type Cache = HashMap<(String, String), Result<Rc<Regex>, String>>;

thread_local! {
    static CACHE: RefCell<Cache> = RefCell::new(HashMap::new());
}

/// Compile (cached per thread) an XPath pattern with its flags.
pub fn compile(pattern: &str, flags: Option<&str>) -> Result<Rc<Regex>, String> {
    let flags = flags.unwrap_or("");
    let key = (pattern.to_string(), flags.to_string());
    if let Some(hit) = CACHE.with(|c| c.borrow().get(&key).cloned()) {
        return hit;
    }
    let out = build(pattern, flags).map(Rc::new);
    CACHE.with(|c| c.borrow_mut().insert(key, out.clone()));
    out
}

fn build(pattern: &str, flags: &str) -> Result<Regex, String> {
    let mut b_flags = (false, false, false, false, false);
    for f in flags.chars() {
        match f {
            's' => b_flags.0 = true,
            'm' => b_flags.1 = true,
            'i' => b_flags.2 = true,
            'x' => b_flags.3 = true,
            'q' => b_flags.4 = true,
            other => return Err(format!("unknown regular-expression flag '{other}'")),
        }
    }
    let (dot_all, multi, ci, extended, literal) = b_flags;
    let translated = if literal {
        regex::escape(pattern)
    } else {
        let src = if extended {
            strip_whitespace(pattern)
        } else {
            pattern.to_string()
        };
        translate(&src, dot_all)?
    };
    RegexBuilder::new(&translated)
        .case_insensitive(ci)
        .multi_line(multi)
        .dot_matches_new_line(dot_all)
        .build()
        .map_err(|e| format!("pattern /{pattern}/ is not a usable regular expression: {e}"))
}

/// The `x` flag: whitespace outside character classes is removed.
fn strip_whitespace(p: &str) -> String {
    let mut out = String::new();
    let mut depth = 0usize;
    let mut chars = p.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                out.push(c);
                if let Some(n) = chars.next() {
                    out.push(n);
                }
            }
            '[' => {
                depth += 1;
                out.push(c);
            }
            ']' if depth > 0 => {
                depth -= 1;
                out.push(c);
            }
            c if depth == 0 && matches!(c, ' ' | '\t' | '\n' | '\r') => {}
            c => out.push(c),
        }
    }
    out
}

fn translate(p: &str, dot_all: bool) -> Result<String, String> {
    let chars: Vec<char> = p.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    let mut class_depth = 0usize;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '\\' => {
                let Some(&n) = chars.get(i + 1) else {
                    return Err("a pattern cannot end with '\\'".into());
                };
                match n {
                    'i' => out.push_str(&class(NAME_START, false, class_depth > 0)),
                    'I' => out.push_str(&class(NAME_START, true, class_depth > 0)),
                    'c' => out.push_str(&class(
                        &format!("{NAME_START}{NAME_EXTRA}"),
                        false,
                        class_depth > 0,
                    )),
                    'C' => out.push_str(&class(
                        &format!("{NAME_START}{NAME_EXTRA}"),
                        true,
                        class_depth > 0,
                    )),
                    '1'..='9' => return Err("back-references are not supported".into()),
                    'p' | 'P' if chars.get(i + 2) == Some(&'{') => {
                        // \p{IsBlock} names Unicode blocks, which `regex`
                        // spells differently; categories pass through.
                        let end = chars[i..].iter().position(|&x| x == '}').map(|e| e + i);
                        let Some(end) = end else {
                            return Err("unterminated \\p{…}".into());
                        };
                        let name: String = chars[i + 3..end].iter().collect();
                        if let Some(block) = name.strip_prefix("Is") {
                            out.push_str(&format!("\\{n}{{Block={block}}}"));
                        } else {
                            out.push_str(&format!("\\{n}{{{name}}}"));
                        }
                        i = end + 1;
                        continue;
                    }
                    _ => {
                        out.push('\\');
                        out.push(n);
                    }
                }
                i += 2;
                continue;
            }
            '[' => {
                class_depth += 1;
                out.push('[');
                if chars.get(i + 1) == Some(&'^') {
                    out.push('^');
                    i += 1;
                }
                // A leading ']' or '-' is literal.
                if let Some(&n) = chars.get(i + 1) {
                    if n == '-' {
                        out.push_str("\\-");
                        i += 1;
                    }
                }
            }
            ']' if class_depth > 0 => {
                class_depth -= 1;
                out.push(']');
            }
            '-' if class_depth > 0 && chars.get(i + 1) == Some(&'[') => {
                // Subtraction: `[a-z-[aeiou]]` → `[a-z--[aeiou]]`.
                out.push_str("--");
            }
            '-' if class_depth > 0 && chars.get(i + 1) == Some(&']') => out.push_str("\\-"),
            '.' if class_depth == 0 && !dot_all => out.push_str("[^\\n\\r]"),
            '&' | '~' if class_depth > 0 => {
                // `regex` treats `&&` / `~~` in classes as operators.
                out.push('\\');
                out.push(c);
            }
            c => out.push(c),
        }
        i += 1;
    }
    Ok(out)
}

fn class(body: &str, negated: bool, inside: bool) -> String {
    if inside && !negated {
        body.to_string()
    } else {
        format!("[{}{body}]", if negated { "^" } else { "" })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(p: &str, f: Option<&str>, s: &str) -> bool {
        compile(p, f).unwrap().is_match(s)
    }

    #[test]
    fn dot_excludes_carriage_return() {
        assert!(!m("^a.b$", None, "a\rb"));
        assert!(m("^a.b$", Some("s"), "a\rb"));
    }

    #[test]
    fn class_subtraction() {
        assert!(m("^[a-z-[aeiou]]+$", None, "xyz"));
        assert!(!m("^[a-z-[aeiou]]+$", None, "xaz"));
    }

    #[test]
    fn x_flag_strips_whitespace_only() {
        assert!(m("a b # c", Some("x"), "ab#c"));
        assert!(m("[ ]", Some("x"), " "));
    }

    #[test]
    fn name_classes_and_literal_flag() {
        assert!(m("^\\i\\c*$", None, "_a-1"));
        assert!(!m("^\\i", None, "1"));
        assert!(m("a.b", Some("q"), "xa.by"));
        assert!(!m("a.b", Some("q"), "axb"));
    }

    #[test]
    fn back_references_are_refused() {
        assert!(compile("(a)\\1", None).is_err());
    }
}
