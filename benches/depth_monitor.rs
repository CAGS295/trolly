use criterion::{black_box, criterion_group, criterion_main, Criterion};
use tonic::Request;
use trolly::grpc::LimitOrderBookServiceClient as Client;
use trolly::grpc::Pair;

fn random_gets(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("Failed building the Runtime");
    let mut client = rt
        .block_on(Client::connect("http://[::1]:50051"))
        .expect("The binary is running and serving.");
    let pair = "btcusdt".to_string();

    c.bench_function("depth grpc", |b| {
        b.iter(|| {
            let book_snapshot = rt
                .block_on(client.get_limit_order_book(Request::new(Pair { pair: pair.clone() })))
                .unwrap()
                .into_inner();

            black_box(book_snapshot);
        })
    });
}

criterion_group!(depth, random_gets);
criterion_main!(depth);
