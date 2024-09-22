#[cfg(any(feature = "codec", feature = "grpc"))]
#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> Result<(), color_eyre::Report> {
    pub use clap::Parser;
    use opentelemetry::global;
    use tracing::Level;
    use tracing_subscriber::{
        fmt, prelude::__tracing_subscriber_SubscriberExt, util::SubscriberInitExt, EnvFilter,
    };
    use trolly::Cli;

    color_eyre::install()?;
    let cli = Cli::parse();

    let filter = EnvFilter::try_from_default_env().or_else(|_| {
        Ok::<_, color_eyre::Report>(EnvFilter::default().add_directive(Level::INFO.into()))
    })?;

    let registry = tracing_subscriber::registry().with(fmt::Layer::default());

    if cli.enable_telemetry {
        global::set_text_map_propagator(opentelemetry_jaeger::Propagator::new());

        let tracer = opentelemetry_jaeger::new_agent_pipeline()
            .with_service_name("Trolly")
            .install_simple()?;

        let otel = tracing_opentelemetry::layer().with_tracer(tracer);
        registry.with(otel).with(filter).try_init()?;
    } else {
        registry.with(filter).try_init()?;
    }

    cli.start().await;
    Ok(())
}

#[cfg(not(any(feature = "codec", feature = "grpc")))]
fn main() {}
