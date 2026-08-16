//! Offline smoke test: mock stream observations through env step and egress.

use std::cell::Cell;

use trolly_gym::{
    run_offline_policy_harness, Action, CheckpointOrHoldPolicy, Env, EnvConfig, PolicyProvider,
};
use trolly_strategy::{
    envelope_message, DepthUpdate, OutboundMessage, PriceLevel, RecordingEgress, StreamEvent,
};

#[test]
fn mock_stream_observations_env_step_and_replay() {
    let mut env = Env::new(EnvConfig::new("BTCUSDT"), RecordingEgress::default());

    let events = [
        StreamEvent::Depth(DepthUpdate {
            symbol: "BTCUSDT".into(),
            bids: vec![PriceLevel {
                price: "99".into(),
                qty: "2".into(),
            }],
            asks: vec![PriceLevel {
                price: "101".into(),
                qty: "1".into(),
            }],
            update_id: Some(1),
        }),
        StreamEvent::Depth(DepthUpdate {
            symbol: "BTCUSDT".into(),
            bids: vec![PriceLevel {
                price: "100".into(),
                qty: "3".into(),
            }],
            asks: vec![PriceLevel {
                price: "102".into(),
                qty: "2".into(),
            }],
            update_id: Some(2),
        }),
    ];

    for event in &events {
        assert!(env.ingest_event(event));
    }

    assert_eq!(env.observation_window().len(), 2);
    let flat = env.observation_window().flattened();
    assert_eq!(flat.len(), 14); // 7 features × 2 frames

    let step = env.step(Action::Sell).unwrap();
    assert_eq!(step.observation, flat);
    assert!(step.reward < 0.0);

    assert!(env.replay_buffer().len() >= 2);

    // Egress dispatch via env's internal recorder
    let dispatched = &env.egress().dispatched;
    assert_eq!(dispatched.len(), 1);
    assert_eq!(
        dispatched[0],
        OutboundMessage::OrderRequest {
            symbol: "BTCUSDT".into(),
            side: "SELL".into(),
            qty: "0.01".into(),
            price: None,
            time_in_force: None,
            position_side: None,
        }
    );

    // Stream envelope ingress hook
    let mut env2 = Env::new(EnvConfig::new("ETHUSDT"), RecordingEgress::default());
    env2.ingest_message(envelope_message(&StreamEvent::Depth(DepthUpdate {
        symbol: "ETHUSDT".into(),
        bids: vec![PriceLevel {
            price: "3000".into(),
            qty: "1".into(),
        }],
        asks: vec![],
        update_id: None,
    })))
    .unwrap();
    assert_eq!(env2.observation_window().len(), 1);

    // torch_enabled() reflects the `torch` feature at compile time
    #[cfg(feature = "torch")]
    assert!(trolly_gym::torch_enabled());
    #[cfg(not(feature = "torch"))]
    assert!(!trolly_gym::torch_enabled());
}

#[test]
fn policy_provider_steps_dispatch_through_action_path() {
    struct SequencePolicy {
        actions: Vec<Action>,
        next: Cell<usize>,
    }

    impl PolicyProvider for SequencePolicy {
        fn act(&self, _obs: &[f32]) -> Action {
            let idx = self.next.get();
            self.next.set(idx + 1);
            self.actions[idx]
        }
    }

    let policy = SequencePolicy {
        actions: vec![Action::Buy, Action::Sell, Action::Hold],
        next: Cell::new(0),
    };
    let mut env = Env::new(EnvConfig::new("BTCUSDT"), RecordingEgress::default());

    for (update_id, (bid, ask)) in [(1, ("99", "101")), (2, ("100", "102")), (3, ("101", "103"))] {
        assert!(env.ingest_event(&StreamEvent::Depth(DepthUpdate {
            symbol: "BTCUSDT".into(),
            bids: vec![PriceLevel {
                price: bid.into(),
                qty: "1".into(),
            }],
            asks: vec![PriceLevel {
                price: ask.into(),
                qty: "1".into(),
            }],
            update_id: Some(update_id),
        })));
        env.step(&policy).unwrap();
    }

    assert_eq!(
        env.egress().dispatched,
        vec![
            Action::Buy.to_outbound("BTCUSDT", "0.01", None),
            Action::Sell.to_outbound("BTCUSDT", "0.01", None),
            Action::Hold.to_outbound("BTCUSDT", "0.01", None),
        ]
    );
}

#[test]
fn offline_harness_emits_normalized_order_requests() {
    struct SequencePolicy {
        next: Cell<usize>,
    }

    impl PolicyProvider for SequencePolicy {
        fn act(&self, obs: &[f32]) -> Action {
            assert_eq!(obs.len(), 7);
            let idx = self.next.get();
            self.next.set(idx + 1);
            if idx == 0 {
                Action::Buy
            } else {
                Action::Sell
            }
        }
    }

    let mut config = EnvConfig::new("BTCUSDT");
    config.window_frames = 1;
    config.default_qty = "0.02".into();
    let mut env = Env::new(config, RecordingEgress::default());
    let policy = SequencePolicy { next: Cell::new(0) };

    let messages = [
        envelope_message(&StreamEvent::Depth(DepthUpdate {
            symbol: "BTCUSDT".into(),
            bids: vec![PriceLevel {
                price: "100".into(),
                qty: "2".into(),
            }],
            asks: vec![PriceLevel {
                price: "101".into(),
                qty: "1".into(),
            }],
            update_id: Some(1),
        })),
        envelope_message(&StreamEvent::Depth(DepthUpdate {
            symbol: "BTCUSDT".into(),
            bids: vec![PriceLevel {
                price: "101".into(),
                qty: "2".into(),
            }],
            asks: vec![PriceLevel {
                price: "102".into(),
                qty: "1".into(),
            }],
            update_id: Some(2),
        })),
    ];

    let steps = run_offline_policy_harness(&mut env, &policy, messages).unwrap();

    assert_eq!(steps.len(), 2);
    assert_eq!(
        env.egress().dispatched,
        vec![
            OutboundMessage::OrderRequest {
                symbol: "BTCUSDT".into(),
                side: "BUY".into(),
                qty: "0.02".into(),
                price: None,
                time_in_force: None,
                position_side: None,
            },
            OutboundMessage::OrderRequest {
                symbol: "BTCUSDT".into(),
                side: "SELL".into(),
                qty: "0.02".into(),
                price: None,
                time_in_force: None,
                position_side: None,
            },
        ]
    );
}

#[test]
fn checkpoint_or_hold_harness_uses_safe_hold_without_torch() {
    let mut config = EnvConfig::new("BTCUSDT");
    config.window_frames = 1;
    let mut env = Env::new(config, RecordingEgress::default());
    let policy = CheckpointOrHoldPolicy::hold();
    let messages = [envelope_message(&StreamEvent::Depth(DepthUpdate {
        symbol: "BTCUSDT".into(),
        bids: vec![PriceLevel {
            price: "100".into(),
            qty: "1".into(),
        }],
        asks: vec![PriceLevel {
            price: "101".into(),
            qty: "1".into(),
        }],
        update_id: Some(1),
    }))];

    let steps = run_offline_policy_harness(&mut env, &policy, messages).unwrap();

    assert_eq!(steps.len(), 1);
    assert_eq!(
        env.egress().dispatched,
        vec![Action::Hold.to_outbound("BTCUSDT", "0.01", None)]
    );
}
