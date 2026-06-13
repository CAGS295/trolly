use std::collections::HashMap;

use tracing::error;
use trolly_stream::{EventHandler, Message, MonitorMultiplexor};

use crate::handler::{UsdmExecContext, UsdmExecHandler};
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
        .handle_update(event)
        .inspect_err(|e| error!("usdm handler error: {e}"))
        .ok();
}

/// Build a multiplexor with per-symbol handlers plus an account-wide handler.
pub fn build_multiplexor(
    symbols: &[&str],
    outbound: Option<tokio::sync::mpsc::UnboundedSender<UsdmExecUpdate>>,
) -> MonitorMultiplexor<UsdmExecHandler, UsdmExec> {
    let ctx = UsdmExecContext::new(outbound);
    build_multiplexor_with_context(symbols, ctx)
}

/// Build a multiplexor with a caller-supplied shared [`UsdmExecContext`].
pub fn build_multiplexor_with_context(
    symbols: &[&str],
    ctx: UsdmExecContext,
) -> MonitorMultiplexor<UsdmExecHandler, UsdmExec> {
    let mut writers = HashMap::new();
    for symbol in symbols {
        writers.insert(
            (*symbol).to_string(),
            UsdmExecHandler::new(*symbol, ctx.clone()),
        );
    }
    writers.insert(
        crate::handler::ACCOUNT_ROUTING_ID.into(),
        UsdmExecHandler::new(crate::handler::ACCOUNT_ROUTING_ID, ctx),
    );
    MonitorMultiplexor::from_writers(writers)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    use trolly_stream::Message;

    use crate::types::PositionKey;

    fn test_context() -> UsdmExecContext {
        UsdmExecContext::new(None)
    }

    #[test]
    fn ingest_routes_order_trade_to_symbol_handler() {
        let order_json = include_str!("../tests/fixtures/order_trade_update.json");
        let btc_rec = Arc::new(Mutex::new(Vec::new()));
        let eth_rec = Arc::new(Mutex::new(Vec::new()));
        let account_rec = Arc::new(Mutex::new(Vec::new()));
        let ctx = test_context();

        let mut hub = MonitorMultiplexor::from_writers(HashMap::from([
            (
                "BTCUSDT".into(),
                UsdmExecHandler::with_recorder("BTCUSDT", btc_rec.clone(), ctx.clone()),
            ),
            (
                "ETHUSDT".into(),
                UsdmExecHandler::with_recorder("ETHUSDT", eth_rec.clone(), ctx.clone()),
            ),
            (
                crate::handler::ACCOUNT_ROUTING_ID.into(),
                UsdmExecHandler::with_recorder(
                    crate::handler::ACCOUNT_ROUTING_ID,
                    account_rec.clone(),
                    ctx,
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
        let ctx = test_context();
        let shared_account = ctx.account.clone();

        let mut hub = MonitorMultiplexor::from_writers(HashMap::from([
            (
                "BTCUSDT".into(),
                UsdmExecHandler::with_recorder("BTCUSDT", btc_rec.clone(), ctx.clone()),
            ),
            (
                "ETHUSDT".into(),
                UsdmExecHandler::with_recorder("ETHUSDT", eth_rec.clone(), ctx.clone()),
            ),
            (
                crate::handler::ACCOUNT_ROUTING_ID.into(),
                UsdmExecHandler::with_recorder(
                    crate::handler::ACCOUNT_ROUTING_ID,
                    account_rec.clone(),
                    ctx,
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
            .contains_key(&PositionKey::new("BTCUSDT", "LONG")));
        assert!(btc_state
            .positions
            .contains_key(&PositionKey::new("BTCUSDT", "SHORT")));

        let account = shared_account.lock().unwrap();
        assert_eq!(account.balances.len(), 2);
        assert!(account.balance("USDT").is_some());
        assert!(account.balance("BUSD").is_some());
        assert_eq!(account.positions.len(), 2);
        assert!(account.position("BTCUSDT", "LONG").is_some());
        assert!(account.position("BTCUSDT", "SHORT").is_some());
        assert!(account
            .positions_for_symbol("__account__")
            .next()
            .is_none());
    }

    #[test]
    fn ingest_account_update_both_mode_and_flatten() {
        let both_json = include_str!("../tests/fixtures/account_update_both.json");
        let flatten_json = include_str!("../tests/fixtures/account_update_flatten.json");
        let ctx = test_context();
        let shared_account = ctx.account.clone();

        let mut hub = build_multiplexor_with_context(&["BTCUSDT"], ctx);

        ingest_user_data(&mut hub, Message::Text(both_json.into()));

        {
            let account = shared_account.lock().unwrap();
            assert_eq!(account.positions.len(), 1);
            let both = account.position("BTCUSDT", "BOTH").unwrap();
            assert_eq!(both.position_amount, "5");
        }

        let btc_state = hub.writers.get("BTCUSDT").unwrap().state();
        assert_eq!(btc_state.positions.len(), 1);
        assert!(btc_state
            .positions
            .contains_key(&PositionKey::new("BTCUSDT", "BOTH")));

        ingest_user_data(&mut hub, Message::Text(flatten_json.into()));

        let account = shared_account.lock().unwrap();
        assert!(account.position("BTCUSDT", "BOTH").is_none());
        assert!(account.positions.is_empty());

        let btc_state = hub.writers.get("BTCUSDT").unwrap().state();
        assert!(btc_state.positions.is_empty());
    }

    #[test]
    fn account_handler_does_not_store_positions() {
        let account_json = include_str!("../tests/fixtures/account_update.json");
        let ctx = test_context();
        let shared_account = ctx.account.clone();

        let mut hub = build_multiplexor_with_context(&["BTCUSDT"], ctx);

        ingest_user_data(&mut hub, Message::Text(account_json.into()));

        let account_handler = hub
            .writers
            .get(crate::handler::ACCOUNT_ROUTING_ID)
            .unwrap();
        assert!(account_handler.state().positions.is_empty());
        assert_eq!(shared_account.lock().unwrap().positions.len(), 2);
    }
}
