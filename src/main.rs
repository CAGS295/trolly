pub use clap::Parser;
use trolly::Cli;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), color_eyre::Report> {
    color_eyre::install()?;
    tracing_subscriber::fmt().init();

    Cli::parse().start().await;
    Ok(())
}
