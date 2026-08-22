use std::cell::Cell;

use binance_spot_exec::{
    OrderSide as SpotOrderSide, OrderType as SpotOrderType,
    PlaceOrderResponse as SpotPlaceOrderResponse,
};
use binance_usdm_exec::{
    OrderSide as UsdmOrderSide, OrderType as UsdmOrderType,
    PlaceOrderResponse as UsdmPlaceOrderResponse,
};
use trolly::policy_demo::{
    run_policy_demo_with_policy, run_spot_policy_demo_with_placer,
    run_usdm_policy_demo_with_placer, DemoVenue, PolicyDemoConfig, PolicyDemoError,
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
