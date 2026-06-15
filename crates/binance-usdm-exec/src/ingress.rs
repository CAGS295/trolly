use std::collections::HashMap;

use tracing::error;
use trolly_stream::{EventHandler, Message, MonitorMultiplexor};

use crate::handler::{UsdmExecHandler, ACCOUNT_ROUTING_ID};
use crate::parse::parse_user_events;
use crate::types::{UsdmExec, UsdmExecUpdate};

/// Fan one raw user-data websocket payload into [`MonitorMultiplexor::ingest_message`]
/// semantics: parse, route by [`UsdmExecHandler::to_id`], and apply bookkeeping.
///
/// `ACCOUNT_UPDATE` payloads may produce multiple routed updates (balances → account
/// handler, each position row → its symbol handler).
pub fn ingest_user_data(
    hub: &mut MonitorMultiplexor<UsdmExecHandler, UsdmExec>,
    msg: Message,
) {
    let events = match parse_user_events(msg) {
        Ok(events) => events,
        Err(e) => {
            error!("usdm user-data parse error: {e}");
            return;
        }
    };

    for event in events {
        route_update(hub, event);
    }
}

fn route_update(
    hub: &mut MonitorMultiplexor<UsdmExecHandler, UsdmExec>,
    event: UsdmExecUpdate,
) {
    let id = UsdmExecHandler::to_id(&event);
    let Some(handler) = hub.writers.get_mut(id) else {
        error!("missing usdm handler id {id}");
        return;
    };

    handler
        .handle_update(event.clone())
        .inspect_err(|e| error!("usdm handler error: {e}"))
        .ok();

    if let UsdmExecUpdate::PositionChange(position) = &event {
        if id != ACCOUNT_ROUTING_ID {
            let Some(account) = hub.writers.get_mut(ACCOUNT_ROUTING_ID) else {
                error!("missing usdm handler id {ACCOUNT_ROUTING_ID}");
                return;
            };
            account.apply_position_bookkeeping(position);
        }
    }
}

/// Build a multiplexor with per-symbol handlers plus an account-wide handler.
pub fn build_multiplexor(
    symbols: &[&str],
    outbound: Option<tokio::sync::mpsc::UnboundedSender<UsdmExecUpdate>>,
) -> MonitorMultiplexor<UsdmExecHandler, UsdmExec> {
    let mut writers = HashMap::new();
    for symbol in symbols {
        writers.insert(
            (*symbol).to_string(),
            UsdmExecHandler::new(*symbol, outbound.clone()),
        );
    }
    writers.insert(
        crate::handler::ACCOUNT_ROUTING_ID.into(),
        UsdmExecHandler::new(crate::handler::ACCOUNT_ROUTING_ID, outbound),
    );
    MonitorMultiplexor::from_writers(writers)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    use trolly_stream::Message;

    #[test]
    fn ingest_routes_order_trade_to_symbol_handler() {
        let order_json = include_str!("../tests/fixtures/order_trade_update.json");
        let btc_rec = Arc::new(Mutex::new(Vec::new()));
        let eth_rec = Arc::new(Mutex::new(Vec::new()));
        let account_rec = Arc::new(Mutex::new(Vec::new()));

        let mut hub = MonitorMultiplexor::from_writers(HashMap::from([
            (
                "BTCUSDT".into(),
                UsdmExecHandler::with_recorder("BTCUSDT", btc_rec.clone()),
            ),
            (
                "ETHUSDT".into(),
                UsdmExecHandler::with_recorder("ETHUSDT", eth_rec.clone()),
            ),
            (
                crate::handler::ACCOUNT_ROUTING_ID.into(),
                UsdmExecHandler::with_recorder(
                    crate::handler::ACCOUNT_ROUTING_ID,
                    account_rec.clone(),
                ),
            ),
        ]));

        ingest_user_data(&mut hub, Message::Text(order_json.into()));

        assert_eq!(btc_rec.lock().unwrap().len(), 1);
        assert!(eth_rec.lock().unwrap().is_empty());
        assert!(account_rec.lock().unwrap().is_empty());

        let order = match &btc_rec.lock().unwrap()[0] {
            UsdmExecUpdate::OrderTrade(o) => o.clone(),
            other => panic!("expected order trade, got {other:?}"),
        };
        assert_eq!(order.symbol, "BTCUSDT");
        assert_eq!(order.order_id, 8886774);
        assert_eq!(order.order_status, "NEW");
    }

    #[test]
    fn ingest_account_update_fans_out_positions_and_balances() {
        let account_json = include_str!("../tests/fixtures/account_update.json");
        let btc_rec = Arc::new(Mutex::new(Vec::new()));
        let eth_rec = Arc::new(Mutex::new(Vec::new()));
        let account_rec = Arc::new(Mutex::new(Vec::new()));

        let mut hub = MonitorMultiplexor::from_writers(HashMap::from([
            (
                "BTCUSDT".into(),
                UsdmExecHandler::with_recorder("BTCUSDT", btc_rec.clone()),
            ),
            (
                "ETHUSDT".into(),
                UsdmExecHandler::with_recorder("ETHUSDT", eth_rec.clone()),
            ),
            (
                crate::handler::ACCOUNT_ROUTING_ID.into(),
                UsdmExecHandler::with_recorder(
                    crate::handler::ACCOUNT_ROUTING_ID,
                    account_rec.clone(),
                ),
            ),
        ]));

        ingest_user_data(&mut hub, Message::Text(account_json.into()));

        assert_eq!(account_rec.lock().unwrap().len(), 2);
        assert_eq!(btc_rec.lock().unwrap().len(), 2);
        assert!(eth_rec.lock().unwrap().is_empty());

        let btc_state = hub.writers.get("BTCUSDT").unwrap().state();
        assert_eq!(btc_state.positions.len(), 2);
        assert!(btc_state
            .positions
            .contains_key(&crate::types::PositionKey::new("BTCUSDT", "LONG")));
        assert!(btc_state
            .positions
            .contains_key(&crate::types::PositionKey::new("BTCUSDT", "SHORT")));

        let account_state = hub
            .writers
            .get(crate::handler::ACCOUNT_ROUTING_ID)
            .unwrap()
            .state();
        assert_eq!(account_state.positions.len(), 2);
        assert!(account_state.positions.contains_key(
            &crate::types::PositionKey::new("BTCUSDT", "LONG")
        ));
        assert!(account_state.positions.contains_key(
            &crate::types::PositionKey::new("BTCUSDT", "SHORT")
        ));
        assert!(account_state.open_orders.is_empty());
    }
}
