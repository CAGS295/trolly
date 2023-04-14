use clap::ValueEnum;

#[derive(Clone, Debug, ValueEnum)]
pub enum Provider {
    Binance,
    Other,
}
