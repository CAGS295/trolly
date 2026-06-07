use clap::{Parser, Subcommand};

#[derive(Parser)]
#[clap(
    about = "Toy streamer client for crypto applications.",
    long_about = "Trolly is a streaming limit order book (LOB) monitor for cryptocurrency exchanges.

It connects to exchange WebSocket feeds, maintains real-time order book state using a lock-free concurrent data structure, and serves snapshots to consumers over gRPC and HTTP.

Use the monitor subcommand to stream depth data from supported exchanges and expose the live book via optional API endpoints."
)]
pub struct Cli {
    #[clap(subcommand)]
    command: Commands,
    #[clap(long)]
    pub enable_telemetry: bool,
}

impl Cli {
    pub async fn start(&self) {
        self.command.run().await
    }
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Stream data from an exchange and monitor a metric or structure.
    Monitor {
        #[clap(subcommand)]
        metric: super::monitor::Monitorables,
    },
    /// Comming soon
    Execute,
}

pub trait Run {
    async fn run(&self);
}

impl Run for Commands {
    async fn run(&self) {
        match self {
            Self::Monitor {
                metric: super::monitor::Monitorables::Depth(args),
            } => {
                use super::monitor::Monitor;
                args.monitor().await;
            }
            _ => todo!(),
        };
    }
}
