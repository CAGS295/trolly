use flate2::read::GzDecoder;
use http::header::{ACCEPT_ENCODING, CONTENT_ENCODING};
use http::StatusCode;
use lob::Decode;
use lob::LimitOrderBook;
use rand::rngs::SmallRng;
use rand::{Rng, SeedableRng};
use reqwest::{Client, Method};
use std::io::Read;
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

    let client = Client::new();

    while !signal.is_terminated() {
        let req = client
            .request(Method::GET, "http://[::1]:50051/scale/depth/btcusdt")
            .header(ACCEPT_ENCODING, "gzip")
            .build()?;

        let response = client.execute(req).await?;

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

        let decode = match response.headers().get(CONTENT_ENCODING) {
            Some(s) if s == "gzip" => true,
            Some(s) => {
                panic!("Unhandled encoding {s:?}")
            }
            None => false,
        };
        let bytes = response.bytes().await?;
        let mut decode_output_buffer;
        let mut bytes = if decode {
            decode_output_buffer = Vec::new();
            GzDecoder::new(&bytes[..])
                .read_to_end(&mut decode_output_buffer)
                .unwrap();
            decode_output_buffer.as_slice()
        } else {
            &bytes[..]
        };

        let book_snapshot = LimitOrderBook::decode(&mut bytes)?;

        println!("{book_snapshot}");

        let x: u64 = rng.gen_range(0..2_000);
        thread::sleep(Duration::from_millis(x));
    }

    Ok(())
}
