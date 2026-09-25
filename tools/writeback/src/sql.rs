//! The SQL a change becomes, and the targets that run it.
//!
//! An upsert names the key and only the columns the member carried: a
//! column the mapping knows but the member did not mention keeps what the
//! row already holds. `INSERT … ON CONFLICT (key) DO UPDATE` is the same
//! statement in SQLite (3.24+) and PostgreSQL (9.5+), and it needs the key
//! to be unique in the table — a primary key, which is what a mapping's
//! subject template names in practice.
//!
//! Values travel as text: SQLite applies the column's affinity, and
//! PostgreSQL coerces an untyped literal to the column's type, so a `"42"`
//! lands in an integer column as 42 and a `"2026-01-01"` in a date column as
//! a date. Nothing is parsed twice on the way.

/// One row-level change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    Upsert {
        table: String,
        key_column: String,
        key: String,
        /// `(column, value)` for what the member carried.
        columns: Vec<(String, String)>,
    },
    Delete {
        table: String,
        key_column: String,
        key: String,
    },
}

impl Change {
    pub fn table(&self) -> &str {
        match self {
            Change::Upsert { table, .. } | Change::Delete { table, .. } => table,
        }
    }
}

/// A double-quoted identifier, the SQL standard form both targets accept.
pub fn quote(ident: &str) -> String {
    format!("\"{}\"", ident.replace('"', "\"\""))
}

/// A single-quoted literal for the simple-query path (PostgreSQL). With
/// standard-conforming strings a backslash is literal, so only the quote
/// needs doubling; a NUL cannot be carried at all.
pub fn literal(value: &str) -> String {
    format!("'{}'", value.replace('\0', "").replace('\'', "''"))
}

/// The statement with `?`-style placeholders (`$n` when `numbered`) and
/// its parameters, in order.
pub fn statement(change: &Change, numbered: bool) -> (String, Vec<String>) {
    let placeholder = |i: usize| {
        if numbered {
            format!("${}", i + 1)
        } else {
            "?".to_string()
        }
    };
    match change {
        Change::Upsert {
            table,
            key_column,
            key,
            columns,
        } => {
            let mut names = vec![quote(key_column)];
            let mut params = vec![key.clone()];
            for (c, v) in columns {
                names.push(quote(c));
                params.push(v.clone());
            }
            let values: Vec<String> = (0..params.len()).map(placeholder).collect();
            let update = if columns.is_empty() {
                "DO NOTHING".to_string()
            } else {
                format!(
                    "DO UPDATE SET {}",
                    columns
                        .iter()
                        .map(|(c, _)| format!("{q} = excluded.{q}", q = quote(c)))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            };
            (
                format!(
                    "INSERT INTO {} ({}) VALUES ({}) ON CONFLICT ({}) {update}",
                    quote(table),
                    names.join(", "),
                    values.join(", "),
                    quote(key_column)
                ),
                params,
            )
        }
        Change::Delete {
            table,
            key_column,
            key,
        } => (
            format!(
                "DELETE FROM {} WHERE {} = {}",
                quote(table),
                quote(key_column),
                placeholder(0)
            ),
            vec![key.clone()],
        ),
    }
}

/// The statement with its values inlined as literals — what a dry run
/// prints, and what the PostgreSQL target sends.
pub fn inline(change: &Change) -> String {
    let (sql, params) = statement(change, false);
    let mut out = String::with_capacity(sql.len() + 32);
    let mut params = params.iter();
    for ch in sql.chars() {
        if ch == '?' {
            out.push_str(&literal(params.next().map(String::as_str).unwrap_or("")));
        } else {
            out.push(ch);
        }
    }
    out
}

/// Somewhere changes can be applied, one transaction per batch.
pub trait Target {
    fn apply(&mut self, changes: &[Change]) -> Result<usize, String>;
}

pub struct Sqlite(pub rusqlite::Connection);

impl Target for Sqlite {
    fn apply(&mut self, changes: &[Change]) -> Result<usize, String> {
        let tx = self.0.transaction().map_err(|e| e.to_string())?;
        let mut affected = 0usize;
        for change in changes {
            let (sql, params) = statement(change, false);
            let refs: Vec<&dyn rusqlite::ToSql> =
                params.iter().map(|p| p as &dyn rusqlite::ToSql).collect();
            affected += tx
                .execute(&sql, refs.as_slice())
                .map_err(|e| format!("{sql}: {e}"))?;
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(affected)
    }
}

pub struct Postgres(pub postgres::Client);

impl Target for Postgres {
    fn apply(&mut self, changes: &[Change]) -> Result<usize, String> {
        if changes.is_empty() {
            return Ok(0);
        }
        // One batch, one transaction: the simple-query path runs a
        // multi-statement string atomically, and untyped literals take the
        // columns' types.
        let mut batch = String::from("BEGIN;\n");
        for change in changes {
            batch.push_str(&inline(change));
            batch.push_str(";\n");
        }
        batch.push_str("COMMIT;");
        self.0
            .batch_execute(&batch)
            .map_err(|e| match e.as_db_error() {
                Some(db) => db.message().to_string(),
                None => e.to_string(),
            })?;
        Ok(changes.len())
    }
}

/// A target that only prints: the dry run.
pub struct DryRun<'a>(pub &'a mut dyn std::io::Write);

impl Target for DryRun<'_> {
    fn apply(&mut self, changes: &[Change]) -> Result<usize, String> {
        for change in changes {
            writeln!(self.0, "{};", inline(change)).map_err(|e| e.to_string())?;
        }
        Ok(changes.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn upsert() -> Change {
        Change::Upsert {
            table: "products".into(),
            key_column: "product_id".into(),
            key: "7".into(),
            columns: vec![
                ("name".into(), "Bo'lt".into()),
                ("price".into(), "0.25".into()),
            ],
        }
    }

    #[test]
    fn statements_upsert_on_the_key_and_delete_by_it() {
        let (sql, params) = statement(&upsert(), false);
        assert_eq!(
            sql,
            "INSERT INTO \"products\" (\"product_id\", \"name\", \"price\") VALUES (?, ?, ?) \
             ON CONFLICT (\"product_id\") DO UPDATE SET \"name\" = excluded.\"name\", \"price\" = excluded.\"price\""
        );
        assert_eq!(params, vec!["7", "Bo'lt", "0.25"]);
        let (sql, _) = statement(&upsert(), true);
        assert!(sql.contains("VALUES ($1, $2, $3)"), "{sql}");
        let key_only = Change::Upsert {
            table: "t".into(),
            key_column: "k".into(),
            key: "1".into(),
            columns: vec![],
        };
        assert!(statement(&key_only, false).0.ends_with("DO NOTHING"));
        let del = Change::Delete {
            table: "products".into(),
            key_column: "product_id".into(),
            key: "8".into(),
        };
        assert_eq!(
            statement(&del, false),
            (
                "DELETE FROM \"products\" WHERE \"product_id\" = ?".to_string(),
                vec!["8".to_string()]
            )
        );
        assert_eq!(
            inline(&del),
            "DELETE FROM \"products\" WHERE \"product_id\" = '8'"
        );
        assert!(
            inline(&upsert()).contains("'Bo''lt'"),
            "{}",
            inline(&upsert())
        );
        assert_eq!(quote("a\"b"), "\"a\"\"b\"");
        assert_eq!(literal("x\0y'"), "'xy'''");
    }

    #[test]
    fn sqlite_applies_a_batch_in_one_transaction_and_is_idempotent() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE products (product_id INTEGER PRIMARY KEY, name TEXT, price REAL, colour TEXT DEFAULT 'red');
             INSERT INTO products VALUES (8, 'Nut', 1.5, 'grey');",
        )
        .unwrap();
        let mut target = Sqlite(conn);
        let changes = vec![
            upsert(),
            Change::Delete {
                table: "products".into(),
                key_column: "product_id".into(),
                key: "8".into(),
            },
        ];
        assert_eq!(target.apply(&changes).unwrap(), 2);
        assert_eq!(
            target.apply(&changes).unwrap(),
            1,
            "the delete finds nothing the second time"
        );
        let (name, price, colour): (String, f64, String) = target
            .0
            .query_row(
                "SELECT name, price, colour FROM products WHERE product_id = 7",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            (name.as_str(), price, colour.as_str()),
            ("Bo'lt", 0.25, "red")
        );
        let gone: i64 = target
            .0
            .query_row(
                "SELECT COUNT(*) FROM products WHERE product_id = 8",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(gone, 0);
        // A partial update keeps the columns the member did not carry.
        let rename = Change::Upsert {
            table: "products".into(),
            key_column: "product_id".into(),
            key: "7".into(),
            columns: vec![("name".into(), "Bolt".into())],
        };
        target.apply(&[rename]).unwrap();
        let (name, price): (String, f64) = target
            .0
            .query_row(
                "SELECT name, price FROM products WHERE product_id = 7",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((name.as_str(), price), ("Bolt", 0.25));
        // A failing statement rolls the whole batch back.
        let bad = vec![
            Change::Delete {
                table: "products".into(),
                key_column: "product_id".into(),
                key: "7".into(),
            },
            Change::Upsert {
                table: "nope".into(),
                key_column: "k".into(),
                key: "1".into(),
                columns: vec![],
            },
        ];
        assert!(target.apply(&bad).is_err());
        let still: i64 = target
            .0
            .query_row(
                "SELECT COUNT(*) FROM products WHERE product_id = 7",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(still, 1);
        let mut out = Vec::new();
        DryRun(&mut out).apply(&changes).unwrap();
        let printed = String::from_utf8(out).unwrap();
        assert!(printed.starts_with("INSERT INTO \"products\""), "{printed}");
        assert!(
            printed.contains("DELETE FROM \"products\" WHERE \"product_id\" = '8';"),
            "{printed}"
        );
    }
}
