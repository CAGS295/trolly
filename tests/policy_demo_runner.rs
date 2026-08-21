use std::cell::Cell;

use binance_spot_exec::{OrderSide as SpotOrderSide, OrderType as SpotOrderType};
use binance_usdm_exec::{OrderSide as UsdmOrderSide, OrderType as UsdmOrderType};
use trolly::policy_demo::{
    run_policy_demo_with_policy, DemoVenue, PolicyDemoConfig, PolicyDemoError, PolicyDemoOrders,
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
