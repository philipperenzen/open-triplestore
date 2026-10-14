//! SQL identifiers as a mapping writes them: `rr:tableName`, `rr:column`,
//! template placeholders and join columns.
//!
//! R2RML takes these from SQL 2008 (§5): a name is a regular identifier
//! (`products`), which is case-insensitive and stands for its upper-case
//! form, or a delimited one (`"Product Lines"`), which is exactly its text;
//! a table name may be qualified by its schema (`sales.products`). A
//! processor compares them by SQL's rules, and follows its engine where the
//! engine departs from SQL 2008 (§3): PostgreSQL folds a regular identifier
//! to lower case, SQLite and MySQL compare names without regard to case
//! ([`IdentifierRules`]). The delimiters belong to the name's spelling, not
//! to the name, so they are parsed off here and each part is quoted again by
//! the dialect that runs the query — which is also why MySQL's backticks and
//! SQL Server's brackets are read as well as the standard double quote: a
//! mapping written against one dialect's spelling still names the same
//! table.

use ots_plugin_api::sources::IdentifierRules;

/// One SQL identifier, as written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Identifier {
    /// Unquoted: stands for its folded form.
    Regular(String),
    /// Quoted: exactly its text, delimiters removed.
    Delimited(String),
}

impl Identifier {
    /// The name this identifier stands for under `rules`.
    pub fn name(&self, rules: &IdentifierRules) -> String {
        match self {
            Identifier::Regular(r) => rules.fold_regular(r),
            Identifier::Delimited(d) => d.clone(),
        }
    }
}

/// The parts of a table name, outermost first, without their delimiters.
///
/// `None` when the text is not a (qualified) SQL name at all — a part that is
/// neither delimited nor a run of identifier characters. The caller then keeps
/// the whole text as one identifier, which is how a name that only means
/// something to its connector (a virtual source's class IRI) travels intact.
pub fn qualified_name(name: &str) -> Option<Vec<String>> {
    Some(
        qualified_identifiers(name)?
            .into_iter()
            .map(|i| match i {
                Identifier::Regular(s) | Identifier::Delimited(s) => s,
            })
            .collect(),
    )
}

/// The parts of a table name, outermost first, each a regular or a
/// delimited identifier. `None` as for [`qualified_name`].
pub fn qualified_identifiers(name: &str) -> Option<Vec<Identifier>> {
    let mut parts = Vec::new();
    let mut chars = name.chars().peekable();
    loop {
        let part = match chars.peek()? {
            '"' => Identifier::Delimited(delimited(&mut chars, '"', '"')?),
            '`' => Identifier::Delimited(delimited(&mut chars, '`', '`')?),
            '[' => Identifier::Delimited(delimited(&mut chars, '[', ']')?),
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
                Identifier::Regular(regular)
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

/// One unqualified identifier: a column name. `None` when the text is not
/// one SQL identifier.
pub fn identifier(text: &str) -> Option<Identifier> {
    let mut parts = qualified_identifiers(text)?;
    (parts.len() == 1).then(|| parts.remove(0))
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

/// The table name as SQL, each part quoted by the dialect. A regular part is
/// folded first when the dialect's `rules` are known. A name that does not
/// parse is quoted whole, as one identifier.
pub fn table_sql(
    name: &str,
    quote: &dyn Fn(&str) -> String,
    rules: Option<&IdentifierRules>,
) -> String {
    match parts_named(name, rules) {
        Some(parts) => parts.iter().map(|p| quote(p)).collect::<Vec<_>>().join("."),
        None => quote(name),
    }
}

/// The bare table a catalogue lookup can answer for: the one part of an
/// unqualified name, folded when the dialect's `rules` are known. A
/// schema-qualified name is `None` — looking its last part up in the current
/// schema could find a different table.
pub fn catalogue_table(name: &str, rules: Option<&IdentifierRules>) -> Option<String> {
    match parts_named(name, rules) {
        Some(parts) if parts.len() == 1 => parts.into_iter().next(),
        Some(_) => None,
        None => Some(name.to_string()),
    }
}

/// The names the parts of a (qualified) table name stand for.
fn parts_named(name: &str, rules: Option<&IdentifierRules>) -> Option<Vec<String>> {
    match rules {
        Some(rules) if rules.sql => Some(
            qualified_identifiers(name)?
                .iter()
                .map(|i| i.name(rules))
                .collect(),
        ),
        _ => qualified_name(name),
    }
}

/// The column of a logical table that the column name `written` names.
///
/// `written` is a column name as the mapping writes it: `rr:column`, a
/// template placeholder, a join column. It is an SQL identifier, resolved by
/// the dialect's `rules` (R2RML §5): a regular identifier stands for its
/// folded form, a delimited one for its text, and either names the column it
/// compares equal to. With the logical table's `columns` known, the result
/// is that column's name as the result set reports it; without them (a
/// connector that cannot describe a query), the name the identifier stands
/// for. A name no column matches is returned as it stands for, for the
/// column check to report.
///
/// An error when `written` is not one SQL identifier (a name with a space
/// must be delimited), or names more than one column.
pub fn resolve_column(
    written: &str,
    rules: &IdentifierRules,
    columns: Option<&[String]>,
) -> Result<String, String> {
    if !rules.sql {
        return Ok(column_name(written));
    }
    let Some(ident) = identifier(written) else {
        return Err(format!(
            "\"{written}\" is not an SQL identifier, which R2RML requires of a column name \
             (§5); write a name with spaces or punctuation delimited, as \"\\\"{}\\\"\"",
            written.replace('"', "\"\"")
        ));
    };
    let wanted = ident.name(rules);
    let Some(columns) = columns else {
        return Ok(wanted);
    };
    let candidates = match &ident {
        Identifier::Regular(r) => rules.regular_names(r),
        Identifier::Delimited(d) => vec![d.clone()],
    };
    for candidate in &candidates {
        let matching: Vec<&String> = columns
            .iter()
            .filter(|c| rules.same(candidate, c))
            .collect();
        match matching.as_slice() {
            [] => continue,
            [one] => return Ok((*one).clone()),
            many => {
                return match many.iter().find(|c| **c == candidate) {
                    Some(exact) => Ok((*exact).clone()),
                    None => Err(format!(
                        "the column name {written} names more than one column of its logical \
                         table: {}",
                        many.iter()
                            .map(|c| format!("\"{c}\""))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )),
                }
            }
        }
    }
    Ok(wanted)
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
        assert_eq!(table_sql("products", &std_quote, None), "\"products\"");
        assert_eq!(
            table_sql("sales.products", &std_quote, None),
            "\"sales\".\"products\""
        );
        assert_eq!(table_sql("\"Student\"", &mysql_quote, None), "`Student`");
        assert_eq!(table_sql("`Student`", &std_quote, None), "\"Student\"");
        // A class IRI (a virtual source's "table") is one identifier.
        assert_eq!(
            table_sql("http://example.org/Thing", &std_quote, None),
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
        assert_eq!(
            catalogue_table("products", None).as_deref(),
            Some("products")
        );
        assert_eq!(
            catalogue_table("\"Student\"", None).as_deref(),
            Some("Student")
        );
        assert_eq!(catalogue_table("sales.products", None), None);
        assert_eq!(
            catalogue_table("http://example.org/Thing", None).as_deref(),
            Some("http://example.org/Thing")
        );
    }

    const PG: IdentifierRules = IdentifierRules::POSTGRESQL;
    const SQL: IdentifierRules = IdentifierRules::SQL_2008;
    const LITE: IdentifierRules = IdentifierRules::SQLITE;
    const MY: IdentifierRules = IdentifierRules::CASE_INSENSITIVE;

    fn cols(v: &[&str]) -> Vec<String> {
        v.iter().map(|c| c.to_string()).collect()
    }

    /// R2RML §5: `deptno` and `"deptno"` are not equivalent, `DEPTNO` and
    /// `"DEPTNO"` are — under SQL 2008, which folds a regular identifier to
    /// upper case.
    #[test]
    fn sql_2008_folds_a_regular_identifier_to_upper_case() {
        let c = cols(&["DEPTNO", "deptno2"]);
        assert_eq!(resolve_column("deptno", &SQL, Some(&c)).unwrap(), "DEPTNO");
        assert_eq!(
            resolve_column("\"DEPTNO\"", &SQL, Some(&c)).unwrap(),
            "DEPTNO"
        );
        // A delimited lower-case name is not the folded regular one.
        let lower = cols(&["deptno"]);
        assert_eq!(
            resolve_column("deptno", &SQL, Some(&lower)).unwrap(),
            "DEPTNO"
        );
        assert_eq!(
            resolve_column("\"deptno\"", &SQL, Some(&lower)).unwrap(),
            "deptno"
        );
    }

    /// R2RMLTC0002f: the table has delimited columns `"ID"` and `"Name"`; a
    /// regular `ID` names the first under SQL 2008 and `Name` names neither.
    #[test]
    fn a_delimited_mixed_case_column_is_not_named_by_a_regular_identifier() {
        let c = cols(&["ID", "Name"]);
        assert_eq!(resolve_column("ID", &SQL, Some(&c)).unwrap(), "ID");
        assert_eq!(resolve_column("Name", &SQL, Some(&c)).unwrap(), "NAME");
        // PostgreSQL folds to lower case, and SQL 2008's upper case still
        // names "ID"; "Name" is named by neither.
        assert_eq!(resolve_column("ID", &PG, Some(&c)).unwrap(), "ID");
        assert_eq!(resolve_column("Name", &PG, Some(&c)).unwrap(), "name");
        assert_eq!(resolve_column("\"Name\"", &PG, Some(&c)).unwrap(), "Name");
    }

    #[test]
    fn postgresql_folds_a_regular_identifier_to_lower_case() {
        let c = cols(&["name", "Mixed"]);
        assert_eq!(resolve_column("NAME", &PG, Some(&c)).unwrap(), "name");
        assert_eq!(resolve_column("Name", &PG, Some(&c)).unwrap(), "name");
        assert_eq!(resolve_column("\"Mixed\"", &PG, Some(&c)).unwrap(), "Mixed");
        assert_eq!(resolve_column("Mixed", &PG, Some(&c)).unwrap(), "mixed");
        // SQL 2008: a regular SPORTCOUNT names "SPORTCOUNT" (R2RMLTC0009d).
        let upper = cols(&["SPORTCOUNT"]);
        assert_eq!(
            resolve_column("SPORTCOUNT", &PG, Some(&upper)).unwrap(),
            "SPORTCOUNT"
        );
        assert_eq!(
            resolve_column("sportcount", &PG, Some(&upper)).unwrap(),
            "SPORTCOUNT"
        );
        // Both forms present: PostgreSQL's own fold wins.
        let both = cols(&["name", "NAME"]);
        assert_eq!(resolve_column("Name", &PG, Some(&both)).unwrap(), "name");
        assert_eq!(
            resolve_column("\"NAME\"", &PG, Some(&both)).unwrap(),
            "NAME"
        );
    }

    /// SQLite and MySQL compare names without regard to case, delimited or
    /// not, and report them as written.
    #[test]
    fn case_insensitive_engines_report_the_catalogue_spelling() {
        let c = cols(&["Name", "ID"]);
        for rules in [LITE, MY] {
            assert_eq!(resolve_column("name", &rules, Some(&c)).unwrap(), "Name");
            assert_eq!(resolve_column("NAME", &rules, Some(&c)).unwrap(), "Name");
            assert_eq!(
                resolve_column("\"name\"", &rules, Some(&c)).unwrap(),
                "Name"
            );
            assert_eq!(resolve_column("`id`", &rules, Some(&c)).unwrap(), "ID");
        }
        // SQLite folds ASCII letters only.
        let u = cols(&["Größe"]);
        assert_eq!(resolve_column("GRÖSSE", &LITE, Some(&u)).unwrap(), "GRÖSSE");
        assert_eq!(resolve_column("GRÖßE", &LITE, Some(&u)).unwrap(), "GRÖßE");
        assert_eq!(resolve_column("größe", &MY, Some(&u)).unwrap(), "Größe");
    }

    #[test]
    fn a_name_that_is_not_one_identifier_is_refused() {
        for bad in ["Department Number", "a.b", "\"open", ""] {
            assert!(resolve_column(bad, &SQL, None).is_err(), "{bad:?}");
        }
        let e = resolve_column("Department Number", &PG, None).unwrap_err();
        assert!(e.contains(r#""\"Department Number\"""#), "{e}");
        // A source that is not SQL takes names verbatim.
        let v = IdentifierRules::VERBATIM;
        assert_eq!(resolve_column("a b", &v, None).unwrap(), "a b");
    }

    #[test]
    fn a_name_matching_two_columns_is_refused_unless_one_is_exact() {
        let c = cols(&["a", "A"]);
        assert_eq!(resolve_column("a", &LITE, Some(&c)).unwrap(), "a");
        assert!(resolve_column("b", &MY, Some(&cols(&["B", "b"]))).is_ok());
        assert!(resolve_column("B", &MY, Some(&cols(&["b", "b2"]))).is_ok());
        let e = resolve_column("x", &MY, Some(&cols(&["X", "X"]))).unwrap_err();
        assert!(e.contains("more than one column"), "{e}");
    }

    #[test]
    fn a_regular_table_name_is_folded_when_the_rules_are_known() {
        assert_eq!(table_sql("Student", &std_quote, Some(&PG)), "\"student\"");
        assert_eq!(
            table_sql("Sales.\"Line\"", &std_quote, Some(&SQL)),
            "\"SALES\".\"Line\""
        );
        assert_eq!(table_sql("Student", &std_quote, Some(&LITE)), "\"Student\"");
        assert_eq!(
            catalogue_table("Student", Some(&PG)).as_deref(),
            Some("student")
        );
        // A class IRI travels intact whatever the rules.
        assert_eq!(
            table_sql("http://example.org/Thing", &std_quote, Some(&PG)),
            "\"http://example.org/Thing\""
        );
    }
}
