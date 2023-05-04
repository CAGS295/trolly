use rand::rngs::SmallRng;
use rand::{Rng, SeedableRng};
use std::time::Duration;
use std::{println, thread};
use tonic::Request;
use tracing::Level;
use tracing_subscriber::EnvFilter;
use trolly::grpc::{LimitOrderBookServiceClient as Client, Pair};
use trolly::tokio::main;

#[main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let filter = EnvFilter::try_from_default_env().or_else(|_| {
        Ok::<_, color_eyre::Report>(EnvFilter::default().add_directive(Level::INFO.into()))
    })?;
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let signal = trolly::signals::Terminate::new();
    let mut client = Client::connect("http://[::1]:50051")
        .await
        .expect("\n Perhaps you forgot to start the server. try $cargo run for help.");

    let mut rng = SmallRng::from_entropy();

    let b = 2_000;
    println!(
        "Generating random uniform probes! Average wait: {}ms",
        b / 2
    );

    while !signal.is_terminated() {
        let book_snapshot = client
            .get_limit_order_book(Request::new(Pair {
                pair: "BTCUSDT".to_string(),
            }))
            .await?
            .into_inner();

        println!("book={:?}", book_snapshot);

        let x: u64 = rng.gen_range(0..2_000);
        thread::sleep(Duration::from_millis(x));
    }

    Ok(())
}
