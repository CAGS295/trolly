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
///
/// All handlers share one [`crate::account::AccountBookkeeping`] instance via
/// [`UsdmExecContext`].
pub fn build_multiplexor(
    symbols: &[&str],
    outbound: Option<tokio::sync::mpsc::UnboundedSender<UsdmExecUpdate>>,
) -> MonitorMultiplexor<UsdmExecHandler, UsdmExec> {
    let ctx = UsdmExecContext::new(outbound);
    let mut writers = HashMap::new();
    for symbol in symbols {
        writers.insert(
            (*symbol).to_string(),
            UsdmExecHandler::with_context(*symbol, ctx.clone()),
        );
    }
    writers.insert(
        crate::handler::ACCOUNT_ROUTING_ID.into(),
        UsdmExecHandler::with_context(crate::handler::ACCOUNT_ROUTING_ID, ctx),
    );
    MonitorMultiplexor::from_writers(writers)
}

/// Shared account bookkeeping from a built multiplexor (any handler's context).
pub fn account_from_multiplexor(
    hub: &MonitorMultiplexor<UsdmExecHandler, UsdmExec>,
) -> Option<std::sync::Arc<std::sync::Mutex<crate::account::AccountBookkeeping>>> {
    hub.writers
        .values()
        .next()
        .map(|handler| handler.account_bookkeeping())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    use trolly_stream::Message;

    fn test_hub(
        btc_rec: Arc<Mutex<Vec<UsdmExecUpdate>>>,
        eth_rec: Arc<Mutex<Vec<UsdmExecUpdate>>>,
        account_rec: Arc<Mutex<Vec<UsdmExecUpdate>>>,
    ) -> (
        MonitorMultiplexor<UsdmExecHandler, UsdmExec>,
        Arc<Mutex<crate::account::AccountBookkeeping>>,
    ) {
        let ctx = UsdmExecContext::new(None);
        let shared_account = Arc::clone(&ctx.account);
        let hub = MonitorMultiplexor::from_writers(HashMap::from([
            (
                "BTCUSDT".into(),
                UsdmExecHandler::with_recorder_and_context("BTCUSDT", btc_rec, ctx.clone()),
            ),
            (
                "ETHUSDT".into(),
                UsdmExecHandler::with_recorder_and_context("ETHUSDT", eth_rec, ctx.clone()),
            ),
            (
                crate::handler::ACCOUNT_ROUTING_ID.into(),
                UsdmExecHandler::with_recorder_and_context(
                    crate::handler::ACCOUNT_ROUTING_ID,
                    account_rec,
                    ctx,
                ),
            ),
        ]));
        (hub, shared_account)
    }

    #[test]
    fn ingest_routes_order_trade_to_symbol_handler() {
        let order_json = include_str!("../tests/fixtures/order_trade_update.json");
        let btc_rec = Arc::new(Mutex::new(Vec::new()));
        let eth_rec = Arc::new(Mutex::new(Vec::new()));
        let account_rec = Arc::new(Mutex::new(Vec::new()));

        let (mut hub, _account) = test_hub(btc_rec.clone(), eth_rec.clone(), account_rec.clone());

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

        let (mut hub, shared_account) =
            test_hub(btc_rec.clone(), eth_rec.clone(), account_rec.clone());

        ingest_user_data(&mut hub, Message::Text(account_json.into()));

        assert_eq!(account_rec.lock().unwrap().len(), 2);
        assert_eq!(btc_rec.lock().unwrap().len(), 2);
        assert!(eth_rec.lock().unwrap().is_empty());

        let btc_state = hub.writers.get("BTCUSDT").unwrap().state();
        assert_eq!(btc_state.positions.len(), 2);
        assert!(btc_state.positions.contains_key("LONG"));
        assert!(btc_state.positions.contains_key("SHORT"));

        let account = shared_account.lock().unwrap();
        assert_eq!(account.balances.len(), 2);
        assert!(account.balance("USDT").is_some());
        assert!(account.balance("BUSD").is_some());
        assert_eq!(account.positions.len(), 2);
        assert!(account.position("BTCUSDT", "LONG").is_some());
        assert!(account.position("BTCUSDT", "SHORT").is_some());
        assert_eq!(
            account.position("BTCUSDT", "LONG").unwrap().position_amount,
            "20"
        );
    }

    #[test]
    fn ingest_account_update_both_position_side() {
        let account_json = include_str!("../tests/fixtures/account_update_both.json");
        let btc_rec = Arc::new(Mutex::new(Vec::new()));
        let eth_rec = Arc::new(Mutex::new(Vec::new()));
        let account_rec = Arc::new(Mutex::new(Vec::new()));

        let (mut hub, shared_account) =
            test_hub(btc_rec.clone(), eth_rec.clone(), account_rec.clone());

        ingest_user_data(&mut hub, Message::Text(account_json.into()));

        assert_eq!(eth_rec.lock().unwrap().len(), 1);
        let eth_state = hub.writers.get("ETHUSDT").unwrap().state();
        assert_eq!(eth_state.positions.len(), 1);
        assert!(eth_state.positions.contains_key("BOTH"));

        let account = shared_account.lock().unwrap();
        let both = account.position("ETHUSDT", "BOTH").unwrap();
        assert_eq!(both.position_amount, "1.5");
    }

    #[test]
    fn ingest_account_update_flattens_position() {
        let open_json = include_str!("../tests/fixtures/account_update_both.json");
        let flatten_json = include_str!("../tests/fixtures/account_update_flatten.json");
        let btc_rec = Arc::new(Mutex::new(Vec::new()));
        let eth_rec = Arc::new(Mutex::new(Vec::new()));
        let account_rec = Arc::new(Mutex::new(Vec::new()));

        let (mut hub, shared_account) =
            test_hub(btc_rec.clone(), eth_rec.clone(), account_rec.clone());

        ingest_user_data(&mut hub, Message::Text(open_json.into()));
        ingest_user_data(&mut hub, Message::Text(flatten_json.into()));

        let eth_state = hub.writers.get("ETHUSDT").unwrap().state();
        assert!(eth_state.positions.is_empty());

        let account = shared_account.lock().unwrap();
        assert!(account.position("ETHUSDT", "BOTH").is_none());
        assert!(account.positions_for_symbol("ETHUSDT").next().is_none());
    }
}
