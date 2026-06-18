use criterion::{criterion_group, criterion_main, Criterion};
use left_right::{Absorb, ReadHandleFactory};
use lob::{LimitOrderBook, PriceAndQuantity};
use std::hint::black_box;

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

fn bench_merge_refresh(c: &mut Criterion) {
    let factories = make_factories(4);

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
}

criterion_group!(merge_refresh, bench_merge_refresh);
criterion_main!(merge_refresh);
