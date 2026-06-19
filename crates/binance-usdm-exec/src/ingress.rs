use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tracing::error;
use trolly_stream::{EventHandler, Message, MonitorMultiplexor};

use crate::handler::{UsdmExecContext, UsdmExecHandler, ACCOUNT_ROUTING_ID};
use crate::parse::parse_user_events;
use crate::types::{SymbolBookkeeping, UsdmExec, UsdmExecUpdate};

/// Hub wiring USDM user-data handlers with a shared account book.
pub struct UsdmExecHub {
    pub multiplexor: MonitorMultiplexor<UsdmExecHandler, UsdmExec>,
    pub account: Arc<Mutex<SymbolBookkeeping>>,
}

/// Fan one raw user-data websocket payload into [`MonitorMultiplexor::ingest_message`]
/// semantics: parse, route by [`UsdmExecHandler::to_id`], and apply bookkeeping.
///
/// `ACCOUNT_UPDATE` payloads may produce multiple routed updates (balances → account
/// handler, each position row → its symbol handler). Position rows are persisted in
/// the shared account [`SymbolBookkeeping`] (see [`UsdmExecHub::account`]).
pub fn ingest_user_data(hub: &mut MonitorMultiplexor<UsdmExecHandler, UsdmExec>, msg: Message) {
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
    ctx: UsdmExecContext,
) -> MonitorMultiplexor<UsdmExecHandler, UsdmExec> {
    build_hub(symbols, ctx).multiplexor
}

/// Build per-symbol handlers, account handler, and expose the shared account book.
pub fn build_hub(symbols: &[&str], ctx: UsdmExecContext) -> UsdmExecHub {
    let account = ctx.account.clone();
    let mut writers = HashMap::new();
    for symbol in symbols {
        writers.insert(
            (*symbol).to_string(),
            UsdmExecHandler::new(*symbol, ctx.clone()),
        );
    }
    writers.insert(
        ACCOUNT_ROUTING_ID.into(),
        UsdmExecHandler::new(ACCOUNT_ROUTING_ID, ctx),
    );
    UsdmExecHub {
        multiplexor: MonitorMultiplexor::from_writers(writers),
        account,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    use trolly_stream::Message;

    #[test]
    fn ingest_routes_order_trade_to_symbol_handler() {
        let order_json = include_str!("../tests/fixtures/order_trade_update.json");
        let ctx = UsdmExecContext::new(None);
        let btc_rec = Arc::new(Mutex::new(Vec::new()));
        let eth_rec = Arc::new(Mutex::new(Vec::new()));
        let account_rec = Arc::new(Mutex::new(Vec::new()));

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
                ACCOUNT_ROUTING_ID.into(),
                UsdmExecHandler::with_recorder(ACCOUNT_ROUTING_ID, account_rec.clone(), ctx),
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
        let account = Arc::new(Mutex::new(SymbolBookkeeping::default()));
        let ctx = UsdmExecContext::with_account(account.clone(), None);
        let btc_rec = Arc::new(Mutex::new(Vec::new()));
        let eth_rec = Arc::new(Mutex::new(Vec::new()));
        let account_rec = Arc::new(Mutex::new(Vec::new()));

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
                ACCOUNT_ROUTING_ID.into(),
                UsdmExecHandler::with_recorder(ACCOUNT_ROUTING_ID, account_rec.clone(), ctx),
            ),
        ]));

        ingest_user_data(&mut hub, Message::Text(account_json.into()));

        assert_eq!(account_rec.lock().unwrap().len(), 2);
        assert_eq!(btc_rec.lock().unwrap().len(), 2);
        assert!(eth_rec.lock().unwrap().is_empty());

        let book = account.lock().unwrap();
        assert_eq!(book.balances.len(), 2);
        assert!(book.balances.contains_key("USDT"));
        assert_eq!(book.positions.len(), 2);
        assert!(book.position("BTCUSDT", "LONG").is_some());
        assert!(book.position("BTCUSDT", "SHORT").is_some());
    }
}
