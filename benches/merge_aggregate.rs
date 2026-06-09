use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use lob::LimitOrderBook;
use std::hint::black_box;

fn fixture_book(id: u64, levels: usize) -> LimitOrderBook {
    let bids: Vec<[String; 2]> = (0..levels)
        .map(|i| {
            [
                format!("{:.1}", 50_000.0 - i as f64),
                format!("{:.1}", (i + 1) as f64),
            ]
        })
        .collect();
    let asks: Vec<[String; 2]> = (0..levels)
        .map(|i| {
            [
                format!("{:.1}", 50_001.0 + i as f64),
                format!("{:.1}", (i + 1) as f64),
            ]
        })
        .collect();
    serde_json::from_value(serde_json::json!({
        "lastUpdateId": id,
        "bids": bids,
        "asks": asks,
    }))
    .expect("fixture book")
}

fn merge_aggregate_benches(c: &mut Criterion) {
    let mut group = c.benchmark_group("merge_aggregate");
    for levels in [100usize, 500] {
        let spot = fixture_book(1, levels);
        let usdm = fixture_book(2, levels);
        let owned = vec![spot.clone(), usdm.clone()];
        let borrowed: Vec<&LimitOrderBook> = vec![&spot, &usdm];

        group.bench_with_input(
            BenchmarkId::new("owned_slice", levels),
            &owned,
            |b, books| {
                b.iter(|| black_box(LimitOrderBook::merge_aggregate(books)));
            },
        );
        group.bench_with_input(
            BenchmarkId::new("borrowed_refs", levels),
            &borrowed,
            |b, books| {
                b.iter(|| black_box(LimitOrderBook::merge_aggregate_refs(books)));
            },
        );
    }
    group.finish();
}

criterion_group!(merge, merge_aggregate_benches);
criterion_main!(merge);
