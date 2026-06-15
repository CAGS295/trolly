//! Offline smoke test: mock stream observations through env step and egress.

use trolly_gym::{Action, Env, EnvConfig};
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
            position_side: None,
        }
    );

    // Stream envelope ingress hook
    let mut env2 = Env::new(EnvConfig::new("ETHUSDT"), RecordingEgress::default());
    env2
        .ingest_message(envelope_message(&StreamEvent::Depth(DepthUpdate {
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

    assert!(!trolly_gym::torch_enabled());
}
