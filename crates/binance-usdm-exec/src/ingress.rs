//! Fan-in routing for USDM user-data frames through [`trolly_stream`] ingress.

use crate::events::{RoutedUsdmEvent, UsdmExec};
use crate::ledger::UsdmSymbolLedger;
use crate::parse::{parse_user_data_message, ParseError};
use std::collections::HashMap;
use trolly_stream::{EventHandler, MessageIngress, MonitorMultiplexor, RouteOutcome};
use tokio_tungstenite::tungstenite::Message;

/// Expand a user-data frame into per-symbol routed events.
pub fn expand_user_data(message: Message) -> Result<Option<Vec<RoutedUsdmEvent>>, ParseError> {
    let events = match parse_user_data_message(message)? {
        None => return Ok(None),
        Some(events) => events,
    };

    let mut routed = Vec::new();
    for event in events {
        let symbols = event.route_symbols();
        if symbols.is_empty() {
            routed.push(RoutedUsdmEvent {
                symbol: String::new(),
                event,
            });
        } else {
            for symbol in symbols {
                routed.push(RoutedUsdmEvent {
                    symbol,
                    event: event.clone(),
                });
            }
        }
    }
    Ok(Some(routed))
}

/// Route expanded events through a symbol ledger map (shared by ingress and tests).
pub fn route_routed_events(
    writers: &mut HashMap<String, UsdmSymbolLedger>,
    events: Vec<RoutedUsdmEvent>,
) -> Vec<RouteOutcome> {
    events
        .into_iter()
        .map(|event| route_routed_event(writers, event))
        .collect()
}

/// Route one raw user-data websocket frame through a symbol ledger multiplexor.
///
/// Multi-symbol [`crate::events::UsdmStreamEvent::AccountUpdate`] payloads are fanned out
/// to each affected symbol handler.
pub fn route_user_data(
    mux: &mut MonitorMultiplexor<UsdmSymbolLedger, UsdmExec>,
    message: Message,
) -> Result<Vec<RouteOutcome>, ParseError> {
    let routed = match expand_user_data(message)? {
        None => return Ok(vec![RouteOutcome::Skipped]),
        Some(events) => events,
    };

    Ok(route_routed_events(&mut mux.writers, routed))
}

fn route_routed_event(
    writers: &mut HashMap<String, UsdmSymbolLedger>,
    event: RoutedUsdmEvent,
) -> RouteOutcome {
    if event.symbol.is_empty() {
        return RouteOutcome::Skipped;
    }

    let id = UsdmSymbolLedger::to_id(&event);
    let Some(handler) = writers.get_mut(id) else {
        return RouteOutcome::MissingHandler;
    };

    match handler.handle_update(event) {
        Ok(()) => RouteOutcome::Handled,
        Err(_) => RouteOutcome::ParseError,
    }
}

/// Push a user-data frame through inject ingress and route expanded events.
pub fn push_user_data_through_ingress(
    ingress: &MessageIngress,
    mux: &mut MonitorMultiplexor<UsdmSymbolLedger, UsdmExec>,
    message: Message,
) -> Result<Vec<RouteOutcome>, ParseError> {
    ingress.push(message.clone()).map_err(|_| ParseError::Unexpected("ingress closed".into()))?;
    route_user_data(mux, message)
}

/// Convenience builder for a symbol ledger map used in tests and wiring.
pub fn build_symbol_ledgers(symbols: &[&str]) -> HashMap<String, UsdmSymbolLedger> {
    symbols
        .iter()
        .map(|&symbol| (symbol.to_string(), UsdmSymbolLedger::new(symbol)))
        .collect()
}

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
    fn account_update_fans_out_to_symbol_handlers() {
        let mut writers = build_symbol_ledgers(&["BTCUSDT"]);
        let outcomes = route_routed_events(
            &mut writers,
            expand_user_data(Message::Text(fixture("account_update.json").into()))
                .unwrap()
                .unwrap(),
        );

        assert!(outcomes.iter().any(|o| *o == RouteOutcome::Handled));
        let ledger = writers.get("BTCUSDT").unwrap();
        assert_eq!(ledger.snapshot.positions.len(), 3);
    }

    #[test]
    fn order_trade_update_routes_to_matching_symbol() {
        let mut writers = build_symbol_ledgers(&["BTCUSDT", "ETHUSDT"]);
        let outcomes = route_routed_events(
            &mut writers,
            expand_user_data(Message::Text(fixture("order_trade_update.json").into()))
                .unwrap()
                .unwrap(),
        );

        assert_eq!(outcomes, vec![RouteOutcome::Handled]);
        assert_eq!(writers.get("BTCUSDT").unwrap().snapshot.orders.len(), 1);
        assert!(writers.get("ETHUSDT").unwrap().snapshot.orders.is_empty());
    }

    #[tokio::test]
    async fn ingress_push_routes_fixture() {
        let mut writers = build_symbol_ledgers(&["BTCUSDT"]);
        let (ingress, mut rx) = trolly_stream::message_ingress();

        ingress
            .push(Message::Text(fixture("order_trade_update.json").into()))
            .unwrap();

        let msg = rx.recv().await.expect("ingress frame");
        let outcomes = route_routed_events(
            &mut writers,
            expand_user_data(msg).unwrap().unwrap(),
        );
        assert_eq!(outcomes, vec![RouteOutcome::Handled]);
    }
}
