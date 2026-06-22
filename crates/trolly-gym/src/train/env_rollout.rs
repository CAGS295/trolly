//! Stream [`Env`] rollout collection without live Binance streams.

use tch::{Kind, Tensor};

use crate::action::Action;
use crate::env::Env;
use crate::ppo::ActorCritic;
use crate::replay::{RolloutCollector, RolloutStep};
use trolly_strategy::StreamEgress;

/// Sample an action from `actor_critic`, step the environment, and append to `collector`.
///
/// Returns `Ok(false)` when the environment has no observation yet (window empty).
pub fn collect_env_rollout_step<E>(
    env: &mut Env<E>,
    actor_critic: &ActorCritic,
    collector: &mut RolloutCollector,
) -> Result<bool, E::Error>
where
    E: StreamEgress,
{
    let observation = env.current_observation();
    if observation.is_empty() {
        return Ok(false);
    }

    let obs_tensor = Tensor::from_slice(&observation)
        .view([1, observation.len() as i64])
        .to_kind(Kind::Float);
    let (actions, log_probs, values) = actor_critic.act(&obs_tensor);

    let action_idx = actions.int64_value(&[0]);
    let action = Action::from_index(action_idx).unwrap_or(Action::Hold);
    let log_prob = log_probs.double_value(&[0]) as f32;
    let value = values.double_value(&[0]) as f32;

    let step = env.step(action)?;
    collector.push(RolloutStep {
        observation,
        action,
        log_prob,
        value,
        reward: step.reward,
        done: step.done,
    });

    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::{Env, EnvConfig};
    use crate::ppo::PpoConfig;
    use tch::nn;
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
    fn collect_env_rollout_step_records_log_prob_and_value() {
        let mut env = Env::new(
            EnvConfig::new("BTCUSDT").with_window_frames(1),
            RecordingEgress::default(),
        );
        assert!(env.ingest_event(&depth_event("BTCUSDT", "100", "102")));

        let obs_dim = env.current_observation().len() as i64;
        let config = PpoConfig::default()
            .with_obs_dim(obs_dim)
            .with_num_actions(Action::COUNT);
        let vs = nn::VarStore::new(tch::Device::Cpu);
        let actor_critic = ActorCritic::new(vs.root(), &config);

        let mut collector = RolloutCollector::with_capacity(1);
        assert!(collect_env_rollout_step(&mut env, &actor_critic, &mut collector).unwrap());
        assert_eq!(collector.len(), 1);
        assert_eq!(collector.steps()[0].observation.len(), obs_dim as usize);
        assert!(collector.steps()[0].log_prob.is_finite());
        assert!(collector.steps()[0].value.is_finite());
    }
}
