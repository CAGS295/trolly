use crate::monitor::{Monitor, Monitorables};
use async_trait::async_trait;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[clap(
    about = "Toy streamer client for crypto applications.",
    long_about = "TODO discribe the main goals"
)]
pub struct Cli {
    #[clap(subcommand)]
    command: Commands,
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
        metric: Monitorables,
    },
    /// Comming soon
    Execute,
}

#[async_trait(?Send)]
pub trait Run {
    async fn run(&self);
}

#[async_trait(?Send)]
impl Run for Commands {
    async fn run(&self) {
        match self {
            Self::Monitor {
                metric: Monitorables::Depth(args),
            } => {
                args.monitor().await;
            }
            _ => todo!(),
        };
    }
}
