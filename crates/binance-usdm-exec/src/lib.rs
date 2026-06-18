//! Binance USDM execution and account bookkeeping over user-data streams.
//!
//! Parses `ORDER_TRADE_UPDATE`, `ACCOUNT_UPDATE`, and related private stream events,
//! maintains per-symbol order/position state, and fans updates into
//! [`trolly_stream::MonitorMultiplexor`] ingress alongside other stream handlers.
//!
//! Outbound order placement is handled by [`order`] (REST `POST /fapi/v1/order`) and
//! [`adapter`] ([`StreamEgress`](trolly_strategy::StreamEgress) integration).

mod adapter;
mod auth;
mod endpoints;
mod handler;
mod ingress;
mod order;
mod parse;
mod types;

pub use adapter::{UsdmExecEgress, UsdmEgressError, run_order_worker};
pub use endpoints::{ApiCredentials, UsdmUserDataStream};
pub use endpoints::demo as usdm_demo_endpoints;
pub use order::{
    HttpOrderClient, OrderAck, OrderClient, OrderError, OrderRequest, OrderSide, OrderType,
    TimeInForce, REST_BASE_URL, place_order,
};
pub use handler::{UsdmExecHandler, ACCOUNT_ROUTING_ID};
pub use ingress::{build_multiplexor, build_multiplexor_with_account, ingest_user_data};
pub use parse::{parse_user_events, ParseError};
pub use types::{
    AccountBookkeeping, BalanceChange, MarginCall, MarginCallPosition, OrderTradeUpdate,
    PositionChange, SymbolBookkeeping, UsdmExec, UsdmExecUpdate,
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

    // ── MARGIN_CALL tests ────────────────────────────────────────────────────

    #[test]
    fn parse_margin_call_fixture() {
        let json = include_str!("../tests/fixtures/margin_call.json");
        let events = parse_user_events(Message::Text(json.into())).unwrap();
        assert_eq!(events.len(), 1);

        let UsdmExecUpdate::MarginCall(call) = &events[0] else {
            panic!("expected MarginCall, got {:?}", events[0]);
        };
        assert_eq!(call.event_time, 1587727187525);
        assert_eq!(call.cross_wallet_balance, "3.16812045");
        assert_eq!(call.positions.len(), 1);
        assert_eq!(call.positions[0].symbol, "ETHUSDT");
        assert_eq!(call.positions[0].position_side, "LONG");
        assert_eq!(call.positions[0].position_amount, "1.327");
        assert_eq!(call.positions[0].mark_price, "187.17127");
        assert_eq!(call.positions[0].maintenance_margin_required, "1.614445");
    }

    #[test]
    fn margin_call_routes_to_account_handler() {
        use crate::ingress::ingest_user_data;
        use crate::handler::ACCOUNT_ROUTING_ID;

        let json = include_str!("../tests/fixtures/margin_call.json");
        let account_rec = Arc::new(Mutex::new(Vec::new()));
        let btc_rec = Arc::new(Mutex::new(Vec::new()));

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

        ingest_user_data(&mut hub, Message::Text(json.into()));

        assert_eq!(account_rec.lock().unwrap().len(), 1, "margin call must route to account handler");
        assert!(btc_rec.lock().unwrap().is_empty(), "margin call must not route to symbol handler");
        assert!(matches!(
            account_rec.lock().unwrap()[0],
            UsdmExecUpdate::MarginCall(_)
        ));
    }

    #[test]
    fn margin_call_updates_account_bookkeeping_state() {
        use crate::ingress::{build_multiplexor_with_account, ingest_user_data};

        let json = include_str!("../tests/fixtures/margin_call.json");
        let (mut hub, account_bk) = build_multiplexor_with_account(&["ETHUSDT"], None);

        ingest_user_data(&mut hub, Message::Text(json.into()));

        let state = account_bk.lock().unwrap();
        let mc = state.latest_margin_call.as_ref().expect("latest_margin_call must be set");
        assert_eq!(mc.event_time, 1587727187525);
        assert_eq!(mc.cross_wallet_balance, "3.16812045");
        assert_eq!(mc.positions.len(), 1);
        assert_eq!(mc.positions[0].symbol, "ETHUSDT");
    }

    #[test]
    fn margin_call_superseded_by_newer_event_time() {
        let mut bk = AccountBookkeeping::default();

        let older = MarginCall {
            event_time: 1000,
            cross_wallet_balance: "10.0".into(),
            positions: vec![],
        };
        let newer = MarginCall {
            event_time: 2000,
            cross_wallet_balance: "5.0".into(),
            positions: vec![],
        };

        bk.apply_margin_call(&newer);
        bk.apply_margin_call(&older);

        assert_eq!(
            bk.latest_margin_call.as_ref().unwrap().event_time,
            2000,
            "older event must not supersede newer"
        );
        assert_eq!(bk.latest_margin_call.as_ref().unwrap().cross_wallet_balance, "5.0");
    }

    #[test]
    fn margin_call_forwarded_on_outbound_channel() {
        use crate::ingress::ingest_user_data;
        use crate::handler::ACCOUNT_ROUTING_ID;
        use tokio::sync::mpsc::unbounded_channel;

        let json = include_str!("../tests/fixtures/margin_call.json");
        let (tx, mut rx) = unbounded_channel::<UsdmExecUpdate>();

        let mut hub = MonitorMultiplexor::from_writers(HashMap::from([(
            ACCOUNT_ROUTING_ID.into(),
            UsdmExecHandler::new(ACCOUNT_ROUTING_ID, Some(tx)),
        )]));

        ingest_user_data(&mut hub, Message::Text(json.into()));

        let received = rx.try_recv().expect("outbound channel must receive MarginCall");
        assert!(matches!(received, UsdmExecUpdate::MarginCall(_)));
    }
}
