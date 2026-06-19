use arc_swap::ArcSwap;
use criterion::{criterion_group, criterion_main, Criterion};
use left_right::{Absorb, ReadHandleFactory};
use lob::{LimitOrderBook, PriceAndQuantity};
use std::hint::black_box;
use std::sync::Arc;

enum BenchOp {
    Append(LimitOrderBook),
}

impl Absorb<BenchOp> for LimitOrderBook {
    fn absorb_first(&mut self, op: &mut BenchOp, _other: &Self) {
        match op {
            BenchOp::Append(book) => {
                *self = std::mem::replace(book, LimitOrderBook::default());
            }
        }
    }

    fn sync_with(&mut self, first: &Self) {
        *self = first.clone();
    }
}

fn fixture_book(update_id: u64) -> LimitOrderBook {
    let mut book = LimitOrderBook::new();
    book.update_id = update_id;
    book.add_bid(PriceAndQuantity(50000.0, 1.0));
    book.add_ask(PriceAndQuantity(50001.0, 2.0));
    book
}

fn make_factories(count: usize) -> Vec<ReadHandleFactory<LimitOrderBook>> {
    (0..count)
        .map(|i| {
            let (mut w, r) = left_right::new::<LimitOrderBook, BenchOp>();
            w.append(BenchOp::Append(fixture_book(i as u64 + 1)));
            w.publish();
            r.factory()
        })
        .collect()
}

fn merge_from_factories(factories: &[ReadHandleFactory<LimitOrderBook>]) -> LimitOrderBook {
    let mut merged = LimitOrderBook::new();
    for factory in factories {
        if let Some(book) = factory.handle().enter() {
            merged.merge_aggregate_absorb(&book);
        }
    }
    merged.update_id = factories
        .iter()
        .filter_map(|fac| fac.handle().enter().map(|g| g.update_id))
        .max()
        .unwrap_or(0);
    merged
}

fn bench_merge_refresh(c: &mut Criterion) {
    let factories = make_factories(4);
    let swap = Arc::new(ArcSwap::from_pointee(LimitOrderBook::new()));

    c.bench_function("merge_aggregate from read guards (no clone)", |b| {
        b.iter(|| {
            let handles: Vec<_> = factories.iter().map(|f| f.handle()).collect();
            let guards: Vec<_> = handles.iter().filter_map(|h| h.enter()).collect();
            black_box(LimitOrderBook::merge_aggregate(guards.iter().map(|g| &**g)));
        });
    });

    c.bench_function("merge_aggregate after full-book clone (old path)", |b| {
        b.iter(|| {
            let snapshots: Vec<LimitOrderBook> = factories
                .iter()
                .filter_map(|f| f.handle().enter().map(|g| g.clone()))
                .collect();
            black_box(LimitOrderBook::merge_aggregate(&snapshots));
        });
    });

    c.bench_function("merged lane ArcSwap store after merge", |b| {
        b.iter(|| {
            let merged = merge_from_factories(&factories);
            swap.store(Arc::new(merged));
        });
    });

    c.bench_function("merged lane ArcSwap load (read path)", |b| {
        swap.store(Arc::new(merge_from_factories(&factories)));
        b.iter(|| black_box(swap.load_full()));
    });
}

criterion_group!(merge_refresh, bench_merge_refresh);
criterion_main!(merge_refresh);
