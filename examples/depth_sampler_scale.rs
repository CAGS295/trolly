use http::StatusCode;
use hyper::body::to_bytes;
use hyper::{Body, Client, Uri};
use lob::Decode;
use lob::LimitOrderBook;
use rand::rngs::SmallRng;
use rand::{Rng, SeedableRng};
use std::time::Duration;
use std::{println, thread};
use tracing::Level;
use tracing_subscriber::EnvFilter;
use trolly::tokio::main;

#[main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let filter = EnvFilter::try_from_default_env().or_else(|_| {
        Ok::<_, color_eyre::Report>(EnvFilter::default().add_directive(Level::INFO.into()))
    })?;
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let signal = trolly::signals::Terminate::new();

    let mut rng = SmallRng::from_entropy();
    let b = 2_000;
    println!(
        "Generating random uniform probes! Average wait: {}ms",
        b / 2
    );

    let client = Client::builder().http2_only(true).build_http::<Body>();

    while !signal.is_terminated() {
        let future = client.get(Uri::from_static("http://[::1]:50051/scale/depth/btcusdt"));
        let response = future.await?;
        match response.status() {
            StatusCode::OK => {}
            err => {
                return Err(format!(
                    "{}:{}: Failed with {:?}",
                    file!(),
                    line!(),
                    err.canonical_reason()
                )
                .into());
            }
        }

        let body = response.into_body();
        let bytes = to_bytes(body).await?;
        let mut bytes = &bytes[..];
        let book_snapshot = LimitOrderBook::decode(&mut bytes)?;

        println!("{book_snapshot}");

        let x: u64 = rng.gen_range(0..2_000);
        thread::sleep(Duration::from_millis(x));
    }

    Ok(())
}
