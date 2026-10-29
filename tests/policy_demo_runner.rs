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
    apply_extra_symbol_fills_to_env, collect_subscribed_public_depth, parse_policy_demo_symbols,
    policy_demo_depth_messages_from_json, policy_demo_messages_from_public_depth_book,
    policy_demo_public_depth_messages_from_texts, policy_demo_public_depth_snapshot_jsons,
    policy_demo_user_data_messages_from_json, public_depth_snapshot_url, public_depth_stream_url,
    public_depth_subscribe_request_json, public_depth_subscribe_request_json_for_symbols,
    reconcile_policy_demo_report, run_policy_demo, run_policy_demo_with_policy,
    run_policy_demo_with_public_depth, run_policy_demo_with_subscribed_public_depth_texts,
    run_spot_policy_demo_with_live_user_data_source, run_spot_policy_demo_with_placer,
    run_spot_policy_demo_with_placer_and_user_data,
    run_usdm_policy_demo_with_live_user_data_source, run_usdm_policy_demo_with_placer,
    run_usdm_policy_demo_with_placer_and_user_data, DemoVenue, PolicyDemoConfig, PolicyDemoError,
    PolicyDemoOrders, PolicyDemoSymbolInventory,
};
use trolly_gym::{
    write_recorded_mean_mu_onnx, Action, Env, EnvConfig, PolicyProvider,
    DEFAULT_GAUSSIAN_MU_ONNX_OBS_DIM, GAUSSIAN_MU_ONNX_INPUT, GAUSSIAN_MU_ONNX_OUTPUT,
};
use trolly_strategy::{DepthUpdate, PriceLevel, RecordingEgress, StreamEvent};

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

    fn with_actions(actions: Vec<Action>) -> Self {
        Self {
            actions,
            next: Cell::new(0),
        }
    }
}

#[derive(Default)]
struct CaptureObsLenPolicy {
    len: Cell<usize>,
}

impl PolicyProvider for CaptureObsLenPolicy {
    fn act(&self, obs: &[f32]) -> Action {
        self.len.set(obs.len());
        Action::Hold
    }
}

#[derive(Default)]
struct CaptureObsPolicy {
    last: std::cell::RefCell<Vec<f32>>,
}

impl PolicyProvider for CaptureObsPolicy {
    fn act(&self, obs: &[f32]) -> Action {
        self.last.replace(obs.to_vec());
        Action::Hold
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
async fn policy_demo_default_hold_emits_no_orders() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT");
    config.max_steps = 3;

    let report = run_policy_demo(config).await.unwrap();

    assert_eq!(report.policy_source, "hold");
    assert_eq!(report.depth_source, "synthetic");
    assert_eq!(report.placed_orders, 0);
    assert!(report.orders.is_empty());
}

#[tokio::test]
async fn policy_demo_unset_depth_json_keeps_synthetic_fixture() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT");
    config.max_steps = 1;
    config.window_frames = 1;
    let policy = CaptureObsPolicy::default();

    let report = run_policy_demo_with_policy(config, &policy, "synthetic-obs")
        .await
        .unwrap();

    assert_eq!(report.steps, 1);
    assert_eq!(report.depth_source, "synthetic");
    let obs = policy.last.borrow();
    assert_eq!(obs.len(), 7);
    assert!((obs[0] - 100.0).abs() < 1e-4, "synthetic bid {}", obs[0]);
    assert!((obs[2] - 101.0).abs() < 1e-4, "synthetic ask {}", obs[2]);
}

#[tokio::test]
async fn policy_demo_captured_stream_event_dispatches_orders() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT");
    config.max_steps = 3;
    config.qty = "0.002".into();
    config.gaussian_mean_actions = Some("0.8,-0.8,0.05".into());
    config.captured_depth_json = Some(
        r#"[
          {"kind":"depth","symbol":"BTCUSDT","bids":[{"price":"64321.50","qty":"1.0"}],"asks":[{"price":"64322.00","qty":"1.0"}],"update_id":1},
          {"kind":"depth","symbol":"BTCUSDT","bids":[{"price":"64321.25","qty":"1.0"}],"asks":[{"price":"64321.75","qty":"1.0"}],"update_id":2},
          {"kind":"depth","symbol":"BTCUSDT","bids":[{"price":"64320.00","qty":"1.0"}],"asks":[{"price":"64320.50","qty":"1.0"}],"update_id":3}
        ]"#
        .into(),
    );

    let report = run_policy_demo(config).await.unwrap();

    assert!(report.policy_source.starts_with("gaussian-mean-actions:"));
    assert_eq!(report.depth_source, "captured-json");
    assert_eq!(report.steps, 3);
    assert_eq!(report.placed_orders, 0);

    let PolicyDemoOrders::Spot(orders) = report.orders else {
        panic!("expected spot orders");
    };
    assert_eq!(orders.len(), 2);
    assert_eq!(orders[0].side, SpotOrderSide::Buy);
    assert_eq!(orders[0].order_type, SpotOrderType::Market);
    assert_eq!(orders[0].quantity, "0.002");
    assert_eq!(orders[1].side, SpotOrderSide::Sell);
}

#[tokio::test]
async fn policy_demo_captured_binance_depth_feeds_observation() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT");
    config.max_steps = 1;
    config.window_frames = 1;
    config.captured_depth_json =
        Some(include_str!("fixtures/binance_usd_m_depth_envelope.json").into());
    let policy = CaptureObsPolicy::default();

    let report = run_policy_demo_with_policy(config, &policy, "captured-binance")
        .await
        .unwrap();

    assert_eq!(report.steps, 1);
    assert_eq!(report.depth_source, "captured-json");
    let obs = policy.last.borrow();
    assert_eq!(obs.len(), 7);
    assert!((obs[0] - 64321.50).abs() < 1e-3, "captured bid {}", obs[0]);
    assert!((obs[2] - 64322.00).abs() < 1e-3, "captured ask {}", obs[2]);
    assert!((obs[4] - 0.50).abs() < 1e-3, "captured spread {}", obs[4]);
}

#[tokio::test]
async fn policy_demo_captured_depth_ndjson_runs_steps() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Usdm, "ETHUSDT");
    config.max_steps = 2;
    config.window_frames = 1;
    config.captured_depth_json = Some(
        r#"{"e":"depthUpdate","s":"ETHUSDT","b":[["3500.10","2.0"]],"a":[["3500.40","1.5"]],"u":11}
{"lastUpdateId":12,"bids":[["3500.00","1.0"]],"asks":[["3500.25","3.0"]]}
"#
        .into(),
    );
    let policy = CaptureObsPolicy::default();

    let report = run_policy_demo_with_policy(config, &policy, "captured-ndjson")
        .await
        .unwrap();

    assert_eq!(report.steps, 2);
    let obs = policy.last.borrow();
    assert_eq!(obs.len(), 7);
    assert!((obs[0] - 3500.00).abs() < 1e-3, "rest bid {}", obs[0]);
    assert!((obs[2] - 3500.25).abs() < 1e-3, "rest ask {}", obs[2]);
}

#[tokio::test]
async fn policy_demo_injected_public_depth_dispatches_orders() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT");
    config.max_steps = 2;
    config.qty = "0.004".into();
    config.gaussian_mean_actions = Some("0.8,-0.8".into());
    let policy =
        trolly_gym::CheckpointOrHoldPolicy::from_mean_actions_csv("0.8,-0.8", 0.25).unwrap();
    let json = include_str!("fixtures/binance_usd_m_depth_envelope.json");

    let report = run_policy_demo_with_public_depth(config, &policy, "injected-binance", || async {
        let mut frames = policy_demo_depth_messages_from_json(json, "BTCUSDT")?;
        frames.extend(policy_demo_depth_messages_from_json(
            r#"{"e":"depthUpdate","s":"BTCUSDT","b":[["64320.00","1.0"]],"a":[["64320.50","1.0"]],"u":99}"#,
            "BTCUSDT",
        )?);
        Ok(frames)
    })
    .await
    .unwrap();

    assert_eq!(report.depth_source, "injected");
    assert_eq!(report.steps, 2);
    let PolicyDemoOrders::Spot(orders) = report.orders else {
        panic!("expected spot orders");
    };
    assert_eq!(orders.len(), 2);
    assert_eq!(orders[0].side, SpotOrderSide::Buy);
    assert_eq!(orders[1].side, SpotOrderSide::Sell);
    assert_eq!(orders[0].quantity, "0.004");
}

#[tokio::test]
async fn policy_demo_injected_two_symbols_joins_observations() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.max_steps = 4;
    config.window_frames = 1;
    config.qty = "0.01".into();
    let policy = CaptureObsPolicy::default();

    let report = run_policy_demo_with_public_depth(config, &policy, "injected-multi", || async {
        policy_demo_depth_messages_from_json(
            r#"[
              {"kind":"depth","symbol":"BTCUSDT","bids":[{"price":"100.00","qty":"1.0"}],"asks":[{"price":"102.00","qty":"1.0"}],"update_id":1},
              {"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"200.00","qty":"1.0"}],"asks":[{"price":"202.00","qty":"1.0"}],"update_id":2}
            ]"#,
            "BTCUSDT",
        )
    })
    .await
    .unwrap();

    assert_eq!(report.depth_source, "injected");
    assert_eq!(report.symbol, "BTCUSDT");
    assert_eq!(report.observation_symbols, vec!["BTCUSDT", "ETHUSDT"]);
    assert_eq!(report.steps, 2);
    let obs = policy.last.borrow();
    assert_eq!(obs.len(), 14);
    assert!((obs[0] - 100.00).abs() < 1e-3, "btc bid {}", obs[0]);
    assert!((obs[7] - 200.00).abs() < 1e-3, "eth bid {}", obs[7]);
}

#[tokio::test]
async fn policy_demo_injected_two_symbols_dispatch_primary() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.max_steps = 4;
    config.qty = "0.02".into();
    let policy = SequencePolicy {
        actions: vec![Action::Hold, Action::Buy],
        next: Cell::new(0),
    };

    let report = run_policy_demo_with_public_depth(config, &policy, "injected-multi-buy", || async {
        policy_demo_depth_messages_from_json(
            r#"[
              {"kind":"depth","symbol":"BTCUSDT","bids":[{"price":"100.00","qty":"1.0"}],"asks":[{"price":"102.00","qty":"1.0"}],"update_id":1},
              {"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"200.00","qty":"1.0"}],"asks":[{"price":"202.00","qty":"1.0"}],"update_id":2}
            ]"#,
            "BTCUSDT",
        )
    })
    .await
    .unwrap();

    let PolicyDemoOrders::Spot(orders) = report.orders else {
        panic!("expected spot orders");
    };
    assert_eq!(orders.len(), 1);
    assert_eq!(orders[0].symbol, "BTCUSDT");
    assert_eq!(orders[0].side, SpotOrderSide::Buy);
    assert_eq!(orders[0].quantity, "0.02");
}

struct ExtraBookBuyPolicy;

impl PolicyProvider for ExtraBookBuyPolicy {
    fn act(&self, obs: &[f32]) -> Action {
        if obs.len() >= 14 && obs[7] > 0.0 {
            Action::Buy
        } else {
            Action::Hold
        }
    }

    fn decide(&self, obs: &[f32]) -> trolly_gym::ActionDecision {
        let action = self.act(obs);
        if action == Action::Buy {
            action.on_symbol("ETHUSDT")
        } else {
            action.into()
        }
    }
}

#[tokio::test]
async fn policy_demo_joined_policy_can_dispatch_extra_symbol() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.max_steps = 4;
    config.qty = "0.03".into();
    let policy = ExtraBookBuyPolicy;

    let report = run_policy_demo_with_public_depth(config, &policy, "joined-extra-buy", || async {
        policy_demo_depth_messages_from_json(
            r#"[
              {"kind":"depth","symbol":"BTCUSDT","bids":[{"price":"100.00","qty":"1.0"}],"asks":[{"price":"102.00","qty":"1.0"}],"update_id":1},
              {"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"200.00","qty":"1.0"}],"asks":[{"price":"202.00","qty":"1.0"}],"update_id":2}
            ]"#,
            "BTCUSDT",
        )
    })
    .await
    .unwrap();

    let PolicyDemoOrders::Spot(orders) = report.orders else {
        panic!("expected spot orders");
    };
    assert_eq!(report.symbol, "BTCUSDT");
    assert_eq!(report.dispatch_symbol, "BTCUSDT");
    assert_eq!(orders.len(), 1);
    assert_eq!(orders[0].symbol, "ETHUSDT");
    assert_eq!(orders[0].side, SpotOrderSide::Buy);
    assert_eq!(orders[0].quantity, "0.03");
}

#[tokio::test]
async fn policy_demo_dispatch_symbol_pin_keeps_qty_and_side() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 4;
    config.qty = "0.04".into();
    let policy = SequencePolicy {
        actions: vec![Action::Hold, Action::Sell],
        next: Cell::new(0),
    };

    let report = run_policy_demo_with_public_depth(config, &policy, "pin-eth-sell", || async {
        policy_demo_depth_messages_from_json(
            r#"[
              {"kind":"depth","symbol":"BTCUSDT","bids":[{"price":"100.00","qty":"1.0"}],"asks":[{"price":"102.00","qty":"1.0"}],"update_id":1},
              {"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"200.00","qty":"1.0"}],"asks":[{"price":"202.00","qty":"1.0"}],"update_id":2}
            ]"#,
            "BTCUSDT",
        )
    })
    .await
    .unwrap();

    let PolicyDemoOrders::Spot(orders) = report.orders else {
        panic!("expected spot orders");
    };
    assert_eq!(report.dispatch_symbol, "ETHUSDT");
    assert_eq!(orders.len(), 1);
    assert_eq!(orders[0].symbol, "ETHUSDT");
    assert_eq!(orders[0].side, SpotOrderSide::Sell);
    assert_eq!(orders[0].quantity, "0.04");
}

#[tokio::test]
async fn policy_demo_gaussian_multi_symbol_keeps_primary_ladder() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.max_steps = 4;
    config.window_frames = 1;
    config.qty = "0.01".into();
    config.gaussian_mean_actions = Some("0.8,-0.8".into());
    let policy =
        trolly_gym::CheckpointOrHoldPolicy::from_mean_actions_csv("0.8,-0.8", 0.25).unwrap();
    let obs = CaptureObsPolicy::default();

    // Drive Env through the same Gaussian config path as execute policy-demo.
    let report = run_policy_demo_with_public_depth(
        config.clone(),
        &policy,
        "gaussian-multi",
        || async {
            policy_demo_depth_messages_from_json(
                r#"[
                  {"kind":"depth","symbol":"BTCUSDT","bids":[{"price":"100.00","qty":"1.0"}],"asks":[{"price":"104.00","qty":"1.0"}],"update_id":1},
                  {"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"200.00","qty":"1.0"}],"asks":[{"price":"202.00","qty":"1.0"}],"update_id":2}
                ]"#,
                "BTCUSDT",
            )
        },
    )
    .await
    .unwrap();

    let PolicyDemoOrders::Spot(orders) = report.orders else {
        panic!("expected spot orders");
    };
    assert_eq!(orders.len(), 2);
    assert_eq!(orders[0].symbol, "BTCUSDT");
    assert_eq!(orders[0].side, SpotOrderSide::Buy);
    assert_eq!(orders[1].side, SpotOrderSide::Sell);

    let report = run_policy_demo_with_public_depth(config, &obs, "gaussian-multi-obs", || async {
        policy_demo_depth_messages_from_json(
            r#"[
              {"kind":"depth","symbol":"BTCUSDT","bids":[{"price":"100.00","qty":"1.0"}],"asks":[{"price":"104.00","qty":"1.0"}],"update_id":1},
              {"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"200.00","qty":"1.0"}],"asks":[{"price":"202.00","qty":"1.0"}],"update_id":2}
            ]"#,
            "BTCUSDT",
        )
    })
    .await
    .unwrap();
    assert_eq!(report.symbol, "BTCUSDT");
    assert_eq!(report.observation_symbols, vec!["BTCUSDT", "ETHUSDT"]);
    let last = obs.last.borrow();
    assert_eq!(last.len(), 40, "primary V×5 (V=8), not 80-D join");
    assert!(
        (last[1] - 2.0).abs() < 1e-5,
        "btc half-spread δ {}",
        last[1]
    );
}

#[tokio::test]
async fn policy_demo_synthetic_default_stays_single_symbol_tape() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.max_steps = 2;
    config.window_frames = 1;
    let policy = CaptureObsPolicy::default();

    let report = run_policy_demo_with_policy(config, &policy, "synthetic-primary")
        .await
        .unwrap();

    assert_eq!(report.depth_source, "synthetic");
    assert_eq!(report.steps, 2);
    let obs = policy.last.borrow();
    assert_eq!(obs.len(), 14);
    assert!(obs[0] > 0.0, "btc synthetic bid");
    assert_eq!(obs[7], 0.0);
}

#[tokio::test]
async fn policy_demo_subscribed_two_symbols_join_stream_frames() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.max_steps = 4;
    config.window_frames = 1;
    config.subscribe_public_depth = true;
    config.public_depth_timeout = Duration::from_secs(2);
    let policy = CaptureObsPolicy::default();

    let report = run_policy_demo_with_subscribed_public_depth_texts(
        config,
        &policy,
        "subscribed-multi",
        || async {
            Ok(vec![
                r#"{"result":null,"id":1}"#.into(),
                r#"{"e":"depthUpdate","s":"BTCUSDT","b":[["100.00","1.0"]],"a":[["102.00","1.0"]],"u":7}"#.into(),
                r#"{"e":"depthUpdate","s":"ETHUSDT","b":[["200.00","1.0"]],"a":[["202.00","1.0"]],"u":8}"#.into(),
            ])
        },
    )
    .await
    .unwrap();

    assert_eq!(report.depth_source, "subscribed");
    assert_eq!(report.symbol, "BTCUSDT");
    assert_eq!(report.observation_symbols, vec!["BTCUSDT", "ETHUSDT"]);
    let obs = policy.last.borrow();
    assert_eq!(obs.len(), 14);
    assert!((obs[0] - 100.00).abs() < 1e-3);
    assert!((obs[7] - 200.00).abs() < 1e-3);
}

#[test]
fn policy_demo_snapshot_jsons_accept_object_or_array() {
    let one = policy_demo_public_depth_snapshot_jsons(
        r#"{"lastUpdateId":1,"symbol":"BTCUSDT","bids":[["100.00","1"]],"asks":[["101.00","1"]]}"#,
    )
    .unwrap();
    assert_eq!(one.len(), 1);
    let two = policy_demo_public_depth_snapshot_jsons(
        r#"[
          {"lastUpdateId":1,"s":"BTCUSDT","bids":[["100.00","1"]],"asks":[["101.00","1"]]},
          {"lastUpdateId":2,"s":"ETHUSDT","bids":[["200.00","1"]],"asks":[["201.00","1"]]}
        ]"#,
    )
    .unwrap();
    assert_eq!(two.len(), 2);
}

#[tokio::test]
async fn policy_demo_subscribed_two_snapshot_books() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.max_steps = 4;
    config.window_frames = 1;
    config.public_depth_timeout = Duration::from_secs(2);
    config.public_depth_snapshot_json = Some(
        r#"[
          {"lastUpdateId":10,"s":"BTCUSDT","bids":[["100.00","1.0"]],"asks":[["101.00","1.0"]]},
          {"lastUpdateId":20,"s":"ETHUSDT","bids":[["200.00","1.0"]],"asks":[["201.00","1.0"]]}
        ]"#
        .into(),
    );
    let policy = CaptureObsPolicy::default();

    let report = run_policy_demo_with_subscribed_public_depth_texts(
        config,
        &policy,
        "subscribed-multi-snap",
        || async {
            Ok(vec![
                r#"{"result":null,"id":1}"#.into(),
                r#"{"e":"depthUpdate","s":"BTCUSDT","b":[["102.00","1.5"]],"a":[["103.00","1.0"]],"u":11}"#.into(),
                r#"{"e":"depthUpdate","s":"ETHUSDT","b":[["204.00","1.0"]],"a":[["205.00","1.0"]],"u":21}"#.into(),
            ])
        },
    )
    .await
    .unwrap();

    assert_eq!(report.depth_source, "subscribed");
    assert_eq!(report.symbol, "BTCUSDT");
    let obs = policy.last.borrow();
    assert_eq!(obs.len(), 14);
    assert!(
        (obs[0] - 102.00).abs() < 1e-3,
        "btc bid after diff {}",
        obs[0]
    );
    assert!(
        (obs[7] - 204.00).abs() < 1e-3,
        "eth bid after diff {}",
        obs[7]
    );
}

#[tokio::test]
async fn policy_demo_depth_json_wins_over_injected_source() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT");
    config.max_steps = 1;
    config.window_frames = 1;
    config.captured_depth_json =
        Some(include_str!("fixtures/binance_usd_m_depth_envelope.json").into());
    let policy = CaptureObsPolicy::default();

    let report = run_policy_demo_with_public_depth(config, &policy, "json-wins", || async {
        policy_demo_depth_messages_from_json(
            r#"{"kind":"depth","symbol":"BTCUSDT","bids":[{"price":"10.00","qty":"1"}],"asks":[{"price":"11.00","qty":"1"}]}"#,
            "BTCUSDT",
        )
    })
    .await
    .unwrap();

    assert_eq!(report.depth_source, "captured-json");
    let obs = policy.last.borrow();
    assert!((obs[0] - 64321.50).abs() < 1e-3, "json bid {}", obs[0]);
}

#[tokio::test]
async fn policy_demo_injected_public_depth_empty_errors() {
    let config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT");
    let policy = CaptureObsPolicy::default();
    let err = run_policy_demo_with_public_depth(config, &policy, "empty-source", || async {
        Ok(Vec::new())
    })
    .await
    .unwrap_err();
    assert!(
        matches!(err, PolicyDemoError::DepthInput(_)),
        "unexpected error: {err}"
    );
}

#[test]
fn policy_demo_public_depth_urls_use_demo_hosts() {
    assert_eq!(
        public_depth_stream_url(DemoVenue::Spot),
        "wss://demo-stream.binance.com/ws"
    );
    assert_eq!(
        public_depth_stream_url(DemoVenue::Usdm),
        "wss://fstream.binancefuture.com/stream"
    );
    assert_eq!(
        public_depth_subscribe_request_json("BTCUSDT"),
        r#"{"method":"SUBSCRIBE","params":["btcusdt@depth"],"id":1}"#
    );
    assert_eq!(
        public_depth_subscribe_request_json_for_symbols(["BTCUSDT", "ETHUSDT"]),
        r#"{"method":"SUBSCRIBE","params":["btcusdt@depth","ethusdt@depth"],"id":1}"#
    );
    assert_eq!(
        parse_policy_demo_symbols("BTCUSDT, ETHUSDT;btcusdt"),
        vec!["BTCUSDT", "ETHUSDT"]
    );
    assert_eq!(
        public_depth_snapshot_url(DemoVenue::Spot, "btcusdt"),
        "https://demo-api.binance.com/api/v3/depth?symbol=BTCUSDT&limit=100"
    );
    assert_eq!(
        public_depth_snapshot_url(DemoVenue::Usdm, "ETHUSDT"),
        "https://demo-fapi.binance.com/fapi/v1/depth?symbol=ETHUSDT&limit=100"
    );
}

#[test]
fn policy_demo_public_depth_texts_skip_subscribe_ack() {
    let messages = policy_demo_public_depth_messages_from_texts(
        [
            r#"{"result":null,"id":1}"#,
            r#"{"e":"depthUpdate","s":"BTCUSDT","b":[["64321.50","1.0"]],"a":[["64322.00","1.0"]],"u":7}"#,
        ],
        "BTCUSDT",
    )
    .unwrap();
    assert_eq!(messages.len(), 1);
}

#[tokio::test]
async fn policy_demo_subscribed_public_depth_dispatches_orders() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT");
    config.max_steps = 2;
    config.qty = "0.005".into();
    config.public_depth_timeout = Duration::from_secs(2);
    let policy =
        trolly_gym::CheckpointOrHoldPolicy::from_mean_actions_csv("0.8,-0.8", 0.25).unwrap();

    let report = run_policy_demo_with_subscribed_public_depth_texts(
        config,
        &policy,
        "subscribed-binance",
        || async {
            Ok(vec![
                r#"{"result":null,"id":1}"#.into(),
                include_str!("fixtures/binance_usd_m_depth_envelope.json").into(),
                r#"{"e":"depthUpdate","s":"BTCUSDT","b":[["64320.00","1.0"]],"a":[["64320.50","1.0"]],"u":99}"#.into(),
            ])
        },
    )
    .await
    .unwrap();

    assert_eq!(report.depth_source, "subscribed");
    assert_eq!(report.steps, 2);
    let PolicyDemoOrders::Spot(orders) = report.orders else {
        panic!("expected spot orders");
    };
    assert_eq!(orders.len(), 2);
    assert_eq!(orders[0].side, SpotOrderSide::Buy);
    assert_eq!(orders[1].side, SpotOrderSide::Sell);
    assert_eq!(orders[0].quantity, "0.005");
}

#[tokio::test]
async fn policy_demo_depth_json_wins_over_subscribe() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT");
    config.max_steps = 1;
    config.window_frames = 1;
    config.subscribe_public_depth = true;
    config.public_depth_timeout = Duration::from_secs(2);
    config.captured_depth_json =
        Some(include_str!("fixtures/binance_usd_m_depth_envelope.json").into());
    let policy = CaptureObsPolicy::default();

    let report = run_policy_demo_with_policy(config, &policy, "json-wins-subscribe")
        .await
        .unwrap();

    assert_eq!(report.depth_source, "captured-json");
    let obs = policy.last.borrow();
    assert!((obs[0] - 64321.50).abs() < 1e-3, "json bid {}", obs[0]);
}

#[tokio::test]
async fn policy_demo_refuses_unbounded_public_depth_subscribe() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT");
    config.subscribe_public_depth = true;
    config.public_depth_timeout = Duration::ZERO;
    let policy = CaptureObsPolicy::default();

    let err = run_policy_demo_with_policy(config, &policy, "subscribe-unbounded")
        .await
        .unwrap_err();
    assert!(
        matches!(err, PolicyDemoError::DepthInput(_)),
        "unexpected error: {err}"
    );
    assert!(err.to_string().contains("greater than zero"));
}

#[tokio::test]
async fn policy_demo_public_depth_book_updates_best_bid() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT");
    config.max_steps = 2;
    config.window_frames = 1;
    config.public_depth_timeout = Duration::from_secs(2);
    config.public_depth_snapshot_json = Some(
        r#"{"lastUpdateId":10,"bids":[["100.00","1.0"],["99.00","2.0"]],"asks":[["101.00","1.0"]]}"#
            .into(),
    );
    let policy = CaptureObsPolicy::default();

    let report = run_policy_demo_with_subscribed_public_depth_texts(
        config,
        &policy,
        "subscribed-book",
        || async {
            Ok(vec![
                r#"{"result":null,"id":1}"#.into(),
                r#"{"e":"depthUpdate","s":"BTCUSDT","b":[["100.00","0"],["102.00","1.5"]],"a":[["101.00","1.0"]],"u":11}"#.into(),
            ])
        },
    )
    .await
    .unwrap();

    assert_eq!(report.depth_source, "subscribed");
    assert_eq!(report.steps, 2);
    let obs = policy.last.borrow();
    assert_eq!(obs.len(), 7);
    assert!(
        (obs[0] - 102.0).abs() < 1e-3,
        "rebuilt best bid after removing 100: {}",
        obs[0]
    );
    assert!((obs[2] - 101.0).abs() < 1e-3, "ask {}", obs[2]);
}

#[test]
fn policy_demo_public_depth_book_skips_stale_and_removes_qty_zero() {
    let messages = policy_demo_messages_from_public_depth_book(
        r#"{"lastUpdateId":10,"bids":[["100.00","1.0"],["99.00","2.0"]],"asks":[["101.00","1.0"]]}"#,
        [
            r#"{"e":"depthUpdate","s":"BTCUSDT","b":[["100.00","0.5"]],"a":[["101.00","1.0"]],"u":9}"#,
            r#"{"e":"depthUpdate","s":"BTCUSDT","b":[["100.00","0"]],"a":[["101.00","1.0"]],"u":12}"#,
        ],
        "BTCUSDT",
    )
    .unwrap();
    assert_eq!(messages.len(), 2, "snapshot + one applied diff");
}

#[tokio::test]
async fn policy_demo_subscribed_book_dispatches_orders() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT");
    config.max_steps = 2;
    config.qty = "0.006".into();
    config.public_depth_timeout = Duration::from_secs(2);
    config.public_depth_snapshot_json = Some(
        r#"{"lastUpdateId":1,"bids":[["64321.50","1.0"]],"asks":[["64322.00","1.0"]]}"#.into(),
    );
    let policy =
        trolly_gym::CheckpointOrHoldPolicy::from_mean_actions_csv("0.8,-0.8", 0.25).unwrap();

    let report = run_policy_demo_with_subscribed_public_depth_texts(
        config,
        &policy,
        "subscribed-book-orders",
        || async {
            Ok(vec![
                r#"{"e":"depthUpdate","s":"BTCUSDT","b":[["64320.00","1.0"]],"a":[["64320.50","1.0"]],"u":2}"#.into(),
            ])
        },
    )
    .await
    .unwrap();

    assert_eq!(report.depth_source, "subscribed");
    let PolicyDemoOrders::Spot(orders) = report.orders else {
        panic!("expected spot orders");
    };
    assert_eq!(orders.len(), 2);
    assert_eq!(orders[0].side, SpotOrderSide::Buy);
    assert_eq!(orders[1].side, SpotOrderSide::Sell);
    assert_eq!(orders[0].quantity, "0.006");
}

#[tokio::test]
async fn policy_demo_subscribed_public_depth_ack_only_errors() {
    let err = collect_subscribed_public_depth("BTCUSDT", 2, Duration::from_secs(1), || async {
        Ok(vec![r#"{"result":null,"id":1}"#.into()])
    })
    .await
    .unwrap_err();
    assert!(
        matches!(err, PolicyDemoError::DepthInput(_)),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn policy_demo_captured_depth_empty_errors() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT");
    config.captured_depth_json = Some("   \n".into());

    let err = run_policy_demo(config).await.unwrap_err();
    assert!(
        matches!(err, PolicyDemoError::DepthInput(_)),
        "unexpected error: {err}"
    );
}

#[test]
fn policy_demo_depth_json_parses_array_and_binance_envelope() {
    let messages = policy_demo_depth_messages_from_json(
        include_str!("fixtures/binance_usd_m_depth_envelope.json"),
        "BTCUSDT",
    )
    .unwrap();
    assert_eq!(messages.len(), 1);
}

#[tokio::test]
async fn policy_demo_recorded_mean_actions_quantize_to_spot_orders() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT");
    config.max_steps = 3;
    config.qty = "0.002".into();
    config.gaussian_mean_actions = Some("0.8,-0.8,0.05".into());

    let report = run_policy_demo(config).await.unwrap();

    assert!(report.policy_source.starts_with("gaussian-mean-actions:"));
    assert_eq!(report.steps, 3);
    assert_eq!(report.placed_orders, 0);

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
async fn policy_demo_recorded_mean_actions_quantize_to_usdm_orders() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Usdm, "ETHUSDT");
    config.max_steps = 3;
    config.qty = "0.003".into();
    config.gaussian_mean_actions = Some("0.8,-0.8,0.05".into());

    let report = run_policy_demo(config).await.unwrap();

    assert!(report.policy_source.starts_with("gaussian-mean-actions:"));
    let PolicyDemoOrders::Usdm(orders) = report.orders else {
        panic!("expected USDM orders");
    };
    assert_eq!(orders.len(), 2);
    assert_eq!(orders[0].side, UsdmOrderSide::Buy);
    assert_eq!(orders[0].order_type, UsdmOrderType::Market);
    assert_eq!(orders[0].quantity, "0.003");
    assert_eq!(orders[1].side, UsdmOrderSide::Sell);
}

#[tokio::test]
async fn policy_demo_gaussian_source_feeds_ladder_observation() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT");
    config.max_steps = 1;
    config.window_frames = 1;
    config.gaussian_mean_actions = Some("0.0".into());
    let policy = CaptureObsLenPolicy::default();

    let report = run_policy_demo_with_policy(config, &policy, "ladder-len")
        .await
        .unwrap();

    assert_eq!(report.steps, 1);
    assert_eq!(policy.len.get(), 40);
}

#[tokio::test]
async fn policy_demo_onnx_gaussian_path_feeds_ladder_observation() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT");
    config.max_steps = 1;
    config.window_frames = 1;
    config.onnx_gaussian_model_path = Some("/tmp/missing-gaussian-mu.onnx".into());
    let policy = CaptureObsLenPolicy::default();

    let report = run_policy_demo_with_policy(config, &policy, "onnx-gaussian-len")
        .await
        .unwrap();

    assert_eq!(report.steps, 1);
    assert_eq!(policy.len.get(), 40);
}

#[tokio::test]
async fn policy_demo_onnx_gaussian_without_ort_stays_hold() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT");
    config.max_steps = 1;
    config.onnx_gaussian_model_path = Some("/tmp/missing-gaussian-mu.onnx".into());

    let report = run_policy_demo(config).await.unwrap();

    assert!(
        report.policy_source.contains("ONNX_GAUSSIAN_MODEL_PATH")
            || report.policy_source.starts_with("onnx-gaussian:")
            || report.policy_source.contains("ONNX Gaussian load failed")
    );
    assert_eq!(report.placed_orders, 0);
    assert!(report.orders.is_empty());
}

#[tokio::test]
async fn policy_demo_exported_gaussian_mu_onnx_stays_offline() {
    let path = std::env::temp_dir().join(format!(
        "trolly_policy_demo_mu_{}.onnx",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let info = write_recorded_mean_mu_onnx(&path, DEFAULT_GAUSSIAN_MU_ONNX_OBS_DIM, 0.8).unwrap();
    assert_eq!(info.obs_dim, 40);
    assert_eq!(info.input_name, GAUSSIAN_MU_ONNX_INPUT);
    assert_eq!(info.output_name, GAUSSIAN_MU_ONNX_OUTPUT);
    assert!((info.mean_bias - 0.8).abs() < f32::EPSILON);

    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT");
    config.max_steps = 1;
    config.window_frames = 1;
    config.onnx_gaussian_model_path = Some(path.to_string_lossy().into_owned());

    let report = run_policy_demo(config).await.unwrap();
    let _ = std::fs::remove_file(&path);

    assert!(
        report.policy_source.contains("ONNX_GAUSSIAN_MODEL_PATH")
            || report.policy_source.starts_with("onnx-gaussian:")
            || report.policy_source.contains("ONNX Gaussian load failed")
    );
    assert_eq!(report.placed_orders, 0);
    assert!(report.orders.is_empty());
}

#[tokio::test]
async fn policy_demo_autoloads_mu_onnx_from_gaussian_checkpoint_dir() {
    let dir = std::env::temp_dir().join(format!(
        "trolly_policy_demo_gaussian_mlp_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("mu.onnx");
    write_recorded_mean_mu_onnx(&path, DEFAULT_GAUSSIAN_MU_ONNX_OBS_DIM, 0.8).unwrap();

    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT");
    config.max_steps = 1;
    config.window_frames = 1;
    config.gaussian_checkpoint_dir = Some(dir.to_string_lossy().into_owned());

    let report = run_policy_demo(config).await.unwrap();
    let _ = std::fs::remove_dir_all(&dir);

    assert!(
        report.policy_source.contains("mu.onnx")
            || report.policy_source.starts_with("onnx-gaussian:")
            || report.policy_source.contains("ONNX_GAUSSIAN_MODEL_PATH")
            || report.policy_source.contains("ONNX Gaussian load failed")
    );
    assert_eq!(report.placed_orders, 0);
    assert!(report.orders.is_empty());
}

#[tokio::test]
async fn policy_demo_recorded_mean_actions_win_over_dir_mu_onnx() {
    let dir = std::env::temp_dir().join(format!(
        "trolly_policy_demo_mean_wins_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    write_recorded_mean_mu_onnx(dir.join("mu.onnx"), DEFAULT_GAUSSIAN_MU_ONNX_OBS_DIM, 0.8)
        .unwrap();

    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT");
    config.max_steps = 1;
    config.gaussian_checkpoint_dir = Some(dir.to_string_lossy().into_owned());
    config.gaussian_mean_actions = Some("0.0".into());

    let report = run_policy_demo(config).await.unwrap();
    let _ = std::fs::remove_dir_all(&dir);

    assert!(report.policy_source.starts_with("gaussian-mean-actions:"));
    assert_eq!(report.placed_orders, 0);
}

#[tokio::test]
async fn policy_demo_refuses_retired_dir_even_with_mu_onnx() {
    let dir = std::env::temp_dir().join(format!(
        "trolly_policy_demo__retired_unit_lot_microstructure_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("mu.onnx"), b"not-an-onnx-stand-in").unwrap();

    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT");
    config.max_steps = 1;
    config.gaussian_checkpoint_dir = Some(dir.to_string_lossy().into_owned());

    let report = run_policy_demo(config).await.unwrap();
    let _ = std::fs::remove_dir_all(&dir);

    assert!(report
        .policy_source
        .contains("refusing retired unit-lot checkpoint"));
    assert_eq!(report.placed_orders, 0);
}

#[tokio::test]
async fn policy_demo_hold_path_keeps_stream_observation() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT");
    config.max_steps = 1;
    config.window_frames = 1;
    let policy = CaptureObsLenPolicy::default();

    let report = run_policy_demo_with_policy(config, &policy, "stream-len")
        .await
        .unwrap();

    assert_eq!(report.steps, 1);
    assert_eq!(policy.len.get(), 7);
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
async fn policy_demo_spot_reconciles_dispatched_extra_symbol() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.client_order_id_prefix = "unit-spot-eth".into();
    let policy = SequencePolicy::hold_buy_sell();

    let report = run_spot_policy_demo_with_placer_and_user_data(
        config,
        &policy,
        "joined-extra-buy",
        |order| async move {
            assert_eq!(order.symbol, "ETHUSDT");
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: if order.side == SpotOrderSide::Buy {
                    51
                } else {
                    52
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
                    "ETHUSDT",
                    &report.receipts[0].client_order_id,
                    report.receipts[0].order_id,
                    "BUY",
                    "FILLED"
                ),
                spot_execution_report_json(
                    "ETHUSDT",
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

    assert_eq!(report.symbol, "BTCUSDT");
    assert_eq!(report.dispatch_symbol, "ETHUSDT");
    assert_eq!(
        report.placed_orders,
        2,
        "steps={} order_requests={} depth={}",
        report.steps,
        report.order_count(),
        report.depth_source
    );
    assert_eq!(report.receipts[0].symbol, "ETHUSDT");
    assert_eq!(report.reconciliations.len(), 2);
    assert_eq!(report.reconciliations[0].symbol, "ETHUSDT");
    assert_eq!(report.reconciliations[0].side, "BUY");
    assert!(report.reconciliations[0].terminal);
    assert_eq!(report.reconciliations[1].side, "SELL");
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: -1,
        }]
    );

    let mut env = Env::new(
        {
            let mut config = EnvConfig::new("BTCUSDT");
            config.window_frames = 1;
            config.use_ladder_observation();
            config.ladder.rung_count = 2;
            config.observe_symbols(["ETHUSDT"]);
            config
        },
        RecordingEgress::default(),
    );
    env.ingest_event(&policy_demo_depth_event("BTCUSDT", "100", "104"));
    env.ingest_event(&policy_demo_depth_event("ETHUSDT", "200", "202"));
    apply_extra_symbol_fills_to_env(&mut env, &report);
    assert_eq!(env.position(), 0);
    assert_eq!(env.position_for("ETHUSDT"), -1);
    assert_eq!(env.position_for("BTCUSDT"), 0);

    let eth_q = Cell::new(-99.0f32);
    let next = |obs: &[f32]| {
        eth_q.set(obs[14]);
        Action::Hold
    };
    env.step(&next).unwrap();
    assert_eq!(eth_q.get(), -1.0);
}

#[tokio::test]
async fn policy_demo_wait_user_data_continues_with_fill_backed_q() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.use_ladder_observation = true;
    config.client_order_id_prefix = "unit-spot-wait".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    let policy = SequencePolicy::hold_buy_sell();

    let report = run_spot_policy_demo_with_placer_and_user_data(
        config,
        &policy,
        "wait-continue-fill",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: if order.side == SpotOrderSide::Buy {
                    61
                } else {
                    62
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
                    "ETHUSDT",
                    &report.receipts[0].client_order_id,
                    report.receipts[0].order_id,
                    "BUY",
                    "FILLED"
                ),
                spot_execution_report_json(
                    "ETHUSDT",
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

    assert_eq!(report.reconciliations.len(), 2);
    assert_eq!(report.continued_steps, 1);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: -1,
        }]
    );
    assert_eq!(report.continued_observation.len(), 80);
    assert!(
        (report.continued_observation[44] + 1.0).abs() < 1e-5,
        "wait-path fill-backed eth q {}",
        report.continued_observation[44]
    );
}

#[tokio::test]
async fn policy_demo_wait_user_data_reconciles_continued_tape() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.client_order_id_prefix = "unit-spot-wait2".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    let policy =
        SequencePolicy::with_actions(vec![Action::Hold, Action::Buy, Action::Sell, Action::Buy]);
    let waits = Cell::new(0usize);

    let report = run_spot_policy_demo_with_placer_and_user_data(
        config,
        &policy,
        "wait-continue-place",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-spot-wait2-spot-0000" => 71,
                    "unit-spot-wait2-spot-0001" => 72,
                    "unit-spot-wait2-spot-0002" => 73,
                    other => panic!("unexpected client order id {other}"),
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
            waits.set(waits.get() + 1);
            let payload = report
                .receipts
                .iter()
                .map(|receipt| {
                    spot_execution_report_json(
                        &receipt.symbol,
                        &receipt.client_order_id,
                        receipt.order_id,
                        &receipt.side,
                        "FILLED",
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            let messages = policy_demo_user_data_messages_from_json(&payload);
            async move { messages }
        },
    )
    .await
    .unwrap();

    assert_eq!(waits.get(), 2);
    assert_eq!(report.placed_orders, 3);
    assert_eq!(report.receipts.len(), 3);
    assert_eq!(report.reconciliations.len(), 3);
    assert_eq!(
        report.reconciliations[0].client_order_id,
        "unit-spot-wait2-spot-0000"
    );
    assert_eq!(
        report.reconciliations[2].client_order_id,
        "unit-spot-wait2-spot-0002"
    );
    assert!(report.reconciliations[2].terminal);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: 1,
        }]
    );
    assert_eq!(report.second_continued_steps, 0);
    assert!(report.second_continued_observation.is_empty());
    assert_eq!(report.third_continued_steps, 0);
    assert!(report.third_continued_observation.is_empty());
    assert_eq!(report.fourth_continued_steps, 0);
    assert!(report.fourth_continued_observation.is_empty());
    assert_eq!(report.fifth_continued_steps, 0);
    assert!(report.fifth_continued_observation.is_empty());
    assert_eq!(report.sixth_continued_steps, 0);
    assert!(report.sixth_continued_observation.is_empty());
    assert_eq!(report.seventh_continued_steps, 0);
    assert_eq!(report.eighth_continued_steps, 0);
    assert_eq!(report.ninth_continued_steps, 0);
    assert_eq!(report.tenth_continued_steps, 0);
    assert_eq!(report.eleventh_continued_steps, 0);
    assert_eq!(report.twelfth_continued_steps, 0);
    assert_eq!(report.thirteenth_continued_steps, 0);
    assert_eq!(report.fourteenth_continued_steps, 0);
    assert!(report.seventh_continued_observation.is_empty());
    assert!(report.eighth_continued_observation.is_empty());
    assert!(report.ninth_continued_observation.is_empty());
    assert!(report.tenth_continued_observation.is_empty());
    assert!(report.eleventh_continued_observation.is_empty());
    assert!(report.twelfth_continued_observation.is_empty());
    assert!(report.thirteenth_continued_observation.is_empty());
    assert!(report.fourteenth_continued_observation.is_empty());
}

#[tokio::test]
async fn policy_demo_wait_user_data_reconciles_second_continued_tape() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.client_order_id_prefix = "unit-spot-wait4".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
    ]);
    let waits = Cell::new(0usize);

    let report = run_spot_policy_demo_with_placer_and_user_data(
        config,
        &policy,
        "wait-second-continue-place",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-spot-wait4-spot-0000" => 171,
                    "unit-spot-wait4-spot-0001" => 172,
                    "unit-spot-wait4-spot-0002" => 173,
                    "unit-spot-wait4-spot-0003" => 174,
                    other => panic!("unexpected placed client order id {other}"),
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
            waits.set(waits.get() + 1);
            let payload = report
                .receipts
                .iter()
                .map(|receipt| {
                    spot_execution_report_json(
                        &receipt.symbol,
                        &receipt.client_order_id,
                        receipt.order_id,
                        &receipt.side,
                        "FILLED",
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            let messages = policy_demo_user_data_messages_from_json(&payload);
            async move { messages }
        },
    )
    .await
    .unwrap();

    assert_eq!(waits.get(), 3);
    assert_eq!(report.second_continued_steps, 1);
    assert_eq!(report.placed_orders, 4);
    assert_eq!(report.receipts.len(), 4);
    assert_eq!(report.reconciliations.len(), 4);
    assert_eq!(
        report.reconciliations[0].client_order_id,
        "unit-spot-wait4-spot-0000"
    );
    assert_eq!(
        report.reconciliations[2].client_order_id,
        "unit-spot-wait4-spot-0002"
    );
    assert_eq!(
        report.reconciliations[3].client_order_id,
        "unit-spot-wait4-spot-0003"
    );
    assert_eq!(report.reconciliations[3].side, "SELL");
    assert!(report.reconciliations[3].terminal);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: -1,
        }]
    );
}

#[tokio::test]
async fn policy_demo_wait_user_data_reconciles_third_continued_tape() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.client_order_id_prefix = "unit-spot-wait5".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
    ]);
    let waits = Cell::new(0usize);

    let report = run_spot_policy_demo_with_placer_and_user_data(
        config,
        &policy,
        "wait-third-continue-place",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-spot-wait5-spot-0000" => 211,
                    "unit-spot-wait5-spot-0001" => 212,
                    "unit-spot-wait5-spot-0002" => 213,
                    "unit-spot-wait5-spot-0003" => 214,
                    "unit-spot-wait5-spot-0004" => 215,
                    other => panic!("unexpected placed client order id {other}"),
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
            waits.set(waits.get() + 1);
            let payload = report
                .receipts
                .iter()
                .map(|receipt| {
                    spot_execution_report_json(
                        &receipt.symbol,
                        &receipt.client_order_id,
                        receipt.order_id,
                        &receipt.side,
                        "FILLED",
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            let messages = policy_demo_user_data_messages_from_json(&payload);
            async move { messages }
        },
    )
    .await
    .unwrap();

    assert_eq!(waits.get(), 4);
    assert_eq!(report.third_continued_steps, 1);
    assert_eq!(report.placed_orders, 5);
    assert_eq!(report.receipts.len(), 5);
    assert_eq!(report.reconciliations.len(), 5);
    assert_eq!(
        report.reconciliations[0].client_order_id,
        "unit-spot-wait5-spot-0000"
    );
    assert_eq!(
        report.reconciliations[3].client_order_id,
        "unit-spot-wait5-spot-0003"
    );
    assert_eq!(
        report.reconciliations[4].client_order_id,
        "unit-spot-wait5-spot-0004"
    );
    assert_eq!(report.reconciliations[4].side, "BUY");
    assert!(report.reconciliations[4].terminal);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: 1,
        }]
    );
}

#[tokio::test]
async fn policy_demo_wait_user_data_reconciles_fourth_continued_tape() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.client_order_id_prefix = "unit-spot-wait6".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
    ]);
    let waits = Cell::new(0usize);

    let report = run_spot_policy_demo_with_placer_and_user_data(
        config,
        &policy,
        "wait-fourth-continue-place",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-spot-wait6-spot-0000" => 241,
                    "unit-spot-wait6-spot-0001" => 242,
                    "unit-spot-wait6-spot-0002" => 243,
                    "unit-spot-wait6-spot-0003" => 244,
                    "unit-spot-wait6-spot-0004" => 245,
                    "unit-spot-wait6-spot-0005" => 246,
                    other => panic!("unexpected placed client order id {other}"),
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
            waits.set(waits.get() + 1);
            let payload = report
                .receipts
                .iter()
                .map(|receipt| {
                    spot_execution_report_json(
                        &receipt.symbol,
                        &receipt.client_order_id,
                        receipt.order_id,
                        &receipt.side,
                        "FILLED",
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            let messages = policy_demo_user_data_messages_from_json(&payload);
            async move { messages }
        },
    )
    .await
    .unwrap();

    assert_eq!(waits.get(), 5);
    assert_eq!(report.fourth_continued_steps, 1);
    assert_eq!(report.placed_orders, 6);
    assert_eq!(report.receipts.len(), 6);
    assert_eq!(report.reconciliations.len(), 6);
    assert_eq!(
        report.reconciliations[0].client_order_id,
        "unit-spot-wait6-spot-0000"
    );
    assert_eq!(
        report.reconciliations[4].client_order_id,
        "unit-spot-wait6-spot-0004"
    );
    assert_eq!(
        report.reconciliations[5].client_order_id,
        "unit-spot-wait6-spot-0005"
    );
    assert_eq!(report.reconciliations[5].side, "SELL");
    assert!(report.reconciliations[5].terminal);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: -1,
        }]
    );
}

#[tokio::test]
async fn policy_demo_wait_user_data_reconciles_fifth_continued_tape() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.client_order_id_prefix = "unit-spot-wait8".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
    ]);
    let waits = Cell::new(0usize);

    let report = run_spot_policy_demo_with_placer_and_user_data(
        config,
        &policy,
        "wait-fifth-continue-place",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-spot-wait8-spot-0000" => 271,
                    "unit-spot-wait8-spot-0001" => 272,
                    "unit-spot-wait8-spot-0002" => 273,
                    "unit-spot-wait8-spot-0003" => 274,
                    "unit-spot-wait8-spot-0004" => 275,
                    "unit-spot-wait8-spot-0005" => 276,
                    "unit-spot-wait8-spot-0006" => 277,
                    other => panic!("unexpected placed client order id {other}"),
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
            waits.set(waits.get() + 1);
            let payload = report
                .receipts
                .iter()
                .map(|receipt| {
                    spot_execution_report_json(
                        &receipt.symbol,
                        &receipt.client_order_id,
                        receipt.order_id,
                        &receipt.side,
                        "FILLED",
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            let messages = policy_demo_user_data_messages_from_json(&payload);
            async move { messages }
        },
    )
    .await
    .unwrap();

    assert_eq!(waits.get(), 6);
    assert_eq!(report.fifth_continued_steps, 1);
    assert_eq!(report.placed_orders, 7);
    assert_eq!(report.receipts.len(), 7);
    assert_eq!(report.reconciliations.len(), 7);
    assert_eq!(
        report.reconciliations[0].client_order_id,
        "unit-spot-wait8-spot-0000"
    );
    assert_eq!(
        report.reconciliations[5].client_order_id,
        "unit-spot-wait8-spot-0005"
    );
    assert_eq!(
        report.reconciliations[6].client_order_id,
        "unit-spot-wait8-spot-0006"
    );
    assert_eq!(report.reconciliations[6].side, "BUY");
    assert!(report.reconciliations[6].terminal);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: 1,
        }]
    );
}

#[tokio::test]
async fn policy_demo_wait_user_data_reconciles_sixth_continued_tape() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.client_order_id_prefix = "unit-spot-wait9".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
    ]);
    let waits = Cell::new(0usize);

    let report = run_spot_policy_demo_with_placer_and_user_data(
        config,
        &policy,
        "wait-sixth-continue-place",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-spot-wait9-spot-0000" => 331,
                    "unit-spot-wait9-spot-0001" => 332,
                    "unit-spot-wait9-spot-0002" => 333,
                    "unit-spot-wait9-spot-0003" => 334,
                    "unit-spot-wait9-spot-0004" => 335,
                    "unit-spot-wait9-spot-0005" => 336,
                    "unit-spot-wait9-spot-0006" => 337,
                    "unit-spot-wait9-spot-0007" => 338,
                    other => panic!("unexpected placed client order id {other}"),
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
            waits.set(waits.get() + 1);
            let payload = report
                .receipts
                .iter()
                .map(|receipt| {
                    spot_execution_report_json(
                        &receipt.symbol,
                        &receipt.client_order_id,
                        receipt.order_id,
                        &receipt.side,
                        "FILLED",
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            let messages = policy_demo_user_data_messages_from_json(&payload);
            async move { messages }
        },
    )
    .await
    .unwrap();

    assert_eq!(waits.get(), 7);
    assert_eq!(report.sixth_continued_steps, 1);
    assert_eq!(report.seventh_continued_steps, 0);
    assert_eq!(report.eighth_continued_steps, 0);
    assert_eq!(report.ninth_continued_steps, 0);
    assert_eq!(report.tenth_continued_steps, 0);
    assert_eq!(report.eleventh_continued_steps, 0);
    assert_eq!(report.twelfth_continued_steps, 0);
    assert_eq!(report.thirteenth_continued_steps, 0);
    assert_eq!(report.fourteenth_continued_steps, 0);
    assert_eq!(report.placed_orders, 8);
    assert_eq!(report.receipts.len(), 8);
    assert_eq!(report.reconciliations.len(), 8);
    assert_eq!(
        report.reconciliations[0].client_order_id,
        "unit-spot-wait9-spot-0000"
    );
    assert_eq!(
        report.reconciliations[6].client_order_id,
        "unit-spot-wait9-spot-0006"
    );
    assert_eq!(
        report.reconciliations[7].client_order_id,
        "unit-spot-wait9-spot-0007"
    );
    assert_eq!(report.reconciliations[7].side, "SELL");
    assert!(report.reconciliations[7].terminal);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: -1,
        }]
    );
}

#[tokio::test]
async fn policy_demo_wait_user_data_reconciles_seventh_continued_tape() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.client_order_id_prefix = "unit-spot-wait10".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    config.seventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"216.00","qty":"1.0"}],"asks":[{"price":"218.00","qty":"1.0"}],"update_id":15}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
    ]);
    let waits = Cell::new(0usize);

    let report = run_spot_policy_demo_with_placer_and_user_data(
        config,
        &policy,
        "wait-seventh-continue-place",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-spot-wait10-spot-0000" => 351,
                    "unit-spot-wait10-spot-0001" => 352,
                    "unit-spot-wait10-spot-0002" => 353,
                    "unit-spot-wait10-spot-0003" => 354,
                    "unit-spot-wait10-spot-0004" => 355,
                    "unit-spot-wait10-spot-0005" => 356,
                    "unit-spot-wait10-spot-0006" => 357,
                    "unit-spot-wait10-spot-0007" => 358,
                    "unit-spot-wait10-spot-0008" => 359,
                    other => panic!("unexpected placed client order id {other}"),
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
            waits.set(waits.get() + 1);
            let payload = report
                .receipts
                .iter()
                .map(|receipt| {
                    spot_execution_report_json(
                        &receipt.symbol,
                        &receipt.client_order_id,
                        receipt.order_id,
                        &receipt.side,
                        "FILLED",
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            let messages = policy_demo_user_data_messages_from_json(&payload);
            async move { messages }
        },
    )
    .await
    .unwrap();

    assert_eq!(waits.get(), 8);
    assert_eq!(report.seventh_continued_steps, 1);
    assert_eq!(report.eighth_continued_steps, 0);
    assert_eq!(report.ninth_continued_steps, 0);
    assert_eq!(report.tenth_continued_steps, 0);
    assert_eq!(report.eleventh_continued_steps, 0);
    assert_eq!(report.twelfth_continued_steps, 0);
    assert_eq!(report.thirteenth_continued_steps, 0);
    assert_eq!(report.fourteenth_continued_steps, 0);
    assert_eq!(report.placed_orders, 9);
    assert_eq!(report.receipts.len(), 9);
    assert_eq!(report.reconciliations.len(), 9);
    assert_eq!(
        report.reconciliations[0].client_order_id,
        "unit-spot-wait10-spot-0000"
    );
    assert_eq!(
        report.reconciliations[7].client_order_id,
        "unit-spot-wait10-spot-0007"
    );
    assert_eq!(
        report.reconciliations[8].client_order_id,
        "unit-spot-wait10-spot-0008"
    );
    assert_eq!(report.reconciliations[8].side, "BUY");
    assert!(report.reconciliations[8].terminal);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: 1,
        }]
    );
}

#[tokio::test]
async fn policy_demo_wait_user_data_reconciles_eighth_continued_tape() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.client_order_id_prefix = "unit-spot-wait11".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    config.seventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"216.00","qty":"1.0"}],"asks":[{"price":"218.00","qty":"1.0"}],"update_id":15}"#
            .into(),
    );
    config.eighth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"217.00","qty":"1.0"}],"asks":[{"price":"219.00","qty":"1.0"}],"update_id":16}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
    ]);
    let waits = Cell::new(0usize);

    let report = run_spot_policy_demo_with_placer_and_user_data(
        config,
        &policy,
        "wait-eighth-continue-place",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-spot-wait11-spot-0000" => 421,
                    "unit-spot-wait11-spot-0001" => 422,
                    "unit-spot-wait11-spot-0002" => 423,
                    "unit-spot-wait11-spot-0003" => 424,
                    "unit-spot-wait11-spot-0004" => 425,
                    "unit-spot-wait11-spot-0005" => 426,
                    "unit-spot-wait11-spot-0006" => 427,
                    "unit-spot-wait11-spot-0007" => 428,
                    "unit-spot-wait11-spot-0008" => 429,
                    "unit-spot-wait11-spot-0009" => 430,
                    other => panic!("unexpected placed client order id {other}"),
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
            waits.set(waits.get() + 1);
            let payload = report
                .receipts
                .iter()
                .map(|receipt| {
                    spot_execution_report_json(
                        &receipt.symbol,
                        &receipt.client_order_id,
                        receipt.order_id,
                        &receipt.side,
                        "FILLED",
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            let messages = policy_demo_user_data_messages_from_json(&payload);
            async move { messages }
        },
    )
    .await
    .unwrap();

    assert_eq!(waits.get(), 9);
    assert_eq!(report.seventh_continued_steps, 1);
    assert_eq!(report.eighth_continued_steps, 1);
    assert_eq!(report.ninth_continued_steps, 0);
    assert_eq!(report.tenth_continued_steps, 0);
    assert_eq!(report.eleventh_continued_steps, 0);
    assert_eq!(report.twelfth_continued_steps, 0);
    assert_eq!(report.thirteenth_continued_steps, 0);
    assert_eq!(report.fourteenth_continued_steps, 0);
    assert_eq!(report.placed_orders, 10);
    assert_eq!(report.receipts.len(), 10);
    assert_eq!(report.reconciliations.len(), 10);
    assert_eq!(
        report.reconciliations[0].client_order_id,
        "unit-spot-wait11-spot-0000"
    );
    assert_eq!(
        report.reconciliations[8].client_order_id,
        "unit-spot-wait11-spot-0008"
    );
    assert_eq!(
        report.reconciliations[9].client_order_id,
        "unit-spot-wait11-spot-0009"
    );
    assert_eq!(report.reconciliations[9].side, "SELL");
    assert!(report.reconciliations[9].terminal);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: -1,
        }]
    );
}

#[tokio::test]
async fn policy_demo_wait_user_data_reconciles_ninth_continued_tape() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.client_order_id_prefix = "unit-spot-wait12".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    config.seventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"216.00","qty":"1.0"}],"asks":[{"price":"218.00","qty":"1.0"}],"update_id":15}"#
            .into(),
    );
    config.eighth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"217.00","qty":"1.0"}],"asks":[{"price":"219.00","qty":"1.0"}],"update_id":16}"#
            .into(),
    );
    config.ninth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"218.00","qty":"1.0"}],"asks":[{"price":"220.00","qty":"1.0"}],"update_id":17}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
    ]);
    let waits = Cell::new(0usize);

    let report = run_spot_policy_demo_with_placer_and_user_data(
        config,
        &policy,
        "wait-ninth-continue-place",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-spot-wait12-spot-0000" => 451,
                    "unit-spot-wait12-spot-0001" => 452,
                    "unit-spot-wait12-spot-0002" => 453,
                    "unit-spot-wait12-spot-0003" => 454,
                    "unit-spot-wait12-spot-0004" => 455,
                    "unit-spot-wait12-spot-0005" => 456,
                    "unit-spot-wait12-spot-0006" => 457,
                    "unit-spot-wait12-spot-0007" => 458,
                    "unit-spot-wait12-spot-0008" => 459,
                    "unit-spot-wait12-spot-0009" => 460,
                    "unit-spot-wait12-spot-0010" => 461,
                    other => panic!("unexpected placed client order id {other}"),
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
            waits.set(waits.get() + 1);
            let payload = report
                .receipts
                .iter()
                .map(|receipt| {
                    spot_execution_report_json(
                        &receipt.symbol,
                        &receipt.client_order_id,
                        receipt.order_id,
                        &receipt.side,
                        "FILLED",
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            let messages = policy_demo_user_data_messages_from_json(&payload);
            async move { messages }
        },
    )
    .await
    .unwrap();

    assert_eq!(waits.get(), 10);
    assert_eq!(report.eighth_continued_steps, 1);
    assert_eq!(report.ninth_continued_steps, 1);
    assert_eq!(report.placed_orders, 11);
    assert_eq!(report.receipts.len(), 11);
    assert_eq!(report.reconciliations.len(), 11);
    assert_eq!(
        report.reconciliations[0].client_order_id,
        "unit-spot-wait12-spot-0000"
    );
    assert_eq!(
        report.reconciliations[9].client_order_id,
        "unit-spot-wait12-spot-0009"
    );
    assert_eq!(
        report.reconciliations[10].client_order_id,
        "unit-spot-wait12-spot-0010"
    );
    assert_eq!(report.reconciliations[10].side, "BUY");
    assert!(report.reconciliations[10].terminal);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: 1,
        }]
    );
}

#[tokio::test]
async fn policy_demo_live_source_waits_after_second_continued_tape_place() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.client_order_id_prefix = "unit-spot-live4".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
    ]);
    let frames = policy_demo_user_data_messages_from_json(&format!(
        "{}\n{}\n{}\n{}",
        spot_execution_report_json("ETHUSDT", "unit-spot-live4-spot-0000", 181, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live4-spot-0001",
            182,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live4-spot-0002", 183, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live4-spot-0003",
            184,
            "SELL",
            "FILLED"
        ),
    ))
    .unwrap();

    let report = run_spot_policy_demo_with_live_user_data_source(
        config,
        &policy,
        "live-source-second-continue-place",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-spot-live4-spot-0000" => 181,
                    "unit-spot-live4-spot-0001" => 182,
                    "unit-spot-live4-spot-0002" => 183,
                    "unit-spot-live4-spot-0003" => 184,
                    other => panic!("unexpected placed client order id {other}"),
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
        frames,
    )
    .await
    .unwrap();

    assert_eq!(report.second_continued_steps, 1);
    assert_eq!(report.placed_orders, 4);
    assert_eq!(report.receipts.len(), 4);
    assert_eq!(report.reconciliations.len(), 4);
    assert_eq!(
        report.reconciliations[0].client_order_id,
        "unit-spot-live4-spot-0000"
    );
    assert_eq!(
        report.reconciliations[2].client_order_id,
        "unit-spot-live4-spot-0002"
    );
    assert_eq!(
        report.reconciliations[3].client_order_id,
        "unit-spot-live4-spot-0003"
    );
    assert_eq!(report.reconciliations[3].side, "SELL");
    assert!(report.reconciliations[3].terminal);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: -1,
        }]
    );
}

#[tokio::test]
async fn policy_demo_usdm_live_source_waits_after_second_continued_tape_place() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Usdm, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.client_order_id_prefix = "unit-usdm-live4".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
    ]);
    let frames = policy_demo_user_data_messages_from_json(
        &serde_json::json!([
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live4-usdm-0000",
                191,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live4-usdm-0001",
                192,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live4-usdm-0002",
                193,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live4-usdm-0003",
                194,
                "SELL",
                "FILLED"
            )
        ])
        .to_string(),
    )
    .unwrap();

    let report = run_usdm_policy_demo_with_live_user_data_source(
        config,
        &policy,
        "live-source-second-continue-place-usdm",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(UsdmPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-usdm-live4-usdm-0000" => 191,
                    "unit-usdm-live4-usdm-0001" => 192,
                    "unit-usdm-live4-usdm-0002" => 193,
                    "unit-usdm-live4-usdm-0003" => 194,
                    other => panic!("unexpected placed client order id {other}"),
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
        frames,
    )
    .await
    .unwrap();

    assert_eq!(report.second_continued_steps, 1);
    assert_eq!(report.placed_orders, 4);
    assert_eq!(report.reconciliations.len(), 4);
    assert_eq!(
        report.reconciliations[3].client_order_id,
        "unit-usdm-live4-usdm-0003"
    );
    assert_eq!(report.reconciliations[3].side, "SELL");
    assert!(report.reconciliations[3].terminal);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: -1,
        }]
    );
}

#[tokio::test]
async fn policy_demo_live_source_waits_after_third_continued_tape_place() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.client_order_id_prefix = "unit-spot-live5".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
    ]);
    let frames = policy_demo_user_data_messages_from_json(&format!(
        "{}\n{}\n{}\n{}\n{}",
        spot_execution_report_json("ETHUSDT", "unit-spot-live5-spot-0000", 221, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live5-spot-0001",
            222,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live5-spot-0002", 223, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live5-spot-0003",
            224,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live5-spot-0004", 225, "BUY", "FILLED"),
    ))
    .unwrap();

    let report = run_spot_policy_demo_with_live_user_data_source(
        config,
        &policy,
        "live-source-third-continue-place",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-spot-live5-spot-0000" => 221,
                    "unit-spot-live5-spot-0001" => 222,
                    "unit-spot-live5-spot-0002" => 223,
                    "unit-spot-live5-spot-0003" => 224,
                    "unit-spot-live5-spot-0004" => 225,
                    other => panic!("unexpected placed client order id {other}"),
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
        frames,
    )
    .await
    .unwrap();

    assert_eq!(report.third_continued_steps, 1);
    assert_eq!(report.placed_orders, 5);
    assert_eq!(report.receipts.len(), 5);
    assert_eq!(report.reconciliations.len(), 5);
    assert_eq!(
        report.reconciliations[0].client_order_id,
        "unit-spot-live5-spot-0000"
    );
    assert_eq!(
        report.reconciliations[3].client_order_id,
        "unit-spot-live5-spot-0003"
    );
    assert_eq!(
        report.reconciliations[4].client_order_id,
        "unit-spot-live5-spot-0004"
    );
    assert_eq!(report.reconciliations[4].side, "BUY");
    assert!(report.reconciliations[4].terminal);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: 1,
        }]
    );
}

#[tokio::test]
async fn policy_demo_usdm_live_source_waits_after_third_continued_tape_place() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Usdm, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.client_order_id_prefix = "unit-usdm-live5".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
    ]);
    let frames = policy_demo_user_data_messages_from_json(
        &serde_json::json!([
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live5-usdm-0000",
                231,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live5-usdm-0001",
                232,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live5-usdm-0002",
                233,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live5-usdm-0003",
                234,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live5-usdm-0004",
                235,
                "BUY",
                "FILLED"
            )
        ])
        .to_string(),
    )
    .unwrap();

    let report = run_usdm_policy_demo_with_live_user_data_source(
        config,
        &policy,
        "live-source-third-continue-place-usdm",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(UsdmPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-usdm-live5-usdm-0000" => 231,
                    "unit-usdm-live5-usdm-0001" => 232,
                    "unit-usdm-live5-usdm-0002" => 233,
                    "unit-usdm-live5-usdm-0003" => 234,
                    "unit-usdm-live5-usdm-0004" => 235,
                    other => panic!("unexpected placed client order id {other}"),
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
        frames,
    )
    .await
    .unwrap();

    assert_eq!(report.third_continued_steps, 1);
    assert_eq!(report.placed_orders, 5);
    assert_eq!(report.reconciliations.len(), 5);
    assert_eq!(
        report.reconciliations[4].client_order_id,
        "unit-usdm-live5-usdm-0004"
    );
    assert_eq!(report.reconciliations[4].side, "BUY");
    assert!(report.reconciliations[4].terminal);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: 1,
        }]
    );
}

#[tokio::test]
async fn policy_demo_live_source_waits_after_fourth_continued_tape_place() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.client_order_id_prefix = "unit-spot-live6".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
    ]);
    let frames = policy_demo_user_data_messages_from_json(&format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        spot_execution_report_json("ETHUSDT", "unit-spot-live6-spot-0000", 251, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live6-spot-0001",
            252,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live6-spot-0002", 253, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live6-spot-0003",
            254,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live6-spot-0004", 255, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live6-spot-0005",
            256,
            "SELL",
            "FILLED"
        ),
    ))
    .unwrap();

    let report = run_spot_policy_demo_with_live_user_data_source(
        config,
        &policy,
        "live-source-fourth-continue-place",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-spot-live6-spot-0000" => 251,
                    "unit-spot-live6-spot-0001" => 252,
                    "unit-spot-live6-spot-0002" => 253,
                    "unit-spot-live6-spot-0003" => 254,
                    "unit-spot-live6-spot-0004" => 255,
                    "unit-spot-live6-spot-0005" => 256,
                    other => panic!("unexpected placed client order id {other}"),
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
        frames,
    )
    .await
    .unwrap();

    assert_eq!(report.fourth_continued_steps, 1);
    assert_eq!(report.placed_orders, 6);
    assert_eq!(report.receipts.len(), 6);
    assert_eq!(report.reconciliations.len(), 6);
    assert_eq!(
        report.reconciliations[0].client_order_id,
        "unit-spot-live6-spot-0000"
    );
    assert_eq!(
        report.reconciliations[4].client_order_id,
        "unit-spot-live6-spot-0004"
    );
    assert_eq!(
        report.reconciliations[5].client_order_id,
        "unit-spot-live6-spot-0005"
    );
    assert_eq!(report.reconciliations[5].side, "SELL");
    assert!(report.reconciliations[5].terminal);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: -1,
        }]
    );
}

#[tokio::test]
async fn policy_demo_usdm_live_source_waits_after_fourth_continued_tape_place() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Usdm, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.client_order_id_prefix = "unit-usdm-live6".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
    ]);
    let frames = policy_demo_user_data_messages_from_json(
        &serde_json::json!([
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live6-usdm-0000",
                261,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live6-usdm-0001",
                262,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live6-usdm-0002",
                263,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live6-usdm-0003",
                264,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live6-usdm-0004",
                265,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live6-usdm-0005",
                266,
                "SELL",
                "FILLED"
            )
        ])
        .to_string(),
    )
    .unwrap();

    let report = run_usdm_policy_demo_with_live_user_data_source(
        config,
        &policy,
        "live-source-fourth-continue-place-usdm",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(UsdmPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-usdm-live6-usdm-0000" => 261,
                    "unit-usdm-live6-usdm-0001" => 262,
                    "unit-usdm-live6-usdm-0002" => 263,
                    "unit-usdm-live6-usdm-0003" => 264,
                    "unit-usdm-live6-usdm-0004" => 265,
                    "unit-usdm-live6-usdm-0005" => 266,
                    other => panic!("unexpected placed client order id {other}"),
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
        frames,
    )
    .await
    .unwrap();

    assert_eq!(report.fourth_continued_steps, 1);
    assert_eq!(report.placed_orders, 6);
    assert_eq!(report.reconciliations.len(), 6);
    assert_eq!(
        report.reconciliations[5].client_order_id,
        "unit-usdm-live6-usdm-0005"
    );
    assert_eq!(report.reconciliations[5].side, "SELL");
    assert!(report.reconciliations[5].terminal);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: -1,
        }]
    );
}

#[tokio::test]
async fn policy_demo_live_source_waits_after_fifth_continued_tape_place() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.client_order_id_prefix = "unit-spot-live8".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
    ]);
    let frames = policy_demo_user_data_messages_from_json(&format!(
        "{}\n{}\n{}\n{}\n{}\n{}\n{}",
        spot_execution_report_json("ETHUSDT", "unit-spot-live8-spot-0000", 281, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live8-spot-0001",
            282,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live8-spot-0002", 283, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live8-spot-0003",
            284,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live8-spot-0004", 285, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live8-spot-0005",
            286,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live8-spot-0006", 287, "BUY", "FILLED"),
    ))
    .unwrap();

    let report = run_spot_policy_demo_with_live_user_data_source(
        config,
        &policy,
        "live-source-fifth-continue-place",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-spot-live8-spot-0000" => 281,
                    "unit-spot-live8-spot-0001" => 282,
                    "unit-spot-live8-spot-0002" => 283,
                    "unit-spot-live8-spot-0003" => 284,
                    "unit-spot-live8-spot-0004" => 285,
                    "unit-spot-live8-spot-0005" => 286,
                    "unit-spot-live8-spot-0006" => 287,
                    other => panic!("unexpected placed client order id {other}"),
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
        frames,
    )
    .await
    .unwrap();

    assert_eq!(report.fifth_continued_steps, 1);
    assert_eq!(report.placed_orders, 7);
    assert_eq!(report.receipts.len(), 7);
    assert_eq!(report.reconciliations.len(), 7);
    assert_eq!(
        report.reconciliations[0].client_order_id,
        "unit-spot-live8-spot-0000"
    );
    assert_eq!(
        report.reconciliations[5].client_order_id,
        "unit-spot-live8-spot-0005"
    );
    assert_eq!(
        report.reconciliations[6].client_order_id,
        "unit-spot-live8-spot-0006"
    );
    assert_eq!(report.reconciliations[6].side, "BUY");
    assert!(report.reconciliations[6].terminal);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: 1,
        }]
    );
}

#[tokio::test]
async fn policy_demo_live_source_waits_after_sixth_continued_tape_place() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.client_order_id_prefix = "unit-spot-live9".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
    ]);
    let frames = policy_demo_user_data_messages_from_json(&format!(
        "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
        spot_execution_report_json("ETHUSDT", "unit-spot-live9-spot-0000", 341, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live9-spot-0001",
            342,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live9-spot-0002", 343, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live9-spot-0003",
            344,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live9-spot-0004", 345, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live9-spot-0005",
            346,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live9-spot-0006", 347, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live9-spot-0007",
            348,
            "SELL",
            "FILLED"
        ),
    ))
    .unwrap();

    let report = run_spot_policy_demo_with_live_user_data_source(
        config,
        &policy,
        "live-source-sixth-continue-place",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-spot-live9-spot-0000" => 341,
                    "unit-spot-live9-spot-0001" => 342,
                    "unit-spot-live9-spot-0002" => 343,
                    "unit-spot-live9-spot-0003" => 344,
                    "unit-spot-live9-spot-0004" => 345,
                    "unit-spot-live9-spot-0005" => 346,
                    "unit-spot-live9-spot-0006" => 347,
                    "unit-spot-live9-spot-0007" => 348,
                    other => panic!("unexpected placed client order id {other}"),
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
        frames,
    )
    .await
    .unwrap();

    assert_eq!(report.sixth_continued_steps, 1);
    assert_eq!(report.seventh_continued_steps, 0);
    assert_eq!(report.eighth_continued_steps, 0);
    assert_eq!(report.ninth_continued_steps, 0);
    assert_eq!(report.tenth_continued_steps, 0);
    assert_eq!(report.eleventh_continued_steps, 0);
    assert_eq!(report.twelfth_continued_steps, 0);
    assert_eq!(report.thirteenth_continued_steps, 0);
    assert_eq!(report.fourteenth_continued_steps, 0);
    assert_eq!(report.placed_orders, 8);
    assert_eq!(report.receipts.len(), 8);
    assert_eq!(report.reconciliations.len(), 8);
    assert_eq!(
        report.reconciliations[0].client_order_id,
        "unit-spot-live9-spot-0000"
    );
    assert_eq!(
        report.reconciliations[6].client_order_id,
        "unit-spot-live9-spot-0006"
    );
    assert_eq!(
        report.reconciliations[7].client_order_id,
        "unit-spot-live9-spot-0007"
    );
    assert_eq!(report.reconciliations[7].side, "SELL");
    assert!(report.reconciliations[7].terminal);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: -1,
        }]
    );
}

#[tokio::test]
async fn policy_demo_live_source_waits_after_seventh_continued_tape_place() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.client_order_id_prefix = "unit-spot-live10".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    config.seventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"216.00","qty":"1.0"}],"asks":[{"price":"218.00","qty":"1.0"}],"update_id":15}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
    ]);
    let frames = policy_demo_user_data_messages_from_json(&format!(
        "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
        spot_execution_report_json("ETHUSDT", "unit-spot-live10-spot-0000", 361, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live10-spot-0001",
            362,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live10-spot-0002", 363, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live10-spot-0003",
            364,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live10-spot-0004", 365, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live10-spot-0005",
            366,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live10-spot-0006", 367, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live10-spot-0007",
            368,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live10-spot-0008", 369, "BUY", "FILLED"),
    ))
    .unwrap();

    let report = run_spot_policy_demo_with_live_user_data_source(
        config,
        &policy,
        "live-source-seventh-continue-place",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-spot-live10-spot-0000" => 361,
                    "unit-spot-live10-spot-0001" => 362,
                    "unit-spot-live10-spot-0002" => 363,
                    "unit-spot-live10-spot-0003" => 364,
                    "unit-spot-live10-spot-0004" => 365,
                    "unit-spot-live10-spot-0005" => 366,
                    "unit-spot-live10-spot-0006" => 367,
                    "unit-spot-live10-spot-0007" => 368,
                    "unit-spot-live10-spot-0008" => 369,
                    other => panic!("unexpected placed client order id {other}"),
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
        frames,
    )
    .await
    .unwrap();

    assert_eq!(report.seventh_continued_steps, 1);
    assert_eq!(report.eighth_continued_steps, 0);
    assert_eq!(report.ninth_continued_steps, 0);
    assert_eq!(report.tenth_continued_steps, 0);
    assert_eq!(report.eleventh_continued_steps, 0);
    assert_eq!(report.twelfth_continued_steps, 0);
    assert_eq!(report.thirteenth_continued_steps, 0);
    assert_eq!(report.fourteenth_continued_steps, 0);
    assert_eq!(report.placed_orders, 9);
    assert_eq!(report.receipts.len(), 9);
    assert_eq!(report.reconciliations.len(), 9);
    assert_eq!(
        report.reconciliations[0].client_order_id,
        "unit-spot-live10-spot-0000"
    );
    assert_eq!(
        report.reconciliations[7].client_order_id,
        "unit-spot-live10-spot-0007"
    );
    assert_eq!(
        report.reconciliations[8].client_order_id,
        "unit-spot-live10-spot-0008"
    );
    assert_eq!(report.reconciliations[8].side, "BUY");
    assert!(report.reconciliations[8].terminal);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: 1,
        }]
    );
}

#[tokio::test]
async fn policy_demo_live_source_waits_after_eighth_continued_tape_place() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.client_order_id_prefix = "unit-spot-live11".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    config.seventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"216.00","qty":"1.0"}],"asks":[{"price":"218.00","qty":"1.0"}],"update_id":15}"#
            .into(),
    );
    config.eighth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"217.00","qty":"1.0"}],"asks":[{"price":"219.00","qty":"1.0"}],"update_id":16}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
    ]);
    let frames = policy_demo_user_data_messages_from_json(&format!(
        "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
        spot_execution_report_json("ETHUSDT", "unit-spot-live11-spot-0000", 431, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live11-spot-0001",
            432,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live11-spot-0002", 433, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live11-spot-0003",
            434,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live11-spot-0004", 435, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live11-spot-0005",
            436,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live11-spot-0006", 437, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live11-spot-0007",
            438,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live11-spot-0008", 439, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live11-spot-0009",
            440,
            "SELL",
            "FILLED"
        ),
    ))
    .unwrap();

    let report = run_spot_policy_demo_with_live_user_data_source(
        config,
        &policy,
        "live-source-eighth-continue-place",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-spot-live11-spot-0000" => 431,
                    "unit-spot-live11-spot-0001" => 432,
                    "unit-spot-live11-spot-0002" => 433,
                    "unit-spot-live11-spot-0003" => 434,
                    "unit-spot-live11-spot-0004" => 435,
                    "unit-spot-live11-spot-0005" => 436,
                    "unit-spot-live11-spot-0006" => 437,
                    "unit-spot-live11-spot-0007" => 438,
                    "unit-spot-live11-spot-0008" => 439,
                    "unit-spot-live11-spot-0009" => 440,
                    other => panic!("unexpected placed client order id {other}"),
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
        frames,
    )
    .await
    .unwrap();

    assert_eq!(report.seventh_continued_steps, 1);
    assert_eq!(report.eighth_continued_steps, 1);
    assert_eq!(report.ninth_continued_steps, 0);
    assert_eq!(report.tenth_continued_steps, 0);
    assert_eq!(report.eleventh_continued_steps, 0);
    assert_eq!(report.twelfth_continued_steps, 0);
    assert_eq!(report.thirteenth_continued_steps, 0);
    assert_eq!(report.fourteenth_continued_steps, 0);
    assert_eq!(report.placed_orders, 10);
    assert_eq!(report.receipts.len(), 10);
    assert_eq!(report.reconciliations.len(), 10);
    assert_eq!(
        report.reconciliations[0].client_order_id,
        "unit-spot-live11-spot-0000"
    );
    assert_eq!(
        report.reconciliations[8].client_order_id,
        "unit-spot-live11-spot-0008"
    );
    assert_eq!(
        report.reconciliations[9].client_order_id,
        "unit-spot-live11-spot-0009"
    );
    assert_eq!(report.reconciliations[9].side, "SELL");
    assert!(report.reconciliations[9].terminal);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: -1,
        }]
    );
}

#[tokio::test]
async fn policy_demo_usdm_live_source_waits_after_fifth_continued_tape_place() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Usdm, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.client_order_id_prefix = "unit-usdm-live8".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
    ]);
    let frames = policy_demo_user_data_messages_from_json(
        &serde_json::json!([
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live8-usdm-0000",
                291,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live8-usdm-0001",
                292,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live8-usdm-0002",
                293,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live8-usdm-0003",
                294,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live8-usdm-0004",
                295,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live8-usdm-0005",
                296,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live8-usdm-0006",
                297,
                "BUY",
                "FILLED"
            )
        ])
        .to_string(),
    )
    .unwrap();

    let report = run_usdm_policy_demo_with_live_user_data_source(
        config,
        &policy,
        "live-source-fifth-continue-place-usdm",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(UsdmPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-usdm-live8-usdm-0000" => 291,
                    "unit-usdm-live8-usdm-0001" => 292,
                    "unit-usdm-live8-usdm-0002" => 293,
                    "unit-usdm-live8-usdm-0003" => 294,
                    "unit-usdm-live8-usdm-0004" => 295,
                    "unit-usdm-live8-usdm-0005" => 296,
                    "unit-usdm-live8-usdm-0006" => 297,
                    other => panic!("unexpected placed client order id {other}"),
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
        frames,
    )
    .await
    .unwrap();

    assert_eq!(report.fifth_continued_steps, 1);
    assert_eq!(report.placed_orders, 7);
    assert_eq!(report.reconciliations.len(), 7);
    assert_eq!(
        report.reconciliations[6].client_order_id,
        "unit-usdm-live8-usdm-0006"
    );
    assert_eq!(report.reconciliations[6].side, "BUY");
    assert!(report.reconciliations[6].terminal);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: 1,
        }]
    );
}

#[tokio::test]
async fn policy_demo_usdm_live_source_waits_after_sixth_continued_tape_place() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Usdm, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.client_order_id_prefix = "unit-usdm-live9".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
    ]);
    let frames = policy_demo_user_data_messages_from_json(
        &serde_json::json!([
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live9-usdm-0000",
                351,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live9-usdm-0001",
                352,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live9-usdm-0002",
                353,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live9-usdm-0003",
                354,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live9-usdm-0004",
                355,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live9-usdm-0005",
                356,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live9-usdm-0006",
                357,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live9-usdm-0007",
                358,
                "SELL",
                "FILLED"
            )
        ])
        .to_string(),
    )
    .unwrap();

    let report = run_usdm_policy_demo_with_live_user_data_source(
        config,
        &policy,
        "live-source-sixth-continue-place-usdm",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(UsdmPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-usdm-live9-usdm-0000" => 351,
                    "unit-usdm-live9-usdm-0001" => 352,
                    "unit-usdm-live9-usdm-0002" => 353,
                    "unit-usdm-live9-usdm-0003" => 354,
                    "unit-usdm-live9-usdm-0004" => 355,
                    "unit-usdm-live9-usdm-0005" => 356,
                    "unit-usdm-live9-usdm-0006" => 357,
                    "unit-usdm-live9-usdm-0007" => 358,
                    other => panic!("unexpected placed client order id {other}"),
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
        frames,
    )
    .await
    .unwrap();

    assert_eq!(report.sixth_continued_steps, 1);
    assert_eq!(report.seventh_continued_steps, 0);
    assert_eq!(report.eighth_continued_steps, 0);
    assert_eq!(report.ninth_continued_steps, 0);
    assert_eq!(report.tenth_continued_steps, 0);
    assert_eq!(report.eleventh_continued_steps, 0);
    assert_eq!(report.twelfth_continued_steps, 0);
    assert_eq!(report.thirteenth_continued_steps, 0);
    assert_eq!(report.fourteenth_continued_steps, 0);
    assert_eq!(report.placed_orders, 8);
    assert_eq!(report.reconciliations.len(), 8);
    assert_eq!(
        report.reconciliations[7].client_order_id,
        "unit-usdm-live9-usdm-0007"
    );
    assert_eq!(report.reconciliations[7].side, "SELL");
    assert!(report.reconciliations[7].terminal);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: -1,
        }]
    );
}

#[tokio::test]
async fn policy_demo_usdm_live_source_waits_after_seventh_continued_tape_place() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Usdm, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.client_order_id_prefix = "unit-usdm-live10".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    config.seventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"216.00","qty":"1.0"}],"asks":[{"price":"218.00","qty":"1.0"}],"update_id":15}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
    ]);
    let frames = policy_demo_user_data_messages_from_json(
        &serde_json::json!([
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live10-usdm-0000",
                371,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live10-usdm-0001",
                372,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live10-usdm-0002",
                373,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live10-usdm-0003",
                374,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live10-usdm-0004",
                375,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live10-usdm-0005",
                376,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live10-usdm-0006",
                377,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live10-usdm-0007",
                378,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live10-usdm-0008",
                379,
                "BUY",
                "FILLED"
            )
        ])
        .to_string(),
    )
    .unwrap();

    let report = run_usdm_policy_demo_with_live_user_data_source(
        config,
        &policy,
        "live-source-seventh-continue-place-usdm",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(UsdmPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-usdm-live10-usdm-0000" => 371,
                    "unit-usdm-live10-usdm-0001" => 372,
                    "unit-usdm-live10-usdm-0002" => 373,
                    "unit-usdm-live10-usdm-0003" => 374,
                    "unit-usdm-live10-usdm-0004" => 375,
                    "unit-usdm-live10-usdm-0005" => 376,
                    "unit-usdm-live10-usdm-0006" => 377,
                    "unit-usdm-live10-usdm-0007" => 378,
                    "unit-usdm-live10-usdm-0008" => 379,
                    other => panic!("unexpected placed client order id {other}"),
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
        frames,
    )
    .await
    .unwrap();

    assert_eq!(report.seventh_continued_steps, 1);
    assert_eq!(report.eighth_continued_steps, 0);
    assert_eq!(report.ninth_continued_steps, 0);
    assert_eq!(report.tenth_continued_steps, 0);
    assert_eq!(report.eleventh_continued_steps, 0);
    assert_eq!(report.twelfth_continued_steps, 0);
    assert_eq!(report.thirteenth_continued_steps, 0);
    assert_eq!(report.fourteenth_continued_steps, 0);
    assert_eq!(report.placed_orders, 9);
    assert_eq!(report.reconciliations.len(), 9);
    assert_eq!(
        report.reconciliations[8].client_order_id,
        "unit-usdm-live10-usdm-0008"
    );
    assert_eq!(report.reconciliations[8].side, "BUY");
    assert!(report.reconciliations[8].terminal);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: 1,
        }]
    );
}

#[tokio::test]
async fn policy_demo_usdm_live_source_waits_after_eighth_continued_tape_place() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Usdm, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.client_order_id_prefix = "unit-usdm-live11".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    config.seventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"216.00","qty":"1.0"}],"asks":[{"price":"218.00","qty":"1.0"}],"update_id":15}"#
            .into(),
    );
    config.eighth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"217.00","qty":"1.0"}],"asks":[{"price":"219.00","qty":"1.0"}],"update_id":16}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
    ]);
    let frames = policy_demo_user_data_messages_from_json(
        &serde_json::json!([
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live11-usdm-0000",
                441,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live11-usdm-0001",
                442,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live11-usdm-0002",
                443,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live11-usdm-0003",
                444,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live11-usdm-0004",
                445,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live11-usdm-0005",
                446,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live11-usdm-0006",
                447,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live11-usdm-0007",
                448,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live11-usdm-0008",
                449,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live11-usdm-0009",
                450,
                "SELL",
                "FILLED"
            )
        ])
        .to_string(),
    )
    .unwrap();

    let report = run_usdm_policy_demo_with_live_user_data_source(
        config,
        &policy,
        "live-source-eighth-continue-place-usdm",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(UsdmPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-usdm-live11-usdm-0000" => 441,
                    "unit-usdm-live11-usdm-0001" => 442,
                    "unit-usdm-live11-usdm-0002" => 443,
                    "unit-usdm-live11-usdm-0003" => 444,
                    "unit-usdm-live11-usdm-0004" => 445,
                    "unit-usdm-live11-usdm-0005" => 446,
                    "unit-usdm-live11-usdm-0006" => 447,
                    "unit-usdm-live11-usdm-0007" => 448,
                    "unit-usdm-live11-usdm-0008" => 449,
                    "unit-usdm-live11-usdm-0009" => 450,
                    other => panic!("unexpected placed client order id {other}"),
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
        frames,
    )
    .await
    .unwrap();

    assert_eq!(report.seventh_continued_steps, 1);
    assert_eq!(report.eighth_continued_steps, 1);
    assert_eq!(report.ninth_continued_steps, 0);
    assert_eq!(report.tenth_continued_steps, 0);
    assert_eq!(report.eleventh_continued_steps, 0);
    assert_eq!(report.twelfth_continued_steps, 0);
    assert_eq!(report.thirteenth_continued_steps, 0);
    assert_eq!(report.fourteenth_continued_steps, 0);
    assert_eq!(report.placed_orders, 10);
    assert_eq!(report.reconciliations.len(), 10);
    assert_eq!(
        report.reconciliations[9].client_order_id,
        "unit-usdm-live11-usdm-0009"
    );
    assert_eq!(report.reconciliations[9].side, "SELL");
    assert!(report.reconciliations[9].terminal);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: -1,
        }]
    );
}

#[tokio::test]
async fn policy_demo_live_source_waits_after_ninth_continued_tape_place() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.client_order_id_prefix = "unit-spot-live12".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    config.seventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"216.00","qty":"1.0"}],"asks":[{"price":"218.00","qty":"1.0"}],"update_id":15}"#
            .into(),
    );
    config.eighth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"217.00","qty":"1.0"}],"asks":[{"price":"219.00","qty":"1.0"}],"update_id":16}"#
            .into(),
    );
    config.ninth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"218.00","qty":"1.0"}],"asks":[{"price":"220.00","qty":"1.0"}],"update_id":17}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
    ]);
    let frames = policy_demo_user_data_messages_from_json(&format!(
        "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
        spot_execution_report_json("ETHUSDT", "unit-spot-live12-spot-0000", 461, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live12-spot-0001",
            462,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live12-spot-0002", 463, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live12-spot-0003",
            464,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live12-spot-0004", 465, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live12-spot-0005",
            466,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live12-spot-0006", 467, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live12-spot-0007",
            468,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live12-spot-0008", 469, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live12-spot-0009",
            470,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live12-spot-0010", 471, "BUY", "FILLED"),
    ))
    .unwrap();

    let report = run_spot_policy_demo_with_live_user_data_source(
        config,
        &policy,
        "live-source-ninth-continue-place",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-spot-live12-spot-0000" => 461,
                    "unit-spot-live12-spot-0001" => 462,
                    "unit-spot-live12-spot-0002" => 463,
                    "unit-spot-live12-spot-0003" => 464,
                    "unit-spot-live12-spot-0004" => 465,
                    "unit-spot-live12-spot-0005" => 466,
                    "unit-spot-live12-spot-0006" => 467,
                    "unit-spot-live12-spot-0007" => 468,
                    "unit-spot-live12-spot-0008" => 469,
                    "unit-spot-live12-spot-0009" => 470,
                    "unit-spot-live12-spot-0010" => 471,
                    other => panic!("unexpected placed client order id {other}"),
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
        frames,
    )
    .await
    .unwrap();

    assert_eq!(report.eighth_continued_steps, 1);
    assert_eq!(report.ninth_continued_steps, 1);
    assert_eq!(report.placed_orders, 11);
    assert_eq!(report.receipts.len(), 11);
    assert_eq!(report.reconciliations.len(), 11);
    assert_eq!(
        report.reconciliations[0].client_order_id,
        "unit-spot-live12-spot-0000"
    );
    assert_eq!(
        report.reconciliations[9].client_order_id,
        "unit-spot-live12-spot-0009"
    );
    assert_eq!(
        report.reconciliations[10].client_order_id,
        "unit-spot-live12-spot-0010"
    );
    assert_eq!(report.reconciliations[10].side, "BUY");
    assert!(report.reconciliations[10].terminal);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: 1,
        }]
    );
}

#[tokio::test]
async fn policy_demo_usdm_live_source_waits_after_ninth_continued_tape_place() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Usdm, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.client_order_id_prefix = "unit-usdm-live12".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    config.seventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"216.00","qty":"1.0"}],"asks":[{"price":"218.00","qty":"1.0"}],"update_id":15}"#
            .into(),
    );
    config.eighth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"217.00","qty":"1.0"}],"asks":[{"price":"219.00","qty":"1.0"}],"update_id":16}"#
            .into(),
    );
    config.ninth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"218.00","qty":"1.0"}],"asks":[{"price":"220.00","qty":"1.0"}],"update_id":17}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
    ]);
    let frames = policy_demo_user_data_messages_from_json(
        &serde_json::json!([
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live12-usdm-0000",
                471,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live12-usdm-0001",
                472,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live12-usdm-0002",
                473,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live12-usdm-0003",
                474,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live12-usdm-0004",
                475,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live12-usdm-0005",
                476,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live12-usdm-0006",
                477,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live12-usdm-0007",
                478,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live12-usdm-0008",
                479,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live12-usdm-0009",
                480,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live12-usdm-0010",
                481,
                "BUY",
                "FILLED"
            )
        ])
        .to_string(),
    )
    .unwrap();

    let report = run_usdm_policy_demo_with_live_user_data_source(
        config,
        &policy,
        "live-source-ninth-continue-place-usdm",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(UsdmPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-usdm-live12-usdm-0000" => 471,
                    "unit-usdm-live12-usdm-0001" => 472,
                    "unit-usdm-live12-usdm-0002" => 473,
                    "unit-usdm-live12-usdm-0003" => 474,
                    "unit-usdm-live12-usdm-0004" => 475,
                    "unit-usdm-live12-usdm-0005" => 476,
                    "unit-usdm-live12-usdm-0006" => 477,
                    "unit-usdm-live12-usdm-0007" => 478,
                    "unit-usdm-live12-usdm-0008" => 479,
                    "unit-usdm-live12-usdm-0009" => 480,
                    "unit-usdm-live12-usdm-0010" => 481,
                    other => panic!("unexpected placed client order id {other}"),
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
        frames,
    )
    .await
    .unwrap();

    assert_eq!(report.eighth_continued_steps, 1);
    assert_eq!(report.ninth_continued_steps, 1);
    assert_eq!(report.placed_orders, 11);
    assert_eq!(report.reconciliations.len(), 11);
    assert_eq!(
        report.reconciliations[10].client_order_id,
        "unit-usdm-live12-usdm-0010"
    );
    assert_eq!(report.reconciliations[10].side, "BUY");
    assert!(report.reconciliations[10].terminal);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: 1,
        }]
    );
}

#[tokio::test]
async fn policy_demo_live_source_waits_after_continued_tape_place() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.client_order_id_prefix = "unit-spot-live2".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    let policy =
        SequencePolicy::with_actions(vec![Action::Hold, Action::Buy, Action::Sell, Action::Buy]);
    let frames = policy_demo_user_data_messages_from_json(&format!(
        "{}\n{}\n{}",
        spot_execution_report_json("ETHUSDT", "unit-spot-live2-spot-0000", 81, "BUY", "FILLED"),
        spot_execution_report_json("ETHUSDT", "unit-spot-live2-spot-0001", 82, "SELL", "FILLED"),
        spot_execution_report_json("ETHUSDT", "unit-spot-live2-spot-0002", 83, "BUY", "FILLED"),
    ))
    .unwrap();

    let report = run_spot_policy_demo_with_live_user_data_source(
        config,
        &policy,
        "live-source-continue",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-spot-live2-spot-0000" => 81,
                    "unit-spot-live2-spot-0001" => 82,
                    "unit-spot-live2-spot-0002" => 83,
                    other => panic!("unexpected client order id {other}"),
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
        frames,
    )
    .await
    .unwrap();

    assert_eq!(report.placed_orders, 3);
    assert_eq!(report.receipts.len(), 3);
    assert_eq!(report.reconciliations.len(), 3);
    assert_eq!(
        report.reconciliations[0].client_order_id,
        "unit-spot-live2-spot-0000"
    );
    assert_eq!(
        report.reconciliations[2].client_order_id,
        "unit-spot-live2-spot-0002"
    );
    assert!(report.reconciliations[2].terminal);
    assert_eq!(report.reconciliations[2].side, "BUY");
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: 1,
        }]
    );
    assert_eq!(report.second_continued_steps, 0);
    assert!(report.second_continued_observation.is_empty());
    assert_eq!(report.third_continued_steps, 0);
    assert!(report.third_continued_observation.is_empty());
    assert_eq!(report.fourth_continued_steps, 0);
    assert!(report.fourth_continued_observation.is_empty());
    assert_eq!(report.fifth_continued_steps, 0);
    assert!(report.fifth_continued_observation.is_empty());
    assert_eq!(report.sixth_continued_steps, 0);
    assert!(report.sixth_continued_observation.is_empty());
    assert_eq!(report.seventh_continued_steps, 0);
    assert_eq!(report.eighth_continued_steps, 0);
    assert_eq!(report.ninth_continued_steps, 0);
    assert_eq!(report.tenth_continued_steps, 0);
    assert_eq!(report.eleventh_continued_steps, 0);
    assert_eq!(report.twelfth_continued_steps, 0);
    assert_eq!(report.thirteenth_continued_steps, 0);
    assert_eq!(report.fourteenth_continued_steps, 0);
    assert!(report.seventh_continued_observation.is_empty());
    assert!(report.eighth_continued_observation.is_empty());
    assert!(report.ninth_continued_observation.is_empty());
    assert!(report.tenth_continued_observation.is_empty());
    assert!(report.eleventh_continued_observation.is_empty());
    assert!(report.twelfth_continued_observation.is_empty());
    assert!(report.thirteenth_continued_observation.is_empty());
    assert!(report.fourteenth_continued_observation.is_empty());
}

#[tokio::test]
async fn policy_demo_usdm_live_source_waits_after_continued_tape_place() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Usdm, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.client_order_id_prefix = "unit-usdm-live2".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    let policy =
        SequencePolicy::with_actions(vec![Action::Hold, Action::Buy, Action::Sell, Action::Buy]);
    let frames = policy_demo_user_data_messages_from_json(
        &serde_json::json!([
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live2-usdm-0000",
                91,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live2-usdm-0001",
                92,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live2-usdm-0002",
                93,
                "BUY",
                "FILLED"
            )
        ])
        .to_string(),
    )
    .unwrap();

    let report = run_usdm_policy_demo_with_live_user_data_source(
        config,
        &policy,
        "live-source-continue-usdm",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(UsdmPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-usdm-live2-usdm-0000" => 91,
                    "unit-usdm-live2-usdm-0001" => 92,
                    "unit-usdm-live2-usdm-0002" => 93,
                    other => panic!("unexpected client order id {other}"),
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
        frames,
    )
    .await
    .unwrap();

    assert_eq!(report.placed_orders, 3);
    assert_eq!(report.reconciliations.len(), 3);
    assert_eq!(
        report.reconciliations[0].client_order_id,
        "unit-usdm-live2-usdm-0000"
    );
    assert_eq!(
        report.reconciliations[2].client_order_id,
        "unit-usdm-live2-usdm-0002"
    );
    assert!(report.reconciliations[2].terminal);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: 1,
        }]
    );
    assert_eq!(report.second_continued_steps, 0);
    assert!(report.second_continued_observation.is_empty());
    assert_eq!(report.third_continued_steps, 0);
    assert!(report.third_continued_observation.is_empty());
    assert_eq!(report.fourth_continued_steps, 0);
    assert!(report.fourth_continued_observation.is_empty());
    assert_eq!(report.fifth_continued_steps, 0);
    assert!(report.fifth_continued_observation.is_empty());
    assert_eq!(report.sixth_continued_steps, 0);
    assert!(report.sixth_continued_observation.is_empty());
    assert_eq!(report.seventh_continued_steps, 0);
    assert_eq!(report.eighth_continued_steps, 0);
    assert_eq!(report.ninth_continued_steps, 0);
    assert_eq!(report.tenth_continued_steps, 0);
    assert_eq!(report.eleventh_continued_steps, 0);
    assert_eq!(report.twelfth_continued_steps, 0);
    assert_eq!(report.thirteenth_continued_steps, 0);
    assert_eq!(report.fourteenth_continued_steps, 0);
    assert!(report.seventh_continued_observation.is_empty());
    assert!(report.eighth_continued_observation.is_empty());
    assert!(report.ninth_continued_observation.is_empty());
    assert!(report.tenth_continued_observation.is_empty());
    assert!(report.eleventh_continued_observation.is_empty());
    assert!(report.twelfth_continued_observation.is_empty());
    assert!(report.thirteenth_continued_observation.is_empty());
    assert!(report.fourteenth_continued_observation.is_empty());
}

#[tokio::test]
async fn policy_demo_live_source_continues_after_second_wait_fills() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.use_ladder_observation = true;
    config.client_order_id_prefix = "unit-spot-live3".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    let policy =
        SequencePolicy::with_actions(vec![Action::Hold, Action::Buy, Action::Sell, Action::Buy]);
    let frames = policy_demo_user_data_messages_from_json(&format!(
        "{}\n{}\n{}",
        spot_execution_report_json("ETHUSDT", "unit-spot-live3-spot-0000", 111, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live3-spot-0001",
            112,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live3-spot-0002", 113, "BUY", "FILLED"),
    ))
    .unwrap();

    let report = run_spot_policy_demo_with_live_user_data_source(
        config,
        &policy,
        "live-source-second-continue",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-spot-live3-spot-0000" => 111,
                    "unit-spot-live3-spot-0001" => 112,
                    "unit-spot-live3-spot-0002" => 113,
                    other => panic!("unexpected client order id {other}"),
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
        frames,
    )
    .await
    .unwrap();

    assert_eq!(report.continued_steps, 1);
    assert_eq!(report.second_continued_steps, 1);
    assert_eq!(report.placed_orders, 3);
    assert_eq!(report.order_count(), 3);
    assert_eq!(report.reconciliations.len(), 3);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: 1,
        }]
    );
    assert_eq!(report.second_continued_observation.len(), 80);
    assert!(
        (report.second_continued_observation[44] - 1.0).abs() < 1e-5,
        "second-hop fill-backed eth q {}",
        report.second_continued_observation[44]
    );
    assert_eq!(report.third_continued_steps, 0);
    assert!(report.third_continued_observation.is_empty());
    assert_eq!(report.fourth_continued_steps, 0);
    assert!(report.fourth_continued_observation.is_empty());
    assert_eq!(report.fifth_continued_steps, 0);
    assert!(report.fifth_continued_observation.is_empty());
    assert_eq!(report.sixth_continued_steps, 0);
    assert!(report.sixth_continued_observation.is_empty());
    assert_eq!(report.seventh_continued_steps, 0);
    assert_eq!(report.eighth_continued_steps, 0);
    assert_eq!(report.ninth_continued_steps, 0);
    assert_eq!(report.tenth_continued_steps, 0);
    assert_eq!(report.eleventh_continued_steps, 0);
    assert_eq!(report.twelfth_continued_steps, 0);
    assert_eq!(report.thirteenth_continued_steps, 0);
    assert_eq!(report.fourteenth_continued_steps, 0);
    assert!(report.seventh_continued_observation.is_empty());
    assert!(report.eighth_continued_observation.is_empty());
    assert!(report.ninth_continued_observation.is_empty());
    assert!(report.tenth_continued_observation.is_empty());
    assert!(report.eleventh_continued_observation.is_empty());
    assert!(report.twelfth_continued_observation.is_empty());
    assert!(report.thirteenth_continued_observation.is_empty());
    assert!(report.fourteenth_continued_observation.is_empty());
}

#[tokio::test]
async fn policy_demo_wait_user_data_continues_after_second_wait_fills() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.use_ladder_observation = true;
    config.client_order_id_prefix = "unit-spot-wait3".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    let policy =
        SequencePolicy::with_actions(vec![Action::Hold, Action::Buy, Action::Sell, Action::Buy]);
    let waits = Cell::new(0usize);

    let report = run_spot_policy_demo_with_placer_and_user_data(
        config,
        &policy,
        "wait-second-continue",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-spot-wait3-spot-0000" => 121,
                    "unit-spot-wait3-spot-0001" => 122,
                    "unit-spot-wait3-spot-0002" => 123,
                    other => panic!("unexpected client order id {other}"),
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
            waits.set(waits.get() + 1);
            let payload = report
                .receipts
                .iter()
                .map(|receipt| {
                    spot_execution_report_json(
                        &receipt.symbol,
                        &receipt.client_order_id,
                        receipt.order_id,
                        &receipt.side,
                        "FILLED",
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            let messages = policy_demo_user_data_messages_from_json(&payload);
            async move { messages }
        },
    )
    .await
    .unwrap();

    assert_eq!(waits.get(), 2);
    assert_eq!(report.continued_steps, 1);
    assert_eq!(report.second_continued_steps, 1);
    assert_eq!(report.order_count(), 3);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: 1,
        }]
    );
    assert_eq!(report.second_continued_observation.len(), 80);
    assert!(
        (report.second_continued_observation[44] - 1.0).abs() < 1e-5,
        "mock wait second-hop fill-backed eth q {}",
        report.second_continued_observation[44]
    );
    assert_eq!(report.third_continued_steps, 0);
    assert!(report.third_continued_observation.is_empty());
    assert_eq!(report.fourth_continued_steps, 0);
    assert!(report.fourth_continued_observation.is_empty());
    assert_eq!(report.fifth_continued_steps, 0);
    assert!(report.fifth_continued_observation.is_empty());
    assert_eq!(report.sixth_continued_steps, 0);
    assert!(report.sixth_continued_observation.is_empty());
    assert_eq!(report.seventh_continued_steps, 0);
    assert_eq!(report.eighth_continued_steps, 0);
    assert_eq!(report.ninth_continued_steps, 0);
    assert_eq!(report.tenth_continued_steps, 0);
    assert_eq!(report.eleventh_continued_steps, 0);
    assert_eq!(report.twelfth_continued_steps, 0);
    assert_eq!(report.thirteenth_continued_steps, 0);
    assert_eq!(report.fourteenth_continued_steps, 0);
    assert!(report.seventh_continued_observation.is_empty());
    assert!(report.eighth_continued_observation.is_empty());
    assert!(report.ninth_continued_observation.is_empty());
    assert!(report.tenth_continued_observation.is_empty());
    assert!(report.eleventh_continued_observation.is_empty());
    assert!(report.twelfth_continued_observation.is_empty());
    assert!(report.thirteenth_continued_observation.is_empty());
    assert!(report.fourteenth_continued_observation.is_empty());
}

#[tokio::test]
async fn policy_demo_usdm_live_source_continues_after_second_wait_fills() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Usdm, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.use_ladder_observation = true;
    config.client_order_id_prefix = "unit-usdm-live3".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    let policy =
        SequencePolicy::with_actions(vec![Action::Hold, Action::Buy, Action::Sell, Action::Buy]);
    let frames = policy_demo_user_data_messages_from_json(
        &serde_json::json!([
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live3-usdm-0000",
                131,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live3-usdm-0001",
                132,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live3-usdm-0002",
                133,
                "BUY",
                "FILLED"
            )
        ])
        .to_string(),
    )
    .unwrap();

    let report = run_usdm_policy_demo_with_live_user_data_source(
        config,
        &policy,
        "live-source-second-continue-usdm",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(UsdmPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-usdm-live3-usdm-0000" => 131,
                    "unit-usdm-live3-usdm-0001" => 132,
                    "unit-usdm-live3-usdm-0002" => 133,
                    other => panic!("unexpected client order id {other}"),
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
        frames,
    )
    .await
    .unwrap();

    assert_eq!(report.continued_steps, 1);
    assert_eq!(report.second_continued_steps, 1);
    assert_eq!(report.order_count(), 3);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: 1,
        }]
    );
    assert_eq!(report.second_continued_observation.len(), 80);
    assert!(
        (report.second_continued_observation[44] - 1.0).abs() < 1e-5,
        "usdm second-hop fill-backed eth q {}",
        report.second_continued_observation[44]
    );
    assert_eq!(report.third_continued_steps, 0);
    assert!(report.third_continued_observation.is_empty());
    assert_eq!(report.fourth_continued_steps, 0);
    assert!(report.fourth_continued_observation.is_empty());
    assert_eq!(report.fifth_continued_steps, 0);
    assert!(report.fifth_continued_observation.is_empty());
    assert_eq!(report.sixth_continued_steps, 0);
    assert!(report.sixth_continued_observation.is_empty());
    assert_eq!(report.seventh_continued_steps, 0);
    assert_eq!(report.eighth_continued_steps, 0);
    assert_eq!(report.ninth_continued_steps, 0);
    assert_eq!(report.tenth_continued_steps, 0);
    assert_eq!(report.eleventh_continued_steps, 0);
    assert_eq!(report.twelfth_continued_steps, 0);
    assert_eq!(report.thirteenth_continued_steps, 0);
    assert_eq!(report.fourteenth_continued_steps, 0);
    assert!(report.seventh_continued_observation.is_empty());
    assert!(report.eighth_continued_observation.is_empty());
    assert!(report.ninth_continued_observation.is_empty());
    assert!(report.tenth_continued_observation.is_empty());
    assert!(report.eleventh_continued_observation.is_empty());
    assert!(report.twelfth_continued_observation.is_empty());
    assert!(report.thirteenth_continued_observation.is_empty());
    assert!(report.fourteenth_continued_observation.is_empty());
}

#[tokio::test]
async fn policy_demo_live_source_continues_after_third_wait_fills() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.use_ladder_observation = true;
    config.client_order_id_prefix = "unit-spot-live4".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
    ]);
    let frames = policy_demo_user_data_messages_from_json(&format!(
        "{}\n{}\n{}\n{}",
        spot_execution_report_json("ETHUSDT", "unit-spot-live4-spot-0000", 141, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live4-spot-0001",
            142,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live4-spot-0002", 143, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live4-spot-0003",
            144,
            "SELL",
            "FILLED"
        ),
    ))
    .unwrap();

    let report = run_spot_policy_demo_with_live_user_data_source(
        config,
        &policy,
        "live-source-third-continue",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-spot-live4-spot-0000" => 141,
                    "unit-spot-live4-spot-0001" => 142,
                    "unit-spot-live4-spot-0002" => 143,
                    "unit-spot-live4-spot-0003" => 144,
                    other => panic!("unexpected client order id {other}"),
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
        frames,
    )
    .await
    .unwrap();

    assert_eq!(report.continued_steps, 1);
    assert_eq!(report.second_continued_steps, 1);
    assert_eq!(report.third_continued_steps, 1);
    assert_eq!(report.fourth_continued_steps, 0);
    assert!(report.fourth_continued_observation.is_empty());
    assert_eq!(report.fifth_continued_steps, 0);
    assert!(report.fifth_continued_observation.is_empty());
    assert_eq!(report.sixth_continued_steps, 0);
    assert!(report.sixth_continued_observation.is_empty());
    assert_eq!(report.seventh_continued_steps, 0);
    assert_eq!(report.eighth_continued_steps, 0);
    assert_eq!(report.ninth_continued_steps, 0);
    assert_eq!(report.tenth_continued_steps, 0);
    assert_eq!(report.eleventh_continued_steps, 0);
    assert_eq!(report.twelfth_continued_steps, 0);
    assert_eq!(report.thirteenth_continued_steps, 0);
    assert_eq!(report.fourteenth_continued_steps, 0);
    assert!(report.seventh_continued_observation.is_empty());
    assert!(report.eighth_continued_observation.is_empty());
    assert!(report.ninth_continued_observation.is_empty());
    assert!(report.tenth_continued_observation.is_empty());
    assert!(report.eleventh_continued_observation.is_empty());
    assert!(report.twelfth_continued_observation.is_empty());
    assert!(report.thirteenth_continued_observation.is_empty());
    assert!(report.fourteenth_continued_observation.is_empty());
    assert_eq!(report.placed_orders, 4);
    assert_eq!(report.order_count(), 4);
    assert_eq!(report.reconciliations.len(), 4);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: -1,
        }]
    );
    assert_eq!(report.third_continued_observation.len(), 80);
    assert!(
        (report.third_continued_observation[44] + 1.0).abs() < 1e-5,
        "third-hop fill-backed eth q {}",
        report.third_continued_observation[44]
    );
}

#[tokio::test]
async fn policy_demo_wait_user_data_continues_after_third_wait_fills() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.use_ladder_observation = true;
    config.client_order_id_prefix = "unit-spot-wait5".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
    ]);
    let waits = Cell::new(0usize);

    let report = run_spot_policy_demo_with_placer_and_user_data(
        config,
        &policy,
        "wait-third-continue",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-spot-wait5-spot-0000" => 151,
                    "unit-spot-wait5-spot-0001" => 152,
                    "unit-spot-wait5-spot-0002" => 153,
                    "unit-spot-wait5-spot-0003" => 154,
                    other => panic!("unexpected client order id {other}"),
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
            waits.set(waits.get() + 1);
            let payload = report
                .receipts
                .iter()
                .map(|receipt| {
                    spot_execution_report_json(
                        &receipt.symbol,
                        &receipt.client_order_id,
                        receipt.order_id,
                        &receipt.side,
                        "FILLED",
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            let messages = policy_demo_user_data_messages_from_json(&payload);
            async move { messages }
        },
    )
    .await
    .unwrap();

    assert_eq!(waits.get(), 3);
    assert_eq!(report.continued_steps, 1);
    assert_eq!(report.second_continued_steps, 1);
    assert_eq!(report.third_continued_steps, 1);
    assert_eq!(report.fourth_continued_steps, 0);
    assert!(report.fourth_continued_observation.is_empty());
    assert_eq!(report.fifth_continued_steps, 0);
    assert!(report.fifth_continued_observation.is_empty());
    assert_eq!(report.sixth_continued_steps, 0);
    assert!(report.sixth_continued_observation.is_empty());
    assert_eq!(report.seventh_continued_steps, 0);
    assert_eq!(report.eighth_continued_steps, 0);
    assert_eq!(report.ninth_continued_steps, 0);
    assert_eq!(report.tenth_continued_steps, 0);
    assert_eq!(report.eleventh_continued_steps, 0);
    assert_eq!(report.twelfth_continued_steps, 0);
    assert_eq!(report.thirteenth_continued_steps, 0);
    assert_eq!(report.fourteenth_continued_steps, 0);
    assert!(report.seventh_continued_observation.is_empty());
    assert!(report.eighth_continued_observation.is_empty());
    assert!(report.ninth_continued_observation.is_empty());
    assert!(report.tenth_continued_observation.is_empty());
    assert!(report.eleventh_continued_observation.is_empty());
    assert!(report.twelfth_continued_observation.is_empty());
    assert!(report.thirteenth_continued_observation.is_empty());
    assert!(report.fourteenth_continued_observation.is_empty());
    assert_eq!(report.order_count(), 4);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: -1,
        }]
    );
    assert_eq!(report.third_continued_observation.len(), 80);
    assert!(
        (report.third_continued_observation[44] + 1.0).abs() < 1e-5,
        "mock wait third-hop fill-backed eth q {}",
        report.third_continued_observation[44]
    );
}

#[tokio::test]
async fn policy_demo_usdm_live_source_continues_after_third_wait_fills() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Usdm, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.use_ladder_observation = true;
    config.client_order_id_prefix = "unit-usdm-live4".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
    ]);
    let frames = policy_demo_user_data_messages_from_json(
        &serde_json::json!([
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live4-usdm-0000",
                161,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live4-usdm-0001",
                162,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live4-usdm-0002",
                163,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live4-usdm-0003",
                164,
                "SELL",
                "FILLED"
            )
        ])
        .to_string(),
    )
    .unwrap();

    let report = run_usdm_policy_demo_with_live_user_data_source(
        config,
        &policy,
        "live-source-third-continue-usdm",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(UsdmPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-usdm-live4-usdm-0000" => 161,
                    "unit-usdm-live4-usdm-0001" => 162,
                    "unit-usdm-live4-usdm-0002" => 163,
                    "unit-usdm-live4-usdm-0003" => 164,
                    other => panic!("unexpected client order id {other}"),
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
        frames,
    )
    .await
    .unwrap();

    assert_eq!(report.continued_steps, 1);
    assert_eq!(report.second_continued_steps, 1);
    assert_eq!(report.third_continued_steps, 1);
    assert_eq!(report.fourth_continued_steps, 0);
    assert!(report.fourth_continued_observation.is_empty());
    assert_eq!(report.fifth_continued_steps, 0);
    assert!(report.fifth_continued_observation.is_empty());
    assert_eq!(report.sixth_continued_steps, 0);
    assert!(report.sixth_continued_observation.is_empty());
    assert_eq!(report.seventh_continued_steps, 0);
    assert_eq!(report.eighth_continued_steps, 0);
    assert_eq!(report.ninth_continued_steps, 0);
    assert_eq!(report.tenth_continued_steps, 0);
    assert_eq!(report.eleventh_continued_steps, 0);
    assert_eq!(report.twelfth_continued_steps, 0);
    assert_eq!(report.thirteenth_continued_steps, 0);
    assert_eq!(report.fourteenth_continued_steps, 0);
    assert!(report.seventh_continued_observation.is_empty());
    assert!(report.eighth_continued_observation.is_empty());
    assert!(report.ninth_continued_observation.is_empty());
    assert!(report.tenth_continued_observation.is_empty());
    assert!(report.eleventh_continued_observation.is_empty());
    assert!(report.twelfth_continued_observation.is_empty());
    assert!(report.thirteenth_continued_observation.is_empty());
    assert!(report.fourteenth_continued_observation.is_empty());
    assert_eq!(report.order_count(), 4);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: -1,
        }]
    );
    assert_eq!(report.third_continued_observation.len(), 80);
    assert!(
        (report.third_continued_observation[44] + 1.0).abs() < 1e-5,
        "usdm third-hop fill-backed eth q {}",
        report.third_continued_observation[44]
    );
}

#[tokio::test]
async fn policy_demo_live_source_continues_after_fourth_wait_fills() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.use_ladder_observation = true;
    config.client_order_id_prefix = "unit-spot-live5".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
    ]);
    let frames = policy_demo_user_data_messages_from_json(&format!(
        "{}\n{}\n{}\n{}\n{}",
        spot_execution_report_json("ETHUSDT", "unit-spot-live5-spot-0000", 171, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live5-spot-0001",
            172,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live5-spot-0002", 173, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live5-spot-0003",
            174,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live5-spot-0004", 175, "BUY", "FILLED"),
    ))
    .unwrap();

    let report = run_spot_policy_demo_with_live_user_data_source(
        config,
        &policy,
        "live-source-fourth-continue",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-spot-live5-spot-0000" => 171,
                    "unit-spot-live5-spot-0001" => 172,
                    "unit-spot-live5-spot-0002" => 173,
                    "unit-spot-live5-spot-0003" => 174,
                    "unit-spot-live5-spot-0004" => 175,
                    other => panic!("unexpected client order id {other}"),
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
        frames,
    )
    .await
    .unwrap();

    assert_eq!(report.continued_steps, 1);
    assert_eq!(report.second_continued_steps, 1);
    assert_eq!(report.third_continued_steps, 1);
    assert_eq!(report.fourth_continued_steps, 1);
    assert_eq!(report.placed_orders, 5);
    assert_eq!(report.order_count(), 5);
    assert_eq!(report.reconciliations.len(), 5);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: 1,
        }]
    );
    assert_eq!(report.fourth_continued_observation.len(), 80);
    assert!(
        (report.fourth_continued_observation[44] - 1.0).abs() < 1e-5,
        "fourth-hop fill-backed eth q {}",
        report.fourth_continued_observation[44]
    );
}

#[tokio::test]
async fn policy_demo_wait_user_data_continues_after_fourth_wait_fills() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.use_ladder_observation = true;
    config.client_order_id_prefix = "unit-spot-wait6".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
    ]);
    let waits = Cell::new(0usize);

    let report = run_spot_policy_demo_with_placer_and_user_data(
        config,
        &policy,
        "wait-fourth-continue",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-spot-wait6-spot-0000" => 181,
                    "unit-spot-wait6-spot-0001" => 182,
                    "unit-spot-wait6-spot-0002" => 183,
                    "unit-spot-wait6-spot-0003" => 184,
                    "unit-spot-wait6-spot-0004" => 185,
                    other => panic!("unexpected client order id {other}"),
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
            waits.set(waits.get() + 1);
            let payload = report
                .receipts
                .iter()
                .map(|receipt| {
                    spot_execution_report_json(
                        &receipt.symbol,
                        &receipt.client_order_id,
                        receipt.order_id,
                        &receipt.side,
                        "FILLED",
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            let messages = policy_demo_user_data_messages_from_json(&payload);
            async move { messages }
        },
    )
    .await
    .unwrap();

    assert_eq!(waits.get(), 4);
    assert_eq!(report.continued_steps, 1);
    assert_eq!(report.second_continued_steps, 1);
    assert_eq!(report.third_continued_steps, 1);
    assert_eq!(report.fourth_continued_steps, 1);
    assert_eq!(report.order_count(), 5);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: 1,
        }]
    );
    assert_eq!(report.fourth_continued_observation.len(), 80);
    assert!(
        (report.fourth_continued_observation[44] - 1.0).abs() < 1e-5,
        "mock wait fourth-hop fill-backed eth q {}",
        report.fourth_continued_observation[44]
    );
}

#[tokio::test]
async fn policy_demo_usdm_live_source_continues_after_fourth_wait_fills() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Usdm, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.use_ladder_observation = true;
    config.client_order_id_prefix = "unit-usdm-live5".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
    ]);
    let frames = policy_demo_user_data_messages_from_json(
        &serde_json::json!([
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live5-usdm-0000",
                191,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live5-usdm-0001",
                192,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live5-usdm-0002",
                193,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live5-usdm-0003",
                194,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live5-usdm-0004",
                195,
                "BUY",
                "FILLED"
            )
        ])
        .to_string(),
    )
    .unwrap();

    let report = run_usdm_policy_demo_with_live_user_data_source(
        config,
        &policy,
        "live-source-fourth-continue-usdm",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(UsdmPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-usdm-live5-usdm-0000" => 191,
                    "unit-usdm-live5-usdm-0001" => 192,
                    "unit-usdm-live5-usdm-0002" => 193,
                    "unit-usdm-live5-usdm-0003" => 194,
                    "unit-usdm-live5-usdm-0004" => 195,
                    other => panic!("unexpected client order id {other}"),
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
        frames,
    )
    .await
    .unwrap();

    assert_eq!(report.continued_steps, 1);
    assert_eq!(report.second_continued_steps, 1);
    assert_eq!(report.third_continued_steps, 1);
    assert_eq!(report.fourth_continued_steps, 1);
    assert_eq!(report.order_count(), 5);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: 1,
        }]
    );
    assert_eq!(report.fourth_continued_observation.len(), 80);
    assert!(
        (report.fourth_continued_observation[44] - 1.0).abs() < 1e-5,
        "usdm fourth-hop fill-backed eth q {}",
        report.fourth_continued_observation[44]
    );
}

#[tokio::test]
async fn policy_demo_second_continued_tape_drains_dispatch_orders() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = false;
    config.client_order_id_prefix = "unit-spot-hop3".into();
    config.captured_user_data_json = Some(format!(
        "{}\n{}",
        spot_execution_report_json("ETHUSDT", "unit-spot-hop3-spot-0000", 91, "BUY", "FILLED"),
        spot_execution_report_json("ETHUSDT", "unit-spot-hop3-spot-0001", 92, "SELL", "FILLED"),
    ));
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop3-spot-0002",
        93,
        "BUY",
        "FILLED",
    ));
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
    ]);

    let report =
        run_spot_policy_demo_with_placer(config, &policy, "second-continue-drain", |_| async {
            panic!("dry-run must not place")
        })
        .await
        .unwrap();

    assert_eq!(report.steps, 3);
    assert_eq!(report.continued_steps, 1);
    assert_eq!(report.second_continued_steps, 1);
    assert_eq!(report.placed_orders, 0);
    assert_eq!(report.order_count(), 4);
    let PolicyDemoOrders::Spot(orders) = &report.orders else {
        panic!("expected spot orders");
    };
    assert_eq!(orders[3].symbol, "ETHUSDT");
    assert_eq!(orders[3].quantity, "0.03");
    assert_eq!(orders[3].side, SpotOrderSide::Sell);
    assert_eq!(
        orders[3].new_client_order_id.as_deref(),
        Some("unit-spot-hop3-spot-0003")
    );
}

#[tokio::test]
async fn policy_demo_second_continued_tape_places_orders_when_guarded() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.client_order_id_prefix = "unit-spot-hop3p".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
    ]);
    let frames = policy_demo_user_data_messages_from_json(&format!(
        "{}\n{}\n{}",
        spot_execution_report_json("ETHUSDT", "unit-spot-hop3p-spot-0000", 141, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop3p-spot-0001",
            142,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-hop3p-spot-0002", 143, "BUY", "FILLED"),
    ))
    .unwrap();

    let report = run_spot_policy_demo_with_live_user_data_source(
        config,
        &policy,
        "second-continue-place",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-spot-hop3p-spot-0000" => 141,
                    "unit-spot-hop3p-spot-0001" => 142,
                    "unit-spot-hop3p-spot-0002" => 143,
                    "unit-spot-hop3p-spot-0003" => 144,
                    other => panic!("unexpected placed client order id {other}"),
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
        frames,
    )
    .await
    .unwrap();

    assert_eq!(report.second_continued_steps, 1);
    assert_eq!(report.placed_orders, 4);
    assert_eq!(report.order_count(), 4);
    assert_eq!(report.receipts.len(), 4);
    assert_eq!(
        report.receipts[0].client_order_id,
        "unit-spot-hop3p-spot-0000"
    );
    assert_eq!(
        report.receipts[3].client_order_id,
        "unit-spot-hop3p-spot-0003"
    );
    assert_eq!(report.receipts[3].side, "SELL");
    let PolicyDemoOrders::Spot(orders) = &report.orders else {
        panic!("expected spot orders");
    };
    assert_eq!(
        orders[3].new_client_order_id.as_deref(),
        Some("unit-spot-hop3p-spot-0003")
    );
    assert_eq!(orders[3].side, SpotOrderSide::Sell);
}

#[tokio::test]
async fn policy_demo_third_continued_tape_drains_dispatch_orders() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = false;
    config.client_order_id_prefix = "unit-spot-hop4".into();
    config.captured_user_data_json = Some(format!(
        "{}\n{}",
        spot_execution_report_json("ETHUSDT", "unit-spot-hop4-spot-0000", 171, "BUY", "FILLED"),
        spot_execution_report_json("ETHUSDT", "unit-spot-hop4-spot-0001", 172, "SELL", "FILLED"),
    ));
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop4-spot-0002",
        173,
        "BUY",
        "FILLED",
    ));
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.second_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop4-spot-0003",
        174,
        "SELL",
        "FILLED",
    ));
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
    ]);

    let report =
        run_spot_policy_demo_with_placer(config, &policy, "third-continue-drain", |_| async {
            panic!("dry-run must not place")
        })
        .await
        .unwrap();

    assert_eq!(report.steps, 3);
    assert_eq!(report.continued_steps, 1);
    assert_eq!(report.second_continued_steps, 1);
    assert_eq!(report.third_continued_steps, 1);
    assert_eq!(report.placed_orders, 0);
    assert_eq!(report.order_count(), 5);
    let PolicyDemoOrders::Spot(orders) = &report.orders else {
        panic!("expected spot orders");
    };
    assert_eq!(orders[4].symbol, "ETHUSDT");
    assert_eq!(orders[4].quantity, "0.03");
    assert_eq!(orders[4].side, SpotOrderSide::Buy);
    assert_eq!(
        orders[4].new_client_order_id.as_deref(),
        Some("unit-spot-hop4-spot-0004")
    );
}

#[tokio::test]
async fn policy_demo_fourth_continued_tape_drains_dispatch_orders() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = false;
    config.client_order_id_prefix = "unit-spot-hop5".into();
    config.captured_user_data_json = Some(format!(
        "{}\n{}",
        spot_execution_report_json("ETHUSDT", "unit-spot-hop5-spot-0000", 191, "BUY", "FILLED"),
        spot_execution_report_json("ETHUSDT", "unit-spot-hop5-spot-0001", 192, "SELL", "FILLED"),
    ));
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop5-spot-0002",
        193,
        "BUY",
        "FILLED",
    ));
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.second_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop5-spot-0003",
        194,
        "SELL",
        "FILLED",
    ));
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.third_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop5-spot-0004",
        195,
        "BUY",
        "FILLED",
    ));
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
    ]);

    let report =
        run_spot_policy_demo_with_placer(config, &policy, "fourth-continue-drain", |_| async {
            panic!("dry-run must not place")
        })
        .await
        .unwrap();

    assert_eq!(report.steps, 3);
    assert_eq!(report.continued_steps, 1);
    assert_eq!(report.second_continued_steps, 1);
    assert_eq!(report.third_continued_steps, 1);
    assert_eq!(report.fourth_continued_steps, 1);
    assert_eq!(report.placed_orders, 0);
    assert_eq!(report.order_count(), 6);
    let PolicyDemoOrders::Spot(orders) = &report.orders else {
        panic!("expected spot orders");
    };
    assert_eq!(orders[5].symbol, "ETHUSDT");
    assert_eq!(orders[5].quantity, "0.03");
    assert_eq!(orders[5].side, SpotOrderSide::Sell);
    assert_eq!(
        orders[5].new_client_order_id.as_deref(),
        Some("unit-spot-hop5-spot-0005")
    );
}

#[tokio::test]
async fn policy_demo_third_continued_tape_places_orders_when_guarded() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.client_order_id_prefix = "unit-spot-hop4p".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
    ]);
    let frames = policy_demo_user_data_messages_from_json(&format!(
        "{}\n{}\n{}\n{}",
        spot_execution_report_json("ETHUSDT", "unit-spot-hop4p-spot-0000", 181, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop4p-spot-0001",
            182,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-hop4p-spot-0002", 183, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop4p-spot-0003",
            184,
            "SELL",
            "FILLED"
        ),
    ))
    .unwrap();

    let report = run_spot_policy_demo_with_live_user_data_source(
        config,
        &policy,
        "third-continue-place",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-spot-hop4p-spot-0000" => 181,
                    "unit-spot-hop4p-spot-0001" => 182,
                    "unit-spot-hop4p-spot-0002" => 183,
                    "unit-spot-hop4p-spot-0003" => 184,
                    "unit-spot-hop4p-spot-0004" => 185,
                    other => panic!("unexpected placed client order id {other}"),
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
        frames,
    )
    .await
    .unwrap();

    assert_eq!(report.third_continued_steps, 1);
    assert_eq!(report.placed_orders, 5);
    assert_eq!(report.order_count(), 5);
    assert_eq!(report.receipts.len(), 5);
    assert_eq!(
        report.receipts[0].client_order_id,
        "unit-spot-hop4p-spot-0000"
    );
    assert_eq!(
        report.receipts[4].client_order_id,
        "unit-spot-hop4p-spot-0004"
    );
    assert_eq!(report.receipts[4].side, "BUY");
    let PolicyDemoOrders::Spot(orders) = &report.orders else {
        panic!("expected spot orders");
    };
    assert_eq!(
        orders[4].new_client_order_id.as_deref(),
        Some("unit-spot-hop4p-spot-0004")
    );
    assert_eq!(orders[4].side, SpotOrderSide::Buy);
}

#[tokio::test]
async fn policy_demo_fourth_continued_tape_places_orders_when_guarded() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.client_order_id_prefix = "unit-spot-hop5p".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
    ]);
    let frames = policy_demo_user_data_messages_from_json(&format!(
        "{}\n{}\n{}\n{}\n{}",
        spot_execution_report_json("ETHUSDT", "unit-spot-hop5p-spot-0000", 201, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop5p-spot-0001",
            202,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-hop5p-spot-0002", 203, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop5p-spot-0003",
            204,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-hop5p-spot-0004", 205, "BUY", "FILLED"),
    ))
    .unwrap();

    let report = run_spot_policy_demo_with_live_user_data_source(
        config,
        &policy,
        "fourth-continue-place",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-spot-hop5p-spot-0000" => 201,
                    "unit-spot-hop5p-spot-0001" => 202,
                    "unit-spot-hop5p-spot-0002" => 203,
                    "unit-spot-hop5p-spot-0003" => 204,
                    "unit-spot-hop5p-spot-0004" => 205,
                    "unit-spot-hop5p-spot-0005" => 206,
                    other => panic!("unexpected placed client order id {other}"),
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
        frames,
    )
    .await
    .unwrap();

    assert_eq!(report.fourth_continued_steps, 1);
    assert_eq!(report.placed_orders, 6);
    assert_eq!(report.order_count(), 6);
    assert_eq!(report.receipts.len(), 6);
    assert_eq!(
        report.receipts[0].client_order_id,
        "unit-spot-hop5p-spot-0000"
    );
    assert_eq!(
        report.receipts[5].client_order_id,
        "unit-spot-hop5p-spot-0005"
    );
    assert_eq!(report.receipts[5].side, "SELL");
    let PolicyDemoOrders::Spot(orders) = &report.orders else {
        panic!("expected spot orders");
    };
    assert_eq!(
        orders[5].new_client_order_id.as_deref(),
        Some("unit-spot-hop5p-spot-0005")
    );
    assert_eq!(orders[5].side, SpotOrderSide::Sell);
}

#[tokio::test]
async fn policy_demo_live_source_continues_after_fifth_wait_fills() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.use_ladder_observation = true;
    config.client_order_id_prefix = "unit-spot-live6".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
    ]);
    let frames = policy_demo_user_data_messages_from_json(&format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        spot_execution_report_json("ETHUSDT", "unit-spot-live6-spot-0000", 211, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live6-spot-0001",
            212,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live6-spot-0002", 213, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live6-spot-0003",
            214,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live6-spot-0004", 215, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live6-spot-0005",
            216,
            "SELL",
            "FILLED"
        ),
    ))
    .unwrap();

    let report = run_spot_policy_demo_with_live_user_data_source(
        config,
        &policy,
        "live-source-fifth-continue",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-spot-live6-spot-0000" => 211,
                    "unit-spot-live6-spot-0001" => 212,
                    "unit-spot-live6-spot-0002" => 213,
                    "unit-spot-live6-spot-0003" => 214,
                    "unit-spot-live6-spot-0004" => 215,
                    "unit-spot-live6-spot-0005" => 216,
                    other => panic!("unexpected client order id {other}"),
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
        frames,
    )
    .await
    .unwrap();

    assert_eq!(report.continued_steps, 1);
    assert_eq!(report.second_continued_steps, 1);
    assert_eq!(report.third_continued_steps, 1);
    assert_eq!(report.fourth_continued_steps, 1);
    assert_eq!(report.fifth_continued_steps, 1);
    assert_eq!(report.placed_orders, 6);
    assert_eq!(report.order_count(), 6);
    assert_eq!(report.reconciliations.len(), 6);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: -1,
        }]
    );
    assert_eq!(report.fifth_continued_observation.len(), 80);
    assert!(
        (report.fifth_continued_observation[44] + 1.0).abs() < 1e-5,
        "fifth-hop fill-backed eth q {}",
        report.fifth_continued_observation[44]
    );
}

#[tokio::test]
async fn policy_demo_wait_user_data_continues_after_fifth_wait_fills() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.use_ladder_observation = true;
    config.client_order_id_prefix = "unit-spot-wait7".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
    ]);
    let waits = Cell::new(0usize);

    let report = run_spot_policy_demo_with_placer_and_user_data(
        config,
        &policy,
        "wait-fifth-continue",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-spot-wait7-spot-0000" => 221,
                    "unit-spot-wait7-spot-0001" => 222,
                    "unit-spot-wait7-spot-0002" => 223,
                    "unit-spot-wait7-spot-0003" => 224,
                    "unit-spot-wait7-spot-0004" => 225,
                    "unit-spot-wait7-spot-0005" => 226,
                    other => panic!("unexpected client order id {other}"),
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
            waits.set(waits.get() + 1);
            let payload = report
                .receipts
                .iter()
                .map(|receipt| {
                    spot_execution_report_json(
                        &receipt.symbol,
                        &receipt.client_order_id,
                        receipt.order_id,
                        &receipt.side,
                        "FILLED",
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            let messages = policy_demo_user_data_messages_from_json(&payload);
            async move { messages }
        },
    )
    .await
    .unwrap();

    assert_eq!(waits.get(), 5);
    assert_eq!(report.continued_steps, 1);
    assert_eq!(report.second_continued_steps, 1);
    assert_eq!(report.third_continued_steps, 1);
    assert_eq!(report.fourth_continued_steps, 1);
    assert_eq!(report.fifth_continued_steps, 1);
    assert_eq!(report.order_count(), 6);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: -1,
        }]
    );
    assert_eq!(report.fifth_continued_observation.len(), 80);
    assert!(
        (report.fifth_continued_observation[44] + 1.0).abs() < 1e-5,
        "mock wait fifth-hop fill-backed eth q {}",
        report.fifth_continued_observation[44]
    );
}

#[tokio::test]
async fn policy_demo_usdm_live_source_continues_after_fifth_wait_fills() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Usdm, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.use_ladder_observation = true;
    config.client_order_id_prefix = "unit-usdm-live6".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
    ]);
    let frames = policy_demo_user_data_messages_from_json(
        &serde_json::json!([
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live6-usdm-0000",
                231,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live6-usdm-0001",
                232,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live6-usdm-0002",
                233,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live6-usdm-0003",
                234,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live6-usdm-0004",
                235,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live6-usdm-0005",
                236,
                "SELL",
                "FILLED"
            )
        ])
        .to_string(),
    )
    .unwrap();

    let report = run_usdm_policy_demo_with_live_user_data_source(
        config,
        &policy,
        "live-source-fifth-continue-usdm",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(UsdmPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-usdm-live6-usdm-0000" => 231,
                    "unit-usdm-live6-usdm-0001" => 232,
                    "unit-usdm-live6-usdm-0002" => 233,
                    "unit-usdm-live6-usdm-0003" => 234,
                    "unit-usdm-live6-usdm-0004" => 235,
                    "unit-usdm-live6-usdm-0005" => 236,
                    other => panic!("unexpected client order id {other}"),
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
        frames,
    )
    .await
    .unwrap();

    assert_eq!(report.continued_steps, 1);
    assert_eq!(report.second_continued_steps, 1);
    assert_eq!(report.third_continued_steps, 1);
    assert_eq!(report.fourth_continued_steps, 1);
    assert_eq!(report.fifth_continued_steps, 1);
    assert_eq!(report.order_count(), 6);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: -1,
        }]
    );
    assert_eq!(report.fifth_continued_observation.len(), 80);
    assert!(
        (report.fifth_continued_observation[44] + 1.0).abs() < 1e-5,
        "usdm fifth-hop fill-backed eth q {}",
        report.fifth_continued_observation[44]
    );
}

#[tokio::test]
async fn policy_demo_fifth_continued_tape_drains_dispatch_orders() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = false;
    config.client_order_id_prefix = "unit-spot-hop6".into();
    config.captured_user_data_json = Some(format!(
        "{}\n{}",
        spot_execution_report_json("ETHUSDT", "unit-spot-hop6-spot-0000", 241, "BUY", "FILLED"),
        spot_execution_report_json("ETHUSDT", "unit-spot-hop6-spot-0001", 242, "SELL", "FILLED"),
    ));
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop6-spot-0002",
        243,
        "BUY",
        "FILLED",
    ));
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.second_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop6-spot-0003",
        244,
        "SELL",
        "FILLED",
    ));
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.third_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop6-spot-0004",
        245,
        "BUY",
        "FILLED",
    ));
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fourth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop6-spot-0005",
        246,
        "SELL",
        "FILLED",
    ));
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
    ]);

    let report =
        run_spot_policy_demo_with_placer(config, &policy, "fifth-continue-drain", |_| async {
            panic!("dry-run must not place")
        })
        .await
        .unwrap();

    assert_eq!(report.steps, 3);
    assert_eq!(report.continued_steps, 1);
    assert_eq!(report.second_continued_steps, 1);
    assert_eq!(report.third_continued_steps, 1);
    assert_eq!(report.fourth_continued_steps, 1);
    assert_eq!(report.fifth_continued_steps, 1);
    assert_eq!(report.placed_orders, 0);
    assert_eq!(report.order_count(), 7);
    let PolicyDemoOrders::Spot(orders) = &report.orders else {
        panic!("expected spot orders");
    };
    assert_eq!(orders[6].symbol, "ETHUSDT");
    assert_eq!(orders[6].quantity, "0.03");
    assert_eq!(orders[6].side, SpotOrderSide::Buy);
    assert_eq!(
        orders[6].new_client_order_id.as_deref(),
        Some("unit-spot-hop6-spot-0006")
    );
}

#[tokio::test]
async fn policy_demo_fifth_continued_tape_places_orders_when_guarded() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.client_order_id_prefix = "unit-spot-hop6p".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
    ]);
    let frames = policy_demo_user_data_messages_from_json(&format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        spot_execution_report_json("ETHUSDT", "unit-spot-hop6p-spot-0000", 251, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop6p-spot-0001",
            252,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-hop6p-spot-0002", 253, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop6p-spot-0003",
            254,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-hop6p-spot-0004", 255, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop6p-spot-0005",
            256,
            "SELL",
            "FILLED"
        ),
    ))
    .unwrap();

    let report = run_spot_policy_demo_with_live_user_data_source(
        config,
        &policy,
        "fifth-continue-place",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-spot-hop6p-spot-0000" => 251,
                    "unit-spot-hop6p-spot-0001" => 252,
                    "unit-spot-hop6p-spot-0002" => 253,
                    "unit-spot-hop6p-spot-0003" => 254,
                    "unit-spot-hop6p-spot-0004" => 255,
                    "unit-spot-hop6p-spot-0005" => 256,
                    "unit-spot-hop6p-spot-0006" => 257,
                    other => panic!("unexpected placed client order id {other}"),
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
        frames,
    )
    .await
    .unwrap();

    assert_eq!(report.fifth_continued_steps, 1);
    assert_eq!(report.placed_orders, 7);
    assert_eq!(report.order_count(), 7);
    assert_eq!(report.receipts.len(), 7);
    assert_eq!(
        report.receipts[0].client_order_id,
        "unit-spot-hop6p-spot-0000"
    );
    assert_eq!(
        report.receipts[6].client_order_id,
        "unit-spot-hop6p-spot-0006"
    );
    assert_eq!(report.receipts[6].side, "BUY");
    let PolicyDemoOrders::Spot(orders) = &report.orders else {
        panic!("expected spot orders");
    };
    assert_eq!(
        orders[6].new_client_order_id.as_deref(),
        Some("unit-spot-hop6p-spot-0006")
    );
    assert_eq!(orders[6].side, SpotOrderSide::Buy);
}

#[tokio::test]
async fn policy_demo_live_source_continues_after_sixth_wait_fills() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.use_ladder_observation = true;
    config.client_order_id_prefix = "unit-spot-live7".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
    ]);
    let frames = policy_demo_user_data_messages_from_json(&format!(
        "{}\n{}\n{}\n{}\n{}\n{}\n{}",
        spot_execution_report_json("ETHUSDT", "unit-spot-live7-spot-0000", 261, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live7-spot-0001",
            262,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live7-spot-0002", 263, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live7-spot-0003",
            264,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live7-spot-0004", 265, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live7-spot-0005",
            266,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live7-spot-0006", 267, "BUY", "FILLED"),
    ))
    .unwrap();

    let report = run_spot_policy_demo_with_live_user_data_source(
        config,
        &policy,
        "live-source-sixth-continue",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-spot-live7-spot-0000" => 261,
                    "unit-spot-live7-spot-0001" => 262,
                    "unit-spot-live7-spot-0002" => 263,
                    "unit-spot-live7-spot-0003" => 264,
                    "unit-spot-live7-spot-0004" => 265,
                    "unit-spot-live7-spot-0005" => 266,
                    "unit-spot-live7-spot-0006" => 267,
                    other => panic!("unexpected client order id {other}"),
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
        frames,
    )
    .await
    .unwrap();

    assert_eq!(report.continued_steps, 1);
    assert_eq!(report.second_continued_steps, 1);
    assert_eq!(report.third_continued_steps, 1);
    assert_eq!(report.fourth_continued_steps, 1);
    assert_eq!(report.fifth_continued_steps, 1);
    assert_eq!(report.sixth_continued_steps, 1);
    assert_eq!(report.seventh_continued_steps, 0);
    assert_eq!(report.eighth_continued_steps, 0);
    assert_eq!(report.ninth_continued_steps, 0);
    assert_eq!(report.tenth_continued_steps, 0);
    assert_eq!(report.eleventh_continued_steps, 0);
    assert_eq!(report.twelfth_continued_steps, 0);
    assert_eq!(report.thirteenth_continued_steps, 0);
    assert_eq!(report.fourteenth_continued_steps, 0);
    assert_eq!(report.placed_orders, 7);
    assert_eq!(report.order_count(), 7);
    assert_eq!(report.reconciliations.len(), 7);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: 1,
        }]
    );
    assert_eq!(report.sixth_continued_observation.len(), 80);
    assert!(
        (report.sixth_continued_observation[44] - 1.0).abs() < 1e-5,
        "sixth-hop fill-backed eth q {}",
        report.sixth_continued_observation[44]
    );
}

#[tokio::test]
async fn policy_demo_wait_user_data_continues_after_sixth_wait_fills() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.use_ladder_observation = true;
    config.client_order_id_prefix = "unit-spot-wait8".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
    ]);
    let waits = Cell::new(0usize);

    let report = run_spot_policy_demo_with_placer_and_user_data(
        config,
        &policy,
        "wait-sixth-continue",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-spot-wait8-spot-0000" => 271,
                    "unit-spot-wait8-spot-0001" => 272,
                    "unit-spot-wait8-spot-0002" => 273,
                    "unit-spot-wait8-spot-0003" => 274,
                    "unit-spot-wait8-spot-0004" => 275,
                    "unit-spot-wait8-spot-0005" => 276,
                    "unit-spot-wait8-spot-0006" => 277,
                    other => panic!("unexpected client order id {other}"),
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
            waits.set(waits.get() + 1);
            let payload = report
                .receipts
                .iter()
                .map(|receipt| {
                    spot_execution_report_json(
                        &receipt.symbol,
                        &receipt.client_order_id,
                        receipt.order_id,
                        &receipt.side,
                        "FILLED",
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            let messages = policy_demo_user_data_messages_from_json(&payload);
            async move { messages }
        },
    )
    .await
    .unwrap();

    assert_eq!(waits.get(), 6);
    assert_eq!(report.continued_steps, 1);
    assert_eq!(report.second_continued_steps, 1);
    assert_eq!(report.third_continued_steps, 1);
    assert_eq!(report.fourth_continued_steps, 1);
    assert_eq!(report.fifth_continued_steps, 1);
    assert_eq!(report.sixth_continued_steps, 1);
    assert_eq!(report.seventh_continued_steps, 0);
    assert_eq!(report.eighth_continued_steps, 0);
    assert_eq!(report.ninth_continued_steps, 0);
    assert_eq!(report.tenth_continued_steps, 0);
    assert_eq!(report.eleventh_continued_steps, 0);
    assert_eq!(report.twelfth_continued_steps, 0);
    assert_eq!(report.thirteenth_continued_steps, 0);
    assert_eq!(report.fourteenth_continued_steps, 0);
    assert_eq!(report.order_count(), 7);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: 1,
        }]
    );
    assert_eq!(report.sixth_continued_observation.len(), 80);
    assert!(
        (report.sixth_continued_observation[44] - 1.0).abs() < 1e-5,
        "mock wait sixth-hop fill-backed eth q {}",
        report.sixth_continued_observation[44]
    );
}

#[tokio::test]
async fn policy_demo_usdm_live_source_continues_after_sixth_wait_fills() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Usdm, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.use_ladder_observation = true;
    config.client_order_id_prefix = "unit-usdm-live7".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
    ]);
    let frames = policy_demo_user_data_messages_from_json(
        &serde_json::json!([
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live7-usdm-0000",
                281,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live7-usdm-0001",
                282,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live7-usdm-0002",
                283,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live7-usdm-0003",
                284,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live7-usdm-0004",
                285,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live7-usdm-0005",
                286,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live7-usdm-0006",
                287,
                "BUY",
                "FILLED"
            )
        ])
        .to_string(),
    )
    .unwrap();

    let report = run_usdm_policy_demo_with_live_user_data_source(
        config,
        &policy,
        "live-source-sixth-continue-usdm",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(UsdmPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-usdm-live7-usdm-0000" => 281,
                    "unit-usdm-live7-usdm-0001" => 282,
                    "unit-usdm-live7-usdm-0002" => 283,
                    "unit-usdm-live7-usdm-0003" => 284,
                    "unit-usdm-live7-usdm-0004" => 285,
                    "unit-usdm-live7-usdm-0005" => 286,
                    "unit-usdm-live7-usdm-0006" => 287,
                    other => panic!("unexpected client order id {other}"),
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
        frames,
    )
    .await
    .unwrap();

    assert_eq!(report.continued_steps, 1);
    assert_eq!(report.second_continued_steps, 1);
    assert_eq!(report.third_continued_steps, 1);
    assert_eq!(report.fourth_continued_steps, 1);
    assert_eq!(report.fifth_continued_steps, 1);
    assert_eq!(report.sixth_continued_steps, 1);
    assert_eq!(report.seventh_continued_steps, 0);
    assert_eq!(report.eighth_continued_steps, 0);
    assert_eq!(report.ninth_continued_steps, 0);
    assert_eq!(report.tenth_continued_steps, 0);
    assert_eq!(report.eleventh_continued_steps, 0);
    assert_eq!(report.twelfth_continued_steps, 0);
    assert_eq!(report.thirteenth_continued_steps, 0);
    assert_eq!(report.fourteenth_continued_steps, 0);
    assert_eq!(report.order_count(), 7);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: 1,
        }]
    );
    assert_eq!(report.sixth_continued_observation.len(), 80);
    assert!(
        (report.sixth_continued_observation[44] - 1.0).abs() < 1e-5,
        "usdm sixth-hop fill-backed eth q {}",
        report.sixth_continued_observation[44]
    );
}

#[tokio::test]
async fn policy_demo_live_source_continues_after_seventh_wait_fills() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.use_ladder_observation = true;
    config.client_order_id_prefix = "unit-spot-live8".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    config.seventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"216.00","qty":"1.0"}],"asks":[{"price":"218.00","qty":"1.0"}],"update_id":15}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
    ]);
    let frames = policy_demo_user_data_messages_from_json(&format!(
        "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
        spot_execution_report_json("ETHUSDT", "unit-spot-live8-spot-0000", 301, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live8-spot-0001",
            302,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live8-spot-0002", 303, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live8-spot-0003",
            304,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live8-spot-0004", 305, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live8-spot-0005",
            306,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live8-spot-0006", 307, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live8-spot-0007",
            308,
            "SELL",
            "FILLED"
        ),
    ))
    .unwrap();

    let report = run_spot_policy_demo_with_live_user_data_source(
        config,
        &policy,
        "live-source-seventh-continue",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-spot-live8-spot-0000" => 301,
                    "unit-spot-live8-spot-0001" => 302,
                    "unit-spot-live8-spot-0002" => 303,
                    "unit-spot-live8-spot-0003" => 304,
                    "unit-spot-live8-spot-0004" => 305,
                    "unit-spot-live8-spot-0005" => 306,
                    "unit-spot-live8-spot-0006" => 307,
                    "unit-spot-live8-spot-0007" => 308,
                    other => panic!("unexpected client order id {other}"),
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
        frames,
    )
    .await
    .unwrap();

    assert_eq!(report.continued_steps, 1);
    assert_eq!(report.second_continued_steps, 1);
    assert_eq!(report.third_continued_steps, 1);
    assert_eq!(report.fourth_continued_steps, 1);
    assert_eq!(report.fifth_continued_steps, 1);
    assert_eq!(report.sixth_continued_steps, 1);
    assert_eq!(report.seventh_continued_steps, 1);
    assert_eq!(report.eighth_continued_steps, 0);
    assert_eq!(report.ninth_continued_steps, 0);
    assert_eq!(report.tenth_continued_steps, 0);
    assert_eq!(report.eleventh_continued_steps, 0);
    assert_eq!(report.twelfth_continued_steps, 0);
    assert_eq!(report.thirteenth_continued_steps, 0);
    assert_eq!(report.fourteenth_continued_steps, 0);
    assert_eq!(report.placed_orders, 8);
    assert_eq!(report.order_count(), 8);
    assert_eq!(report.reconciliations.len(), 8);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: -1,
        }]
    );
    assert_eq!(report.seventh_continued_observation.len(), 80);
    assert!(
        (report.seventh_continued_observation[44] + 1.0).abs() < 1e-5,
        "seventh-hop fill-backed eth q {}",
        report.seventh_continued_observation[44]
    );
}

#[tokio::test]
async fn policy_demo_wait_user_data_continues_after_seventh_wait_fills() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.use_ladder_observation = true;
    config.client_order_id_prefix = "unit-spot-wait9".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    config.seventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"216.00","qty":"1.0"}],"asks":[{"price":"218.00","qty":"1.0"}],"update_id":15}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
    ]);
    let waits = Cell::new(0usize);

    let report = run_spot_policy_demo_with_placer_and_user_data(
        config,
        &policy,
        "wait-seventh-continue",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-spot-wait9-spot-0000" => 311,
                    "unit-spot-wait9-spot-0001" => 312,
                    "unit-spot-wait9-spot-0002" => 313,
                    "unit-spot-wait9-spot-0003" => 314,
                    "unit-spot-wait9-spot-0004" => 315,
                    "unit-spot-wait9-spot-0005" => 316,
                    "unit-spot-wait9-spot-0006" => 317,
                    "unit-spot-wait9-spot-0007" => 318,
                    other => panic!("unexpected client order id {other}"),
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
            waits.set(waits.get() + 1);
            let payload = report
                .receipts
                .iter()
                .map(|receipt| {
                    spot_execution_report_json(
                        &receipt.symbol,
                        &receipt.client_order_id,
                        receipt.order_id,
                        &receipt.side,
                        "FILLED",
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            let messages = policy_demo_user_data_messages_from_json(&payload);
            async move { messages }
        },
    )
    .await
    .unwrap();

    assert_eq!(waits.get(), 7);
    assert_eq!(report.continued_steps, 1);
    assert_eq!(report.second_continued_steps, 1);
    assert_eq!(report.third_continued_steps, 1);
    assert_eq!(report.fourth_continued_steps, 1);
    assert_eq!(report.fifth_continued_steps, 1);
    assert_eq!(report.sixth_continued_steps, 1);
    assert_eq!(report.seventh_continued_steps, 1);
    assert_eq!(report.eighth_continued_steps, 0);
    assert_eq!(report.ninth_continued_steps, 0);
    assert_eq!(report.tenth_continued_steps, 0);
    assert_eq!(report.eleventh_continued_steps, 0);
    assert_eq!(report.twelfth_continued_steps, 0);
    assert_eq!(report.thirteenth_continued_steps, 0);
    assert_eq!(report.fourteenth_continued_steps, 0);
    assert_eq!(report.order_count(), 8);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: -1,
        }]
    );
    assert_eq!(report.seventh_continued_observation.len(), 80);
    assert!(
        (report.seventh_continued_observation[44] + 1.0).abs() < 1e-5,
        "mock wait seventh-hop fill-backed eth q {}",
        report.seventh_continued_observation[44]
    );
}

#[tokio::test]
async fn policy_demo_usdm_live_source_continues_after_seventh_wait_fills() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Usdm, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.use_ladder_observation = true;
    config.client_order_id_prefix = "unit-usdm-live8".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    config.seventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"216.00","qty":"1.0"}],"asks":[{"price":"218.00","qty":"1.0"}],"update_id":15}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
    ]);
    let frames = policy_demo_user_data_messages_from_json(
        &serde_json::json!([
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live8-usdm-0000",
                321,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live8-usdm-0001",
                322,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live8-usdm-0002",
                323,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live8-usdm-0003",
                324,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live8-usdm-0004",
                325,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live8-usdm-0005",
                326,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live8-usdm-0006",
                327,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live8-usdm-0007",
                328,
                "SELL",
                "FILLED"
            )
        ])
        .to_string(),
    )
    .unwrap();

    let report = run_usdm_policy_demo_with_live_user_data_source(
        config,
        &policy,
        "live-source-seventh-continue-usdm",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(UsdmPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-usdm-live8-usdm-0000" => 321,
                    "unit-usdm-live8-usdm-0001" => 322,
                    "unit-usdm-live8-usdm-0002" => 323,
                    "unit-usdm-live8-usdm-0003" => 324,
                    "unit-usdm-live8-usdm-0004" => 325,
                    "unit-usdm-live8-usdm-0005" => 326,
                    "unit-usdm-live8-usdm-0006" => 327,
                    "unit-usdm-live8-usdm-0007" => 328,
                    other => panic!("unexpected client order id {other}"),
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
        frames,
    )
    .await
    .unwrap();

    assert_eq!(report.continued_steps, 1);
    assert_eq!(report.second_continued_steps, 1);
    assert_eq!(report.third_continued_steps, 1);
    assert_eq!(report.fourth_continued_steps, 1);
    assert_eq!(report.fifth_continued_steps, 1);
    assert_eq!(report.sixth_continued_steps, 1);
    assert_eq!(report.seventh_continued_steps, 1);
    assert_eq!(report.eighth_continued_steps, 0);
    assert_eq!(report.ninth_continued_steps, 0);
    assert_eq!(report.tenth_continued_steps, 0);
    assert_eq!(report.eleventh_continued_steps, 0);
    assert_eq!(report.twelfth_continued_steps, 0);
    assert_eq!(report.thirteenth_continued_steps, 0);
    assert_eq!(report.fourteenth_continued_steps, 0);
    assert_eq!(report.order_count(), 8);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: -1,
        }]
    );
    assert_eq!(report.seventh_continued_observation.len(), 80);
    assert!(
        (report.seventh_continued_observation[44] + 1.0).abs() < 1e-5,
        "usdm seventh-hop fill-backed eth q {}",
        report.seventh_continued_observation[44]
    );
}

#[tokio::test]
async fn policy_demo_sixth_continued_tape_drains_dispatch_orders() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = false;
    config.client_order_id_prefix = "unit-spot-hop7".into();
    config.captured_user_data_json = Some(format!(
        "{}\n{}",
        spot_execution_report_json("ETHUSDT", "unit-spot-hop7-spot-0000", 291, "BUY", "FILLED"),
        spot_execution_report_json("ETHUSDT", "unit-spot-hop7-spot-0001", 292, "SELL", "FILLED"),
    ));
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop7-spot-0002",
        293,
        "BUY",
        "FILLED",
    ));
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.second_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop7-spot-0003",
        294,
        "SELL",
        "FILLED",
    ));
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.third_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop7-spot-0004",
        295,
        "BUY",
        "FILLED",
    ));
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fourth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop7-spot-0005",
        296,
        "SELL",
        "FILLED",
    ));
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.fifth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop7-spot-0006",
        297,
        "BUY",
        "FILLED",
    ));
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
    ]);

    let report =
        run_spot_policy_demo_with_placer(config, &policy, "sixth-continue-drain", |_| async {
            panic!("dry-run must not place")
        })
        .await
        .unwrap();

    assert_eq!(report.steps, 3);
    assert_eq!(report.continued_steps, 1);
    assert_eq!(report.second_continued_steps, 1);
    assert_eq!(report.third_continued_steps, 1);
    assert_eq!(report.fourth_continued_steps, 1);
    assert_eq!(report.fifth_continued_steps, 1);
    assert_eq!(report.sixth_continued_steps, 1);
    assert_eq!(report.seventh_continued_steps, 0);
    assert_eq!(report.eighth_continued_steps, 0);
    assert_eq!(report.ninth_continued_steps, 0);
    assert_eq!(report.tenth_continued_steps, 0);
    assert_eq!(report.eleventh_continued_steps, 0);
    assert_eq!(report.twelfth_continued_steps, 0);
    assert_eq!(report.thirteenth_continued_steps, 0);
    assert_eq!(report.fourteenth_continued_steps, 0);
    assert_eq!(report.placed_orders, 0);
    assert_eq!(report.order_count(), 8);
    let PolicyDemoOrders::Spot(orders) = &report.orders else {
        panic!("expected spot orders");
    };
    assert_eq!(orders[7].symbol, "ETHUSDT");
    assert_eq!(orders[7].quantity, "0.03");
    assert_eq!(orders[7].side, SpotOrderSide::Sell);
    assert_eq!(
        orders[7].new_client_order_id.as_deref(),
        Some("unit-spot-hop7-spot-0007")
    );
}

#[tokio::test]
async fn policy_demo_sixth_continued_tape_places_orders_when_guarded() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.client_order_id_prefix = "unit-spot-hop7p".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
    ]);
    let frames = policy_demo_user_data_messages_from_json(&format!(
        "{}\n{}\n{}\n{}\n{}\n{}\n{}",
        spot_execution_report_json("ETHUSDT", "unit-spot-hop7p-spot-0000", 301, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop7p-spot-0001",
            302,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-hop7p-spot-0002", 303, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop7p-spot-0003",
            304,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-hop7p-spot-0004", 305, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop7p-spot-0005",
            306,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-hop7p-spot-0006", 307, "BUY", "FILLED"),
    ))
    .unwrap();

    let report = run_spot_policy_demo_with_live_user_data_source(
        config,
        &policy,
        "sixth-continue-place",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-spot-hop7p-spot-0000" => 301,
                    "unit-spot-hop7p-spot-0001" => 302,
                    "unit-spot-hop7p-spot-0002" => 303,
                    "unit-spot-hop7p-spot-0003" => 304,
                    "unit-spot-hop7p-spot-0004" => 305,
                    "unit-spot-hop7p-spot-0005" => 306,
                    "unit-spot-hop7p-spot-0006" => 307,
                    "unit-spot-hop7p-spot-0007" => 308,
                    other => panic!("unexpected placed client order id {other}"),
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
        frames,
    )
    .await
    .unwrap();

    assert_eq!(report.sixth_continued_steps, 1);
    assert_eq!(report.seventh_continued_steps, 0);
    assert_eq!(report.eighth_continued_steps, 0);
    assert_eq!(report.ninth_continued_steps, 0);
    assert_eq!(report.tenth_continued_steps, 0);
    assert_eq!(report.eleventh_continued_steps, 0);
    assert_eq!(report.twelfth_continued_steps, 0);
    assert_eq!(report.thirteenth_continued_steps, 0);
    assert_eq!(report.fourteenth_continued_steps, 0);
    assert_eq!(report.placed_orders, 8);
    assert_eq!(report.order_count(), 8);
    assert_eq!(report.receipts.len(), 8);
    assert_eq!(
        report.receipts[0].client_order_id,
        "unit-spot-hop7p-spot-0000"
    );
    assert_eq!(
        report.receipts[7].client_order_id,
        "unit-spot-hop7p-spot-0007"
    );
    assert_eq!(report.receipts[7].side, "SELL");
    let PolicyDemoOrders::Spot(orders) = &report.orders else {
        panic!("expected spot orders");
    };
    assert_eq!(
        orders[7].new_client_order_id.as_deref(),
        Some("unit-spot-hop7p-spot-0007")
    );
    assert_eq!(orders[7].side, SpotOrderSide::Sell);
}

#[tokio::test]
async fn policy_demo_seventh_continued_tape_drains_dispatch_orders() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = false;
    config.client_order_id_prefix = "unit-spot-hop8".into();
    config.captured_user_data_json = Some(format!(
        "{}\n{}",
        spot_execution_report_json("ETHUSDT", "unit-spot-hop8-spot-0000", 331, "BUY", "FILLED"),
        spot_execution_report_json("ETHUSDT", "unit-spot-hop8-spot-0001", 332, "SELL", "FILLED"),
    ));
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop8-spot-0002",
        333,
        "BUY",
        "FILLED",
    ));
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.second_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop8-spot-0003",
        334,
        "SELL",
        "FILLED",
    ));
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.third_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop8-spot-0004",
        335,
        "BUY",
        "FILLED",
    ));
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fourth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop8-spot-0005",
        336,
        "SELL",
        "FILLED",
    ));
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.fifth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop8-spot-0006",
        337,
        "BUY",
        "FILLED",
    ));
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    config.sixth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop8-spot-0007",
        338,
        "SELL",
        "FILLED",
    ));
    config.seventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"216.00","qty":"1.0"}],"asks":[{"price":"218.00","qty":"1.0"}],"update_id":15}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
    ]);

    let report =
        run_spot_policy_demo_with_placer(config, &policy, "seventh-continue-drain", |_| async {
            panic!("dry-run must not place")
        })
        .await
        .unwrap();

    assert_eq!(report.steps, 3);
    assert_eq!(report.continued_steps, 1);
    assert_eq!(report.second_continued_steps, 1);
    assert_eq!(report.third_continued_steps, 1);
    assert_eq!(report.fourth_continued_steps, 1);
    assert_eq!(report.fifth_continued_steps, 1);
    assert_eq!(report.sixth_continued_steps, 1);
    assert_eq!(report.seventh_continued_steps, 1);
    assert_eq!(report.eighth_continued_steps, 0);
    assert_eq!(report.ninth_continued_steps, 0);
    assert_eq!(report.tenth_continued_steps, 0);
    assert_eq!(report.eleventh_continued_steps, 0);
    assert_eq!(report.twelfth_continued_steps, 0);
    assert_eq!(report.thirteenth_continued_steps, 0);
    assert_eq!(report.fourteenth_continued_steps, 0);
    assert_eq!(report.placed_orders, 0);
    assert_eq!(report.order_count(), 9);
    let PolicyDemoOrders::Spot(orders) = &report.orders else {
        panic!("expected spot orders");
    };
    assert_eq!(orders[8].symbol, "ETHUSDT");
    assert_eq!(orders[8].quantity, "0.03");
    assert_eq!(orders[8].side, SpotOrderSide::Buy);
    assert_eq!(
        orders[8].new_client_order_id.as_deref(),
        Some("unit-spot-hop8-spot-0008")
    );
}

#[tokio::test]
async fn policy_demo_seventh_continued_tape_places_orders_when_guarded() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.client_order_id_prefix = "unit-spot-hop8p".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    config.seventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"216.00","qty":"1.0"}],"asks":[{"price":"218.00","qty":"1.0"}],"update_id":15}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
    ]);
    let frames = policy_demo_user_data_messages_from_json(&format!(
        "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
        spot_execution_report_json("ETHUSDT", "unit-spot-hop8p-spot-0000", 341, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop8p-spot-0001",
            342,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-hop8p-spot-0002", 343, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop8p-spot-0003",
            344,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-hop8p-spot-0004", 345, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop8p-spot-0005",
            346,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-hop8p-spot-0006", 347, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop8p-spot-0007",
            348,
            "SELL",
            "FILLED"
        ),
    ))
    .unwrap();

    let report = run_spot_policy_demo_with_live_user_data_source(
        config,
        &policy,
        "seventh-continue-place",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-spot-hop8p-spot-0000" => 341,
                    "unit-spot-hop8p-spot-0001" => 342,
                    "unit-spot-hop8p-spot-0002" => 343,
                    "unit-spot-hop8p-spot-0003" => 344,
                    "unit-spot-hop8p-spot-0004" => 345,
                    "unit-spot-hop8p-spot-0005" => 346,
                    "unit-spot-hop8p-spot-0006" => 347,
                    "unit-spot-hop8p-spot-0007" => 348,
                    "unit-spot-hop8p-spot-0008" => 349,
                    other => panic!("unexpected placed client order id {other}"),
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
        frames,
    )
    .await
    .unwrap();

    assert_eq!(report.seventh_continued_steps, 1);
    assert_eq!(report.eighth_continued_steps, 0);
    assert_eq!(report.ninth_continued_steps, 0);
    assert_eq!(report.tenth_continued_steps, 0);
    assert_eq!(report.eleventh_continued_steps, 0);
    assert_eq!(report.twelfth_continued_steps, 0);
    assert_eq!(report.thirteenth_continued_steps, 0);
    assert_eq!(report.fourteenth_continued_steps, 0);
    assert_eq!(report.placed_orders, 9);
    assert_eq!(report.order_count(), 9);
    assert_eq!(report.receipts.len(), 9);
    assert_eq!(
        report.receipts[0].client_order_id,
        "unit-spot-hop8p-spot-0000"
    );
    assert_eq!(
        report.receipts[8].client_order_id,
        "unit-spot-hop8p-spot-0008"
    );
    assert_eq!(report.receipts[8].side, "BUY");
    let PolicyDemoOrders::Spot(orders) = &report.orders else {
        panic!("expected spot orders");
    };
    assert_eq!(
        orders[8].new_client_order_id.as_deref(),
        Some("unit-spot-hop8p-spot-0008")
    );
    assert_eq!(orders[8].side, SpotOrderSide::Buy);
}

#[tokio::test]
async fn policy_demo_live_source_continues_after_eighth_wait_fills() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.use_ladder_observation = true;
    config.client_order_id_prefix = "unit-spot-live9".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    config.seventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"216.00","qty":"1.0"}],"asks":[{"price":"218.00","qty":"1.0"}],"update_id":15}"#
            .into(),
    );
    config.eighth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"217.00","qty":"1.0"}],"asks":[{"price":"219.00","qty":"1.0"}],"update_id":16}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
    ]);
    let frames = policy_demo_user_data_messages_from_json(&format!(
        "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
        spot_execution_report_json("ETHUSDT", "unit-spot-live9-spot-0000", 351, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live9-spot-0001",
            352,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live9-spot-0002", 353, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live9-spot-0003",
            354,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live9-spot-0004", 355, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live9-spot-0005",
            356,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live9-spot-0006", 357, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live9-spot-0007",
            358,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live9-spot-0008", 359, "BUY", "FILLED"),
    ))
    .unwrap();

    let report = run_spot_policy_demo_with_live_user_data_source(
        config,
        &policy,
        "live-source-eighth-continue",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-spot-live9-spot-0000" => 351,
                    "unit-spot-live9-spot-0001" => 352,
                    "unit-spot-live9-spot-0002" => 353,
                    "unit-spot-live9-spot-0003" => 354,
                    "unit-spot-live9-spot-0004" => 355,
                    "unit-spot-live9-spot-0005" => 356,
                    "unit-spot-live9-spot-0006" => 357,
                    "unit-spot-live9-spot-0007" => 358,
                    "unit-spot-live9-spot-0008" => 359,
                    other => panic!("unexpected client order id {other}"),
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
        frames,
    )
    .await
    .unwrap();

    assert_eq!(report.continued_steps, 1);
    assert_eq!(report.second_continued_steps, 1);
    assert_eq!(report.third_continued_steps, 1);
    assert_eq!(report.fourth_continued_steps, 1);
    assert_eq!(report.fifth_continued_steps, 1);
    assert_eq!(report.sixth_continued_steps, 1);
    assert_eq!(report.seventh_continued_steps, 1);
    assert_eq!(report.eighth_continued_steps, 1);
    assert_eq!(report.ninth_continued_steps, 0);
    assert_eq!(report.tenth_continued_steps, 0);
    assert_eq!(report.eleventh_continued_steps, 0);
    assert_eq!(report.twelfth_continued_steps, 0);
    assert_eq!(report.thirteenth_continued_steps, 0);
    assert_eq!(report.fourteenth_continued_steps, 0);
    assert_eq!(report.placed_orders, 9);
    assert_eq!(report.order_count(), 9);
    assert_eq!(report.reconciliations.len(), 9);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: 1,
        }]
    );
    assert_eq!(report.eighth_continued_observation.len(), 80);
    assert!(
        (report.eighth_continued_observation[44] - 1.0).abs() < 1e-5,
        "eighth-hop fill-backed eth q {}",
        report.eighth_continued_observation[44]
    );
}

#[tokio::test]
async fn policy_demo_wait_user_data_continues_after_eighth_wait_fills() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.use_ladder_observation = true;
    config.client_order_id_prefix = "unit-spot-wait10".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    config.seventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"216.00","qty":"1.0"}],"asks":[{"price":"218.00","qty":"1.0"}],"update_id":15}"#
            .into(),
    );
    config.eighth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"217.00","qty":"1.0"}],"asks":[{"price":"219.00","qty":"1.0"}],"update_id":16}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
    ]);
    let waits = Cell::new(0usize);

    let report = run_spot_policy_demo_with_placer_and_user_data(
        config,
        &policy,
        "wait-eighth-continue",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-spot-wait10-spot-0000" => 361,
                    "unit-spot-wait10-spot-0001" => 362,
                    "unit-spot-wait10-spot-0002" => 363,
                    "unit-spot-wait10-spot-0003" => 364,
                    "unit-spot-wait10-spot-0004" => 365,
                    "unit-spot-wait10-spot-0005" => 366,
                    "unit-spot-wait10-spot-0006" => 367,
                    "unit-spot-wait10-spot-0007" => 368,
                    "unit-spot-wait10-spot-0008" => 369,
                    other => panic!("unexpected client order id {other}"),
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
            waits.set(waits.get() + 1);
            let payload = report
                .receipts
                .iter()
                .map(|receipt| {
                    spot_execution_report_json(
                        &receipt.symbol,
                        &receipt.client_order_id,
                        receipt.order_id,
                        &receipt.side,
                        "FILLED",
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            let messages = policy_demo_user_data_messages_from_json(&payload);
            async move { messages }
        },
    )
    .await
    .unwrap();

    assert_eq!(waits.get(), 8);
    assert_eq!(report.continued_steps, 1);
    assert_eq!(report.second_continued_steps, 1);
    assert_eq!(report.third_continued_steps, 1);
    assert_eq!(report.fourth_continued_steps, 1);
    assert_eq!(report.fifth_continued_steps, 1);
    assert_eq!(report.sixth_continued_steps, 1);
    assert_eq!(report.seventh_continued_steps, 1);
    assert_eq!(report.eighth_continued_steps, 1);
    assert_eq!(report.ninth_continued_steps, 0);
    assert_eq!(report.tenth_continued_steps, 0);
    assert_eq!(report.eleventh_continued_steps, 0);
    assert_eq!(report.twelfth_continued_steps, 0);
    assert_eq!(report.thirteenth_continued_steps, 0);
    assert_eq!(report.fourteenth_continued_steps, 0);
    assert_eq!(report.order_count(), 9);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: 1,
        }]
    );
    assert_eq!(report.eighth_continued_observation.len(), 80);
    assert!(
        (report.eighth_continued_observation[44] - 1.0).abs() < 1e-5,
        "mock wait eighth-hop fill-backed eth q {}",
        report.eighth_continued_observation[44]
    );
}

#[tokio::test]
async fn policy_demo_usdm_live_source_continues_after_eighth_wait_fills() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Usdm, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.use_ladder_observation = true;
    config.client_order_id_prefix = "unit-usdm-live9".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    config.seventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"216.00","qty":"1.0"}],"asks":[{"price":"218.00","qty":"1.0"}],"update_id":15}"#
            .into(),
    );
    config.eighth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"217.00","qty":"1.0"}],"asks":[{"price":"219.00","qty":"1.0"}],"update_id":16}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
    ]);
    let frames = policy_demo_user_data_messages_from_json(
        &serde_json::json!([
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live9-usdm-0000",
                371,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live9-usdm-0001",
                372,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live9-usdm-0002",
                373,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live9-usdm-0003",
                374,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live9-usdm-0004",
                375,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live9-usdm-0005",
                376,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live9-usdm-0006",
                377,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live9-usdm-0007",
                378,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live9-usdm-0008",
                379,
                "BUY",
                "FILLED"
            )
        ])
        .to_string(),
    )
    .unwrap();

    let report = run_usdm_policy_demo_with_live_user_data_source(
        config,
        &policy,
        "live-source-eighth-continue-usdm",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(UsdmPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-usdm-live9-usdm-0000" => 371,
                    "unit-usdm-live9-usdm-0001" => 372,
                    "unit-usdm-live9-usdm-0002" => 373,
                    "unit-usdm-live9-usdm-0003" => 374,
                    "unit-usdm-live9-usdm-0004" => 375,
                    "unit-usdm-live9-usdm-0005" => 376,
                    "unit-usdm-live9-usdm-0006" => 377,
                    "unit-usdm-live9-usdm-0007" => 378,
                    "unit-usdm-live9-usdm-0008" => 379,
                    other => panic!("unexpected client order id {other}"),
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
        frames,
    )
    .await
    .unwrap();

    assert_eq!(report.continued_steps, 1);
    assert_eq!(report.second_continued_steps, 1);
    assert_eq!(report.third_continued_steps, 1);
    assert_eq!(report.fourth_continued_steps, 1);
    assert_eq!(report.fifth_continued_steps, 1);
    assert_eq!(report.sixth_continued_steps, 1);
    assert_eq!(report.seventh_continued_steps, 1);
    assert_eq!(report.eighth_continued_steps, 1);
    assert_eq!(report.ninth_continued_steps, 0);
    assert_eq!(report.tenth_continued_steps, 0);
    assert_eq!(report.eleventh_continued_steps, 0);
    assert_eq!(report.twelfth_continued_steps, 0);
    assert_eq!(report.thirteenth_continued_steps, 0);
    assert_eq!(report.fourteenth_continued_steps, 0);
    assert_eq!(report.order_count(), 9);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: 1,
        }]
    );
    assert_eq!(report.eighth_continued_observation.len(), 80);
    assert!(
        (report.eighth_continued_observation[44] - 1.0).abs() < 1e-5,
        "usdm eighth-hop fill-backed eth q {}",
        report.eighth_continued_observation[44]
    );
}

#[tokio::test]
async fn policy_demo_eighth_continued_tape_drains_dispatch_orders() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = false;
    config.client_order_id_prefix = "unit-spot-hop9".into();
    config.captured_user_data_json = Some(format!(
        "{}\n{}",
        spot_execution_report_json("ETHUSDT", "unit-spot-hop9-spot-0000", 381, "BUY", "FILLED"),
        spot_execution_report_json("ETHUSDT", "unit-spot-hop9-spot-0001", 382, "SELL", "FILLED"),
    ));
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop9-spot-0002",
        383,
        "BUY",
        "FILLED",
    ));
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.second_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop9-spot-0003",
        384,
        "SELL",
        "FILLED",
    ));
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.third_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop9-spot-0004",
        385,
        "BUY",
        "FILLED",
    ));
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fourth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop9-spot-0005",
        386,
        "SELL",
        "FILLED",
    ));
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.fifth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop9-spot-0006",
        387,
        "BUY",
        "FILLED",
    ));
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    config.sixth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop9-spot-0007",
        388,
        "SELL",
        "FILLED",
    ));
    config.seventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"216.00","qty":"1.0"}],"asks":[{"price":"218.00","qty":"1.0"}],"update_id":15}"#
            .into(),
    );
    config.seventh_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop9-spot-0008",
        389,
        "BUY",
        "FILLED",
    ));
    config.eighth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"217.00","qty":"1.0"}],"asks":[{"price":"219.00","qty":"1.0"}],"update_id":16}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
    ]);

    let report =
        run_spot_policy_demo_with_placer(config, &policy, "eighth-continue-drain", |_| async {
            panic!("dry-run must not place")
        })
        .await
        .unwrap();

    assert_eq!(report.steps, 3);
    assert_eq!(report.continued_steps, 1);
    assert_eq!(report.second_continued_steps, 1);
    assert_eq!(report.third_continued_steps, 1);
    assert_eq!(report.fourth_continued_steps, 1);
    assert_eq!(report.fifth_continued_steps, 1);
    assert_eq!(report.sixth_continued_steps, 1);
    assert_eq!(report.seventh_continued_steps, 1);
    assert_eq!(report.eighth_continued_steps, 1);
    assert_eq!(report.ninth_continued_steps, 0);
    assert_eq!(report.tenth_continued_steps, 0);
    assert_eq!(report.eleventh_continued_steps, 0);
    assert_eq!(report.twelfth_continued_steps, 0);
    assert_eq!(report.thirteenth_continued_steps, 0);
    assert_eq!(report.fourteenth_continued_steps, 0);
    assert_eq!(report.placed_orders, 0);
    assert_eq!(report.order_count(), 10);
    let PolicyDemoOrders::Spot(orders) = &report.orders else {
        panic!("expected spot orders");
    };
    assert_eq!(orders[9].symbol, "ETHUSDT");
    assert_eq!(orders[9].quantity, "0.03");
    assert_eq!(orders[9].side, SpotOrderSide::Sell);
    assert_eq!(
        orders[9].new_client_order_id.as_deref(),
        Some("unit-spot-hop9-spot-0009")
    );
}

#[tokio::test]
async fn policy_demo_eighth_continued_tape_places_orders_when_guarded() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.client_order_id_prefix = "unit-spot-hop9p".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    config.seventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"216.00","qty":"1.0"}],"asks":[{"price":"218.00","qty":"1.0"}],"update_id":15}"#
            .into(),
    );
    config.eighth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"217.00","qty":"1.0"}],"asks":[{"price":"219.00","qty":"1.0"}],"update_id":16}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
    ]);
    let frames = policy_demo_user_data_messages_from_json(&format!(
        "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
        spot_execution_report_json("ETHUSDT", "unit-spot-hop9p-spot-0000", 391, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop9p-spot-0001",
            392,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-hop9p-spot-0002", 393, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop9p-spot-0003",
            394,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-hop9p-spot-0004", 395, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop9p-spot-0005",
            396,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-hop9p-spot-0006", 397, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop9p-spot-0007",
            398,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-hop9p-spot-0008", 399, "BUY", "FILLED"),
    ))
    .unwrap();

    let report = run_spot_policy_demo_with_live_user_data_source(
        config,
        &policy,
        "eighth-continue-place",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-spot-hop9p-spot-0000" => 391,
                    "unit-spot-hop9p-spot-0001" => 392,
                    "unit-spot-hop9p-spot-0002" => 393,
                    "unit-spot-hop9p-spot-0003" => 394,
                    "unit-spot-hop9p-spot-0004" => 395,
                    "unit-spot-hop9p-spot-0005" => 396,
                    "unit-spot-hop9p-spot-0006" => 397,
                    "unit-spot-hop9p-spot-0007" => 398,
                    "unit-spot-hop9p-spot-0008" => 399,
                    "unit-spot-hop9p-spot-0009" => 400,
                    other => panic!("unexpected placed client order id {other}"),
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
        frames,
    )
    .await
    .unwrap();

    assert_eq!(report.eighth_continued_steps, 1);
    assert_eq!(report.ninth_continued_steps, 0);
    assert_eq!(report.tenth_continued_steps, 0);
    assert_eq!(report.eleventh_continued_steps, 0);
    assert_eq!(report.twelfth_continued_steps, 0);
    assert_eq!(report.thirteenth_continued_steps, 0);
    assert_eq!(report.fourteenth_continued_steps, 0);
    assert_eq!(report.placed_orders, 10);
    assert_eq!(report.order_count(), 10);
    assert_eq!(report.receipts.len(), 10);
    assert_eq!(
        report.receipts[0].client_order_id,
        "unit-spot-hop9p-spot-0000"
    );
    assert_eq!(
        report.receipts[9].client_order_id,
        "unit-spot-hop9p-spot-0009"
    );
    assert_eq!(report.receipts[9].side, "SELL");
    let PolicyDemoOrders::Spot(orders) = &report.orders else {
        panic!("expected spot orders");
    };
    assert_eq!(
        orders[9].new_client_order_id.as_deref(),
        Some("unit-spot-hop9p-spot-0009")
    );
    assert_eq!(orders[9].side, SpotOrderSide::Sell);
}

#[tokio::test]
async fn policy_demo_live_source_continues_after_ninth_wait_fills() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.use_ladder_observation = true;
    config.client_order_id_prefix = "unit-spot-live10".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    config.seventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"216.00","qty":"1.0"}],"asks":[{"price":"218.00","qty":"1.0"}],"update_id":15}"#
            .into(),
    );
    config.eighth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"217.00","qty":"1.0"}],"asks":[{"price":"219.00","qty":"1.0"}],"update_id":16}"#
            .into(),
    );
    config.ninth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"218.00","qty":"1.0"}],"asks":[{"price":"220.00","qty":"1.0"}],"update_id":17}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
    ]);
    let frames = policy_demo_user_data_messages_from_json(&format!(
        "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
        spot_execution_report_json("ETHUSDT", "unit-spot-live10-spot-0000", 401, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live10-spot-0001",
            402,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live10-spot-0002", 403, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live10-spot-0003",
            404,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live10-spot-0004", 405, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live10-spot-0005",
            406,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live10-spot-0006", 407, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live10-spot-0007",
            408,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live10-spot-0008", 409, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live10-spot-0009",
            410,
            "SELL",
            "FILLED"
        ),
    ))
    .unwrap();

    let report = run_spot_policy_demo_with_live_user_data_source(
        config,
        &policy,
        "live-source-ninth-continue",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-spot-live10-spot-0000" => 401,
                    "unit-spot-live10-spot-0001" => 402,
                    "unit-spot-live10-spot-0002" => 403,
                    "unit-spot-live10-spot-0003" => 404,
                    "unit-spot-live10-spot-0004" => 405,
                    "unit-spot-live10-spot-0005" => 406,
                    "unit-spot-live10-spot-0006" => 407,
                    "unit-spot-live10-spot-0007" => 408,
                    "unit-spot-live10-spot-0008" => 409,
                    "unit-spot-live10-spot-0009" => 410,
                    other => panic!("unexpected client order id {other}"),
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
        frames,
    )
    .await
    .unwrap();

    assert_eq!(report.continued_steps, 1);
    assert_eq!(report.second_continued_steps, 1);
    assert_eq!(report.third_continued_steps, 1);
    assert_eq!(report.fourth_continued_steps, 1);
    assert_eq!(report.fifth_continued_steps, 1);
    assert_eq!(report.sixth_continued_steps, 1);
    assert_eq!(report.seventh_continued_steps, 1);
    assert_eq!(report.eighth_continued_steps, 1);
    assert_eq!(report.ninth_continued_steps, 1);
    assert_eq!(report.placed_orders, 10);
    assert_eq!(report.order_count(), 10);
    assert_eq!(report.reconciliations.len(), 10);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: -1,
        }]
    );
    assert_eq!(report.ninth_continued_observation.len(), 80);
    assert!(
        (report.ninth_continued_observation[44] + 1.0).abs() < 1e-5,
        "ninth-hop fill-backed eth q {}",
        report.ninth_continued_observation[44]
    );
}

#[tokio::test]
async fn policy_demo_wait_user_data_continues_after_ninth_wait_fills() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.use_ladder_observation = true;
    config.client_order_id_prefix = "unit-spot-wait11".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    config.seventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"216.00","qty":"1.0"}],"asks":[{"price":"218.00","qty":"1.0"}],"update_id":15}"#
            .into(),
    );
    config.eighth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"217.00","qty":"1.0"}],"asks":[{"price":"219.00","qty":"1.0"}],"update_id":16}"#
            .into(),
    );
    config.ninth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"218.00","qty":"1.0"}],"asks":[{"price":"220.00","qty":"1.0"}],"update_id":17}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
    ]);
    let waits = Cell::new(0usize);

    let report = run_spot_policy_demo_with_placer_and_user_data(
        config,
        &policy,
        "wait-ninth-continue",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-spot-wait11-spot-0000" => 411,
                    "unit-spot-wait11-spot-0001" => 412,
                    "unit-spot-wait11-spot-0002" => 413,
                    "unit-spot-wait11-spot-0003" => 414,
                    "unit-spot-wait11-spot-0004" => 415,
                    "unit-spot-wait11-spot-0005" => 416,
                    "unit-spot-wait11-spot-0006" => 417,
                    "unit-spot-wait11-spot-0007" => 418,
                    "unit-spot-wait11-spot-0008" => 419,
                    "unit-spot-wait11-spot-0009" => 420,
                    other => panic!("unexpected client order id {other}"),
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
            waits.set(waits.get() + 1);
            let payload = report
                .receipts
                .iter()
                .map(|receipt| {
                    spot_execution_report_json(
                        &receipt.symbol,
                        &receipt.client_order_id,
                        receipt.order_id,
                        &receipt.side,
                        "FILLED",
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            let messages = policy_demo_user_data_messages_from_json(&payload);
            async move { messages }
        },
    )
    .await
    .unwrap();

    assert_eq!(waits.get(), 9);
    assert_eq!(report.continued_steps, 1);
    assert_eq!(report.second_continued_steps, 1);
    assert_eq!(report.third_continued_steps, 1);
    assert_eq!(report.fourth_continued_steps, 1);
    assert_eq!(report.fifth_continued_steps, 1);
    assert_eq!(report.sixth_continued_steps, 1);
    assert_eq!(report.seventh_continued_steps, 1);
    assert_eq!(report.eighth_continued_steps, 1);
    assert_eq!(report.ninth_continued_steps, 1);
    assert_eq!(report.order_count(), 10);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: -1,
        }]
    );
    assert_eq!(report.ninth_continued_observation.len(), 80);
    assert!(
        (report.ninth_continued_observation[44] + 1.0).abs() < 1e-5,
        "mock wait ninth-hop fill-backed eth q {}",
        report.ninth_continued_observation[44]
    );
}

#[tokio::test]
async fn policy_demo_usdm_live_source_continues_after_ninth_wait_fills() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Usdm, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.use_ladder_observation = true;
    config.client_order_id_prefix = "unit-usdm-live10".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    config.seventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"216.00","qty":"1.0"}],"asks":[{"price":"218.00","qty":"1.0"}],"update_id":15}"#
            .into(),
    );
    config.eighth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"217.00","qty":"1.0"}],"asks":[{"price":"219.00","qty":"1.0"}],"update_id":16}"#
            .into(),
    );
    config.ninth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"218.00","qty":"1.0"}],"asks":[{"price":"220.00","qty":"1.0"}],"update_id":17}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
    ]);
    let frames = policy_demo_user_data_messages_from_json(
        &serde_json::json!([
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live10-usdm-0000",
                421,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live10-usdm-0001",
                422,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live10-usdm-0002",
                423,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live10-usdm-0003",
                424,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live10-usdm-0004",
                425,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live10-usdm-0005",
                426,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live10-usdm-0006",
                427,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live10-usdm-0007",
                428,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live10-usdm-0008",
                429,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live10-usdm-0009",
                430,
                "SELL",
                "FILLED"
            )
        ])
        .to_string(),
    )
    .unwrap();

    let report = run_usdm_policy_demo_with_live_user_data_source(
        config,
        &policy,
        "live-source-ninth-continue-usdm",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(UsdmPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-usdm-live10-usdm-0000" => 421,
                    "unit-usdm-live10-usdm-0001" => 422,
                    "unit-usdm-live10-usdm-0002" => 423,
                    "unit-usdm-live10-usdm-0003" => 424,
                    "unit-usdm-live10-usdm-0004" => 425,
                    "unit-usdm-live10-usdm-0005" => 426,
                    "unit-usdm-live10-usdm-0006" => 427,
                    "unit-usdm-live10-usdm-0007" => 428,
                    "unit-usdm-live10-usdm-0008" => 429,
                    "unit-usdm-live10-usdm-0009" => 430,
                    other => panic!("unexpected client order id {other}"),
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
        frames,
    )
    .await
    .unwrap();

    assert_eq!(report.continued_steps, 1);
    assert_eq!(report.second_continued_steps, 1);
    assert_eq!(report.third_continued_steps, 1);
    assert_eq!(report.fourth_continued_steps, 1);
    assert_eq!(report.fifth_continued_steps, 1);
    assert_eq!(report.sixth_continued_steps, 1);
    assert_eq!(report.seventh_continued_steps, 1);
    assert_eq!(report.eighth_continued_steps, 1);
    assert_eq!(report.ninth_continued_steps, 1);
    assert_eq!(report.order_count(), 10);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: -1,
        }]
    );
    assert_eq!(report.ninth_continued_observation.len(), 80);
    assert!(
        (report.ninth_continued_observation[44] + 1.0).abs() < 1e-5,
        "usdm ninth-hop fill-backed eth q {}",
        report.ninth_continued_observation[44]
    );
}

#[tokio::test]
async fn policy_demo_ninth_continued_tape_drains_dispatch_orders() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = false;
    config.client_order_id_prefix = "unit-spot-hop10".into();
    config.captured_user_data_json = Some(format!(
        "{}\n{}",
        spot_execution_report_json("ETHUSDT", "unit-spot-hop10-spot-0000", 431, "BUY", "FILLED"),
        spot_execution_report_json("ETHUSDT", "unit-spot-hop10-spot-0001", 432, "SELL", "FILLED"),
    ));
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop10-spot-0002",
        433,
        "BUY",
        "FILLED",
    ));
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.second_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop10-spot-0003",
        434,
        "SELL",
        "FILLED",
    ));
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.third_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop10-spot-0004",
        435,
        "BUY",
        "FILLED",
    ));
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fourth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop10-spot-0005",
        436,
        "SELL",
        "FILLED",
    ));
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.fifth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop10-spot-0006",
        437,
        "BUY",
        "FILLED",
    ));
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    config.sixth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop10-spot-0007",
        438,
        "SELL",
        "FILLED",
    ));
    config.seventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"216.00","qty":"1.0"}],"asks":[{"price":"218.00","qty":"1.0"}],"update_id":15}"#
            .into(),
    );
    config.seventh_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop10-spot-0008",
        439,
        "BUY",
        "FILLED",
    ));
    config.eighth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"217.00","qty":"1.0"}],"asks":[{"price":"219.00","qty":"1.0"}],"update_id":16}"#
            .into(),
    );
    config.eighth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop10-spot-0009",
        440,
        "SELL",
        "FILLED",
    ));
    config.ninth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"218.00","qty":"1.0"}],"asks":[{"price":"220.00","qty":"1.0"}],"update_id":17}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
    ]);

    let report =
        run_spot_policy_demo_with_placer(config, &policy, "ninth-continue-drain", |_| async {
            panic!("dry-run must not place")
        })
        .await
        .unwrap();

    assert_eq!(report.steps, 3);
    assert_eq!(report.continued_steps, 1);
    assert_eq!(report.second_continued_steps, 1);
    assert_eq!(report.third_continued_steps, 1);
    assert_eq!(report.fourth_continued_steps, 1);
    assert_eq!(report.fifth_continued_steps, 1);
    assert_eq!(report.sixth_continued_steps, 1);
    assert_eq!(report.seventh_continued_steps, 1);
    assert_eq!(report.eighth_continued_steps, 1);
    assert_eq!(report.ninth_continued_steps, 1);
    assert_eq!(report.placed_orders, 0);
    assert_eq!(report.order_count(), 11);
    let PolicyDemoOrders::Spot(orders) = &report.orders else {
        panic!("expected spot orders");
    };
    assert_eq!(orders[10].symbol, "ETHUSDT");
    assert_eq!(orders[10].quantity, "0.03");
    assert_eq!(orders[10].side, SpotOrderSide::Buy);
    assert_eq!(
        orders[10].new_client_order_id.as_deref(),
        Some("unit-spot-hop10-spot-0010")
    );
}

#[tokio::test]
async fn policy_demo_ninth_continued_tape_places_orders_when_guarded() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.client_order_id_prefix = "unit-spot-hop10p".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    config.seventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"216.00","qty":"1.0"}],"asks":[{"price":"218.00","qty":"1.0"}],"update_id":15}"#
            .into(),
    );
    config.eighth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"217.00","qty":"1.0"}],"asks":[{"price":"219.00","qty":"1.0"}],"update_id":16}"#
            .into(),
    );
    config.ninth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"218.00","qty":"1.0"}],"asks":[{"price":"220.00","qty":"1.0"}],"update_id":17}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
    ]);
    let frames = policy_demo_user_data_messages_from_json(&format!(
        "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
        spot_execution_report_json("ETHUSDT", "unit-spot-hop10p-spot-0000", 441, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop10p-spot-0001",
            442,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-hop10p-spot-0002", 443, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop10p-spot-0003",
            444,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-hop10p-spot-0004", 445, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop10p-spot-0005",
            446,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-hop10p-spot-0006", 447, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop10p-spot-0007",
            448,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-hop10p-spot-0008", 449, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop10p-spot-0009",
            450,
            "SELL",
            "FILLED"
        ),
    ))
    .unwrap();

    let report = run_spot_policy_demo_with_live_user_data_source(
        config,
        &policy,
        "ninth-continue-place",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-spot-hop10p-spot-0000" => 441,
                    "unit-spot-hop10p-spot-0001" => 442,
                    "unit-spot-hop10p-spot-0002" => 443,
                    "unit-spot-hop10p-spot-0003" => 444,
                    "unit-spot-hop10p-spot-0004" => 445,
                    "unit-spot-hop10p-spot-0005" => 446,
                    "unit-spot-hop10p-spot-0006" => 447,
                    "unit-spot-hop10p-spot-0007" => 448,
                    "unit-spot-hop10p-spot-0008" => 449,
                    "unit-spot-hop10p-spot-0009" => 450,
                    "unit-spot-hop10p-spot-0010" => 451,
                    other => panic!("unexpected placed client order id {other}"),
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
        frames,
    )
    .await
    .unwrap();

    assert_eq!(report.ninth_continued_steps, 1);
    assert_eq!(report.placed_orders, 11);
    assert_eq!(report.order_count(), 11);
    assert_eq!(report.receipts.len(), 11);
    assert_eq!(
        report.receipts[0].client_order_id,
        "unit-spot-hop10p-spot-0000"
    );
    assert_eq!(
        report.receipts[10].client_order_id,
        "unit-spot-hop10p-spot-0010"
    );
    assert_eq!(report.receipts[10].side, "BUY");
    let PolicyDemoOrders::Spot(orders) = &report.orders else {
        panic!("expected spot orders");
    };
    assert_eq!(
        orders[10].new_client_order_id.as_deref(),
        Some("unit-spot-hop10p-spot-0010")
    );
    assert_eq!(orders[10].side, SpotOrderSide::Buy);
}


#[tokio::test]
async fn policy_demo_live_source_continues_after_tenth_wait_fills() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.use_ladder_observation = true;
    config.client_order_id_prefix = "unit-spot-live11".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    config.seventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"216.00","qty":"1.0"}],"asks":[{"price":"218.00","qty":"1.0"}],"update_id":15}"#
            .into(),
    );
    config.eighth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"217.00","qty":"1.0"}],"asks":[{"price":"219.00","qty":"1.0"}],"update_id":16}"#
            .into(),
    );
    config.ninth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"218.00","qty":"1.0"}],"asks":[{"price":"220.00","qty":"1.0"}],"update_id":17}"#
            .into(),
    );
    config.tenth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"219.00","qty":"1.0"}],"asks":[{"price":"221.00","qty":"1.0"}],"update_id":18}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
    ]);
    let frames = policy_demo_user_data_messages_from_json(&format!(
        "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
        spot_execution_report_json("ETHUSDT", "unit-spot-live11-spot-0000", 461, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live11-spot-0001",
            462,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live11-spot-0002", 463, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live11-spot-0003",
            464,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live11-spot-0004", 465, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live11-spot-0005",
            466,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live11-spot-0006", 467, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live11-spot-0007",
            468,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live11-spot-0008", 469, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live11-spot-0009",
            470,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live11-spot-0010", 471, "BUY", "FILLED"),
    ))
    .unwrap();

    let report = run_spot_policy_demo_with_live_user_data_source(
        config,
        &policy,
        "live-source-tenth-continue",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-spot-live11-spot-0000" => 461,
                    "unit-spot-live11-spot-0001" => 462,
                    "unit-spot-live11-spot-0002" => 463,
                    "unit-spot-live11-spot-0003" => 464,
                    "unit-spot-live11-spot-0004" => 465,
                    "unit-spot-live11-spot-0005" => 466,
                    "unit-spot-live11-spot-0006" => 467,
                    "unit-spot-live11-spot-0007" => 468,
                    "unit-spot-live11-spot-0008" => 469,
                    "unit-spot-live11-spot-0009" => 470,
                    "unit-spot-live11-spot-0010" => 471,
                    other => panic!("unexpected client order id {other}"),
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
        frames,
    )
    .await
    .unwrap();

    assert_eq!(report.continued_steps, 1);
    assert_eq!(report.second_continued_steps, 1);
    assert_eq!(report.third_continued_steps, 1);
    assert_eq!(report.fourth_continued_steps, 1);
    assert_eq!(report.fifth_continued_steps, 1);
    assert_eq!(report.sixth_continued_steps, 1);
    assert_eq!(report.seventh_continued_steps, 1);
    assert_eq!(report.eighth_continued_steps, 1);
    assert_eq!(report.ninth_continued_steps, 1);
    assert_eq!(report.tenth_continued_steps, 1);
    assert_eq!(report.eleventh_continued_steps, 0);
    assert_eq!(report.twelfth_continued_steps, 0);
    assert_eq!(report.thirteenth_continued_steps, 0);
    assert_eq!(report.fourteenth_continued_steps, 0);
    assert_eq!(report.placed_orders, 11);
    assert_eq!(report.order_count(), 11);
    assert_eq!(report.reconciliations.len(), 11);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: 1,
        }]
    );
    assert_eq!(report.tenth_continued_observation.len(), 80);
    assert!(
        (report.tenth_continued_observation[44] - 1.0).abs() < 1e-5,
        "tenth-hop fill-backed eth q {}",
        report.tenth_continued_observation[44]
    );
}

#[tokio::test]
async fn policy_demo_wait_user_data_continues_after_tenth_wait_fills() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.use_ladder_observation = true;
    config.client_order_id_prefix = "unit-spot-wait12".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    config.seventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"216.00","qty":"1.0"}],"asks":[{"price":"218.00","qty":"1.0"}],"update_id":15}"#
            .into(),
    );
    config.eighth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"217.00","qty":"1.0"}],"asks":[{"price":"219.00","qty":"1.0"}],"update_id":16}"#
            .into(),
    );
    config.ninth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"218.00","qty":"1.0"}],"asks":[{"price":"220.00","qty":"1.0"}],"update_id":17}"#
            .into(),
    );
    config.tenth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"219.00","qty":"1.0"}],"asks":[{"price":"221.00","qty":"1.0"}],"update_id":18}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
    ]);
    let waits = Cell::new(0usize);

    let report = run_spot_policy_demo_with_placer_and_user_data(
        config,
        &policy,
        "wait-tenth-continue",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-spot-wait12-spot-0000" => 481,
                    "unit-spot-wait12-spot-0001" => 482,
                    "unit-spot-wait12-spot-0002" => 483,
                    "unit-spot-wait12-spot-0003" => 484,
                    "unit-spot-wait12-spot-0004" => 485,
                    "unit-spot-wait12-spot-0005" => 486,
                    "unit-spot-wait12-spot-0006" => 487,
                    "unit-spot-wait12-spot-0007" => 488,
                    "unit-spot-wait12-spot-0008" => 489,
                    "unit-spot-wait12-spot-0009" => 490,
                    "unit-spot-wait12-spot-0010" => 491,
                    other => panic!("unexpected client order id {other}"),
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
            waits.set(waits.get() + 1);
            let payload = report
                .receipts
                .iter()
                .map(|receipt| {
                    spot_execution_report_json(
                        &receipt.symbol,
                        &receipt.client_order_id,
                        receipt.order_id,
                        &receipt.side,
                        "FILLED",
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            let messages = policy_demo_user_data_messages_from_json(&payload);
            async move { messages }
        },
    )
    .await
    .unwrap();

    assert_eq!(waits.get(), 10);
    assert_eq!(report.continued_steps, 1);
    assert_eq!(report.second_continued_steps, 1);
    assert_eq!(report.third_continued_steps, 1);
    assert_eq!(report.fourth_continued_steps, 1);
    assert_eq!(report.fifth_continued_steps, 1);
    assert_eq!(report.sixth_continued_steps, 1);
    assert_eq!(report.seventh_continued_steps, 1);
    assert_eq!(report.eighth_continued_steps, 1);
    assert_eq!(report.ninth_continued_steps, 1);
    assert_eq!(report.tenth_continued_steps, 1);
    assert_eq!(report.eleventh_continued_steps, 0);
    assert_eq!(report.twelfth_continued_steps, 0);
    assert_eq!(report.thirteenth_continued_steps, 0);
    assert_eq!(report.fourteenth_continued_steps, 0);
    assert_eq!(report.order_count(), 11);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: 1,
        }]
    );
    assert_eq!(report.tenth_continued_observation.len(), 80);
    assert!(
        (report.tenth_continued_observation[44] - 1.0).abs() < 1e-5,
        "mock wait tenth-hop fill-backed eth q {}",
        report.tenth_continued_observation[44]
    );
}

#[tokio::test]
async fn policy_demo_usdm_live_source_continues_after_tenth_wait_fills() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Usdm, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.use_ladder_observation = true;
    config.client_order_id_prefix = "unit-usdm-live11".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    config.seventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"216.00","qty":"1.0"}],"asks":[{"price":"218.00","qty":"1.0"}],"update_id":15}"#
            .into(),
    );
    config.eighth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"217.00","qty":"1.0"}],"asks":[{"price":"219.00","qty":"1.0"}],"update_id":16}"#
            .into(),
    );
    config.ninth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"218.00","qty":"1.0"}],"asks":[{"price":"220.00","qty":"1.0"}],"update_id":17}"#
            .into(),
    );
    config.tenth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"219.00","qty":"1.0"}],"asks":[{"price":"221.00","qty":"1.0"}],"update_id":18}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
    ]);
    let frames = policy_demo_user_data_messages_from_json(
        &serde_json::json!([
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live11-usdm-0000",
                501,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live11-usdm-0001",
                502,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live11-usdm-0002",
                503,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live11-usdm-0003",
                504,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live11-usdm-0004",
                505,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live11-usdm-0005",
                506,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live11-usdm-0006",
                507,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live11-usdm-0007",
                508,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live11-usdm-0008",
                509,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live11-usdm-0009",
                510,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live11-usdm-0010",
                511,
                "BUY",
                "FILLED"
            )
        ])
        .to_string(),
    )
    .unwrap();

    let report = run_usdm_policy_demo_with_live_user_data_source(
        config,
        &policy,
        "live-source-tenth-continue-usdm",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(UsdmPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-usdm-live11-usdm-0000" => 501,
                    "unit-usdm-live11-usdm-0001" => 502,
                    "unit-usdm-live11-usdm-0002" => 503,
                    "unit-usdm-live11-usdm-0003" => 504,
                    "unit-usdm-live11-usdm-0004" => 505,
                    "unit-usdm-live11-usdm-0005" => 506,
                    "unit-usdm-live11-usdm-0006" => 507,
                    "unit-usdm-live11-usdm-0007" => 508,
                    "unit-usdm-live11-usdm-0008" => 509,
                    "unit-usdm-live11-usdm-0009" => 510,
                    "unit-usdm-live11-usdm-0010" => 511,
                    other => panic!("unexpected client order id {other}"),
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
        frames,
    )
    .await
    .unwrap();

    assert_eq!(report.continued_steps, 1);
    assert_eq!(report.second_continued_steps, 1);
    assert_eq!(report.third_continued_steps, 1);
    assert_eq!(report.fourth_continued_steps, 1);
    assert_eq!(report.fifth_continued_steps, 1);
    assert_eq!(report.sixth_continued_steps, 1);
    assert_eq!(report.seventh_continued_steps, 1);
    assert_eq!(report.eighth_continued_steps, 1);
    assert_eq!(report.ninth_continued_steps, 1);
    assert_eq!(report.tenth_continued_steps, 1);
    assert_eq!(report.eleventh_continued_steps, 0);
    assert_eq!(report.twelfth_continued_steps, 0);
    assert_eq!(report.thirteenth_continued_steps, 0);
    assert_eq!(report.fourteenth_continued_steps, 0);
    assert_eq!(report.order_count(), 11);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: 1,
        }]
    );
    assert_eq!(report.tenth_continued_observation.len(), 80);
    assert!(
        (report.tenth_continued_observation[44] - 1.0).abs() < 1e-5,
        "usdm tenth-hop fill-backed eth q {}",
        report.tenth_continued_observation[44]
    );
}

#[tokio::test]
async fn policy_demo_tenth_continued_tape_drains_dispatch_orders() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = false;
    config.client_order_id_prefix = "unit-spot-hop11".into();
    config.captured_user_data_json = Some(format!(
        "{}\n{}",
        spot_execution_report_json("ETHUSDT", "unit-spot-hop11-spot-0000", 521, "BUY", "FILLED"),
        spot_execution_report_json("ETHUSDT", "unit-spot-hop11-spot-0001", 522, "SELL", "FILLED"),
    ));
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop11-spot-0002",
        523,
        "BUY",
        "FILLED",
    ));
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.second_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop11-spot-0003",
        524,
        "SELL",
        "FILLED",
    ));
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.third_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop11-spot-0004",
        525,
        "BUY",
        "FILLED",
    ));
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fourth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop11-spot-0005",
        526,
        "SELL",
        "FILLED",
    ));
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.fifth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop11-spot-0006",
        527,
        "BUY",
        "FILLED",
    ));
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    config.sixth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop11-spot-0007",
        528,
        "SELL",
        "FILLED",
    ));
    config.seventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"216.00","qty":"1.0"}],"asks":[{"price":"218.00","qty":"1.0"}],"update_id":15}"#
            .into(),
    );
    config.seventh_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop11-spot-0008",
        529,
        "BUY",
        "FILLED",
    ));
    config.eighth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"217.00","qty":"1.0"}],"asks":[{"price":"219.00","qty":"1.0"}],"update_id":16}"#
            .into(),
    );
    config.eighth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop11-spot-0009",
        530,
        "SELL",
        "FILLED",
    ));
    config.ninth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"218.00","qty":"1.0"}],"asks":[{"price":"220.00","qty":"1.0"}],"update_id":17}"#
            .into(),
    );
    config.ninth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop11-spot-0010",
        531,
        "BUY",
        "FILLED",
    ));
    config.tenth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"219.00","qty":"1.0"}],"asks":[{"price":"221.00","qty":"1.0"}],"update_id":18}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
    ]);

    let report =
        run_spot_policy_demo_with_placer(config, &policy, "tenth-continue-drain", |_| async {
            panic!("dry-run must not place")
        })
        .await
        .unwrap();

    assert_eq!(report.steps, 3);
    assert_eq!(report.continued_steps, 1);
    assert_eq!(report.second_continued_steps, 1);
    assert_eq!(report.third_continued_steps, 1);
    assert_eq!(report.fourth_continued_steps, 1);
    assert_eq!(report.fifth_continued_steps, 1);
    assert_eq!(report.sixth_continued_steps, 1);
    assert_eq!(report.seventh_continued_steps, 1);
    assert_eq!(report.eighth_continued_steps, 1);
    assert_eq!(report.ninth_continued_steps, 1);
    assert_eq!(report.tenth_continued_steps, 1);
    assert_eq!(report.eleventh_continued_steps, 0);
    assert_eq!(report.twelfth_continued_steps, 0);
    assert_eq!(report.thirteenth_continued_steps, 0);
    assert_eq!(report.fourteenth_continued_steps, 0);
    assert_eq!(report.placed_orders, 0);
    assert_eq!(report.order_count(), 12);
    let PolicyDemoOrders::Spot(orders) = &report.orders else {
        panic!("expected spot orders");
    };
    assert_eq!(orders[11].symbol, "ETHUSDT");
    assert_eq!(orders[11].quantity, "0.03");
    assert_eq!(orders[11].side, SpotOrderSide::Sell);
    assert_eq!(
        orders[11].new_client_order_id.as_deref(),
        Some("unit-spot-hop11-spot-0011")
    );
}

#[tokio::test]
async fn policy_demo_tenth_continued_tape_places_orders_when_guarded() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.client_order_id_prefix = "unit-spot-hop11p".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    config.seventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"216.00","qty":"1.0"}],"asks":[{"price":"218.00","qty":"1.0"}],"update_id":15}"#
            .into(),
    );
    config.eighth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"217.00","qty":"1.0"}],"asks":[{"price":"219.00","qty":"1.0"}],"update_id":16}"#
            .into(),
    );
    config.ninth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"218.00","qty":"1.0"}],"asks":[{"price":"220.00","qty":"1.0"}],"update_id":17}"#
            .into(),
    );
    config.tenth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"219.00","qty":"1.0"}],"asks":[{"price":"221.00","qty":"1.0"}],"update_id":18}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
    ]);
    let frames = policy_demo_user_data_messages_from_json(&format!(
        "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
        spot_execution_report_json("ETHUSDT", "unit-spot-hop11p-spot-0000", 541, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop11p-spot-0001",
            542,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-hop11p-spot-0002", 543, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop11p-spot-0003",
            544,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-hop11p-spot-0004", 545, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop11p-spot-0005",
            546,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-hop11p-spot-0006", 547, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop11p-spot-0007",
            548,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-hop11p-spot-0008", 549, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop11p-spot-0009",
            550,
            "SELL",
            "FILLED"
        ),
    ))
    .unwrap();

    let report = run_spot_policy_demo_with_live_user_data_source(
        config,
        &policy,
        "tenth-continue-place",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-spot-hop11p-spot-0000" => 541,
                    "unit-spot-hop11p-spot-0001" => 542,
                    "unit-spot-hop11p-spot-0002" => 543,
                    "unit-spot-hop11p-spot-0003" => 544,
                    "unit-spot-hop11p-spot-0004" => 545,
                    "unit-spot-hop11p-spot-0005" => 546,
                    "unit-spot-hop11p-spot-0006" => 547,
                    "unit-spot-hop11p-spot-0007" => 548,
                    "unit-spot-hop11p-spot-0008" => 549,
                    "unit-spot-hop11p-spot-0009" => 550,
                    "unit-spot-hop11p-spot-0010" => 551,
                    "unit-spot-hop11p-spot-0011" => 552,
                    other => panic!("unexpected placed client order id {other}"),
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
        frames,
    )
    .await
    .unwrap();

    assert_eq!(report.tenth_continued_steps, 1);
    assert_eq!(report.eleventh_continued_steps, 0);
    assert_eq!(report.twelfth_continued_steps, 0);
    assert_eq!(report.thirteenth_continued_steps, 0);
    assert_eq!(report.fourteenth_continued_steps, 0);
    assert_eq!(report.placed_orders, 12);
    assert_eq!(report.order_count(), 12);
    assert_eq!(report.receipts.len(), 12);
    assert_eq!(
        report.receipts[0].client_order_id,
        "unit-spot-hop11p-spot-0000"
    );
    assert_eq!(
        report.receipts[11].client_order_id,
        "unit-spot-hop11p-spot-0011"
    );
    assert_eq!(report.receipts[11].side, "SELL");
    let PolicyDemoOrders::Spot(orders) = &report.orders else {
        panic!("expected spot orders");
    };
    assert_eq!(
        orders[11].new_client_order_id.as_deref(),
        Some("unit-spot-hop11p-spot-0011")
    );
    assert_eq!(orders[11].side, SpotOrderSide::Sell);
}

#[tokio::test]
async fn policy_demo_live_source_continues_after_eleventh_wait_fills() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.use_ladder_observation = true;
    config.client_order_id_prefix = "unit-spot-live12".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    config.seventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"216.00","qty":"1.0"}],"asks":[{"price":"218.00","qty":"1.0"}],"update_id":15}"#
            .into(),
    );
    config.eighth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"217.00","qty":"1.0"}],"asks":[{"price":"219.00","qty":"1.0"}],"update_id":16}"#
            .into(),
    );
    config.ninth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"218.00","qty":"1.0"}],"asks":[{"price":"220.00","qty":"1.0"}],"update_id":17}"#
            .into(),
    );
    config.tenth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"219.00","qty":"1.0"}],"asks":[{"price":"221.00","qty":"1.0"}],"update_id":18}"#
            .into(),
    );
    config.eleventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"220.00","qty":"1.0"}],"asks":[{"price":"222.00","qty":"1.0"}],"update_id":19}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
    ]);
    let frames = policy_demo_user_data_messages_from_json(&format!(
        "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
        spot_execution_report_json("ETHUSDT", "unit-spot-live12-spot-0000", 581, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live12-spot-0001",
            582,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live12-spot-0002", 583, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live12-spot-0003",
            584,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live12-spot-0004", 585, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live12-spot-0005",
            586,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live12-spot-0006", 587, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live12-spot-0007",
            588,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live12-spot-0008", 589, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live12-spot-0009",
            590,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live12-spot-0010", 591, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live12-spot-0011",
            592,
            "SELL",
            "FILLED"
        ),
    ))
    .unwrap();

    let report = run_spot_policy_demo_with_live_user_data_source(
        config,
        &policy,
        "live-source-eleventh-continue",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-spot-live12-spot-0000" => 581,
                    "unit-spot-live12-spot-0001" => 582,
                    "unit-spot-live12-spot-0002" => 583,
                    "unit-spot-live12-spot-0003" => 584,
                    "unit-spot-live12-spot-0004" => 585,
                    "unit-spot-live12-spot-0005" => 586,
                    "unit-spot-live12-spot-0006" => 587,
                    "unit-spot-live12-spot-0007" => 588,
                    "unit-spot-live12-spot-0008" => 589,
                    "unit-spot-live12-spot-0009" => 590,
                    "unit-spot-live12-spot-0010" => 591,
                    "unit-spot-live12-spot-0011" => 592,
                    other => panic!("unexpected client order id {other}"),
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
        frames,
    )
    .await
    .unwrap();

    assert_eq!(report.continued_steps, 1);
    assert_eq!(report.second_continued_steps, 1);
    assert_eq!(report.third_continued_steps, 1);
    assert_eq!(report.fourth_continued_steps, 1);
    assert_eq!(report.fifth_continued_steps, 1);
    assert_eq!(report.sixth_continued_steps, 1);
    assert_eq!(report.seventh_continued_steps, 1);
    assert_eq!(report.eighth_continued_steps, 1);
    assert_eq!(report.ninth_continued_steps, 1);
    assert_eq!(report.tenth_continued_steps, 1);
    assert_eq!(report.eleventh_continued_steps, 1);
    assert_eq!(report.twelfth_continued_steps, 0);
    assert_eq!(report.thirteenth_continued_steps, 0);
    assert_eq!(report.fourteenth_continued_steps, 0);
    assert_eq!(report.placed_orders, 12);
    assert_eq!(report.order_count(), 12);
    assert_eq!(report.reconciliations.len(), 12);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: -1,
        }]
    );
    assert_eq!(report.eleventh_continued_observation.len(), 80);
    assert!(
        (report.eleventh_continued_observation[44] + 1.0).abs() < 1e-5,
        "eleventh-hop fill-backed eth q {}",
        report.eleventh_continued_observation[44]
    );
}

#[tokio::test]
async fn policy_demo_wait_user_data_continues_after_eleventh_wait_fills() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.use_ladder_observation = true;
    config.client_order_id_prefix = "unit-spot-wait13".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    config.seventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"216.00","qty":"1.0"}],"asks":[{"price":"218.00","qty":"1.0"}],"update_id":15}"#
            .into(),
    );
    config.eighth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"217.00","qty":"1.0"}],"asks":[{"price":"219.00","qty":"1.0"}],"update_id":16}"#
            .into(),
    );
    config.ninth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"218.00","qty":"1.0"}],"asks":[{"price":"220.00","qty":"1.0"}],"update_id":17}"#
            .into(),
    );
    config.tenth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"219.00","qty":"1.0"}],"asks":[{"price":"221.00","qty":"1.0"}],"update_id":18}"#
            .into(),
    );
    config.eleventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"220.00","qty":"1.0"}],"asks":[{"price":"222.00","qty":"1.0"}],"update_id":19}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
    ]);
    let waits = Cell::new(0usize);

    let report = run_spot_policy_demo_with_placer_and_user_data(
        config,
        &policy,
        "wait-eleventh-continue",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-spot-wait13-spot-0000" => 601,
                    "unit-spot-wait13-spot-0001" => 602,
                    "unit-spot-wait13-spot-0002" => 603,
                    "unit-spot-wait13-spot-0003" => 604,
                    "unit-spot-wait13-spot-0004" => 605,
                    "unit-spot-wait13-spot-0005" => 606,
                    "unit-spot-wait13-spot-0006" => 607,
                    "unit-spot-wait13-spot-0007" => 608,
                    "unit-spot-wait13-spot-0008" => 609,
                    "unit-spot-wait13-spot-0009" => 610,
                    "unit-spot-wait13-spot-0010" => 611,
                    "unit-spot-wait13-spot-0011" => 612,
                    other => panic!("unexpected client order id {other}"),
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
            waits.set(waits.get() + 1);
            let payload = report
                .receipts
                .iter()
                .map(|receipt| {
                    spot_execution_report_json(
                        &receipt.symbol,
                        &receipt.client_order_id,
                        receipt.order_id,
                        &receipt.side,
                        "FILLED",
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            let messages = policy_demo_user_data_messages_from_json(&payload);
            async move { messages }
        },
    )
    .await
    .unwrap();

    assert_eq!(waits.get(), 11);
    assert_eq!(report.continued_steps, 1);
    assert_eq!(report.second_continued_steps, 1);
    assert_eq!(report.third_continued_steps, 1);
    assert_eq!(report.fourth_continued_steps, 1);
    assert_eq!(report.fifth_continued_steps, 1);
    assert_eq!(report.sixth_continued_steps, 1);
    assert_eq!(report.seventh_continued_steps, 1);
    assert_eq!(report.eighth_continued_steps, 1);
    assert_eq!(report.ninth_continued_steps, 1);
    assert_eq!(report.tenth_continued_steps, 1);
    assert_eq!(report.eleventh_continued_steps, 1);
    assert_eq!(report.twelfth_continued_steps, 0);
    assert_eq!(report.thirteenth_continued_steps, 0);
    assert_eq!(report.fourteenth_continued_steps, 0);
    assert_eq!(report.order_count(), 12);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: -1,
        }]
    );
    assert_eq!(report.eleventh_continued_observation.len(), 80);
    assert!(
        (report.eleventh_continued_observation[44] + 1.0).abs() < 1e-5,
        "mock wait eleventh-hop fill-backed eth q {}",
        report.eleventh_continued_observation[44]
    );
}

#[tokio::test]
async fn policy_demo_usdm_live_source_continues_after_eleventh_wait_fills() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Usdm, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.use_ladder_observation = true;
    config.client_order_id_prefix = "unit-usdm-live12".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    config.seventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"216.00","qty":"1.0"}],"asks":[{"price":"218.00","qty":"1.0"}],"update_id":15}"#
            .into(),
    );
    config.eighth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"217.00","qty":"1.0"}],"asks":[{"price":"219.00","qty":"1.0"}],"update_id":16}"#
            .into(),
    );
    config.ninth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"218.00","qty":"1.0"}],"asks":[{"price":"220.00","qty":"1.0"}],"update_id":17}"#
            .into(),
    );
    config.tenth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"219.00","qty":"1.0"}],"asks":[{"price":"221.00","qty":"1.0"}],"update_id":18}"#
            .into(),
    );
    config.eleventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"220.00","qty":"1.0"}],"asks":[{"price":"222.00","qty":"1.0"}],"update_id":19}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
    ]);
    let frames = policy_demo_user_data_messages_from_json(
        &serde_json::json!([
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live12-usdm-0000",
                621,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live12-usdm-0001",
                622,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live12-usdm-0002",
                623,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live12-usdm-0003",
                624,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live12-usdm-0004",
                625,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live12-usdm-0005",
                626,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live12-usdm-0006",
                627,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live12-usdm-0007",
                628,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live12-usdm-0008",
                629,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live12-usdm-0009",
                630,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live12-usdm-0010",
                631,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live12-usdm-0011",
                632,
                "SELL",
                "FILLED"
            )
        ])
        .to_string(),
    )
    .unwrap();

    let report = run_usdm_policy_demo_with_live_user_data_source(
        config,
        &policy,
        "live-source-eleventh-continue-usdm",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(UsdmPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-usdm-live12-usdm-0000" => 621,
                    "unit-usdm-live12-usdm-0001" => 622,
                    "unit-usdm-live12-usdm-0002" => 623,
                    "unit-usdm-live12-usdm-0003" => 624,
                    "unit-usdm-live12-usdm-0004" => 625,
                    "unit-usdm-live12-usdm-0005" => 626,
                    "unit-usdm-live12-usdm-0006" => 627,
                    "unit-usdm-live12-usdm-0007" => 628,
                    "unit-usdm-live12-usdm-0008" => 629,
                    "unit-usdm-live12-usdm-0009" => 630,
                    "unit-usdm-live12-usdm-0010" => 631,
                    "unit-usdm-live12-usdm-0011" => 632,
                    other => panic!("unexpected client order id {other}"),
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
        frames,
    )
    .await
    .unwrap();

    assert_eq!(report.continued_steps, 1);
    assert_eq!(report.second_continued_steps, 1);
    assert_eq!(report.third_continued_steps, 1);
    assert_eq!(report.fourth_continued_steps, 1);
    assert_eq!(report.fifth_continued_steps, 1);
    assert_eq!(report.sixth_continued_steps, 1);
    assert_eq!(report.seventh_continued_steps, 1);
    assert_eq!(report.eighth_continued_steps, 1);
    assert_eq!(report.ninth_continued_steps, 1);
    assert_eq!(report.tenth_continued_steps, 1);
    assert_eq!(report.eleventh_continued_steps, 1);
    assert_eq!(report.twelfth_continued_steps, 0);
    assert_eq!(report.thirteenth_continued_steps, 0);
    assert_eq!(report.fourteenth_continued_steps, 0);
    assert_eq!(report.order_count(), 12);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: -1,
        }]
    );
    assert_eq!(report.eleventh_continued_observation.len(), 80);
    assert!(
        (report.eleventh_continued_observation[44] + 1.0).abs() < 1e-5,
        "usdm eleventh-hop fill-backed eth q {}",
        report.eleventh_continued_observation[44]
    );
}

#[tokio::test]
async fn policy_demo_eleventh_continued_tape_drains_dispatch_orders() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = false;
    config.client_order_id_prefix = "unit-spot-hop12".into();
    config.captured_user_data_json = Some(format!(
        "{}\n{}",
        spot_execution_report_json("ETHUSDT", "unit-spot-hop12-spot-0000", 641, "BUY", "FILLED"),
        spot_execution_report_json("ETHUSDT", "unit-spot-hop12-spot-0001", 642, "SELL", "FILLED"),
    ));
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop12-spot-0002",
        643,
        "BUY",
        "FILLED",
    ));
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.second_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop12-spot-0003",
        644,
        "SELL",
        "FILLED",
    ));
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.third_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop12-spot-0004",
        645,
        "BUY",
        "FILLED",
    ));
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fourth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop12-spot-0005",
        646,
        "SELL",
        "FILLED",
    ));
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.fifth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop12-spot-0006",
        647,
        "BUY",
        "FILLED",
    ));
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    config.sixth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop12-spot-0007",
        648,
        "SELL",
        "FILLED",
    ));
    config.seventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"216.00","qty":"1.0"}],"asks":[{"price":"218.00","qty":"1.0"}],"update_id":15}"#
            .into(),
    );
    config.seventh_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop12-spot-0008",
        649,
        "BUY",
        "FILLED",
    ));
    config.eighth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"217.00","qty":"1.0"}],"asks":[{"price":"219.00","qty":"1.0"}],"update_id":16}"#
            .into(),
    );
    config.eighth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop12-spot-0009",
        650,
        "SELL",
        "FILLED",
    ));
    config.ninth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"218.00","qty":"1.0"}],"asks":[{"price":"220.00","qty":"1.0"}],"update_id":17}"#
            .into(),
    );
    config.ninth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop12-spot-0010",
        651,
        "BUY",
        "FILLED",
    ));
    config.tenth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"219.00","qty":"1.0"}],"asks":[{"price":"221.00","qty":"1.0"}],"update_id":18}"#
            .into(),
    );
    config.tenth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop12-spot-0011",
        652,
        "SELL",
        "FILLED",
    ));
    config.eleventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"220.00","qty":"1.0"}],"asks":[{"price":"222.00","qty":"1.0"}],"update_id":19}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
    ]);

    let report =
        run_spot_policy_demo_with_placer(config, &policy, "eleventh-continue-drain", |_| async {
            panic!("dry-run must not place")
        })
        .await
        .unwrap();

    assert_eq!(report.steps, 3);
    assert_eq!(report.continued_steps, 1);
    assert_eq!(report.second_continued_steps, 1);
    assert_eq!(report.third_continued_steps, 1);
    assert_eq!(report.fourth_continued_steps, 1);
    assert_eq!(report.fifth_continued_steps, 1);
    assert_eq!(report.sixth_continued_steps, 1);
    assert_eq!(report.seventh_continued_steps, 1);
    assert_eq!(report.eighth_continued_steps, 1);
    assert_eq!(report.ninth_continued_steps, 1);
    assert_eq!(report.tenth_continued_steps, 1);
    assert_eq!(report.eleventh_continued_steps, 1);
    assert_eq!(report.twelfth_continued_steps, 0);
    assert_eq!(report.thirteenth_continued_steps, 0);
    assert_eq!(report.fourteenth_continued_steps, 0);
    assert_eq!(report.placed_orders, 0);
    assert_eq!(report.order_count(), 13);
    let PolicyDemoOrders::Spot(orders) = &report.orders else {
        panic!("expected spot orders");
    };
    assert_eq!(orders[12].symbol, "ETHUSDT");
    assert_eq!(orders[12].quantity, "0.03");
    assert_eq!(orders[12].side, SpotOrderSide::Buy);
    assert_eq!(
        orders[12].new_client_order_id.as_deref(),
        Some("unit-spot-hop12-spot-0012")
    );
}

#[tokio::test]
async fn policy_demo_eleventh_continued_tape_places_orders_when_guarded() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.client_order_id_prefix = "unit-spot-hop12p".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    config.seventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"216.00","qty":"1.0"}],"asks":[{"price":"218.00","qty":"1.0"}],"update_id":15}"#
            .into(),
    );
    config.eighth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"217.00","qty":"1.0"}],"asks":[{"price":"219.00","qty":"1.0"}],"update_id":16}"#
            .into(),
    );
    config.ninth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"218.00","qty":"1.0"}],"asks":[{"price":"220.00","qty":"1.0"}],"update_id":17}"#
            .into(),
    );
    config.tenth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"219.00","qty":"1.0"}],"asks":[{"price":"221.00","qty":"1.0"}],"update_id":18}"#
            .into(),
    );
    config.eleventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"220.00","qty":"1.0"}],"asks":[{"price":"222.00","qty":"1.0"}],"update_id":19}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
    ]);
    let frames = policy_demo_user_data_messages_from_json(&format!(
        "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
        spot_execution_report_json("ETHUSDT", "unit-spot-hop12p-spot-0000", 661, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop12p-spot-0001",
            662,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-hop12p-spot-0002", 663, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop12p-spot-0003",
            664,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-hop12p-spot-0004", 665, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop12p-spot-0005",
            666,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-hop12p-spot-0006", 667, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop12p-spot-0007",
            668,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-hop12p-spot-0008", 669, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop12p-spot-0009",
            670,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-hop12p-spot-0010", 671, "BUY", "FILLED"),
    ))
    .unwrap();

    let report = run_spot_policy_demo_with_live_user_data_source(
        config,
        &policy,
        "eleventh-continue-place",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-spot-hop12p-spot-0000" => 661,
                    "unit-spot-hop12p-spot-0001" => 662,
                    "unit-spot-hop12p-spot-0002" => 663,
                    "unit-spot-hop12p-spot-0003" => 664,
                    "unit-spot-hop12p-spot-0004" => 665,
                    "unit-spot-hop12p-spot-0005" => 666,
                    "unit-spot-hop12p-spot-0006" => 667,
                    "unit-spot-hop12p-spot-0007" => 668,
                    "unit-spot-hop12p-spot-0008" => 669,
                    "unit-spot-hop12p-spot-0009" => 670,
                    "unit-spot-hop12p-spot-0010" => 671,
                    "unit-spot-hop12p-spot-0011" => 672,
                    "unit-spot-hop12p-spot-0012" => 673,
                    other => panic!("unexpected placed client order id {other}"),
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
        frames,
    )
    .await
    .unwrap();

    assert_eq!(report.eleventh_continued_steps, 1);
    assert_eq!(report.twelfth_continued_steps, 0);
    assert_eq!(report.thirteenth_continued_steps, 0);
    assert_eq!(report.fourteenth_continued_steps, 0);
    assert_eq!(report.placed_orders, 13);
    assert_eq!(report.order_count(), 13);
    assert_eq!(report.receipts.len(), 13);
    assert_eq!(
        report.receipts[0].client_order_id,
        "unit-spot-hop12p-spot-0000"
    );
    assert_eq!(
        report.receipts[12].client_order_id,
        "unit-spot-hop12p-spot-0012"
    );
    assert_eq!(report.receipts[12].side, "BUY");
    let PolicyDemoOrders::Spot(orders) = &report.orders else {
        panic!("expected spot orders");
    };
    assert_eq!(
        orders[12].new_client_order_id.as_deref(),
        Some("unit-spot-hop12p-spot-0012")
    );
    assert_eq!(orders[12].side, SpotOrderSide::Buy);
}


#[tokio::test]
async fn policy_demo_live_source_continues_after_twelfth_wait_fills() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.use_ladder_observation = true;
    config.client_order_id_prefix = "unit-spot-live13".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    config.seventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"216.00","qty":"1.0"}],"asks":[{"price":"218.00","qty":"1.0"}],"update_id":15}"#
            .into(),
    );
    config.eighth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"217.00","qty":"1.0"}],"asks":[{"price":"219.00","qty":"1.0"}],"update_id":16}"#
            .into(),
    );
    config.ninth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"218.00","qty":"1.0"}],"asks":[{"price":"220.00","qty":"1.0"}],"update_id":17}"#
            .into(),
    );
    config.tenth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"219.00","qty":"1.0"}],"asks":[{"price":"221.00","qty":"1.0"}],"update_id":18}"#
            .into(),
    );
    config.eleventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"220.00","qty":"1.0"}],"asks":[{"price":"222.00","qty":"1.0"}],"update_id":19}"#
            .into(),
    );
    config.twelfth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"221.00","qty":"1.0"}],"asks":[{"price":"223.00","qty":"1.0"}],"update_id":20}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
    ]);
    let frames = policy_demo_user_data_messages_from_json(&format!(
        "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
        spot_execution_report_json("ETHUSDT", "unit-spot-live13-spot-0000", 701, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live13-spot-0001",
            702,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live13-spot-0002", 703, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live13-spot-0003",
            704,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live13-spot-0004", 705, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live13-spot-0005",
            706,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live13-spot-0006", 707, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live13-spot-0007",
            708,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live13-spot-0008", 709, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live13-spot-0009",
            710,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live13-spot-0010", 711, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live13-spot-0011",
            712,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live13-spot-0012", 713, "BUY", "FILLED"),
    ))
    .unwrap();

    let report = run_spot_policy_demo_with_live_user_data_source(
        config,
        &policy,
        "live-source-twelfth-continue",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-spot-live13-spot-0000" => 701,
                    "unit-spot-live13-spot-0001" => 702,
                    "unit-spot-live13-spot-0002" => 703,
                    "unit-spot-live13-spot-0003" => 704,
                    "unit-spot-live13-spot-0004" => 705,
                    "unit-spot-live13-spot-0005" => 706,
                    "unit-spot-live13-spot-0006" => 707,
                    "unit-spot-live13-spot-0007" => 708,
                    "unit-spot-live13-spot-0008" => 709,
                    "unit-spot-live13-spot-0009" => 710,
                    "unit-spot-live13-spot-0010" => 711,
                    "unit-spot-live13-spot-0011" => 712,
                    "unit-spot-live13-spot-0012" => 713,
                    other => panic!("unexpected client order id {other}"),
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
        frames,
    )
    .await
    .unwrap();

    assert_eq!(report.continued_steps, 1);
    assert_eq!(report.second_continued_steps, 1);
    assert_eq!(report.third_continued_steps, 1);
    assert_eq!(report.fourth_continued_steps, 1);
    assert_eq!(report.fifth_continued_steps, 1);
    assert_eq!(report.sixth_continued_steps, 1);
    assert_eq!(report.seventh_continued_steps, 1);
    assert_eq!(report.eighth_continued_steps, 1);
    assert_eq!(report.ninth_continued_steps, 1);
    assert_eq!(report.tenth_continued_steps, 1);
    assert_eq!(report.eleventh_continued_steps, 1);
    assert_eq!(report.twelfth_continued_steps, 1);
    assert_eq!(report.thirteenth_continued_steps, 0);
    assert_eq!(report.fourteenth_continued_steps, 0);
    assert_eq!(report.placed_orders, 13);
    assert_eq!(report.order_count(), 13);
    assert_eq!(report.reconciliations.len(), 13);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: 1,
        }]
    );
    assert_eq!(report.twelfth_continued_observation.len(), 80);
    assert!(
        (report.twelfth_continued_observation[44] - 1.0).abs() < 1e-5,
        "twelfth-hop fill-backed eth q {}",
        report.twelfth_continued_observation[44]
    );
}

#[tokio::test]
async fn policy_demo_wait_user_data_continues_after_twelfth_wait_fills() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.use_ladder_observation = true;
    config.client_order_id_prefix = "unit-spot-wait14".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    config.seventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"216.00","qty":"1.0"}],"asks":[{"price":"218.00","qty":"1.0"}],"update_id":15}"#
            .into(),
    );
    config.eighth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"217.00","qty":"1.0"}],"asks":[{"price":"219.00","qty":"1.0"}],"update_id":16}"#
            .into(),
    );
    config.ninth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"218.00","qty":"1.0"}],"asks":[{"price":"220.00","qty":"1.0"}],"update_id":17}"#
            .into(),
    );
    config.tenth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"219.00","qty":"1.0"}],"asks":[{"price":"221.00","qty":"1.0"}],"update_id":18}"#
            .into(),
    );
    config.eleventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"220.00","qty":"1.0"}],"asks":[{"price":"222.00","qty":"1.0"}],"update_id":19}"#
            .into(),
    );
    config.twelfth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"221.00","qty":"1.0"}],"asks":[{"price":"223.00","qty":"1.0"}],"update_id":20}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
    ]);
    let waits = Cell::new(0usize);

    let report = run_spot_policy_demo_with_placer_and_user_data(
        config,
        &policy,
        "wait-twelfth-continue",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-spot-wait14-spot-0000" => 721,
                    "unit-spot-wait14-spot-0001" => 722,
                    "unit-spot-wait14-spot-0002" => 723,
                    "unit-spot-wait14-spot-0003" => 724,
                    "unit-spot-wait14-spot-0004" => 725,
                    "unit-spot-wait14-spot-0005" => 726,
                    "unit-spot-wait14-spot-0006" => 727,
                    "unit-spot-wait14-spot-0007" => 728,
                    "unit-spot-wait14-spot-0008" => 729,
                    "unit-spot-wait14-spot-0009" => 730,
                    "unit-spot-wait14-spot-0010" => 731,
                    "unit-spot-wait14-spot-0011" => 732,
                    "unit-spot-wait14-spot-0012" => 733,
                    other => panic!("unexpected client order id {other}"),
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
            waits.set(waits.get() + 1);
            let payload = report
                .receipts
                .iter()
                .map(|receipt| {
                    spot_execution_report_json(
                        &receipt.symbol,
                        &receipt.client_order_id,
                        receipt.order_id,
                        &receipt.side,
                        "FILLED",
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            let messages = policy_demo_user_data_messages_from_json(&payload);
            async move { messages }
        },
    )
    .await
    .unwrap();

    assert_eq!(waits.get(), 12);
    assert_eq!(report.continued_steps, 1);
    assert_eq!(report.second_continued_steps, 1);
    assert_eq!(report.third_continued_steps, 1);
    assert_eq!(report.fourth_continued_steps, 1);
    assert_eq!(report.fifth_continued_steps, 1);
    assert_eq!(report.sixth_continued_steps, 1);
    assert_eq!(report.seventh_continued_steps, 1);
    assert_eq!(report.eighth_continued_steps, 1);
    assert_eq!(report.ninth_continued_steps, 1);
    assert_eq!(report.tenth_continued_steps, 1);
    assert_eq!(report.eleventh_continued_steps, 1);
    assert_eq!(report.twelfth_continued_steps, 1);
    assert_eq!(report.thirteenth_continued_steps, 0);
    assert_eq!(report.fourteenth_continued_steps, 0);
    assert_eq!(report.order_count(), 13);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: 1,
        }]
    );
    assert_eq!(report.twelfth_continued_observation.len(), 80);
    assert!(
        (report.twelfth_continued_observation[44] - 1.0).abs() < 1e-5,
        "mock wait twelfth-hop fill-backed eth q {}",
        report.twelfth_continued_observation[44]
    );
}

#[tokio::test]
async fn policy_demo_usdm_live_source_continues_after_twelfth_wait_fills() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Usdm, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.use_ladder_observation = true;
    config.client_order_id_prefix = "unit-usdm-live13".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    config.seventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"216.00","qty":"1.0"}],"asks":[{"price":"218.00","qty":"1.0"}],"update_id":15}"#
            .into(),
    );
    config.eighth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"217.00","qty":"1.0"}],"asks":[{"price":"219.00","qty":"1.0"}],"update_id":16}"#
            .into(),
    );
    config.ninth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"218.00","qty":"1.0"}],"asks":[{"price":"220.00","qty":"1.0"}],"update_id":17}"#
            .into(),
    );
    config.tenth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"219.00","qty":"1.0"}],"asks":[{"price":"221.00","qty":"1.0"}],"update_id":18}"#
            .into(),
    );
    config.eleventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"220.00","qty":"1.0"}],"asks":[{"price":"222.00","qty":"1.0"}],"update_id":19}"#
            .into(),
    );
    config.twelfth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"221.00","qty":"1.0"}],"asks":[{"price":"223.00","qty":"1.0"}],"update_id":20}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
    ]);
    let frames = policy_demo_user_data_messages_from_json(
        &serde_json::json!([
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live13-usdm-0000",
                741,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live13-usdm-0001",
                742,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live13-usdm-0002",
                743,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live13-usdm-0003",
                744,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live13-usdm-0004",
                745,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live13-usdm-0005",
                746,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live13-usdm-0006",
                747,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live13-usdm-0007",
                748,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live13-usdm-0008",
                749,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live13-usdm-0009",
                750,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live13-usdm-0010",
                751,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live13-usdm-0011",
                752,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live13-usdm-0012",
                753,
                "BUY",
                "FILLED"
            )
        ])
        .to_string(),
    )
    .unwrap();

    let report = run_usdm_policy_demo_with_live_user_data_source(
        config,
        &policy,
        "live-source-twelfth-continue-usdm",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(UsdmPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-usdm-live13-usdm-0000" => 741,
                    "unit-usdm-live13-usdm-0001" => 742,
                    "unit-usdm-live13-usdm-0002" => 743,
                    "unit-usdm-live13-usdm-0003" => 744,
                    "unit-usdm-live13-usdm-0004" => 745,
                    "unit-usdm-live13-usdm-0005" => 746,
                    "unit-usdm-live13-usdm-0006" => 747,
                    "unit-usdm-live13-usdm-0007" => 748,
                    "unit-usdm-live13-usdm-0008" => 749,
                    "unit-usdm-live13-usdm-0009" => 750,
                    "unit-usdm-live13-usdm-0010" => 751,
                    "unit-usdm-live13-usdm-0011" => 752,
                    "unit-usdm-live13-usdm-0012" => 753,
                    other => panic!("unexpected client order id {other}"),
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
        frames,
    )
    .await
    .unwrap();

    assert_eq!(report.continued_steps, 1);
    assert_eq!(report.second_continued_steps, 1);
    assert_eq!(report.third_continued_steps, 1);
    assert_eq!(report.fourth_continued_steps, 1);
    assert_eq!(report.fifth_continued_steps, 1);
    assert_eq!(report.sixth_continued_steps, 1);
    assert_eq!(report.seventh_continued_steps, 1);
    assert_eq!(report.eighth_continued_steps, 1);
    assert_eq!(report.ninth_continued_steps, 1);
    assert_eq!(report.tenth_continued_steps, 1);
    assert_eq!(report.eleventh_continued_steps, 1);
    assert_eq!(report.twelfth_continued_steps, 1);
    assert_eq!(report.thirteenth_continued_steps, 0);
    assert_eq!(report.fourteenth_continued_steps, 0);
    assert_eq!(report.order_count(), 13);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: 1,
        }]
    );
    assert_eq!(report.twelfth_continued_observation.len(), 80);
    assert!(
        (report.twelfth_continued_observation[44] - 1.0).abs() < 1e-5,
        "usdm twelfth-hop fill-backed eth q {}",
        report.twelfth_continued_observation[44]
    );
}

#[tokio::test]
async fn policy_demo_twelfth_continued_tape_drains_dispatch_orders() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = false;
    config.client_order_id_prefix = "unit-spot-hop13".into();
    config.captured_user_data_json = Some(format!(
        "{}\n{}",
        spot_execution_report_json("ETHUSDT", "unit-spot-hop13-spot-0000", 761, "BUY", "FILLED"),
        spot_execution_report_json("ETHUSDT", "unit-spot-hop13-spot-0001", 762, "SELL", "FILLED"),
    ));
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop13-spot-0002",
        763,
        "BUY",
        "FILLED",
    ));
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.second_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop13-spot-0003",
        764,
        "SELL",
        "FILLED",
    ));
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.third_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop13-spot-0004",
        765,
        "BUY",
        "FILLED",
    ));
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fourth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop13-spot-0005",
        766,
        "SELL",
        "FILLED",
    ));
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.fifth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop13-spot-0006",
        767,
        "BUY",
        "FILLED",
    ));
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    config.sixth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop13-spot-0007",
        768,
        "SELL",
        "FILLED",
    ));
    config.seventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"216.00","qty":"1.0"}],"asks":[{"price":"218.00","qty":"1.0"}],"update_id":15}"#
            .into(),
    );
    config.seventh_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop13-spot-0008",
        769,
        "BUY",
        "FILLED",
    ));
    config.eighth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"217.00","qty":"1.0"}],"asks":[{"price":"219.00","qty":"1.0"}],"update_id":16}"#
            .into(),
    );
    config.eighth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop13-spot-0009",
        770,
        "SELL",
        "FILLED",
    ));
    config.ninth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"218.00","qty":"1.0"}],"asks":[{"price":"220.00","qty":"1.0"}],"update_id":17}"#
            .into(),
    );
    config.ninth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop13-spot-0010",
        771,
        "BUY",
        "FILLED",
    ));
    config.tenth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"219.00","qty":"1.0"}],"asks":[{"price":"221.00","qty":"1.0"}],"update_id":18}"#
            .into(),
    );
    config.tenth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop13-spot-0011",
        772,
        "SELL",
        "FILLED",
    ));
    config.eleventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"220.00","qty":"1.0"}],"asks":[{"price":"222.00","qty":"1.0"}],"update_id":19}"#
            .into(),
    );
    config.eleventh_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop13-spot-0012",
        773,
        "BUY",
        "FILLED",
    ));
    config.twelfth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"221.00","qty":"1.0"}],"asks":[{"price":"223.00","qty":"1.0"}],"update_id":20}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
    ]);

    let report =
        run_spot_policy_demo_with_placer(config, &policy, "twelfth-continue-drain", |_| async {
            panic!("dry-run must not place")
        })
        .await
        .unwrap();

    assert_eq!(report.steps, 3);
    assert_eq!(report.continued_steps, 1);
    assert_eq!(report.second_continued_steps, 1);
    assert_eq!(report.third_continued_steps, 1);
    assert_eq!(report.fourth_continued_steps, 1);
    assert_eq!(report.fifth_continued_steps, 1);
    assert_eq!(report.sixth_continued_steps, 1);
    assert_eq!(report.seventh_continued_steps, 1);
    assert_eq!(report.eighth_continued_steps, 1);
    assert_eq!(report.ninth_continued_steps, 1);
    assert_eq!(report.tenth_continued_steps, 1);
    assert_eq!(report.eleventh_continued_steps, 1);
    assert_eq!(report.twelfth_continued_steps, 1);
    assert_eq!(report.thirteenth_continued_steps, 0);
    assert_eq!(report.fourteenth_continued_steps, 0);
    assert_eq!(report.placed_orders, 0);
    assert_eq!(report.order_count(), 14);
    let PolicyDemoOrders::Spot(orders) = &report.orders else {
        panic!("expected spot orders");
    };
    assert_eq!(orders[13].symbol, "ETHUSDT");
    assert_eq!(orders[13].quantity, "0.03");
    assert_eq!(orders[13].side, SpotOrderSide::Sell);
    assert_eq!(
        orders[13].new_client_order_id.as_deref(),
        Some("unit-spot-hop13-spot-0013")
    );
}

#[tokio::test]
async fn policy_demo_twelfth_continued_tape_places_orders_when_guarded() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.client_order_id_prefix = "unit-spot-hop13p".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    config.seventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"216.00","qty":"1.0"}],"asks":[{"price":"218.00","qty":"1.0"}],"update_id":15}"#
            .into(),
    );
    config.eighth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"217.00","qty":"1.0"}],"asks":[{"price":"219.00","qty":"1.0"}],"update_id":16}"#
            .into(),
    );
    config.ninth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"218.00","qty":"1.0"}],"asks":[{"price":"220.00","qty":"1.0"}],"update_id":17}"#
            .into(),
    );
    config.tenth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"219.00","qty":"1.0"}],"asks":[{"price":"221.00","qty":"1.0"}],"update_id":18}"#
            .into(),
    );
    config.eleventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"220.00","qty":"1.0"}],"asks":[{"price":"222.00","qty":"1.0"}],"update_id":19}"#
            .into(),
    );
    config.twelfth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"221.00","qty":"1.0"}],"asks":[{"price":"223.00","qty":"1.0"}],"update_id":20}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
    ]);
    let frames = policy_demo_user_data_messages_from_json(&format!(
        "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
        spot_execution_report_json("ETHUSDT", "unit-spot-hop13p-spot-0000", 781, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop13p-spot-0001",
            782,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-hop13p-spot-0002", 783, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop13p-spot-0003",
            784,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-hop13p-spot-0004", 785, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop13p-spot-0005",
            786,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-hop13p-spot-0006", 787, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop13p-spot-0007",
            788,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-hop13p-spot-0008", 789, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop13p-spot-0009",
            790,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-hop13p-spot-0010", 791, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop13p-spot-0011",
            792,
            "SELL",
            "FILLED"
        ),
    ))
    .unwrap();

    let report = run_spot_policy_demo_with_live_user_data_source(
        config,
        &policy,
        "twelfth-continue-place",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-spot-hop13p-spot-0000" => 781,
                    "unit-spot-hop13p-spot-0001" => 782,
                    "unit-spot-hop13p-spot-0002" => 783,
                    "unit-spot-hop13p-spot-0003" => 784,
                    "unit-spot-hop13p-spot-0004" => 785,
                    "unit-spot-hop13p-spot-0005" => 786,
                    "unit-spot-hop13p-spot-0006" => 787,
                    "unit-spot-hop13p-spot-0007" => 788,
                    "unit-spot-hop13p-spot-0008" => 789,
                    "unit-spot-hop13p-spot-0009" => 790,
                    "unit-spot-hop13p-spot-0010" => 791,
                    "unit-spot-hop13p-spot-0011" => 792,
                    "unit-spot-hop13p-spot-0012" => 793,
                    "unit-spot-hop13p-spot-0013" => 794,
                    other => panic!("unexpected placed client order id {other}"),
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
        frames,
    )
    .await
    .unwrap();

    assert_eq!(report.eleventh_continued_steps, 1);
    assert_eq!(report.twelfth_continued_steps, 1);
    assert_eq!(report.thirteenth_continued_steps, 0);
    assert_eq!(report.fourteenth_continued_steps, 0);
    assert_eq!(report.placed_orders, 14);
    assert_eq!(report.order_count(), 14);
    assert_eq!(report.receipts.len(), 14);
    assert_eq!(
        report.receipts[0].client_order_id,
        "unit-spot-hop13p-spot-0000"
    );
    assert_eq!(
        report.receipts[13].client_order_id,
        "unit-spot-hop13p-spot-0013"
    );
    assert_eq!(report.receipts[13].side, "SELL");
    let PolicyDemoOrders::Spot(orders) = &report.orders else {
        panic!("expected spot orders");
    };
    assert_eq!(
        orders[13].new_client_order_id.as_deref(),
        Some("unit-spot-hop13p-spot-0013")
    );
    assert_eq!(orders[13].side, SpotOrderSide::Sell);
}


#[tokio::test]
async fn policy_demo_live_source_continues_after_thirteenth_wait_fills() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.use_ladder_observation = true;
    config.client_order_id_prefix = "unit-spot-live14".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    config.seventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"216.00","qty":"1.0"}],"asks":[{"price":"218.00","qty":"1.0"}],"update_id":15}"#
            .into(),
    );
    config.eighth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"217.00","qty":"1.0"}],"asks":[{"price":"219.00","qty":"1.0"}],"update_id":16}"#
            .into(),
    );
    config.ninth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"218.00","qty":"1.0"}],"asks":[{"price":"220.00","qty":"1.0"}],"update_id":17}"#
            .into(),
    );
    config.tenth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"219.00","qty":"1.0"}],"asks":[{"price":"221.00","qty":"1.0"}],"update_id":18}"#
            .into(),
    );
    config.eleventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"220.00","qty":"1.0"}],"asks":[{"price":"222.00","qty":"1.0"}],"update_id":19}"#
            .into(),
    );
    config.twelfth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"221.00","qty":"1.0"}],"asks":[{"price":"223.00","qty":"1.0"}],"update_id":20}"#
            .into(),
    );
    config.thirteenth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"222.00","qty":"1.0"}],"asks":[{"price":"224.00","qty":"1.0"}],"update_id":21}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
    ]);
    let frames = policy_demo_user_data_messages_from_json(&format!(
        "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
        spot_execution_report_json("ETHUSDT", "unit-spot-live14-spot-0000", 801, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live14-spot-0001",
            802,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live14-spot-0002", 803, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live14-spot-0003",
            804,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live14-spot-0004", 805, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live14-spot-0005",
            806,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live14-spot-0006", 807, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live14-spot-0007",
            808,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live14-spot-0008", 809, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live14-spot-0009",
            810,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live14-spot-0010", 811, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live14-spot-0011",
            812,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live14-spot-0012", 813, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live14-spot-0013",
            814,
            "SELL",
            "FILLED"
        ),
    ))
    .unwrap();

    let report = run_spot_policy_demo_with_live_user_data_source(
        config,
        &policy,
        "live-source-thirteenth-continue",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-spot-live14-spot-0000" => 801,
                    "unit-spot-live14-spot-0001" => 802,
                    "unit-spot-live14-spot-0002" => 803,
                    "unit-spot-live14-spot-0003" => 804,
                    "unit-spot-live14-spot-0004" => 805,
                    "unit-spot-live14-spot-0005" => 806,
                    "unit-spot-live14-spot-0006" => 807,
                    "unit-spot-live14-spot-0007" => 808,
                    "unit-spot-live14-spot-0008" => 809,
                    "unit-spot-live14-spot-0009" => 810,
                    "unit-spot-live14-spot-0010" => 811,
                    "unit-spot-live14-spot-0011" => 812,
                    "unit-spot-live14-spot-0012" => 813,
                    "unit-spot-live14-spot-0013" => 814,
                    other => panic!("unexpected client order id {other}"),
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
        frames,
    )
    .await
    .unwrap();

    assert_eq!(report.continued_steps, 1);
    assert_eq!(report.second_continued_steps, 1);
    assert_eq!(report.third_continued_steps, 1);
    assert_eq!(report.fourth_continued_steps, 1);
    assert_eq!(report.fifth_continued_steps, 1);
    assert_eq!(report.sixth_continued_steps, 1);
    assert_eq!(report.seventh_continued_steps, 1);
    assert_eq!(report.eighth_continued_steps, 1);
    assert_eq!(report.ninth_continued_steps, 1);
    assert_eq!(report.tenth_continued_steps, 1);
    assert_eq!(report.eleventh_continued_steps, 1);
    assert_eq!(report.twelfth_continued_steps, 1);
    assert_eq!(report.thirteenth_continued_steps, 1);
    assert_eq!(report.fourteenth_continued_steps, 0);
    assert_eq!(report.placed_orders, 14);
    assert_eq!(report.order_count(), 14);
    assert_eq!(report.reconciliations.len(), 14);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: -1,
        }]
    );
    assert_eq!(report.thirteenth_continued_observation.len(), 80);
    assert!(
        (report.thirteenth_continued_observation[44] + 1.0).abs() < 1e-5,
        "thirteenth-hop fill-backed eth q {}",
        report.thirteenth_continued_observation[44]
    );
}

#[tokio::test]
async fn policy_demo_wait_user_data_continues_after_thirteenth_wait_fills() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.use_ladder_observation = true;
    config.client_order_id_prefix = "unit-spot-wait15".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    config.seventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"216.00","qty":"1.0"}],"asks":[{"price":"218.00","qty":"1.0"}],"update_id":15}"#
            .into(),
    );
    config.eighth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"217.00","qty":"1.0"}],"asks":[{"price":"219.00","qty":"1.0"}],"update_id":16}"#
            .into(),
    );
    config.ninth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"218.00","qty":"1.0"}],"asks":[{"price":"220.00","qty":"1.0"}],"update_id":17}"#
            .into(),
    );
    config.tenth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"219.00","qty":"1.0"}],"asks":[{"price":"221.00","qty":"1.0"}],"update_id":18}"#
            .into(),
    );
    config.eleventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"220.00","qty":"1.0"}],"asks":[{"price":"222.00","qty":"1.0"}],"update_id":19}"#
            .into(),
    );
    config.twelfth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"221.00","qty":"1.0"}],"asks":[{"price":"223.00","qty":"1.0"}],"update_id":20}"#
            .into(),
    );
    config.thirteenth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"222.00","qty":"1.0"}],"asks":[{"price":"224.00","qty":"1.0"}],"update_id":21}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
    ]);
    let waits = Cell::new(0usize);

    let report = run_spot_policy_demo_with_placer_and_user_data(
        config,
        &policy,
        "wait-thirteenth-continue",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-spot-wait15-spot-0000" => 821,
                    "unit-spot-wait15-spot-0001" => 822,
                    "unit-spot-wait15-spot-0002" => 823,
                    "unit-spot-wait15-spot-0003" => 824,
                    "unit-spot-wait15-spot-0004" => 825,
                    "unit-spot-wait15-spot-0005" => 826,
                    "unit-spot-wait15-spot-0006" => 827,
                    "unit-spot-wait15-spot-0007" => 828,
                    "unit-spot-wait15-spot-0008" => 829,
                    "unit-spot-wait15-spot-0009" => 830,
                    "unit-spot-wait15-spot-0010" => 831,
                    "unit-spot-wait15-spot-0011" => 832,
                    "unit-spot-wait15-spot-0012" => 833,
                    "unit-spot-wait15-spot-0013" => 834,
                    other => panic!("unexpected client order id {other}"),
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
            waits.set(waits.get() + 1);
            let payload = report
                .receipts
                .iter()
                .map(|receipt| {
                    spot_execution_report_json(
                        &receipt.symbol,
                        &receipt.client_order_id,
                        receipt.order_id,
                        &receipt.side,
                        "FILLED",
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            let messages = policy_demo_user_data_messages_from_json(&payload);
            async move { messages }
        },
    )
    .await
    .unwrap();

    assert_eq!(waits.get(), 13);
    assert_eq!(report.continued_steps, 1);
    assert_eq!(report.second_continued_steps, 1);
    assert_eq!(report.third_continued_steps, 1);
    assert_eq!(report.fourth_continued_steps, 1);
    assert_eq!(report.fifth_continued_steps, 1);
    assert_eq!(report.sixth_continued_steps, 1);
    assert_eq!(report.seventh_continued_steps, 1);
    assert_eq!(report.eighth_continued_steps, 1);
    assert_eq!(report.ninth_continued_steps, 1);
    assert_eq!(report.tenth_continued_steps, 1);
    assert_eq!(report.eleventh_continued_steps, 1);
    assert_eq!(report.twelfth_continued_steps, 1);
    assert_eq!(report.thirteenth_continued_steps, 1);
    assert_eq!(report.fourteenth_continued_steps, 0);
    assert_eq!(report.order_count(), 14);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: -1,
        }]
    );
    assert_eq!(report.thirteenth_continued_observation.len(), 80);
    assert!(
        (report.thirteenth_continued_observation[44] + 1.0).abs() < 1e-5,
        "mock wait thirteenth-hop fill-backed eth q {}",
        report.thirteenth_continued_observation[44]
    );
}

#[tokio::test]
async fn policy_demo_usdm_live_source_continues_after_thirteenth_wait_fills() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Usdm, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.use_ladder_observation = true;
    config.client_order_id_prefix = "unit-usdm-live14".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    config.seventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"216.00","qty":"1.0"}],"asks":[{"price":"218.00","qty":"1.0"}],"update_id":15}"#
            .into(),
    );
    config.eighth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"217.00","qty":"1.0"}],"asks":[{"price":"219.00","qty":"1.0"}],"update_id":16}"#
            .into(),
    );
    config.ninth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"218.00","qty":"1.0"}],"asks":[{"price":"220.00","qty":"1.0"}],"update_id":17}"#
            .into(),
    );
    config.tenth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"219.00","qty":"1.0"}],"asks":[{"price":"221.00","qty":"1.0"}],"update_id":18}"#
            .into(),
    );
    config.eleventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"220.00","qty":"1.0"}],"asks":[{"price":"222.00","qty":"1.0"}],"update_id":19}"#
            .into(),
    );
    config.twelfth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"221.00","qty":"1.0"}],"asks":[{"price":"223.00","qty":"1.0"}],"update_id":20}"#
            .into(),
    );
    config.thirteenth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"222.00","qty":"1.0"}],"asks":[{"price":"224.00","qty":"1.0"}],"update_id":21}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
    ]);
    let frames = policy_demo_user_data_messages_from_json(
        &serde_json::json!([
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live14-usdm-0000",
                841,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live14-usdm-0001",
                842,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live14-usdm-0002",
                843,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live14-usdm-0003",
                844,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live14-usdm-0004",
                845,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live14-usdm-0005",
                846,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live14-usdm-0006",
                847,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live14-usdm-0007",
                848,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live14-usdm-0008",
                849,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live14-usdm-0009",
                850,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live14-usdm-0010",
                851,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live14-usdm-0011",
                852,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live14-usdm-0012",
                853,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live14-usdm-0013",
                854,
                "SELL",
                "FILLED"
            )
        ])
        .to_string(),
    )
    .unwrap();

    let report = run_usdm_policy_demo_with_live_user_data_source(
        config,
        &policy,
        "live-source-thirteenth-continue-usdm",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(UsdmPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-usdm-live14-usdm-0000" => 841,
                    "unit-usdm-live14-usdm-0001" => 842,
                    "unit-usdm-live14-usdm-0002" => 843,
                    "unit-usdm-live14-usdm-0003" => 844,
                    "unit-usdm-live14-usdm-0004" => 845,
                    "unit-usdm-live14-usdm-0005" => 846,
                    "unit-usdm-live14-usdm-0006" => 847,
                    "unit-usdm-live14-usdm-0007" => 848,
                    "unit-usdm-live14-usdm-0008" => 849,
                    "unit-usdm-live14-usdm-0009" => 850,
                    "unit-usdm-live14-usdm-0010" => 851,
                    "unit-usdm-live14-usdm-0011" => 852,
                    "unit-usdm-live14-usdm-0012" => 853,
                    "unit-usdm-live14-usdm-0013" => 854,
                    other => panic!("unexpected client order id {other}"),
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
        frames,
    )
    .await
    .unwrap();

    assert_eq!(report.continued_steps, 1);
    assert_eq!(report.second_continued_steps, 1);
    assert_eq!(report.third_continued_steps, 1);
    assert_eq!(report.fourth_continued_steps, 1);
    assert_eq!(report.fifth_continued_steps, 1);
    assert_eq!(report.sixth_continued_steps, 1);
    assert_eq!(report.seventh_continued_steps, 1);
    assert_eq!(report.eighth_continued_steps, 1);
    assert_eq!(report.ninth_continued_steps, 1);
    assert_eq!(report.tenth_continued_steps, 1);
    assert_eq!(report.eleventh_continued_steps, 1);
    assert_eq!(report.twelfth_continued_steps, 1);
    assert_eq!(report.thirteenth_continued_steps, 1);
    assert_eq!(report.fourteenth_continued_steps, 0);
    assert_eq!(report.order_count(), 14);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: -1,
        }]
    );
    assert_eq!(report.thirteenth_continued_observation.len(), 80);
    assert!(
        (report.thirteenth_continued_observation[44] + 1.0).abs() < 1e-5,
        "usdm thirteenth-hop fill-backed eth q {}",
        report.thirteenth_continued_observation[44]
    );
}

#[tokio::test]
async fn policy_demo_thirteenth_continued_tape_drains_dispatch_orders() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = false;
    config.client_order_id_prefix = "unit-spot-hop14".into();
    config.captured_user_data_json = Some(format!(
        "{}\n{}",
        spot_execution_report_json("ETHUSDT", "unit-spot-hop14-spot-0000", 861, "BUY", "FILLED"),
        spot_execution_report_json("ETHUSDT", "unit-spot-hop14-spot-0001", 862, "SELL", "FILLED"),
    ));
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop14-spot-0002",
        863,
        "BUY",
        "FILLED",
    ));
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.second_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop14-spot-0003",
        864,
        "SELL",
        "FILLED",
    ));
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.third_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop14-spot-0004",
        865,
        "BUY",
        "FILLED",
    ));
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fourth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop14-spot-0005",
        866,
        "SELL",
        "FILLED",
    ));
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.fifth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop14-spot-0006",
        867,
        "BUY",
        "FILLED",
    ));
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    config.sixth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop14-spot-0007",
        868,
        "SELL",
        "FILLED",
    ));
    config.seventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"216.00","qty":"1.0"}],"asks":[{"price":"218.00","qty":"1.0"}],"update_id":15}"#
            .into(),
    );
    config.seventh_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop14-spot-0008",
        869,
        "BUY",
        "FILLED",
    ));
    config.eighth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"217.00","qty":"1.0"}],"asks":[{"price":"219.00","qty":"1.0"}],"update_id":16}"#
            .into(),
    );
    config.eighth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop14-spot-0009",
        870,
        "SELL",
        "FILLED",
    ));
    config.ninth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"218.00","qty":"1.0"}],"asks":[{"price":"220.00","qty":"1.0"}],"update_id":17}"#
            .into(),
    );
    config.ninth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop14-spot-0010",
        871,
        "BUY",
        "FILLED",
    ));
    config.tenth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"219.00","qty":"1.0"}],"asks":[{"price":"221.00","qty":"1.0"}],"update_id":18}"#
            .into(),
    );
    config.tenth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop14-spot-0011",
        872,
        "SELL",
        "FILLED",
    ));
    config.eleventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"220.00","qty":"1.0"}],"asks":[{"price":"222.00","qty":"1.0"}],"update_id":19}"#
            .into(),
    );
    config.eleventh_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop14-spot-0012",
        873,
        "BUY",
        "FILLED",
    ));
    config.twelfth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"221.00","qty":"1.0"}],"asks":[{"price":"223.00","qty":"1.0"}],"update_id":20}"#
            .into(),
    );
    config.twelfth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop14-spot-0013",
        874,
        "SELL",
        "FILLED",
    ));
    config.thirteenth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"222.00","qty":"1.0"}],"asks":[{"price":"224.00","qty":"1.0"}],"update_id":21}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
    ]);

    let report =
        run_spot_policy_demo_with_placer(config, &policy, "thirteenth-continue-drain", |_| async {
            panic!("dry-run must not place")
        })
        .await
        .unwrap();

    assert_eq!(report.steps, 3);
    assert_eq!(report.continued_steps, 1);
    assert_eq!(report.second_continued_steps, 1);
    assert_eq!(report.third_continued_steps, 1);
    assert_eq!(report.fourth_continued_steps, 1);
    assert_eq!(report.fifth_continued_steps, 1);
    assert_eq!(report.sixth_continued_steps, 1);
    assert_eq!(report.seventh_continued_steps, 1);
    assert_eq!(report.eighth_continued_steps, 1);
    assert_eq!(report.ninth_continued_steps, 1);
    assert_eq!(report.tenth_continued_steps, 1);
    assert_eq!(report.eleventh_continued_steps, 1);
    assert_eq!(report.twelfth_continued_steps, 1);
    assert_eq!(report.thirteenth_continued_steps, 1);
    assert_eq!(report.fourteenth_continued_steps, 0);
    assert_eq!(report.placed_orders, 0);
    assert_eq!(report.order_count(), 15);
    let PolicyDemoOrders::Spot(orders) = &report.orders else {
        panic!("expected spot orders");
    };
    assert_eq!(orders[14].symbol, "ETHUSDT");
    assert_eq!(orders[14].quantity, "0.03");
    assert_eq!(orders[14].side, SpotOrderSide::Buy);
    assert_eq!(
        orders[14].new_client_order_id.as_deref(),
        Some("unit-spot-hop14-spot-0014")
    );
}

#[tokio::test]
async fn policy_demo_thirteenth_continued_tape_places_orders_when_guarded() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.client_order_id_prefix = "unit-spot-hop14p".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    config.seventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"216.00","qty":"1.0"}],"asks":[{"price":"218.00","qty":"1.0"}],"update_id":15}"#
            .into(),
    );
    config.eighth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"217.00","qty":"1.0"}],"asks":[{"price":"219.00","qty":"1.0"}],"update_id":16}"#
            .into(),
    );
    config.ninth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"218.00","qty":"1.0"}],"asks":[{"price":"220.00","qty":"1.0"}],"update_id":17}"#
            .into(),
    );
    config.tenth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"219.00","qty":"1.0"}],"asks":[{"price":"221.00","qty":"1.0"}],"update_id":18}"#
            .into(),
    );
    config.eleventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"220.00","qty":"1.0"}],"asks":[{"price":"222.00","qty":"1.0"}],"update_id":19}"#
            .into(),
    );
    config.twelfth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"221.00","qty":"1.0"}],"asks":[{"price":"223.00","qty":"1.0"}],"update_id":20}"#
            .into(),
    );
    config.thirteenth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"222.00","qty":"1.0"}],"asks":[{"price":"224.00","qty":"1.0"}],"update_id":21}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
    ]);
    let frames = policy_demo_user_data_messages_from_json(&format!(
        "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
        spot_execution_report_json("ETHUSDT", "unit-spot-hop14p-spot-0000", 881, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop14p-spot-0001",
            882,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-hop14p-spot-0002", 883, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop14p-spot-0003",
            884,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-hop14p-spot-0004", 885, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop14p-spot-0005",
            886,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-hop14p-spot-0006", 887, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop14p-spot-0007",
            888,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-hop14p-spot-0008", 889, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop14p-spot-0009",
            890,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-hop14p-spot-0010", 891, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop14p-spot-0011",
            892,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-hop14p-spot-0012", 893, "BUY", "FILLED"),
    ))
    .unwrap();

    let report = run_spot_policy_demo_with_live_user_data_source(
        config,
        &policy,
        "thirteenth-continue-place",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-spot-hop14p-spot-0000" => 881,
                    "unit-spot-hop14p-spot-0001" => 882,
                    "unit-spot-hop14p-spot-0002" => 883,
                    "unit-spot-hop14p-spot-0003" => 884,
                    "unit-spot-hop14p-spot-0004" => 885,
                    "unit-spot-hop14p-spot-0005" => 886,
                    "unit-spot-hop14p-spot-0006" => 887,
                    "unit-spot-hop14p-spot-0007" => 888,
                    "unit-spot-hop14p-spot-0008" => 889,
                    "unit-spot-hop14p-spot-0009" => 890,
                    "unit-spot-hop14p-spot-0010" => 891,
                    "unit-spot-hop14p-spot-0011" => 892,
                    "unit-spot-hop14p-spot-0012" => 893,
                    "unit-spot-hop14p-spot-0013" => 894,
                    "unit-spot-hop14p-spot-0014" => 895,
                    other => panic!("unexpected placed client order id {other}"),
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
        frames,
    )
    .await
    .unwrap();

    assert_eq!(report.twelfth_continued_steps, 1);
    assert_eq!(report.thirteenth_continued_steps, 1);
    assert_eq!(report.fourteenth_continued_steps, 0);
    assert_eq!(report.placed_orders, 15);
    assert_eq!(report.order_count(), 15);
    assert_eq!(report.receipts.len(), 15);
    assert_eq!(
        report.receipts[0].client_order_id,
        "unit-spot-hop14p-spot-0000"
    );
    assert_eq!(
        report.receipts[14].client_order_id,
        "unit-spot-hop14p-spot-0014"
    );
    assert_eq!(report.receipts[14].side, "BUY");
    let PolicyDemoOrders::Spot(orders) = &report.orders else {
        panic!("expected spot orders");
    };
    assert_eq!(
        orders[14].new_client_order_id.as_deref(),
        Some("unit-spot-hop14p-spot-0014")
    );
    assert_eq!(orders[14].side, SpotOrderSide::Buy);
}

#[tokio::test]
async fn policy_demo_live_source_continues_after_fourteenth_wait_fills() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.use_ladder_observation = true;
    config.client_order_id_prefix = "unit-spot-live15".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    config.seventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"216.00","qty":"1.0"}],"asks":[{"price":"218.00","qty":"1.0"}],"update_id":15}"#
            .into(),
    );
    config.eighth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"217.00","qty":"1.0"}],"asks":[{"price":"219.00","qty":"1.0"}],"update_id":16}"#
            .into(),
    );
    config.ninth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"218.00","qty":"1.0"}],"asks":[{"price":"220.00","qty":"1.0"}],"update_id":17}"#
            .into(),
    );
    config.tenth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"219.00","qty":"1.0"}],"asks":[{"price":"221.00","qty":"1.0"}],"update_id":18}"#
            .into(),
    );
    config.eleventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"220.00","qty":"1.0"}],"asks":[{"price":"222.00","qty":"1.0"}],"update_id":19}"#
            .into(),
    );
    config.twelfth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"221.00","qty":"1.0"}],"asks":[{"price":"223.00","qty":"1.0"}],"update_id":20}"#
            .into(),
    );
    config.thirteenth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"222.00","qty":"1.0"}],"asks":[{"price":"224.00","qty":"1.0"}],"update_id":21}"#
            .into(),
    );
    config.fourteenth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"223.00","qty":"1.0"}],"asks":[{"price":"225.00","qty":"1.0"}],"update_id":22}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
    ]);
    let frames = policy_demo_user_data_messages_from_json(&format!(
        "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
        spot_execution_report_json("ETHUSDT", "unit-spot-live15-spot-0000", 801, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live15-spot-0001",
            802,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live15-spot-0002", 803, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live15-spot-0003",
            804,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live15-spot-0004", 805, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live15-spot-0005",
            806,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live15-spot-0006", 807, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live15-spot-0007",
            808,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live15-spot-0008", 809, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live15-spot-0009",
            810,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live15-spot-0010", 811, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live15-spot-0011",
            812,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live15-spot-0012", 813, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live15-spot-0013",
            814,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live15-spot-0014", 815, "BUY", "FILLED"),
    ))
    .unwrap();

    let report = run_spot_policy_demo_with_live_user_data_source(
        config,
        &policy,
        "live-source-fourteenth-continue",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-spot-live15-spot-0000" => 801,
                    "unit-spot-live15-spot-0001" => 802,
                    "unit-spot-live15-spot-0002" => 803,
                    "unit-spot-live15-spot-0003" => 804,
                    "unit-spot-live15-spot-0004" => 805,
                    "unit-spot-live15-spot-0005" => 806,
                    "unit-spot-live15-spot-0006" => 807,
                    "unit-spot-live15-spot-0007" => 808,
                    "unit-spot-live15-spot-0008" => 809,
                    "unit-spot-live15-spot-0009" => 810,
                    "unit-spot-live15-spot-0010" => 811,
                    "unit-spot-live15-spot-0011" => 812,
                    "unit-spot-live15-spot-0012" => 813,
                    "unit-spot-live15-spot-0013" => 814,
                    "unit-spot-live15-spot-0014" => 815,
                    other => panic!("unexpected client order id {other}"),
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
        frames,
    )
    .await
    .unwrap();

    assert_eq!(report.continued_steps, 1);
    assert_eq!(report.second_continued_steps, 1);
    assert_eq!(report.third_continued_steps, 1);
    assert_eq!(report.fourth_continued_steps, 1);
    assert_eq!(report.fifth_continued_steps, 1);
    assert_eq!(report.sixth_continued_steps, 1);
    assert_eq!(report.seventh_continued_steps, 1);
    assert_eq!(report.eighth_continued_steps, 1);
    assert_eq!(report.ninth_continued_steps, 1);
    assert_eq!(report.tenth_continued_steps, 1);
    assert_eq!(report.eleventh_continued_steps, 1);
    assert_eq!(report.twelfth_continued_steps, 1);
    assert_eq!(report.thirteenth_continued_steps, 1);
    assert_eq!(report.fourteenth_continued_steps, 1);
    assert_eq!(report.placed_orders, 15);
    assert_eq!(report.order_count(), 15);
    assert_eq!(report.reconciliations.len(), 15);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: 1,
        }]
    );
    assert_eq!(report.fourteenth_continued_observation.len(), 80);
    assert!(
        (report.fourteenth_continued_observation[44] - 1.0).abs() < 1e-5,
        "fourteenth-hop fill-backed eth q {}",
        report.fourteenth_continued_observation[44]
    );
}

#[tokio::test]
async fn policy_demo_wait_user_data_continues_after_fourteenth_wait_fills() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.use_ladder_observation = true;
    config.client_order_id_prefix = "unit-spot-wait16".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    config.seventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"216.00","qty":"1.0"}],"asks":[{"price":"218.00","qty":"1.0"}],"update_id":15}"#
            .into(),
    );
    config.eighth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"217.00","qty":"1.0"}],"asks":[{"price":"219.00","qty":"1.0"}],"update_id":16}"#
            .into(),
    );
    config.ninth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"218.00","qty":"1.0"}],"asks":[{"price":"220.00","qty":"1.0"}],"update_id":17}"#
            .into(),
    );
    config.tenth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"219.00","qty":"1.0"}],"asks":[{"price":"221.00","qty":"1.0"}],"update_id":18}"#
            .into(),
    );
    config.eleventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"220.00","qty":"1.0"}],"asks":[{"price":"222.00","qty":"1.0"}],"update_id":19}"#
            .into(),
    );
    config.twelfth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"221.00","qty":"1.0"}],"asks":[{"price":"223.00","qty":"1.0"}],"update_id":20}"#
            .into(),
    );
    config.thirteenth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"222.00","qty":"1.0"}],"asks":[{"price":"224.00","qty":"1.0"}],"update_id":21}"#
            .into(),
    );
    config.fourteenth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"223.00","qty":"1.0"}],"asks":[{"price":"225.00","qty":"1.0"}],"update_id":22}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
    ]);
    let waits = Cell::new(0usize);

    let report = run_spot_policy_demo_with_placer_and_user_data(
        config,
        &policy,
        "wait-fourteenth-continue",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-spot-wait16-spot-0000" => 821,
                    "unit-spot-wait16-spot-0001" => 822,
                    "unit-spot-wait16-spot-0002" => 823,
                    "unit-spot-wait16-spot-0003" => 824,
                    "unit-spot-wait16-spot-0004" => 825,
                    "unit-spot-wait16-spot-0005" => 826,
                    "unit-spot-wait16-spot-0006" => 827,
                    "unit-spot-wait16-spot-0007" => 828,
                    "unit-spot-wait16-spot-0008" => 829,
                    "unit-spot-wait16-spot-0009" => 830,
                    "unit-spot-wait16-spot-0010" => 831,
                    "unit-spot-wait16-spot-0011" => 832,
                    "unit-spot-wait16-spot-0012" => 833,
                    "unit-spot-wait16-spot-0013" => 834,
                    "unit-spot-wait16-spot-0014" => 835,
                    other => panic!("unexpected client order id {other}"),
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
            waits.set(waits.get() + 1);
            let payload = report
                .receipts
                .iter()
                .map(|receipt| {
                    spot_execution_report_json(
                        &receipt.symbol,
                        &receipt.client_order_id,
                        receipt.order_id,
                        &receipt.side,
                        "FILLED",
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            let messages = policy_demo_user_data_messages_from_json(&payload);
            async move { messages }
        },
    )
    .await
    .unwrap();

    assert_eq!(waits.get(), 14);
    assert_eq!(report.continued_steps, 1);
    assert_eq!(report.second_continued_steps, 1);
    assert_eq!(report.third_continued_steps, 1);
    assert_eq!(report.fourth_continued_steps, 1);
    assert_eq!(report.fifth_continued_steps, 1);
    assert_eq!(report.sixth_continued_steps, 1);
    assert_eq!(report.seventh_continued_steps, 1);
    assert_eq!(report.eighth_continued_steps, 1);
    assert_eq!(report.ninth_continued_steps, 1);
    assert_eq!(report.tenth_continued_steps, 1);
    assert_eq!(report.eleventh_continued_steps, 1);
    assert_eq!(report.twelfth_continued_steps, 1);
    assert_eq!(report.thirteenth_continued_steps, 1);
    assert_eq!(report.fourteenth_continued_steps, 1);
    assert_eq!(report.order_count(), 15);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: 1,
        }]
    );
    assert_eq!(report.fourteenth_continued_observation.len(), 80);
    assert!(
        (report.fourteenth_continued_observation[44] - 1.0).abs() < 1e-5,
        "mock wait fourteenth-hop fill-backed eth q {}",
        report.fourteenth_continued_observation[44]
    );
}

#[tokio::test]
async fn policy_demo_usdm_live_source_continues_after_fourteenth_wait_fills() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Usdm, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.use_ladder_observation = true;
    config.client_order_id_prefix = "unit-usdm-live15".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    config.seventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"216.00","qty":"1.0"}],"asks":[{"price":"218.00","qty":"1.0"}],"update_id":15}"#
            .into(),
    );
    config.eighth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"217.00","qty":"1.0"}],"asks":[{"price":"219.00","qty":"1.0"}],"update_id":16}"#
            .into(),
    );
    config.ninth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"218.00","qty":"1.0"}],"asks":[{"price":"220.00","qty":"1.0"}],"update_id":17}"#
            .into(),
    );
    config.tenth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"219.00","qty":"1.0"}],"asks":[{"price":"221.00","qty":"1.0"}],"update_id":18}"#
            .into(),
    );
    config.eleventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"220.00","qty":"1.0"}],"asks":[{"price":"222.00","qty":"1.0"}],"update_id":19}"#
            .into(),
    );
    config.twelfth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"221.00","qty":"1.0"}],"asks":[{"price":"223.00","qty":"1.0"}],"update_id":20}"#
            .into(),
    );
    config.thirteenth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"222.00","qty":"1.0"}],"asks":[{"price":"224.00","qty":"1.0"}],"update_id":21}"#
            .into(),
    );
    config.fourteenth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"223.00","qty":"1.0"}],"asks":[{"price":"225.00","qty":"1.0"}],"update_id":22}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
    ]);
    let frames = policy_demo_user_data_messages_from_json(
        &serde_json::json!([
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live15-usdm-0000",
                841,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live15-usdm-0001",
                842,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live15-usdm-0002",
                843,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live15-usdm-0003",
                844,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live15-usdm-0004",
                845,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live15-usdm-0005",
                846,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live15-usdm-0006",
                847,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live15-usdm-0007",
                848,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live15-usdm-0008",
                849,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live15-usdm-0009",
                850,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live15-usdm-0010",
                851,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live15-usdm-0011",
                852,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live15-usdm-0012",
                853,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live15-usdm-0013",
                854,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live15-usdm-0014",
                855,
                "BUY",
                "FILLED"
            )
        ])
        .to_string(),
    )
    .unwrap();

    let report = run_usdm_policy_demo_with_live_user_data_source(
        config,
        &policy,
        "live-source-fourteenth-continue-usdm",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(UsdmPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-usdm-live15-usdm-0000" => 841,
                    "unit-usdm-live15-usdm-0001" => 842,
                    "unit-usdm-live15-usdm-0002" => 843,
                    "unit-usdm-live15-usdm-0003" => 844,
                    "unit-usdm-live15-usdm-0004" => 845,
                    "unit-usdm-live15-usdm-0005" => 846,
                    "unit-usdm-live15-usdm-0006" => 847,
                    "unit-usdm-live15-usdm-0007" => 848,
                    "unit-usdm-live15-usdm-0008" => 849,
                    "unit-usdm-live15-usdm-0009" => 850,
                    "unit-usdm-live15-usdm-0010" => 851,
                    "unit-usdm-live15-usdm-0011" => 852,
                    "unit-usdm-live15-usdm-0012" => 853,
                    "unit-usdm-live15-usdm-0013" => 854,
                    "unit-usdm-live15-usdm-0014" => 855,
                    other => panic!("unexpected client order id {other}"),
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
        frames,
    )
    .await
    .unwrap();

    assert_eq!(report.continued_steps, 1);
    assert_eq!(report.second_continued_steps, 1);
    assert_eq!(report.third_continued_steps, 1);
    assert_eq!(report.fourth_continued_steps, 1);
    assert_eq!(report.fifth_continued_steps, 1);
    assert_eq!(report.sixth_continued_steps, 1);
    assert_eq!(report.seventh_continued_steps, 1);
    assert_eq!(report.eighth_continued_steps, 1);
    assert_eq!(report.ninth_continued_steps, 1);
    assert_eq!(report.tenth_continued_steps, 1);
    assert_eq!(report.eleventh_continued_steps, 1);
    assert_eq!(report.twelfth_continued_steps, 1);
    assert_eq!(report.thirteenth_continued_steps, 1);
    assert_eq!(report.fourteenth_continued_steps, 1);
    assert_eq!(report.order_count(), 15);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: 1,
        }]
    );
    assert_eq!(report.fourteenth_continued_observation.len(), 80);
    assert!(
        (report.fourteenth_continued_observation[44] - 1.0).abs() < 1e-5,
        "usdm fourteenth-hop fill-backed eth q {}",
        report.fourteenth_continued_observation[44]
    );
}

#[tokio::test]
async fn policy_demo_fourteenth_continued_tape_drains_dispatch_orders() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = false;
    config.client_order_id_prefix = "unit-spot-hop15".into();
    config.captured_user_data_json = Some(format!(
        "{}\n{}",
        spot_execution_report_json("ETHUSDT", "unit-spot-hop15-spot-0000", 861, "BUY", "FILLED"),
        spot_execution_report_json("ETHUSDT", "unit-spot-hop15-spot-0001", 862, "SELL", "FILLED"),
    ));
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop15-spot-0002",
        863,
        "BUY",
        "FILLED",
    ));
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.second_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop15-spot-0003",
        864,
        "SELL",
        "FILLED",
    ));
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.third_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop15-spot-0004",
        865,
        "BUY",
        "FILLED",
    ));
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fourth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop15-spot-0005",
        866,
        "SELL",
        "FILLED",
    ));
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.fifth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop15-spot-0006",
        867,
        "BUY",
        "FILLED",
    ));
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    config.sixth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop15-spot-0007",
        868,
        "SELL",
        "FILLED",
    ));
    config.seventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"216.00","qty":"1.0"}],"asks":[{"price":"218.00","qty":"1.0"}],"update_id":15}"#
            .into(),
    );
    config.seventh_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop15-spot-0008",
        869,
        "BUY",
        "FILLED",
    ));
    config.eighth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"217.00","qty":"1.0"}],"asks":[{"price":"219.00","qty":"1.0"}],"update_id":16}"#
            .into(),
    );
    config.eighth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop15-spot-0009",
        870,
        "SELL",
        "FILLED",
    ));
    config.ninth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"218.00","qty":"1.0"}],"asks":[{"price":"220.00","qty":"1.0"}],"update_id":17}"#
            .into(),
    );
    config.ninth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop15-spot-0010",
        871,
        "BUY",
        "FILLED",
    ));
    config.tenth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"219.00","qty":"1.0"}],"asks":[{"price":"221.00","qty":"1.0"}],"update_id":18}"#
            .into(),
    );
    config.tenth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop15-spot-0011",
        872,
        "SELL",
        "FILLED",
    ));
    config.eleventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"220.00","qty":"1.0"}],"asks":[{"price":"222.00","qty":"1.0"}],"update_id":19}"#
            .into(),
    );
    config.eleventh_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop15-spot-0012",
        873,
        "BUY",
        "FILLED",
    ));
    config.twelfth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"221.00","qty":"1.0"}],"asks":[{"price":"223.00","qty":"1.0"}],"update_id":20}"#
            .into(),
    );
    config.twelfth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop15-spot-0013",
        874,
        "SELL",
        "FILLED",
    ));
    config.thirteenth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop15-spot-0014",
        875,
        "BUY",
        "FILLED",
    ));
    config.thirteenth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"222.00","qty":"1.0"}],"asks":[{"price":"224.00","qty":"1.0"}],"update_id":21}"#
            .into(),
    );
    config.fourteenth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"223.00","qty":"1.0"}],"asks":[{"price":"225.00","qty":"1.0"}],"update_id":22}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
    ]);

    let report =
        run_spot_policy_demo_with_placer(config, &policy, "fourteenth-continue-drain", |_| async {
            panic!("dry-run must not place")
        })
        .await
        .unwrap();

    assert_eq!(report.steps, 3);
    assert_eq!(report.continued_steps, 1);
    assert_eq!(report.second_continued_steps, 1);
    assert_eq!(report.third_continued_steps, 1);
    assert_eq!(report.fourth_continued_steps, 1);
    assert_eq!(report.fifth_continued_steps, 1);
    assert_eq!(report.sixth_continued_steps, 1);
    assert_eq!(report.seventh_continued_steps, 1);
    assert_eq!(report.eighth_continued_steps, 1);
    assert_eq!(report.ninth_continued_steps, 1);
    assert_eq!(report.tenth_continued_steps, 1);
    assert_eq!(report.eleventh_continued_steps, 1);
    assert_eq!(report.twelfth_continued_steps, 1);
    assert_eq!(report.thirteenth_continued_steps, 1);
    assert_eq!(report.fourteenth_continued_steps, 1);
    assert_eq!(report.placed_orders, 0);
    assert_eq!(report.order_count(), 16);
    let PolicyDemoOrders::Spot(orders) = &report.orders else {
        panic!("expected spot orders");
    };
    assert_eq!(orders[15].symbol, "ETHUSDT");
    assert_eq!(orders[15].quantity, "0.03");
    assert_eq!(orders[15].side, SpotOrderSide::Sell);
    assert_eq!(
        orders[15].new_client_order_id.as_deref(),
        Some("unit-spot-hop15-spot-0015")
    );
}

#[tokio::test]
async fn policy_demo_fourteenth_continued_tape_places_orders_when_guarded() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.client_order_id_prefix = "unit-spot-hop15p".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    config.seventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"216.00","qty":"1.0"}],"asks":[{"price":"218.00","qty":"1.0"}],"update_id":15}"#
            .into(),
    );
    config.eighth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"217.00","qty":"1.0"}],"asks":[{"price":"219.00","qty":"1.0"}],"update_id":16}"#
            .into(),
    );
    config.ninth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"218.00","qty":"1.0"}],"asks":[{"price":"220.00","qty":"1.0"}],"update_id":17}"#
            .into(),
    );
    config.tenth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"219.00","qty":"1.0"}],"asks":[{"price":"221.00","qty":"1.0"}],"update_id":18}"#
            .into(),
    );
    config.eleventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"220.00","qty":"1.0"}],"asks":[{"price":"222.00","qty":"1.0"}],"update_id":19}"#
            .into(),
    );
    config.twelfth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"221.00","qty":"1.0"}],"asks":[{"price":"223.00","qty":"1.0"}],"update_id":20}"#
            .into(),
    );
    config.thirteenth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"222.00","qty":"1.0"}],"asks":[{"price":"224.00","qty":"1.0"}],"update_id":21}"#
            .into(),
    );
    config.fourteenth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"223.00","qty":"1.0"}],"asks":[{"price":"225.00","qty":"1.0"}],"update_id":22}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
    ]);
    let frames = policy_demo_user_data_messages_from_json(&format!(
        "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
        spot_execution_report_json("ETHUSDT", "unit-spot-hop15p-spot-0000", 881, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop15p-spot-0001",
            882,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-hop15p-spot-0002", 883, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop15p-spot-0003",
            884,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-hop15p-spot-0004", 885, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop15p-spot-0005",
            886,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-hop15p-spot-0006", 887, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop15p-spot-0007",
            888,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-hop15p-spot-0008", 889, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop15p-spot-0009",
            890,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-hop15p-spot-0010", 891, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop15p-spot-0011",
            892,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-hop15p-spot-0012", 893, "BUY", "FILLED"),
    ))
    .unwrap();

    let report = run_spot_policy_demo_with_live_user_data_source(
        config,
        &policy,
        "fourteenth-continue-place",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-spot-hop15p-spot-0000" => 881,
                    "unit-spot-hop15p-spot-0001" => 882,
                    "unit-spot-hop15p-spot-0002" => 883,
                    "unit-spot-hop15p-spot-0003" => 884,
                    "unit-spot-hop15p-spot-0004" => 885,
                    "unit-spot-hop15p-spot-0005" => 886,
                    "unit-spot-hop15p-spot-0006" => 887,
                    "unit-spot-hop15p-spot-0007" => 888,
                    "unit-spot-hop15p-spot-0008" => 889,
                    "unit-spot-hop15p-spot-0009" => 890,
                    "unit-spot-hop15p-spot-0010" => 891,
                    "unit-spot-hop15p-spot-0011" => 892,
                    "unit-spot-hop15p-spot-0012" => 893,
                    "unit-spot-hop15p-spot-0013" => 894,
                    "unit-spot-hop15p-spot-0014" => 895,
                    "unit-spot-hop15p-spot-0015" => 896,
                    other => panic!("unexpected placed client order id {other}"),
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
        frames,
    )
    .await
    .unwrap();

    assert_eq!(report.twelfth_continued_steps, 1);
    assert_eq!(report.thirteenth_continued_steps, 1);
    assert_eq!(report.fourteenth_continued_steps, 1);
    assert_eq!(report.placed_orders, 16);
    assert_eq!(report.order_count(), 16);
    assert_eq!(report.receipts.len(), 16);
    assert_eq!(
        report.receipts[0].client_order_id,
        "unit-spot-hop15p-spot-0000"
    );
    assert_eq!(
        report.receipts[15].client_order_id,
        "unit-spot-hop15p-spot-0015"
    );
    assert_eq!(report.receipts[15].side, "SELL");
    let PolicyDemoOrders::Spot(orders) = &report.orders else {
        panic!("expected spot orders");
    };
    assert_eq!(
        orders[15].new_client_order_id.as_deref(),
        Some("unit-spot-hop15p-spot-0015")
    );
    assert_eq!(orders[15].side, SpotOrderSide::Sell);
}

#[tokio::test]
async fn policy_demo_spot_captured_user_data_writes_extra_inventory() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.client_order_id_prefix = "unit-spot-eth".into();
    config.captured_user_data_json = Some(format!(
        "{}\n{}",
        spot_execution_report_json("ETHUSDT", "unit-spot-eth-spot-0000", 51, "BUY", "FILLED"),
        spot_execution_report_json("ETHUSDT", "unit-spot-eth-spot-0001", 52, "SELL", "FILLED"),
    ));
    let policy = SequencePolicy::hold_buy_sell();

    let report = run_spot_policy_demo_with_placer(
        config,
        &policy,
        "captured-json-env",
        |order| async move {
            assert_eq!(order.symbol, "ETHUSDT");
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: if order.side == SpotOrderSide::Buy {
                    51
                } else {
                    52
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
    )
    .await
    .unwrap();

    assert_eq!(report.symbol, "BTCUSDT");
    assert_eq!(report.dispatch_symbol, "ETHUSDT");
    assert_eq!(report.reconciliations.len(), 2);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: -1,
        }]
    );
}

#[tokio::test]
async fn policy_demo_dry_run_captured_user_data_matches_client_ids() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.qty = "0.03".into();
    config.execute_demo_orders = false;
    config.client_order_id_prefix = "unit-spot-dry".into();
    config.captured_user_data_json = Some(format!(
        "{}\n{}",
        spot_execution_report_json("ETHUSDT", "unit-spot-dry-spot-0000", 91, "BUY", "FILLED"),
        spot_execution_report_json("ETHUSDT", "unit-spot-dry-spot-0001", 92, "SELL", "FILLED"),
    ));
    let policy = SequencePolicy::hold_buy_sell();

    let report =
        run_spot_policy_demo_with_placer(config, &policy, "dry-run-client-ids", |_| async {
            panic!("dry-run must not place")
        })
        .await
        .unwrap();

    assert!(!report.execute_demo_orders);
    assert_eq!(report.placed_orders, 0);
    assert_eq!(report.receipts.len(), 0);
    assert_eq!(report.reconciliations.len(), 2);
    assert_eq!(report.reconciliations[0].symbol, "ETHUSDT");
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: -1,
        }]
    );
    assert_eq!(report.continued_steps, 0);
    assert!(report.continued_observation.is_empty());
}

#[tokio::test]
async fn policy_demo_dry_run_continues_after_extra_symbol_fill() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = false;
    config.client_order_id_prefix = "unit-spot-cont".into();
    config.use_ladder_observation = true;
    config.captured_user_data_json = Some(format!(
        "{}\n{}",
        spot_execution_report_json("ETHUSDT", "unit-spot-cont-spot-0000", 91, "BUY", "FILLED"),
        spot_execution_report_json("ETHUSDT", "unit-spot-cont-spot-0001", 92, "SELL", "FILLED"),
    ));
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    let policy = SequencePolicy::hold_buy_sell();

    let report = run_spot_policy_demo_with_placer(config, &policy, "dry-run-continue", |_| async {
        panic!("dry-run must not place")
    })
    .await
    .unwrap();

    assert!(!report.execute_demo_orders);
    assert_eq!(report.placed_orders, 0);
    assert_eq!(report.reconciliations.len(), 2);
    assert_eq!(report.continued_steps, 1);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: -1,
        }]
    );
    // Joined ladder: default V=8 → 40-D per book; ETH q is index 44.
    assert_eq!(report.continued_observation.len(), 80);
    assert!(
        (report.continued_observation[44] + 1.0).abs() < 1e-5,
        "fill-backed eth q {}",
        report.continued_observation[44]
    );
}

#[tokio::test]
async fn policy_demo_continued_tape_drains_dispatch_orders() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = false;
    config.client_order_id_prefix = "unit-spot-more".into();
    config.captured_user_data_json = Some(format!(
        "{}\n{}",
        spot_execution_report_json("ETHUSDT", "unit-spot-more-spot-0000", 91, "BUY", "FILLED"),
        spot_execution_report_json("ETHUSDT", "unit-spot-more-spot-0001", 92, "SELL", "FILLED"),
    ));
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    let policy =
        SequencePolicy::with_actions(vec![Action::Hold, Action::Buy, Action::Sell, Action::Buy]);

    let report = run_spot_policy_demo_with_placer(config, &policy, "continue-drain", |_| async {
        panic!("dry-run must not place")
    })
    .await
    .unwrap();

    assert_eq!(report.steps, 3);
    assert_eq!(report.continued_steps, 1);
    assert_eq!(report.placed_orders, 0);
    assert_eq!(report.order_count(), 3);
    let PolicyDemoOrders::Spot(orders) = &report.orders else {
        panic!("expected spot orders");
    };
    assert_eq!(orders[2].symbol, "ETHUSDT");
    assert_eq!(orders[2].quantity, "0.03");
    assert_eq!(
        orders[2].new_client_order_id.as_deref(),
        Some("unit-spot-more-spot-0002")
    );
}

#[tokio::test]
async fn policy_demo_continued_tape_places_orders_when_guarded() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.client_order_id_prefix = "unit-spot-place".into();
    config.captured_user_data_json = Some(format!(
        "{}\n{}",
        spot_execution_report_json("ETHUSDT", "unit-spot-place-spot-0000", 91, "BUY", "FILLED"),
        spot_execution_report_json("ETHUSDT", "unit-spot-place-spot-0001", 92, "SELL", "FILLED"),
    ));
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    let policy =
        SequencePolicy::with_actions(vec![Action::Hold, Action::Buy, Action::Sell, Action::Buy]);

    let report =
        run_spot_policy_demo_with_placer(config, &policy, "continue-place", |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-spot-place-spot-0000" => 101,
                    "unit-spot-place-spot-0001" => 102,
                    "unit-spot-place-spot-0002" => 103,
                    other => panic!("unexpected client order id {other}"),
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
        })
        .await
        .unwrap();

    assert_eq!(report.steps, 3);
    assert_eq!(report.continued_steps, 1);
    assert_eq!(report.order_count(), 3);
    assert_eq!(report.placed_orders, 3);
    assert_eq!(report.receipts.len(), 3);
    assert_eq!(report.receipts[0].order_id, 101);
    assert_eq!(
        report.receipts[0].client_order_id,
        "unit-spot-place-spot-0000"
    );
    assert_eq!(report.receipts[0].side, "BUY");
    assert_eq!(report.receipts[1].order_id, 102);
    assert_eq!(
        report.receipts[1].client_order_id,
        "unit-spot-place-spot-0001"
    );
    assert_eq!(report.receipts[1].side, "SELL");
    assert_eq!(report.receipts[2].order_id, 103);
    assert_eq!(
        report.receipts[2].client_order_id,
        "unit-spot-place-spot-0002"
    );
    assert_eq!(report.receipts[2].side, "BUY");
    assert_eq!(report.receipts[2].symbol, "ETHUSDT");
}

#[tokio::test]
async fn policy_demo_continued_tape_reconciles_placed_receipts() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.client_order_id_prefix = "unit-spot-cont-rec".into();
    config.captured_user_data_json = Some(format!(
        "{}\n{}",
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-cont-rec-spot-0000",
            91,
            "BUY",
            "FILLED"
        ),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-cont-rec-spot-0001",
            92,
            "SELL",
            "FILLED"
        ),
    ));
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-cont-rec-spot-0002",
        103,
        "BUY",
        "FILLED",
    ));
    let policy =
        SequencePolicy::with_actions(vec![Action::Hold, Action::Buy, Action::Sell, Action::Buy]);

    let report = run_spot_policy_demo_with_placer(
        config,
        &policy,
        "continue-reconcile",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-spot-cont-rec-spot-0000" => 101,
                    "unit-spot-cont-rec-spot-0001" => 102,
                    "unit-spot-cont-rec-spot-0002" => 103,
                    other => panic!("unexpected client order id {other}"),
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
    )
    .await
    .unwrap();

    assert_eq!(report.placed_orders, 3);
    assert_eq!(report.receipts.len(), 3);
    assert_eq!(
        report.receipts[0].client_order_id,
        "unit-spot-cont-rec-spot-0000"
    );
    assert_eq!(
        report.receipts[2].client_order_id,
        "unit-spot-cont-rec-spot-0002"
    );
    assert_eq!(report.reconciliations.len(), 3);
    assert_eq!(
        report.reconciliations[0].client_order_id,
        "unit-spot-cont-rec-spot-0000"
    );
    assert_eq!(
        report.reconciliations[2].client_order_id,
        "unit-spot-cont-rec-spot-0002"
    );
    assert_eq!(report.reconciliations[2].status, "FILLED");
    assert!(report.reconciliations[2].terminal);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: 1,
        }]
    );
}

#[tokio::test]
async fn policy_demo_dry_run_continued_user_data_matches_assigned_ids() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = false;
    config.client_order_id_prefix = "unit-spot-cont-dry".into();
    config.captured_user_data_json = Some(format!(
        "{}\n{}",
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-cont-dry-spot-0000",
            91,
            "BUY",
            "FILLED"
        ),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-cont-dry-spot-0001",
            92,
            "SELL",
            "FILLED"
        ),
    ));
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-cont-dry-spot-0002",
        93,
        "BUY",
        "FILLED",
    ));
    let policy =
        SequencePolicy::with_actions(vec![Action::Hold, Action::Buy, Action::Sell, Action::Buy]);

    let report =
        run_spot_policy_demo_with_placer(config, &policy, "continue-dry-reconcile", |_| async {
            panic!("dry-run must not place")
        })
        .await
        .unwrap();

    assert!(!report.execute_demo_orders);
    assert_eq!(report.placed_orders, 0);
    assert!(report.receipts.is_empty());
    assert_eq!(report.order_count(), 3);
    assert_eq!(report.reconciliations.len(), 3);
    assert_eq!(
        report.reconciliations[2].client_order_id,
        "unit-spot-cont-dry-spot-0002"
    );
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: 1,
        }]
    );
}

#[tokio::test]
async fn policy_demo_second_continued_tape_reconciles_placed_receipts() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.client_order_id_prefix = "unit-spot-hop3-rec".into();
    config.captured_user_data_json = Some(format!(
        "{}\n{}",
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop3-rec-spot-0000",
            151,
            "BUY",
            "FILLED"
        ),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop3-rec-spot-0001",
            152,
            "SELL",
            "FILLED"
        ),
    ));
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop3-rec-spot-0002",
        153,
        "BUY",
        "FILLED",
    ));
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.second_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop3-rec-spot-0003",
        154,
        "SELL",
        "FILLED",
    ));
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
    ]);

    let report = run_spot_policy_demo_with_placer(
        config,
        &policy,
        "second-continue-reconcile",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-spot-hop3-rec-spot-0000" => 151,
                    "unit-spot-hop3-rec-spot-0001" => 152,
                    "unit-spot-hop3-rec-spot-0002" => 153,
                    "unit-spot-hop3-rec-spot-0003" => 154,
                    other => panic!("unexpected client order id {other}"),
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
    )
    .await
    .unwrap();

    assert_eq!(report.second_continued_steps, 1);
    assert_eq!(report.placed_orders, 4);
    assert_eq!(report.receipts.len(), 4);
    assert_eq!(
        report.receipts[0].client_order_id,
        "unit-spot-hop3-rec-spot-0000"
    );
    assert_eq!(
        report.receipts[2].client_order_id,
        "unit-spot-hop3-rec-spot-0002"
    );
    assert_eq!(
        report.receipts[3].client_order_id,
        "unit-spot-hop3-rec-spot-0003"
    );
    assert_eq!(report.receipts[3].side, "SELL");
    assert_eq!(report.reconciliations.len(), 4);
    assert_eq!(
        report.reconciliations[0].client_order_id,
        "unit-spot-hop3-rec-spot-0000"
    );
    assert_eq!(
        report.reconciliations[2].client_order_id,
        "unit-spot-hop3-rec-spot-0002"
    );
    assert_eq!(
        report.reconciliations[3].client_order_id,
        "unit-spot-hop3-rec-spot-0003"
    );
    assert_eq!(report.reconciliations[3].status, "FILLED");
    assert!(report.reconciliations[3].terminal);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: -1,
        }]
    );
}

#[tokio::test]
async fn policy_demo_dry_run_second_continued_user_data_matches_assigned_ids() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = false;
    config.client_order_id_prefix = "unit-spot-hop3-dry".into();
    config.captured_user_data_json = Some(format!(
        "{}\n{}",
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop3-dry-spot-0000",
            161,
            "BUY",
            "FILLED"
        ),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop3-dry-spot-0001",
            162,
            "SELL",
            "FILLED"
        ),
    ));
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop3-dry-spot-0002",
        163,
        "BUY",
        "FILLED",
    ));
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.second_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop3-dry-spot-0003",
        164,
        "SELL",
        "FILLED",
    ));
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
    ]);

    let report = run_spot_policy_demo_with_placer(
        config,
        &policy,
        "second-continue-dry-reconcile",
        |_| async { panic!("dry-run must not place") },
    )
    .await
    .unwrap();

    assert!(!report.execute_demo_orders);
    assert_eq!(report.placed_orders, 0);
    assert!(report.receipts.is_empty());
    assert_eq!(report.order_count(), 4);
    assert_eq!(report.second_continued_steps, 1);
    assert_eq!(report.reconciliations.len(), 4);
    assert_eq!(
        report.reconciliations[2].client_order_id,
        "unit-spot-hop3-dry-spot-0002"
    );
    assert_eq!(
        report.reconciliations[3].client_order_id,
        "unit-spot-hop3-dry-spot-0003"
    );
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: -1,
        }]
    );
}

#[tokio::test]
async fn policy_demo_third_continued_tape_reconciles_placed_receipts() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.client_order_id_prefix = "unit-spot-hop4-rec".into();
    config.captured_user_data_json = Some(format!(
        "{}\n{}",
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop4-rec-spot-0000",
            191,
            "BUY",
            "FILLED"
        ),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop4-rec-spot-0001",
            192,
            "SELL",
            "FILLED"
        ),
    ));
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop4-rec-spot-0002",
        193,
        "BUY",
        "FILLED",
    ));
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.second_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop4-rec-spot-0003",
        194,
        "SELL",
        "FILLED",
    ));
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.third_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop4-rec-spot-0004",
        195,
        "BUY",
        "FILLED",
    ));
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
    ]);

    let report = run_spot_policy_demo_with_placer(
        config,
        &policy,
        "third-continue-reconcile",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-spot-hop4-rec-spot-0000" => 191,
                    "unit-spot-hop4-rec-spot-0001" => 192,
                    "unit-spot-hop4-rec-spot-0002" => 193,
                    "unit-spot-hop4-rec-spot-0003" => 194,
                    "unit-spot-hop4-rec-spot-0004" => 195,
                    other => panic!("unexpected client order id {other}"),
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
    )
    .await
    .unwrap();

    assert_eq!(report.third_continued_steps, 1);
    assert_eq!(report.placed_orders, 5);
    assert_eq!(report.receipts.len(), 5);
    assert_eq!(
        report.receipts[0].client_order_id,
        "unit-spot-hop4-rec-spot-0000"
    );
    assert_eq!(
        report.receipts[3].client_order_id,
        "unit-spot-hop4-rec-spot-0003"
    );
    assert_eq!(
        report.receipts[4].client_order_id,
        "unit-spot-hop4-rec-spot-0004"
    );
    assert_eq!(report.receipts[4].side, "BUY");
    assert_eq!(report.reconciliations.len(), 5);
    assert_eq!(
        report.reconciliations[0].client_order_id,
        "unit-spot-hop4-rec-spot-0000"
    );
    assert_eq!(
        report.reconciliations[2].client_order_id,
        "unit-spot-hop4-rec-spot-0002"
    );
    assert_eq!(
        report.reconciliations[3].client_order_id,
        "unit-spot-hop4-rec-spot-0003"
    );
    assert_eq!(
        report.reconciliations[4].client_order_id,
        "unit-spot-hop4-rec-spot-0004"
    );
    assert_eq!(report.reconciliations[4].status, "FILLED");
    assert!(report.reconciliations[4].terminal);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: 1,
        }]
    );
}

#[tokio::test]
async fn policy_demo_dry_run_third_continued_user_data_matches_assigned_ids() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = false;
    config.client_order_id_prefix = "unit-spot-hop4-dry".into();
    config.captured_user_data_json = Some(format!(
        "{}\n{}",
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop4-dry-spot-0000",
            201,
            "BUY",
            "FILLED"
        ),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop4-dry-spot-0001",
            202,
            "SELL",
            "FILLED"
        ),
    ));
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop4-dry-spot-0002",
        203,
        "BUY",
        "FILLED",
    ));
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.second_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop4-dry-spot-0003",
        204,
        "SELL",
        "FILLED",
    ));
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.third_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop4-dry-spot-0004",
        205,
        "BUY",
        "FILLED",
    ));
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
    ]);

    let report = run_spot_policy_demo_with_placer(
        config,
        &policy,
        "third-continue-dry-reconcile",
        |_| async { panic!("dry-run must not place") },
    )
    .await
    .unwrap();

    assert!(!report.execute_demo_orders);
    assert_eq!(report.placed_orders, 0);
    assert!(report.receipts.is_empty());
    assert_eq!(report.order_count(), 5);
    assert_eq!(report.third_continued_steps, 1);
    assert_eq!(report.reconciliations.len(), 5);
    assert_eq!(
        report.reconciliations[2].client_order_id,
        "unit-spot-hop4-dry-spot-0002"
    );
    assert_eq!(
        report.reconciliations[3].client_order_id,
        "unit-spot-hop4-dry-spot-0003"
    );
    assert_eq!(
        report.reconciliations[4].client_order_id,
        "unit-spot-hop4-dry-spot-0004"
    );
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: 1,
        }]
    );
}

#[tokio::test]
async fn policy_demo_fourth_continued_tape_reconciles_placed_receipts() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.client_order_id_prefix = "unit-spot-hop5-rec".into();
    config.captured_user_data_json = Some(format!(
        "{}\n{}",
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop5-rec-spot-0000",
            211,
            "BUY",
            "FILLED"
        ),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop5-rec-spot-0001",
            212,
            "SELL",
            "FILLED"
        ),
    ));
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop5-rec-spot-0002",
        213,
        "BUY",
        "FILLED",
    ));
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.second_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop5-rec-spot-0003",
        214,
        "SELL",
        "FILLED",
    ));
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.third_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop5-rec-spot-0004",
        215,
        "BUY",
        "FILLED",
    ));
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fourth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop5-rec-spot-0005",
        216,
        "SELL",
        "FILLED",
    ));
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
    ]);

    let report = run_spot_policy_demo_with_placer(
        config,
        &policy,
        "fourth-continue-reconcile",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-spot-hop5-rec-spot-0000" => 211,
                    "unit-spot-hop5-rec-spot-0001" => 212,
                    "unit-spot-hop5-rec-spot-0002" => 213,
                    "unit-spot-hop5-rec-spot-0003" => 214,
                    "unit-spot-hop5-rec-spot-0004" => 215,
                    "unit-spot-hop5-rec-spot-0005" => 216,
                    other => panic!("unexpected client order id {other}"),
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
    )
    .await
    .unwrap();

    assert_eq!(report.fourth_continued_steps, 1);
    assert_eq!(report.placed_orders, 6);
    assert_eq!(report.receipts.len(), 6);
    assert_eq!(
        report.receipts[0].client_order_id,
        "unit-spot-hop5-rec-spot-0000"
    );
    assert_eq!(
        report.receipts[4].client_order_id,
        "unit-spot-hop5-rec-spot-0004"
    );
    assert_eq!(
        report.receipts[5].client_order_id,
        "unit-spot-hop5-rec-spot-0005"
    );
    assert_eq!(report.receipts[5].side, "SELL");
    assert_eq!(report.reconciliations.len(), 6);
    assert_eq!(
        report.reconciliations[0].client_order_id,
        "unit-spot-hop5-rec-spot-0000"
    );
    assert_eq!(
        report.reconciliations[3].client_order_id,
        "unit-spot-hop5-rec-spot-0003"
    );
    assert_eq!(
        report.reconciliations[4].client_order_id,
        "unit-spot-hop5-rec-spot-0004"
    );
    assert_eq!(
        report.reconciliations[5].client_order_id,
        "unit-spot-hop5-rec-spot-0005"
    );
    assert_eq!(report.reconciliations[5].status, "FILLED");
    assert!(report.reconciliations[5].terminal);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: -1,
        }]
    );
}

#[tokio::test]
async fn policy_demo_dry_run_fourth_continued_user_data_matches_assigned_ids() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = false;
    config.client_order_id_prefix = "unit-spot-hop5-dry".into();
    config.captured_user_data_json = Some(format!(
        "{}\n{}",
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop5-dry-spot-0000",
            221,
            "BUY",
            "FILLED"
        ),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop5-dry-spot-0001",
            222,
            "SELL",
            "FILLED"
        ),
    ));
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop5-dry-spot-0002",
        223,
        "BUY",
        "FILLED",
    ));
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.second_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop5-dry-spot-0003",
        224,
        "SELL",
        "FILLED",
    ));
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.third_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop5-dry-spot-0004",
        225,
        "BUY",
        "FILLED",
    ));
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fourth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop5-dry-spot-0005",
        226,
        "SELL",
        "FILLED",
    ));
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
    ]);

    let report = run_spot_policy_demo_with_placer(
        config,
        &policy,
        "fourth-continue-dry-reconcile",
        |_| async { panic!("dry-run must not place") },
    )
    .await
    .unwrap();

    assert!(!report.execute_demo_orders);
    assert_eq!(report.placed_orders, 0);
    assert!(report.receipts.is_empty());
    assert_eq!(report.order_count(), 6);
    assert_eq!(report.fourth_continued_steps, 1);
    assert_eq!(report.reconciliations.len(), 6);
    assert_eq!(
        report.reconciliations[2].client_order_id,
        "unit-spot-hop5-dry-spot-0002"
    );
    assert_eq!(
        report.reconciliations[4].client_order_id,
        "unit-spot-hop5-dry-spot-0004"
    );
    assert_eq!(
        report.reconciliations[5].client_order_id,
        "unit-spot-hop5-dry-spot-0005"
    );
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: -1,
        }]
    );
}

#[tokio::test]
async fn policy_demo_fifth_continued_tape_reconciles_placed_receipts() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.client_order_id_prefix = "unit-spot-hop6-rec".into();
    config.captured_user_data_json = Some(format!(
        "{}\n{}",
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop6-rec-spot-0000",
            261,
            "BUY",
            "FILLED"
        ),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop6-rec-spot-0001",
            262,
            "SELL",
            "FILLED"
        ),
    ));
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop6-rec-spot-0002",
        263,
        "BUY",
        "FILLED",
    ));
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.second_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop6-rec-spot-0003",
        264,
        "SELL",
        "FILLED",
    ));
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.third_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop6-rec-spot-0004",
        265,
        "BUY",
        "FILLED",
    ));
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fourth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop6-rec-spot-0005",
        266,
        "SELL",
        "FILLED",
    ));
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.fifth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop6-rec-spot-0006",
        267,
        "BUY",
        "FILLED",
    ));
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
    ]);

    let report = run_spot_policy_demo_with_placer(
        config,
        &policy,
        "fifth-continue-reconcile",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-spot-hop6-rec-spot-0000" => 261,
                    "unit-spot-hop6-rec-spot-0001" => 262,
                    "unit-spot-hop6-rec-spot-0002" => 263,
                    "unit-spot-hop6-rec-spot-0003" => 264,
                    "unit-spot-hop6-rec-spot-0004" => 265,
                    "unit-spot-hop6-rec-spot-0005" => 266,
                    "unit-spot-hop6-rec-spot-0006" => 267,
                    other => panic!("unexpected client order id {other}"),
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
    )
    .await
    .unwrap();

    assert_eq!(report.fifth_continued_steps, 1);
    assert_eq!(report.placed_orders, 7);
    assert_eq!(report.receipts.len(), 7);
    assert_eq!(
        report.receipts[0].client_order_id,
        "unit-spot-hop6-rec-spot-0000"
    );
    assert_eq!(
        report.receipts[5].client_order_id,
        "unit-spot-hop6-rec-spot-0005"
    );
    assert_eq!(
        report.receipts[6].client_order_id,
        "unit-spot-hop6-rec-spot-0006"
    );
    assert_eq!(report.receipts[6].side, "BUY");
    assert_eq!(report.reconciliations.len(), 7);
    assert_eq!(
        report.reconciliations[0].client_order_id,
        "unit-spot-hop6-rec-spot-0000"
    );
    assert_eq!(
        report.reconciliations[4].client_order_id,
        "unit-spot-hop6-rec-spot-0004"
    );
    assert_eq!(
        report.reconciliations[5].client_order_id,
        "unit-spot-hop6-rec-spot-0005"
    );
    assert_eq!(
        report.reconciliations[6].client_order_id,
        "unit-spot-hop6-rec-spot-0006"
    );
    assert_eq!(report.reconciliations[6].status, "FILLED");
    assert!(report.reconciliations[6].terminal);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: 1,
        }]
    );
}

#[tokio::test]
async fn policy_demo_dry_run_fifth_continued_user_data_matches_assigned_ids() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = false;
    config.client_order_id_prefix = "unit-spot-hop6-dry".into();
    config.captured_user_data_json = Some(format!(
        "{}\n{}",
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop6-dry-spot-0000",
            271,
            "BUY",
            "FILLED"
        ),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop6-dry-spot-0001",
            272,
            "SELL",
            "FILLED"
        ),
    ));
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop6-dry-spot-0002",
        273,
        "BUY",
        "FILLED",
    ));
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.second_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop6-dry-spot-0003",
        274,
        "SELL",
        "FILLED",
    ));
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.third_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop6-dry-spot-0004",
        275,
        "BUY",
        "FILLED",
    ));
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fourth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop6-dry-spot-0005",
        276,
        "SELL",
        "FILLED",
    ));
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.fifth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop6-dry-spot-0006",
        277,
        "BUY",
        "FILLED",
    ));
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
    ]);

    let report = run_spot_policy_demo_with_placer(
        config,
        &policy,
        "fifth-continue-dry-reconcile",
        |_| async { panic!("dry-run must not place") },
    )
    .await
    .unwrap();

    assert!(!report.execute_demo_orders);
    assert_eq!(report.placed_orders, 0);
    assert!(report.receipts.is_empty());
    assert_eq!(report.order_count(), 7);
    assert_eq!(report.fifth_continued_steps, 1);
    assert_eq!(report.reconciliations.len(), 7);
    assert_eq!(
        report.reconciliations[2].client_order_id,
        "unit-spot-hop6-dry-spot-0002"
    );
    assert_eq!(
        report.reconciliations[5].client_order_id,
        "unit-spot-hop6-dry-spot-0005"
    );
    assert_eq!(
        report.reconciliations[6].client_order_id,
        "unit-spot-hop6-dry-spot-0006"
    );
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: 1,
        }]
    );
}

#[tokio::test]
async fn policy_demo_sixth_continued_tape_reconciles_placed_receipts() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.client_order_id_prefix = "unit-spot-hop7-rec".into();
    config.captured_user_data_json = Some(format!(
        "{}\n{}",
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop7-rec-spot-0000",
            311,
            "BUY",
            "FILLED"
        ),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop7-rec-spot-0001",
            312,
            "SELL",
            "FILLED"
        ),
    ));
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop7-rec-spot-0002",
        313,
        "BUY",
        "FILLED",
    ));
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.second_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop7-rec-spot-0003",
        314,
        "SELL",
        "FILLED",
    ));
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.third_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop7-rec-spot-0004",
        315,
        "BUY",
        "FILLED",
    ));
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fourth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop7-rec-spot-0005",
        316,
        "SELL",
        "FILLED",
    ));
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.fifth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop7-rec-spot-0006",
        317,
        "BUY",
        "FILLED",
    ));
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    config.sixth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop7-rec-spot-0007",
        318,
        "SELL",
        "FILLED",
    ));
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
    ]);

    let report = run_spot_policy_demo_with_placer(
        config,
        &policy,
        "sixth-continue-reconcile",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-spot-hop7-rec-spot-0000" => 311,
                    "unit-spot-hop7-rec-spot-0001" => 312,
                    "unit-spot-hop7-rec-spot-0002" => 313,
                    "unit-spot-hop7-rec-spot-0003" => 314,
                    "unit-spot-hop7-rec-spot-0004" => 315,
                    "unit-spot-hop7-rec-spot-0005" => 316,
                    "unit-spot-hop7-rec-spot-0006" => 317,
                    "unit-spot-hop7-rec-spot-0007" => 318,
                    other => panic!("unexpected client order id {other}"),
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
    )
    .await
    .unwrap();

    assert_eq!(report.sixth_continued_steps, 1);
    assert_eq!(report.seventh_continued_steps, 0);
    assert_eq!(report.eighth_continued_steps, 0);
    assert_eq!(report.ninth_continued_steps, 0);
    assert_eq!(report.tenth_continued_steps, 0);
    assert_eq!(report.eleventh_continued_steps, 0);
    assert_eq!(report.twelfth_continued_steps, 0);
    assert_eq!(report.thirteenth_continued_steps, 0);
    assert_eq!(report.fourteenth_continued_steps, 0);
    assert_eq!(report.placed_orders, 8);
    assert_eq!(report.receipts.len(), 8);
    assert_eq!(
        report.receipts[0].client_order_id,
        "unit-spot-hop7-rec-spot-0000"
    );
    assert_eq!(
        report.receipts[6].client_order_id,
        "unit-spot-hop7-rec-spot-0006"
    );
    assert_eq!(
        report.receipts[7].client_order_id,
        "unit-spot-hop7-rec-spot-0007"
    );
    assert_eq!(report.receipts[7].side, "SELL");
    assert_eq!(report.reconciliations.len(), 8);
    assert_eq!(
        report.reconciliations[0].client_order_id,
        "unit-spot-hop7-rec-spot-0000"
    );
    assert_eq!(
        report.reconciliations[5].client_order_id,
        "unit-spot-hop7-rec-spot-0005"
    );
    assert_eq!(
        report.reconciliations[6].client_order_id,
        "unit-spot-hop7-rec-spot-0006"
    );
    assert_eq!(
        report.reconciliations[7].client_order_id,
        "unit-spot-hop7-rec-spot-0007"
    );
    assert_eq!(report.reconciliations[7].status, "FILLED");
    assert!(report.reconciliations[7].terminal);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: -1,
        }]
    );
}

#[tokio::test]
async fn policy_demo_dry_run_sixth_continued_user_data_matches_assigned_ids() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = false;
    config.client_order_id_prefix = "unit-spot-hop7-dry".into();
    config.captured_user_data_json = Some(format!(
        "{}\n{}",
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop7-dry-spot-0000",
            321,
            "BUY",
            "FILLED"
        ),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop7-dry-spot-0001",
            322,
            "SELL",
            "FILLED"
        ),
    ));
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop7-dry-spot-0002",
        323,
        "BUY",
        "FILLED",
    ));
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.second_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop7-dry-spot-0003",
        324,
        "SELL",
        "FILLED",
    ));
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.third_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop7-dry-spot-0004",
        325,
        "BUY",
        "FILLED",
    ));
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fourth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop7-dry-spot-0005",
        326,
        "SELL",
        "FILLED",
    ));
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.fifth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop7-dry-spot-0006",
        327,
        "BUY",
        "FILLED",
    ));
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    config.sixth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop7-dry-spot-0007",
        328,
        "SELL",
        "FILLED",
    ));
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
    ]);

    let report = run_spot_policy_demo_with_placer(
        config,
        &policy,
        "sixth-continue-dry-reconcile",
        |_| async { panic!("dry-run must not place") },
    )
    .await
    .unwrap();

    assert!(!report.execute_demo_orders);
    assert_eq!(report.placed_orders, 0);
    assert!(report.receipts.is_empty());
    assert_eq!(report.order_count(), 8);
    assert_eq!(report.sixth_continued_steps, 1);
    assert_eq!(report.seventh_continued_steps, 0);
    assert_eq!(report.eighth_continued_steps, 0);
    assert_eq!(report.ninth_continued_steps, 0);
    assert_eq!(report.tenth_continued_steps, 0);
    assert_eq!(report.eleventh_continued_steps, 0);
    assert_eq!(report.twelfth_continued_steps, 0);
    assert_eq!(report.thirteenth_continued_steps, 0);
    assert_eq!(report.fourteenth_continued_steps, 0);
    assert_eq!(report.reconciliations.len(), 8);
    assert_eq!(
        report.reconciliations[2].client_order_id,
        "unit-spot-hop7-dry-spot-0002"
    );
    assert_eq!(
        report.reconciliations[6].client_order_id,
        "unit-spot-hop7-dry-spot-0006"
    );
    assert_eq!(
        report.reconciliations[7].client_order_id,
        "unit-spot-hop7-dry-spot-0007"
    );
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: -1,
        }]
    );
}

#[tokio::test]
async fn policy_demo_seventh_continued_tape_reconciles_placed_receipts() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.client_order_id_prefix = "unit-spot-hop8-rec".into();
    config.captured_user_data_json = Some(format!(
        "{}\n{}",
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop8-rec-spot-0000",
            331,
            "BUY",
            "FILLED"
        ),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop8-rec-spot-0001",
            332,
            "SELL",
            "FILLED"
        ),
    ));
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop8-rec-spot-0002",
        333,
        "BUY",
        "FILLED",
    ));
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.second_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop8-rec-spot-0003",
        334,
        "SELL",
        "FILLED",
    ));
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.third_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop8-rec-spot-0004",
        335,
        "BUY",
        "FILLED",
    ));
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fourth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop8-rec-spot-0005",
        336,
        "SELL",
        "FILLED",
    ));
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.fifth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop8-rec-spot-0006",
        337,
        "BUY",
        "FILLED",
    ));
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    config.sixth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop8-rec-spot-0007",
        338,
        "SELL",
        "FILLED",
    ));
    config.seventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"216.00","qty":"1.0"}],"asks":[{"price":"218.00","qty":"1.0"}],"update_id":15}"#
            .into(),
    );
    config.seventh_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop8-rec-spot-0008",
        339,
        "BUY",
        "FILLED",
    ));
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
    ]);

    let report = run_spot_policy_demo_with_placer(
        config,
        &policy,
        "seventh-continue-reconcile",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-spot-hop8-rec-spot-0000" => 331,
                    "unit-spot-hop8-rec-spot-0001" => 332,
                    "unit-spot-hop8-rec-spot-0002" => 333,
                    "unit-spot-hop8-rec-spot-0003" => 334,
                    "unit-spot-hop8-rec-spot-0004" => 335,
                    "unit-spot-hop8-rec-spot-0005" => 336,
                    "unit-spot-hop8-rec-spot-0006" => 337,
                    "unit-spot-hop8-rec-spot-0007" => 338,
                    "unit-spot-hop8-rec-spot-0008" => 339,
                    other => panic!("unexpected client order id {other}"),
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
    )
    .await
    .unwrap();

    assert_eq!(report.seventh_continued_steps, 1);
    assert_eq!(report.eighth_continued_steps, 0);
    assert_eq!(report.ninth_continued_steps, 0);
    assert_eq!(report.tenth_continued_steps, 0);
    assert_eq!(report.eleventh_continued_steps, 0);
    assert_eq!(report.twelfth_continued_steps, 0);
    assert_eq!(report.thirteenth_continued_steps, 0);
    assert_eq!(report.fourteenth_continued_steps, 0);
    assert_eq!(report.placed_orders, 9);
    assert_eq!(report.receipts.len(), 9);
    assert_eq!(
        report.receipts[0].client_order_id,
        "unit-spot-hop8-rec-spot-0000"
    );
    assert_eq!(
        report.receipts[7].client_order_id,
        "unit-spot-hop8-rec-spot-0007"
    );
    assert_eq!(
        report.receipts[8].client_order_id,
        "unit-spot-hop8-rec-spot-0008"
    );
    assert_eq!(report.receipts[8].side, "BUY");
    assert_eq!(report.reconciliations.len(), 9);
    assert_eq!(
        report.reconciliations[0].client_order_id,
        "unit-spot-hop8-rec-spot-0000"
    );
    assert_eq!(
        report.reconciliations[6].client_order_id,
        "unit-spot-hop8-rec-spot-0006"
    );
    assert_eq!(
        report.reconciliations[7].client_order_id,
        "unit-spot-hop8-rec-spot-0007"
    );
    assert_eq!(
        report.reconciliations[8].client_order_id,
        "unit-spot-hop8-rec-spot-0008"
    );
    assert_eq!(report.reconciliations[8].status, "FILLED");
    assert!(report.reconciliations[8].terminal);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: 1,
        }]
    );
}

#[tokio::test]
async fn policy_demo_dry_run_seventh_continued_user_data_matches_assigned_ids() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = false;
    config.client_order_id_prefix = "unit-spot-hop8-dry".into();
    config.captured_user_data_json = Some(format!(
        "{}\n{}",
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop8-dry-spot-0000",
            341,
            "BUY",
            "FILLED"
        ),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop8-dry-spot-0001",
            342,
            "SELL",
            "FILLED"
        ),
    ));
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop8-dry-spot-0002",
        343,
        "BUY",
        "FILLED",
    ));
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.second_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop8-dry-spot-0003",
        344,
        "SELL",
        "FILLED",
    ));
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.third_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop8-dry-spot-0004",
        345,
        "BUY",
        "FILLED",
    ));
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fourth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop8-dry-spot-0005",
        346,
        "SELL",
        "FILLED",
    ));
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.fifth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop8-dry-spot-0006",
        347,
        "BUY",
        "FILLED",
    ));
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    config.sixth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop8-dry-spot-0007",
        348,
        "SELL",
        "FILLED",
    ));
    config.seventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"216.00","qty":"1.0"}],"asks":[{"price":"218.00","qty":"1.0"}],"update_id":15}"#
            .into(),
    );
    config.seventh_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop8-dry-spot-0008",
        349,
        "BUY",
        "FILLED",
    ));
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
    ]);

    let report = run_spot_policy_demo_with_placer(
        config,
        &policy,
        "seventh-continue-dry-reconcile",
        |_| async { panic!("dry-run must not place") },
    )
    .await
    .unwrap();

    assert!(!report.execute_demo_orders);
    assert_eq!(report.placed_orders, 0);
    assert!(report.receipts.is_empty());
    assert_eq!(report.order_count(), 9);
    assert_eq!(report.seventh_continued_steps, 1);
    assert_eq!(report.eighth_continued_steps, 0);
    assert_eq!(report.ninth_continued_steps, 0);
    assert_eq!(report.tenth_continued_steps, 0);
    assert_eq!(report.eleventh_continued_steps, 0);
    assert_eq!(report.twelfth_continued_steps, 0);
    assert_eq!(report.thirteenth_continued_steps, 0);
    assert_eq!(report.fourteenth_continued_steps, 0);
    assert_eq!(report.reconciliations.len(), 9);
    assert_eq!(
        report.reconciliations[2].client_order_id,
        "unit-spot-hop8-dry-spot-0002"
    );
    assert_eq!(
        report.reconciliations[7].client_order_id,
        "unit-spot-hop8-dry-spot-0007"
    );
    assert_eq!(
        report.reconciliations[8].client_order_id,
        "unit-spot-hop8-dry-spot-0008"
    );
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: 1,
        }]
    );
}

#[tokio::test]
async fn policy_demo_eighth_continued_tape_reconciles_placed_receipts() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.client_order_id_prefix = "unit-spot-hop9-rec".into();
    config.captured_user_data_json = Some(format!(
        "{}\n{}",
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop9-rec-spot-0000",
            401,
            "BUY",
            "FILLED"
        ),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop9-rec-spot-0001",
            402,
            "SELL",
            "FILLED"
        ),
    ));
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop9-rec-spot-0002",
        403,
        "BUY",
        "FILLED",
    ));
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.second_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop9-rec-spot-0003",
        404,
        "SELL",
        "FILLED",
    ));
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.third_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop9-rec-spot-0004",
        405,
        "BUY",
        "FILLED",
    ));
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fourth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop9-rec-spot-0005",
        406,
        "SELL",
        "FILLED",
    ));
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.fifth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop9-rec-spot-0006",
        407,
        "BUY",
        "FILLED",
    ));
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    config.sixth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop9-rec-spot-0007",
        408,
        "SELL",
        "FILLED",
    ));
    config.seventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"216.00","qty":"1.0"}],"asks":[{"price":"218.00","qty":"1.0"}],"update_id":15}"#
            .into(),
    );
    config.seventh_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop9-rec-spot-0008",
        409,
        "BUY",
        "FILLED",
    ));
    config.eighth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"217.00","qty":"1.0"}],"asks":[{"price":"219.00","qty":"1.0"}],"update_id":16}"#
            .into(),
    );
    config.eighth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop9-rec-spot-0009",
        410,
        "SELL",
        "FILLED",
    ));
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
    ]);

    let report = run_spot_policy_demo_with_placer(
        config,
        &policy,
        "eighth-continue-reconcile",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-spot-hop9-rec-spot-0000" => 401,
                    "unit-spot-hop9-rec-spot-0001" => 402,
                    "unit-spot-hop9-rec-spot-0002" => 403,
                    "unit-spot-hop9-rec-spot-0003" => 404,
                    "unit-spot-hop9-rec-spot-0004" => 405,
                    "unit-spot-hop9-rec-spot-0005" => 406,
                    "unit-spot-hop9-rec-spot-0006" => 407,
                    "unit-spot-hop9-rec-spot-0007" => 408,
                    "unit-spot-hop9-rec-spot-0008" => 409,
                    "unit-spot-hop9-rec-spot-0009" => 410,
                    other => panic!("unexpected client order id {other}"),
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
    )
    .await
    .unwrap();

    assert_eq!(report.seventh_continued_steps, 1);
    assert_eq!(report.eighth_continued_steps, 1);
    assert_eq!(report.ninth_continued_steps, 0);
    assert_eq!(report.tenth_continued_steps, 0);
    assert_eq!(report.eleventh_continued_steps, 0);
    assert_eq!(report.twelfth_continued_steps, 0);
    assert_eq!(report.thirteenth_continued_steps, 0);
    assert_eq!(report.fourteenth_continued_steps, 0);
    assert_eq!(report.placed_orders, 10);
    assert_eq!(report.receipts.len(), 10);
    assert_eq!(
        report.receipts[0].client_order_id,
        "unit-spot-hop9-rec-spot-0000"
    );
    assert_eq!(
        report.receipts[8].client_order_id,
        "unit-spot-hop9-rec-spot-0008"
    );
    assert_eq!(
        report.receipts[9].client_order_id,
        "unit-spot-hop9-rec-spot-0009"
    );
    assert_eq!(report.receipts[9].side, "SELL");
    assert_eq!(report.reconciliations.len(), 10);
    assert_eq!(
        report.reconciliations[0].client_order_id,
        "unit-spot-hop9-rec-spot-0000"
    );
    assert_eq!(
        report.reconciliations[7].client_order_id,
        "unit-spot-hop9-rec-spot-0007"
    );
    assert_eq!(
        report.reconciliations[8].client_order_id,
        "unit-spot-hop9-rec-spot-0008"
    );
    assert_eq!(
        report.reconciliations[9].client_order_id,
        "unit-spot-hop9-rec-spot-0009"
    );
    assert_eq!(report.reconciliations[9].status, "FILLED");
    assert!(report.reconciliations[9].terminal);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: -1,
        }]
    );
}

#[tokio::test]
async fn policy_demo_dry_run_eighth_continued_user_data_matches_assigned_ids() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = false;
    config.client_order_id_prefix = "unit-spot-hop9-dry".into();
    config.captured_user_data_json = Some(format!(
        "{}\n{}",
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop9-dry-spot-0000",
            411,
            "BUY",
            "FILLED"
        ),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop9-dry-spot-0001",
            412,
            "SELL",
            "FILLED"
        ),
    ));
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop9-dry-spot-0002",
        413,
        "BUY",
        "FILLED",
    ));
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.second_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop9-dry-spot-0003",
        414,
        "SELL",
        "FILLED",
    ));
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.third_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop9-dry-spot-0004",
        415,
        "BUY",
        "FILLED",
    ));
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fourth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop9-dry-spot-0005",
        416,
        "SELL",
        "FILLED",
    ));
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.fifth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop9-dry-spot-0006",
        417,
        "BUY",
        "FILLED",
    ));
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    config.sixth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop9-dry-spot-0007",
        418,
        "SELL",
        "FILLED",
    ));
    config.seventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"216.00","qty":"1.0"}],"asks":[{"price":"218.00","qty":"1.0"}],"update_id":15}"#
            .into(),
    );
    config.seventh_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop9-dry-spot-0008",
        419,
        "BUY",
        "FILLED",
    ));
    config.eighth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"217.00","qty":"1.0"}],"asks":[{"price":"219.00","qty":"1.0"}],"update_id":16}"#
            .into(),
    );
    config.eighth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop9-dry-spot-0009",
        420,
        "SELL",
        "FILLED",
    ));
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
    ]);

    let report = run_spot_policy_demo_with_placer(
        config,
        &policy,
        "eighth-continue-dry-reconcile",
        |_| async { panic!("dry-run must not place") },
    )
    .await
    .unwrap();

    assert!(!report.execute_demo_orders);
    assert_eq!(report.placed_orders, 0);
    assert!(report.receipts.is_empty());
    assert_eq!(report.order_count(), 10);
    assert_eq!(report.seventh_continued_steps, 1);
    assert_eq!(report.eighth_continued_steps, 1);
    assert_eq!(report.ninth_continued_steps, 0);
    assert_eq!(report.tenth_continued_steps, 0);
    assert_eq!(report.eleventh_continued_steps, 0);
    assert_eq!(report.twelfth_continued_steps, 0);
    assert_eq!(report.thirteenth_continued_steps, 0);
    assert_eq!(report.fourteenth_continued_steps, 0);
    assert_eq!(report.reconciliations.len(), 10);
    assert_eq!(
        report.reconciliations[2].client_order_id,
        "unit-spot-hop9-dry-spot-0002"
    );
    assert_eq!(
        report.reconciliations[8].client_order_id,
        "unit-spot-hop9-dry-spot-0008"
    );
    assert_eq!(
        report.reconciliations[9].client_order_id,
        "unit-spot-hop9-dry-spot-0009"
    );
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: -1,
        }]
    );
}

#[tokio::test]
async fn policy_demo_ninth_continued_tape_reconciles_placed_receipts() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.client_order_id_prefix = "unit-spot-hop10-rec".into();
    config.captured_user_data_json = Some(format!(
        "{}\n{}",
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop10-rec-spot-0000",
            431,
            "BUY",
            "FILLED"
        ),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop10-rec-spot-0001",
            432,
            "SELL",
            "FILLED"
        ),
    ));
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop10-rec-spot-0002",
        433,
        "BUY",
        "FILLED",
    ));
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.second_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop10-rec-spot-0003",
        434,
        "SELL",
        "FILLED",
    ));
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.third_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop10-rec-spot-0004",
        435,
        "BUY",
        "FILLED",
    ));
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fourth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop10-rec-spot-0005",
        436,
        "SELL",
        "FILLED",
    ));
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.fifth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop10-rec-spot-0006",
        437,
        "BUY",
        "FILLED",
    ));
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    config.sixth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop10-rec-spot-0007",
        438,
        "SELL",
        "FILLED",
    ));
    config.seventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"216.00","qty":"1.0"}],"asks":[{"price":"218.00","qty":"1.0"}],"update_id":15}"#
            .into(),
    );
    config.seventh_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop10-rec-spot-0008",
        439,
        "BUY",
        "FILLED",
    ));
    config.eighth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"217.00","qty":"1.0"}],"asks":[{"price":"219.00","qty":"1.0"}],"update_id":16}"#
            .into(),
    );
    config.eighth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop10-rec-spot-0009",
        440,
        "SELL",
        "FILLED",
    ));
    config.ninth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"218.00","qty":"1.0"}],"asks":[{"price":"220.00","qty":"1.0"}],"update_id":17}"#
            .into(),
    );
    config.ninth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop10-rec-spot-0010",
        441,
        "BUY",
        "FILLED",
    ));
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
    ]);

    let report = run_spot_policy_demo_with_placer(
        config,
        &policy,
        "ninth-continue-reconcile",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-spot-hop10-rec-spot-0000" => 431,
                    "unit-spot-hop10-rec-spot-0001" => 432,
                    "unit-spot-hop10-rec-spot-0002" => 433,
                    "unit-spot-hop10-rec-spot-0003" => 434,
                    "unit-spot-hop10-rec-spot-0004" => 435,
                    "unit-spot-hop10-rec-spot-0005" => 436,
                    "unit-spot-hop10-rec-spot-0006" => 437,
                    "unit-spot-hop10-rec-spot-0007" => 438,
                    "unit-spot-hop10-rec-spot-0008" => 439,
                    "unit-spot-hop10-rec-spot-0009" => 440,
                    "unit-spot-hop10-rec-spot-0010" => 441,
                    other => panic!("unexpected client order id {other}"),
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
    )
    .await
    .unwrap();

    assert_eq!(report.eighth_continued_steps, 1);
    assert_eq!(report.ninth_continued_steps, 1);
    assert_eq!(report.placed_orders, 11);
    assert_eq!(report.receipts.len(), 11);
    assert_eq!(
        report.receipts[0].client_order_id,
        "unit-spot-hop10-rec-spot-0000"
    );
    assert_eq!(
        report.receipts[9].client_order_id,
        "unit-spot-hop10-rec-spot-0009"
    );
    assert_eq!(
        report.receipts[10].client_order_id,
        "unit-spot-hop10-rec-spot-0010"
    );
    assert_eq!(report.receipts[10].side, "BUY");
    assert_eq!(report.reconciliations.len(), 11);
    assert_eq!(
        report.reconciliations[0].client_order_id,
        "unit-spot-hop10-rec-spot-0000"
    );
    assert_eq!(
        report.reconciliations[8].client_order_id,
        "unit-spot-hop10-rec-spot-0008"
    );
    assert_eq!(
        report.reconciliations[9].client_order_id,
        "unit-spot-hop10-rec-spot-0009"
    );
    assert_eq!(
        report.reconciliations[10].client_order_id,
        "unit-spot-hop10-rec-spot-0010"
    );
    assert_eq!(report.reconciliations[10].status, "FILLED");
    assert!(report.reconciliations[10].terminal);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: 1,
        }]
    );
}

#[tokio::test]
async fn policy_demo_dry_run_ninth_continued_user_data_matches_assigned_ids() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = false;
    config.client_order_id_prefix = "unit-spot-hop10-dry".into();
    config.captured_user_data_json = Some(format!(
        "{}\n{}",
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop10-dry-spot-0000",
            441,
            "BUY",
            "FILLED"
        ),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop10-dry-spot-0001",
            442,
            "SELL",
            "FILLED"
        ),
    ));
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop10-dry-spot-0002",
        443,
        "BUY",
        "FILLED",
    ));
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.second_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop10-dry-spot-0003",
        444,
        "SELL",
        "FILLED",
    ));
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.third_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop10-dry-spot-0004",
        445,
        "BUY",
        "FILLED",
    ));
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fourth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop10-dry-spot-0005",
        446,
        "SELL",
        "FILLED",
    ));
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.fifth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop10-dry-spot-0006",
        447,
        "BUY",
        "FILLED",
    ));
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    config.sixth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop10-dry-spot-0007",
        448,
        "SELL",
        "FILLED",
    ));
    config.seventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"216.00","qty":"1.0"}],"asks":[{"price":"218.00","qty":"1.0"}],"update_id":15}"#
            .into(),
    );
    config.seventh_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop10-dry-spot-0008",
        449,
        "BUY",
        "FILLED",
    ));
    config.eighth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"217.00","qty":"1.0"}],"asks":[{"price":"219.00","qty":"1.0"}],"update_id":16}"#
            .into(),
    );
    config.eighth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop10-dry-spot-0009",
        450,
        "SELL",
        "FILLED",
    ));
    config.ninth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"218.00","qty":"1.0"}],"asks":[{"price":"220.00","qty":"1.0"}],"update_id":17}"#
            .into(),
    );
    config.ninth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop10-dry-spot-0010",
        451,
        "BUY",
        "FILLED",
    ));
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
    ]);

    let report = run_spot_policy_demo_with_placer(
        config,
        &policy,
        "ninth-continue-dry-reconcile",
        |_| async { panic!("dry-run must not place") },
    )
    .await
    .unwrap();

    assert!(!report.execute_demo_orders);
    assert_eq!(report.placed_orders, 0);
    assert!(report.receipts.is_empty());
    assert_eq!(report.order_count(), 11);
    assert_eq!(report.eighth_continued_steps, 1);
    assert_eq!(report.ninth_continued_steps, 1);
    assert_eq!(report.reconciliations.len(), 11);
    assert_eq!(
        report.reconciliations[2].client_order_id,
        "unit-spot-hop10-dry-spot-0002"
    );
    assert_eq!(
        report.reconciliations[9].client_order_id,
        "unit-spot-hop10-dry-spot-0009"
    );
    assert_eq!(
        report.reconciliations[10].client_order_id,
        "unit-spot-hop10-dry-spot-0010"
    );
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: 1,
        }]
    );
}

#[tokio::test]
async fn policy_demo_usdm_continued_tape_places_orders_when_guarded() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Usdm, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.client_order_id_prefix = "unit-usdm-place".into();
    config.captured_user_data_json = Some(
        serde_json::json!([
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-place-usdm-0000",
                91,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-place-usdm-0001",
                92,
                "SELL",
                "FILLED"
            )
        ])
        .to_string(),
    );
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    let policy =
        SequencePolicy::with_actions(vec![Action::Hold, Action::Buy, Action::Sell, Action::Buy]);

    let report = run_usdm_policy_demo_with_placer(
        config,
        &policy,
        "continue-place-usdm",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(UsdmPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-usdm-place-usdm-0000" => 201,
                    "unit-usdm-place-usdm-0001" => 202,
                    "unit-usdm-place-usdm-0002" => 203,
                    other => panic!("unexpected client order id {other}"),
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
    )
    .await
    .unwrap();

    assert_eq!(report.steps, 3);
    assert_eq!(report.continued_steps, 1);
    assert_eq!(report.order_count(), 3);
    assert_eq!(report.placed_orders, 3);
    assert_eq!(report.receipts.len(), 3);
    assert_eq!(report.receipts[0].order_id, 201);
    assert_eq!(
        report.receipts[0].client_order_id,
        "unit-usdm-place-usdm-0000"
    );
    assert_eq!(report.receipts[1].order_id, 202);
    assert_eq!(
        report.receipts[1].client_order_id,
        "unit-usdm-place-usdm-0001"
    );
    assert_eq!(report.receipts[2].order_id, 203);
    assert_eq!(
        report.receipts[2].client_order_id,
        "unit-usdm-place-usdm-0002"
    );
    assert_eq!(report.receipts[2].side, "BUY");
    assert_eq!(report.receipts[2].symbol, "ETHUSDT");
}

#[tokio::test]
async fn policy_demo_primary_only_continue_keeps_empty_extra_inventory() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT");
    config.max_steps = 1;
    config.window_frames = 1;
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"BTCUSDT","bids":[{"price":"110.00","qty":"1.0"}],"asks":[{"price":"111.00","qty":"1.0"}],"update_id":3}"#
            .into(),
    );
    let policy = CaptureObsPolicy::default();

    let report = run_policy_demo_with_policy(config, &policy, "primary-continue")
        .await
        .unwrap();

    assert_eq!(report.symbol, "BTCUSDT");
    assert_eq!(report.continued_steps, 1);
    assert!(report.extra_symbol_inventory.is_empty());
    assert!(!report.continued_observation.is_empty());
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
    assert!(
        report.extra_symbol_inventory.is_empty(),
        "primary-only fills stay on the policy-step inventory path"
    );
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
    assert!(
        report.extra_symbol_inventory.is_empty(),
        "primary-only fills stay on the policy-step inventory path"
    );
}

#[tokio::test]
async fn policy_demo_usdm_reconciles_dispatched_extra_symbol() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Usdm, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.client_order_id_prefix = "unit-usdm-eth".into();
    let policy = SequencePolicy::hold_buy_sell();

    let report = run_usdm_policy_demo_with_placer_and_user_data(
        config,
        &policy,
        "pin-eth-usdm",
        |order| async move {
            assert_eq!(order.symbol, "ETHUSDT");
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(UsdmPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: if order.side == UsdmOrderSide::Buy {
                    61
                } else {
                    62
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

    assert_eq!(report.symbol, "BTCUSDT");
    assert_eq!(report.dispatch_symbol, "ETHUSDT");
    assert_eq!(
        report.placed_orders,
        2,
        "steps={} order_requests={} depth={}",
        report.steps,
        report.order_count(),
        report.depth_source
    );
    assert_eq!(report.receipts[0].symbol, "ETHUSDT");
    assert_eq!(report.reconciliations.len(), 2);
    assert_eq!(report.reconciliations[0].symbol, "ETHUSDT");
    assert!(report.reconciliations[0].terminal);
    assert_eq!(report.reconciliations[1].side, "SELL");
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: -1,
        }]
    );
}

fn policy_demo_depth_event(symbol: &str, bid: &str, ask: &str) -> StreamEvent {
    StreamEvent::Depth(DepthUpdate {
        symbol: symbol.into(),
        bids: vec![PriceLevel {
            price: bid.into(),
            qty: "1".into(),
        }],
        asks: vec![PriceLevel {
            price: ask.into(),
            qty: "1".into(),
        }],
        update_id: Some(1),
    })
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


#[tokio::test]
async fn policy_demo_wait_user_data_reconciles_tenth_continued_tape() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.client_order_id_prefix = "unit-spot-wait13".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    config.seventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"216.00","qty":"1.0"}],"asks":[{"price":"218.00","qty":"1.0"}],"update_id":15}"#
            .into(),
    );
    config.eighth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"217.00","qty":"1.0"}],"asks":[{"price":"219.00","qty":"1.0"}],"update_id":16}"#
            .into(),
    );
    config.ninth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"218.00","qty":"1.0"}],"asks":[{"price":"220.00","qty":"1.0"}],"update_id":17}"#
            .into(),
    );
    config.tenth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"219.00","qty":"1.0"}],"asks":[{"price":"221.00","qty":"1.0"}],"update_id":18}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
    ]);
    let waits = Cell::new(0usize);

    let report = run_spot_policy_demo_with_placer_and_user_data(
        config,
        &policy,
        "wait-tenth-continue-place",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-spot-wait13-spot-0000" => 581,
                    "unit-spot-wait13-spot-0001" => 582,
                    "unit-spot-wait13-spot-0002" => 583,
                    "unit-spot-wait13-spot-0003" => 584,
                    "unit-spot-wait13-spot-0004" => 585,
                    "unit-spot-wait13-spot-0005" => 586,
                    "unit-spot-wait13-spot-0006" => 587,
                    "unit-spot-wait13-spot-0007" => 588,
                    "unit-spot-wait13-spot-0008" => 589,
                    "unit-spot-wait13-spot-0009" => 590,
                    "unit-spot-wait13-spot-0010" => 591,
                    "unit-spot-wait13-spot-0011" => 592,
                    other => panic!("unexpected placed client order id {other}"),
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
            waits.set(waits.get() + 1);
            let payload = report
                .receipts
                .iter()
                .map(|receipt| {
                    spot_execution_report_json(
                        &receipt.symbol,
                        &receipt.client_order_id,
                        receipt.order_id,
                        &receipt.side,
                        "FILLED",
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            let messages = policy_demo_user_data_messages_from_json(&payload);
            async move { messages }
        },
    )
    .await
    .unwrap();

    assert_eq!(waits.get(), 11);
    assert_eq!(report.ninth_continued_steps, 1);
    assert_eq!(report.tenth_continued_steps, 1);
    assert_eq!(report.eleventh_continued_steps, 0);
    assert_eq!(report.twelfth_continued_steps, 0);
    assert_eq!(report.thirteenth_continued_steps, 0);
    assert_eq!(report.fourteenth_continued_steps, 0);
    assert_eq!(report.placed_orders, 12);
    assert_eq!(report.receipts.len(), 12);
    assert_eq!(report.reconciliations.len(), 12);
    assert_eq!(
        report.reconciliations[0].client_order_id,
        "unit-spot-wait13-spot-0000"
    );
    assert_eq!(
        report.reconciliations[10].client_order_id,
        "unit-spot-wait13-spot-0010"
    );
    assert_eq!(
        report.reconciliations[11].client_order_id,
        "unit-spot-wait13-spot-0011"
    );
    assert_eq!(report.reconciliations[11].side, "SELL");
    assert!(report.reconciliations[11].terminal);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: -1,
        }]
    );
}

#[tokio::test]
async fn policy_demo_live_source_waits_after_tenth_continued_tape_place() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.client_order_id_prefix = "unit-spot-live13".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    config.seventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"216.00","qty":"1.0"}],"asks":[{"price":"218.00","qty":"1.0"}],"update_id":15}"#
            .into(),
    );
    config.eighth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"217.00","qty":"1.0"}],"asks":[{"price":"219.00","qty":"1.0"}],"update_id":16}"#
            .into(),
    );
    config.ninth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"218.00","qty":"1.0"}],"asks":[{"price":"220.00","qty":"1.0"}],"update_id":17}"#
            .into(),
    );
    config.tenth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"219.00","qty":"1.0"}],"asks":[{"price":"221.00","qty":"1.0"}],"update_id":18}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
    ]);
    let frames = policy_demo_user_data_messages_from_json(&format!(
        "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
        spot_execution_report_json("ETHUSDT", "unit-spot-live13-spot-0000", 601, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live13-spot-0001",
            602,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live13-spot-0002", 603, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live13-spot-0003",
            604,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live13-spot-0004", 605, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live13-spot-0005",
            606,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live13-spot-0006", 607, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live13-spot-0007",
            608,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live13-spot-0008", 609, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live13-spot-0009",
            610,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live13-spot-0010", 611, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live13-spot-0011",
            612,
            "SELL",
            "FILLED"
        ),
    ))
    .unwrap();

    let report = run_spot_policy_demo_with_live_user_data_source(
        config,
        &policy,
        "live-source-tenth-continue-place",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-spot-live13-spot-0000" => 601,
                    "unit-spot-live13-spot-0001" => 602,
                    "unit-spot-live13-spot-0002" => 603,
                    "unit-spot-live13-spot-0003" => 604,
                    "unit-spot-live13-spot-0004" => 605,
                    "unit-spot-live13-spot-0005" => 606,
                    "unit-spot-live13-spot-0006" => 607,
                    "unit-spot-live13-spot-0007" => 608,
                    "unit-spot-live13-spot-0008" => 609,
                    "unit-spot-live13-spot-0009" => 610,
                    "unit-spot-live13-spot-0010" => 611,
                    "unit-spot-live13-spot-0011" => 612,
                    other => panic!("unexpected placed client order id {other}"),
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
        frames,
    )
    .await
    .unwrap();

    assert_eq!(report.ninth_continued_steps, 1);
    assert_eq!(report.tenth_continued_steps, 1);
    assert_eq!(report.eleventh_continued_steps, 0);
    assert_eq!(report.twelfth_continued_steps, 0);
    assert_eq!(report.thirteenth_continued_steps, 0);
    assert_eq!(report.fourteenth_continued_steps, 0);
    assert_eq!(report.placed_orders, 12);
    assert_eq!(report.receipts.len(), 12);
    assert_eq!(report.reconciliations.len(), 12);
    assert_eq!(
        report.reconciliations[0].client_order_id,
        "unit-spot-live13-spot-0000"
    );
    assert_eq!(
        report.reconciliations[10].client_order_id,
        "unit-spot-live13-spot-0010"
    );
    assert_eq!(
        report.reconciliations[11].client_order_id,
        "unit-spot-live13-spot-0011"
    );
    assert_eq!(report.reconciliations[11].side, "SELL");
    assert!(report.reconciliations[11].terminal);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: -1,
        }]
    );
}

#[tokio::test]
async fn policy_demo_usdm_live_source_waits_after_tenth_continued_tape_place() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Usdm, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.client_order_id_prefix = "unit-usdm-live13".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    config.seventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"216.00","qty":"1.0"}],"asks":[{"price":"218.00","qty":"1.0"}],"update_id":15}"#
            .into(),
    );
    config.eighth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"217.00","qty":"1.0"}],"asks":[{"price":"219.00","qty":"1.0"}],"update_id":16}"#
            .into(),
    );
    config.ninth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"218.00","qty":"1.0"}],"asks":[{"price":"220.00","qty":"1.0"}],"update_id":17}"#
            .into(),
    );
    config.tenth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"219.00","qty":"1.0"}],"asks":[{"price":"221.00","qty":"1.0"}],"update_id":18}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
    ]);
    let frames = policy_demo_user_data_messages_from_json(
        &serde_json::json!([
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live13-usdm-0000",
                611,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live13-usdm-0001",
                612,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live13-usdm-0002",
                613,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live13-usdm-0003",
                614,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live13-usdm-0004",
                615,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live13-usdm-0005",
                616,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live13-usdm-0006",
                617,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live13-usdm-0007",
                618,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live13-usdm-0008",
                619,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live13-usdm-0009",
                620,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live13-usdm-0010",
                621,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live13-usdm-0011",
                622,
                "SELL",
                "FILLED"
            )
        ])
        .to_string(),
    )
    .unwrap();

    let report = run_usdm_policy_demo_with_live_user_data_source(
        config,
        &policy,
        "live-source-tenth-continue-place-usdm",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(UsdmPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-usdm-live13-usdm-0000" => 611,
                    "unit-usdm-live13-usdm-0001" => 612,
                    "unit-usdm-live13-usdm-0002" => 613,
                    "unit-usdm-live13-usdm-0003" => 614,
                    "unit-usdm-live13-usdm-0004" => 615,
                    "unit-usdm-live13-usdm-0005" => 616,
                    "unit-usdm-live13-usdm-0006" => 617,
                    "unit-usdm-live13-usdm-0007" => 618,
                    "unit-usdm-live13-usdm-0008" => 619,
                    "unit-usdm-live13-usdm-0009" => 620,
                    "unit-usdm-live13-usdm-0010" => 621,
                    "unit-usdm-live13-usdm-0011" => 622,
                    other => panic!("unexpected placed client order id {other}"),
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
        frames,
    )
    .await
    .unwrap();

    assert_eq!(report.ninth_continued_steps, 1);
    assert_eq!(report.tenth_continued_steps, 1);
    assert_eq!(report.eleventh_continued_steps, 0);
    assert_eq!(report.twelfth_continued_steps, 0);
    assert_eq!(report.thirteenth_continued_steps, 0);
    assert_eq!(report.fourteenth_continued_steps, 0);
    assert_eq!(report.placed_orders, 12);
    assert_eq!(report.reconciliations.len(), 12);
    assert_eq!(
        report.reconciliations[11].client_order_id,
        "unit-usdm-live13-usdm-0011"
    );
    assert_eq!(report.reconciliations[11].side, "SELL");
    assert!(report.reconciliations[11].terminal);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: -1,
        }]
    );
}

#[tokio::test]
async fn policy_demo_tenth_continued_tape_reconciles_placed_receipts() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.client_order_id_prefix = "unit-spot-hop11-rec".into();
    config.captured_user_data_json = Some(format!(
        "{}\n{}",
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop11-rec-spot-0000",
            561,
            "BUY",
            "FILLED"
        ),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop11-rec-spot-0001",
            562,
            "SELL",
            "FILLED"
        ),
    ));
    config.continued_user_data_json = Some(spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop11-rec-spot-0002",
            563,
            "BUY",
            "FILLED"
        ));
    config.second_continued_user_data_json = Some(spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop11-rec-spot-0003",
            564,
            "SELL",
            "FILLED"
        ));
    config.third_continued_user_data_json = Some(spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop11-rec-spot-0004",
            565,
            "BUY",
            "FILLED"
        ));
    config.fourth_continued_user_data_json = Some(spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop11-rec-spot-0005",
            566,
            "SELL",
            "FILLED"
        ));
    config.fifth_continued_user_data_json = Some(spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop11-rec-spot-0006",
            567,
            "BUY",
            "FILLED"
        ));
    config.sixth_continued_user_data_json = Some(spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop11-rec-spot-0007",
            568,
            "SELL",
            "FILLED"
        ));
    config.seventh_continued_user_data_json = Some(spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop11-rec-spot-0008",
            569,
            "BUY",
            "FILLED"
        ));
    config.eighth_continued_user_data_json = Some(spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop11-rec-spot-0009",
            570,
            "SELL",
            "FILLED"
        ));
    config.ninth_continued_user_data_json = Some(spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop11-rec-spot-0010",
            571,
            "BUY",
            "FILLED"
        ));
    config.tenth_continued_user_data_json = Some(spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop11-rec-spot-0011",
            572,
            "SELL",
            "FILLED"
        ));
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    config.seventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"216.00","qty":"1.0"}],"asks":[{"price":"218.00","qty":"1.0"}],"update_id":15}"#
            .into(),
    );
    config.eighth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"217.00","qty":"1.0"}],"asks":[{"price":"219.00","qty":"1.0"}],"update_id":16}"#
            .into(),
    );
    config.ninth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"218.00","qty":"1.0"}],"asks":[{"price":"220.00","qty":"1.0"}],"update_id":17}"#
            .into(),
    );
    config.tenth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"219.00","qty":"1.0"}],"asks":[{"price":"221.00","qty":"1.0"}],"update_id":18}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
    ]);

    let report = run_spot_policy_demo_with_placer(
        config,
        &policy,
        "tenth-continue-reconcile",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-spot-hop11-rec-spot-0000" => 561,
                    "unit-spot-hop11-rec-spot-0001" => 562,
                    "unit-spot-hop11-rec-spot-0002" => 563,
                    "unit-spot-hop11-rec-spot-0003" => 564,
                    "unit-spot-hop11-rec-spot-0004" => 565,
                    "unit-spot-hop11-rec-spot-0005" => 566,
                    "unit-spot-hop11-rec-spot-0006" => 567,
                    "unit-spot-hop11-rec-spot-0007" => 568,
                    "unit-spot-hop11-rec-spot-0008" => 569,
                    "unit-spot-hop11-rec-spot-0009" => 570,
                    "unit-spot-hop11-rec-spot-0010" => 571,
                    "unit-spot-hop11-rec-spot-0011" => 572,
                    other => panic!("unexpected client order id {other}"),
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
    )
    .await
    .unwrap();

    assert_eq!(report.ninth_continued_steps, 1);
    assert_eq!(report.tenth_continued_steps, 1);
    assert_eq!(report.eleventh_continued_steps, 0);
    assert_eq!(report.twelfth_continued_steps, 0);
    assert_eq!(report.thirteenth_continued_steps, 0);
    assert_eq!(report.fourteenth_continued_steps, 0);
    assert_eq!(report.placed_orders, 12);
    assert_eq!(report.receipts.len(), 12);
    assert_eq!(
        report.receipts[0].client_order_id,
        "unit-spot-hop11-rec-spot-0000"
    );
    assert_eq!(
        report.receipts[10].client_order_id,
        "unit-spot-hop11-rec-spot-0010"
    );
    assert_eq!(
        report.receipts[11].client_order_id,
        "unit-spot-hop11-rec-spot-0011"
    );
    assert_eq!(report.receipts[11].side, "SELL");
    assert_eq!(report.reconciliations.len(), 12);
    assert_eq!(
        report.reconciliations[0].client_order_id,
        "unit-spot-hop11-rec-spot-0000"
    );
    assert_eq!(
        report.reconciliations[9].client_order_id,
        "unit-spot-hop11-rec-spot-0009"
    );
    assert_eq!(
        report.reconciliations[10].client_order_id,
        "unit-spot-hop11-rec-spot-0010"
    );
    assert_eq!(
        report.reconciliations[11].client_order_id,
        "unit-spot-hop11-rec-spot-0011"
    );
    assert_eq!(report.reconciliations[11].status, "FILLED");
    assert!(report.reconciliations[11].terminal);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: -1,
        }]
    );
}

#[tokio::test]
async fn policy_demo_dry_run_tenth_continued_user_data_matches_assigned_ids() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = false;
    config.client_order_id_prefix = "unit-spot-hop11-dry".into();
    config.captured_user_data_json = Some(format!(
        "{}\n{}",
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop11-dry-spot-0000",
            571,
            "BUY",
            "FILLED"
        ),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop11-dry-spot-0001",
            572,
            "SELL",
            "FILLED"
        ),
    ));
    config.continued_user_data_json = Some(spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop11-dry-spot-0002",
            573,
            "BUY",
            "FILLED"
        ));
    config.second_continued_user_data_json = Some(spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop11-dry-spot-0003",
            574,
            "SELL",
            "FILLED"
        ));
    config.third_continued_user_data_json = Some(spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop11-dry-spot-0004",
            575,
            "BUY",
            "FILLED"
        ));
    config.fourth_continued_user_data_json = Some(spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop11-dry-spot-0005",
            576,
            "SELL",
            "FILLED"
        ));
    config.fifth_continued_user_data_json = Some(spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop11-dry-spot-0006",
            577,
            "BUY",
            "FILLED"
        ));
    config.sixth_continued_user_data_json = Some(spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop11-dry-spot-0007",
            578,
            "SELL",
            "FILLED"
        ));
    config.seventh_continued_user_data_json = Some(spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop11-dry-spot-0008",
            579,
            "BUY",
            "FILLED"
        ));
    config.eighth_continued_user_data_json = Some(spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop11-dry-spot-0009",
            580,
            "SELL",
            "FILLED"
        ));
    config.ninth_continued_user_data_json = Some(spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop11-dry-spot-0010",
            581,
            "BUY",
            "FILLED"
        ));
    config.tenth_continued_user_data_json = Some(spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop11-dry-spot-0011",
            582,
            "SELL",
            "FILLED"
        ));
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    config.seventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"216.00","qty":"1.0"}],"asks":[{"price":"218.00","qty":"1.0"}],"update_id":15}"#
            .into(),
    );
    config.eighth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"217.00","qty":"1.0"}],"asks":[{"price":"219.00","qty":"1.0"}],"update_id":16}"#
            .into(),
    );
    config.ninth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"218.00","qty":"1.0"}],"asks":[{"price":"220.00","qty":"1.0"}],"update_id":17}"#
            .into(),
    );
    config.tenth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"219.00","qty":"1.0"}],"asks":[{"price":"221.00","qty":"1.0"}],"update_id":18}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
    ]);

    let report = run_spot_policy_demo_with_placer(
        config,
        &policy,
        "tenth-continue-dry-reconcile",
        |_| async { panic!("dry-run must not place") },
    )
    .await
    .unwrap();

    assert!(!report.execute_demo_orders);
    assert_eq!(report.placed_orders, 0);
    assert!(report.receipts.is_empty());
    assert_eq!(report.order_count(), 12);
    assert_eq!(report.ninth_continued_steps, 1);
    assert_eq!(report.tenth_continued_steps, 1);
    assert_eq!(report.eleventh_continued_steps, 0);
    assert_eq!(report.twelfth_continued_steps, 0);
    assert_eq!(report.thirteenth_continued_steps, 0);
    assert_eq!(report.fourteenth_continued_steps, 0);
    assert_eq!(report.reconciliations.len(), 12);
    assert_eq!(
        report.reconciliations[2].client_order_id,
        "unit-spot-hop11-dry-spot-0002"
    );
    assert_eq!(
        report.reconciliations[10].client_order_id,
        "unit-spot-hop11-dry-spot-0010"
    );
    assert_eq!(
        report.reconciliations[11].client_order_id,
        "unit-spot-hop11-dry-spot-0011"
    );
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: -1,
        }]
    );
}

#[tokio::test]
async fn policy_demo_eleventh_continued_tape_reconciles_placed_receipts() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.client_order_id_prefix = "unit-spot-hop12-rec".into();
    config.captured_user_data_json = Some(format!(
        "{}\n{}",
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop12-rec-spot-0000",
            681,
            "BUY",
            "FILLED"
        ),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop12-rec-spot-0001",
            682,
            "SELL",
            "FILLED"
        ),
    ));
    config.continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop12-rec-spot-0002",
        683,
        "BUY",
        "FILLED",
    ));
    config.second_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop12-rec-spot-0003",
        684,
        "SELL",
        "FILLED",
    ));
    config.third_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop12-rec-spot-0004",
        685,
        "BUY",
        "FILLED",
    ));
    config.fourth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop12-rec-spot-0005",
        686,
        "SELL",
        "FILLED",
    ));
    config.fifth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop12-rec-spot-0006",
        687,
        "BUY",
        "FILLED",
    ));
    config.sixth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop12-rec-spot-0007",
        688,
        "SELL",
        "FILLED",
    ));
    config.seventh_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop12-rec-spot-0008",
        689,
        "BUY",
        "FILLED",
    ));
    config.eighth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop12-rec-spot-0009",
        690,
        "SELL",
        "FILLED",
    ));
    config.ninth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop12-rec-spot-0010",
        691,
        "BUY",
        "FILLED",
    ));
    config.tenth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop12-rec-spot-0011",
        692,
        "SELL",
        "FILLED",
    ));
    config.eleventh_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop12-rec-spot-0012",
        693,
        "BUY",
        "FILLED",
    ));
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    config.seventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"216.00","qty":"1.0"}],"asks":[{"price":"218.00","qty":"1.0"}],"update_id":15}"#
            .into(),
    );
    config.eighth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"217.00","qty":"1.0"}],"asks":[{"price":"219.00","qty":"1.0"}],"update_id":16}"#
            .into(),
    );
    config.ninth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"218.00","qty":"1.0"}],"asks":[{"price":"220.00","qty":"1.0"}],"update_id":17}"#
            .into(),
    );
    config.tenth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"219.00","qty":"1.0"}],"asks":[{"price":"221.00","qty":"1.0"}],"update_id":18}"#
            .into(),
    );
    config.eleventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"220.00","qty":"1.0"}],"asks":[{"price":"222.00","qty":"1.0"}],"update_id":19}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
    ]);

    let report = run_spot_policy_demo_with_placer(
        config,
        &policy,
        "eleventh-continue-reconcile",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-spot-hop12-rec-spot-0000" => 681,
                    "unit-spot-hop12-rec-spot-0001" => 682,
                    "unit-spot-hop12-rec-spot-0002" => 683,
                    "unit-spot-hop12-rec-spot-0003" => 684,
                    "unit-spot-hop12-rec-spot-0004" => 685,
                    "unit-spot-hop12-rec-spot-0005" => 686,
                    "unit-spot-hop12-rec-spot-0006" => 687,
                    "unit-spot-hop12-rec-spot-0007" => 688,
                    "unit-spot-hop12-rec-spot-0008" => 689,
                    "unit-spot-hop12-rec-spot-0009" => 690,
                    "unit-spot-hop12-rec-spot-0010" => 691,
                    "unit-spot-hop12-rec-spot-0011" => 692,
                    "unit-spot-hop12-rec-spot-0012" => 693,
                    other => panic!("unexpected client order id {other}"),
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
    )
    .await
    .unwrap();

    assert_eq!(report.tenth_continued_steps, 1);
    assert_eq!(report.eleventh_continued_steps, 1);
    assert_eq!(report.twelfth_continued_steps, 0);
    assert_eq!(report.thirteenth_continued_steps, 0);
    assert_eq!(report.fourteenth_continued_steps, 0);
    assert_eq!(report.placed_orders, 13);
    assert_eq!(report.receipts.len(), 13);
    assert_eq!(
        report.receipts[0].client_order_id,
        "unit-spot-hop12-rec-spot-0000"
    );
    assert_eq!(
        report.receipts[12].client_order_id,
        "unit-spot-hop12-rec-spot-0012"
    );
    assert_eq!(report.receipts[12].side, "BUY");
    assert_eq!(report.reconciliations.len(), 13);
    assert_eq!(
        report.reconciliations[0].client_order_id,
        "unit-spot-hop12-rec-spot-0000"
    );
    assert_eq!(
        report.reconciliations[11].client_order_id,
        "unit-spot-hop12-rec-spot-0011"
    );
    assert_eq!(
        report.reconciliations[12].client_order_id,
        "unit-spot-hop12-rec-spot-0012"
    );
    assert_eq!(report.reconciliations[12].status, "FILLED");
    assert!(report.reconciliations[12].terminal);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: 1,
        }]
    );
}

#[tokio::test]
async fn policy_demo_dry_run_eleventh_continued_user_data_matches_assigned_ids() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = false;
    config.client_order_id_prefix = "unit-spot-hop12-dry".into();
    config.captured_user_data_json = Some(format!(
        "{}\n{}",
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop12-dry-spot-0000",
            701,
            "BUY",
            "FILLED"
        ),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop12-dry-spot-0001",
            702,
            "SELL",
            "FILLED"
        ),
    ));
    config.continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop12-dry-spot-0002",
        703,
        "BUY",
        "FILLED",
    ));
    config.second_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop12-dry-spot-0003",
        704,
        "SELL",
        "FILLED",
    ));
    config.third_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop12-dry-spot-0004",
        705,
        "BUY",
        "FILLED",
    ));
    config.fourth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop12-dry-spot-0005",
        706,
        "SELL",
        "FILLED",
    ));
    config.fifth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop12-dry-spot-0006",
        707,
        "BUY",
        "FILLED",
    ));
    config.sixth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop12-dry-spot-0007",
        708,
        "SELL",
        "FILLED",
    ));
    config.seventh_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop12-dry-spot-0008",
        709,
        "BUY",
        "FILLED",
    ));
    config.eighth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop12-dry-spot-0009",
        710,
        "SELL",
        "FILLED",
    ));
    config.ninth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop12-dry-spot-0010",
        711,
        "BUY",
        "FILLED",
    ));
    config.tenth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop12-dry-spot-0011",
        712,
        "SELL",
        "FILLED",
    ));
    config.eleventh_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop12-dry-spot-0012",
        713,
        "BUY",
        "FILLED",
    ));
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    config.seventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"216.00","qty":"1.0"}],"asks":[{"price":"218.00","qty":"1.0"}],"update_id":15}"#
            .into(),
    );
    config.eighth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"217.00","qty":"1.0"}],"asks":[{"price":"219.00","qty":"1.0"}],"update_id":16}"#
            .into(),
    );
    config.ninth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"218.00","qty":"1.0"}],"asks":[{"price":"220.00","qty":"1.0"}],"update_id":17}"#
            .into(),
    );
    config.tenth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"219.00","qty":"1.0"}],"asks":[{"price":"221.00","qty":"1.0"}],"update_id":18}"#
            .into(),
    );
    config.eleventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"220.00","qty":"1.0"}],"asks":[{"price":"222.00","qty":"1.0"}],"update_id":19}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
    ]);

    let report = run_spot_policy_demo_with_placer(
        config,
        &policy,
        "eleventh-continue-dry-reconcile",
        |_| async { panic!("dry-run must not place") },
    )
    .await
    .unwrap();

    assert!(!report.execute_demo_orders);
    assert_eq!(report.placed_orders, 0);
    assert!(report.receipts.is_empty());
    assert_eq!(report.order_count(), 13);
    assert_eq!(report.tenth_continued_steps, 1);
    assert_eq!(report.eleventh_continued_steps, 1);
    assert_eq!(report.twelfth_continued_steps, 0);
    assert_eq!(report.thirteenth_continued_steps, 0);
    assert_eq!(report.fourteenth_continued_steps, 0);
    assert_eq!(report.reconciliations.len(), 13);
    assert_eq!(
        report.reconciliations[2].client_order_id,
        "unit-spot-hop12-dry-spot-0002"
    );
    assert_eq!(
        report.reconciliations[11].client_order_id,
        "unit-spot-hop12-dry-spot-0011"
    );
    assert_eq!(
        report.reconciliations[12].client_order_id,
        "unit-spot-hop12-dry-spot-0012"
    );
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: 1,
        }]
    );
}

#[tokio::test]
async fn policy_demo_wait_user_data_reconciles_eleventh_continued_tape() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.client_order_id_prefix = "unit-spot-wait14".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    config.seventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"216.00","qty":"1.0"}],"asks":[{"price":"218.00","qty":"1.0"}],"update_id":15}"#
            .into(),
    );
    config.eighth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"217.00","qty":"1.0"}],"asks":[{"price":"219.00","qty":"1.0"}],"update_id":16}"#
            .into(),
    );
    config.ninth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"218.00","qty":"1.0"}],"asks":[{"price":"220.00","qty":"1.0"}],"update_id":17}"#
            .into(),
    );
    config.tenth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"219.00","qty":"1.0"}],"asks":[{"price":"221.00","qty":"1.0"}],"update_id":18}"#
            .into(),
    );
    config.eleventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"220.00","qty":"1.0"}],"asks":[{"price":"222.00","qty":"1.0"}],"update_id":19}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
    ]);
    let waits = Cell::new(0usize);

    let report = run_spot_policy_demo_with_placer_and_user_data(
        config,
        &policy,
        "wait-eleventh-continue-place",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-spot-wait14-spot-0000" => 721,
                    "unit-spot-wait14-spot-0001" => 722,
                    "unit-spot-wait14-spot-0002" => 723,
                    "unit-spot-wait14-spot-0003" => 724,
                    "unit-spot-wait14-spot-0004" => 725,
                    "unit-spot-wait14-spot-0005" => 726,
                    "unit-spot-wait14-spot-0006" => 727,
                    "unit-spot-wait14-spot-0007" => 728,
                    "unit-spot-wait14-spot-0008" => 729,
                    "unit-spot-wait14-spot-0009" => 730,
                    "unit-spot-wait14-spot-0010" => 731,
                    "unit-spot-wait14-spot-0011" => 732,
                    "unit-spot-wait14-spot-0012" => 733,
                    other => panic!("unexpected placed client order id {other}"),
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
            waits.set(waits.get() + 1);
            let payload = report
                .receipts
                .iter()
                .map(|receipt| {
                    spot_execution_report_json(
                        &receipt.symbol,
                        &receipt.client_order_id,
                        receipt.order_id,
                        &receipt.side,
                        "FILLED",
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            let messages = policy_demo_user_data_messages_from_json(&payload);
            async move { messages }
        },
    )
    .await
    .unwrap();

    assert_eq!(waits.get(), 12);
    assert_eq!(report.tenth_continued_steps, 1);
    assert_eq!(report.eleventh_continued_steps, 1);
    assert_eq!(report.twelfth_continued_steps, 0);
    assert_eq!(report.thirteenth_continued_steps, 0);
    assert_eq!(report.fourteenth_continued_steps, 0);
    assert_eq!(report.placed_orders, 13);
    assert_eq!(report.receipts.len(), 13);
    assert_eq!(report.reconciliations.len(), 13);
    assert_eq!(
        report.reconciliations[0].client_order_id,
        "unit-spot-wait14-spot-0000"
    );
    assert_eq!(
        report.reconciliations[11].client_order_id,
        "unit-spot-wait14-spot-0011"
    );
    assert_eq!(
        report.reconciliations[12].client_order_id,
        "unit-spot-wait14-spot-0012"
    );
    assert_eq!(report.reconciliations[12].side, "BUY");
    assert!(report.reconciliations[12].terminal);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: 1,
        }]
    );
}

#[tokio::test]
async fn policy_demo_live_source_waits_after_eleventh_continued_tape_place() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.client_order_id_prefix = "unit-spot-live14".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    config.seventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"216.00","qty":"1.0"}],"asks":[{"price":"218.00","qty":"1.0"}],"update_id":15}"#
            .into(),
    );
    config.eighth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"217.00","qty":"1.0"}],"asks":[{"price":"219.00","qty":"1.0"}],"update_id":16}"#
            .into(),
    );
    config.ninth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"218.00","qty":"1.0"}],"asks":[{"price":"220.00","qty":"1.0"}],"update_id":17}"#
            .into(),
    );
    config.tenth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"219.00","qty":"1.0"}],"asks":[{"price":"221.00","qty":"1.0"}],"update_id":18}"#
            .into(),
    );
    config.eleventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"220.00","qty":"1.0"}],"asks":[{"price":"222.00","qty":"1.0"}],"update_id":19}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
    ]);
    let frames = policy_demo_user_data_messages_from_json(&format!(
        "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
        spot_execution_report_json("ETHUSDT", "unit-spot-live14-spot-0000", 741, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live14-spot-0001",
            742,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live14-spot-0002", 743, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live14-spot-0003",
            744,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live14-spot-0004", 745, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live14-spot-0005",
            746,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live14-spot-0006", 747, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live14-spot-0007",
            748,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live14-spot-0008", 749, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live14-spot-0009",
            750,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live14-spot-0010", 751, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live14-spot-0011",
            752,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live14-spot-0012", 753, "BUY", "FILLED"),
    ))
    .unwrap();

    let report = run_spot_policy_demo_with_live_user_data_source(
        config,
        &policy,
        "live-source-eleventh-continue-place",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-spot-live14-spot-0000" => 741,
                    "unit-spot-live14-spot-0001" => 742,
                    "unit-spot-live14-spot-0002" => 743,
                    "unit-spot-live14-spot-0003" => 744,
                    "unit-spot-live14-spot-0004" => 745,
                    "unit-spot-live14-spot-0005" => 746,
                    "unit-spot-live14-spot-0006" => 747,
                    "unit-spot-live14-spot-0007" => 748,
                    "unit-spot-live14-spot-0008" => 749,
                    "unit-spot-live14-spot-0009" => 750,
                    "unit-spot-live14-spot-0010" => 751,
                    "unit-spot-live14-spot-0011" => 752,
                    "unit-spot-live14-spot-0012" => 753,
                    other => panic!("unexpected placed client order id {other}"),
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
        frames,
    )
    .await
    .unwrap();

    assert_eq!(report.tenth_continued_steps, 1);
    assert_eq!(report.eleventh_continued_steps, 1);
    assert_eq!(report.twelfth_continued_steps, 0);
    assert_eq!(report.thirteenth_continued_steps, 0);
    assert_eq!(report.fourteenth_continued_steps, 0);
    assert_eq!(report.placed_orders, 13);
    assert_eq!(report.receipts.len(), 13);
    assert_eq!(report.reconciliations.len(), 13);
    assert_eq!(
        report.reconciliations[0].client_order_id,
        "unit-spot-live14-spot-0000"
    );
    assert_eq!(
        report.reconciliations[11].client_order_id,
        "unit-spot-live14-spot-0011"
    );
    assert_eq!(
        report.reconciliations[12].client_order_id,
        "unit-spot-live14-spot-0012"
    );
    assert_eq!(report.reconciliations[12].side, "BUY");
    assert!(report.reconciliations[12].terminal);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: 1,
        }]
    );
}

#[tokio::test]
async fn policy_demo_usdm_live_source_waits_after_eleventh_continued_tape_place() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Usdm, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.client_order_id_prefix = "unit-usdm-live14".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    config.seventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"216.00","qty":"1.0"}],"asks":[{"price":"218.00","qty":"1.0"}],"update_id":15}"#
            .into(),
    );
    config.eighth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"217.00","qty":"1.0"}],"asks":[{"price":"219.00","qty":"1.0"}],"update_id":16}"#
            .into(),
    );
    config.ninth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"218.00","qty":"1.0"}],"asks":[{"price":"220.00","qty":"1.0"}],"update_id":17}"#
            .into(),
    );
    config.tenth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"219.00","qty":"1.0"}],"asks":[{"price":"221.00","qty":"1.0"}],"update_id":18}"#
            .into(),
    );
    config.eleventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"220.00","qty":"1.0"}],"asks":[{"price":"222.00","qty":"1.0"}],"update_id":19}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
    ]);
    let frames = policy_demo_user_data_messages_from_json(
        &serde_json::json!([
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live14-usdm-0000",
                761,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live14-usdm-0001",
                762,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live14-usdm-0002",
                763,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live14-usdm-0003",
                764,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live14-usdm-0004",
                765,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live14-usdm-0005",
                766,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live14-usdm-0006",
                767,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live14-usdm-0007",
                768,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live14-usdm-0008",
                769,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live14-usdm-0009",
                770,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live14-usdm-0010",
                771,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live14-usdm-0011",
                772,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live14-usdm-0012",
                773,
                "BUY",
                "FILLED"
            )
        ])
        .to_string(),
    )
    .unwrap();

    let report = run_usdm_policy_demo_with_live_user_data_source(
        config,
        &policy,
        "live-source-eleventh-continue-place-usdm",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(UsdmPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-usdm-live14-usdm-0000" => 761,
                    "unit-usdm-live14-usdm-0001" => 762,
                    "unit-usdm-live14-usdm-0002" => 763,
                    "unit-usdm-live14-usdm-0003" => 764,
                    "unit-usdm-live14-usdm-0004" => 765,
                    "unit-usdm-live14-usdm-0005" => 766,
                    "unit-usdm-live14-usdm-0006" => 767,
                    "unit-usdm-live14-usdm-0007" => 768,
                    "unit-usdm-live14-usdm-0008" => 769,
                    "unit-usdm-live14-usdm-0009" => 770,
                    "unit-usdm-live14-usdm-0010" => 771,
                    "unit-usdm-live14-usdm-0011" => 772,
                    "unit-usdm-live14-usdm-0012" => 773,
                    other => panic!("unexpected placed client order id {other}"),
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
        frames,
    )
    .await
    .unwrap();

    assert_eq!(report.tenth_continued_steps, 1);
    assert_eq!(report.eleventh_continued_steps, 1);
    assert_eq!(report.twelfth_continued_steps, 0);
    assert_eq!(report.thirteenth_continued_steps, 0);
    assert_eq!(report.fourteenth_continued_steps, 0);
    assert_eq!(report.placed_orders, 13);
    assert_eq!(report.reconciliations.len(), 13);
    assert_eq!(
        report.reconciliations[12].client_order_id,
        "unit-usdm-live14-usdm-0012"
    );
    assert_eq!(report.reconciliations[12].side, "BUY");
    assert!(report.reconciliations[12].terminal);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: 1,
        }]
    );
}

#[tokio::test]
async fn policy_demo_twelfth_continued_tape_reconciles_placed_receipts() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.client_order_id_prefix = "unit-spot-hop13-rec".into();
    config.captured_user_data_json = Some(format!(
        "{}\n{}",
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop13-rec-spot-0000",
            801,
            "BUY",
            "FILLED"
        ),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop13-rec-spot-0001",
            802,
            "SELL",
            "FILLED"
        ),
    ));
    config.continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop13-rec-spot-0002",
        803,
        "BUY",
        "FILLED",
    ));
    config.second_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop13-rec-spot-0003",
        804,
        "SELL",
        "FILLED",
    ));
    config.third_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop13-rec-spot-0004",
        805,
        "BUY",
        "FILLED",
    ));
    config.fourth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop13-rec-spot-0005",
        806,
        "SELL",
        "FILLED",
    ));
    config.fifth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop13-rec-spot-0006",
        807,
        "BUY",
        "FILLED",
    ));
    config.sixth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop13-rec-spot-0007",
        808,
        "SELL",
        "FILLED",
    ));
    config.seventh_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop13-rec-spot-0008",
        809,
        "BUY",
        "FILLED",
    ));
    config.eighth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop13-rec-spot-0009",
        810,
        "SELL",
        "FILLED",
    ));
    config.ninth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop13-rec-spot-0010",
        811,
        "BUY",
        "FILLED",
    ));
    config.tenth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop13-rec-spot-0011",
        812,
        "SELL",
        "FILLED",
    ));
    config.eleventh_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop13-rec-spot-0012",
        813,
        "BUY",
        "FILLED",
    ));
    config.twelfth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop13-rec-spot-0013",
        814,
        "SELL",
        "FILLED",
    ));
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    config.seventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"216.00","qty":"1.0"}],"asks":[{"price":"218.00","qty":"1.0"}],"update_id":15}"#
            .into(),
    );
    config.eighth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"217.00","qty":"1.0"}],"asks":[{"price":"219.00","qty":"1.0"}],"update_id":16}"#
            .into(),
    );
    config.ninth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"218.00","qty":"1.0"}],"asks":[{"price":"220.00","qty":"1.0"}],"update_id":17}"#
            .into(),
    );
    config.tenth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"219.00","qty":"1.0"}],"asks":[{"price":"221.00","qty":"1.0"}],"update_id":18}"#
            .into(),
    );
    config.eleventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"220.00","qty":"1.0"}],"asks":[{"price":"222.00","qty":"1.0"}],"update_id":19}"#
            .into(),
    );
    config.twelfth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"221.00","qty":"1.0"}],"asks":[{"price":"223.00","qty":"1.0"}],"update_id":20}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
    ]);

    let report = run_spot_policy_demo_with_placer(
        config,
        &policy,
        "twelfth-continue-reconcile",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-spot-hop13-rec-spot-0000" => 801,
                    "unit-spot-hop13-rec-spot-0001" => 802,
                    "unit-spot-hop13-rec-spot-0002" => 803,
                    "unit-spot-hop13-rec-spot-0003" => 804,
                    "unit-spot-hop13-rec-spot-0004" => 805,
                    "unit-spot-hop13-rec-spot-0005" => 806,
                    "unit-spot-hop13-rec-spot-0006" => 807,
                    "unit-spot-hop13-rec-spot-0007" => 808,
                    "unit-spot-hop13-rec-spot-0008" => 809,
                    "unit-spot-hop13-rec-spot-0009" => 810,
                    "unit-spot-hop13-rec-spot-0010" => 811,
                    "unit-spot-hop13-rec-spot-0011" => 812,
                    "unit-spot-hop13-rec-spot-0012" => 813,
                    "unit-spot-hop13-rec-spot-0013" => 814,
                    other => panic!("unexpected client order id {other}"),
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
    )
    .await
    .unwrap();

    assert_eq!(report.eleventh_continued_steps, 1);
    assert_eq!(report.twelfth_continued_steps, 1);
    assert_eq!(report.thirteenth_continued_steps, 0);
    assert_eq!(report.fourteenth_continued_steps, 0);
    assert_eq!(report.placed_orders, 14);
    assert_eq!(report.receipts.len(), 14);
    assert_eq!(
        report.receipts[0].client_order_id,
        "unit-spot-hop13-rec-spot-0000"
    );
    assert_eq!(
        report.receipts[13].client_order_id,
        "unit-spot-hop13-rec-spot-0013"
    );
    assert_eq!(report.receipts[13].side, "SELL");
    assert_eq!(report.reconciliations.len(), 14);
    assert_eq!(
        report.reconciliations[0].client_order_id,
        "unit-spot-hop13-rec-spot-0000"
    );
    assert_eq!(
        report.reconciliations[12].client_order_id,
        "unit-spot-hop13-rec-spot-0012"
    );
    assert_eq!(
        report.reconciliations[13].client_order_id,
        "unit-spot-hop13-rec-spot-0013"
    );
    assert_eq!(report.reconciliations[13].status, "FILLED");
    assert!(report.reconciliations[13].terminal);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: -1,
        }]
    );
}

#[tokio::test]
async fn policy_demo_dry_run_twelfth_continued_user_data_matches_assigned_ids() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = false;
    config.client_order_id_prefix = "unit-spot-hop13-dry".into();
    config.captured_user_data_json = Some(format!(
        "{}\n{}",
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop13-dry-spot-0000",
            821,
            "BUY",
            "FILLED"
        ),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop13-dry-spot-0001",
            822,
            "SELL",
            "FILLED"
        ),
    ));
    config.continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop13-dry-spot-0002",
        823,
        "BUY",
        "FILLED",
    ));
    config.second_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop13-dry-spot-0003",
        824,
        "SELL",
        "FILLED",
    ));
    config.third_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop13-dry-spot-0004",
        825,
        "BUY",
        "FILLED",
    ));
    config.fourth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop13-dry-spot-0005",
        826,
        "SELL",
        "FILLED",
    ));
    config.fifth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop13-dry-spot-0006",
        827,
        "BUY",
        "FILLED",
    ));
    config.sixth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop13-dry-spot-0007",
        828,
        "SELL",
        "FILLED",
    ));
    config.seventh_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop13-dry-spot-0008",
        829,
        "BUY",
        "FILLED",
    ));
    config.eighth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop13-dry-spot-0009",
        830,
        "SELL",
        "FILLED",
    ));
    config.ninth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop13-dry-spot-0010",
        831,
        "BUY",
        "FILLED",
    ));
    config.tenth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop13-dry-spot-0011",
        832,
        "SELL",
        "FILLED",
    ));
    config.eleventh_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop13-dry-spot-0012",
        833,
        "BUY",
        "FILLED",
    ));
    config.twelfth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop13-dry-spot-0013",
        834,
        "SELL",
        "FILLED",
    ));
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    config.seventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"216.00","qty":"1.0"}],"asks":[{"price":"218.00","qty":"1.0"}],"update_id":15}"#
            .into(),
    );
    config.eighth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"217.00","qty":"1.0"}],"asks":[{"price":"219.00","qty":"1.0"}],"update_id":16}"#
            .into(),
    );
    config.ninth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"218.00","qty":"1.0"}],"asks":[{"price":"220.00","qty":"1.0"}],"update_id":17}"#
            .into(),
    );
    config.tenth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"219.00","qty":"1.0"}],"asks":[{"price":"221.00","qty":"1.0"}],"update_id":18}"#
            .into(),
    );
    config.eleventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"220.00","qty":"1.0"}],"asks":[{"price":"222.00","qty":"1.0"}],"update_id":19}"#
            .into(),
    );
    config.twelfth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"221.00","qty":"1.0"}],"asks":[{"price":"223.00","qty":"1.0"}],"update_id":20}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
    ]);

    let report = run_spot_policy_demo_with_placer(
        config,
        &policy,
        "twelfth-continue-dry-reconcile",
        |_| async { panic!("dry-run must not place") },
    )
    .await
    .unwrap();

    assert!(!report.execute_demo_orders);
    assert_eq!(report.placed_orders, 0);
    assert!(report.receipts.is_empty());
    assert_eq!(report.order_count(), 14);
    assert_eq!(report.eleventh_continued_steps, 1);
    assert_eq!(report.twelfth_continued_steps, 1);
    assert_eq!(report.thirteenth_continued_steps, 0);
    assert_eq!(report.fourteenth_continued_steps, 0);
    assert_eq!(report.reconciliations.len(), 14);
    assert_eq!(
        report.reconciliations[2].client_order_id,
        "unit-spot-hop13-dry-spot-0002"
    );
    assert_eq!(
        report.reconciliations[12].client_order_id,
        "unit-spot-hop13-dry-spot-0012"
    );
    assert_eq!(
        report.reconciliations[13].client_order_id,
        "unit-spot-hop13-dry-spot-0013"
    );
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: -1,
        }]
    );
}

#[tokio::test]
async fn policy_demo_wait_user_data_reconciles_twelfth_continued_tape() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.client_order_id_prefix = "unit-spot-wait15".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    config.seventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"216.00","qty":"1.0"}],"asks":[{"price":"218.00","qty":"1.0"}],"update_id":15}"#
            .into(),
    );
    config.eighth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"217.00","qty":"1.0"}],"asks":[{"price":"219.00","qty":"1.0"}],"update_id":16}"#
            .into(),
    );
    config.ninth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"218.00","qty":"1.0"}],"asks":[{"price":"220.00","qty":"1.0"}],"update_id":17}"#
            .into(),
    );
    config.tenth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"219.00","qty":"1.0"}],"asks":[{"price":"221.00","qty":"1.0"}],"update_id":18}"#
            .into(),
    );
    config.eleventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"220.00","qty":"1.0"}],"asks":[{"price":"222.00","qty":"1.0"}],"update_id":19}"#
            .into(),
    );
    config.twelfth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"221.00","qty":"1.0"}],"asks":[{"price":"223.00","qty":"1.0"}],"update_id":20}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
    ]);
    let waits = Cell::new(0usize);

    let report = run_spot_policy_demo_with_placer_and_user_data(
        config,
        &policy,
        "wait-twelfth-continue-place",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-spot-wait15-spot-0000" => 841,
                    "unit-spot-wait15-spot-0001" => 842,
                    "unit-spot-wait15-spot-0002" => 843,
                    "unit-spot-wait15-spot-0003" => 844,
                    "unit-spot-wait15-spot-0004" => 845,
                    "unit-spot-wait15-spot-0005" => 846,
                    "unit-spot-wait15-spot-0006" => 847,
                    "unit-spot-wait15-spot-0007" => 848,
                    "unit-spot-wait15-spot-0008" => 849,
                    "unit-spot-wait15-spot-0009" => 850,
                    "unit-spot-wait15-spot-0010" => 851,
                    "unit-spot-wait15-spot-0011" => 852,
                    "unit-spot-wait15-spot-0012" => 853,
                    "unit-spot-wait15-spot-0013" => 854,
                    other => panic!("unexpected placed client order id {other}"),
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
            waits.set(waits.get() + 1);
            let payload = report
                .receipts
                .iter()
                .map(|receipt| {
                    spot_execution_report_json(
                        &receipt.symbol,
                        &receipt.client_order_id,
                        receipt.order_id,
                        &receipt.side,
                        "FILLED",
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            let messages = policy_demo_user_data_messages_from_json(&payload);
            async move { messages }
        },
    )
    .await
    .unwrap();

    assert_eq!(waits.get(), 13);
    assert_eq!(report.eleventh_continued_steps, 1);
    assert_eq!(report.twelfth_continued_steps, 1);
    assert_eq!(report.thirteenth_continued_steps, 0);
    assert_eq!(report.fourteenth_continued_steps, 0);
    assert_eq!(report.placed_orders, 14);
    assert_eq!(report.receipts.len(), 14);
    assert_eq!(report.reconciliations.len(), 14);
    assert_eq!(
        report.reconciliations[0].client_order_id,
        "unit-spot-wait15-spot-0000"
    );
    assert_eq!(
        report.reconciliations[12].client_order_id,
        "unit-spot-wait15-spot-0012"
    );
    assert_eq!(
        report.reconciliations[13].client_order_id,
        "unit-spot-wait15-spot-0013"
    );
    assert_eq!(report.reconciliations[13].side, "SELL");
    assert!(report.reconciliations[13].terminal);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: -1,
        }]
    );
}

#[tokio::test]
async fn policy_demo_live_source_waits_after_twelfth_continued_tape_place() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.client_order_id_prefix = "unit-spot-live15".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    config.seventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"216.00","qty":"1.0"}],"asks":[{"price":"218.00","qty":"1.0"}],"update_id":15}"#
            .into(),
    );
    config.eighth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"217.00","qty":"1.0"}],"asks":[{"price":"219.00","qty":"1.0"}],"update_id":16}"#
            .into(),
    );
    config.ninth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"218.00","qty":"1.0"}],"asks":[{"price":"220.00","qty":"1.0"}],"update_id":17}"#
            .into(),
    );
    config.tenth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"219.00","qty":"1.0"}],"asks":[{"price":"221.00","qty":"1.0"}],"update_id":18}"#
            .into(),
    );
    config.eleventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"220.00","qty":"1.0"}],"asks":[{"price":"222.00","qty":"1.0"}],"update_id":19}"#
            .into(),
    );
    config.twelfth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"221.00","qty":"1.0"}],"asks":[{"price":"223.00","qty":"1.0"}],"update_id":20}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
    ]);
    let frames = policy_demo_user_data_messages_from_json(&format!(
        "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
        spot_execution_report_json("ETHUSDT", "unit-spot-live15-spot-0000", 861, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live15-spot-0001",
            862,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live15-spot-0002", 863, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live15-spot-0003",
            864,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live15-spot-0004", 865, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live15-spot-0005",
            866,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live15-spot-0006", 867, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live15-spot-0007",
            868,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live15-spot-0008", 869, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live15-spot-0009",
            870,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live15-spot-0010", 871, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live15-spot-0011",
            872,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live15-spot-0012", 873, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live15-spot-0013",
            874,
            "SELL",
            "FILLED"
        ),
    ))
    .unwrap();

    let report = run_spot_policy_demo_with_live_user_data_source(
        config,
        &policy,
        "live-source-twelfth-continue-place",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-spot-live15-spot-0000" => 861,
                    "unit-spot-live15-spot-0001" => 862,
                    "unit-spot-live15-spot-0002" => 863,
                    "unit-spot-live15-spot-0003" => 864,
                    "unit-spot-live15-spot-0004" => 865,
                    "unit-spot-live15-spot-0005" => 866,
                    "unit-spot-live15-spot-0006" => 867,
                    "unit-spot-live15-spot-0007" => 868,
                    "unit-spot-live15-spot-0008" => 869,
                    "unit-spot-live15-spot-0009" => 870,
                    "unit-spot-live15-spot-0010" => 871,
                    "unit-spot-live15-spot-0011" => 872,
                    "unit-spot-live15-spot-0012" => 873,
                    "unit-spot-live15-spot-0013" => 874,
                    other => panic!("unexpected placed client order id {other}"),
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
        frames,
    )
    .await
    .unwrap();

    assert_eq!(report.eleventh_continued_steps, 1);
    assert_eq!(report.twelfth_continued_steps, 1);
    assert_eq!(report.thirteenth_continued_steps, 0);
    assert_eq!(report.fourteenth_continued_steps, 0);
    assert_eq!(report.placed_orders, 14);
    assert_eq!(report.receipts.len(), 14);
    assert_eq!(report.reconciliations.len(), 14);
    assert_eq!(
        report.reconciliations[0].client_order_id,
        "unit-spot-live15-spot-0000"
    );
    assert_eq!(
        report.reconciliations[12].client_order_id,
        "unit-spot-live15-spot-0012"
    );
    assert_eq!(
        report.reconciliations[13].client_order_id,
        "unit-spot-live15-spot-0013"
    );
    assert_eq!(report.reconciliations[13].side, "SELL");
    assert!(report.reconciliations[13].terminal);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: -1,
        }]
    );
}

#[tokio::test]
async fn policy_demo_usdm_live_source_waits_after_twelfth_continued_tape_place() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Usdm, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.client_order_id_prefix = "unit-usdm-live15".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    config.seventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"216.00","qty":"1.0"}],"asks":[{"price":"218.00","qty":"1.0"}],"update_id":15}"#
            .into(),
    );
    config.eighth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"217.00","qty":"1.0"}],"asks":[{"price":"219.00","qty":"1.0"}],"update_id":16}"#
            .into(),
    );
    config.ninth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"218.00","qty":"1.0"}],"asks":[{"price":"220.00","qty":"1.0"}],"update_id":17}"#
            .into(),
    );
    config.tenth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"219.00","qty":"1.0"}],"asks":[{"price":"221.00","qty":"1.0"}],"update_id":18}"#
            .into(),
    );
    config.eleventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"220.00","qty":"1.0"}],"asks":[{"price":"222.00","qty":"1.0"}],"update_id":19}"#
            .into(),
    );
    config.twelfth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"221.00","qty":"1.0"}],"asks":[{"price":"223.00","qty":"1.0"}],"update_id":20}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
    ]);
    let frames = policy_demo_user_data_messages_from_json(
        &serde_json::json!([
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live15-usdm-0000",
                881,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live15-usdm-0001",
                882,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live15-usdm-0002",
                883,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live15-usdm-0003",
                884,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live15-usdm-0004",
                885,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live15-usdm-0005",
                886,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live15-usdm-0006",
                887,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live15-usdm-0007",
                888,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live15-usdm-0008",
                889,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live15-usdm-0009",
                890,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live15-usdm-0010",
                891,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live15-usdm-0011",
                892,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live15-usdm-0012",
                893,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live15-usdm-0013",
                894,
                "SELL",
                "FILLED"
            )
        ])
        .to_string(),
    )
    .unwrap();

    let report = run_usdm_policy_demo_with_live_user_data_source(
        config,
        &policy,
        "live-source-twelfth-continue-place-usdm",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(UsdmPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-usdm-live15-usdm-0000" => 881,
                    "unit-usdm-live15-usdm-0001" => 882,
                    "unit-usdm-live15-usdm-0002" => 883,
                    "unit-usdm-live15-usdm-0003" => 884,
                    "unit-usdm-live15-usdm-0004" => 885,
                    "unit-usdm-live15-usdm-0005" => 886,
                    "unit-usdm-live15-usdm-0006" => 887,
                    "unit-usdm-live15-usdm-0007" => 888,
                    "unit-usdm-live15-usdm-0008" => 889,
                    "unit-usdm-live15-usdm-0009" => 890,
                    "unit-usdm-live15-usdm-0010" => 891,
                    "unit-usdm-live15-usdm-0011" => 892,
                    "unit-usdm-live15-usdm-0012" => 893,
                    "unit-usdm-live15-usdm-0013" => 894,
                    other => panic!("unexpected placed client order id {other}"),
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
        frames,
    )
    .await
    .unwrap();

    assert_eq!(report.eleventh_continued_steps, 1);
    assert_eq!(report.twelfth_continued_steps, 1);
    assert_eq!(report.thirteenth_continued_steps, 0);
    assert_eq!(report.fourteenth_continued_steps, 0);
    assert_eq!(report.placed_orders, 14);
    assert_eq!(report.reconciliations.len(), 14);
    assert_eq!(
        report.reconciliations[13].client_order_id,
        "unit-usdm-live15-usdm-0013"
    );
    assert_eq!(report.reconciliations[13].side, "SELL");
    assert!(report.reconciliations[13].terminal);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: -1,
        }]
    );
}

#[tokio::test]
async fn policy_demo_thirteenth_continued_tape_reconciles_placed_receipts() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.client_order_id_prefix = "unit-spot-hop14-rec".into();
    config.captured_user_data_json = Some(format!(
        "{}\n{}",
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop14-rec-spot-0000",
            901,
            "BUY",
            "FILLED"
        ),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop14-rec-spot-0001",
            902,
            "SELL",
            "FILLED"
        ),
    ));
    config.continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop14-rec-spot-0002",
        903,
        "BUY",
        "FILLED",
    ));
    config.second_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop14-rec-spot-0003",
        904,
        "SELL",
        "FILLED",
    ));
    config.third_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop14-rec-spot-0004",
        905,
        "BUY",
        "FILLED",
    ));
    config.fourth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop14-rec-spot-0005",
        906,
        "SELL",
        "FILLED",
    ));
    config.fifth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop14-rec-spot-0006",
        907,
        "BUY",
        "FILLED",
    ));
    config.sixth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop14-rec-spot-0007",
        908,
        "SELL",
        "FILLED",
    ));
    config.seventh_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop14-rec-spot-0008",
        909,
        "BUY",
        "FILLED",
    ));
    config.eighth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop14-rec-spot-0009",
        910,
        "SELL",
        "FILLED",
    ));
    config.ninth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop14-rec-spot-0010",
        911,
        "BUY",
        "FILLED",
    ));
    config.tenth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop14-rec-spot-0011",
        912,
        "SELL",
        "FILLED",
    ));
    config.eleventh_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop14-rec-spot-0012",
        913,
        "BUY",
        "FILLED",
    ));
    config.twelfth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop14-rec-spot-0013",
        914,
        "SELL",
        "FILLED",
    ));
    config.thirteenth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop14-rec-spot-0014",
        915,
        "BUY",
        "FILLED",
    ));
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    config.seventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"216.00","qty":"1.0"}],"asks":[{"price":"218.00","qty":"1.0"}],"update_id":15}"#
            .into(),
    );
    config.eighth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"217.00","qty":"1.0"}],"asks":[{"price":"219.00","qty":"1.0"}],"update_id":16}"#
            .into(),
    );
    config.ninth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"218.00","qty":"1.0"}],"asks":[{"price":"220.00","qty":"1.0"}],"update_id":17}"#
            .into(),
    );
    config.tenth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"219.00","qty":"1.0"}],"asks":[{"price":"221.00","qty":"1.0"}],"update_id":18}"#
            .into(),
    );
    config.eleventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"220.00","qty":"1.0"}],"asks":[{"price":"222.00","qty":"1.0"}],"update_id":19}"#
            .into(),
    );
    config.twelfth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"221.00","qty":"1.0"}],"asks":[{"price":"223.00","qty":"1.0"}],"update_id":20}"#
            .into(),
    );
    config.thirteenth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"222.00","qty":"1.0"}],"asks":[{"price":"224.00","qty":"1.0"}],"update_id":21}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
    ]);

    let report = run_spot_policy_demo_with_placer(
        config,
        &policy,
        "thirteenth-continue-reconcile",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-spot-hop14-rec-spot-0000" => 901,
                    "unit-spot-hop14-rec-spot-0001" => 902,
                    "unit-spot-hop14-rec-spot-0002" => 903,
                    "unit-spot-hop14-rec-spot-0003" => 904,
                    "unit-spot-hop14-rec-spot-0004" => 905,
                    "unit-spot-hop14-rec-spot-0005" => 906,
                    "unit-spot-hop14-rec-spot-0006" => 907,
                    "unit-spot-hop14-rec-spot-0007" => 908,
                    "unit-spot-hop14-rec-spot-0008" => 909,
                    "unit-spot-hop14-rec-spot-0009" => 910,
                    "unit-spot-hop14-rec-spot-0010" => 911,
                    "unit-spot-hop14-rec-spot-0011" => 912,
                    "unit-spot-hop14-rec-spot-0012" => 913,
                    "unit-spot-hop14-rec-spot-0013" => 914,
                    "unit-spot-hop14-rec-spot-0014" => 915,
                    other => panic!("unexpected client order id {other}"),
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
    )
    .await
    .unwrap();

    assert_eq!(report.twelfth_continued_steps, 1);
    assert_eq!(report.thirteenth_continued_steps, 1);
    assert_eq!(report.fourteenth_continued_steps, 0);
    assert_eq!(report.placed_orders, 15);
    assert_eq!(report.receipts.len(), 15);
    assert_eq!(
        report.receipts[0].client_order_id,
        "unit-spot-hop14-rec-spot-0000"
    );
    assert_eq!(
        report.receipts[14].client_order_id,
        "unit-spot-hop14-rec-spot-0014"
    );
    assert_eq!(report.receipts[14].side, "BUY");
    assert_eq!(report.reconciliations.len(), 15);
    assert_eq!(
        report.reconciliations[0].client_order_id,
        "unit-spot-hop14-rec-spot-0000"
    );
    assert_eq!(
        report.reconciliations[13].client_order_id,
        "unit-spot-hop14-rec-spot-0013"
    );
    assert_eq!(
        report.reconciliations[14].client_order_id,
        "unit-spot-hop14-rec-spot-0014"
    );
    assert_eq!(report.reconciliations[14].status, "FILLED");
    assert!(report.reconciliations[14].terminal);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: 1,
        }]
    );
}

#[tokio::test]
async fn policy_demo_dry_run_thirteenth_continued_user_data_matches_assigned_ids() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = false;
    config.client_order_id_prefix = "unit-spot-hop14-dry".into();
    config.captured_user_data_json = Some(format!(
        "{}\n{}",
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop14-dry-spot-0000",
            921,
            "BUY",
            "FILLED"
        ),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-hop14-dry-spot-0001",
            922,
            "SELL",
            "FILLED"
        ),
    ));
    config.continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop14-dry-spot-0002",
        923,
        "BUY",
        "FILLED",
    ));
    config.second_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop14-dry-spot-0003",
        924,
        "SELL",
        "FILLED",
    ));
    config.third_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop14-dry-spot-0004",
        925,
        "BUY",
        "FILLED",
    ));
    config.fourth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop14-dry-spot-0005",
        926,
        "SELL",
        "FILLED",
    ));
    config.fifth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop14-dry-spot-0006",
        927,
        "BUY",
        "FILLED",
    ));
    config.sixth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop14-dry-spot-0007",
        928,
        "SELL",
        "FILLED",
    ));
    config.seventh_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop14-dry-spot-0008",
        929,
        "BUY",
        "FILLED",
    ));
    config.eighth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop14-dry-spot-0009",
        930,
        "SELL",
        "FILLED",
    ));
    config.ninth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop14-dry-spot-0010",
        931,
        "BUY",
        "FILLED",
    ));
    config.tenth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop14-dry-spot-0011",
        932,
        "SELL",
        "FILLED",
    ));
    config.eleventh_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop14-dry-spot-0012",
        933,
        "BUY",
        "FILLED",
    ));
    config.twelfth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop14-dry-spot-0013",
        934,
        "SELL",
        "FILLED",
    ));
    config.thirteenth_continued_user_data_json = Some(spot_execution_report_json(
        "ETHUSDT",
        "unit-spot-hop14-dry-spot-0014",
        935,
        "BUY",
        "FILLED",
    ));
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    config.seventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"216.00","qty":"1.0"}],"asks":[{"price":"218.00","qty":"1.0"}],"update_id":15}"#
            .into(),
    );
    config.eighth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"217.00","qty":"1.0"}],"asks":[{"price":"219.00","qty":"1.0"}],"update_id":16}"#
            .into(),
    );
    config.ninth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"218.00","qty":"1.0"}],"asks":[{"price":"220.00","qty":"1.0"}],"update_id":17}"#
            .into(),
    );
    config.tenth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"219.00","qty":"1.0"}],"asks":[{"price":"221.00","qty":"1.0"}],"update_id":18}"#
            .into(),
    );
    config.eleventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"220.00","qty":"1.0"}],"asks":[{"price":"222.00","qty":"1.0"}],"update_id":19}"#
            .into(),
    );
    config.twelfth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"221.00","qty":"1.0"}],"asks":[{"price":"223.00","qty":"1.0"}],"update_id":20}"#
            .into(),
    );
    config.thirteenth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"222.00","qty":"1.0"}],"asks":[{"price":"224.00","qty":"1.0"}],"update_id":21}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
    ]);

    let report = run_spot_policy_demo_with_placer(
        config,
        &policy,
        "thirteenth-continue-dry-reconcile",
        |_| async { panic!("dry-run must not place") },
    )
    .await
    .unwrap();

    assert!(!report.execute_demo_orders);
    assert_eq!(report.placed_orders, 0);
    assert!(report.receipts.is_empty());
    assert_eq!(report.order_count(), 15);
    assert_eq!(report.twelfth_continued_steps, 1);
    assert_eq!(report.thirteenth_continued_steps, 1);
    assert_eq!(report.fourteenth_continued_steps, 0);
    assert_eq!(report.reconciliations.len(), 15);
    assert_eq!(
        report.reconciliations[2].client_order_id,
        "unit-spot-hop14-dry-spot-0002"
    );
    assert_eq!(
        report.reconciliations[13].client_order_id,
        "unit-spot-hop14-dry-spot-0013"
    );
    assert_eq!(
        report.reconciliations[14].client_order_id,
        "unit-spot-hop14-dry-spot-0014"
    );
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: 1,
        }]
    );
}

#[tokio::test]
async fn policy_demo_wait_user_data_reconciles_thirteenth_continued_tape() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.client_order_id_prefix = "unit-spot-wait16".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    config.seventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"216.00","qty":"1.0"}],"asks":[{"price":"218.00","qty":"1.0"}],"update_id":15}"#
            .into(),
    );
    config.eighth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"217.00","qty":"1.0"}],"asks":[{"price":"219.00","qty":"1.0"}],"update_id":16}"#
            .into(),
    );
    config.ninth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"218.00","qty":"1.0"}],"asks":[{"price":"220.00","qty":"1.0"}],"update_id":17}"#
            .into(),
    );
    config.tenth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"219.00","qty":"1.0"}],"asks":[{"price":"221.00","qty":"1.0"}],"update_id":18}"#
            .into(),
    );
    config.eleventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"220.00","qty":"1.0"}],"asks":[{"price":"222.00","qty":"1.0"}],"update_id":19}"#
            .into(),
    );
    config.twelfth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"221.00","qty":"1.0"}],"asks":[{"price":"223.00","qty":"1.0"}],"update_id":20}"#
            .into(),
    );
    config.thirteenth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"222.00","qty":"1.0"}],"asks":[{"price":"224.00","qty":"1.0"}],"update_id":21}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
    ]);
    let waits = Cell::new(0usize);

    let report = run_spot_policy_demo_with_placer_and_user_data(
        config,
        &policy,
        "wait-thirteenth-continue-place",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-spot-wait16-spot-0000" => 941,
                    "unit-spot-wait16-spot-0001" => 942,
                    "unit-spot-wait16-spot-0002" => 943,
                    "unit-spot-wait16-spot-0003" => 944,
                    "unit-spot-wait16-spot-0004" => 945,
                    "unit-spot-wait16-spot-0005" => 946,
                    "unit-spot-wait16-spot-0006" => 947,
                    "unit-spot-wait16-spot-0007" => 948,
                    "unit-spot-wait16-spot-0008" => 949,
                    "unit-spot-wait16-spot-0009" => 950,
                    "unit-spot-wait16-spot-0010" => 951,
                    "unit-spot-wait16-spot-0011" => 952,
                    "unit-spot-wait16-spot-0012" => 953,
                    "unit-spot-wait16-spot-0013" => 954,
                    "unit-spot-wait16-spot-0014" => 955,
                    other => panic!("unexpected placed client order id {other}"),
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
            waits.set(waits.get() + 1);
            let payload = report
                .receipts
                .iter()
                .map(|receipt| {
                    spot_execution_report_json(
                        &receipt.symbol,
                        &receipt.client_order_id,
                        receipt.order_id,
                        &receipt.side,
                        "FILLED",
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            let messages = policy_demo_user_data_messages_from_json(&payload);
            async move { messages }
        },
    )
    .await
    .unwrap();

    assert_eq!(waits.get(), 14);
    assert_eq!(report.twelfth_continued_steps, 1);
    assert_eq!(report.thirteenth_continued_steps, 1);
    assert_eq!(report.fourteenth_continued_steps, 0);
    assert_eq!(report.placed_orders, 15);
    assert_eq!(report.receipts.len(), 15);
    assert_eq!(report.reconciliations.len(), 15);
    assert_eq!(
        report.reconciliations[0].client_order_id,
        "unit-spot-wait16-spot-0000"
    );
    assert_eq!(
        report.reconciliations[13].client_order_id,
        "unit-spot-wait16-spot-0013"
    );
    assert_eq!(
        report.reconciliations[14].client_order_id,
        "unit-spot-wait16-spot-0014"
    );
    assert_eq!(report.reconciliations[14].side, "BUY");
    assert!(report.reconciliations[14].terminal);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: 1,
        }]
    );
}

#[tokio::test]
async fn policy_demo_live_source_waits_after_thirteenth_continued_tape_place() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Spot, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.client_order_id_prefix = "unit-spot-live16".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    config.seventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"216.00","qty":"1.0"}],"asks":[{"price":"218.00","qty":"1.0"}],"update_id":15}"#
            .into(),
    );
    config.eighth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"217.00","qty":"1.0"}],"asks":[{"price":"219.00","qty":"1.0"}],"update_id":16}"#
            .into(),
    );
    config.ninth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"218.00","qty":"1.0"}],"asks":[{"price":"220.00","qty":"1.0"}],"update_id":17}"#
            .into(),
    );
    config.tenth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"219.00","qty":"1.0"}],"asks":[{"price":"221.00","qty":"1.0"}],"update_id":18}"#
            .into(),
    );
    config.eleventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"220.00","qty":"1.0"}],"asks":[{"price":"222.00","qty":"1.0"}],"update_id":19}"#
            .into(),
    );
    config.twelfth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"221.00","qty":"1.0"}],"asks":[{"price":"223.00","qty":"1.0"}],"update_id":20}"#
            .into(),
    );
    config.thirteenth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"222.00","qty":"1.0"}],"asks":[{"price":"224.00","qty":"1.0"}],"update_id":21}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
    ]);
    let frames = policy_demo_user_data_messages_from_json(&format!(
        "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
        spot_execution_report_json("ETHUSDT", "unit-spot-live16-spot-0000", 961, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live16-spot-0001",
            962,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live16-spot-0002", 963, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live16-spot-0003",
            964,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live16-spot-0004", 965, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live16-spot-0005",
            966,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live16-spot-0006", 967, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live16-spot-0007",
            968,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live16-spot-0008", 969, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live16-spot-0009",
            970,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live16-spot-0010", 971, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live16-spot-0011",
            972,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live16-spot-0012", 973, "BUY", "FILLED"),
        spot_execution_report_json(
            "ETHUSDT",
            "unit-spot-live16-spot-0013",
            974,
            "SELL",
            "FILLED"
        ),
        spot_execution_report_json("ETHUSDT", "unit-spot-live16-spot-0014", 975, "BUY", "FILLED")
    ))
    .unwrap();

    let report = run_spot_policy_demo_with_live_user_data_source(
        config,
        &policy,
        "live-source-thirteenth-continue-place",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(SpotPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-spot-live16-spot-0000" => 961,
                    "unit-spot-live16-spot-0001" => 962,
                    "unit-spot-live16-spot-0002" => 963,
                    "unit-spot-live16-spot-0003" => 964,
                    "unit-spot-live16-spot-0004" => 965,
                    "unit-spot-live16-spot-0005" => 966,
                    "unit-spot-live16-spot-0006" => 967,
                    "unit-spot-live16-spot-0007" => 968,
                    "unit-spot-live16-spot-0008" => 969,
                    "unit-spot-live16-spot-0009" => 970,
                    "unit-spot-live16-spot-0010" => 971,
                    "unit-spot-live16-spot-0011" => 972,
                    "unit-spot-live16-spot-0012" => 973,
                    "unit-spot-live16-spot-0013" => 974,
                    "unit-spot-live16-spot-0014" => 975,
                    other => panic!("unexpected placed client order id {other}"),
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
        frames,
    )
    .await
    .unwrap();

    assert_eq!(report.twelfth_continued_steps, 1);
    assert_eq!(report.thirteenth_continued_steps, 1);
    assert_eq!(report.fourteenth_continued_steps, 0);
    assert_eq!(report.placed_orders, 15);
    assert_eq!(report.receipts.len(), 15);
    assert_eq!(report.reconciliations.len(), 15);
    assert_eq!(
        report.reconciliations[0].client_order_id,
        "unit-spot-live16-spot-0000"
    );
    assert_eq!(
        report.reconciliations[13].client_order_id,
        "unit-spot-live16-spot-0013"
    );
    assert_eq!(
        report.reconciliations[14].client_order_id,
        "unit-spot-live16-spot-0014"
    );
    assert_eq!(report.reconciliations[14].side, "BUY");
    assert!(report.reconciliations[14].terminal);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: 1,
        }]
    );
}

#[tokio::test]
async fn policy_demo_usdm_live_source_waits_after_thirteenth_continued_tape_place() {
    let mut config = PolicyDemoConfig::new(DemoVenue::Usdm, "BTCUSDT,ETHUSDT");
    config.set_dispatch_symbol("ETHUSDT");
    config.max_steps = 3;
    config.window_frames = 1;
    config.qty = "0.03".into();
    config.execute_demo_orders = true;
    config.wait_for_user_data = true;
    config.client_order_id_prefix = "unit-usdm-live16".into();
    config.continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"210.00","qty":"1.0"}],"asks":[{"price":"212.00","qty":"1.0"}],"update_id":9}"#
            .into(),
    );
    config.second_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"211.00","qty":"1.0"}],"asks":[{"price":"213.00","qty":"1.0"}],"update_id":10}"#
            .into(),
    );
    config.third_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"212.00","qty":"1.0"}],"asks":[{"price":"214.00","qty":"1.0"}],"update_id":11}"#
            .into(),
    );
    config.fourth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"213.00","qty":"1.0"}],"asks":[{"price":"215.00","qty":"1.0"}],"update_id":12}"#
            .into(),
    );
    config.fifth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"214.00","qty":"1.0"}],"asks":[{"price":"216.00","qty":"1.0"}],"update_id":13}"#
            .into(),
    );
    config.sixth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"215.00","qty":"1.0"}],"asks":[{"price":"217.00","qty":"1.0"}],"update_id":14}"#
            .into(),
    );
    config.seventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"216.00","qty":"1.0"}],"asks":[{"price":"218.00","qty":"1.0"}],"update_id":15}"#
            .into(),
    );
    config.eighth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"217.00","qty":"1.0"}],"asks":[{"price":"219.00","qty":"1.0"}],"update_id":16}"#
            .into(),
    );
    config.ninth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"218.00","qty":"1.0"}],"asks":[{"price":"220.00","qty":"1.0"}],"update_id":17}"#
            .into(),
    );
    config.tenth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"219.00","qty":"1.0"}],"asks":[{"price":"221.00","qty":"1.0"}],"update_id":18}"#
            .into(),
    );
    config.eleventh_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"220.00","qty":"1.0"}],"asks":[{"price":"222.00","qty":"1.0"}],"update_id":19}"#
            .into(),
    );
    config.twelfth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"221.00","qty":"1.0"}],"asks":[{"price":"223.00","qty":"1.0"}],"update_id":20}"#
            .into(),
    );
    config.thirteenth_continued_depth_json = Some(
        r#"{"kind":"depth","symbol":"ETHUSDT","bids":[{"price":"222.00","qty":"1.0"}],"asks":[{"price":"224.00","qty":"1.0"}],"update_id":21}"#
            .into(),
    );
    let policy = SequencePolicy::with_actions(vec![
        Action::Hold,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
        Action::Sell,
        Action::Buy,
    ]);
    let frames = policy_demo_user_data_messages_from_json(
        &serde_json::json!([
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live16-usdm-0000",
                981,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live16-usdm-0001",
                982,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live16-usdm-0002",
                983,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live16-usdm-0003",
                984,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live16-usdm-0004",
                985,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live16-usdm-0005",
                986,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live16-usdm-0006",
                987,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live16-usdm-0007",
                988,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live16-usdm-0008",
                989,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live16-usdm-0009",
                990,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live16-usdm-0010",
                991,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live16-usdm-0011",
                992,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live16-usdm-0012",
                993,
                "BUY",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live16-usdm-0013",
                994,
                "SELL",
                "FILLED"
            ),
            usdm_order_trade_update_json(
                "ETHUSDT",
                "unit-usdm-live16-usdm-0014",
                995,
                "BUY",
                "FILLED"
            )
        ])
        .to_string(),
    )
    .unwrap();

    let report = run_usdm_policy_demo_with_live_user_data_source(
        config,
        &policy,
        "live-source-thirteenth-continue-place-usdm",
        |order| async move {
            let client_order_id = order
                .new_client_order_id
                .clone()
                .expect("client order id assigned before placement");
            Ok(UsdmPlaceOrderResponse {
                symbol: order.symbol.clone(),
                order_id: match client_order_id.as_str() {
                    "unit-usdm-live16-usdm-0000" => 981,
                    "unit-usdm-live16-usdm-0001" => 982,
                    "unit-usdm-live16-usdm-0002" => 983,
                    "unit-usdm-live16-usdm-0003" => 984,
                    "unit-usdm-live16-usdm-0004" => 985,
                    "unit-usdm-live16-usdm-0005" => 986,
                    "unit-usdm-live16-usdm-0006" => 987,
                    "unit-usdm-live16-usdm-0007" => 988,
                    "unit-usdm-live16-usdm-0008" => 989,
                    "unit-usdm-live16-usdm-0009" => 990,
                    "unit-usdm-live16-usdm-0010" => 991,
                    "unit-usdm-live16-usdm-0011" => 992,
                    "unit-usdm-live16-usdm-0012" => 993,
                    "unit-usdm-live16-usdm-0013" => 994,
                    "unit-usdm-live16-usdm-0014" => 995,
                    other => panic!("unexpected placed client order id {other}"),
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
        frames,
    )
    .await
    .unwrap();

    assert_eq!(report.twelfth_continued_steps, 1);
    assert_eq!(report.thirteenth_continued_steps, 1);
    assert_eq!(report.fourteenth_continued_steps, 0);
    assert_eq!(report.placed_orders, 15);
    assert_eq!(report.reconciliations.len(), 15);
    assert_eq!(
        report.reconciliations[14].client_order_id,
        "unit-usdm-live16-usdm-0014"
    );
    assert_eq!(report.reconciliations[14].side, "BUY");
    assert!(report.reconciliations[14].terminal);
    assert_eq!(
        report.extra_symbol_inventory,
        vec![PolicyDemoSymbolInventory {
            symbol: "ETHUSDT".into(),
            position: 1,
        }]
    );
}
