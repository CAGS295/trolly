//! Binance spot execution and account bookkeeping over user-data websocket streams.
//!
//! This crate parses Binance spot **user data stream** events (`executionReport`,
//! `outboundAccountPosition`, `balanceUpdate`, etc.) and routes them through
//! [`trolly_stream::MonitorMultiplexor`] using the shared injectable ingress API.
//!
//! # Subscription flow
//!
//! 1. Open a WebSocket API session at [`endpoints::WS_API_STREAM`]
//!    (`wss://ws-api.binance.com:443/ws-api/v3`).
//! 2. Authenticate with `session.logon` **or** send a signed
//!    `userDataStream.subscribe.signature` payload from [`BinanceSpotUserStream`].
//! 3. Fan received text frames into the multiplexor via [`MessageIngress::push`]
//!    or [`ingress::parse_and_push`].
//! 4. Register handlers with symbols you care about (e.g. `BTCUSDT`, `ETHBTC`) plus
//!    [`ACCOUNT_ROUTE_ID`] (`"__account__"`) for balance and account-position events.
//!
//! Order placement and cancellation are **not** implemented here; this crate is
//! stream-native bookkeeping only (no REST trading endpoints).
//!
//! [`endpoints::WS_API_STREAM`]: endpoints::WS_API_STREAM
//! [`BinanceSpotUserStream`]: endpoints::BinanceSpotUserStream
//! [`MessageIngress::push`]: trolly_stream::MessageIngress::push
//! [`ingress::parse_and_push`]: ingress::parse_and_push
//! [`ACCOUNT_ROUTE_ID`]: ACCOUNT_ROUTE_ID

mod endpoints;
mod handler;
mod ingress;
mod parse;
mod types;

pub use endpoints::{BinanceSpotUserStream, WS_API_STREAM};
pub use handler::{SpotExecContext, SpotExecHandler};
pub use ingress::{parse_and_push, push_raw_user_data, IngressError};
pub use parse::{parse_user_data_message, ParseError};
pub use types::{
    AssetBalance, BalanceUpdate, EventStreamTerminated, ExecutionReport, ExternalLockUpdate,
    ListStatus, ListStatusOrder, OrderSide, OutboundAccountPosition, SpotExec, SpotUserDataEvent,
    ACCOUNT_ROUTE_ID,
};
pub use trolly_stream;

/// Crate identifier for workspace wiring tests.
pub const CRATE: &str = env!("CARGO_PKG_NAME");

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::fs;
    use std::path::PathBuf;
    use std::rc::Rc;
    use trolly_stream::{
        message_ingress, EventHandler, Message, MonitorMultiplexor, RouteOutcome,
    };

    fn fixture(name: &str) -> String {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name);
        fs::read_to_string(path).expect("fixture file")
    }

    #[test]
    fn workspace_crate_name() {
        assert_eq!(CRATE, "binance-spot-exec");
    }

    #[test]
    fn parses_execution_report_fixture() {
        let text = fixture("execution_report_new.json");
        let event = parse_user_data_message(&text)
            .expect("parse")
            .expect("event");

        assert_eq!(event.route_id(), "ETHBTC");
        match event {
            SpotUserDataEvent::ExecutionReport(report) => {
                assert_eq!(report.order_status, "NEW");
                assert_eq!(report.execution_type, "NEW");
                assert_eq!(report.order_id, 4_293_153);
                assert_eq!(report.side, OrderSide::Buy);
                assert_eq!(report.subscription_id, Some(0));
            }
            other => panic!("expected execution report, got {other:?}"),
        }
    }

    #[test]
    fn parses_outbound_account_position_fixture() {
        let text = fixture("outbound_account_position.json");
        let event = parse_user_data_message(&text)
            .expect("parse")
            .expect("event");

        assert_eq!(event.route_id(), ACCOUNT_ROUTE_ID);
        match event {
            SpotUserDataEvent::OutboundAccountPosition(pos) => {
                assert_eq!(pos.balances.len(), 2);
                assert_eq!(pos.balances[0].asset, "ETH");
                assert_eq!(pos.balances[1].free, "1.50000000");
            }
            other => panic!("expected outboundAccountPosition, got {other:?}"),
        }
    }

    #[test]
    fn parses_balance_update_fixture() {
        let text = fixture("balance_update.json");
        let event = parse_user_data_message(&text)
            .expect("parse")
            .expect("event");

        assert_eq!(event.route_id(), ACCOUNT_ROUTE_ID);
        match event {
            SpotUserDataEvent::BalanceUpdate(update) => {
                assert_eq!(update.asset, "BTC");
                assert_eq!(update.balance_delta, "100.00000000");
                assert_eq!(update.clear_time, 1_573_200_697_068);
            }
            other => panic!("expected balanceUpdate, got {other:?}"),
        }
    }

    #[test]
    fn parses_legacy_listen_key_payload_without_envelope() {
        let text = r#"{
            "e": "balanceUpdate",
            "E": 1573200697110,
            "a": "USDT",
            "d": "10.00000000",
            "T": 1573200697068
        }"#;

        let event = parse_user_data_message(text)
            .expect("parse")
            .expect("event");
        assert_eq!(event.route_id(), ACCOUNT_ROUTE_ID);
    }

    #[test]
    fn handler_records_execution_report_for_symbol() {
        let events: SpotExecContext = Rc::new(RefCell::new(Vec::new()));
        let mut handler = SpotExecHandler::for_route("ETHBTC", events.clone());
        let text = fixture("execution_report_new.json");
        let event = parse_user_data_message(&text)
            .expect("parse")
            .expect("event");

        assert_eq!(SpotExecHandler::to_id(&event), "ETHBTC");
        handler.handle_update(event).expect("handle");
        assert_eq!(handler.recorded_events().len(), 1);
    }

    #[test]
    fn handler_records_balance_update_for_account_route() {
        let events: SpotExecContext = Rc::new(RefCell::new(Vec::new()));
        let mut handler = SpotExecHandler::for_route(ACCOUNT_ROUTE_ID, events.clone());
        let text = fixture("balance_update.json");
        let event = parse_user_data_message(&text)
            .expect("parse")
            .expect("event");

        assert_eq!(SpotExecHandler::to_id(&event), ACCOUNT_ROUTE_ID);
        handler.handle_update(event).expect("handle");
        assert!(matches!(
            handler.recorded_events()[0],
            SpotUserDataEvent::BalanceUpdate(_)
        ));
    }

    #[tokio::test]
    async fn multiplexor_build_routes_fixture_via_handler() {
        tokio::task::LocalSet::new()
            .run_until(async {
                let events: SpotExecContext = Rc::new(RefCell::new(Vec::new()));
                let provider = BinanceSpotUserStream {
                    api_key: String::new(),
                    api_secret: String::new(),
                };
                let mut mux = MonitorMultiplexor::<SpotExecHandler, SpotExec>::build(
                    provider,
                    &["ETHBTC"],
                    events.clone(),
                )
                .await
                .expect("build");

                let text = fixture("execution_report_new.json");
                let outcome = mux.route_message(Message::Text(text.into()));
                assert_eq!(outcome, RouteOutcome::Handled);
                assert_eq!(events.borrow().len(), 1);
            })
            .await;
    }

    #[test]
    fn ingress_push_queues_raw_frame() {
        let events: SpotExecContext = Rc::new(RefCell::new(Vec::new()));
        let mut handler = SpotExecHandler::for_route("ETHBTC", events.clone());
        let (ingress, mut rx) = message_ingress();
        let text = fixture("execution_report_new.json");
        parse_and_push(&ingress, &text).expect("push");

        let msg = rx.try_recv().expect("queued frame");
        let event = SpotExecHandler::parse_update(msg)
            .expect("parse frame")
            .expect("event");
        handler.handle_update(event).expect("handle");
        assert_eq!(handler.recorded_events().len(), 1);
    }
}
