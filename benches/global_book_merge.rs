use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use lob::LimitOrderBook;
use std::hint::black_box;

fn make_book(n_levels: usize, update_id: u64) -> LimitOrderBook {
    let json_bids: Vec<String> = (0..n_levels)
        .map(|i| {
            let price = 50000.0 - i as f64;
            let qty = 1.0 + (i as f64 * 0.1);
            format!("[\"{price:.1}\",\"{qty:.4}\"]")
        })
        .collect();
    let json_asks: Vec<String> = (0..n_levels)
        .map(|i| {
            let price = 50001.0 + i as f64;
            let qty = 1.0 + (i as f64 * 0.1);
            format!("[\"{price:.1}\",\"{qty:.4}\"]")
        })
        .collect();
    let json = format!(
        r#"{{"lastUpdateId":{update_id},"bids":[{}],"asks":[{}]}}"#,
        json_bids.join(","),
        json_asks.join(","),
    );
    serde_json::from_str(&json).expect("valid LOB json")
}

fn bench_merge_aggregate(c: &mut Criterion) {
    let mut group = c.benchmark_group("merge_aggregate");
    for n_sources in [2, 4, 8] {
        let books: Vec<LimitOrderBook> = (0..n_sources)
            .map(|i| make_book(100, i as u64 + 1))
            .collect();

        group.bench_with_input(
            BenchmarkId::new("owned_slice", n_sources),
            &books,
            |b, books| {
                b.iter(|| black_box(LimitOrderBook::merge_aggregate(books)));
            },
        );

        let refs: Vec<&LimitOrderBook> = books.iter().collect();
        group.bench_with_input(
            BenchmarkId::new("refs_no_clone", n_sources),
            &refs,
            |b, refs| {
                b.iter(|| black_box(LimitOrderBook::merge_aggregate_refs(refs)));
            },
        );
    }
    group.finish();
}

fn bench_refresh_merged_simulation(c: &mut Criterion) {
    let mut group = c.benchmark_group("refresh_merged_sim");
    for n_sources in [2, 4] {
        let books: Vec<LimitOrderBook> = (0..n_sources)
            .map(|i| make_book(200, i as u64 + 1))
            .collect();

        group.bench_with_input(
            BenchmarkId::new("clone_then_merge", n_sources),
            &books,
            |b, books| {
                b.iter(|| {
                    let cloned: Vec<LimitOrderBook> = books.iter().map(|b| b.clone()).collect();
                    black_box(LimitOrderBook::merge_aggregate(&cloned));
                });
            },
        );

        let refs: Vec<&LimitOrderBook> = books.iter().collect();
        group.bench_with_input(
            BenchmarkId::new("refs_direct", n_sources),
            &refs,
            |b, refs| {
                b.iter(|| {
                    black_box(LimitOrderBook::merge_aggregate_refs(refs));
                });
            },
        );
    }
    group.finish();
}

criterion_group!(benches, bench_merge_aggregate, bench_refresh_merged_simulation);
criterion_main!(benches);
