//! Offline bridge tests from gym policy output into execution order egresses.

use std::cell::Cell;

use binance_spot_exec::{OrderSide as SpotOrderSide, OrderType as SpotOrderType, SpotOrderEgress};
use binance_usdm_exec::{OrderSide as UsdmOrderSide, OrderType as UsdmOrderType, UsdmOrderEgress};
use trolly_gym::{run_offline_policy_harness, Action, Env, EnvConfig, PolicyProvider};
use trolly_strategy::{envelope_message, DepthUpdate, OrderOnlyEgress, PriceLevel, StreamEvent};
use trolly_stream::Message;

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

fn depth_messages(symbol: &str) -> Vec<Message> {
    [
        ("100.00", "101.00", 1_u64),
        ("100.25", "101.25", 2_u64),
        ("100.50", "101.50", 3_u64),
    ]
    .into_iter()
    .map(|(bid, ask, update_id)| {
        envelope_message(&StreamEvent::Depth(DepthUpdate {
            symbol: symbol.into(),
            bids: vec![PriceLevel {
                price: bid.into(),
                qty: "1.0".into(),
            }],
            asks: vec![PriceLevel {
                price: ask.into(),
                qty: "1.0".into(),
            }],
            update_id: Some(update_id),
        }))
    })
    .collect()
}

#[test]
fn order_only_bridge_routes_policy_orders_to_spot_egress() {
    let symbol = "BTCUSDT";
    let mut config = EnvConfig::new(symbol);
    config.window_frames = 1;

    let (spot_egress, mut rx) = SpotOrderEgress::channel();
    let mut env = Env::new(config, OrderOnlyEgress::new(spot_egress));
    let policy = SequencePolicy::hold_buy_sell();

    let steps = run_offline_policy_harness(&mut env, &policy, depth_messages(symbol)).unwrap();

    assert_eq!(steps.len(), 3);
    let first = rx.try_recv().expect("buy order enqueued");
    let second = rx.try_recv().expect("sell order enqueued");
    assert!(rx.try_recv().is_err(), "hold/subscribe must not enqueue");

    assert_eq!(first.symbol, symbol);
    assert_eq!(first.side, SpotOrderSide::Buy);
    assert_eq!(first.order_type, SpotOrderType::Market);
    assert_eq!(first.quantity, "0.01");
    assert_eq!(first.price, None);

    assert_eq!(second.symbol, symbol);
    assert_eq!(second.side, SpotOrderSide::Sell);
    assert_eq!(second.order_type, SpotOrderType::Market);
    assert_eq!(second.quantity, "0.01");
    assert_eq!(second.price, None);
}

#[test]
fn order_only_bridge_routes_policy_orders_to_usdm_egress() {
    let symbol = "BTCUSDT";
    let mut config = EnvConfig::new(symbol);
    config.window_frames = 1;

    let (usdm_egress, mut rx) = UsdmOrderEgress::channel();
    let mut env = Env::new(config, OrderOnlyEgress::new(usdm_egress));
    let policy = SequencePolicy::hold_buy_sell();

    let steps = run_offline_policy_harness(&mut env, &policy, depth_messages(symbol)).unwrap();

    assert_eq!(steps.len(), 3);
    let first = rx.try_recv().expect("buy order enqueued");
    let second = rx.try_recv().expect("sell order enqueued");
    assert!(rx.try_recv().is_err(), "hold/subscribe must not enqueue");

    assert_eq!(first.symbol, symbol);
    assert_eq!(first.side, UsdmOrderSide::Buy);
    assert_eq!(first.order_type, UsdmOrderType::Market);
    assert_eq!(first.quantity, "0.01");
    assert_eq!(first.price, None);
    assert_eq!(first.position_side, None);

    assert_eq!(second.symbol, symbol);
    assert_eq!(second.side, UsdmOrderSide::Sell);
    assert_eq!(second.order_type, UsdmOrderType::Market);
    assert_eq!(second.quantity, "0.01");
    assert_eq!(second.price, None);
    assert_eq!(second.position_side, None);
}
