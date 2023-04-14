pub use clap::Parser;
use trolly::Cli;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    Cli::parse().start().await;
}
