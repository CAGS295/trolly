pub use clap::Parser;
use tracing::Level;
use tracing_subscriber::EnvFilter;
use trolly::Cli;

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> Result<(), color_eyre::Report> {
    color_eyre::install()?;
    let filter = EnvFilter::try_from_default_env().or_else(|_| {
        Ok::<_, color_eyre::Report>(EnvFilter::default().add_directive(Level::INFO.into()))
    })?;
    tracing_subscriber::fmt().with_env_filter(filter).init();

    Cli::parse().start().await;
    Ok(())
}
