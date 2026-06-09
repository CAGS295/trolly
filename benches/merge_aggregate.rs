use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use lob::LimitOrderBook;

fn sample_book(levels: usize, update_id: u64, bid_offset: f64) -> LimitOrderBook {
    let bids: Vec<[String; 2]> = (0..levels)
        .map(|i| {
            [
                format!("{:.1}", 100.0 + bid_offset + i as f64),
                "1.0".into(),
            ]
        })
        .collect();
    let asks: Vec<[String; 2]> = (0..levels)
        .map(|i| {
            [
                format!("{:.1}", 200.0 + bid_offset + i as f64),
                "1.0".into(),
            ]
        })
        .collect();
    serde_json::from_value(serde_json::json!({
        "lastUpdateId": update_id,
        "bids": bids,
        "asks": asks,
    }))
    .expect("fixture book")
}

fn merge_aggregate_benches(c: &mut Criterion) {
    let mut group = c.benchmark_group("merge_aggregate");
    for levels in [100usize, 500] {
        let books: Vec<LimitOrderBook> = (0..4)
            .map(|i| sample_book(levels, i as u64 + 1, i as f64 * 0.01))
            .collect();

        group.bench_with_input(
            BenchmarkId::new("clone_then_merge", levels),
            &books,
            |b, books| {
                b.iter(|| {
                    let snapshots: Vec<LimitOrderBook> =
                        books.iter().map(|book| book.clone()).collect();
                    black_box(LimitOrderBook::merge_aggregate(&snapshots))
                });
            },
        );

        group.bench_with_input(
            BenchmarkId::new("merge_by_reference", levels),
            &books,
            |b, books| {
                b.iter(|| {
                    let mut merged = LimitOrderBook::new();
                    let mut max_update_id = 0;
                    for book in books {
                        merged.merge_aggregate_from(book);
                        max_update_id = max_update_id.max(book.update_id);
                    }
                    merged.update_id = max_update_id;
                    black_box(merged)
                });
            },
        );

        group.bench_with_input(
            BenchmarkId::new("merge_aggregate_refs", levels),
            &books,
            |b, books| {
                b.iter(|| {
                    let refs: Vec<&LimitOrderBook> = books.iter().collect();
                    black_box(LimitOrderBook::merge_aggregate_refs(&refs))
                });
            },
        );
    }
    group.finish();
}

criterion_group!(benches, merge_aggregate_benches);
criterion_main!(benches);
