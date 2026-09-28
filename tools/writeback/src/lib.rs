//! The writeback worker's engine: an RML mapping read backwards, an LDES
//! fragment read forwards, and the SQL that reconciles the two.
//!
//! The store publishes what changed as an LDES stream: one member per
//! changed entity, carrying the entity's properties, or a tombstone when it
//! disappeared. This crate turns those members back into rows. It is a
//! separate binary on purpose: the store never writes to a source system,
//! and a datasource account the store holds is read-only by contract. The
//! worker holds its own, writing, credential — outside the store — and
//! reaches the store as any client would, with an API token.
//!
//! What it can invert is what a mapping states plainly: a triples map that
//! reads a **table** (`rr:tableName`), mints its subject from a **template
//! with one placeholder** (or takes a column as the IRI), and asserts
//! **columns** as objects. Anything else — a query source, a computed
//! object, a join — is reported and left alone: the worker never guesses a
//! value, and a column it cannot derive keeps what the row already holds.

pub mod ldes;
pub mod rml;
pub mod sql;

use ldes::MemberChange;
use rml::TableRule;
use sql::Change;

/// The changes a set of members implies under a set of rules, in member
/// order. A member that no rule recognises yields nothing (and is counted in
/// `skipped`); a tombstone becomes a delete on every rule that recognised
/// the entity.
#[derive(Debug, Default)]
pub struct Plan {
    pub changes: Vec<Change>,
    pub skipped: Vec<String>,
}

pub fn plan(members: &[MemberChange], rules: &[TableRule]) -> Plan {
    let mut plan = Plan::default();
    for m in members {
        let mut matched = false;
        for rule in rules {
            // A tombstone carries no properties, so no class: its IRI alone
            // says which table it left.
            let key = if m.deleted {
                rule.subject.key_of(&m.entity)
            } else {
                rule.key_of(&m.entity, &m.types())
            };
            let Some(key) = key else {
                continue;
            };
            matched = true;
            if m.deleted {
                plan.changes.push(Change::Delete {
                    table: rule.table.clone(),
                    key_column: rule.key_column.clone(),
                    key,
                });
                continue;
            }
            let mut columns: Vec<(String, String)> = Vec::new();
            for (predicate, column) in &rule.columns {
                if let Some(value) = m.value_of(predicate) {
                    if !columns.iter().any(|(c, _)| c == column) {
                        columns.push((column.clone(), value));
                    }
                }
            }
            plan.changes.push(Change::Upsert {
                table: rule.table.clone(),
                key_column: rule.key_column.clone(),
                key,
                columns,
            });
        }
        if !matched {
            plan.skipped.push(m.entity.clone());
        }
    }
    plan
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxrdf::{Literal, Term};

    fn member(entity: &str, deleted: bool, props: &[(&str, &str)]) -> MemberChange {
        MemberChange {
            member_id: 1,
            member_iri: "http://store/api/datasets/d/ldes/members/1".into(),
            entity: entity.into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            deleted,
            properties: props
                .iter()
                .map(|(p, v)| {
                    (
                        p.to_string(),
                        Term::Literal(Literal::new_simple_literal(*v)),
                    )
                })
                .collect(),
        }
    }

    fn rule() -> TableRule {
        TableRule {
            triples_map: "http://example.org/map#Products".into(),
            table: "products".into(),
            key_column: "product_id".into(),
            subject: rml::SubjectPattern::Template {
                prefix: "http://example.org/products/product_".into(),
                suffix: String::new(),
            },
            classes: vec![],
            columns: vec![
                ("http://example.org/ontology#name".into(), "name".into()),
                ("http://example.org/ontology#price".into(), "price".into()),
            ],
        }
    }

    #[test]
    fn members_become_upserts_and_tombstones_deletes() {
        let members = vec![
            member(
                "http://example.org/products/product_7",
                false,
                &[
                    ("http://example.org/ontology#name", "Bolt"),
                    ("http://example.org/ontology#colour", "red"),
                ],
            ),
            member("http://example.org/products/product_8", true, &[]),
            member("http://example.org/other/thing", false, &[]),
        ];
        let p = plan(&members, &[rule()]);
        assert_eq!(p.changes.len(), 2);
        assert_eq!(
            p.changes[0],
            Change::Upsert {
                table: "products".into(),
                key_column: "product_id".into(),
                key: "7".into(),
                columns: vec![("name".into(), "Bolt".into())],
            }
        );
        assert_eq!(
            p.changes[1],
            Change::Delete {
                table: "products".into(),
                key_column: "product_id".into(),
                key: "8".into(),
            }
        );
        assert_eq!(
            p.skipped,
            vec!["http://example.org/other/thing".to_string()]
        );
    }
}
