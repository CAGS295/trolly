//! Collect on-policy rollouts from [`Env`] ingest/step without live streams.

use crate::env::Env;
use crate::libtorch::observation_tensor;
use crate::ppo::ActorCritic;
use crate::replay::{action_from_index, OnPolicyRolloutBuffer};
use trolly_strategy::{StreamEgress, StreamEvent};

/// Ingest stream events and step the env with actions sampled from `model`.
///
/// Each ingested event updates the observation window; when the window is
/// non-empty the policy acts, [`Env::step`] records the transition (reward
/// stub is fine), and an on-policy step is appended to the buffer.
pub fn collect_env_rollout<E>(
    env: &mut Env<E>,
    model: &ActorCritic,
    events: &[StreamEvent],
) -> OnPolicyRolloutBuffer
where
    E: StreamEgress,
{
    let mut buffer = OnPolicyRolloutBuffer::with_capacity(events.len());

    for event in events {
        if !env.ingest_event(event) {
            continue;
        }
        let observation = env
            .observation_window()
            .flattened();
        if observation.is_empty() {
            continue;
        }

        let (action_idx, log_prob, value) = sample_action(model, &observation);
        let action = action_from_index(action_idx);
        let step = env
            .step(action)
            .unwrap_or_else(|_| panic!("env step must succeed in rollout collection"));

        buffer.push_env_step(
            observation,
            action,
            log_prob,
            value,
            step.reward as f64,
            step.done,
        );
    }

    buffer
}

fn sample_action(model: &ActorCritic, observation: &[f32]) -> (i64, f64, f64) {
    tch::no_grad(|| {
        let obs = observation_tensor(observation).unsqueeze(0);
        let (actions, log_probs, values, _) = model.act(&obs);
        (
            actions.int64_value(&[0]),
            log_probs.double_value(&[0]),
            values.double_value(&[0]),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EnvConfig;
    use tch::{nn, Device};
    use trolly_strategy::{DepthUpdate, PriceLevel, RecordingEgress, StreamEvent};

    fn depth_event(symbol: &str, bid: &str, ask: &str) -> StreamEvent {
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

    #[test]
    fn collect_env_rollout_from_mock_events() {
        let mut config = EnvConfig::new("BTCUSDT");
        config.window_frames = 1;
        let mut env = Env::new(config, RecordingEgress::default());
        let events = [
            depth_event("BTCUSDT", "100", "102"),
            depth_event("BTCUSDT", "101", "103"),
        ];

        let vs = nn::VarStore::new(Device::Cpu);
        let obs_dim = 7_i64;
        let model = ActorCritic::new(
            &vs.root(),
            obs_dim,
            3,
            &crate::ppo::default_hidden_layers(),
            &crate::ppo::PpoConfig::default(),
        );

        let buffer = collect_env_rollout(&mut env, &model, &events);
        assert_eq!(buffer.len(), 2);
        assert!(buffer.steps()[0].log_prob.is_finite());
        assert!(buffer.steps()[0].value.is_finite());
    }
}
