//! Collect on-policy rollouts from stream-fed [`Env`](crate::Env) without live Binance.

use trolly_strategy::StreamEgress;
use trolly_strategy::StreamEvent;

use crate::env::Env;
use crate::ppo::WolfPpoTrainer;
use crate::replay::OnPolicyRolloutBuffer;
use crate::train::rollout::RolloutCollector;

/// Options for env-driven rollout collection.
#[derive(Debug, Clone)]
pub struct EnvRolloutConfig {
    /// Number of policy steps after events are ingested.
    pub steps: usize,
    /// When true, ingest one event before each env step (cycling the slice).
    pub interleave_events: bool,
}

impl Default for EnvRolloutConfig {
    fn default() -> Self {
        Self {
            steps: 4,
            interleave_events: true,
        }
    }
}

/// Feed depth events through [`Env::ingest_event`], sample actions from the
/// trainer policy, and record on-policy transitions via [`Env::step`].
pub fn collect_env_rollout<E: StreamEgress>(
    trainer: &WolfPpoTrainer,
    env: &mut Env<E>,
    events: &[StreamEvent],
    config: &EnvRolloutConfig,
) -> OnPolicyRolloutBuffer {
    let mut collector = RolloutCollector::with_capacity(config.steps);
    let ac = trainer.actor_critic();

    for step_idx in 0..config.steps {
        if config.interleave_events && !events.is_empty() {
            let event = &events[step_idx % events.len()];
            env.ingest_event(event);
        }

        let observation = current_observation(env);
        if observation.is_empty() {
            continue;
        }

        let (action, log_prob, value) = RolloutCollector::sample_action(ac, &observation);
        let step = match env.step(action) {
            Ok(step) => step,
            Err(_) => continue,
        };
        collector.record_step(
            observation,
            action,
            log_prob,
            value,
            step.reward,
            step.done,
        );
    }

    if !config.interleave_events {
        for event in events {
            env.ingest_event(event);
        }
    }

    collector.buffer().clone()
}

fn current_observation<E: StreamEgress>(env: &Env<E>) -> Vec<f32> {
    env.observation_window().flattened()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::{Env, EnvConfig};
    use crate::ppo::WolfPpoConfig;
    use trolly_strategy::{DepthUpdate, PriceLevel, RecordingEgress, StreamEvent};

    fn depth(symbol: &str, bid: &str, ask: &str, id: u64) -> StreamEvent {
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
            update_id: Some(id),
        })
    }

    #[test]
    fn collect_env_rollout_from_mock_stream() {
        tch::manual_seed(11);
        let mut config = EnvConfig::new("BTCUSDT");
        config.window_frames = 1;
        let mut env = Env::new(config, RecordingEgress::default());
        let trainer = WolfPpoTrainer::new(7, 3, WolfPpoConfig::default());

        let events = [
            depth("BTCUSDT", "100", "102", 1),
            depth("BTCUSDT", "101", "103", 2),
        ];

        let rollout = collect_env_rollout(
            &trainer,
            &mut env,
            &events,
            &EnvRolloutConfig {
                steps: 3,
                interleave_events: true,
            },
        );

        assert_eq!(rollout.len(), 3);
        assert_eq!(rollout.steps()[0].observation.len(), 7);
        assert!(rollout.steps()[0].log_prob <= 0.0);
        assert!(env.steps() >= 3);
    }
}
