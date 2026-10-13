//! XML Schema regular expressions (`xs:pattern`) → SPARQL / XPath regular
//! expressions (`sh:pattern`, `REGEX()`).
//!
//! The two dialects differ in ways that change what matches:
//!
//! - an `xs:pattern` always matches the **whole** value, while a SPARQL regex
//!   matches anywhere — the translation anchors it as `^(?:…)$`;
//! - `^` and `$` are ordinary characters in XSD, so they are escaped;
//! - XSD's name-character escapes `\i`, `\I`, `\c`, `\C` have no SPARQL form
//!   and become the character classes they stand for (letters, `_`, `:`, plus
//!   digits, `.` and `-` for `\c`);
//! - XSD character-class subtraction `[a-z-[aeiou]]` is written `[a-z--[aeiou]]`
//!   in the engine's regex syntax.
//!
//! Unicode block escapes (`\p{IsBasicLatin}`) are refused rather than guessed.

/// Translate `pattern` and check the result compiles.
pub fn to_sparql(pattern: &str) -> Result<String, String> {
    let mut out = String::with_capacity(pattern.len() + 8);
    let chars: Vec<char> = pattern.chars().collect();
    let mut i = 0usize;
    let mut class_depth = 0usize;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '\\' => {
                let Some(&n) = chars.get(i + 1) else {
                    return Err(format!("pattern `{pattern}` ends in a lone backslash"));
                };
                match n {
                    'i' => out.push_str(if class_depth > 0 {
                        "_:\\p{L}"
                    } else {
                        "[_:\\p{L}]"
                    }),
                    'I' => out.push_str("[^_:\\p{L}]"),
                    'c' => out.push_str(if class_depth > 0 {
                        "\\-._:\\p{L}\\p{Nd}"
                    } else {
                        "[\\-._:\\p{L}\\p{Nd}]"
                    }),
                    'C' => out.push_str("[^\\-._:\\p{L}\\p{Nd}]"),
                    'p' | 'P' => {
                        let rest: String = chars[i + 2..].iter().collect();
                        if rest.starts_with("{Is") {
                            return Err(format!(
                                "pattern `{pattern}` uses a Unicode block escape (\\p{{Is…}}), which is not supported"
                            ));
                        }
                        out.push('\\');
                        out.push(n);
                    }
                    other => {
                        out.push('\\');
                        out.push(other);
                    }
                }
                i += 2;
                continue;
            }
            '[' => {
                class_depth += 1;
                out.push('[');
            }
            ']' if class_depth > 0 => {
                class_depth -= 1;
                out.push(']');
            }
            '-' if class_depth > 0 && chars.get(i + 1) == Some(&'[') => {
                // Character-class subtraction.
                out.push_str("--");
            }
            '^' if class_depth > 0 && out.ends_with('[') => out.push('^'),
            '^' => out.push_str("\\^"),
            '$' => out.push_str("\\$"),
            other => out.push(other),
        }
        i += 1;
    }
    let anchored = format!("^(?:{out})$");
    regex::Regex::new(&anchored)
        .map_err(|e| format!("pattern `{pattern}` is not a supported regular expression: {e}"))?;
    Ok(anchored)
}

#[cfg(test)]
mod tests {
    use super::to_sparql;

    fn matches(p: &str, s: &str) -> bool {
        regex::Regex::new(&to_sparql(p).unwrap())
            .unwrap()
            .is_match(s)
    }

    #[test]
    fn xsd_patterns_match_the_whole_value() {
        assert!(matches("[A-Z]{2}[0-9]{2}", "AB12"));
        assert!(!matches("[A-Z]{2}[0-9]{2}", "xAB12x"), "anchored");
        assert!(matches("Foo.*", "Foobar"));
        assert!(!matches("Foo.*", "aFoo"));
        assert!(matches("1.*", "11"));
    }

    #[test]
    fn xsd_specials_translate() {
        assert!(matches("a^b$", "a^b$"), "^ and $ are literal in XSD");
        assert!(matches("[a-z-[aeiou]]+", "bcd"));
        assert!(!matches("[a-z-[aeiou]]+", "bad"));
        assert!(matches("\\i\\c*", "_x1"));
        assert!(to_sparql("\\p{IsBasicLatin}").is_err());
        assert!(to_sparql("(unclosed").is_err());
    }
}
