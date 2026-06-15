//! Binance USDM execution: signed REST order placement and account bookkeeping over user-data streams.
//!
//! Parses `ORDER_TRADE_UPDATE`, `ACCOUNT_UPDATE`, and related private stream events,
//! maintains per-symbol order/position state, and fans updates into
//! [`trolly_stream::MonitorMultiplexor`] ingress alongside other stream handlers.
//!
//! See [`README.md`](../README.md) for outbound order placement and stream subscription setup.

mod auth;
mod egress;
mod endpoints;
mod handler;
mod ingress;
mod order;
mod parse;
mod types;

pub use auth::{current_timestamp_ms, sign_hmac_sha256_hex};
pub use egress::UsdmRestEgress;
pub use endpoints::{ApiCredentials, UsdmUserDataStream};
pub use order::{
    build_signed_order_form, parse_order_side, parse_position_side, OrderError, OrderSide,
    OrderType, PositionSide, TimeInForce, UsdmOrderClient, UsdmOrderRequest, UsdmOrderResponse,
    DEFAULT_REST_BASE_URL,
};
pub use handler::{UsdmExecHandler, ACCOUNT_ROUTING_ID};
pub use ingress::{build_multiplexor, ingest_user_data};
pub use parse::{parse_user_events, ParseError};
pub use types::{
    position_amount_is_zero, BalanceChange, MarginCall, MarginCallPosition, OrderTradeUpdate,
    PositionChange, PositionKey, SymbolBookkeeping, UsdmExec, UsdmExecUpdate,
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
    fn parse_margin_call_fixture() {
        let json = include_str!("../tests/fixtures/margin_call.json");
        let events = parse_user_events(Message::Text(json.into())).unwrap();
        assert_eq!(events.len(), 1);

        let UsdmExecUpdate::MarginCall(call) = &events[0] else {
            panic!("expected margin call update");
        };
        assert_eq!(call.event_time, 1587727187525);
        assert_eq!(call.cross_wallet_balance, "3.16812045");
        assert_eq!(call.positions.len(), 2);
        assert_eq!(call.positions[0].symbol, "ETHUSDT");
        assert_eq!(call.positions[0].position_side, "LONG");
        assert_eq!(call.positions[0].position_amount, "1.327");
        assert_eq!(call.positions[0].maintenance_margin_required, "1.614445");
        assert_eq!(call.positions[1].symbol, "BTCUSDT");
        assert_eq!(call.positions[1].position_side, "SHORT");
    }

    #[test]
    fn handler_records_latest_margin_call_and_supersedes_on_event_time() {
        let older_json = include_str!("../tests/fixtures/margin_call_older.json");
        let newer_json = include_str!("../tests/fixtures/margin_call.json");

        let mut handler = UsdmExecHandler::new(ACCOUNT_ROUTING_ID, None);

        let newer = UsdmExecHandler::parse_update(Message::Text(newer_json.into()))
            .unwrap()
            .unwrap();
        handler.handle_update(newer).unwrap();

        let state = handler.state();
        assert_eq!(state.cross_wallet_balance, "3.16812045");
        let latest = state.latest_margin_call.as_ref().unwrap();
        assert_eq!(latest.event_time, 1587727187525);
        assert_eq!(latest.positions.len(), 2);

        let older = UsdmExecHandler::parse_update(Message::Text(older_json.into()))
            .unwrap()
            .unwrap();
        handler.handle_update(older).unwrap();

        let state = handler.state();
        assert_eq!(state.cross_wallet_balance, "3.16812045");
        assert_eq!(
            state.latest_margin_call.as_ref().unwrap().event_time,
            1587727187525
        );
    }

    #[test]
    fn handler_forwards_margin_call_on_outbound_channel() {
        let json = include_str!("../tests/fixtures/margin_call.json");
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut handler = UsdmExecHandler::new(ACCOUNT_ROUTING_ID, Some(tx));

        let update = UsdmExecHandler::parse_update(Message::Text(json.into()))
            .unwrap()
            .unwrap();
        handler.handle_update(update).unwrap();

        let forwarded = rx.try_recv().unwrap();
        assert!(matches!(forwarded, UsdmExecUpdate::MarginCall(_)));
    }

    #[test]
    fn ingest_margin_call_routes_to_account_handler() {
        let margin_call_json = include_str!("../tests/fixtures/margin_call.json");
        let btc_rec = Arc::new(Mutex::new(Vec::new()));
        let account_rec = Arc::new(Mutex::new(Vec::new()));

        let mut hub = MonitorMultiplexor::from_writers(HashMap::from([
            (
                "BTCUSDT".into(),
                UsdmExecHandler::with_recorder("BTCUSDT", btc_rec.clone()),
            ),
            (
                ACCOUNT_ROUTING_ID.into(),
                UsdmExecHandler::with_recorder(ACCOUNT_ROUTING_ID, account_rec.clone()),
            ),
        ]));

        ingest_user_data(&mut hub, Message::Text(margin_call_json.into()));

        assert!(btc_rec.lock().unwrap().is_empty());
        assert_eq!(account_rec.lock().unwrap().len(), 1);
        assert!(matches!(
            account_rec.lock().unwrap()[0],
            UsdmExecUpdate::MarginCall(_)
        ));

        let account_state = hub.writers.get(ACCOUNT_ROUTING_ID).unwrap().state();
        assert_eq!(account_state.cross_wallet_balance, "3.16812045");
        let latest = account_state.latest_margin_call.as_ref().unwrap();
        assert_eq!(latest.event_time, 1587727187525);
        assert_eq!(latest.positions.len(), 2);
        assert_eq!(latest.positions[0].symbol, "ETHUSDT");
        assert_eq!(latest.positions[1].symbol, "BTCUSDT");
        assert_eq!(UsdmExecHandler::to_id(&account_rec.lock().unwrap()[0]), ACCOUNT_ROUTING_ID);
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
    fn position_amount_is_zero_detects_closed_positions() {
        assert!(position_amount_is_zero("0"));
        assert!(position_amount_is_zero("0.000"));
        assert!(position_amount_is_zero("-0"));
        assert!(!position_amount_is_zero("20"));
        assert!(!position_amount_is_zero("-10"));
    }

    #[test]
    fn handler_position_bookkeeping_uses_composite_keys_and_removes_flatten() {
        let open_json = include_str!("../tests/fixtures/account_update.json");
        let flatten_json = include_str!("../tests/fixtures/account_update_flatten.json");

        let mut handler = UsdmExecHandler::new("BTCUSDT", None);
        for event in parse_user_events(Message::Text(open_json.into())).unwrap() {
            if let UsdmExecUpdate::PositionChange(position) = event {
                handler.apply_position_bookkeeping(&position);
            }
        }

        let long_key = PositionKey::new("BTCUSDT", "LONG");
        let short_key = PositionKey::new("BTCUSDT", "SHORT");
        assert_eq!(handler.state().positions.len(), 2);
        assert!(handler.state().positions.contains_key(&long_key));
        assert!(handler.state().positions.contains_key(&short_key));

        for event in parse_user_events(Message::Text(flatten_json.into())).unwrap() {
            if let UsdmExecUpdate::PositionChange(position) = event {
                handler.apply_position_bookkeeping(&position);
            }
        }

        assert_eq!(handler.state().positions.len(), 1);
        assert!(!handler.state().positions.contains_key(&long_key));
        assert!(handler.state().positions.contains_key(&short_key));
    }

    #[test]
    fn ingest_account_update_persists_both_leg_on_account_and_symbol_handlers() {
        let both_json = include_str!("../tests/fixtures/account_update_both.json");
        let mut hub = build_multiplexor(&["ETHUSDT"], None);

        ingest_user_data(&mut hub, Message::Text(both_json.into()));

        let both_key = PositionKey::new("ETHUSDT", "BOTH");
        let eth_state = hub.writers.get("ETHUSDT").unwrap().state();
        assert_eq!(eth_state.positions.len(), 1);
        assert_eq!(
            eth_state.positions.get(&both_key).unwrap().position_amount,
            "2.500"
        );

        let account_state = hub.writers.get(ACCOUNT_ROUTING_ID).unwrap().state();
        assert_eq!(account_state.positions.len(), 1);
        assert_eq!(
            account_state.positions.get(&both_key).unwrap().position_side,
            "BOTH"
        );
        assert!(account_state.open_orders.is_empty());
    }

    #[test]
    fn ingest_account_update_flattens_account_wide_positions() {
        let open_json = include_str!("../tests/fixtures/account_update.json");
        let flatten_json = include_str!("../tests/fixtures/account_update_flatten.json");
        let mut hub = build_multiplexor(&["BTCUSDT", "ETHUSDT"], None);

        ingest_user_data(&mut hub, Message::Text(open_json.into()));
        let account = hub.writers.get(ACCOUNT_ROUTING_ID).unwrap().state();
        assert_eq!(account.positions.len(), 2);

        ingest_user_data(&mut hub, Message::Text(flatten_json.into()));
        let account = hub.writers.get(ACCOUNT_ROUTING_ID).unwrap().state();
        let long_key = PositionKey::new("BTCUSDT", "LONG");
        assert_eq!(account.positions.len(), 1);
        assert!(!account.positions.contains_key(&long_key));
        assert!(account
            .positions
            .contains_key(&PositionKey::new("BTCUSDT", "SHORT")));
    }
}
