//! Throwaway probe for the scale work: times query variants and graph
//! operations on a scale_otl dump. `scale_probe <turtle-file> <data_dir>`
use open_triplestore::store::TripleStore;
use oxigraph::io::RdfFormat;
use oxigraph::sparql::QueryResults;
use std::path::Path;
use std::time::Instant;

const G: &str = "https://example.org/otl/instances";
const EX: &str = "https://example.org/otl/";

fn consume(r: QueryResults) -> usize {
    match r {
        QueryResults::Solutions(s) => s.count(),
        QueryResults::Boolean(_) => 1,
        QueryResults::Graph(g) => g.count(),
    }
}
fn timed(store: &TripleStore, label: &str, q: &str) {
    let _ = consume(store.query(q).expect("warm"));
    let mut ms = Vec::new();
    let mut n = 0;
    for _ in 0..3 {
        let t = Instant::now();
        n = consume(store.query(q).expect("query"));
        ms.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    println!("{label:38} {:9.1} ms  ({n} rows)", ms[1]);
}

fn main() {
    let file = std::env::args().nth(1).expect("file");
    let dir = std::env::args().nth(2).expect("dir");
    let store = TripleStore::open(Path::new(&dir)).expect("open");
    let text = std::fs::read_to_string(&file).expect("read");
    let t = Instant::now();
    store
        .load_str(&text, RdfFormat::Turtle, Some(G))
        .expect("load named");
    println!(
        "load into <G>: {:.1}s ({} quads)",
        t.elapsed().as_secs_f64(),
        store.count_graph(Some(G)).unwrap()
    );
    // Also a default-graph copy for the no-GRAPH variants.
    let t = Instant::now();
    store
        .load_str(&text, RdfFormat::Turtle, None)
        .expect("load default");
    println!("load into default: {:.1}s", t.elapsed().as_secs_f64());
    // Trigger the mirror build with a non-fast-path query, then wait for it.
    let group_d0 =
        format!("SELECT ?t (COUNT(?a) AS ?c) WHERE {{ ?a a ?t ; <{EX}length> ?l }} GROUP BY ?t");
    std::thread::sleep(std::time::Duration::from_millis(800));
    let _ = consume(store.query(&group_d0).unwrap());
    let t = Instant::now();
    while store.parallel_build_count() == 0 && t.elapsed().as_secs() < 90 {
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    println!(
        "mirror builds: {} (waited {:.1}s)",
        store.parallel_build_count(),
        t.elapsed().as_secs_f64()
    );

    let group_g = format!("SELECT ?t (COUNT(?a) AS ?c) (AVG(?l) AS ?avg) WHERE {{ GRAPH <{G}> {{ ?a a ?t ; <{EX}length> ?l }} }} GROUP BY ?t");
    let group_d = format!("SELECT ?t (COUNT(?a) AS ?c) (AVG(?l) AS ?avg) WHERE {{ ?a a ?t ; <{EX}length> ?l }} GROUP BY ?t");
    let group_g_count = format!("SELECT ?t (COUNT(?a) AS ?c) WHERE {{ GRAPH <{G}> {{ ?a a ?t ; <{EX}length> ?l }} }} GROUP BY ?t");
    let group_d_count =
        format!("SELECT ?t (COUNT(?a) AS ?c) WHERE {{ ?a a ?t ; <{EX}length> ?l }} GROUP BY ?t");
    let count_g = format!("SELECT (COUNT(*) AS ?c) WHERE {{ GRAPH <{G}> {{ ?s ?p ?o }} }}");
    let count_gv = "SELECT (COUNT(*) AS ?c) WHERE { GRAPH ?g { ?s ?p ?o } }".to_string();
    let join_g = format!("SELECT ?a ?pn WHERE {{ GRAPH <{G}> {{ ?a <{EX}partOf> ?p . ?p <{EX}name> ?pn }} }} LIMIT 10000");
    let join_d =
        format!("SELECT ?a ?pn WHERE {{ ?a <{EX}partOf> ?p . ?p <{EX}name> ?pn }} LIMIT 10000");
    timed(&store, "group_by AVG in GRAPH", &group_g);
    timed(&store, "group_by AVG default graph", &group_d);
    timed(&store, "group_by COUNT in GRAPH", &group_g_count);
    timed(&store, "group_by COUNT default graph", &group_d_count);
    timed(&store, "COUNT(*) in GRAPH <g>", &count_g);
    timed(&store, "COUNT(*) in GRAPH ?g", &count_gv);
    timed(&store, "join in GRAPH", &join_g);
    timed(&store, "join default graph", &join_d);
    println!("mirror builds: {}", store.parallel_build_count());

    // Scoped forms, as the HTTP path rewrites them (FROM / FROM NAMED): are they still shardable?
    for (label, q) in [
        ("plain GRAPH group_by", group_g.clone()),
        ("FROM NAMED + GRAPH group_by", format!("SELECT ?t (COUNT(?a) AS ?c) (AVG(?l) AS ?avg) FROM NAMED <{G}> WHERE {{ GRAPH <{G}> {{ ?a a ?t ; <{EX}length> ?l }} }} GROUP BY ?t")),
        ("FROM + FROM NAMED + GRAPH group_by", format!("SELECT ?t (COUNT(?a) AS ?c) (AVG(?l) AS ?avg) FROM <urn:x> FROM NAMED <{G}> WHERE {{ GRAPH <{G}> {{ ?a a ?t ; <{EX}length> ?l }} }} GROUP BY ?t")),
    ] {
        println!("decomposable? {label:38} {}", opengraph::parallel::is_decomposable(&q));
        timed(&store, label, &q);
    }

    // SHACL over the graph (shapes in their own graph).
    let shapes =
        std::fs::read_to_string(std::env::args().nth(3).unwrap_or_default()).unwrap_or_default();
    if !shapes.is_empty() {
        store
            .load_str(&shapes, RdfFormat::Turtle, Some("urn:shapes"))
            .expect("shapes");
        // Runs 0 and 1 start right after the shapes load, so the accelerator is
        // dirty and the engine reads one RocksDB snapshot. Run 2 waits for the
        // background tick to publish a clean RAM copy and reads that instead —
        // the two data paths a server run can take.
        for i in 0..3 {
            if i == 2 {
                let builds = store.parallel_build_count();
                let t = Instant::now();
                while store.parallel_build_count() == builds && t.elapsed().as_secs() < 120 {
                    store.accelerator_tick();
                    std::thread::sleep(std::time::Duration::from_millis(200));
                }
                println!(
                    "mirror rebuilt for run 2: {} (waited {:.1}s)",
                    store.parallel_build_count() > builds,
                    t.elapsed().as_secs_f64()
                );
            }
            let t = Instant::now();
            let r =
                open_triplestore::shacl::engine::validate(&store, "urn:shapes", &[G.to_string()])
                    .expect("shacl");
            println!(
                "SHACL run {i}: {:.2}s ({} results)",
                t.elapsed().as_secs_f64(),
                r.results.len()
            );
        }
    }

    // Graph clear costs: the current path, then alternatives.
    let t = Instant::now();
    store.graph_store_delete(Some(G)).expect("delete");
    println!(
        "graph_store_delete <G> (clear + remove): {:.1}s",
        t.elapsed().as_secs_f64()
    );
    store
        .load_str(&text, RdfFormat::Turtle, Some(G))
        .expect("reload");
    let g = oxigraph::model::NamedNodeRef::new(G).unwrap();
    let t = Instant::now();
    store
        .store()
        .remove_named_graph(g)
        .expect("remove_named_graph");
    println!(
        "remove_named_graph alone: {:.1}s",
        t.elapsed().as_secs_f64()
    );
    store
        .load_str(&text, RdfFormat::Turtle, Some(G))
        .expect("reload");
    let t = Instant::now();
    store
        .store()
        .clear_graph(oxigraph::model::GraphNameRef::NamedNode(g))
        .expect("clear_graph");
    println!("clear_graph alone: {:.1}s", t.elapsed().as_secs_f64());
    store
        .load_str(&text, RdfFormat::Turtle, Some(G))
        .expect("reload");
    let t = Instant::now();
    let quads: Vec<oxigraph::model::Quad> = store
        .store()
        .quads_for_pattern(
            None,
            None,
            None,
            Some(oxigraph::model::GraphNameRef::NamedNode(g)),
        )
        .map(|q| q.unwrap())
        .collect();
    let collect_s = t.elapsed().as_secs_f64();
    for chunk in quads.chunks(50_000) {
        let mut tx = store.store().start_transaction().expect("tx");
        for q in chunk {
            tx.remove(q.as_ref());
        }
        tx.commit().expect("commit");
    }
    println!(
        "collect ({collect_s:.1}s) + chunked transaction remove: {:.1}s total",
        t.elapsed().as_secs_f64()
    );
}
