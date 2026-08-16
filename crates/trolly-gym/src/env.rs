//! Stream-fed training environment stepping on observations and dispatching actions.

use trolly_strategy::{OutboundMessage, StreamEgress, StreamEvent};
use trolly_stream::Message;

use crate::action::Action;
use crate::observation::{features_from_event, ObservationWindow};
use crate::policy::PolicyProvider;
use crate::replay::ReplayBuffer;

/// Result of one environment step.
#[derive(Debug, Clone, PartialEq)]
pub struct StepResult {
    pub observation: Vec<f32>,
    pub reward: f32,
    pub done: bool,
}

/// Configuration for [`Env`] observation windows and replay capacity.
#[derive(Debug, Clone)]
pub struct EnvConfig {
    pub symbol: String,
    pub window_frames: usize,
    pub replay_capacity: usize,
    pub default_qty: String,
    pub episode_steps: u64,
    pub reward: RewardConfig,
}

impl EnvConfig {
    pub fn new(symbol: impl Into<String>) -> Self {
        Self {
            symbol: symbol.into(),
            window_frames: 8,
            replay_capacity: 256,
            default_qty: "0.01".into(),
            episode_steps: 128,
            reward: RewardConfig::default(),
        }
    }
}

/// Market reward configuration.
///
/// Reward is `inventory * Δmid - spread_cost` where inventory is the unit
/// position after the action (`-1`, `0`, or `1`) and spread cost is charged
/// only when the action changes inventory.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RewardConfig {
    pub spread_cost_multiplier: f32,
}

impl Default for RewardConfig {
    fn default() -> Self {
        Self {
            spread_cost_multiplier: 1.0,
        }
    }
}

/// Input accepted by [`Env::step`].
pub trait StepActionSource {
    fn select_action(&self, observation: &[f32]) -> Action;
}

impl StepActionSource for Action {
    fn select_action(&self, _observation: &[f32]) -> Action {
        *self
    }
}

impl<P> StepActionSource for &P
where
    P: PolicyProvider + ?Sized,
{
    fn select_action(&self, observation: &[f32]) -> Action {
        (*self).act(observation)
    }
}

/// Training gym environment fed by normalized stream events.
///
/// Ingest stream updates to grow the observation window; call [`Env::step`] to
/// choose an action, record a transition, and dispatch via strategy egress.
pub struct Env<E>
where
    E: StreamEgress,
{
    config: EnvConfig,
    window: ObservationWindow,
    replay: ReplayBuffer,
    egress: E,
    last_observation: Vec<f32>,
    steps: u64,
    reward_state: RewardState,
}

#[derive(Debug, Clone, Copy, Default)]
struct RewardState {
    position: i8,
    last_mid: Option<f32>,
}

impl<E> Env<E>
where
    E: StreamEgress,
{
    pub fn new(config: EnvConfig, egress: E) -> Self {
        let window_frames = config.window_frames;
        let replay_capacity = config.replay_capacity;
        Self {
            config,
            window: ObservationWindow::new(window_frames),
            replay: ReplayBuffer::new(replay_capacity),
            egress,
            last_observation: Vec::new(),
            steps: 0,
            reward_state: RewardState::default(),
        }
    }

    pub fn symbol(&self) -> &str {
        &self.config.symbol
    }

    pub fn observation_window(&self) -> &ObservationWindow {
        &self.window
    }

    pub fn replay_buffer(&self) -> &ReplayBuffer {
        &self.replay
    }

    pub fn replay_buffer_mut(&mut self) -> &mut ReplayBuffer {
        &mut self.replay
    }

    pub fn egress(&self) -> &E {
        &self.egress
    }

    pub fn egress_mut(&mut self) -> &mut E {
        &mut self.egress
    }

    pub fn steps(&self) -> u64 {
        self.steps
    }

    pub fn position(&self) -> i8 {
        self.reward_state.position
    }

    /// Consume one normalized stream event and update the observation window.
    pub fn ingest_event(&mut self, event: &StreamEvent) -> bool {
        if event.routing_id() != self.config.symbol {
            return false;
        }
        if let Some(frame) = features_from_event(event) {
            self.window.push(frame);
            self.last_observation = self.window.flattened();
            self.replay.push_observation_window(&self.last_observation);
            crate::ticks::try_ingest_event(event, "stream", "stream");
            true
        } else {
            false
        }
    }

    /// Ingest a websocket text envelope (stream ingress hook).
    pub fn ingest_message(&mut self, msg: Message) -> Result<bool, trolly_strategy::ParseError> {
        let event = trolly_strategy::parse_envelope(msg)?;
        Ok(self.ingest_event(&event))
    }

    /// Choose/apply an action, dispatch egress, record transition, return step result.
    ///
    /// Pass either an explicit [`Action`] or a borrowed [`PolicyProvider`]:
    /// `env.step(Action::Buy)` and `env.step(&policy)` both use
    /// [`Action::dispatch`] as the only egress path.
    pub fn step<S>(&mut self, source: S) -> Result<StepResult, E::Error>
    where
        S: StepActionSource,
    {
        let observation = if self.last_observation.is_empty() {
            self.window.flattened()
        } else {
            self.last_observation.clone()
        };
        let action = source.select_action(&observation);
        let reward = self.market_reward(&observation, action);
        let done = self.steps + 1 >= self.config.episode_steps.max(1);

        action.dispatch(
            &mut self.egress,
            &self.config.symbol,
            &self.config.default_qty,
            None,
        )?;

        self.replay
            .push_step(observation.clone(), action, reward, done);
        self.steps += 1;

        Ok(StepResult {
            observation,
            reward,
            done,
        })
    }

    /// Dispatch a pre-built outbound message (escape hatch for custom policies).
    pub fn dispatch(&mut self, message: OutboundMessage) -> Result<(), E::Error> {
        self.egress.dispatch(message)
    }

    fn market_reward(&mut self, observation: &[f32], action: Action) -> f32 {
        let previous_position = self.reward_state.position;
        let next_position = action.target_position(previous_position);
        let snapshot = MarketSnapshot::from_observation(observation);
        let (delta_mid, spread) = match snapshot {
            Some(snapshot) => {
                let delta = self
                    .reward_state
                    .last_mid
                    .map(|last_mid| snapshot.mid - last_mid)
                    .unwrap_or(0.0);
                self.reward_state.last_mid = Some(snapshot.mid);
                (delta, snapshot.spread)
            }
            None => (0.0, 0.0),
        };
        let spread_cost = if next_position != previous_position {
            spread * self.config.reward.spread_cost_multiplier
        } else {
            0.0
        };
        self.reward_state.position = next_position;
        next_position as f32 * delta_mid - spread_cost
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct MarketSnapshot {
    mid: f32,
    spread: f32,
}

impl MarketSnapshot {
    fn from_observation(observation: &[f32]) -> Option<Self> {
        if observation.len() < 7 {
            return None;
        }
        let frame = &observation[observation.len() - 7..];
        Some(Self {
            spread: frame[4],
            mid: frame[5],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use trolly_strategy::{DepthUpdate, PriceLevel, RecordingEgress};

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
    fn ingest_filters_by_symbol() {
        let mut env = Env::new(EnvConfig::new("BTCUSDT"), RecordingEgress::default());
        assert!(!env.ingest_event(&depth_event("ETHUSDT", "100", "101")));
        assert!(env.ingest_event(&depth_event("BTCUSDT", "100", "102")));
        assert_eq!(env.observation_window().len(), 1);
    }

    #[test]
    fn step_dispatches_buy_order() {
        let mut env = Env::new(EnvConfig::new("BTCUSDT"), RecordingEgress::default());
        env.ingest_event(&depth_event("BTCUSDT", "100", "102"));

        let result = env.step(Action::Buy).unwrap();
        assert!(!result.observation.is_empty());
        assert_eq!(result.reward, -2.0);
        assert_eq!(env.position(), 1);
        assert_eq!(env.replay_buffer().len(), 2); // ingest prefill + step
        assert_eq!(
            env.egress().dispatched[0],
            Action::Buy.to_outbound("BTCUSDT", "0.01", None)
        );
    }

    #[test]
    fn step_accepts_policy_provider_and_marks_done_at_horizon() {
        let mut config = EnvConfig::new("BTCUSDT");
        config.episode_steps = 2;
        config.reward.spread_cost_multiplier = 0.5;
        let mut env = Env::new(config, RecordingEgress::default());

        env.ingest_event(&depth_event("BTCUSDT", "100", "102"));
        let buy_policy = |_obs: &[f32]| Action::Buy;
        let first = env.step(&buy_policy).unwrap();
        assert!(!first.done);
        assert_eq!(first.reward, -1.0);

        env.ingest_event(&depth_event("BTCUSDT", "101", "103"));
        let hold_policy = |_obs: &[f32]| Action::Hold;
        let second = env.step(&hold_policy).unwrap();
        assert!(second.done);
        assert_eq!(second.reward, 1.0);
        assert_eq!(
            env.egress().dispatched,
            vec![
                Action::Buy.to_outbound("BTCUSDT", "0.01", None),
                Action::Hold.to_outbound("BTCUSDT", "0.01", None),
            ]
        );
    }
}
