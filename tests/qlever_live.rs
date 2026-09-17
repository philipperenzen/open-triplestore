//! The live head-to-head against a running QLever: speed, and fidelity.
//!
//! Ignored by default — it needs a QLever server holding the same data, which
//! no CI job has. Bring one up as `docs/operations.md` describes, then:
//!
//! ```text
//! OTS_QLEVER_URL=http://localhost:7011 \
//!   cargo test --features full,test-utils --test qlever_live -- --ignored --nocapture
//! ```
//!
//! The fixture is the one `benches/performance.rs` uses (`gen_persons_ttl`),
//! so the timings can be read beside the paired benchmark table in
//! `docs/performance.md`.
//!
//! Two questions, in this order:
//!
//! 1. **Does QLever answer the same?** The platform's answer must not depend
//!    on whether a QLever backend happens to be configured. Every query is run
//!    through both and the solutions compared as multisets. A divergence here
//!    is a standards-compliance problem, not a performance note.
//! 2. **Is it faster?** Only worth asking of the queries that agree.

use std::time::{Duration, Instant};

use open_triplestore::store::qlever::{HttpQlever, QleverEndpoint};
use open_triplestore::store::TripleStore;
use oxigraph::io::RdfFormat;
use oxigraph::sparql::QueryResults;
use oxrdf::Term;

const N: usize = 100_000;
const RUNS: usize = 5;

/// The benchmark's own fixture, verbatim.
fn persons(n: usize) -> String {
    let mut s = String::from(
        "@prefix ex: <http://example.org/> .\n\
         @prefix xsd: <http://www.w3.org/2001/XMLSchema#> .\n",
    );
    for i in 0..n {
        let kind = i % 10;
        let age = 18 + (i % 65);
        let score = (i as f64 * 7.13) % 100.0;
        s.push_str(&format!(
            "ex:p{i} ex:name \"Person {i}\" ; \
             ex:age {age} ; \
             ex:type ex:Type{kind} ; \
             ex:score {score:.2} ; \
             ex:email \"person{i}@example.org\" .\n"
        ));
    }
    for i in 0..1000 {
        s.push_str(&format!("ex:n{i} ex:next ex:n{} .\n", i + 1));
    }
    s
}

fn render(t: &Term) -> String {
    match t {
        Term::BlankNode(_) => "_:".to_string(),
        other => other.to_string(),
    }
}

/// A header, and the solutions as rendered rows.
type Answer = (Vec<String>, Vec<Vec<Option<String>>>);

/// A query's answer as a sorted multiset of rendered rows, plus its header.
fn normalise(r: QueryResults<'_>) -> Result<Answer, String> {
    match r {
        QueryResults::Solutions(s) => {
            let vars: Vec<String> = s
                .variables()
                .iter()
                .map(|v| v.as_str().to_string())
                .collect();
            let mut rows = Vec::new();
            for sol in s {
                let sol = sol.map_err(|e| e.to_string())?;
                rows.push(
                    vars.iter()
                        .map(|v| sol.get(v.as_str()).map(render))
                        .collect::<Vec<_>>(),
                );
            }
            rows.sort();
            Ok((vars, rows))
        }
        QueryResults::Boolean(b) => Ok((vec!["ask".into()], vec![vec![Some(b.to_string())]])),
        QueryResults::Graph(g) => {
            let mut rows = Vec::new();
            for t in g {
                let t = t.map_err(|e| e.to_string())?;
                rows.push(vec![
                    Some(t.subject.to_string()),
                    Some(t.predicate.to_string()),
                    Some(render(&t.object)),
                ]);
            }
            rows.sort();
            Ok((vec!["s".into(), "p".into(), "o".into()], rows))
        }
    }
}

fn median(mut v: Vec<Duration>) -> Duration {
    v.sort();
    v[v.len() / 2]
}

/// Every query shape the platform's own suites and benchmarks use.
const QUERIES: &[(&str, &str)] = &[
    ("lookup", "SELECT ?s ?name WHERE { ?s ex:name ?name }"),
    (
        "lookup_limit",
        "SELECT ?s ?name WHERE { ?s ex:name ?name } LIMIT 10",
    ),
    (
        "join_2way",
        "SELECT ?name ?age WHERE { ?s ex:name ?name . ?s ex:age ?age }",
    ),
    (
        "join_3way",
        "SELECT ?name ?age ?t WHERE { ?s ex:name ?name . ?s ex:age ?age . ?s ex:type ?t }",
    ),
    (
        "filter",
        "SELECT ?s WHERE { ?s ex:age ?age FILTER(?age > 70) }",
    ),
    (
        "filter_decimal",
        "SELECT ?s WHERE { ?s ex:score ?sc FILTER(?sc > 99.0) }",
    ),
    ("count_star", "SELECT (COUNT(*) AS ?n) WHERE { ?s ?p ?o }"),
    (
        "count_var",
        "SELECT (COUNT(?s) AS ?n) WHERE { ?s ex:name ?name }",
    ),
    (
        "group_by",
        "SELECT ?t (COUNT(?s) AS ?n) WHERE { ?s ex:type ?t } GROUP BY ?t",
    ),
    (
        "group_sum_avg",
        "SELECT ?t (SUM(?age) AS ?s) (AVG(?age) AS ?a) WHERE { ?p ex:type ?t ; ex:age ?age } GROUP BY ?t",
    ),
    (
        "group_min_max",
        "SELECT ?t (MIN(?age) AS ?lo) (MAX(?age) AS ?hi) WHERE { ?p ex:type ?t ; ex:age ?age } GROUP BY ?t",
    ),
    (
        "count_distinct",
        "SELECT (COUNT(DISTINCT ?t) AS ?n) WHERE { ?s ex:type ?t }",
    ),
    (
        "order_limit",
        "SELECT ?s ?age WHERE { ?s ex:age ?age } ORDER BY DESC(?age) ?s LIMIT 10",
    ),
    (
        "optional",
        "SELECT ?s ?e WHERE { ?s ex:name ?n OPTIONAL { ?s ex:email ?e } } LIMIT 100",
    ),
    (
        "union",
        "SELECT ?s WHERE { { ?s ex:type ex:Type0 } UNION { ?s ex:type ex:Type1 } }",
    ),
    (
        "minus",
        "SELECT ?s WHERE { ?s ex:name ?n MINUS { ?s ex:type ex:Type0 } }",
    ),
    (
        "subquery",
        "SELECT ?t ?n WHERE { { SELECT ?t (COUNT(?s) AS ?n) WHERE { ?s ex:type ?t } GROUP BY ?t } } ORDER BY ?t",
    ),
    (
        "bind",
        "SELECT ?s ?d WHERE { ?s ex:age ?age BIND(?age * 2 AS ?d) } LIMIT 100",
    ),
    (
        "values",
        "SELECT ?s ?t WHERE { VALUES ?t { ex:Type0 ex:Type3 } ?s ex:type ?t }",
    ),
    (
        "distinct",
        "SELECT DISTINCT ?t WHERE { ?s ex:type ?t }",
    ),
    ("ask_true", "ASK { ?s ex:name \"Person 7\" }"),
    ("ask_false", "ASK { ?s ex:name \"Nobody\" }"),
    (
        "str_fn",
        "SELECT ?s (STRLEN(?n) AS ?l) (UCASE(?n) AS ?u) WHERE { ?s ex:name ?n } LIMIT 50",
    ),
    (
        "regex",
        "SELECT ?s WHERE { ?s ex:name ?n FILTER(REGEX(?n, \"^Person 1[0-9]$\")) }",
    ),
    (
        "path_seq",
        "SELECT ?a ?c WHERE { ?a ex:next/ex:next ?c } LIMIT 100",
    ),
    (
        "path_star",
        "SELECT ?b WHERE { ex:n0 ex:next* ?b } LIMIT 100",
    ),
    (
        "lang_datatype",
        "SELECT ?s (DATATYPE(?age) AS ?d) WHERE { ?s ex:age ?age } LIMIT 10",
    ),
    (
        "arithmetic",
        "SELECT (SUM(?age) AS ?total) WHERE { ?s ex:age ?age }",
    ),
    (
        "construct",
        "CONSTRUCT { ?s ex:label ?n } WHERE { ?s ex:name ?n FILTER(?s = ex:p3) }",
    ),
];

const PREFIX: &str =
    "PREFIX ex: <http://example.org/> PREFIX xsd: <http://www.w3.org/2001/XMLSchema#> ";

#[test]
#[ignore = "needs a running QLever holding the same data; see the module docs"]
fn qlever_head_to_head() {
    let url = std::env::var("OTS_QLEVER_URL").unwrap_or_else(|_| {
        panic!("set OTS_QLEVER_URL to the QLever endpoint holding the same fixture")
    });
    let token = std::env::var("OTS_QLEVER_ACCESS_TOKEN").ok();
    let remote = HttpQlever::new(&url, token.as_deref(), Duration::from_secs(120));

    eprintln!("loading {N} persons into the local store ...");
    let store = TripleStore::in_memory()
        .unwrap()
        .with_query_cache(false, 0, 0)
        .with_parallel_query(false, 1, 0);
    let t0 = Instant::now();
    store
        .load_str(&persons(N), RdfFormat::Turtle, None)
        .unwrap();
    eprintln!(
        "loaded {} triples in {:?}\n",
        store.len().unwrap(),
        t0.elapsed()
    );

    let mut agree = Vec::new();
    let mut differ = Vec::new();
    let mut refused = Vec::new();

    for (name, body) in QUERIES {
        let q = format!("{PREFIX}{body}");

        // The platform's own answer, and its time.
        let mut ours = Vec::new();
        let mut our_answer = None;
        for _ in 0..RUNS {
            let t = Instant::now();
            let r = store.query(&q).expect("the engine must answer");
            let n = normalise(r).expect("the engine's answer must read");
            ours.push(t.elapsed());
            our_answer = Some(n);
        }
        let our_answer = our_answer.unwrap();

        // QLever's answer, and its time.
        let mut theirs = Vec::new();
        let mut their_answer = None;
        let mut error = None;
        for _ in 0..RUNS {
            let t = Instant::now();
            match remote.query(&q) {
                Ok(r) => {
                    let elapsed = t.elapsed();
                    match normalise(r) {
                        Ok(n) => {
                            theirs.push(elapsed);
                            their_answer = Some(n);
                        }
                        Err(e) => error = Some(format!("unreadable answer: {e}")),
                    }
                }
                Err(e) => error = Some(e),
            }
        }

        let Some(their_answer) = their_answer else {
            refused.push((*name, error.unwrap_or_else(|| "no answer".into())));
            continue;
        };

        let ours_ms = median(ours).as_secs_f64() * 1000.0;
        let theirs_ms = median(theirs).as_secs_f64() * 1000.0;

        if our_answer == their_answer {
            agree.push((*name, ours_ms, theirs_ms, our_answer.1.len()));
        } else {
            differ.push((
                *name,
                ours_ms,
                theirs_ms,
                our_answer.clone(),
                their_answer.clone(),
            ));
        }
    }

    // ── Fidelity first ──────────────────────────────────────────────────
    println!("\n=== FIDELITY ===");
    println!(
        "{} of {} queries answered identically; {} differently; {} refused by QLever.\n",
        agree.len(),
        QUERIES.len(),
        differ.len(),
        refused.len()
    );
    for (name, _, _, ours, theirs) in &differ {
        println!("--- {name}: DIFFERENT");
        println!("    header  ours={:?}  qlever={:?}", ours.0, theirs.0);
        println!(
            "    rows    ours={}  qlever={}",
            ours.1.len(),
            theirs.1.len()
        );
        let show = |rows: &Vec<Vec<Option<String>>>| {
            rows.iter()
                .take(3)
                .map(|r| {
                    r.iter()
                        .map(|c| c.clone().unwrap_or_else(|| "UNBOUND".into()))
                        .collect::<Vec<_>>()
                        .join(" | ")
                })
                .collect::<Vec<_>>()
                .join("\n              ")
        };
        println!("    ours:     {}", show(&ours.1));
        println!("    qlever:   {}", show(&theirs.1));
    }
    for (name, why) in &refused {
        println!(
            "--- {name}: REFUSED by QLever ({})",
            why.lines().next().unwrap_or("")
        );
    }

    // ── Then speed, for the queries that agree ──────────────────────────
    println!("\n=== SPEED (median of {RUNS}, queries that agree) ===");
    println!("| query | rows | engine | QLever | QLever is |");
    println!("|---|--:|--:|--:|--:|");
    for (name, ours_ms, theirs_ms, rows) in &agree {
        let factor = ours_ms / theirs_ms.max(0.0001);
        let verdict = if factor >= 1.0 {
            format!("{factor:.1}x faster")
        } else {
            format!("{:.1}x slower", 1.0 / factor)
        };
        println!("| `{name}` | {rows} | {ours_ms:.1} ms | {theirs_ms:.1} ms | {verdict} |");
    }
    println!();

    // The fidelity result is the one that decides whether this backend may be
    // consulted by default, so make it the assertion.
    assert!(
        differ.is_empty(),
        "{} of {} queries answered differently from the engine — see the report above",
        differ.len(),
        QUERIES.len()
    );
}
