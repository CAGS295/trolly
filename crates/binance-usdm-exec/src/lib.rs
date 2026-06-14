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
    apply_margin_call, apply_position_change, is_flat_position, position_key, AccountBookkeeping,
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
        assert_eq!(handler.symbol_state().open_orders.len(), 1);

        let filled = UsdmExecHandler::parse_update(Message::Text(filled_json.into()))
            .unwrap()
            .unwrap();
        handler.handle_update(filled).unwrap();
        assert!(handler.symbol_state().open_orders.is_empty());
    }

    #[test]
    fn position_flat_removes_leg_from_symbol_and_account_maps() {
        use crate::types::{is_flat_position, position_key};

        assert!(is_flat_position("0"));
        assert!(is_flat_position("0.000"));

        let open_json = include_str!("../tests/fixtures/account_update_multi_leg.json");
        let flatten_json = include_str!("../tests/fixtures/account_update_flatten.json");

        let mut symbol_handler = UsdmExecHandler::new("BTCUSDT", None);
        let mut account_handler = UsdmExecHandler::new(ACCOUNT_ROUTING_ID, None);

        for event in parse_user_events(Message::Text(open_json.into())).unwrap() {
            if let UsdmExecUpdate::PositionChange(ref position) = event {
                if position.symbol == "BTCUSDT" {
                    symbol_handler.apply_bookkeeping(&event);
                }
                account_handler.apply_bookkeeping(&UsdmExecUpdate::PositionChange(position.clone()));
            }
        }

        assert_eq!(symbol_handler.symbol_state().positions.len(), 2);

        let flatten = parse_user_events(Message::Text(flatten_json.into()))
            .unwrap()
            .into_iter()
            .find_map(|e| match e {
                UsdmExecUpdate::PositionChange(p) => Some(p),
                _ => None,
            })
            .expect("flatten fixture has position row");

        symbol_handler.apply_bookkeeping(&UsdmExecUpdate::PositionChange(flatten.clone()));
        account_handler.apply_bookkeeping(&UsdmExecUpdate::PositionChange(flatten));

        assert!(!symbol_handler
            .symbol_state()
            .positions
            .contains_key(&position_key("BTCUSDT", "LONG")));
        assert!(!account_handler
            .account_state()
            .positions
            .contains_key(&position_key("BTCUSDT", "LONG")));
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
        assert_eq!(call.positions[0].maintenance_margin_required, "1.614445");
        assert_eq!(call.positions[1].symbol, "BTCUSDT");
        assert_eq!(UsdmExecUpdate::MarginCall(call.clone()).routing_id(), ACCOUNT_ROUTING_ID);
    }

    #[test]
    fn account_handler_records_margin_call_and_supersedes_on_newer() {
        let newer_json = include_str!("../tests/fixtures/margin_call.json");
        let older_json = include_str!("../tests/fixtures/margin_call_older.json");

        let mut account = UsdmExecHandler::new(ACCOUNT_ROUTING_ID, None);

        let older = parse_user_events(Message::Text(older_json.into()))
            .unwrap()
            .into_iter()
            .next()
            .unwrap();
        account.apply_bookkeeping(&older);

        let newer = parse_user_events(Message::Text(newer_json.into()))
            .unwrap()
            .into_iter()
            .next()
            .unwrap();
        account.apply_bookkeeping(&newer);

        let call = account
            .account_state()
            .latest_margin_call()
            .expect("margin call recorded");
        assert_eq!(call.event_time, 1587727187525);
        assert_eq!(call.cross_wallet_balance, "3.16812045");
        assert_eq!(call.positions.len(), 2);

        account.apply_bookkeeping(&older);
        let still_newer = account.account_state().latest_margin_call().unwrap();
        assert_eq!(still_newer.event_time, 1587727187525);
    }

    #[test]
    fn margin_call_forwards_on_outbound_channel() {
        let json = include_str!("../tests/fixtures/margin_call.json");
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

        let mut account = UsdmExecHandler::new(ACCOUNT_ROUTING_ID, Some(tx));
        let update = parse_user_events(Message::Text(json.into()))
            .unwrap()
            .into_iter()
            .next()
            .unwrap();
        account.handle_update(update).unwrap();

        let outbound = rx.try_recv().expect("margin call forwarded");
        assert!(matches!(outbound, UsdmExecUpdate::MarginCall(_)));
        assert!(account.account_state().latest_margin_call().is_some());
    }

    #[test]
    fn symbol_handler_ignores_margin_call_bookkeeping() {
        let json = include_str!("../tests/fixtures/margin_call.json");
        let mut symbol = UsdmExecHandler::new("ETHUSDT", None);
        let update = parse_user_events(Message::Text(json.into()))
            .unwrap()
            .into_iter()
            .next()
            .unwrap();
        symbol.apply_bookkeeping(&update);
        assert!(symbol.account_state().latest_margin_call().is_none());
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
}
