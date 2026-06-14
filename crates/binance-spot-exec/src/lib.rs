//! Binance spot execution and account bookkeeping over user-data streams.
//!
//! See [`README.md`](../README.md) for WebSocket subscription setup and multiplexor routing.

mod account;
mod auth;
mod egress;
mod endpoints;
mod events;
mod handler;
mod ingress;
mod order;
mod parse;

pub use account::AccountBook;
pub use auth::{
    build_subscribe_signature_params, current_timestamp_ms, sign_hmac_sha256_hex, sign_params,
    signed_params_payload,
};
pub use egress::SpotOrderEgress;
pub use endpoints::{
    ApiCredentials, BinanceSpotUserStream, DEMO_MARKET_STREAM_URL, DEMO_WS_API_URL,
};
pub use events::{
    AssetBalance, BalanceUpdate, EventStreamTerminated, ExecutionReport, ExternalLockUpdate,
    ListStatus, ListStatusOrder, OutboundAccountPosition, SpotUserEvent, ACCOUNT_ROUTE_ID,
};
pub use handler::{SpotExecContext, SpotExecHandler};
pub use ingress::{build_multiplexor, ingest_user_data};
pub use order::{
    NewOrderRequest, NewOrderResponse, OrderSide, OrderType, SpotOrderClient, SpotOrderError,
    TimeInForce, DEFAULT_REST_BASE, DEMO_REST_BASE, ORDER_PATH, demo_depth_rest_url,
};
pub use parse::{ParseError, parse_user_data_message};

/// Append [`ACCOUNT_ROUTE_ID`] when absent so account events route through the multiplexor.
pub fn exec_subscription_symbols(symbols: &[impl AsRef<str>]) -> Vec<String> {
    let mut out: Vec<String> = symbols
        .iter()
        .map(|symbol| symbol.as_ref().to_uppercase())
        .collect();
    if !out.iter().any(|symbol| symbol == ACCOUNT_ROUTE_ID) {
        out.push(ACCOUNT_ROUTE_ID.into());
    }
    out
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use trolly_stream::{EventHandler, Message};
    use tokio::sync::mpsc;

    use super::*;

    #[test]
    fn parse_execution_report_fixture() {
        let msg = Message::Text(
            include_str!("../tests/fixtures/execution_report.json").into(),
        );
        let event = parse_user_data_message(msg).unwrap().unwrap();
        let SpotUserEvent::ExecutionReport(report) = event else {
            panic!("expected execution report");
        };
        assert_eq!(report.symbol, "ETHBTC");
        assert_eq!(report.order_id, 4_293_153);
        assert_eq!(report.execution_type, "NEW");
        assert_eq!(report.order_status, "NEW");
    }

    #[test]
    fn parse_outbound_account_position_fixture() {
        let msg = Message::Text(
            include_str!("../tests/fixtures/outbound_account_position.json").into(),
        );
        let event = parse_user_data_message(msg).unwrap().unwrap();
        let SpotUserEvent::OutboundAccountPosition(pos) = event else {
            panic!("expected outbound account position");
        };
        assert_eq!(pos.balances.len(), 2);
        assert_eq!(pos.balances[0].asset, "BTC");
    }

    #[test]
    fn parse_balance_update_fixture() {
        let msg = Message::Text(include_str!("../tests/fixtures/balance_update.json").into());
        let event = parse_user_data_message(msg).unwrap().unwrap();
        let SpotUserEvent::BalanceUpdate(update) = event else {
            panic!("expected balance update");
        };
        assert_eq!(update.asset, "BTC");
        assert_eq!(update.delta, "100.00000000");
    }

    #[test]
    fn parse_legacy_unwrapped_execution_report() {
        let msg = Message::Text(
            include_str!("../tests/fixtures/execution_report_legacy.json").into(),
        );
        let event = parse_user_data_message(msg).unwrap().unwrap();
        assert!(matches!(event, SpotUserEvent::ExecutionReport(_)));
    }

    #[test]
    fn subscribe_message_is_websocket_api_signature_subscription() {
        let stream = BinanceSpotUserStream::new(ApiCredentials {
            api_key: "test-key".into(),
            secret_key: "test-secret".into(),
        });
        let payload = stream.subscribe_request_json();
        let value: serde_json::Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(
            value["method"].as_str(),
            Some("userDataStream.subscribe.signature")
        );
        assert!(value["params"]["apiKey"].is_string());
        assert!(value["params"]["timestamp"].is_string());
        assert!(value["params"]["signature"].is_string());
    }

    #[test]
    fn exec_subscription_symbols_appends_account_route() {
        let symbols = exec_subscription_symbols(&["btcusdt", "ETHUSDT"]);
        assert_eq!(symbols, vec!["BTCUSDT", "ETHUSDT", ACCOUNT_ROUTE_ID]);
    }

    #[test]
    fn account_book_applies_position_and_balance_delta() {
        let mut book = AccountBook::default();
        let position = parse_user_data_message(Message::Text(
            include_str!("../tests/fixtures/outbound_account_position.json").into(),
        ))
        .unwrap()
        .unwrap();
        book.apply(&position);

        let update = parse_user_data_message(Message::Text(
            include_str!("../tests/fixtures/balance_update.json").into(),
        ))
            .unwrap()
            .unwrap();
        book.apply(&update);

        let btc = book.balances.get("BTC").expect("btc balance");
        assert_eq!(btc.free, "11918.00000000");
    }

    #[test]
    fn handlers_route_execution_and_account_events() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let ctx = SpotExecContext {
            events: tx,
            account: Arc::new(Mutex::new(AccountBook::default())),
        };

        let mut symbol_handler = SpotExecHandler::new("ETHBTC", ctx.clone());
        let mut account_handler = SpotExecHandler::new(ACCOUNT_ROUTE_ID, ctx);

        let execution = parse_user_data_message(Message::Text(
            include_str!("../tests/fixtures/execution_report.json").into(),
        ))
        .unwrap()
        .unwrap();
        assert_eq!(SpotExecHandler::to_id(&execution), "ETHBTC");
        symbol_handler.handle_update(execution).unwrap();

        let position = parse_user_data_message(Message::Text(
            include_str!("../tests/fixtures/outbound_account_position.json").into(),
        ))
        .unwrap()
        .unwrap();
        assert_eq!(SpotExecHandler::to_id(&position), ACCOUNT_ROUTE_ID);
        account_handler.handle_update(position).unwrap();

        let balance = parse_user_data_message(Message::Text(
            include_str!("../tests/fixtures/balance_update.json").into(),
        ))
        .unwrap()
        .unwrap();
        account_handler.handle_update(balance).unwrap();

        let first = rx.try_recv().unwrap();
        assert!(matches!(first, SpotUserEvent::ExecutionReport(_)));

        let second = rx.try_recv().unwrap();
        assert!(matches!(
            second,
            SpotUserEvent::OutboundAccountPosition(_)
        ));

        let third = rx.try_recv().unwrap();
        assert!(matches!(third, SpotUserEvent::BalanceUpdate(_)));
    }
}
