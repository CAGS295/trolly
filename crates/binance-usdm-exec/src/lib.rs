//! Binance USDM execution and account bookkeeping over user-data streams.
//!
//! See [`README.md`](../README.md) for WebSocket subscription setup, multiplexor routing,
//! and REST order placement.
//!
//! ## `MARGIN_CALL` lifecycle
//!
//! Margin-call events route to the [`handler::ACCOUNT_ROUTING_ID`] handler. Each call
//! carries a cross-wallet balance and a snapshot of affected position legs. Bookkeeping
//! keeps only the latest snapshot: a call supersedes the previous one when its
//! `event_time` is greater than or equal to the stored call. Older calls are dropped
//! from state but are still forwarded on the outbound channel when received.

mod account;
mod auth;
mod egress;
mod endpoints;
mod handler;
mod ingress;
mod order;
mod parse;
mod types;

pub use account::{is_position_closed, UsdmAccountBookkeeping};
pub use auth::{current_timestamp_ms, sign_hmac_sha256_hex, signed_params_payload};
pub use egress::{
    EgressError, UsdmOrderEgress, UsdmOrderEgressDirect, place_order_from_outbound,
    run_order_executor,
};
pub use endpoints::{ApiCredentials, UsdmUserDataStream};
pub use order::{
    BinanceApiErrorBody, HttpResponse, NativeTlsTransport, OrderBuilderError, OrderSide,
    OrderTransport, OrderType, PlaceOrderError, PlaceOrderRequest, PlaceOrderResponse,
    PositionSide, REST_BASE_URL, TimeInForce, UsdmOrderClient, build_order_params,
    build_signed_form_params, parse_place_order_response,
};
pub use handler::{UsdmExecContext, UsdmExecHandler, ACCOUNT_ROUTING_ID};
pub use ingress::{build_multiplexor, build_multiplexor_with_context, ingest_user_data};
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

    fn test_ctx() -> UsdmExecContext {
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
    fn parse_margin_call_fixture() {
        let json = include_str!("../tests/fixtures/margin_call.json");
        let events = parse_user_events(Message::Text(json.into())).unwrap();
        assert_eq!(events.len(), 1);

        let UsdmExecUpdate::MarginCall(call) = &events[0] else {
            panic!("expected margin call");
        };
        assert_eq!(call.event_time, 1587727187525);
        assert_eq!(call.cross_wallet_balance, "3.16812045");
        assert_eq!(call.positions.len(), 1);
        assert_eq!(call.positions[0].symbol, "ETHUSDT");
        assert_eq!(call.positions[0].maintenance_margin_required, "1.614445");
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
    fn account_bookkeeping_query_api() {
        let json = include_str!("../tests/fixtures/account_update.json");
        let ctx = test_ctx();
        let account = ctx.account.clone();
        let mut hub = build_multiplexor_with_context(&["BTCUSDT"], ctx);

        ingest_user_data(&mut hub, Message::Text(json.into()));

        let book = account.lock().unwrap();
        let long = book.position("BTCUSDT", "LONG").unwrap();
        assert_eq!(long.position_amount, "20");
        let short = book.position("BTCUSDT", "SHORT").unwrap();
        assert_eq!(short.position_amount, "-10");
        assert_eq!(book.balance("USDT").unwrap().wallet_balance, "122624.12345678");
        assert_eq!(book.positions_for_symbol("BTCUSDT").count(), 2);
    }

    #[test]
    fn handler_tracks_open_and_closed_orders() {
        let new_json = include_str!("../tests/fixtures/order_trade_update.json");
        let filled_json = include_str!("../tests/fixtures/order_trade_update_filled.json");

        let recorded = Arc::new(Mutex::new(Vec::new()));
        let mut handler = UsdmExecHandler::with_recorder("BTCUSDT", recorded, test_ctx());

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
    fn margin_call_forwarded_on_outbound_channel() {
        let json = include_str!("../tests/fixtures/margin_call.json");
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let ctx = UsdmExecContext::new(Some(tx));
        let mut handler = UsdmExecHandler::new(ACCOUNT_ROUTING_ID, ctx);

        let update = UsdmExecHandler::parse_update(Message::Text(json.into()))
            .unwrap()
            .unwrap();
        handler.handle_update(update).unwrap();

        let outbound = rx.try_recv().unwrap();
        assert!(matches!(outbound, UsdmExecUpdate::MarginCall(_)));
        assert!(handler.state().latest_margin_call.is_some());
    }

    #[test]
    fn multiplexor_ingest_message_routes_single_event() {
        let json = include_str!("../tests/fixtures/order_trade_update.json");
        let recorded = Arc::new(Mutex::new(Vec::new()));

        let mut hub = MonitorMultiplexor::from_writers(HashMap::from([(
            "BTCUSDT".into(),
            UsdmExecHandler::with_recorder("BTCUSDT", recorded.clone(), test_ctx()),
        )]));

        hub.ingest_message(Message::Text(json.into()));

        assert_eq!(recorded.lock().unwrap().len(), 1);
    }
}
