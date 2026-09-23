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
    run_spot_policy_demo_with_placer, run_spot_policy_demo_with_placer_and_user_data,
    run_usdm_policy_demo_with_placer, run_usdm_policy_demo_with_placer_and_user_data, DemoVenue,
    PolicyDemoConfig, PolicyDemoError, PolicyDemoOrders, PolicyDemoSymbolInventory,
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
