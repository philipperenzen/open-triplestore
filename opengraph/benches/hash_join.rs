use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use opengraph::hash_join::{HashJoin, Row};
use std::collections::HashMap;

fn make_rows(n: usize, key_range: usize) -> Vec<Row> {
    (0..n)
        .map(|i| {
            let mut row = HashMap::new();
            row.insert("x".to_string(), (i % key_range).to_string());
            row.insert("y".to_string(), i.to_string());
            row
        })
        .collect()
}

fn bench_hash_join(c: &mut Criterion) {
    let mut group = c.benchmark_group("hash_join");
    for size in [100usize, 1_000, 10_000] {
        let build = make_rows(size, size / 2);
        let probe = make_rows(size * 10, size / 2);
        let join_vars = vec!["x".to_string()];

        group.bench_with_input(BenchmarkId::new("join_size", size), &size, |b, _| {
            b.iter(|| {
                HashJoin::join(
                    black_box(build.clone()),
                    black_box(probe.clone()),
                    black_box(&join_vars),
                )
            })
        });
    }
    group.finish();
}

criterion_group!(benches, bench_hash_join);
criterion_main!(benches);
