//! Label registry for depth providers beyond the built-in Binance venues.
//!
//! Built-in labels (`binance`, `binance-usd-m`) map to [`Provider`] variants.
//! Extension labels (e.g. scaffold `stub`, or runtime-registered venues) map to
//! [`Provider::Other`] while preserving the original CLI label on [`BookSource`].

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

use crate::monitor::Provider;

/// Scaffold third-venue label registered at startup; endpoints are not wired yet.
pub const STUB_PROVIDER_LABEL: &str = "stub";

static EXTENSION_LABELS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

fn extension_labels() -> &'static Mutex<HashSet<String>> {
    EXTENSION_LABELS.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Normalize a provider label from `--sources` / `provider:SYMBOL` syntax.
pub fn normalize_label(label: &str) -> String {
    label.trim().to_ascii_lowercase()
}

/// Built-in depth provider labels (not extension registry entries).
pub fn builtin_label(provider: Provider) -> &'static str {
    match provider {
        Provider::Binance => "binance",
        Provider::BinanceUsdM => "binance-usd-m",
        Provider::Other => "other",
    }
}

/// Register an extension label for `provider:SYMBOL` parsing.
///
/// Returns an error when `label` collides with a built-in provider label.
pub fn register_provider_label(label: &str) -> Result<(), String> {
    let normalized = normalize_label(label);
    if normalized.is_empty() {
        return Err("provider label must not be empty".into());
    }
    if Provider::from_builtin_label(&normalized).is_some() {
        return Err(format!(
            "provider label {normalized:?} is reserved for a built-in venue"
        ));
    }
    extension_labels()
        .lock()
        .expect("extension label registry lock")
        .insert(normalized);
    Ok(())
}

/// Resolve a CLI label to a [`Provider`], including registered extensions.
pub fn resolve_provider_label(label: &str) -> Option<Provider> {
    let normalized = normalize_label(label);
    Provider::from_builtin_label(&normalized).or_else(|| {
        extension_labels()
            .lock()
            .expect("extension label registry lock")
            .contains(&normalized)
            .then_some(Provider::Other)
    })
}

/// Register scaffold extension labels that ship with the crate.
pub fn init_builtin_extensions() {
    register_provider_label(STUB_PROVIDER_LABEL).expect("register stub provider label");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_labels_resolve_without_registry() {
        assert_eq!(
            resolve_provider_label("binance"),
            Some(Provider::Binance)
        );
        assert_eq!(
            resolve_provider_label("binance-usd-m"),
            Some(Provider::BinanceUsdM)
        );
    }

    #[test]
    fn register_extension_label_allows_parse() {
        let label = "test-venue-wp003";
        register_provider_label(label).expect("register");
        assert_eq!(resolve_provider_label(label), Some(Provider::Other));
    }

    #[test]
    fn register_builtin_label_is_rejected() {
        let err = register_provider_label("binance").unwrap_err();
        assert!(err.contains("reserved"), "{err}");
    }
}
