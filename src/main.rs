pub use clap::Parser;
use tracing_subscriber::EnvFilter;
use trolly::Cli;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), color_eyre::Report> {
    color_eyre::install()?;
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    Cli::parse().start().await;
    Ok(())
}
