use axum::body::Body;
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use flate2::read::GzDecoder;
use http::header::ACCEPT_ENCODING;
use http::Request;
use http::Response;
use http_body_util::BodyExt;
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;
use hyper_util::rt::TokioTimer;
use lob::Decode;
use lob::LimitOrderBook;
use std::{io::Read, time::Duration};

fn random_gets(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("Failed building the Runtime");
    let client = Client::builder(TokioExecutor::new())
        .http2_only(true)
        .http2_keep_alive_interval(Some(Duration::from_millis(200)))
        .timer(TokioTimer::new())
        .build_http();

    c.bench_function("depth scale", |b| {
        b.iter(|| {
            let req = Request::builder()
                .header(ACCEPT_ENCODING, "gzip")
                .method("GET")
                .uri("http://[::1]:50051/scale/depth/btcusdt")
                .body(Body::empty())
                .unwrap();
            let response: Response<_> = rt.block_on(client.request(req)).unwrap();
            let body = response.into_body();
            let bytes: Vec<u8> = rt.block_on(body.collect()).unwrap().to_bytes().into();
            let mut buf = Vec::new();
            GzDecoder::new(&bytes[..]).read_to_end(&mut buf).unwrap();

            let book_snapshot = LimitOrderBook::decode(&mut buf.as_slice()).unwrap();

            black_box(book_snapshot);
        })
    });
}

criterion_group!(depth, random_gets);
criterion_main!(depth);
