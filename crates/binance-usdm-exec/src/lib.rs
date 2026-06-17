//! Binance USDM execution and account bookkeeping over user-data streams.
//!
//! Parses `ORDER_TRADE_UPDATE`, `ACCOUNT_UPDATE`, and related private stream events,
//! maintains per-symbol order/position state plus account-wide balances and positions,
//! and fans updates into [`trolly_stream::MonitorMultiplexor`] ingress alongside other
//! stream handlers. Outbound order placement uses signed REST (see [`README.md`](../README.md)).

mod account;
mod auth;
mod client;
mod egress;
mod endpoints;
mod handler;
mod ingress;
mod order;
mod parse;
mod types;

pub use account::{
    apply_position_to_symbol, position_is_closed, AccountBookkeeping, PositionKey,
};
pub use auth::{
    current_timestamp_ms, sign_hmac_sha256_hex, sign_params, signed_params_payload,
};
pub use client::{
    ListenKeyError, PlaceOrderError, PlaceOrderResult, UsdmListenKeyClient, UsdmOrderClient,
};
pub use egress::{UsdmExecEgress, UsdmExecEgressError};
pub use endpoints::{ApiCredentials, UsdmUserDataStream};
pub use handler::{UsdmExecContext, UsdmExecHandler, ACCOUNT_ROUTING_ID};
pub use ingress::{account_from_multiplexor, build_multiplexor, ingest_user_data};
pub use order::{
    order_from_outbound, OrderBuildError, OrderType, PlaceOrderRequest, PositionSide, Side,
    TimeInForce,
};
pub use parse::{parse_user_events, ParseError};
pub use types::{
    BalanceChange, MarginCall, MarginCallPosition, OrderTradeUpdate, PositionChange,
    SymbolBookkeeping, UsdmExec, UsdmExecUpdate,
};

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    use trolly_stream::{EventHandler, Message, MonitorMultiplexor};

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
        assert_eq!(UsdmExecHandler::to_id(&events[0]), ACCOUNT_ROUTING_ID);
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
        let mut handler = UsdmExecHandler::with_recorder("BTCUSDT", recorded);

        let new_msg = UsdmExecHandler::parse_update(Message::Text(new_json.into()))
            .unwrap()
            .unwrap();
        handler.handle_update(new_msg).unwrap();
        assert_eq!(handler.state().open_orders.len(), 1);

        let filled = UsdmExecHandler::parse_update(Message::Text(filled_json.into()))
            .unwrap()
            .unwrap();
        handler.handle_update(filled).unwrap();
        assert!(handler.state().open_orders.is_empty());
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
            UsdmExecHandler::with_recorder("BTCUSDT", recorded.clone()),
        )]));

        hub.ingest_message(Message::Text(json.into()));

        assert_eq!(recorded.lock().unwrap().len(), 1);
    }

    #[test]
    fn account_query_api_reflects_shared_bookkeeping() {
        let mut hub = build_multiplexor(&["BTCUSDT", "ETHUSDT"], None);
        let account_json = include_str!("../tests/fixtures/account_update.json");
        ingest_user_data(&mut hub, Message::Text(account_json.into()));

        let shared = account_from_multiplexor(&hub).expect("shared account");
        let book = shared.lock().unwrap();
        assert_eq!(book.balance("USDT").unwrap().wallet_balance, "122624.12345678");
        assert_eq!(
            book.position("BTCUSDT", "SHORT").unwrap().position_amount,
            "-10"
        );
        assert_eq!(book.positions_for_symbol("BTCUSDT").count(), 2);
    }

    #[test]
    fn symbol_bookkeeping_removes_closed_position_legs() {
        let ctx = UsdmExecContext::new(None);
        let mut handler = UsdmExecHandler::with_context("ETHUSDT", ctx);

        let open = PositionChange {
            event_time: 1,
            reason: "ORDER".into(),
            symbol: "ETHUSDT".into(),
            position_amount: "2".into(),
            entry_price: "100".into(),
            unrealized_pnl: "0".into(),
            margin_type: "cross".into(),
            isolated_wallet: "0".into(),
            position_side: "BOTH".into(),
        };
        handler
            .handle_update(UsdmExecUpdate::PositionChange(open.clone()))
            .unwrap();
        assert!(handler.state().positions.contains_key("BOTH"));

        let closed = PositionChange {
            position_amount: "0".into(),
            ..open
        };
        handler
            .handle_update(UsdmExecUpdate::PositionChange(closed))
            .unwrap();
        assert!(handler.state().positions.is_empty());
    }

    #[test]
    fn margin_call_forwards_on_outbound_channel() {
        let json = include_str!("../tests/fixtures/margin_call.json");
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let ctx = UsdmExecContext::new(Some(tx));
        let mut handler = UsdmExecHandler::with_context(ACCOUNT_ROUTING_ID, ctx);

        let update = UsdmExecHandler::parse_update(Message::Text(json.into()))
            .unwrap()
            .unwrap();
        handler.handle_update(update).unwrap();

        let forwarded = rx.try_recv().expect("margin call forwarded");
        let UsdmExecUpdate::MarginCall(call) = forwarded else {
            panic!("expected margin call on outbound channel");
        };
        assert_eq!(call.event_time, 1587727187525);
        assert_eq!(call.cross_wallet_balance, "3.16812045");

        let account = handler.account_bookkeeping();
        let book = account.lock().unwrap();
        assert_eq!(
            book.latest_margin_call().unwrap().event_time,
            1587727187525
        );
    }
}
