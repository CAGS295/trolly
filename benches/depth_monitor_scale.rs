use criterion::{black_box, criterion_group, criterion_main, Criterion};
use flate2::read::GzDecoder;
use http::header::ACCEPT_ENCODING;
use http::Request;
use hyper::body::to_bytes;
use hyper::Body;
use hyper::Client;
use lob::Decode;
use lob::LimitOrderBook;
use std::io::Read;
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
            let req = Request::builder()
                .header(ACCEPT_ENCODING, "gzip")
                .method("GET")
                .uri("http://[::1]:50051/scale/depth/btcusdt")
                .body(Body::default())
                .unwrap();
            let response = rt.block_on(client.request(req)).unwrap();
            let body = response.into_body();
            let bytes = rt.block_on(to_bytes(body)).unwrap();
            let mut x = Vec::new();
            GzDecoder::new(&bytes[..]).read_to_end(&mut x).unwrap();

            let book_snapshot = LimitOrderBook::decode(&mut x.as_slice()).unwrap();

            black_box(book_snapshot);
        })
    });
}

criterion_group!(depth, random_gets);
criterion_main!(depth);
