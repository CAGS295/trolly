use criterion::{black_box, criterion_group, criterion_main, Criterion};
use hyper::body::to_bytes;
use hyper::Body;
use hyper::Client;
use hyper::Uri;
use lob::Decode;
use lob::LimitOrderBook;
use std::time::Duration;

fn random_gets(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("Failed building the Runtime");
    let client = Client::builder()
        .http2_only(true)
        .http2_keep_alive_interval(Some(Duration::from_millis(20)))
        .build_http::<Body>();

    c.bench_function("depth scale", |b| {
        b.iter(|| {
            let future = client.get(Uri::from_static("http://[::1]:50051/scale/depth/btcusdt"));
            let response = rt.block_on(future).unwrap();
            let body = response.into_body();
            let bytes = rt.block_on(to_bytes(body)).unwrap();
            let mut bytes = &bytes[..];
            let book_snapshot = LimitOrderBook::decode(&mut bytes).unwrap();

            black_box(book_snapshot);
        })
    });
}

criterion_group!(depth, random_gets);
criterion_main!(depth);
