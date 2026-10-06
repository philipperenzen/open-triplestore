//! SQL identifiers as a mapping writes them: `rr:tableName`, `rr:column`,
//! template placeholders and join columns.
//!
//! R2RML takes these from SQL: a name is a regular identifier (`products`) or
//! a delimited one (`"Product Lines"`), and a table name may be qualified by
//! its schema (`sales.products`). The delimiters belong to the name's spelling,
//! not to the name, so they are parsed off here and each part is quoted again
//! by the dialect that runs the query — which is also why MySQL's backticks
//! and SQL Server's brackets are read as well as the standard double quote: a
//! mapping written against one dialect's spelling still names the same table.

/// The parts of a table name, outermost first, without their delimiters.
///
/// `None` when the text is not a (qualified) SQL name at all — a part that is
/// neither delimited nor a run of identifier characters. The caller then keeps
/// the whole text as one identifier, which is how a name that only means
/// something to its connector (a virtual source's class IRI) travels intact.
pub fn qualified_name(name: &str) -> Option<Vec<String>> {
    let mut parts = Vec::new();
    let mut chars = name.chars().peekable();
    loop {
        let part = match chars.peek()? {
            '"' => delimited(&mut chars, '"', '"')?,
            '`' => delimited(&mut chars, '`', '`')?,
            '[' => delimited(&mut chars, '[', ']')?,
            _ => {
                let mut regular = String::new();
                while let Some(&c) = chars.peek() {
                    if c == '.' {
                        break;
                    }
                    if !is_identifier_char(c) {
                        return None;
                    }
                    regular.push(c);
                    chars.next();
                }
                if regular.is_empty() {
                    return None;
                }
                regular
            }
        };
        parts.push(part);
        match chars.next() {
            None => return Some(parts),
            Some('.') => continue,
            Some(_) => return None,
        }
    }
}

/// A delimited identifier, opening delimiter next in `chars`. A doubled
/// closing delimiter stands for itself.
fn delimited(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
    open: char,
    close: char,
) -> Option<String> {
    if chars.next()? != open {
        return None;
    }
    let mut out = String::new();
    loop {
        let c = chars.next()?;
        if c == close {
            if chars.peek() == Some(&close) {
                chars.next();
                out.push(close);
                continue;
            }
            return (!out.is_empty()).then_some(out);
        }
        out.push(c);
    }
}

/// What a regular identifier may contain, across the dialects this engine
/// speaks: letters and digits (Unicode included), `_`, and the `$`, `#` and
/// `@` that PostgreSQL, MySQL and SQL Server variously allow.
fn is_identifier_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '$' | '#' | '@')
}

/// A column name as the result set reports it: a delimited identifier loses
/// its delimiters, anything else is kept exactly as written.
pub fn column_name(name: &str) -> String {
    match qualified_name(name) {
        Some(parts) if parts.len() == 1 && starts_delimited(name) => {
            parts.into_iter().next().unwrap_or_default()
        }
        _ => name.to_string(),
    }
}

fn starts_delimited(name: &str) -> bool {
    name.starts_with(['"', '`', '['])
}

/// The table name as SQL, each part quoted by the dialect. A name that does
/// not parse is quoted whole, as one identifier.
pub fn table_sql(name: &str, quote: &dyn Fn(&str) -> String) -> String {
    match qualified_name(name) {
        Some(parts) => parts.iter().map(|p| quote(p)).collect::<Vec<_>>().join("."),
        None => quote(name),
    }
}

/// The bare table a catalogue lookup can answer for: the one part of an
/// unqualified name. A schema-qualified name is `None` — looking its last
/// part up in the current schema could find a different table.
pub fn catalogue_table(name: &str) -> Option<String> {
    match qualified_name(name) {
        Some(parts) if parts.len() == 1 => parts.into_iter().next(),
        Some(_) => None,
        None => Some(name.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn std_quote(i: &str) -> String {
        format!("\"{}\"", i.replace('"', "\"\""))
    }
    fn mysql_quote(i: &str) -> String {
        format!("`{}`", i.replace('`', "``"))
    }

    #[test]
    fn names_split_on_dots_outside_delimiters() {
        assert_eq!(qualified_name("products").unwrap(), vec!["products"]);
        assert_eq!(
            qualified_name("sales.products").unwrap(),
            vec!["sales", "products"]
        );
        assert_eq!(qualified_name("\"Student\"").unwrap(), vec!["Student"]);
        assert_eq!(
            qualified_name("\"my.schema\".\"Line \"\"A\"\"\"").unwrap(),
            vec!["my.schema", "Line \"A\""]
        );
        assert_eq!(qualified_name("`a``b`").unwrap(), vec!["a`b"]);
        assert_eq!(qualified_name("[dbo].[x]]y]").unwrap(), vec!["dbo", "x]y"]);
        assert_eq!(qualified_name("Größe").unwrap(), vec!["Größe"]);
    }

    #[test]
    fn text_that_is_not_a_sql_name_is_refused() {
        for bad in [
            "http://example.org/Thing",
            "a..b",
            ".a",
            "a.",
            "has space",
            "\"open",
            "\"\"",
            "\"a\"b",
            "",
        ] {
            assert_eq!(qualified_name(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn a_table_is_quoted_part_by_part_in_the_dialects_spelling() {
        assert_eq!(table_sql("products", &std_quote), "\"products\"");
        assert_eq!(
            table_sql("sales.products", &std_quote),
            "\"sales\".\"products\""
        );
        assert_eq!(table_sql("\"Student\"", &mysql_quote), "`Student`");
        assert_eq!(table_sql("`Student`", &std_quote), "\"Student\"");
        // A class IRI (a virtual source's "table") is one identifier.
        assert_eq!(
            table_sql("http://example.org/Thing", &std_quote),
            "\"http://example.org/Thing\""
        );
    }

    #[test]
    fn a_delimited_column_loses_its_delimiters() {
        assert_eq!(column_name("\"ID\""), "ID");
        assert_eq!(column_name("`Name`"), "Name");
        assert_eq!(column_name("[Full Name]"), "Full Name");
        assert_eq!(column_name("id"), "id");
        assert_eq!(column_name("a.b"), "a.b", "a regular name stays as written");
        assert_eq!(column_name("\"x\".\"y\""), "\"x\".\"y\"");
    }

    #[test]
    fn only_an_unqualified_name_is_looked_up_in_the_catalogue() {
        assert_eq!(catalogue_table("products").as_deref(), Some("products"));
        assert_eq!(catalogue_table("\"Student\"").as_deref(), Some("Student"));
        assert_eq!(catalogue_table("sales.products"), None);
        assert_eq!(
            catalogue_table("http://example.org/Thing").as_deref(),
            Some("http://example.org/Thing")
        );
    }
}
