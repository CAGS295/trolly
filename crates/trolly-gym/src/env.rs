//! Stream-fed training environment stepping on observations and dispatching actions.

use trolly_strategy::{OutboundMessage, StreamEgress, StreamEvent};
use trolly_stream::Message;

use crate::action::Action;
use crate::observation::{features_from_event, ObservationWindow};
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
}

impl EnvConfig {
    pub fn new(symbol: impl Into<String>) -> Self {
        Self {
            symbol: symbol.into(),
            window_frames: 8,
            replay_capacity: 256,
            default_qty: "0.01".into(),
        }
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

    /// Consume one normalized stream event and update the observation window.
    pub fn ingest_event(&mut self, event: &StreamEvent) -> bool {
        if event.routing_id() != self.config.symbol {
            return false;
        }
        if let Some(frame) = features_from_event(event) {
            self.window.push(frame);
            self.last_observation = self.window.flattened();
            self.replay
                .push_observation_window(&self.last_observation);
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

    /// Apply an action, dispatch egress, record transition, return step result.
    pub fn step(&mut self, action: Action) -> Result<StepResult, E::Error> {
        let observation = if self.last_observation.is_empty() {
            self.window.flattened()
        } else {
            self.last_observation.clone()
        };
        let reward = reward_stub(&observation, action);
        let done = false;

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
}

fn reward_stub(observation: &[f32], action: Action) -> f32 {
    let mid = observation.get(5).copied().unwrap_or(0.0);
    match action {
        Action::Hold => 0.0,
        Action::Buy => mid * 0.001,
        Action::Sell => -mid * 0.001,
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
        assert_eq!(env.replay_buffer().len(), 2); // ingest prefill + step
        assert_eq!(
            env.egress().dispatched[0],
            Action::Buy.to_outbound("BTCUSDT", "0.01", None)
        );
    }
}
