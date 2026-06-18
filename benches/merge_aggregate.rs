use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use lob::{LimitOrderBook, PriceAndQuantity};
use std::hint::black_box;

fn fixture_book(levels: usize, update_id: u64) -> LimitOrderBook {
    let mut book = LimitOrderBook::new();
    book.update_id = update_id;
    for i in 0..levels {
        let price = 50_000.0 + i as f64;
        book.add_bid(PriceAndQuantity(price, 1.0));
        book.add_ask(PriceAndQuantity(price + 0.5, 2.0));
    }
    book
}

fn merge_with_full_clones(books: &[LimitOrderBook]) -> LimitOrderBook {
    let snapshots: Vec<_> = books.iter().cloned().collect();
    LimitOrderBook::merge_aggregate(&snapshots)
}

fn merge_with_borrowed_refs(books: &[LimitOrderBook]) -> LimitOrderBook {
    let mut merged = LimitOrderBook::new();
    for book in books {
        merged.merge_into(book);
    }
    merged
}

fn merge_aggregate_bench(c: &mut Criterion) {
    let mut group = c.benchmark_group("merge_aggregate");
    for &(sources, levels) in &[(2usize, 100usize), (4, 250)] {
        let books: Vec<_> = (0..sources)
            .map(|i| fixture_book(levels, i as u64 + 1))
            .collect();
        let id = format!("{sources}x{levels}");
        group.bench_with_input(BenchmarkId::new("clone_each_source", &id), &books, |b, books| {
            b.iter(|| black_box(merge_with_full_clones(black_box(books))));
        });
        group.bench_with_input(BenchmarkId::new("incremental_borrow", &id), &books, |b, books| {
            b.iter(|| black_box(merge_with_borrowed_refs(black_box(books))));
        });
    }
    group.finish();
}

criterion_group!(merge_benches, merge_aggregate_bench);
criterion_main!(merge_benches);
