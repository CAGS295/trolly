use std::collections::HashMap;

use tracing::error;
use trolly_stream::{EventHandler, Message, MonitorMultiplexor};

use crate::handler::UsdmExecHandler;
use crate::parse::parse_user_events;
use crate::types::{SymbolBookkeeping, UsdmExec, UsdmExecUpdate};

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
        .handle_update(event)
        .inspect_err(|e| error!("usdm handler error: {e}"))
        .ok();
}

/// Account-wide bookkeeping state (`__account__` handler).
pub fn account_state(
    hub: &MonitorMultiplexor<UsdmExecHandler, UsdmExec>,
) -> Option<&SymbolBookkeeping> {
    hub.writers
        .get(crate::handler::ACCOUNT_ROUTING_ID)
        .map(UsdmExecHandler::state)
}
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

        assert_eq!(account_rec.lock().unwrap().len(), 4);
        assert!(btc_rec.lock().unwrap().is_empty());
        assert!(eth_rec.lock().unwrap().is_empty());

        let account_state = account_state(&hub).expect("account handler");
        assert_eq!(account_state.positions.len(), 2);
        assert!(account_state.position("BTCUSDT", "LONG").is_some());
        assert!(account_state.position("BTCUSDT", "SHORT").is_some());
    }

    #[test]
    fn ingest_account_update_both_mode_persisted_on_account() {
        let account_json = include_str!("../tests/fixtures/account_update_both.json");
        let account_rec = Arc::new(Mutex::new(Vec::new()));

        let mut hub = MonitorMultiplexor::from_writers(HashMap::from([(
            crate::handler::ACCOUNT_ROUTING_ID.into(),
            UsdmExecHandler::with_recorder(
                crate::handler::ACCOUNT_ROUTING_ID,
                account_rec.clone(),
            ),
        )]));

        ingest_user_data(&mut hub, Message::Text(account_json.into()));

        assert_eq!(account_rec.lock().unwrap().len(), 1);
        let account_state = account_state(&hub).expect("account handler");
        assert_eq!(account_state.positions.len(), 1);
        let position = account_state.position("ETHUSDT", "BOTH").expect("BOTH leg");
        assert_eq!(position.position_amount, "1.5");
    }

    #[test]
    fn ingest_account_update_flatten_removes_closed_leg() {
        let open_json = include_str!("../tests/fixtures/account_update.json");
        let flatten_json = include_str!("../tests/fixtures/account_update_flatten_long.json");

        let mut hub = build_multiplexor(&["BTCUSDT"], None);

        ingest_user_data(&mut hub, Message::Text(open_json.into()));
        let state = account_state(&hub).expect("account handler");
        assert_eq!(state.positions.len(), 2);

        ingest_user_data(&mut hub, Message::Text(flatten_json.into()));
        let state = account_state(&hub).expect("account handler");
        assert_eq!(state.positions.len(), 1);
        assert!(state.position("BTCUSDT", "LONG").is_none());
        assert!(state.position("BTCUSDT", "SHORT").is_some());
    }
}
