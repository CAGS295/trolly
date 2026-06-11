//! Per-symbol ledger handler implementing [`trolly_stream::EventHandler`].

use crate::events::{RoutedUsdmEvent, SymbolLedgerSnapshot, UsdmExec};
use crate::endpoints::UsdmUserDataEndpoints;
use crate::parse::{parse_user_data_message, ParseError};
use std::cell::RefCell;
use std::rc::Rc;
use trolly_stream::{Endpoints, EventHandler};
use tokio_tungstenite::tungstenite::Message;

/// Per-symbol execution ledger driven by user-data stream events.
#[derive(Debug, Clone)]
pub struct UsdmSymbolLedger {
    pub symbol: String,
    pub snapshot: SymbolLedgerSnapshot,
}

impl UsdmSymbolLedger {
    pub fn new(symbol: impl Into<String>) -> Self {
        let symbol = symbol.into();
        Self {
            snapshot: SymbolLedgerSnapshot::new(symbol.clone()),
            symbol,
        }
    }
}

impl EventHandler<UsdmExec> for UsdmSymbolLedger {
    type Error = ParseError;
    type Context = Rc<RefCell<Vec<RoutedUsdmEvent>>>;
    type Update = RoutedUsdmEvent;

    fn parse_update(value: Message) -> Result<Option<Self::Update>, Self::Error> {
        let events = match parse_user_data_message(value)? {
            None => return Ok(None),
            Some(events) => events,
        };

        let event = events.into_iter().next().expect("non-empty batch");
        let symbol = event
            .route_symbols()
            .into_iter()
            .next()
            .unwrap_or_default();
        Ok(Some(RoutedUsdmEvent { symbol, event }))
    }

    fn to_id(event: &Self::Update) -> &str {
        &event.symbol
    }

    fn handle_update(&mut self, event: Self::Update) -> Result<(), Self::Error> {
        if event.symbol == self.symbol {
            self.snapshot.apply(&event.event);
        }
        Ok(())
    }

    async fn build<En>(
        _provider: En,
        symbols: &[impl AsRef<str>],
        _ctx: Self::Context,
    ) -> Result<(String, Self), Self::Error>
    where
        En: Endpoints<UsdmExec> + Clone + 'static,
    {
        let symbol = symbols
            .first()
            .expect("one symbol")
            .as_ref()
            .to_string();
        Ok((symbol.clone(), Self::new(symbol)))
    }
}

/// Build-time helper documenting expected provider type for user-data streams.
pub type UsdmUserDataProvider = UsdmUserDataEndpoints;

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn fixture(name: &str) -> String {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name);
        fs::read_to_string(path).expect("fixture")
    }

    #[test]
    fn ledger_applies_order_trade_update() {
        let mut ledger = UsdmSymbolLedger::new("BTCUSDT");
        let routed = UsdmSymbolLedger::parse_update(Message::Text(
            fixture("order_trade_update.json").into(),
        ))
        .unwrap()
        .unwrap();

        ledger.handle_update(routed).unwrap();
        assert_eq!(ledger.snapshot.orders.len(), 1);
        assert!(ledger.snapshot.orders.contains_key(&8886774));
    }

    #[test]
    fn ledger_applies_account_update_positions() {
        let mut ledger = UsdmSymbolLedger::new("BTCUSDT");
        let routed = UsdmSymbolLedger::parse_update(Message::Text(
            fixture("account_update.json").into(),
        ))
        .unwrap()
        .unwrap();

        ledger.handle_update(routed).unwrap();
        assert_eq!(ledger.snapshot.last_account_reason.as_deref(), Some("ORDER"));
        assert_eq!(ledger.snapshot.positions.len(), 3);
    }

    #[test]
    fn to_id_uses_symbol_from_order() {
        let routed = UsdmSymbolLedger::parse_update(Message::Text(
            fixture("order_trade_update.json").into(),
        ))
        .unwrap()
        .unwrap();
        assert_eq!(UsdmSymbolLedger::to_id(&routed), "BTCUSDT");
    }

    #[test]
    fn ignores_events_for_other_symbols() {
        let mut ledger = UsdmSymbolLedger::new("ETHUSDT");
        let routed = UsdmSymbolLedger::parse_update(Message::Text(
            fixture("order_trade_update.json").into(),
        ))
        .unwrap()
        .unwrap();
        assert_eq!(UsdmSymbolLedger::to_id(&routed), "BTCUSDT");

        ledger.handle_update(routed).unwrap();
        assert!(ledger.snapshot.orders.is_empty());
    }
}
