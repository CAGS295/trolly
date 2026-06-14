use std::collections::HashMap;

use tracing::error;
use trolly_stream::{EventHandler, Message, MonitorMultiplexor};

use crate::handler::UsdmExecHandler;
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
    if let UsdmExecUpdate::PositionChange(ref position) = event {
        persist_account_position(hub, position);
    }

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

/// Mirror position rows into the account-wide handler without duplicating outbound events.
fn persist_account_position(
    hub: &mut MonitorMultiplexor<UsdmExecHandler, UsdmExec>,
    position: &crate::types::PositionChange,
) {
    let Some(account) = hub.writers.get_mut(crate::handler::ACCOUNT_ROUTING_ID) else {
        error!("missing usdm account handler");
        return;
    };
    account.apply_bookkeeping(&UsdmExecUpdate::PositionChange(position.clone()));
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

        let btc_state = hub.writers.get("BTCUSDT").unwrap().symbol_state();
        assert_eq!(btc_state.positions.len(), 2);
        assert!(btc_state.positions.contains_key("BTCUSDT:LONG"));
        assert!(btc_state.positions.contains_key("BTCUSDT:SHORT"));

        let account_state = hub
            .writers
            .get(crate::handler::ACCOUNT_ROUTING_ID)
            .unwrap()
            .account_state();
        assert_eq!(account_state.balances.len(), 2);
        assert_eq!(account_state.positions.len(), 2);
        assert!(account_state.positions.contains_key("BTCUSDT:LONG"));
        assert!(account_state.positions.contains_key("BTCUSDT:SHORT"));
        assert!(account_state.balances.contains_key("USDT"));
    }

    #[test]
    fn ingest_account_update_multi_leg_and_flatten() {
        use crate::types::position_key;

        let multi_leg_json = include_str!("../tests/fixtures/account_update_multi_leg.json");
        let flatten_json = include_str!("../tests/fixtures/account_update_flatten.json");
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

        ingest_user_data(&mut hub, Message::Text(multi_leg_json.into()));

        let account = hub
            .writers
            .get(crate::handler::ACCOUNT_ROUTING_ID)
            .unwrap()
            .account_state();
        assert_eq!(account.positions.len(), 3);
        assert!(account.positions.contains_key(&position_key("BTCUSDT", "LONG")));
        assert!(account.positions.contains_key(&position_key("BTCUSDT", "SHORT")));
        assert!(account.positions.contains_key(&position_key("ETHUSDT", "BOTH")));

        let btc = hub.writers.get("BTCUSDT").unwrap().symbol_state();
        assert_eq!(btc.positions.len(), 2);

        let eth = hub.writers.get("ETHUSDT").unwrap().symbol_state();
        assert_eq!(eth.positions.len(), 1);
        assert!(eth.positions.contains_key(&position_key("ETHUSDT", "BOTH")));

        ingest_user_data(&mut hub, Message::Text(flatten_json.into()));

        let account = hub
            .writers
            .get(crate::handler::ACCOUNT_ROUTING_ID)
            .unwrap()
            .account_state();
        assert_eq!(account.positions.len(), 2);
        assert!(!account.positions.contains_key(&position_key("BTCUSDT", "LONG")));
        assert!(account.positions.contains_key(&position_key("BTCUSDT", "SHORT")));
        assert!(account.positions.contains_key(&position_key("ETHUSDT", "BOTH")));

        let btc = hub.writers.get("BTCUSDT").unwrap().symbol_state();
        assert_eq!(btc.positions.len(), 1);
        assert!(!btc.positions.contains_key(&position_key("BTCUSDT", "LONG")));
    }

    #[test]
    fn ingest_margin_call_routes_to_account_state() {
        let margin_json = include_str!("../tests/fixtures/margin_call.json");
        let btc_rec = Arc::new(Mutex::new(Vec::new()));
        let account_rec = Arc::new(Mutex::new(Vec::new()));

        let mut hub = MonitorMultiplexor::from_writers(HashMap::from([
            (
                "BTCUSDT".into(),
                UsdmExecHandler::with_recorder("BTCUSDT", btc_rec.clone()),
            ),
            (
                crate::handler::ACCOUNT_ROUTING_ID.into(),
                UsdmExecHandler::with_recorder(
                    crate::handler::ACCOUNT_ROUTING_ID,
                    account_rec.clone(),
                ),
            ),
        ]));

        ingest_user_data(&mut hub, Message::Text(margin_json.into()));

        assert!(btc_rec.lock().unwrap().is_empty());
        assert_eq!(account_rec.lock().unwrap().len(), 1);
        assert!(matches!(
            account_rec.lock().unwrap()[0],
            UsdmExecUpdate::MarginCall(_)
        ));

        let account = hub
            .writers
            .get(crate::handler::ACCOUNT_ROUTING_ID)
            .unwrap()
            .account_state();
        let call = account.latest_margin_call().expect("margin call persisted");
        assert_eq!(call.event_time, 1587727187525);
        assert_eq!(call.cross_wallet_balance, "3.16812045");
        assert_eq!(call.positions.len(), 2);
        assert_eq!(call.positions[0].symbol, "ETHUSDT");
        assert_eq!(call.positions[1].symbol, "BTCUSDT");
    }
}
