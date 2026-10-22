#[cfg(feature = "rdf-12")]
use crate::storage::numeric_encoder::EncodedTriple;
use crate::storage::numeric_encoder::{
    Decoder, EncodedTerm, StrHash, StrHashHasher, StrLookup, insert_term,
};
use crate::storage::{CorruptionError, StorageError, StorageReader};
use oxrdf::Term;
use oxsdatatypes::Boolean;
use rustc_hash::{FxHashMap, FxHashSet};
#[cfg(feature = "rdf-12")]
use spareval::ExpressionTriple;
use spareval::{ExpressionTerm, InternalQuad, InternalTriple, QueryableDataset};
use std::cell::RefCell;
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::hash::BuildHasherDefault;
#[cfg(feature = "rdf-12")]
use std::sync::Arc;

pub struct DatasetView<'a> {
    reader: StorageReader<'a>,
    extra: RefCell<HashMap<StrHash, String, BuildHasherDefault<StrHashHasher>>>,
    /// The predicates a default graph that merges several graphs was scanned
    /// with so far in this evaluation.
    scanned: RefCell<FxHashSet<EncodedTerm>>,
    /// Whether a graph holds a quad with a predicate, keyed by (graph,
    /// predicate), for the graphs of a default graph that merges several.
    /// The reader is a snapshot, so an answer holds for the whole evaluation.
    predicates: RefCell<FxHashMap<(EncodedTerm, EncodedTerm), bool>>,
}

/// At most this many predicates and (graph, predicate) answers are kept per
/// evaluation; past that, a new predicate is scanned over every graph.
const MAX_PREDICATE_PROBES: usize = 1024;

impl<'a> DatasetView<'a> {
    pub fn new(reader: StorageReader<'a>) -> Self {
        Self {
            reader,
            extra: RefCell::new(HashMap::default()),
            scanned: RefCell::new(FxHashSet::default()),
            predicates: RefCell::new(FxHashMap::default()),
        }
    }

    /// Whether a merge was scanned with `predicate` before in this
    /// evaluation. The first scan opens every graph, as before; the graphs are
    /// probed from the second scan on, so a predicate that a query looks up
    /// once costs no probe.
    fn scanned_before(&self, predicate: &EncodedTerm) -> bool {
        let mut scanned = self.scanned.borrow_mut();
        if scanned.contains(predicate) {
            return true;
        }
        if scanned.len() < MAX_PREDICATE_PROBES {
            scanned.insert(predicate.clone());
        }
        false
    }

    /// Whether `graph_name` (`None`: the default graph) holds a quad with
    /// `predicate`, probed once per graph and predicate. A failed probe keeps
    /// the graph, so the scan that follows reports the error.
    fn graph_has_predicate(
        &self,
        graph_name: Option<&EncodedTerm>,
        predicate: &EncodedTerm,
    ) -> bool {
        let graph_name = graph_name.unwrap_or(&EncodedTerm::DefaultGraph);
        let key = (graph_name.clone(), predicate.clone());
        if let Some(found) = self.predicates.borrow().get(&key) {
            return *found;
        }
        let found = self
            .reader
            .quads_for_pattern(None, Some(predicate), None, Some(graph_name))
            .next()
            .is_some();
        let mut predicates = self.predicates.borrow_mut();
        if predicates.len() < MAX_PREDICATE_PROBES {
            predicates.insert(key, found);
        }
        found
    }

    pub fn insert_str(&self, key: &StrHash, value: &str) {
        if let Entry::Vacant(e) = self.extra.borrow_mut().entry(*key) {
            if !matches!(self.reader.contains_str(key), Ok(true)) {
                e.insert(value.to_owned());
            }
        }
    }
}

impl<'a> QueryableDataset<'a> for DatasetView<'a> {
    type InternalTerm = EncodedTerm;
    type Error = StorageError;

    fn internal_quads_for_pattern(
        &self,
        subject: Option<&EncodedTerm>,
        predicate: Option<&EncodedTerm>,
        object: Option<&EncodedTerm>,
        graph_name: Option<Option<&EncodedTerm>>,
    ) -> impl Iterator<Item = Result<InternalQuad<EncodedTerm>, StorageError>> + use<'a> {
        self.reader
            .quads_for_pattern(
                subject,
                predicate,
                object,
                graph_name.map(|graph_name| graph_name.unwrap_or(&EncodedTerm::DefaultGraph)),
            )
            .map(|quad| {
                let quad = quad?;
                Ok(InternalQuad {
                    subject: quad.subject,
                    predicate: quad.predicate,
                    object: quad.object,
                    graph_name: if quad.graph_name.is_default_graph() {
                        None
                    } else {
                        Some(quad.graph_name)
                    },
                })
            })
    }

    fn internal_triples_for_pattern(
        &self,
        subject: Option<&EncodedTerm>,
        predicate: Option<&EncodedTerm>,
        object: Option<&EncodedTerm>,
        graph_names: Option<&[Option<EncodedTerm>]>,
    ) -> impl Iterator<Item = Result<InternalTriple<EncodedTerm>, StorageError>> + use<'a> {
        // A graph with no quad of the pattern's predicate cannot match: from
        // the second scan with a predicate on, it is left out before a scan is
        // opened on it, so a data graph merged with a small side graph
        // (shapes, a vocabulary) costs what the data graph alone costs. A
        // for-loop join opened a scan on every graph per row.
        let kept: Vec<Option<EncodedTerm>>;
        let graph_names = match (graph_names, predicate) {
            (Some(graph_names), Some(predicate))
                if graph_names.len() > 1 && self.scanned_before(predicate) =>
            {
                kept = graph_names
                    .iter()
                    .filter(|graph_name| self.graph_has_predicate(graph_name.as_ref(), predicate))
                    .cloned()
                    .collect();
                Some(kept.as_slice())
            }
            (graph_names, _) => graph_names,
        };
        self.reader
            .quads_for_pattern_in_union(subject, predicate, object, graph_names)
            .map(|quad| {
                let quad = quad?;
                Ok(InternalTriple {
                    subject: quad.subject,
                    predicate: quad.predicate,
                    object: quad.object,
                })
            })
    }

    fn internal_named_graphs(
        &self,
    ) -> impl Iterator<Item = Result<EncodedTerm, StorageError>> + use<'a> {
        self.reader.named_graphs()
    }

    fn contains_internal_graph_name(&self, graph_name: &EncodedTerm) -> Result<bool, StorageError> {
        self.reader.contains_named_graph(graph_name)
    }

    fn internalize_term(&self, term: Term) -> Result<EncodedTerm, StorageError> {
        let encoded = term.as_ref().into();
        insert_term(term.as_ref(), &encoded, &mut |key, value| {
            self.insert_str(key, value)
        });
        Ok(encoded)
    }

    fn externalize_term(&self, term: EncodedTerm) -> Result<Term, StorageError> {
        self.decode_term(&term)
    }

    fn externalize_expression_term(
        &self,
        term: EncodedTerm,
    ) -> Result<ExpressionTerm, StorageError> {
        Ok(match term {
            EncodedTerm::DefaultGraph => {
                return Err(CorruptionError::new("Unexpected default graph").into());
            }
            EncodedTerm::BooleanLiteral(value) => ExpressionTerm::BooleanLiteral(value),
            EncodedTerm::FloatLiteral(value) => ExpressionTerm::FloatLiteral(value),
            EncodedTerm::DoubleLiteral(value) => ExpressionTerm::DoubleLiteral(value),
            EncodedTerm::IntegerLiteral(value) => ExpressionTerm::IntegerLiteral(value),
            EncodedTerm::DecimalLiteral(value) => ExpressionTerm::DecimalLiteral(value),
            EncodedTerm::DateTimeLiteral(value) => ExpressionTerm::DateTimeLiteral(value),
            EncodedTerm::TimeLiteral(value) => ExpressionTerm::TimeLiteral(value),
            EncodedTerm::DateLiteral(value) => ExpressionTerm::DateLiteral(value),
            EncodedTerm::GYearMonthLiteral(value) => ExpressionTerm::GYearMonthLiteral(value),
            EncodedTerm::GYearLiteral(value) => ExpressionTerm::GYearLiteral(value),
            EncodedTerm::GMonthDayLiteral(value) => ExpressionTerm::GMonthDayLiteral(value),
            EncodedTerm::GDayLiteral(value) => ExpressionTerm::GDayLiteral(value),
            EncodedTerm::GMonthLiteral(value) => ExpressionTerm::GMonthLiteral(value),
            EncodedTerm::DurationLiteral(value) => ExpressionTerm::DurationLiteral(value),
            EncodedTerm::YearMonthDurationLiteral(value) => {
                ExpressionTerm::YearMonthDurationLiteral(value)
            }
            EncodedTerm::DayTimeDurationLiteral(value) => {
                ExpressionTerm::DayTimeDurationLiteral(value)
            }
            #[cfg(feature = "rdf-12")]
            EncodedTerm::Triple(t) => ExpressionTriple::new(
                self.externalize_expression_term(t.subject.clone())?,
                self.externalize_expression_term(t.predicate.clone())?,
                self.externalize_expression_term(t.object.clone())?,
            )
            .ok_or_else(|| CorruptionError::msg("Invalid triple term in the storage"))?
            .into(),
            _ => self.decode_term(&term)?.into(), // No escape
        })
    }

    fn internalize_expression_term(
        &self,
        term: ExpressionTerm,
    ) -> Result<EncodedTerm, StorageError> {
        Ok(match term {
            ExpressionTerm::BooleanLiteral(value) => EncodedTerm::BooleanLiteral(value),
            ExpressionTerm::FloatLiteral(value) => EncodedTerm::FloatLiteral(value),
            ExpressionTerm::DoubleLiteral(value) => EncodedTerm::DoubleLiteral(value),
            ExpressionTerm::IntegerLiteral(value) => EncodedTerm::IntegerLiteral(value),
            ExpressionTerm::DecimalLiteral(value) => EncodedTerm::DecimalLiteral(value),
            ExpressionTerm::DateTimeLiteral(value) => EncodedTerm::DateTimeLiteral(value),
            ExpressionTerm::TimeLiteral(value) => EncodedTerm::TimeLiteral(value),
            ExpressionTerm::DateLiteral(value) => EncodedTerm::DateLiteral(value),
            ExpressionTerm::GYearMonthLiteral(value) => EncodedTerm::GYearMonthLiteral(value),
            ExpressionTerm::GYearLiteral(value) => EncodedTerm::GYearLiteral(value),
            ExpressionTerm::GMonthDayLiteral(value) => EncodedTerm::GMonthDayLiteral(value),
            ExpressionTerm::GDayLiteral(value) => EncodedTerm::GDayLiteral(value),
            ExpressionTerm::GMonthLiteral(value) => EncodedTerm::GMonthLiteral(value),
            ExpressionTerm::DurationLiteral(value) => EncodedTerm::DurationLiteral(value),
            ExpressionTerm::YearMonthDurationLiteral(value) => {
                EncodedTerm::YearMonthDurationLiteral(value)
            }
            ExpressionTerm::DayTimeDurationLiteral(value) => {
                EncodedTerm::DayTimeDurationLiteral(value)
            }
            #[cfg(feature = "rdf-12")]
            ExpressionTerm::Triple(t) => EncodedTerm::Triple(Arc::new(EncodedTriple {
                subject: self.internalize_expression_term(t.subject.into())?,
                predicate: self.internalize_expression_term(t.predicate.into())?,
                object: self.internalize_expression_term(t.object)?,
            })),
            _ => self.internalize_term(term.into())?, // No fast path
        })
    }

    fn internal_term_effective_boolean_value(
        &self,
        term: EncodedTerm,
    ) -> Result<Option<bool>, StorageError> {
        Ok(match term {
            EncodedTerm::BooleanLiteral(value) => Some(value.into()),
            EncodedTerm::SmallStringLiteral(value) => Some(!value.is_empty()),
            EncodedTerm::BigStringLiteral { .. } => {
                Some(true) // A big literal can't be empty
            }
            EncodedTerm::FloatLiteral(value) => Some(Boolean::from(value).into()),
            EncodedTerm::DoubleLiteral(value) => Some(Boolean::from(value).into()),
            EncodedTerm::IntegerLiteral(value) => Some(Boolean::from(value).into()),
            EncodedTerm::DecimalLiteral(value) => Some(Boolean::from(value).into()),
            // A typed literal kept as written may still be a boolean or a
            // number ("1"^^xsd:boolean, "8.0e0"^^xsd:double, "5"^^xsd:int):
            // read its value.
            EncodedTerm::SmallTypedLiteral { .. } | EncodedTerm::BigTypedLiteral { .. } => {
                match self.externalize_expression_term(term)? {
                    ExpressionTerm::BooleanLiteral(value) => Some(value.into()),
                    ExpressionTerm::FloatLiteral(value) => Some(Boolean::from(value).into()),
                    ExpressionTerm::DoubleLiteral(value) => Some(Boolean::from(value).into()),
                    ExpressionTerm::IntegerLiteral(value) => Some(Boolean::from(value).into()),
                    ExpressionTerm::DecimalLiteral(value) => Some(Boolean::from(value).into()),
                    _ => None,
                }
            }
            _ => None,
        })
    }
}

impl StrLookup for DatasetView<'_> {
    fn get_str(&self, key: &StrHash) -> Result<Option<String>, StorageError> {
        Ok(if let Some(value) = self.extra.borrow().get(key) {
            Some(value.clone())
        } else {
            self.reader.get_str(key)?
        })
    }
}
