use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tracing::error;
use trolly_stream::{EventHandler, Message, MonitorMultiplexor};

use crate::handler::UsdmExecHandler;
use crate::parse::parse_user_events;
use crate::types::{AccountBookkeeping, UsdmExec, UsdmExecUpdate};

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

/// Build a multiplexor with per-symbol handlers plus an account-wide handler.
///
/// All per-symbol handlers share a [`AccountBookkeeping`] instance so that every
/// `PositionChange` event is aggregated into an account-wide positions map keyed
/// by `(symbol, position_side)`.  Returns both the multiplexor and the shared state.
pub fn build_multiplexor_with_account(
    symbols: &[&str],
    outbound: Option<tokio::sync::mpsc::UnboundedSender<UsdmExecUpdate>>,
) -> (MonitorMultiplexor<UsdmExecHandler, UsdmExec>, Arc<Mutex<AccountBookkeeping>>) {
    let account_bk = Arc::new(Mutex::new(AccountBookkeeping::default()));
    let mut writers = HashMap::new();
    for symbol in symbols {
        writers.insert(
            (*symbol).to_string(),
            UsdmExecHandler::new_with_account_state(
                *symbol,
                outbound.clone(),
                account_bk.clone(),
            ),
        );
    }
    writers.insert(
        crate::handler::ACCOUNT_ROUTING_ID.into(),
        UsdmExecHandler::new(crate::handler::ACCOUNT_ROUTING_ID, outbound),
    );
    (MonitorMultiplexor::from_writers(writers), account_bk)
}

/// Build a multiplexor with per-symbol handlers plus an account-wide handler.
///
/// Convenience wrapper around [`build_multiplexor_with_account`] for callers that
/// manage their own [`AccountBookkeeping`] or do not need account-wide queries.
pub fn build_multiplexor(
    symbols: &[&str],
    outbound: Option<tokio::sync::mpsc::UnboundedSender<UsdmExecUpdate>>,
) -> MonitorMultiplexor<UsdmExecHandler, UsdmExec> {
    build_multiplexor_with_account(symbols, outbound).0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    use trolly_stream::Message;
    use crate::handler::ACCOUNT_ROUTING_ID;
    use crate::types::UsdmExecUpdate;

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
                ACCOUNT_ROUTING_ID.into(),
                UsdmExecHandler::with_recorder(ACCOUNT_ROUTING_ID, account_rec.clone()),
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
                ACCOUNT_ROUTING_ID.into(),
                UsdmExecHandler::with_recorder(ACCOUNT_ROUTING_ID, account_rec.clone()),
            ),
        ]));

        ingest_user_data(&mut hub, Message::Text(account_json.into()));

        assert_eq!(account_rec.lock().unwrap().len(), 2);
        assert_eq!(btc_rec.lock().unwrap().len(), 2);
        assert!(eth_rec.lock().unwrap().is_empty());

        let btc_state = hub.writers.get("BTCUSDT").unwrap().state();
        assert_eq!(btc_state.positions.len(), 2);
        assert!(btc_state.positions.contains_key("LONG"));
        assert!(btc_state.positions.contains_key("SHORT"));
    }

    // ── Account-wide bookkeeping tests ──────────────────────────────────────

    #[test]
    fn account_wide_positions_aggregated_across_symbols() {
        let multi_leg_json = include_str!("../tests/fixtures/account_update_multi_leg.json");

        let (mut hub, account_bk) =
            build_multiplexor_with_account(&["BTCUSDT", "ETHUSDT"], None);

        ingest_user_data(&mut hub, Message::Text(multi_leg_json.into()));

        let positions = account_bk.lock().unwrap();
        assert_eq!(positions.positions.len(), 3, "expected BTCUSDT LONG, BTCUSDT SHORT, ETHUSDT BOTH");

        assert!(positions.positions.contains_key(&("BTCUSDT".into(), "LONG".into())));
        assert!(positions.positions.contains_key(&("BTCUSDT".into(), "SHORT".into())));
        assert!(positions.positions.contains_key(&("ETHUSDT".into(), "BOTH".into())));

        let eth_pos = &positions.positions[&("ETHUSDT".into(), "BOTH".into())];
        assert_eq!(eth_pos.position_amount, "5.0");
        assert_eq!(eth_pos.entry_price, "3000.0");
    }

    #[test]
    fn account_wide_balances_not_duplicated_in_position_state() {
        let multi_leg_json = include_str!("../tests/fixtures/account_update_multi_leg.json");

        let (mut hub, account_bk) =
            build_multiplexor_with_account(&["BTCUSDT", "ETHUSDT"], None);

        ingest_user_data(&mut hub, Message::Text(multi_leg_json.into()));

        // Balance rows must route to account handler, not pollute position map.
        let positions = account_bk.lock().unwrap();
        assert!(
            positions.positions.keys().all(|(sym, _)| sym != "USDT"),
            "balance assets must not appear in position map"
        );
    }

    #[test]
    fn account_wide_position_flatten_removes_entry() {
        let multi_leg_json = include_str!("../tests/fixtures/account_update_multi_leg.json");
        let flatten_json = include_str!("../tests/fixtures/account_update_flatten.json");

        let (mut hub, account_bk) =
            build_multiplexor_with_account(&["BTCUSDT", "ETHUSDT"], None);

        // Establish initial positions.
        ingest_user_data(&mut hub, Message::Text(multi_leg_json.into()));
        assert_eq!(account_bk.lock().unwrap().positions.len(), 3);

        // Flatten BTCUSDT LONG (amount "0").
        ingest_user_data(&mut hub, Message::Text(flatten_json.into()));

        let positions = account_bk.lock().unwrap();
        assert_eq!(positions.positions.len(), 2, "flattened BTCUSDT LONG must be removed");
        assert!(
            !positions.positions.contains_key(&("BTCUSDT".into(), "LONG".into())),
            "(BTCUSDT, LONG) must be absent after flatten"
        );
        assert!(positions.positions.contains_key(&("BTCUSDT".into(), "SHORT".into())));
        assert!(positions.positions.contains_key(&("ETHUSDT".into(), "BOTH".into())));
    }

    #[test]
    fn symbol_bookkeeping_zero_amount_removes_position() {
        let multi_leg_json = include_str!("../tests/fixtures/account_update_multi_leg.json");
        let flatten_json = include_str!("../tests/fixtures/account_update_flatten.json");

        let (mut hub, _account_bk) =
            build_multiplexor_with_account(&["BTCUSDT", "ETHUSDT"], None);

        ingest_user_data(&mut hub, Message::Text(multi_leg_json.into()));
        {
            let btc_state = hub.writers.get("BTCUSDT").unwrap().state();
            assert_eq!(btc_state.positions.len(), 2);
        }

        ingest_user_data(&mut hub, Message::Text(flatten_json.into()));
        {
            let btc_state = hub.writers.get("BTCUSDT").unwrap().state();
            assert_eq!(btc_state.positions.len(), 1, "LONG should be removed after zero-amount update");
            assert!(!btc_state.positions.contains_key("LONG"));
            assert!(btc_state.positions.contains_key("SHORT"));
        }
    }

    #[test]
    fn multi_leg_both_side_position_captured() {
        let multi_leg_json = include_str!("../tests/fixtures/account_update_multi_leg.json");

        let (mut hub, account_bk) =
            build_multiplexor_with_account(&["BTCUSDT", "ETHUSDT"], None);

        ingest_user_data(&mut hub, Message::Text(multi_leg_json.into()));

        let positions = account_bk.lock().unwrap();
        let eth_both = positions
            .positions
            .get(&("ETHUSDT".into(), "BOTH".into()))
            .expect("ETHUSDT BOTH position must be present");
        assert_eq!(eth_both.position_side, "BOTH");
        assert_eq!(eth_both.symbol, "ETHUSDT");
        assert_eq!(eth_both.position_amount, "5.0");
    }
}
