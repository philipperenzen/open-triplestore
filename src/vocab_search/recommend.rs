//! Vocabulary recommender — a Rust port of the CLARIAH
//! `vocabulary-recommender` (Triply B.V., MIT), adapted to run against the
//! internal term search engine instead of remote SPARQL/Elasticsearch
//! endpoints.
//!
//! Given a list of search terms (e.g. the column names of a dataset being
//! mapped to linked data), the recommender:
//!
//! 1. runs each term through the search engine (classes and/or properties),
//! 2. min-max normalizes each term's result scores (as `singleRecommend.ts`
//!    does),
//! 3. applies the **combiSQORE** homogenization from
//!    `homogeneousRecommendation`: score each vocabulary by the mean of its
//!    normalized result scores across all search terms (plus optional user
//!    preference weights), then walk the vocabularies in ascending score
//!    order, dropping each unless dropping it would leave some search term
//!    without any covering vocabulary — yielding a minimal, high-scoring set
//!    of vocabularies that covers every term,
//! 4. returns per-term best results restricted to the kept set
//!    (`instance-focused`) alongside the raw per-term rankings.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use super::corpus::TermType;
use super::index::{Candidate, VocabSearchEngine};

/// What kind of terms a search term should match.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum RecommendCategory {
    Class,
    Property,
    #[default]
    All,
}

impl RecommendCategory {
    fn types(self) -> Vec<TermType> {
        match self {
            RecommendCategory::Class => vec![TermType::Class],
            RecommendCategory::Property => vec![TermType::Property],
            RecommendCategory::All => Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct RecommendTerm {
    pub term: String,
    #[serde(default)]
    pub category: RecommendCategory,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RecommendRequest {
    pub terms: Vec<RecommendTerm>,
    /// Optional vocabulary preference weights (prefix → additive weight),
    /// like the CLARIAH `preferredVocabs` input.
    #[serde(default)]
    pub preferred_vocabs: HashMap<String, f64>,
    /// Results kept per term (default 10).
    #[serde(default)]
    pub per_term_limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScoredResult {
    pub iri: String,
    pub prefixed: String,
    pub label: Option<String>,
    pub description: Option<String>,
    pub ttype: String,
    pub vocab: String,
    /// Min-max normalized score within this term's result set.
    pub score: f64,
    pub source: String,
}

#[derive(Debug, Serialize)]
pub struct TermRecommendation {
    pub search_term: String,
    /// All candidate vocabularies for this term (by prefix).
    pub vocabs: Vec<String>,
    /// Ranked results (normalized scores).
    pub results: Vec<ScoredResult>,
    /// Best result within the homogeneous vocabulary set, if any.
    pub homogeneous_best: Option<ScoredResult>,
}

#[derive(Debug, Serialize)]
pub struct RecommendResponse {
    /// The minimal covering vocabulary set, best-scoring first.
    pub homogeneous_vocabs: Vec<String>,
    pub terms: Vec<TermRecommendation>,
}

fn to_scored(c: &Candidate, norm: f64) -> ScoredResult {
    ScoredResult {
        iri: c.iri.clone(),
        prefixed: c.prefixed.clone(),
        label: c
            .labels
            .lines()
            .next()
            .map(str::to_string)
            .filter(|s| !s.is_empty()),
        description: c
            .secondary
            .lines()
            .next()
            .map(str::to_string)
            .filter(|s| !s.is_empty()),
        ttype: c.ttype.clone(),
        vocab: c.vocab.clone(),
        score: norm,
        source: c.source.clone(),
    }
}

/// Min-max normalize a slice of scores (CLARIAH `singleRecommend.ts`,
/// including its `+0.01` fudge when all scores are equal).
fn normalize(scores: &[f64]) -> Vec<f64> {
    let min = scores.iter().copied().fold(f64::INFINITY, f64::min);
    let max = scores.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    if !min.is_finite() || !max.is_finite() {
        return vec![0.0; scores.len()];
    }
    let range = if (max - min).abs() < f64::EPSILON {
        0.01
    } else {
        max - min
    };
    scores.iter().map(|s| (s - min) / range).collect()
}

/// combiSQORE elimination: walk vocabularies in ascending score order and
/// drop each unless it is the last cover for some term.
fn combi_sqore(vocab_scores: &HashMap<String, f64>, coverage: &[HashSet<String>]) -> Vec<String> {
    let mut kept: HashSet<String> = vocab_scores.keys().cloned().collect();
    let mut ascending: Vec<&String> = vocab_scores.keys().collect();
    ascending.sort_by(|a, b| {
        vocab_scores[*a]
            .partial_cmp(&vocab_scores[*b])
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.cmp(a)) // deterministic: later alphabetical dropped first
    });

    for vocab in ascending {
        // Would dropping `vocab` leave some term with zero covering vocabs?
        let breaks_coverage = coverage.iter().any(|term_vocabs| {
            !term_vocabs.is_empty()
                && term_vocabs.iter().all(|v| v == vocab || !kept.contains(v))
                && term_vocabs.contains(vocab)
        });
        if !breaks_coverage {
            kept.remove(vocab);
        }
    }

    let mut result: Vec<String> = kept.into_iter().collect();
    result.sort_by(|a, b| {
        vocab_scores[b]
            .partial_cmp(&vocab_scores[a])
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.cmp(b))
    });
    result
}

/// Run the full recommendation flow against the search engine.
pub fn recommend(engine: &VocabSearchEngine, request: &RecommendRequest) -> RecommendResponse {
    let per_term = request.per_term_limit.unwrap_or(10).clamp(1, 50);

    // 1. Per-term search + normalization.
    struct TermRun {
        term: String,
        results: Vec<ScoredResult>,
        vocabs: HashSet<String>,
    }
    let mut runs: Vec<TermRun> = Vec::new();
    for t in &request.terms {
        let outcome =
            engine.search_terms(&t.term, &t.category.types(), None, &[], None, 1, per_term);
        let raw_scores: Vec<f64> = outcome.results.iter().map(|c| c.score).collect();
        let normalized = normalize(&raw_scores);
        let results: Vec<ScoredResult> = outcome
            .results
            .iter()
            .zip(normalized)
            .map(|(c, n)| to_scored(c, n))
            .collect();
        let vocabs: HashSet<String> = results.iter().map(|r| r.vocab.clone()).collect();
        runs.push(TermRun {
            term: t.term.clone(),
            results,
            vocabs,
        });
    }

    // 2. Vocabulary scores: mean of normalized scores across all terms,
    //    plus user preference weights (CLARIAH adds them onto the score).
    let mut sums: HashMap<String, (f64, usize)> = HashMap::new();
    for run in &runs {
        for r in &run.results {
            let entry = sums.entry(r.vocab.clone()).or_insert((0.0, 0));
            entry.0 += r.score;
            entry.1 += 1;
        }
    }
    let mut vocab_scores: HashMap<String, f64> = sums
        .into_iter()
        .map(|(vocab, (sum, n))| (vocab, sum / n.max(1) as f64))
        .collect();
    for (vocab, weight) in &request.preferred_vocabs {
        if let Some(score) = vocab_scores.get_mut(vocab) {
            *score += *weight;
        }
    }

    // 3. Minimal covering set.
    let coverage: Vec<HashSet<String>> = runs.iter().map(|r| r.vocabs.clone()).collect();
    let homogeneous = combi_sqore(&vocab_scores, &coverage);
    let kept: HashSet<&String> = homogeneous.iter().collect();

    // 4. Per-term output with the homogeneous best.
    let terms = runs
        .into_iter()
        .map(|run| {
            let mut vocabs: Vec<String> = run.vocabs.iter().cloned().collect();
            vocabs.sort();
            let homogeneous_best = run
                .results
                .iter()
                .filter(|r| kept.contains(&r.vocab))
                .max_by(|a, b| {
                    a.score
                        .partial_cmp(&b.score)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .cloned();
            TermRecommendation {
                search_term: run.term,
                vocabs,
                results: run.results,
                homogeneous_best,
            }
        })
        .collect();

    RecommendResponse {
        homogeneous_vocabs: homogeneous,
        terms,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_minmax_and_flat() {
        let n = normalize(&[1.0, 3.0, 5.0]);
        assert_eq!(n, vec![0.0, 0.5, 1.0]);
        // All-equal: divided by the 0.01 fudge → all zeros.
        let flat = normalize(&[2.0, 2.0]);
        assert_eq!(flat, vec![0.0, 0.0]);
        assert!(normalize(&[]).is_empty());
    }

    #[test]
    fn combi_sqore_keeps_minimal_cover() {
        // Terms: t1 covered by {a, b}, t2 by {b}, t3 by {c}.
        // b covers t1+t2, so a is droppable; c is the only cover for t3.
        let mut scores = HashMap::new();
        scores.insert("a".to_string(), 0.9);
        scores.insert("b".to_string(), 0.8);
        scores.insert("c".to_string(), 0.1);
        let coverage = vec![
            ["a", "b"].iter().map(|s| s.to_string()).collect(),
            ["b"].iter().map(|s| s.to_string()).collect(),
            ["c"].iter().map(|s| s.to_string()).collect(),
        ];
        let kept = combi_sqore(&scores, &coverage);
        assert_eq!(kept, vec!["b".to_string(), "c".to_string()]);
    }

    #[test]
    fn combi_sqore_preference_can_flip_choice() {
        // Both vocabs cover the single term; the higher score survives.
        let coverage: Vec<HashSet<String>> =
            vec![["a", "b"].iter().map(|s| s.to_string()).collect()];
        let mut scores = HashMap::new();
        scores.insert("a".to_string(), 0.5);
        scores.insert("b".to_string(), 0.6);
        assert_eq!(combi_sqore(&scores, &coverage), vec!["b".to_string()]);
        // A preference weight on `a` (as `recommend` applies) flips it.
        scores.insert("a".to_string(), 0.5 + 0.3);
        assert_eq!(combi_sqore(&scores, &coverage), vec!["a".to_string()]);
    }

    #[test]
    fn combi_sqore_empty() {
        assert!(combi_sqore(&HashMap::new(), &[]).is_empty());
    }
}
