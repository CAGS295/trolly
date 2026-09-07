use std::cell::Cell;
use std::time::Duration;

use binance_spot_exec::{
    OrderSide as SpotOrderSide, OrderType as SpotOrderType,
    PlaceOrderResponse as SpotPlaceOrderResponse,
};
use binance_usdm_exec::{
    OrderSide as UsdmOrderSide, OrderType as UsdmOrderType,
    PlaceOrderResponse as UsdmPlaceOrderResponse,
};
use trolly::policy_demo::{
    policy_demo_user_data_messages_from_json, reconcile_policy_demo_report,
    run_policy_demo_with_policy, run_spot_policy_demo_with_placer,
    run_spot_policy_demo_with_placer_and_user_data, run_usdm_policy_demo_with_placer,
    run_usdm_policy_demo_with_placer_and_user_data, DemoVenue, PolicyDemoConfig, PolicyDemoError,
    PolicyDemoOrders,
};
use trolly_gym::{Action, PolicyProvider};

struct SequencePolicy {
    actions: Vec<Action>,
    next: Cell<usize>,
}

impl SequencePolicy {
    fn hold_buy_sell() -> Self {
        Self {
            actions: vec![Action::Hold, Action::Buy, Action::Sell],
            next: Cell::new(0),
        }
    }
}

impl PolicyProvider for SequencePolicy {
    fn act(&self, _obs: &[f32]) -> Action {
        let idx = self.next.get();
        self.next.set(idx + 1);
        self.actions.get(idx).copied().unwrap_or(Action::Hold)
    }
}

#[tokio::test]
async fn policy_demo_dry_run_generates_spot_requests() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT");
    config.max_steps = 3;
    config.qty = "0.002".into();
    let policy = SequencePolicy::hold_buy_sell();

    let report = run_policy_demo_with_policy(config, &policy, "test-sequence")
        .await
        .unwrap();

    assert_eq!(report.steps, 3);
    assert_eq!(report.placed_orders, 0);
    assert_eq!(report.policy_source, "test-sequence");

    let PolicyDemoOrders::Spot(orders) = report.orders else {
        panic!("expected spot orders");
    };
    assert_eq!(orders.len(), 2);
    assert_eq!(orders[0].symbol, "BTCUSDT");
    assert_eq!(orders[0].side, SpotOrderSide::Buy);
    assert_eq!(orders[0].order_type, SpotOrderType::Market);
    assert_eq!(orders[0].quantity, "0.002");
    assert_eq!(
        orders[0].new_client_order_id.as_deref(),
        Some("trolly-demo-spot-0000")
    );
    assert_eq!(orders[1].side, SpotOrderSide::Sell);
}

#[tokio::test]
async fn policy_demo_dry_run_generates_usdm_requests() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Usdm, "ETHUSDT");
    config.max_steps = 3;
    config.qty = "0.003".into();
    let policy = SequencePolicy::hold_buy_sell();

    let report = run_policy_demo_with_policy(config, &policy, "test-sequence")
        .await
        .unwrap();

    assert_eq!(report.steps, 3);
    assert_eq!(report.placed_orders, 0);

    let PolicyDemoOrders::Usdm(orders) = report.orders else {
        panic!("expected USDM orders");
    };
    assert_eq!(orders.len(), 2);
    assert_eq!(orders[0].symbol, "ETHUSDT");
    assert_eq!(orders[0].side, UsdmOrderSide::Buy);
    assert_eq!(orders[0].order_type, UsdmOrderType::Market);
    assert_eq!(orders[0].quantity, "0.003");
    assert_eq!(orders[0].position_side, None);
    assert_eq!(
        orders[0].new_client_order_id.as_deref(),
        Some("trolly-demo-usdm-0000")
    );
    assert_eq!(orders[1].side, UsdmOrderSide::Sell);
}

#[tokio::test]
async fn policy_demo_refuses_demo_orders_without_guard() {
    let guard_var = format!("TROLLY_TEST_DEMO_ORDER_GUARD_{}", std::process::id());
    std::env::remove_var(&guard_var);

    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT");
    config.execute_demo_orders = true;
    config.demo_order_guard_var = guard_var.clone();
    let policy = SequencePolicy::hold_buy_sell();

    let err = run_policy_demo_with_policy(config, &policy, "test-sequence")
        .await
        .unwrap_err();

    assert!(matches!(
        err,
        PolicyDemoError::MissingDemoOrderGuard { var } if var == guard_var
    ));
}

#[tokio::test]
async fn policy_demo_refuses_live_user_data_without_demo_order_execution() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT");
    config.wait_for_user_data = true;
    let policy = SequencePolicy::hold_buy_sell();

    let err = run_policy_demo_with_policy(config, &policy, "test-sequence")
        .await
        .unwrap_err();

    assert!(matches!(err, PolicyDemoError::LiveReconciliation(_)));
    assert!(err.to_string().contains("--execute-demo-orders"));
}

#[tokio::test]
async fn policy_demo_refuses_unbounded_live_user_data_wait() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT");
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.user_data_timeout = Duration::ZERO;
    let policy = SequencePolicy::hold_buy_sell();

    let err = run_policy_demo_with_policy(config, &policy, "test-sequence")
        .await
        .unwrap_err();

    assert!(matches!(err, PolicyDemoError::LiveReconciliation(_)));
    assert!(err.to_string().contains("greater than zero"));
}

#[tokio::test]
async fn policy_demo_spot_mock_placement_reports_receipts() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT");
    config.max_steps = 3;
    config.execute_demo_orders = true;
    config.client_order_id_prefix = "unit-spot".into();
    let policy = SequencePolicy::hold_buy_sell();

    let report =
        run_spot_policy_demo_with_placer(config, &policy, "test-sequence", |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: if order.side == SpotOrderSide::Buy {
                    11
                } else {
                    12
                },
                client_order_id,
                transact_time: 1,
                price: "0.00000000".into(),
                orig_qty: order.quantity.clone(),
                executed_qty: order.quantity,
                status: "FILLED".into(),
                side: order.side.as_str().into(),
                order_type: order.order_type.as_str().into(),
            })
        })
        .await
        .unwrap();

    assert_eq!(report.placed_orders, 2);
    assert_eq!(report.receipts.len(), 2);
    assert_eq!(report.receipts[0].venue, DemoVenue::Spot);
    assert_eq!(report.receipts[0].order_id, 11);
    assert_eq!(report.receipts[0].client_order_id, "unit-spot-spot-0000");
    assert_eq!(report.receipts[0].status, "FILLED");
    assert_eq!(report.receipts[1].client_order_id, "unit-spot-spot-0001");
}

#[tokio::test]
async fn policy_demo_spot_mock_frame_source_reconciles_after_placement() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT");
    config.max_steps = 3;
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.client_order_id_prefix = "unit-spot".into();
    let policy = SequencePolicy::hold_buy_sell();

    let report = run_spot_policy_demo_with_placer_and_user_data(
        config,
        &policy,
        "test-sequence",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: if order.side == SpotOrderSide::Buy {
                    31
                } else {
                    32
                },
                client_order_id,
                transact_time: 1,
                price: "0.00000000".into(),
                orig_qty: order.quantity.clone(),
                executed_qty: order.quantity,
                status: "NEW".into(),
                side: order.side.as_str().into(),
                order_type: order.order_type.as_str().into(),
            })
        },
        |report| {
            let messages = policy_demo_user_data_messages_from_json(&format!(
                "{}\n{}",
                spot_execution_report_json(
                    "BTCUSDT",
                    &report.receipts[0].client_order_id,
                    report.receipts[0].order_id,
                    "BUY",
                    "FILLED"
                ),
                spot_execution_report_json(
                    "BTCUSDT",
                    &report.receipts[1].client_order_id,
                    report.receipts[1].order_id,
                    "SELL",
                    "FILLED"
                ),
            ));
            async move { messages }
        },
    )
    .await
    .unwrap();

    assert_eq!(report.placed_orders, 2);
    assert_eq!(report.reconciliations.len(), 2);
    assert_eq!(report.reconciliations[0].venue, DemoVenue::Spot);
    assert_eq!(
        report.reconciliations[0].client_order_id,
        "unit-spot-spot-0000"
    );
    assert!(report.reconciliations[0].terminal);
    assert_eq!(report.reconciliations[1].side, "SELL");
}

#[tokio::test]
async fn policy_demo_usdm_mock_placement_reports_receipts() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Usdm, "ETHUSDT");
    config.max_steps = 3;
    config.execute_demo_orders = true;
    config.client_order_id_prefix = "unit-usdm".into();
    let policy = SequencePolicy::hold_buy_sell();

    let report =
        run_usdm_policy_demo_with_placer(config, &policy, "test-sequence", |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(UsdmPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: if order.side == UsdmOrderSide::Buy {
                    21
                } else {
                    22
                },
                client_order_id,
                update_time: 1,
                price: "0.00000000".into(),
                orig_qty: order.quantity.clone(),
                executed_qty: order.quantity,
                status: "FILLED".into(),
                side: order.side.as_str().into(),
                order_type: order.order_type.as_str().into(),
                position_side: order
                    .position_side
                    .map(|side| side.as_str())
                    .unwrap_or("BOTH")
                    .into(),
            })
        })
        .await
        .unwrap();

    assert_eq!(report.placed_orders, 2);
    assert_eq!(report.receipts.len(), 2);
    assert_eq!(report.receipts[0].venue, DemoVenue::Usdm);
    assert_eq!(report.receipts[0].order_id, 21);
    assert_eq!(report.receipts[0].client_order_id, "unit-usdm-usdm-0000");
    assert_eq!(report.receipts[0].status, "FILLED");
    assert_eq!(report.receipts[1].client_order_id, "unit-usdm-usdm-0001");
}

#[tokio::test]
async fn policy_demo_usdm_mock_frame_source_reconciles_after_placement() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Usdm, "ETHUSDT");
    config.max_steps = 3;
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.client_order_id_prefix = "unit-usdm".into();
    let policy = SequencePolicy::hold_buy_sell();

    let report = run_usdm_policy_demo_with_placer_and_user_data(
        config,
        &policy,
        "test-sequence",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(UsdmPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: if order.side == UsdmOrderSide::Buy {
                    41
                } else {
                    42
                },
                client_order_id,
                update_time: 1,
                price: "0.00000000".into(),
                orig_qty: order.quantity.clone(),
                executed_qty: order.quantity,
                status: "NEW".into(),
                side: order.side.as_str().into(),
                order_type: order.order_type.as_str().into(),
                position_side: order
                    .position_side
                    .map(|side| side.as_str())
                    .unwrap_or("BOTH")
                    .into(),
            })
        },
        |report| {
            let messages = policy_demo_user_data_messages_from_json(
                &serde_json::json!([
                    usdm_order_trade_update_json(
                        "ETHUSDT",
                        &report.receipts[0].client_order_id,
                        report.receipts[0].order_id,
                        "BUY",
                        "FILLED"
                    ),
                    usdm_order_trade_update_json(
                        "ETHUSDT",
                        &report.receipts[1].client_order_id,
                        report.receipts[1].order_id,
                        "SELL",
                        "FILLED"
                    )
                ])
                .to_string(),
            );
            async move { messages }
        },
    )
    .await
    .unwrap();

    assert_eq!(report.placed_orders, 2);
    assert_eq!(report.reconciliations.len(), 2);
    assert_eq!(report.reconciliations[0].venue, DemoVenue::Usdm);
    assert_eq!(
        report.reconciliations[0].client_order_id,
        "unit-usdm-usdm-0000"
    );
    assert!(report.reconciliations[0].terminal);
    assert_eq!(report.reconciliations[1].side, "SELL");
}

#[tokio::test]
async fn policy_demo_spot_reconciles_mock_user_data_receipts() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT");
    config.max_steps = 3;
    config.execute_demo_orders = true;
    config.client_order_id_prefix = "unit-spot".into();
    let policy = SequencePolicy::hold_buy_sell();

    let mut report =
        run_spot_policy_demo_with_placer(config, &policy, "test-sequence", |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: if order.side == SpotOrderSide::Buy {
                    11
                } else {
                    12
                },
                client_order_id,
                transact_time: 1,
                price: "0.00000000".into(),
                orig_qty: order.quantity.clone(),
                executed_qty: order.quantity,
                status: "FILLED".into(),
                side: order.side.as_str().into(),
                order_type: order.order_type.as_str().into(),
            })
        })
        .await
        .unwrap();

    let messages = policy_demo_user_data_messages_from_json(&format!(
        "{}\n{}",
        spot_execution_report_json("BTCUSDT", "unit-spot-spot-0000", 11, "BUY", "FILLED"),
        spot_execution_report_json("BTCUSDT", "unit-spot-spot-0001", 12, "SELL", "FILLED"),
    ))
    .unwrap();
    reconcile_policy_demo_report(&mut report, messages);

    assert_eq!(report.reconciliations.len(), 2);
    assert_eq!(report.reconciliations[0].venue, DemoVenue::Spot);
    assert_eq!(
        report.reconciliations[0].client_order_id,
        "unit-spot-spot-0000"
    );
    assert_eq!(report.reconciliations[0].status, "FILLED");
    assert!(report.reconciliations[0].terminal);
    assert_eq!(report.reconciliations[1].side, "SELL");
}

#[tokio::test]
async fn policy_demo_usdm_reconciles_mock_user_data_receipts() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Usdm, "ETHUSDT");
    config.max_steps = 3;
    config.execute_demo_orders = true;
    config.client_order_id_prefix = "unit-usdm".into();
    let policy = SequencePolicy::hold_buy_sell();

    let mut report =
        run_usdm_policy_demo_with_placer(config, &policy, "test-sequence", |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(UsdmPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: if order.side == UsdmOrderSide::Buy {
                    21
                } else {
                    22
                },
                client_order_id,
                update_time: 1,
                price: "0.00000000".into(),
                orig_qty: order.quantity.clone(),
                executed_qty: order.quantity,
                status: "FILLED".into(),
                side: order.side.as_str().into(),
                order_type: order.order_type.as_str().into(),
                position_side: order
                    .position_side
                    .map(|side| side.as_str())
                    .unwrap_or("BOTH")
                    .into(),
            })
        })
        .await
        .unwrap();

    let messages = policy_demo_user_data_messages_from_json(
        &serde_json::json!([
            usdm_order_trade_update_json("ETHUSDT", "unit-usdm-usdm-0000", 21, "BUY", "FILLED"),
            usdm_order_trade_update_json("ETHUSDT", "unit-usdm-usdm-0001", 22, "SELL", "FILLED")
        ])
        .to_string(),
    )
    .unwrap();
    reconcile_policy_demo_report(&mut report, messages);

    assert_eq!(report.reconciliations.len(), 2);
    assert_eq!(report.reconciliations[0].venue, DemoVenue::Usdm);
    assert_eq!(
        report.reconciliations[0].client_order_id,
        "unit-usdm-usdm-0000"
    );
    assert_eq!(report.reconciliations[0].status, "FILLED");
    assert!(report.reconciliations[0].terminal);
    assert_eq!(report.reconciliations[1].side, "SELL");
}

fn spot_execution_report_json(
    symbol: &str,
    client_order_id: &str,
    order_id: i64,
    side: &str,
    status: &str,
) -> String {
    serde_json::json!({
        "e": "executionReport",
        "E": 1499405658658_u64,
        "s": symbol,
        "c": client_order_id,
        "S": side,
        "o": "MARKET",
        "f": "GTC",
        "q": "0.002",
        "p": "0.00000000",
        "x": "TRADE",
        "X": status,
        "i": order_id,
        "l": "0.002",
        "z": "0.002",
        "L": "100.00",
        "T": 1499405658657_u64,
        "t": 123_i64,
        "w": false
    })
    .to_string()
}

fn usdm_order_trade_update_json(
    symbol: &str,
    client_order_id: &str,
    order_id: i64,
    side: &str,
    status: &str,
) -> serde_json::Value {
    serde_json::json!({
        "e": "ORDER_TRADE_UPDATE",
        "E": 1568879465652_u64,
        "T": 1568879465651_u64,
        "o": {
            "s": symbol,
            "c": client_order_id,
            "S": side,
            "o": "MARKET",
            "f": "GTC",
            "q": "0.003",
            "p": "0",
            "ap": "100.00",
            "x": "TRADE",
            "X": status,
            "i": order_id,
            "l": "0.003",
            "z": "0.003",
            "L": "100.00",
            "t": 456_i64,
            "ps": "BOTH",
            "rp": "0"
        }
    })
}
