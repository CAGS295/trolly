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
pub use ingress::{account_state, build_multiplexor, ingest_user_data};
pub use parse::{parse_user_events, ParseError};
pub use types::{
    BalanceChange, MarginCall, MarginCallPosition, OrderTradeUpdate, PositionChange, PositionKey,
    SymbolBookkeeping, UsdmExec, UsdmExecUpdate, position_is_flat,
};

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    use trolly_stream::{EventHandler, Message, MonitorMultiplexor};

    #[test]
    fn position_is_flat_detects_zero_amounts() {
        assert!(position_is_flat("0"));
        assert!(position_is_flat("0.0"));
        assert!(position_is_flat("0.00000000"));
        assert!(position_is_flat("-0"));
        assert!(!position_is_flat("20"));
        assert!(!position_is_flat("-10"));
    }

    #[test]
    fn handler_position_bookkeeping_on_account_handler() {
        let account_json = include_str!("../tests/fixtures/account_update.json");
        let events = parse_user_events(Message::Text(account_json.into())).unwrap();

        let mut handler = UsdmExecHandler::new(ACCOUNT_ROUTING_ID, None);
        for event in events {
            handler.handle_update(event).unwrap();
        }

        let state = handler.state();
        assert_eq!(state.positions.len(), 2);
        assert_eq!(
            state.position("BTCUSDT", "LONG").unwrap().position_amount,
            "20"
        );
        assert_eq!(
            state.position("BTCUSDT", "SHORT").unwrap().position_amount,
            "-10"
        );
        assert_eq!(state.positions_for_symbol("BTCUSDT").count(), 2);
    }

    #[test]
    fn handler_position_flatten_removes_leg() {
        let open_json = include_str!("../tests/fixtures/account_update.json");
        let flatten_json = include_str!("../tests/fixtures/account_update_flatten_long.json");

        let mut handler = UsdmExecHandler::new(ACCOUNT_ROUTING_ID, None);
        for event in parse_user_events(Message::Text(open_json.into())).unwrap() {
            handler.handle_update(event).unwrap();
        }
        assert_eq!(handler.state().positions.len(), 2);

        for event in parse_user_events(Message::Text(flatten_json.into())).unwrap() {
            handler.handle_update(event).unwrap();
        }
        assert_eq!(handler.state().positions.len(), 1);
        assert!(handler.state().position("BTCUSDT", "LONG").is_none());
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

        let account_json = include_str!("../tests/fixtures/account_update.json");
        let events = parse_user_events(Message::Text(account_json.into())).unwrap();
        for event in events {
            assert_eq!(UsdmExecHandler::to_id(&event), ACCOUNT_ROUTING_ID);
        }
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
}
