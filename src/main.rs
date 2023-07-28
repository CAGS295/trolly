pub use clap::Parser;
use opentelemetry::global;
use tracing::Level;
use tracing_subscriber::{
    fmt, prelude::__tracing_subscriber_SubscriberExt, util::SubscriberInitExt, EnvFilter,
};
use trolly::Cli;

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> Result<(), color_eyre::Report> {
    color_eyre::install()?;
    let filter = EnvFilter::try_from_default_env().or_else(|_| {
        Ok::<_, color_eyre::Report>(EnvFilter::default().add_directive(Level::INFO.into()))
    })?;

    //tracing_subscriber::fmt().with_env_filter(filter).init();
    global::set_text_map_propagator(opentelemetry_jaeger::Propagator::new());

    let tracer = opentelemetry_jaeger::new_agent_pipeline()
        .with_service_name("depth server")
        .install_simple()?;

    let otel = tracing_opentelemetry::layer().with_tracer(tracer);

    tracing_subscriber::registry()
        .with(otel)
        // Continue logging to stdout
        .with(fmt::Layer::default())
        .with(filter)
        .try_init()?;

    Cli::parse().start().await;
    Ok(())
}
