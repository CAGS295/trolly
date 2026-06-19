//! Binance USDM execution and account bookkeeping over user-data streams.
//!
//! Parses `ORDER_TRADE_UPDATE`, `ACCOUNT_UPDATE`, and related private stream events,
//! maintains per-symbol order state and account-wide position/balance books, and fans
//! updates into [`trolly_stream::MonitorMultiplexor`] ingress alongside other stream handlers.

mod endpoints;
mod handler;
mod ingress;
mod parse;
mod types;

pub use endpoints::UsdmUserDataStream;
pub use handler::{UsdmExecContext, UsdmExecHandler, ACCOUNT_ROUTING_ID};
pub use ingress::{build_hub, build_multiplexor, ingest_user_data, UsdmExecHub};
pub use parse::{parse_user_events, ParseError};
pub use types::{
    BalanceChange, MarginCall, MarginCallPosition, OrderTradeUpdate, PositionChange, PositionKey,
    SymbolBookkeeping, UsdmExec, UsdmExecUpdate,
};

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    use trolly_stream::{EventHandler, Message, MonitorMultiplexor};

    fn shared_ctx() -> UsdmExecContext {
        UsdmExecContext::new(None)
    }

    #[test]
    fn parse_order_trade_update_fixture() {
        let json = include_str!("../tests/fixtures/order_trade_update.json");
        let events = parse_user_events(Message::Text(json.into())).unwrap();
        assert_eq!(events.len(), 1);

        let UsdmExecUpdate::OrderTrade(order) = &events[0] else {
            panic!("expected order trade update");
        };
        assert_eq!(order.symbol, "BTCUSDT");
        assert_eq!(order.side, "SELL");
        assert_eq!(order.order_type, "TRAILING_STOP_MARKET");
        assert_eq!(order.execution_type, "NEW");
        assert_eq!(order.order_status, "NEW");
        assert_eq!(order.order_id, 8886774);
        assert_eq!(order.position_side, "LONG");
    }

    #[test]
    fn parse_account_update_fixture() {
        let json = include_str!("../tests/fixtures/account_update.json");
        let events = parse_user_events(Message::Text(json.into())).unwrap();

        let balances: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                UsdmExecUpdate::BalanceChange(b) => Some(b),
                _ => None,
            })
            .collect();
        let positions: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                UsdmExecUpdate::PositionChange(p) => Some(p),
                _ => None,
            })
            .collect();

        assert_eq!(balances.len(), 2);
        assert_eq!(balances[0].asset, "USDT");
        assert_eq!(positions.len(), 2);
        assert_eq!(positions[0].symbol, "BTCUSDT");
        assert_eq!(positions[0].position_side, "LONG");
        assert_eq!(positions[1].position_side, "SHORT");
    }

    #[test]
    fn parse_combined_stream_envelope() {
        let inner = include_str!("../tests/fixtures/order_trade_update.json");
        let wrapped = format!(r#"{{"stream":"lk","data":{inner}}}"#);
        let events = parse_user_events(Message::Text(wrapped.into())).unwrap();
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], UsdmExecUpdate::OrderTrade(_)));
    }

    #[test]
    fn parse_subscription_ack_returns_empty() {
        let json = r#"{"result":null,"id":1}"#;
        let events = parse_user_events(Message::Text(json.into())).unwrap();
        assert!(events.is_empty());
    }

    #[test]
    fn handler_tracks_open_and_closed_orders() {
        let new_json = include_str!("../tests/fixtures/order_trade_update.json");
        let filled_json = include_str!("../tests/fixtures/order_trade_update_filled.json");

        let recorded = Arc::new(Mutex::new(Vec::new()));
        let mut handler =
            UsdmExecHandler::with_recorder("BTCUSDT", recorded, shared_ctx());

        let new_msg = UsdmExecHandler::parse_update(Message::Text(new_json.into()))
            .unwrap()
            .unwrap();
        handler.handle_update(new_msg).unwrap();
        assert_eq!(handler.symbol_state().open_orders.len(), 1);

        let filled = UsdmExecHandler::parse_update(Message::Text(filled_json.into()))
            .unwrap()
            .unwrap();
        handler.handle_update(filled).unwrap();
        assert!(handler.symbol_state().open_orders.is_empty());
    }

    #[test]
    fn event_handler_to_id_routes_by_symbol() {
        let json = include_str!("../tests/fixtures/order_trade_update.json");
        let update = UsdmExecHandler::parse_update(Message::Text(json.into()))
            .unwrap()
            .unwrap();
        assert_eq!(UsdmExecHandler::to_id(&update), "BTCUSDT");
    }

    #[test]
    fn multiplexor_ingest_message_routes_single_event() {
        let json = include_str!("../tests/fixtures/order_trade_update.json");
        let recorded = Arc::new(Mutex::new(Vec::new()));

        let mut hub = MonitorMultiplexor::from_writers(HashMap::from([(
            "BTCUSDT".into(),
            UsdmExecHandler::with_recorder("BTCUSDT", recorded.clone(), shared_ctx()),
        )]));

        hub.ingest_message(Message::Text(json.into()));

        assert_eq!(recorded.lock().unwrap().len(), 1);
    }

    #[test]
    fn account_bookkeeping_multi_leg_long_short_both() {
        let hedge_json = include_str!("../tests/fixtures/account_update.json");
        let both_json = include_str!("../tests/fixtures/account_update_both.json");

        let hub = build_hub(&["BTCUSDT", "ETHUSDT"], shared_ctx());
        let mut multiplexor = hub.multiplexor;

        ingest_user_data(&mut multiplexor, Message::Text(hedge_json.into()));
        ingest_user_data(&mut multiplexor, Message::Text(both_json.into()));

        let book = hub.account.lock().unwrap();
        assert_eq!(book.positions.len(), 3);
        assert!(book.position("BTCUSDT", "LONG").is_some());
        assert!(book.position("BTCUSDT", "SHORT").is_some());
        assert!(book.position("ETHUSDT", "BOTH").is_some());
        assert_eq!(
            book.position("ETHUSDT", "BOTH").unwrap().position_amount,
            "5"
        );
    }

    #[test]
    fn account_bookkeeping_position_flatten_removes_long_leg() {
        let open_json = include_str!("../tests/fixtures/account_update.json");
        let flatten_json = include_str!("../tests/fixtures/account_update_flatten_long.json");

        let hub = build_hub(&["BTCUSDT"], shared_ctx());
        let mut multiplexor = hub.multiplexor;

        ingest_user_data(&mut multiplexor, Message::Text(open_json.into()));
        {
            let book = hub.account.lock().unwrap();
            assert!(book.position("BTCUSDT", "LONG").is_some());
        }

        ingest_user_data(&mut multiplexor, Message::Text(flatten_json.into()));
        let book = hub.account.lock().unwrap();
        assert!(book.position("BTCUSDT", "LONG").is_none());
        assert!(book.position("BTCUSDT", "SHORT").is_some());
        assert_eq!(
            book.position("BTCUSDT", "SHORT")
                .unwrap()
                .position_amount,
            "-5"
        );
    }

    #[test]
    fn query_api_reads_open_position_from_context() {
        let json = include_str!("../tests/fixtures/account_update_both.json");
        let ctx = shared_ctx();
        let mut handler = UsdmExecHandler::new("ETHUSDT", ctx.clone());

        let events = parse_user_events(Message::Text(json.into())).unwrap();
        for event in events {
            if UsdmExecHandler::to_id(&event) == "ETHUSDT" {
                handler.handle_update(event).unwrap();
            }
        }

        let leg = ctx.position("ETHUSDT", "BOTH").expect("BOTH leg");
        assert_eq!(leg.position_amount, "5");
        assert!(!leg.is_flat());
    }

    #[test]
    fn parse_margin_call_fixture() {
        let json = include_str!("../tests/fixtures/margin_call.json");
        let events = parse_user_events(Message::Text(json.into())).unwrap();
        assert_eq!(events.len(), 1);

        let UsdmExecUpdate::MarginCall(call) = &events[0] else {
            panic!("expected margin call");
        };
        assert_eq!(call.event_time, 1587727187525);
        assert_eq!(call.cross_wallet_balance, "3.16812045");
        assert_eq!(call.positions.len(), 2);
        assert_eq!(call.positions[0].symbol, "ETHUSDT");
        assert_eq!(call.positions[0].position_side, "LONG");
        assert_eq!(call.positions[1].symbol, "BTCUSDT");
    }

    #[test]
    fn margin_call_routes_to_account_and_persists_state() {
        let json = include_str!("../tests/fixtures/margin_call.json");
        let account = Arc::new(Mutex::new(SymbolBookkeeping::default()));
        let ctx = UsdmExecContext::with_account(account.clone(), None);
        let btc_rec = Arc::new(Mutex::new(Vec::new()));
        let account_rec = Arc::new(Mutex::new(Vec::new()));

        let mut hub = MonitorMultiplexor::from_writers(HashMap::from([
            (
                "BTCUSDT".into(),
                UsdmExecHandler::with_recorder("BTCUSDT", btc_rec.clone(), ctx.clone()),
            ),
            (
                "ETHUSDT".into(),
                UsdmExecHandler::with_recorder("ETHUSDT", Arc::new(Mutex::new(Vec::new())), ctx.clone()),
            ),
            (
                ACCOUNT_ROUTING_ID.into(),
                UsdmExecHandler::with_recorder(ACCOUNT_ROUTING_ID, account_rec.clone(), ctx.clone()),
            ),
        ]));

        ingest_user_data(&mut hub, Message::Text(json.into()));

        assert_eq!(account_rec.lock().unwrap().len(), 1);
        assert!(btc_rec.lock().unwrap().is_empty());
        assert!(matches!(
            account_rec.lock().unwrap()[0],
            UsdmExecUpdate::MarginCall(_)
        ));

        let book = account.lock().unwrap();
        let call = book.margin_call().expect("margin call persisted");
        assert_eq!(call.event_time, 1587727187525);
        assert_eq!(call.cross_wallet_balance, "3.16812045");
        assert_eq!(call.positions.len(), 2);
        assert_eq!(call.positions[0].symbol, "ETHUSDT");
        assert_eq!(call.positions[1].symbol, "BTCUSDT");
        drop(book);

        assert_eq!(ctx.latest_margin_call().unwrap().event_time, 1587727187525);
    }

    #[test]
    fn margin_call_supersedes_older_on_ingress() {
        let older = r#"{"e":"MARGIN_CALL","E":100,"cw":"1.0","p":[]}"#;
        let newer = r#"{"e":"MARGIN_CALL","E":200,"cw":"2.0","p":[]}"#;

        let hub = build_hub(&["BTCUSDT"], shared_ctx());
        let mut multiplexor = hub.multiplexor;

        ingest_user_data(&mut multiplexor, Message::Text(older.into()));
        ingest_user_data(&mut multiplexor, Message::Text(newer.into()));

        {
            let book = hub.account.lock().unwrap();
            let call = book.margin_call().unwrap();
            assert_eq!(call.event_time, 200);
            assert_eq!(call.cross_wallet_balance, "2.0");
        }

        ingest_user_data(&mut multiplexor, Message::Text(older.into()));
        {
            let book = hub.account.lock().unwrap();
            let call = book.margin_call().unwrap();
            assert_eq!(call.event_time, 200);
        }
    }

    #[test]
    fn margin_call_forwards_on_outbound_channel() {
        let json = include_str!("../tests/fixtures/margin_call.json");
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let ctx = UsdmExecContext::new(Some(tx));
        let mut handler = UsdmExecHandler::new(ACCOUNT_ROUTING_ID, ctx);

        let update = UsdmExecHandler::parse_update(Message::Text(json.into()))
            .unwrap()
            .unwrap();
        handler.handle_update(update).unwrap();

        let forwarded = rx.try_recv().unwrap();
        let UsdmExecUpdate::MarginCall(call) = forwarded else {
            panic!("expected margin call on outbound");
        };
        assert_eq!(call.event_time, 1587727187525);
        assert_eq!(call.positions.len(), 2);
    }
}
