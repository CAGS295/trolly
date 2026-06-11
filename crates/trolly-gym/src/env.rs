//! Stream-fed RL-style environment stepping with command egress dispatch.

use trolly_strategy::{Command, CommandEgress, StreamEvent};

use crate::buffer::ReplayBuffer;
use crate::observation::event_to_features;

/// Discrete gym action mapped to a [`Command`] on the strategy egress path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GymAction {
    SendMessage(String),
    Subscribe { symbol: String, channel: String },
    NoOp,
}

impl GymAction {
    pub fn into_command(self) -> Option<Command> {
        match self {
            Self::SendMessage(text) => Some(Command::send_message(text)),
            Self::Subscribe { symbol, channel } => Some(Command::subscribe(symbol, channel)),
            Self::NoOp => None,
        }
    }
}

/// Result of a single environment step.
#[derive(Debug, Clone, PartialEq)]
pub struct StepResult {
    pub observation: Vec<f64>,
    pub action_index: usize,
    pub dispatched: bool,
}

/// Environment consuming stream events and dispatching actions through egress.
#[derive(Debug)]
pub struct Env {
    buffer: ReplayBuffer,
    egress: CommandEgress,
    window_len: usize,
}

impl Env {
    pub fn new(egress: CommandEgress, window_len: usize) -> Self {
        Self {
            buffer: ReplayBuffer::new(window_len),
            egress,
            window_len: window_len.max(1),
        }
    }

    pub fn window_len(&self) -> usize {
        self.window_len
    }

    pub fn replay_buffer(&self) -> &ReplayBuffer {
        &self.buffer
    }

    /// Ingest a stream event, append derived features, and return the new row.
    pub fn push_event(&mut self, event: StreamEvent) -> Vec<f64> {
        let features = event_to_features(&event);
        self.buffer.push_observation(features.clone());
        features
    }

    /// Current flattened observation window over recent stream events.
    pub fn observation(&self) -> Vec<f64> {
        self.buffer.window(self.window_len)
    }

    /// Apply an action, dispatch through egress when non-noop, and record the transition.
    pub fn step(&mut self, action_index: usize, action: GymAction) -> StepResult {
        let observation = self.observation();
        let dispatched = if let Some(command) = action.into_command() {
            self.egress.dispatch(command).is_ok()
        } else {
            false
        };
        self.buffer
            .record_transition(observation.clone(), action_index);
        StepResult {
            observation,
            action_index,
            dispatched,
        }
    }
}
