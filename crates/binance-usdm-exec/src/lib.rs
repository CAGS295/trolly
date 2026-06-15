//! Binance USDM execution and account bookkeeping over user-data streams.
//!
//! Parses `ORDER_TRADE_UPDATE`, `ACCOUNT_UPDATE`, and related private stream events,
//! maintains per-symbol order/position state, and fans updates into
//! [`trolly_stream::MonitorMultiplexor`] ingress alongside other stream handlers.

mod endpoints;
mod handler;
mod ingress;
mod parse;
mod types;

pub use endpoints::UsdmUserDataStream;
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
