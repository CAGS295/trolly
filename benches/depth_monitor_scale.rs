use criterion::{black_box, criterion_group, criterion_main, Criterion};
use hyper::body::to_bytes;
use hyper::Body;
use hyper::Client;
use hyper::Uri;
use lob::Decode;
use lob::LimitOrderBook;

fn random_probes(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("Failed building the Runtime");
    let client = Client::builder().http2_only(true).build_http::<Body>();

    c.bench_function("random_probes", |b| {
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

criterion_group!(benches, random_probes);
criterion_main!(benches);
