use crate::monitor::Provider;

/// Resolve the left-hand side of `provider:SYMBOL` from `--sources`.
pub fn parse_provider_label(label: &str) -> Provider {
    Provider::from_label(label)
}

/// Returns true when the provider has a wired depth endpoint in this crate.
pub fn is_wired(provider: &Provider) -> bool {
    matches!(provider, Provider::Binance | Provider::BinanceUsdM)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_labels_are_wired() {
        assert!(is_wired(&parse_provider_label("binance")));
        assert!(is_wired(&parse_provider_label("binance-usd-m")));
    }

    #[test]
    fn custom_labels_are_not_wired() {
        assert!(!is_wired(&parse_provider_label("kraken")));
    }
}
